//! Links the way Hermann Schinagl's Link Shell Extension (LSE) makes and shows them: symbolic
//! links, hard links, junctions and shell links, the overlays that mark them, and the tree
//! operations (Smart Copy, Smart Mirror, DeLorean Copy, Hardlink Clone, Symbolic Link Clone,
//! Smart Move). LSE is closed freeware; its own documentation is the specification followed
//! (README: Links), and every rule here is checked against real filesystems in the tests.

pub mod make;
pub mod moved;
pub mod siblings;
pub mod tree;

use std::path::{Component, Path, PathBuf};

/// A link that can be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    Symbolic,
    Hard,
    Junction,
    /// The platform's link file: `.lnk` on Windows, a `.desktop` link elsewhere.
    Shell,
}

impl LinkKind {
    pub const ALL: [LinkKind; 4] = [LinkKind::Symbolic, LinkKind::Hard, LinkKind::Junction, LinkKind::Shell];

    /// The menu label.
    pub fn label(self) -> &'static str {
        match self {
            LinkKind::Symbolic => "Symbolic Link",
            LinkKind::Hard => "Hard Link",
            LinkKind::Junction => "Junction",
            LinkKind::Shell => if cfg!(windows) { "Shell Link (.lnk)" } else { "Shell Link (.desktop)" },
        }
    }

    /// The word a link made beside its source carries in its name: "notes (hardlink).txt".
    pub fn name_tag(self) -> &'static str {
        match self {
            LinkKind::Symbolic => "symlink",
            LinkKind::Hard => "hardlink",
            LinkKind::Junction => "junction",
            LinkKind::Shell => "link",
        }
    }

    /// The id the menu actions use.
    pub fn id(self) -> &'static str {
        match self {
            LinkKind::Symbolic => "symbolic",
            LinkKind::Hard => "hard",
            LinkKind::Junction => "junction",
            LinkKind::Shell => "shell",
        }
    }

    pub fn from_id(id: &str) -> Option<LinkKind> {
        LinkKind::ALL.into_iter().find(|k| k.id() == id)
    }
}

/// What a filesystem can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FsCaps {
    pub symlinks: bool,
    pub hardlinks: bool,
    pub junctions: bool,
}

/// Magic numbers of `statfs` (`linux/magic.h`, and `fs/ntfs3/super.c` for ntfs3).
pub mod magic {
    pub const MSDOS: u64 = 0x4d44;
    pub const EXFAT: u64 = 0x2011_bab0;
    pub const ISOFS: u64 = 0x9660;
    pub const SQUASHFS: u64 = 0x7371_7368;
    pub const EXT4: u64 = 0xef53;
    pub const BTRFS: u64 = 0x9123_683e;
    pub const TMPFS: u64 = 0x0102_1994;
    pub const NTFS3: u64 = 0x7366_746e;
}

/// What a Linux filesystem of `statfs` type `magic` holds. Filesystems not known to refuse links
/// (network and FUSE ones among them) are offered both; the client's answer then decides.
pub fn caps_linux(magic: u64) -> FsCaps {
    let none = matches!(magic, magic::MSDOS | magic::EXFAT | magic::ISOFS);
    FsCaps { symlinks: !none, hardlinks: !none, junctions: false }
}

/// What a Windows volume of filesystem `name` (`GetVolumeInformationW`) holds: NTFS everything,
/// ReFS symbolic links and junctions (hard links from ReFS 3.5, left to the call to decide), FAT
/// and exFAT nothing.
pub fn caps_windows(name: &str) -> FsCaps {
    match name.to_ascii_uppercase().as_str() {
        "NTFS" => FsCaps { symlinks: true, hardlinks: true, junctions: true },
        "REFS" => FsCaps { symlinks: true, hardlinks: true, junctions: true },
        "FAT" | "FAT12" | "FAT16" | "FAT32" | "EXFAT" | "CDFS" | "UDF" => FsCaps::default(),
        // Third-party filesystems (LSE lists Btrfs by default): offered, the call decides.
        _ => FsCaps { symlinks: true, hardlinks: true, junctions: false },
    }
}

