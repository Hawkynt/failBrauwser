//! Recursive folder sizes.
//!
//! Symbolic links are never followed (a link to `/` inside a folder would otherwise count
//! the whole disk, and link loops would never end); a link counts as an entry of 0 bytes.
//! Files with several hard links count once. Other filesystems mounted below the folder
//! are not entered, like `du -x`. A folder met a second time (a bind mount of a folder
//! inside itself has the same device) is not entered again, so no walk goes round in circles.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirSize {
    /// Sum of file sizes (apparent size, like the Size column).
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    /// Some entries could not be read; the size is a lower bound.
    pub incomplete: bool,
    /// Mount points below were skipped.
    pub skipped_mounts: bool,
}

/// Computes the size of `root`. `progress` is called now and then with the running total.
/// Returns `None` when cancelled.
pub fn dir_size(root: &Path, cancel: &AtomicBool, mut progress: impl FnMut(&DirSize)) -> Option<DirSize> {
    let mut total = DirSize::default();
    let Ok(meta) = std::fs::symlink_metadata(root) else {
        total.incomplete = true;
        return Some(total);
    };
    if !meta.is_dir() {
        total.bytes = meta.len();
        total.files = 1;
        return Some(total);
    }
    let dev = meta.dev();
    let mut seen_links: HashSet<(u64, u64)> = HashSet::new();
    let mut seen_dirs: HashSet<(u64, u64)> = HashSet::from([(meta.dev(), meta.ino())]);
    let mut stack = vec![root.to_path_buf()];
    let mut last_report = Instant::now();
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        total.dirs += 1;
        let Ok(rd) = std::fs::read_dir(&dir) else {
            total.incomplete = true;
            continue;
        };
        for entry in rd {
            let Ok(entry) = entry else {
                total.incomplete = true;
                continue;
            };
            // DirEntry::metadata is an fstatat relative to the open directory: no symlink follow.
            let Ok(m) = entry.metadata() else {
                total.incomplete = true;
                continue;
            };
            let ft = m.file_type();
            if ft.is_dir() {
                if m.dev() != dev {
                    total.skipped_mounts = true;
                } else if seen_dirs.insert((m.dev(), m.ino())) {
                    stack.push(entry.path());
                }
                continue;
            }
            if m.nlink() > 1 && !ft.is_symlink() && !seen_links.insert((m.dev(), m.ino())) {
                continue;
            }
            total.files += 1;
            if !ft.is_symlink() {
                total.bytes += m.len();
            }
        }
        if last_report.elapsed() >= Duration::from_millis(200) {
            last_report = Instant::now();
            progress(&total);
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn size(p: &Path) -> DirSize {
        dir_size(p, &AtomicBool::new(false), |_| {}).unwrap()
    }

    #[test]
    fn sums_nested_files() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("a/b")).unwrap();
        fs::write(d.path().join("x"), vec![0u8; 100]).unwrap();
        fs::write(d.path().join("a/b/y"), vec![0u8; 23]).unwrap();
        let s = size(d.path());
        assert_eq!((s.bytes, s.files, s.dirs), (123, 2, 3));
        assert!(!s.incomplete);
    }

    #[test]
    fn symlinks_are_not_followed_and_loops_end() {
        let d = tempfile::tempdir().unwrap();
        let big = tempfile::tempdir().unwrap();
        fs::write(big.path().join("huge"), vec![0u8; 50_000]).unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        fs::write(d.path().join("sub/f"), vec![0u8; 10]).unwrap();
        std::os::unix::fs::symlink(big.path(), d.path().join("to-big")).unwrap();
        std::os::unix::fs::symlink(d.path(), d.path().join("sub/loop")).unwrap();
        let s = size(d.path());
        assert_eq!(s.bytes, 10, "links must not add their targets");
        assert_eq!(s.files, 3); // f and the two links themselves
    }

    #[test]
    fn hard_links_count_once() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("orig"), vec![0u8; 1000]).unwrap();
        fs::hard_link(d.path().join("orig"), d.path().join("link1")).unwrap();
        fs::hard_link(d.path().join("orig"), d.path().join("link2")).unwrap();
        assert_eq!(size(d.path()).bytes, 1000);
    }

    #[test]
    fn plain_file_and_missing_path() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("f"), vec![0u8; 7]).unwrap();
        assert_eq!(size(&d.path().join("f")).bytes, 7);
        assert!(size(&d.path().join("missing")).incomplete);
    }

    #[test]
    fn cancel_returns_none() {
        let d = tempfile::tempdir().unwrap();
        assert!(dir_size(d.path(), &AtomicBool::new(true), |_| {}).is_none());
    }

    #[test]
    fn unreadable_folder_marks_incomplete() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("locked")).unwrap();
        fs::write(d.path().join("locked/f"), "x").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(d.path().join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        let s = size(d.path());
        fs::set_permissions(d.path().join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(s.incomplete);
    }
}
