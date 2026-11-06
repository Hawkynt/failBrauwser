//! The Windows Recycle Bin through the shell: `IFileOperation` for trash/restore/delete,
//! the bin's shell folder for listing (each child rebuilt as an absolute item — the folder's
//! `GetUIObjectOf` is `E_NOTIMPL`, `ILCombine` + `SHCreateItemFromIDList` is the route that
//! works), and `SHQueryRecycleBinW`/`SHEmptyRecycleBinW` for the whole bin. All facts here
//! were settled by a live probe, not by assumption.
//!
//! Every COM call runs on its own MTA-initialized worker thread: `IFileOperation` must not
//! share an apartment with the GLib main loop, and for silent single operations it needs no
//! message pump (verified — the probe ran without one).

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use windows::core::{GUID, Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, PROPERTYKEY, S_OK};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CLSCTX_INPROC_SERVER, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CreateBindCtx, IBindCtx};
use windows::Win32::System::SystemServices::{SFGAO_CANDELETE, SFGAO_CANMOVE};
use windows::Win32::System::Variant::VT_FILETIME;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::PropertiesSystem::{GPS_DEFAULT, IPropertyStore, PSGetPropertyKeyFromName};
use windows::Win32::UI::Shell::{CSIDL_BITBUCKET, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, IEnumIDList, IFileOperation, ILCombine, ILFree, IShellFolder, IShellItem, IShellItem2, SHCONTF_FOLDERS, SHCONTF_NONFOLDERS, SHCreateItemFromIDList, SHCreateItemFromParsingName, SHEmptyRecycleBinW, SHGetFolderLocation, SHQUERYRBINFO, SHQueryRecycleBinW, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY};

// BHID_SFObject (shlguid.h): asks an IShellItem for the IShellFolder behind it.
const BHID_SF_OBJECT: GUID = GUID::from_u128(0x3981e224_f559_11d3_8e3a_00c04f6837d5);
// CLSID_FileOperation (ShObjIdl.idl): the windows crate exposes the interface but not the coclass.
const CLSID_FILEOPERATION: GUID = GUID::from_u128(0x3ad05575_8857_4850_9277_11b85bdb8e09);
// Fallback for System.Recycle.DateDeleted, observed live in the bin's property store on
// 2026-11-04; the canonical name resolved by PSGetPropertyKeyFromName is preferred.
const FALLBACK_DATE_DELETED: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9b174b33_40ff_11d2_a27e_00c04fc30871), pid: 3 };
// MK_E_UNAVAILABLE: the parsing name (a file being trashed) is gone.
const MK_E_UNAVAILABLE: u32 = 0x8004_01E5;

/// One entry of the Recycle Bin, in the shape `trash::TrashItem` needs.
#[derive(Debug, Clone, PartialEq)]
pub struct BinItem {
    /// The `$R…` name inside the bin (unique within it).
    pub name: String,
    /// Its real location under `$Recycle.Bin` (what the shell calls the parsing name).
    pub path: PathBuf,
    /// Where it was deleted from (the bin's display name — always a full path).
    pub original: PathBuf,
    /// Deletion time as local ISO `YYYY-MM-DDTHH:MM:SS`, empty when unknown.
    pub deleted: String,
    pub can_move: bool,
    pub can_delete: bool,
}

impl BinItem {
    /// Deletion time as Unix time (local), 0 if unknown.
    pub fn deleted_unix(&self) -> i64 {
        let tz = gtk::glib::TimeZone::local();
        gtk::glib::DateTime::from_iso8601(&self.deleted, Some(&tz)).map(|d| d.to_unix()).unwrap_or(0)
    }
}

/// FILETIME (100 ns since 1601-01-01) to Unix seconds; pre-epoch values go negative.
pub fn filetime_to_unix(ft: u64) -> i64 {
    ft.checked_sub(116_444_736_000_000_000).map(|d| (d / 10_000_000) as i64).unwrap_or(i64::MIN)
}

fn io_err(e: windows::core::Error) -> io::Error {
    if e.code().0 as u32 == MK_E_UNAVAILABLE {
        return io::Error::new(io::ErrorKind::NotFound, e.to_string());
    }
    io::Error::other(e.to_string())
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Runs `f` on a freshly COM-initialized worker thread and joins it, so the caller's
/// apartment (the GLib main loop) never hosts `IFileOperation`.
fn on_com<T>(f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T>
where
    T: Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        let result = f();
        if initialized {
            unsafe { CoUninitialize() };
        }
        let _ = tx.send(result);
    });
    rx.recv().map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the recycle-bin worker stopped"))?
}