/// What the filesystem holding `dir` can hold.
pub fn caps_at(dir: &Path) -> FsCaps {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return FsCaps::default() };
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and a zeroed out-structure of the right type.
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return FsCaps::default();
        }
        caps_linux(st.f_type as u64)
    }
    #[cfg(windows)]
    {
        caps_windows(&crate::links::make::volume_fs_name(dir).unwrap_or_default())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        FsCaps::default()
    }
}

/// One thing dragged or picked, as far as making links from it is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub is_dir: bool,
    /// The filesystem the entry itself lives on.
    pub dev: Option<u64>,
}

impl Source {
    pub fn of(path: &Path) -> Source {
        Source { path: path.to_path_buf(), is_dir: path.is_dir(), dev: crate::osprim::entry_device(path) }
    }
}

/// The links that can be made from `sources` in a folder of capabilities `caps` on filesystem
/// `dest_dev` (README: Links, *What can be made where*).
pub fn offered(sources: &[Source], caps: FsCaps, dest_dev: Option<u64>) -> Vec<LinkKind> {
    if sources.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if caps.symlinks {
        out.push(LinkKind::Symbolic);
    }
    if caps.hardlinks && dest_dev.is_some() && sources.iter().all(|s| !s.is_dir && s.dev == dest_dev) {
        out.push(LinkKind::Hard);
    }
    if caps.junctions && cfg!(windows) && sources.iter().all(|s| s.is_dir) {
        out.push(LinkKind::Junction);
    }
    out.push(LinkKind::Shell);
    out
}

/// The overlay an entry shows (README: Links, *Overlay icons*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Overlay {
    Hard,
    Junction,
    Symbolic,
    /// A symbolic link or junction whose target is gone.
    Broken,
}

impl Overlay {
    pub fn code(self) -> u32 {
        match self {
            Overlay::Hard => 1,
            Overlay::Junction => 2,
            Overlay::Symbolic => 3,
            Overlay::Broken => 4,
        }
    }

    pub fn from_code(code: u32) -> Option<Overlay> {
        [Overlay::Hard, Overlay::Junction, Overlay::Symbolic, Overlay::Broken].into_iter().find(|o| o.code() == code)
    }
}

/// The overlay of an entry: a link marks itself whatever it points at; a plain file with more
/// than one name is a hard link; folders (whose link count counts their subfolders) never are.
pub fn overlay(is_link: bool, junction: bool, broken: bool, is_file: bool, nlink: u64) -> Option<Overlay> {
    if is_link {
        return Some(if broken {
            Overlay::Broken
        } else if junction {
            Overlay::Junction
        } else {
            Overlay::Symbolic
        });
    }
    (is_file && nlink > 1).then_some(Overlay::Hard)
}

/// `path` with `.` and `..` worked out by its spelling only (no filesystem access), the way a
/// symbolic link's target is read relative to the folder holding the link.
pub fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Where the link at `link` with the stored target `raw` points, as an absolute lexical path.
pub fn resolve(link: &Path, raw: &Path) -> PathBuf {
    if raw.is_absolute() {
        lexical(raw)
    } else {
        lexical(&link.parent().unwrap_or(Path::new("/")).join(raw))
    }
}

/// The relative spelling of `target` seen from the folder `from` (both absolute and lexical);
/// `None` when they share no root (other drives on Windows).
pub fn relative(from: &Path, target: &Path) -> Option<PathBuf> {
    let f: Vec<Component> = from.components().collect();
    let t: Vec<Component> = target.components().collect();
    if f.first() != t.first() {
        return None;
    }
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..f.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Some(out)
}

