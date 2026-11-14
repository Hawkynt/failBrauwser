//! The trash as the freedesktop.org Trash specification defines it — the same trash Thunar,
//! Nautilus and Dolphin use. A trashed file lives in `<trash>/files/NAME` and is described by
//! `<trash>/info/NAME.trashinfo`:
//!
//! ```text
//! [Trash Info]
//! Path=/home/me/Documents/report%20final.pdf
//! DeletionDate=2026-09-29T10:15:02
//! ```
//!
//! The home trash is `$XDG_DATA_HOME/Trash`; files on other drives go to `<top>/.Trash-<uid>`
//! (or `<top>/.Trash/<uid>`), where `Path` is relative to the drive's top directory.
//!
//! On Windows the same model is backed by the shell Recycle Bin (`win::recyclebin`): a bin
//! entry maps onto `TrashItem` with the `$R…` name, its `$Recycle.Bin` location, the bin's
//! display name as the original place and the `DateDeleted` property as the deletion time.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashDir {
    /// The trash folder itself (holding `files` and `info`).
    pub root: PathBuf,
    /// For trash on another drive: its top directory, which relative paths start from.
    pub top: Option<PathBuf>,
}

impl TrashDir {
    pub fn files(&self) -> PathBuf {
        self.root.join("files")
    }
    pub fn info(&self) -> PathBuf {
        self.root.join("info")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrashItem {
    /// Name inside `files/` (unique within this trash folder).
    pub name: String,
    /// The file in the trash.
    pub path: PathBuf,
    /// Its `.trashinfo`.
    pub info: PathBuf,
    /// Where it was deleted from.
    pub original: PathBuf,
    /// Local time as written by the spec, e.g. `2026-09-29T10:15:02`.
    pub deleted: String,
}

impl TrashItem {
    /// The name it had before (shown in the trash view).
    pub fn original_name(&self) -> String {
        self.original.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.name.clone())
    }

    /// Deletion time as Unix time (local), 0 if unknown.
    pub fn deleted_unix(&self) -> i64 {
        let tz = gtk::glib::TimeZone::local();
        gtk::glib::DateTime::from_iso8601(&self.deleted, Some(&tz)).map(|d| d.to_unix()).unwrap_or(0)
    }
}

/// Decodes the percent-escapes the spec uses in `Path=`.
pub fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Reads a `.trashinfo`; returns (original path as written, deletion date).
pub fn parse_trashinfo(text: &str) -> Option<(PathBuf, String)> {
    let mut in_section = false;
    let (mut path, mut date) = (None, String::new());
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Trash Info]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(v) = line.strip_prefix("Path=") {
            path = Some(decode_path(percent_decode(v)));
        } else if let Some(v) = line.strip_prefix("DeletionDate=") {
            date = v.to_string();
        }
    }
    path.map(|p| (p, date))
}

/// The percent-decoded spec bytes as a path: raw on Unix (where they are the name),
/// UTF-8 text elsewhere.
#[cfg(unix)]
fn decode_path(bytes: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn decode_path(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

/// The home trash and the trash folders of mounted drives that exist.
pub fn trash_dirs() -> Vec<TrashDir> {
    #[cfg(unix)]
    return unix_trash_dirs();
    // Windows: there are no trash folders to visit — the single shell Recycle Bin behind
    // win::recyclebin is what list/is_empty/empty consult, whatever they are handed here.
    #[cfg(windows)]
    return Vec::new();
    #[cfg(not(any(unix, windows)))]
    Vec::new()
}

#[cfg(unix)]
fn unix_trash_dirs() -> Vec<TrashDir> {
    let uid = unsafe { libc::getuid() };
    let mut dirs = vec![TrashDir { root: gtk::glib::user_data_dir().join("Trash"), top: None }];
    let home_dev = std::fs::metadata(gtk::glib::user_data_dir()).ok().map(|m| std::os::unix::fs::MetadataExt::dev(&m));
    for vol in crate::drives::read_mountinfo() {
        for top in &vol.mount_points {
            // The home trash already covers the drive the home folder is on.
            let dev = std::fs::metadata(top).ok().map(|m| std::os::unix::fs::MetadataExt::dev(&m));
            if dev.is_some() && dev == home_dev {
                continue;
            }
            for root in [top.join(format!(".Trash-{uid}")), top.join(".Trash").join(uid.to_string())] {
                if root.join("files").is_dir() {
                    dirs.push(TrashDir { root, top: Some(top.clone()) });
                }
            }
        }
    }
    dirs
}

/// Everything in the given trash folders. Entries without a valid `.trashinfo` are listed
/// with an unknown origin, so they can still be deleted.
#[cfg(unix)]
pub fn list(dirs: &[TrashDir]) -> Vec<TrashItem> {
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d.files()) else { continue };
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            let info = d.info().join(format!("{name}.trashinfo"));
            let (original, deleted) = std::fs::read_to_string(&info)
                .ok()
                .and_then(|t| parse_trashinfo(&t))
                .map(|(p, date)| {
                    let full = match (&d.top, p.is_absolute()) {
                        (Some(top), false) => top.join(p),
                        _ => p,
                    };
                    (full, date)
                })
                .unwrap_or_else(|| (PathBuf::from(&name), String::new()));
            out.push(TrashItem { name, path: e.path(), info, original, deleted });
        }
    }
    out
}

