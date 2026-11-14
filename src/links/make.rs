//! Making one link (README: Links, *What can be made where*), and reading what a link is.
//!
//! The Windows arms follow Microsoft's documentation: `CreateSymbolicLinkW` with
//! `SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE`, `CreateHardLinkW` (through `std::fs::hard_link`)
//! and a junction as the mount-point reparse point `FSCTL_SET_REPARSE_POINT` writes ([MS-FSCC]
//! 2.1.2.5, Mount Point Reparse Data Buffer: substitute name `\??\C:\path`, print name `C:\path`).
//! No Windows machine runs the tests here, so the Windows arms are unmeasured.

use super::LinkKind;
use crate::ops::names::free_name;
use std::io;
use std::path::{Path, PathBuf};

/// Makes a link of `kind` to `source` in the folder `dest_dir`, named like the source (with
/// "(hardlink)" and so on when that name is taken, as in the source's own folder); `relative`
/// asks for a relative symbolic link where one can be; `icon` is the themed icon a `.desktop`
/// link shows. Gives the path made.
pub fn create(kind: LinkKind, source: &Path, dest_dir: &Path, relative: bool, icon: &str) -> io::Result<PathBuf> {
    let name = source.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no name"))?;
    if kind == LinkKind::Shell {
        let url = crate::desktoplink::file_url(source).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        return crate::desktoplink::place_in(dest_dir, &name.to_string_lossy(), icon, &crate::desktoplink::Target::Url(url));
    }
    let mut link = dest_dir.join(name);
    if std::fs::symlink_metadata(&link).is_ok() {
        link = dest_dir.join(free_name(dest_dir, name, Some(kind.name_tag())));
    }
    make_at(kind, source, &link, relative)?;
    Ok(link)
}

/// Makes a link of `kind` at exactly `link` pointing to `target`.
pub fn make_at(kind: LinkKind, target: &Path, link: &Path, relative: bool) -> io::Result<()> {
    match kind {
        LinkKind::Symbolic => symlink(target, link, relative),
        LinkKind::Hard => std::fs::hard_link(target, link),
        LinkKind::Junction => junction(target, link),
        LinkKind::Shell => Err(io::Error::from(io::ErrorKind::Unsupported)),
    }
}

/// A symbolic link at `link` to `target`, relative when wanted (and possible).
pub fn symlink(target: &Path, link: &Path, relative: bool) -> io::Result<()> {
    let abs = if target.is_absolute() { target.to_path_buf() } else { std::env::current_dir()?.join(target) };
    let stored = super::spelling(link, &abs, relative);
    crate::osprim::make_symlink(&stored, link, abs.is_dir())
}

/// Points the existing symbolic link or junction `link` at `target`, keeping its kind and name:
/// the new link is made beside it and renamed over it, so the name is never missing.
pub fn retarget(link: &Path, target: &Path, relative: bool) -> io::Result<()> {
    let junction = is_junction(link);
    let meta = std::fs::symlink_metadata(link)?;
    if !meta.file_type().is_symlink() && !junction {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a link"));
    }
    let dir = link.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".{}.fb-relink", link.file_name().unwrap_or_default().to_string_lossy()));
    let _ = remove_link(&tmp);
    if junction {
        self::junction(target, &tmp)?;
    } else {
        symlink(target, &tmp, relative)?;
    }
    #[cfg(windows)]
    {
        // A directory link cannot be renamed over another one: the old one goes first.
        remove_link(link)?;
    }
    std::fs::rename(&tmp, link)
}

/// Removes a link itself, never what it points at.
pub fn remove_link(link: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if std::fs::symlink_metadata(link)?.is_dir() {
            return std::fs::remove_dir(link);
        }
    }
    std::fs::remove_file(link)
}

/// What a symbolic link or junction stores.
pub fn target_of(link: &Path) -> io::Result<PathBuf> {
    std::fs::read_link(link)
}