/// The target a new symbolic link at `link` to `target` stores: relative when wanted and both are
/// on one filesystem root, absolute otherwise.
pub fn spelling(link: &Path, target: &Path, relative_wanted: bool) -> PathBuf {
    let target = lexical(target);
    if relative_wanted {
        if let Some(r) = link.parent().and_then(|p| relative(&lexical(p), &target)) {
            return r;
        }
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystems_that_keep_no_links_are_not_offered_them() {
        assert_eq!(caps_linux(magic::EXT4), FsCaps { symlinks: true, hardlinks: true, junctions: false });
        assert_eq!(caps_linux(magic::BTRFS), caps_linux(magic::EXT4));
        assert_eq!(caps_linux(magic::NTFS3), caps_linux(magic::EXT4));
        for m in [magic::MSDOS, magic::EXFAT, magic::ISOFS] {
            assert_eq!(caps_linux(m), FsCaps::default(), "{m:#x}");
        }
        assert!(caps_windows("NTFS").junctions && caps_windows("ntfs").hardlinks);
        assert_eq!(caps_windows("FAT32"), FsCaps::default());
        assert_eq!(caps_windows("exFAT"), FsCaps::default());
        assert!(!caps_windows("btrfs").junctions && caps_windows("btrfs").symlinks);
    }

    #[test]
    fn the_folder_s_filesystem_is_read_by_statfs() {
        let d = tempfile::tempdir().unwrap();
        // Whatever the temporary folder is on, it keeps links on the machines this runs on.
        assert!(caps_at(d.path()).symlinks);
        assert_eq!(caps_at(Path::new("/no/such/folder")), FsCaps::default());
    }

    fn src(p: &str, is_dir: bool, dev: u64) -> Source {
        Source { path: p.into(), is_dir, dev: Some(dev) }
    }

    #[test]
    fn hard_links_only_for_files_on_the_same_filesystem() {
        let all = FsCaps { symlinks: true, hardlinks: true, junctions: true };
        assert_eq!(offered(&[src("/a/f", false, 1)], all, Some(1)), {
            let mut v = vec![LinkKind::Symbolic, LinkKind::Hard];
            if cfg!(windows) {
                // Junctions are for folders only.
            }
            v.push(LinkKind::Shell);
            v
        });
        assert!(!offered(&[src("/a/f", false, 1)], all, Some(2)).contains(&LinkKind::Hard));
        assert!(!offered(&[src("/a/f", false, 1), src("/a/d", true, 1)], all, Some(1)).contains(&LinkKind::Hard));
        assert_eq!(offered(&[src("/a/f", false, 1)], FsCaps::default(), Some(1)), [LinkKind::Shell]);
        assert!(offered(&[], all, Some(1)).is_empty());
        assert_eq!(offered(&[src("/a/d", true, 1)], all, Some(1)).contains(&LinkKind::Junction), cfg!(windows));
    }

    #[test]
    fn each_link_kind_has_its_overlay() {
        assert_eq!(overlay(true, false, false, false, 1), Some(Overlay::Symbolic));
        assert_eq!(overlay(true, true, false, false, 1), Some(Overlay::Junction));
        assert_eq!(overlay(true, false, true, false, 1), Some(Overlay::Broken));
        assert_eq!(overlay(true, true, true, false, 1), Some(Overlay::Broken));
        assert_eq!(overlay(false, false, false, true, 2), Some(Overlay::Hard));
        assert_eq!(overlay(false, false, false, true, 1), None);
        assert_eq!(overlay(false, false, false, false, 5), None, "a folder's count is its subfolders");
        for o in [Overlay::Hard, Overlay::Junction, Overlay::Symbolic, Overlay::Broken] {
            assert_eq!(Overlay::from_code(o.code()), Some(o));
        }
        assert_eq!(Overlay::from_code(0), None);
    }

    #[test]
    fn link_targets_are_read_relative_to_the_link_s_folder() {
        assert_eq!(lexical(Path::new("/a/./b/../c")), Path::new("/a/c"));
        assert_eq!(resolve(Path::new("/t/sub/link"), Path::new("../x/y")), Path::new("/t/x/y"));
        assert_eq!(resolve(Path::new("/t/sub/link"), Path::new("/abs/./z")), Path::new("/abs/z"));
        assert_eq!(relative(Path::new("/t/a/b"), Path::new("/t/c")).unwrap(), Path::new("../../c"));
        assert_eq!(relative(Path::new("/t"), Path::new("/t/c/d")).unwrap(), Path::new("c/d"));
        assert_eq!(relative(Path::new("/t"), Path::new("/t")).unwrap(), Path::new("."));
        assert_eq!(spelling(Path::new("/t/a/link"), Path::new("/t/b/f"), true), Path::new("../b/f"));
        assert_eq!(spelling(Path::new("/t/a/link"), Path::new("/t/b/f"), false), Path::new("/t/b/f"));
    }

    #[test]
    fn kinds_round_trip_through_their_ids() {
        for k in LinkKind::ALL {
            assert_eq!(LinkKind::from_id(k.id()), Some(k));
        }
    }
}