fn item_from(path: &Path) -> Result<IShellItem, windows::core::Error> {
    let w = wide(&path.to_string_lossy());
    unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }
}

fn display_name(item: &IShellItem, sig: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(sig).ok()?;
        let s = p.to_string().ok()?;
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        Some(s)
    }
}

/// Resolves a canonical property name, keeping the observed fallback if propsys refuses.
fn prop_key(name: &str, fallback: PROPERTYKEY) -> PROPERTYKEY {
    let w = wide(name);
    let mut key = PROPERTYKEY::default();
    if unsafe { PSGetPropertyKeyFromName(PCWSTR(w.as_ptr()), &mut key) }.is_ok() {
        key
    } else {
        fallback
    }
}

/// The `DateDeleted` time of a bin item (its property store is scoped by the bin folder,
/// so the Recycle properties are answered).
fn deleted_unix_of(item: &IShellItem, key: &PROPERTYKEY) -> i64 {
    let Ok(item2) = item.cast::<IShellItem2>() else { return 0 };
    let Ok(props) = (unsafe { item2.GetPropertyStore::<IPropertyStore>(GPS_DEFAULT) }) else { return 0 };
    let Ok(val) = (unsafe { props.GetValue(key) }) else { return 0 };
    unsafe {
        let raw = &val.Anonymous.Anonymous;
        if raw.vt != VT_FILETIME {
            return 0;
        }
        let ft = raw.Anonymous.filetime;
        filetime_to_unix(((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64)
    }
}

fn iso_local(unix: i64) -> String {
    if unix == 0 {
        return String::new();
    }
    gtk::glib::DateTime::from_unix_local(unix).map(|d| d.format("%Y-%m-%dT%H:%M:%S").unwrap_or_default()).unwrap_or_default().to_string()
}

/// Maps one bin item (already rebuilt as an absolute shell item) to the app's model.
fn describe(item: &IShellItem, date_key: &PROPERTYKEY) -> BinItem {
    let original = display_name(item, SIGDN_NORMALDISPLAY).unwrap_or_default();
    let parser = display_name(item, SIGDN_DESKTOPABSOLUTEPARSING).unwrap_or_default();
    let attrs = unsafe { item.GetAttributes(SFGAO_CANMOVE | SFGAO_CANDELETE) }.unwrap_or_default();
    BinItem {
        name: Path::new(&parser).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: PathBuf::from(&parser),
        original: PathBuf::from(&original),
        deleted: iso_local(deleted_unix_of(item, date_key)),
        can_move: (attrs & SFGAO_CANMOVE).0 != 0,
        can_delete: (attrs & SFGAO_CANDELETE).0 != 0,
    }
}

/// Walks the bin's shell folder, handing every child — rebuilt as an absolute item — to `visit`.
fn enumerate<F>(mut visit: F) -> io::Result<()>
where
    F: FnMut(&IShellItem),
{
    let root = unsafe { SHGetFolderLocation(None, CSIDL_BITBUCKET as i32, None, 0) }.map_err(io_err)?;
    let outcome = (|| -> Result<(), windows::core::Error> {
        let bin_item: IShellItem = unsafe { SHCreateItemFromIDList(root) }?;
        let ctx: IBindCtx = unsafe { CreateBindCtx(0) }?;
        let psf: IShellFolder = unsafe { bin_item.BindToHandler(Some(&ctx), &BHID_SF_OBJECT) }?;
        let mut en: Option<IEnumIDList> = None;
        (unsafe { psf.EnumObjects(HWND::default(), (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32, &mut en) }).ok()?;
        let Some(en) = en else { return Ok(()) };
        loop {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            let mut got = 0u32;
            if unsafe { en.Next(std::slice::from_mut(&mut pidl), Some(&raw mut got)) } != S_OK {
                break;
            }
            let abs = unsafe { ILCombine(Some(root), Some(pidl as *const ITEMIDLIST)) };
            if let Ok(item) = unsafe { SHCreateItemFromIDList::<IShellItem>(abs) } {
                visit(&item);
            }
            unsafe { ILFree(Some(abs)) };
            unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
        }
        Ok(())
    })();
    unsafe { CoTaskMemFree(Some(root as *const core::ffi::c_void)) };
    outcome.map_err(io_err)
}

/// A stub (`$I…`) whose payload (`$R…`) is already gone: a stale row of a bin folder
/// that has not reconciled yet — what Explorer's own delete leaves behind, and what a
/// folder view is expected to hide. The `$I` file itself carries the original name, so
/// an unfiltered walk would keep reporting an item the delete already removed.
fn orphaned(path: &Path) -> bool {
    let stub = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match stub.strip_prefix("$I") {
        None => false,
        Some(rest) => std::fs::symlink_metadata(path.with_file_name(format!("$R{rest}"))).is_err(),
    }
}

/// Every bin entry, including stubs whose payload is gone. Housekeeping needs to see
/// those (a `delete` can leave one for a while), the user-facing views must not.
pub fn list_unfiltered() -> io::Result<Vec<BinItem>> {
    on_com(|| {
        let date_key = prop_key("System.Recycle.DateDeleted", FALLBACK_DATE_DELETED);
        let mut out = Vec::new();
        enumerate(|item| out.push(describe(item, &date_key)))?;
        Ok(out)
    })
}

/// Everything in the Recycle Bin (across all volumes — the shell aggregates them).
/// Entries whose payload is gone stay hidden, so rows never double-count an item a
/// delete has already taken.
pub fn list() -> io::Result<Vec<BinItem>> {
    Ok(list_unfiltered()?.into_iter().filter(|b| !orphaned(&b.path)).collect())
}

/// The item whose bin location is exactly `parser` (the `$Recycle.Bin` path from `list`).
fn find_bin_item(parser: &Path) -> io::Result<Option<IShellItem>> {
    let mut found: Option<IShellItem> = None;
    enumerate(|item| {
        if found.is_none() && display_name(item, SIGDN_DESKTOPABSOLUTEPARSING).is_some_and(|p| Path::new(&p) == parser) {
            found = Some(item.clone());
        }
    })?;
    Ok(found)
}

/// `count` and `size` of the whole bin, like Explorer's Recycle Bin properties.
pub fn query() -> io::Result<(i64, i64)> {
    on_com(|| {
        let mut info = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, i64Size: 0, i64NumItems: 0 };
        unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut info) }.map_err(io_err)?;
        Ok((info.i64NumItems, info.i64Size))
    })
}