/// Whether `path` is a junction (a mount-point reparse point); never on other systems.
pub fn is_junction(path: &Path) -> bool {
    #[cfg(windows)]
    {
        win::reparse_tag(path) == Some(win::IO_REPARSE_TAG_MOUNT_POINT)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

fn junction(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        win::junction(target, link)
    }
    #[cfg(not(windows))]
    {
        let _ = (target, link);
        Err(io::Error::new(io::ErrorKind::Unsupported, "junctions are a Windows filesystem's"))
    }
}

/// The filesystem name of the volume holding `dir` ("NTFS", "FAT32", …).
#[cfg(windows)]
pub fn volume_fs_name(dir: &Path) -> Option<String> {
    win::volume_fs_name(dir)
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FindClose, FindFirstFileW, GetVolumeInformationW, GetVolumePathNameW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WIN32_FIND_DATAW,
    };
    use windows::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;
    use windows::Win32::System::IO::DeviceIoControl;

    /// `IO_REPARSE_TAG_MOUNT_POINT` (winnt.h).
    pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// The reparse tag of `path`: FindFirstFileW reports it in `dwReserved0` for a reparse point.
    pub fn reparse_tag(path: &Path) -> Option<u32> {
        let w = wide(path);
        let mut data = WIN32_FIND_DATAW::default();
        let h = unsafe { FindFirstFileW(PCWSTR(w.as_ptr()), &mut data) }.ok()?;
        unsafe {
            let _ = FindClose(h);
        }
        (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0).then_some(data.dwReserved0)
    }

    pub fn volume_fs_name(dir: &Path) -> Option<String> {
        let w = wide(dir);
        let mut root = [0u16; 512];
        unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut root) }.ok()?;
        let mut fs = [0u16; 64];
        unsafe { GetVolumeInformationW(PCWSTR(root.as_ptr()), None, None, None, None, Some(&mut fs)) }.ok()?;
        let n = fs.iter().position(|c| *c == 0).unwrap_or(fs.len());
        Some(String::from_utf16_lossy(&fs[..n]))
    }

    /// A junction at `link` (made as an empty folder) to the folder `target`: the reparse data
    /// buffer of a mount point ([MS-FSCC] 2.1.2.5) set on it.
    pub fn junction(target: &Path, link: &Path) -> io::Result<()> {
        let abs = std::path::absolute(target)?;
        let print: Vec<u16> = abs.as_os_str().encode_wide().collect();
        let mut subst: Vec<u16> = r"\??\".encode_utf16().collect();
        subst.extend_from_slice(&print);
        // Header: tag, data length, reserved; then the four name offsets and lengths in bytes,
        // the substitute name and print name each NUL-terminated.
        let names_len = (subst.len() + 1 + print.len() + 1) * 2;
        let data_len = 8 + names_len;
        let mut buf: Vec<u8> = Vec::with_capacity(8 + data_len);
        buf.extend_from_slice(&IO_REPARSE_TAG_MOUNT_POINT.to_le_bytes());
        buf.extend_from_slice(&(data_len as u16).to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // SubstituteNameOffset
        buf.extend_from_slice(&((subst.len() * 2) as u16).to_le_bytes()); // SubstituteNameLength
        buf.extend_from_slice(&(((subst.len() + 1) * 2) as u16).to_le_bytes()); // PrintNameOffset
        buf.extend_from_slice(&((print.len() * 2) as u16).to_le_bytes()); // PrintNameLength
        for c in subst.iter().chain(Some(&0)).chain(print.iter()).chain(Some(&0)) {
            buf.extend_from_slice(&c.to_le_bytes());
        }
        std::fs::create_dir(link)?;
        let w = wide(link);
        let h: HANDLE = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                None,
            )
        }
        .map_err(crate::win::err::to_io)?;
        let r = unsafe { DeviceIoControl(h, FSCTL_SET_REPARSE_POINT, Some(buf.as_ptr() as *const _), buf.len() as u32, None, 0, None, None) };
        unsafe {
            let _ = CloseHandle(h);
        }
        if let Err(e) = r {
            let _ = std::fs::remove_dir(link);
            return Err(crate::win::err::to_io(e));
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_link_beside_its_source_gets_a_free_name() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("notes.txt");
        std::fs::write(&f, "x").unwrap();
        let h = create(LinkKind::Hard, &f, d.path(), true, "").unwrap();
        assert_eq!(h, d.path().join("notes (hardlink).txt"));
        let s = create(LinkKind::Symbolic, &f, d.path(), true, "").unwrap();
        assert_eq!(s, d.path().join("notes (symlink).txt"));
        assert_eq!(std::fs::read_link(&s).unwrap(), Path::new("notes.txt"), "relative within one folder");
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&f).unwrap().nlink(), 2);
    }

    #[test]
    fn symbolic_links_are_relative_or_absolute_as_asked() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::create_dir_all(d.path().join("c")).unwrap();
        let t = d.path().join("a/b/t.txt");
        std::fs::write(&t, "x").unwrap();
        let rel = create(LinkKind::Symbolic, &t, &d.path().join("c"), true, "").unwrap();
        assert_eq!(std::fs::read_link(&rel).unwrap(), Path::new("../a/b/t.txt"));
        assert_eq!(std::fs::read_to_string(&rel).unwrap(), "x");
        std::fs::remove_file(&rel).unwrap();
        let abs = create(LinkKind::Symbolic, &t, &d.path().join("c"), false, "").unwrap();
        assert_eq!(std::fs::read_link(&abs).unwrap(), t);
    }

    #[test]
    fn a_shell_link_is_a_desktop_link_here() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("Music");
        std::fs::create_dir(&f).unwrap();
        let out = d.path().join("out");
        let l = create(LinkKind::Shell, &f, &out, true, "folder-music").unwrap();
        assert_eq!(l, out.join("Music.desktop"));
        let text = std::fs::read_to_string(&l).unwrap();
        assert!(text.contains("Type=Link") && text.contains("Icon=folder-music"), "{text}");
    }

    #[test]
    fn a_retargeted_link_keeps_its_name() {
        let d = tempfile::tempdir().unwrap();
        for n in ["one", "two"] {
            std::fs::create_dir(d.path().join(n)).unwrap();
            std::fs::write(d.path().join(n).join("which"), n).unwrap();
        }
        let l = d.path().join("current");
        symlink(&d.path().join("one"), &l, true).unwrap();
        retarget(&l, &d.path().join("two"), true).unwrap();
        assert_eq!(std::fs::read_to_string(l.join("which")).unwrap(), "two");
        assert_eq!(std::fs::read_link(&l).unwrap(), Path::new("two"));
        assert!(retarget(&d.path().join("one"), &d.path().join("two"), true).is_err(), "a folder is no link");
        assert!(!is_junction(&l));
    }

    #[test]
    fn junctions_are_refused_off_windows() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(make_at(LinkKind::Junction, d.path(), &d.path().join("j"), true).unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