#[cfg(unix)]
pub fn is_empty(dirs: &[TrashDir]) -> bool {
    dirs.iter().all(|d| std::fs::read_dir(d.files()).map(|mut r| r.next().is_none()).unwrap_or(true))
}

/// All bin entries across the drives' Recycle Bins, mapped onto this module's model.
#[cfg(windows)]
pub fn list(_dirs: &[TrashDir]) -> Vec<TrashItem> {
    crate::win::recyclebin::list()
        .unwrap_or_default()
        .into_iter()
        .map(|b| TrashItem { name: b.name.clone(), path: b.path.clone(), info: bin_info_path(&b.path), original: b.original, deleted: b.deleted })
        .collect()
}

/// The shell's `$I…` metadata file that belongs to a `$R…` entry — carried for identity
/// only; the shell owns its contents and removal.
#[cfg(windows)]
fn bin_info_path(parser: &Path) -> PathBuf {
    let name = parser.file_name().map(|n| n.to_string_lossy().replacen("$R", "$I", 1)).unwrap_or_default();
    parser.parent().map(|p| p.join(name)).unwrap_or_else(|| parser.to_path_buf())
}

#[cfg(windows)]
pub fn is_empty(_dirs: &[TrashDir]) -> bool {
    crate::win::recyclebin::query().map(|(n, _)| n == 0).unwrap_or(true)
}

/// Puts one file into the platform trash (the delete operation and the restore-conflict
/// path both use this). `NotFound` when it is already gone.
pub fn trash_file(p: &Path) -> io::Result<()> {
    #[cfg(windows)]
    return crate::win::recyclebin::trash(p);
    #[cfg(not(windows))]
    match gtk::gio::prelude::FileExt::trash(&gtk::gio::File::for_path(p), gtk::gio::Cancellable::NONE) {
        Ok(()) => Ok(()),
        Err(e) if e.matches(gtk::gio::IOErrorEnum::NotFound) => Err(io::Error::new(io::ErrorKind::NotFound, e.to_string())),
        Err(e) => Err(io::Error::other(e.to_string())),
    }
}

#[cfg(unix)]
fn remove_any(p: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Puts an item back where it came from (or at `to`, e.g. a free name next to it).
/// Missing parent folders are created. Fails with `AlreadyExists` if the target is taken.
#[cfg(unix)]
pub fn restore(item: &TrashItem, to: Option<&Path>) -> io::Result<PathBuf> {
    let target = to.map(Path::to_path_buf).unwrap_or_else(|| item.original.clone());
    if !target.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the original location is unknown"));
    }
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("“{}” already exists", target.display())));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match crate::ops::copy::rename_noreplace(&item.path, &target) {
        Ok(()) => {}
        // Restoring to another drive (the folder was moved meanwhile): copy, then delete.
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let ctx = crate::ops::job::JobCtx::new();
            let parent = target.parent().unwrap_or(Path::new("/"));
            let copied = crate::ops::copy::transfer(&ctx, std::slice::from_ref(&item.path), parent, crate::ops::copy::Mode::Copy, &Default::default())?;
            if let Some(c) = copied.first() {
                if c != &target {
                    std::fs::rename(c, &target)?;
                }
            }
            remove_any(&item.path)?;
        }
        Err(e) => return Err(e),
    }
    let _ = std::fs::remove_file(&item.info);
    Ok(target)
}

/// Puts a bin entry back where the shell says it came from (or at `to`),
/// renaming the `$R…` file to its original name on the way out.
#[cfg(windows)]
pub fn restore(item: &TrashItem, to: Option<&Path>) -> io::Result<PathBuf> {
    crate::win::recyclebin::restore(&item.path, &item.original, to)
}

/// Deletes an item for good (the file and its `.trashinfo`).
#[cfg(unix)]
pub fn delete(item: &TrashItem) -> io::Result<()> {
    remove_any(&item.path)?;
    let _ = std::fs::remove_file(&item.info);
    Ok(())
}