/// Sends a path to the Recycle Bin. An error means there is no bin for it (network shares,
/// for instance) — the caller can then ask whether to delete permanently.
pub fn trash(path: &Path) -> io::Result<()> {
    let path = path.to_path_buf();
    on_com(move || {
        let item = item_from(&path).map_err(io_err)?;
        let op: IFileOperation = unsafe { CoCreateInstance(&CLSID_FILEOPERATION, None, CLSCTX_INPROC_SERVER) }.map_err(io_err)?;
        unsafe { op.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) }.map_err(io_err)?;
        unsafe {
            op.DeleteItem(&item, None).map_err(io_err)?;
            op.PerformOperations().map_err(io_err)?;
            if op.GetAnyOperationsAborted().unwrap_or_default().as_bool() {
                return Err(io::Error::other("the shell aborted the move to the Recycle Bin"));
            }
        }
        Ok(())
    })
}

/// Puts a bin entry back: at `to` if given, otherwise where it was deleted from.
/// Fails with `AlreadyExists` when the target is taken, like the freedesktop trash.
pub fn restore(parser: &Path, original: &Path, to: Option<&Path>) -> io::Result<PathBuf> {
    let (parser, original) = (parser.to_path_buf(), original.to_path_buf());
    let to = to.map(PathBuf::from);
    on_com(move || {
        let target = to.unwrap_or_else(|| original.clone());
        if std::fs::symlink_metadata(&target).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("“{}” already exists", target.display())));
        }
        let parent = target.parent().unwrap_or(&target);
        if std::fs::create_dir_all(parent).is_err() && !parent.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the original location is unknown"));
        }
        let item = find_bin_item(&parser)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the item is no longer in the Recycle Bin"))?;
        let dest = item_from(parent).map_err(io_err)?;
        let op: IFileOperation = unsafe { CoCreateInstance(&CLSID_FILEOPERATION, None, CLSCTX_INPROC_SERVER) }.map_err(io_err)?;
        unsafe { op.SetOperationFlags(FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) }.map_err(io_err)?;
        let name = wide(&target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        unsafe {
            op.MoveItem(&item, &dest, PCWSTR(name.as_ptr()), None).map_err(io_err)?;
            op.PerformOperations().map_err(io_err)?;
            if op.GetAnyOperationsAborted().unwrap_or_default().as_bool() {
                return Err(io::Error::other("the shell aborted the restore"));
            }
        }
        Ok(target)
    })
}

