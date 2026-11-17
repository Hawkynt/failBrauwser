//! The other names of a file with several hard links (README: Links, *Link properties*).
//!
//! Linux keeps no list of a file's names: its filesystem is searched for the same file (device and
//! inode), from the mount point down, never into other filesystems or through links, and stops as
//! soon as every name is found. Windows lists them through `FindFirstFileNameW` /
//! `FindNextFileNameW` (unmeasured here: no Windows machine runs the tests).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
#[cfg(not(windows))]
use std::sync::atomic::Ordering;

/// The other names of `path`; empty for a file with one name. `stop` ends a search early.
pub fn siblings(path: &Path, stop: &AtomicBool) -> std::io::Result<Vec<PathBuf>> {
    let meta = std::fs::symlink_metadata(path)?;
    let me = crate::osprim::identity(path, &meta);
    if !meta.is_file() || me.nlink < 2 {
        return Ok(Vec::new());
    }
    #[cfg(windows)]
    {
        let _ = stop;
        return win::names(path);
    }
    #[cfg(not(windows))]
    {
        let own = std::fs::canonicalize(path)?;
        let root = mount_root(&own, me.dev);
        let mut found = Vec::new();
        let want = (me.nlink - 1) as usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            if stop.load(Ordering::Relaxed) || found.len() >= want {
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let Ok(m) = e.metadata() else { continue };
                let p = e.path();
                if m.is_dir() {
                    if crate::osprim::identity(&p, &m).dev == me.dev {
                        stack.push(p);
                    }
                } else if m.is_file() {
                    let id = crate::osprim::identity(&p, &m);
                    if id.dev == me.dev && id.id == me.id && p != own {
                        found.push(p);
                    }
                }
            }
        }
        found.sort();
        Ok(found)
    }
}

/// The highest folder above `path` still on the filesystem `dev`.
#[cfg(not(windows))]
fn mount_root(path: &Path, dev: u64) -> PathBuf {
    let mut root = path.parent().unwrap_or(Path::new("/")).to_path_buf();
    while let Some(up) = root.parent() {
        match crate::osprim::device_of(up) {
            Some(d) if d == dev => root = up.to_path_buf(),
            _ => break,
        }
    }
    root
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Storage::FileSystem::{FindClose, FindFirstFileNameW, FindNextFileNameW, GetVolumePathNameW};

    /// Every name of `path` but its own: the names come relative to the volume root.
    pub fn names(path: &Path) -> std::io::Result<Vec<PathBuf>> {
        let w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut root = [0u16; 512];
        unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut root) }.map_err(crate::win::err::to_io)?;
        let root = PathBuf::from(String::from_utf16_lossy(&root[..root.iter().position(|c| *c == 0).unwrap_or(0)]));
        let own = std::fs::canonicalize(path).ok();
        let mut out = Vec::new();
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        let h = unsafe { FindFirstFileNameW(PCWSTR(w.as_ptr()), 0, &mut len, PWSTR(buf.as_mut_ptr())) }.map_err(crate::win::err::to_io)?;
        loop {
            let name = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
            let p = root.join(name.trim_start_matches('\\'));
            if std::fs::canonicalize(&p).ok() != own {
                out.push(p);
            }
            len = buf.len() as u32;
            if unsafe { FindNextFileNameW(h, &mut len, PWSTR(buf.as_mut_ptr())) }.is_err() {
                break;
            }
        }
        unsafe {
            let _ = FindClose(h);
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn every_other_name_of_a_file_is_found() {
        // Searched from the mount point: a folder of its own under the user's cache keeps the
        // test bounded to this fixture plus whatever that filesystem holds.
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        std::fs::create_dir_all(t.path().join("x/y")).unwrap();
        std::fs::write(&a, "data").unwrap();
        std::fs::hard_link(&a, t.path().join("x/b")).unwrap();
        std::fs::hard_link(&a, t.path().join("x/y/c")).unwrap();
        let stop = AtomicBool::new(false);
        let got = siblings(&a, &stop).unwrap();
        let own = |p: &str| std::fs::canonicalize(t.path().join(p)).unwrap();
        assert_eq!(got, [own("x/b"), own("x/y/c")]);
        std::fs::write(t.path().join("single"), "").unwrap();
        assert!(siblings(&t.path().join("single"), &stop).unwrap().is_empty());
        assert!(siblings(&t.path().join("x"), &stop).unwrap().is_empty(), "a folder has no siblings");
    }
}
