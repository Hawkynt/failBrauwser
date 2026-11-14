//! `.lnk` shell links, made the way the shell makes them: an `IShellLinkW` filled in and
//! saved through `IPersistFile`. Windows writes no text file here — the link is a binary
//! record, so it is built on the calling (UI) thread, which already pumps messages.

use std::io;
use std::path::Path;

use windows::core::{GUID, Interface, PCWSTR};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, STGM_READ};
use windows::Win32::UI::Shell::{FOLDERID_RecycleBinFolder, IShellLinkW, SHGetKnownFolderIDList};

/// The class windows creates for a link. windows 0.62 projects no CLSID for it; the value
/// is the SDK's own: ShObjIdl_core.h declares `coclass ShellLink` with DECLSPEC_UUID
/// "00021401-0000-0000-C000-000000000046" (also in ShObjIdl_core.idl:8272).
const SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_C000_000000000046);

/// What a made link opens: a path (file, folder or program; `arguments` for a program),
/// or the Recycle Bin itself, which no path names.
pub enum Target<'a> {
    Path { target: &'a Path, arguments: &'a str },
    RecycleBin,
}

/// Writes `lnk` pointing at `target`; an already existing file at `lnk` is replaced.
/// COM errors keep their message: only the user sees these (the link menus).
pub fn create(lnk: &Path, target: &Target) -> io::Result<()> {
    // Ask for an apartment only if this thread has none; never pair with CoUninitialize —
    // the toolkit may already own COM here, and taking it down would take the app down.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&SHELL_LINK, None, CLSCTX_INPROC_SERVER).map_err(shell_err)?;
        match target {
            Target::Path { target, arguments } => {
                link.SetPath(PCWSTR(wide(target).as_ptr())).map_err(shell_err)?;
                if !arguments.is_empty() {
                    link.SetArguments(PCWSTR(wide(arguments).as_ptr())).map_err(shell_err)?;
                }
            }
            Target::RecycleBin => {
                let pidl = SHGetKnownFolderIDList(&FOLDERID_RecycleBinFolder, 0, None).map_err(shell_err)?;
                // The link copies the item id list; the returned one is ours to free either way.
                let made = link.SetIDList(pidl.cast_const());
                CoTaskMemFree(Some(pidl.cast()));
                made.map_err(shell_err)?;
            }
        }
        let file: IPersistFile = link.cast().map_err(shell_err)?;
        file.Save(PCWSTR(wide(lnk).as_ptr()), true).map_err(shell_err)?;
    }
    Ok(())
}

/// Plain UTF-16 with NUL: no `\\?\` (that prefix belongs inside osprim opens only —
/// a link storing it would show it to every shell that later reads the link).
fn wide(text: &(impl AsRef<std::ffi::OsStr> + ?Sized)) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

fn shell_err(e: windows::core::Error) -> io::Error {
    io::Error::other(e.to_string())
}

/// Reads back what a link points at (its path and arguments): the observable contract
/// of the made file, used by tests and nothing else that could guess it cheaper.
pub fn read(lnk: &Path) -> io::Result<(String, String)> {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&SHELL_LINK, None, CLSCTX_INPROC_SERVER).map_err(shell_err)?;
        let file: IPersistFile = link.cast().map_err(shell_err)?;
        file.Load(PCWSTR(wide(lnk).as_ptr()), STGM_READ).map_err(shell_err)?;
        let mut buf = vec![0u16; 1_024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).map_err(shell_err)?;
        let path = decode(&buf);
        let mut args = vec![0u16; 1_024];
        link.GetArguments(&mut args).map_err(shell_err)?;
        Ok((path, decode(&args)))
    }
}

fn decode(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_made_link_names_the_program_and_arguments_it_opens() {
        let dir = tempfile::tempdir().unwrap();
        let lnk = dir.path().join("probe.lnk");
        let exe = std::env::current_exe().unwrap();
        create(&lnk, &Target::Path { target: &exe, arguments: "drives:///" }).expect("the shell builds the link");
        let (path, args) = read(&lnk).expect("the link file reads back");
        assert_eq!(path, exe.to_string_lossy(), "the link points at this very program");
        assert_eq!(args, "drives:///", "the argument survives the binary round-trip");
    }

    #[test]
    fn a_link_to_the_recycle_bin_opens_the_bin() {
        let dir = tempfile::tempdir().unwrap();
        let lnk = dir.path().join("bin.lnk");
        create(&lnk, &Target::RecycleBin).expect("the known folder is a link target");
        // A folder link's target is an item id list, not a path: the bin has no plain path
        // (GetPath answers empty), so what proves the link is the bin is the kept id list.
        let (path, _) = read(&lnk).expect("the bin link reads back");
        assert!(path.is_empty(), "the bin stays a shell folder, not a path ({path:?})");
        let holds = unsafe {
            let link: IShellLinkW = CoCreateInstance(&SHELL_LINK, None, CLSCTX_INPROC_SERVER).unwrap();
            let file: IPersistFile = link.cast().unwrap();
            file.Load(PCWSTR(wide(lnk.as_os_str()).as_ptr()), STGM_READ).unwrap();
            let pidl = link.GetIDList();
            if let Ok(p) = pidl {
                CoTaskMemFree(Some(p.cast()));
            }
            pidl
        };
        assert!(holds.is_ok_and(|p| !p.is_null()), "the link carries the bin's item id list");
    }
}