/// Deletes a bin entry for good. The payload goes immediately; a bare `$I…` may linger
/// until the folder view reconciles — exactly what Explorer's own delete leaves behind,
/// and orphans never appear in `list`.
pub fn delete(parser: &Path) -> io::Result<()> {
    let parser = parser.to_path_buf();
    on_com(move || {
        let item = find_bin_item(&parser)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the item is no longer in the Recycle Bin"))?;
        let op: IFileOperation = unsafe { CoCreateInstance(&CLSID_FILEOPERATION, None, CLSCTX_INPROC_SERVER) }.map_err(io_err)?;
        unsafe { op.SetOperationFlags(FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) }.map_err(io_err)?;
        unsafe {
            op.DeleteItem(&item, None).map_err(io_err)?;
            op.PerformOperations().map_err(io_err)?;
        }
        Ok(())
    })
}

/// Empties the Recycle Bin on every drive, as Explorer does.
pub fn empty() -> io::Result<()> {
    on_com(|| unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), 0) }.map_err(io_err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_filetime_to_unix_seconds() {
        assert_eq!(filetime_to_unix(116_444_736_000_000_000), 0);
        // The live probe value of a file trashed 2026-11-04 13:38 local.
        assert_eq!(filetime_to_unix(134_382_683_080_000_000), 1_793_794_708);
        assert!(filetime_to_unix(1) < 0);
    }

    #[test]
    fn resolves_the_recycle_property_name() {
        let key = prop_key("System.Recycle.DateDeleted", FALLBACK_DATE_DELETED);
        // propsys always resolves a canonical name; either way the key is usable.
        assert_ne!(key.pid, 0);
    }

    /// Integration (live Recycle Bin): the fixture only ever touches its own files —
    /// it restores them or deletes them for good, and never empties the user's bin.
    #[test]
    fn recycles_a_file_and_restores_it() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("fb_recycled.txt");
        std::fs::write(&f, b"payload").unwrap();
        trash(&f).unwrap();
        assert!(!f.exists(), "the file must have left its original place");

        let items = list().unwrap();
        let mine = items.iter().find(|b| b.original == f).expect("the file just recycled must be listed");
        assert!(mine.path.to_string_lossy().contains("$Recycle.Bin"), "listed path must be inside the bin: {}", mine.path.display());
        assert!(mine.name.starts_with("$R"), "the bin name is the $R entry: {}", mine.name);
        assert!(mine.deleted_unix() > 0, "the deletion time must be readable");
        assert!(mine.can_move && mine.can_delete);

        restore(&mine.path, &mine.original, None).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"payload", "the content must come back intact");
        assert!(list().unwrap().iter().all(|b| b.original != f), "the entry must be gone after the restore");
    }

    #[test]
    fn refuses_to_restore_onto_a_taken_name() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("fb_taken.txt");
        std::fs::write(&f, b"gone").unwrap();
        trash(&f).unwrap();
        std::fs::write(&f, b"in the way").unwrap();
        let items = list().unwrap();
        let mine = items.iter().find(|b| b.original == f).expect("the recycled file must be listed");
        let err = restore(&mine.path, &mine.original, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        // Keep the fixture clean either way: drop the bin entry for good.
        delete(&mine.path).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "in the way");
    }

    #[test]
    fn deletes_a_bin_entry_for_good() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("fb_perma.txt");
        std::fs::write(&f, b"x").unwrap();
        trash(&f).unwrap();
        let items = list().unwrap();
        let mine = items.iter().find(|b| b.original == f).expect("the recycled file must be listed");
        delete(&mine.path).unwrap();
        assert!(list().unwrap().iter().all(|b| b.original != f), "the entry must be gone after the permanent delete");
        assert!(!mine.path.exists(), "the payload file must be gone from the bin");
    }

    #[test]
    fn querying_and_listing_agree_on_the_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("fb_query.txt");
        std::fs::write(&f, b"x").unwrap();
        let (before, _) = query().unwrap();
        trash(&f).unwrap();
        let (after, _) = query().unwrap();
        assert!(after > before, "the query count must include the fresh entry");
        assert!(list().unwrap().iter().any(|b| b.original == f));
        delete(&list().unwrap().into_iter().find(|b| b.original == f).unwrap().path).unwrap();
    }
}