/// Deletes a bin entry for good; what is left of the `$I…` metadata is shell business —
/// orphaned entries never come back into `list`.
#[cfg(windows)]
pub fn delete(item: &TrashItem) -> io::Result<()> {
    crate::win::recyclebin::delete(&item.path)
}

/// Empties the given trash folders (also stray `.trashinfo` files without a file).
#[cfg(unix)]
pub fn empty(dirs: &[TrashDir]) -> io::Result<()> {
    for item in list(dirs) {
        delete(&item)?;
    }
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(d.info()) {
            for e in rd.filter_map(Result::ok) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

/// Empties every drive's Recycle Bin, as Explorer's Empty Recycle Bin does.
#[cfg(windows)]
pub fn empty(_dirs: &[TrashDir]) -> io::Result<()> {
    crate::win::recyclebin::empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)] // every fixture that touches the filesystem is a Unix scenario
    use std::fs;

    #[cfg(unix)] // builds the freedesktop folders the Unix scenarios need
    fn put(dir: &TrashDir, name: &str, path_line: &str, content: &str) {
        fs::create_dir_all(dir.files()).unwrap();
        fs::create_dir_all(dir.info()).unwrap();
        fs::write(dir.files().join(name), content).unwrap();
        fs::write(dir.info().join(format!("{name}.trashinfo")), format!("[Trash Info]\nPath={path_line}\nDeletionDate=2026-09-29T10:15:02\n")).unwrap();
    }

    #[test]
    fn parses_trashinfo_with_escapes() {
        let (p, d) = parse_trashinfo("[Trash Info]\nPath=/home/me/My%20Docs/%C3%A4.txt\nDeletionDate=2026-09-29T10:15:02\n").unwrap();
        assert_eq!(p, PathBuf::from("/home/me/My Docs/ä.txt"));
        assert_eq!(d, "2026-09-29T10:15:02");
        assert!(parse_trashinfo("[Other]\nPath=/x\n").is_none());
    }

    #[test]
    #[cfg(unix)] // the fixture builds freedesktop trash folders; Windows tests live in win::recyclebin
    fn lists_home_and_volume_trash() {
        let t = tempfile::tempdir().unwrap();
        let home = TrashDir { root: t.path().join("home/Trash"), top: None };
        let vol = TrashDir { root: t.path().join("usb/.Trash-1000"), top: Some(t.path().join("usb")) };
        put(&home, "a.txt", "/somewhere/a.txt", "a");
        put(&vol, "b.txt", "photos/b.txt", "b");
        fs::write(home.files().join("orphan"), "o").unwrap();
        let mut items = list(&[home.clone(), vol.clone()]);
        items.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].original, PathBuf::from("/somewhere/a.txt"));
        assert_eq!(items[1].original, t.path().join("usb/photos/b.txt"));
        assert_eq!(items[2].original_name(), "orphan");
        assert!(items[0].deleted_unix() > 0);
        assert!(!is_empty(&[home.clone(), vol.clone()]));
    }

    #[test]
    #[cfg(unix)] // same fixture caveat: restore/delete/empty here drive the folder implementation
    fn restore_delete_and_empty() {
        let t = tempfile::tempdir().unwrap();
        let home = TrashDir { root: t.path().join("Trash"), top: None };
        let orig = t.path().join("gone/deeper/x.txt");
        put(&home, "x.txt", &orig.to_string_lossy().replace(' ', "%20"), "x");
        let item = list(std::slice::from_ref(&home)).remove(0);
        // Missing parent folders come back too.
        assert_eq!(restore(&item, None).unwrap(), orig);
        assert_eq!(fs::read_to_string(&orig).unwrap(), "x");
        assert!(!item.info.exists());

        // A taken name is refused; restoring elsewhere works.
        put(&home, "x.txt", &orig.to_string_lossy(), "second");
        let item = list(std::slice::from_ref(&home)).remove(0);
        assert_eq!(restore(&item, None).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        let other = t.path().join("gone/deeper/x (2).txt");
        restore(&item, Some(&other)).unwrap();
        assert_eq!(fs::read_to_string(&other).unwrap(), "second");

        put(&home, "d", "/d", "1");
        fs::create_dir_all(home.files().join("folder/sub")).unwrap();
        fs::write(home.info().join("folder.trashinfo"), "[Trash Info]\nPath=/folder\n").unwrap();
        let d = list(std::slice::from_ref(&home)).into_iter().find(|i| i.name == "d").unwrap();
        delete(&d).unwrap();
        assert!(!d.path.exists() && !d.info.exists());
        empty(std::slice::from_ref(&home)).unwrap();
        assert!(is_empty(std::slice::from_ref(&home)));
        assert_eq!(fs::read_dir(home.info()).unwrap().count(), 0);
    }
}
