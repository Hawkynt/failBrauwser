//! The tree operations of Link Shell Extension (README: Links, *The tree operations*): Smart
//! Copy, Smart Mirror, Hardlink Clone, Symbolic Link Clone and DeLorean Copy.
//!
//! All source folders count as one tree: a link pointing anywhere into any of them is an inner
//! link and is made again pointing at the same place in the destination; files hard-linked to
//! each other inside the tree are hard-linked to each other in the copy. Links pointing outside
//! (outer links) are unrolled, spliced or cropped. A file is never written through an existing
//! name: it is copied beside it and renamed over it, so a DeLorean backup sharing the file with
//! an earlier one (a hard link) never changes the earlier one.

use super::{lexical, resolve};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeOp {
    SmartCopy,
    SmartMirror,
    HardlinkClone,
    SymlinkClone,
}

impl TreeOp {
    pub fn label(self) -> &'static str {
        match self {
            TreeOp::SmartCopy => "Smart Copy",
            TreeOp::SmartMirror => "Smart Mirror",
            TreeOp::HardlinkClone => "Hardlink Clone",
            TreeOp::SymlinkClone => "Symbolic Link Clone",
        }
    }

    fn mirrors(self) -> bool {
        self == TreeOp::SmartMirror
    }
}

/// What happens to links pointing outside the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Outer {
    /// What they point at is copied in their place (loops cut).
    #[default]
    Unroll,
    /// They are made again pointing at the same outside target.
    Splice,
    /// They are left out.
    Crop,
}

impl Outer {
    pub fn id(self) -> &'static str {
        match self {
            Outer::Unroll => "unroll",
            Outer::Splice => "splice",
            Outer::Crop => "crop",
        }
    }

    pub fn from_id(id: &str) -> Outer {
        match id {
            "splice" => Outer::Splice,
            "crop" => Outer::Crop,
            _ => Outer::Unroll,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub outer: Outer,
    /// Symbolic links made are relative where they can be.
    pub relative: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { outer: Outer::Unroll, relative: true }
    }
}

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub copied: usize,
    pub linked: usize,
    /// Already up to date.
    pub kept: usize,
    pub removed: usize,
    pub folders: usize,
    pub errors: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

impl Report {
    fn absorb(&mut self, o: Report) {
        self.copied += o.copied;
        self.linked += o.linked;
        self.kept += o.kept;
        self.removed += o.removed;
        self.folders += o.folders;
        self.errors.extend(o.errors);
        self.cancelled |= o.cancelled;
    }
}

/// The destination of each source folder dropped into `dest_dir`: the folder of the same name.
pub fn destinations(sources: &[PathBuf], dest_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    sources.iter().filter_map(|s| Some((s.clone(), dest_dir.join(s.file_name()?)))).collect()
}

/// Runs `op` for each (source, destination) pair; `progress` is told each entry and stops the run
/// by answering false.
pub fn run(op: TreeOp, pairs: &[(PathBuf, PathBuf)], opts: Options, progress: &mut dyn FnMut(&Path) -> bool) -> Report {
    // The roots as their real paths, so that links met while walking (always under the real
    // path) compare with them.
    let roots: Vec<(PathBuf, PathBuf)> = pairs.iter().map(|(s, d)| (std::fs::canonicalize(s).unwrap_or_else(|_| lexical(s)), d.clone())).collect();
    let mut r = Run { op, opts, roots: roots.clone(), inodes: HashMap::new(), report: Report::default(), progress };
    for (s, d) in &roots {
        if r.report.cancelled {
            break;
        }
        if let Some(parent) = d.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                r.fail(d, e);
                continue;
            }
        }
        r.walk(s, d, &mut Vec::new());
    }
    r.report
}

/// The backups a DeLorean Copy of a folder named `name` made in `dest_dir`, oldest first.
pub fn backups(dest_dir: &Path, name: &str) -> Vec<PathBuf> {
    let prefix = format!("{name} - ");
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dest_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let stamp = n.strip_prefix(&prefix)?.to_string();
            (is_stamp(&stamp) && e.file_type().ok()?.is_dir()).then(|| (stamp, e.path()))
        })
        .collect();
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

/// "YYYY-MM-DD HH-MM-SS".
fn is_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 19
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 | 13 | 16 => *c == b'-',
            10 => *c == b' ',
            _ => c.is_ascii_digit(),
        })
}

/// The time stamp a DeLorean backup made now carries (local time).
pub fn stamp_now() -> String {
    gtk::glib::DateTime::now_local().and_then(|t| t.format("%Y-%m-%d %H-%M-%S")).map(|s| s.to_string()).unwrap_or_else(|_| "0000-00-00 00-00-00".into())
}

/// DeLorean Copy of `source` into `dest_dir` as `<name> - <stamp>`: the first one a Smart Copy,
/// each later one a Hardlink Clone of the newest earlier backup mirrored onto the source.
pub fn delorean(source: &Path, dest_dir: &Path, stamp: &str, opts: Options, progress: &mut dyn FnMut(&Path) -> bool) -> io::Result<(PathBuf, Report)> {
    let name = source.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no name"))?.to_string_lossy().into_owned();
    let new = dest_dir.join(format!("{name} - {stamp}"));
    if std::fs::symlink_metadata(&new).is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", new.display())));
    }
    let previous = backups(dest_dir, &name).pop();
    let mut report = Report::default();
    if let Some(prev) = previous {
        // The earlier backup as it is, its own links spliced as they are.
        report.absorb(run(TreeOp::HardlinkClone, &[(prev, new.clone())], Options { outer: Outer::Splice, relative: opts.relative }, progress));
        if !report.errors.is_empty() || report.cancelled {
            return Ok((new, report));
        }
        // What the clone made counts as kept unless the mirror replaces it.
        let mirror = run(TreeOp::SmartMirror, &[(source.to_path_buf(), new.clone())], opts, progress);
        report.kept = report.linked.saturating_sub(mirror.copied + mirror.removed);
        report.linked = 0;
        report.absorb(mirror);
    } else {
        report = run(TreeOp::SmartCopy, &[(source.to_path_buf(), new.clone())], opts, progress);
    }
    Ok((new, report))
}

struct Run<'a> {
    op: TreeOp,
    opts: Options,
    roots: Vec<(PathBuf, PathBuf)>,
    /// Source files with several names already made in the destination: (filesystem, file) →
    /// the name made first.
    inodes: HashMap<(u64, u128), PathBuf>,
    report: Report,
    progress: &'a mut dyn FnMut(&Path) -> bool,
}

impl Run<'_> {
    fn fail(&mut self, p: &Path, e: io::Error) {
        self.report.errors.push((p.to_path_buf(), e.to_string()));
    }

    /// The place in the destination of `abs` when it lies in the tree.
    fn map(&self, abs: &Path) -> Option<PathBuf> {
        self.roots.iter().find_map(|(s, d)| abs.strip_prefix(s).ok().map(|rest| if rest.as_os_str().is_empty() { d.clone() } else { d.join(rest) }))
    }

    fn walk(&mut self, src: &Path, dst: &Path, chain: &mut Vec<PathBuf>) {
        if self.report.cancelled {
            return;
        }
        if !(self.progress)(src) {
            self.report.cancelled = true;
            return;
        }
        let meta = match std::fs::symlink_metadata(src) {
            Ok(m) => m,
            Err(e) => return self.fail(src, e),
        };
        if meta.file_type().is_symlink() || super::make::is_junction(src) {
            self.link(src, dst, chain);
        } else if meta.is_dir() {
            self.dir(src, dst, chain);
        } else if meta.is_file() {
            self.file(src, dst, &meta);
        } else {
            self.fail(src, io::Error::new(io::ErrorKind::Unsupported, "not a file, folder or link: left out"));
        }
    }

    fn dir(&mut self, src: &Path, dst: &Path, chain: &mut Vec<PathBuf>) {
        if let Ok(m) = std::fs::symlink_metadata(dst) {
            if !m.is_dir() || m.file_type().is_symlink() {
                if let Err(e) = clear(dst) {
                    return self.fail(dst, e);
                }
            }
        }
        if std::fs::symlink_metadata(dst).is_err() {
            if let Err(e) = std::fs::create_dir(dst) {
                return self.fail(dst, e);
            }
            self.report.folders += 1;
        }
        let mut names: Vec<std::ffi::OsString> = match std::fs::read_dir(src) {
            Ok(rd) => rd.flatten().map(|e| e.file_name()).collect(),
            Err(e) => return self.fail(src, e),
        };
        names.sort();
        for n in &names {
            self.walk(&src.join(n), &dst.join(n), chain);
        }
        if self.op.mirrors() && !self.report.cancelled {
            let keep: HashSet<&std::ffi::OsString> = names.iter().collect();
            let extra: Vec<PathBuf> = std::fs::read_dir(dst).into_iter().flatten().flatten().filter(|e| !keep.contains(&e.file_name())).map(|e| e.path()).collect();
            for x in extra {
                match clear(&x) {
                    Ok(()) => self.report.removed += 1,
                    Err(e) => self.fail(&x, e),
                }
            }
        }
        if let Ok(m) = std::fs::metadata(src) {
            let (a, mt) = crate::osprim::times(&m);
            crate::osprim::set_times(dst, a, mt, true);
        }
    }

    fn file(&mut self, src: &Path, dst: &Path, meta: &std::fs::Metadata) {
        match self.op {
            TreeOp::HardlinkClone => {
                if same_file(src, dst) {
                    self.report.kept += 1;
                    return;
                }
                let r = clear_if_present(dst).and_then(|()| std::fs::hard_link(src, dst));
                match r {
                    Ok(()) => self.report.linked += 1,
                    Err(e) => self.fail(dst, e),
                }
            }
            TreeOp::SymlinkClone => self.symlink_to(src, dst, false),
            TreeOp::SmartCopy | TreeOp::SmartMirror => self.smart_file(src, dst, meta),
        }
    }

    fn smart_file(&mut self, src: &Path, dst: &Path, meta: &std::fs::Metadata) {
        let id = crate::osprim::identity(src, meta);
        let key = (id.nlink > 1).then_some((id.dev, id.id));
        if let Some(first) = key.and_then(|k| self.inodes.get(&k).cloned()) {
            if same_file(&first, dst) {
                self.report.kept += 1;
                return;
            }
            match clear_if_present(dst).and_then(|()| std::fs::hard_link(&first, dst)) {
                Ok(()) => self.report.linked += 1,
                Err(e) => self.fail(dst, e),
            }
            return;
        }
        let up_to_date = match std::fs::symlink_metadata(dst) {
            Ok(d) if d.is_file() && !d.file_type().is_symlink() => {
                let (src_t, dst_t) = (crate::osprim::times(meta).1, crate::osprim::times(&d).1);
                if self.op.mirrors() {
                    src_t == dst_t && meta.len() == d.len() && crate::osprim::mode_of(meta) == crate::osprim::mode_of(&d)
                } else {
                    dst_t >= src_t
                }
            }
            _ => false,
        };
        if up_to_date {
            self.report.kept += 1;
        } else {
            match copy_replace(src, dst, meta) {
                Ok(()) => self.report.copied += 1,
                Err(e) => return self.fail(dst, e),
            }
        }
        if let Some(k) = key {
            self.inodes.insert(k, dst.to_path_buf());
        }
    }

    /// A symbolic link at `dst` to `target` (an absolute path), unless `dst` already is one
    /// storing what would be stored.
    fn symlink_to(&mut self, target: &Path, dst: &Path, junction: bool) {
        let stored = if junction { lexical(target) } else { super::spelling(dst, target, self.opts.relative) };
        if std::fs::read_link(dst).is_ok_and(|t| t == stored) {
            self.report.kept += 1;
            return;
        }
        let is_dir = std::fs::metadata(target).is_ok_and(|m| m.is_dir());
        let r = clear_if_present(dst).and_then(|()| {
            if junction {
                super::make::make_at(super::LinkKind::Junction, target, dst, false)
            } else {
                crate::osprim::make_symlink(&stored, dst, is_dir)
            }
        });
        match r {
            Ok(()) => self.report.linked += 1,
            Err(e) => self.fail(dst, e),
        }
    }

    fn link(&mut self, src: &Path, dst: &Path, chain: &mut Vec<PathBuf>) {
        let raw = match std::fs::read_link(src) {
            Ok(r) => r,
            Err(e) => return self.fail(src, e),
        };
        let abs = resolve(src, &raw);
        let junction = super::make::is_junction(src);
        if let Some(mapped) = self.map(&abs) {
            return self.symlink_to(&mapped, dst, junction);
        }
        match self.opts.outer {
            Outer::Crop => {
                if self.op.mirrors() {
                    let _ = clear_if_present(dst);
                }
            }
            Outer::Splice => self.symlink_to(&abs, dst, junction),
            Outer::Unroll => {
                let (Ok(meta), Ok(real)) = (std::fs::metadata(src), std::fs::canonicalize(src)) else {
                    // A dead link has nothing to unroll.
                    return;
                };
                // A target holding (or being) something already being copied would copy itself
                // again and again: the loop is cut there.
                if chain.iter().chain(self.roots.iter().map(|(s, _)| s)).any(|c| c.starts_with(&real)) {
                    return;
                }
                chain.push(real.clone());
                if meta.is_dir() {
                    self.dir(&real, dst, chain);
                } else if meta.is_file() {
                    self.file(&real, dst, &meta);
                }
                chain.pop();
            }
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) {
        (Ok(ma), Ok(mb)) if !mb.file_type().is_symlink() => {
            let (ia, ib) = (crate::osprim::identity(a, &ma), crate::osprim::identity(b, &mb));
            ia.dev == ib.dev && ia.id == ib.id
        }
        _ => false,
    }
}

/// Removes whatever `p` is, a link as itself.
fn clear(p: &Path) -> io::Result<()> {
    let m = std::fs::symlink_metadata(p)?;
    if m.is_dir() && !m.file_type().is_symlink() && !super::make::is_junction(p) {
        std::fs::remove_dir_all(p)
    } else {
        super::make::remove_link(p)
    }
}

fn clear_if_present(p: &Path) -> io::Result<()> {
    match clear(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Copies `src` beside `dst` and renames it over `dst`: never written through the old name.
fn copy_replace(src: &Path, dst: &Path, meta: &std::fs::Metadata) -> io::Result<()> {
    let dir = dst.parent().unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".{}.fb-copy", dst.file_name().unwrap_or_default().to_string_lossy()));
    let r = (|| {
        std::fs::copy(src, &tmp)?;
        let (a, m) = crate::osprim::times(meta);
        crate::osprim::set_times(&tmp, a, m, true);
        if let Ok(d) = std::fs::symlink_metadata(dst) {
            if d.is_dir() && !d.file_type().is_symlink() {
                std::fs::remove_dir_all(dst)?;
            } else if cfg!(windows) || d.file_type().is_symlink() {
                super::make::remove_link(dst)?;
            }
        }
        std::fs::rename(&tmp, dst)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt};

    fn put(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn ino(p: &Path) -> u64 {
        std::fs::symlink_metadata(p).unwrap().ino()
    }

    fn go(op: TreeOp, src: &Path, dst: &Path, opts: Options) -> Report {
        let r = run(op, &[(src.to_path_buf(), dst.to_path_buf())], opts, &mut |_| true);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        r
    }

    /// src/ with a.txt, sub/b.txt, inner (a link into the tree), outer (a link out of it to
    /// out/), h1 and h2 hard-linked to each other, h3 hard-linked to a file outside.
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("src");
        put(&s.join("a.txt"), "a");
        put(&s.join("sub/b.txt"), "b");
        put(&t.path().join("out/o.txt"), "o");
        symlink("sub/b.txt", s.join("inner")).unwrap();
        symlink(t.path().join("out"), s.join("outer")).unwrap();
        put(&s.join("h1"), "h");
        std::fs::hard_link(s.join("h1"), s.join("sub/h2")).unwrap();
        put(&t.path().join("elsewhere"), "e");
        std::fs::hard_link(t.path().join("elsewhere"), s.join("h3")).unwrap();
        (t, s)
    }

    #[test]
    fn smart_copy_makes_inner_links_again_inside_the_copy() {
        let (t, s) = fixture();
        let d = t.path().join("copy");
        go(TreeOp::SmartCopy, &s, &d, Options::default());
        assert_eq!(std::fs::read_to_string(d.join("sub/b.txt")).unwrap(), "b");
        // The inner link points into the copy, relative as made.
        assert_eq!(std::fs::read_link(d.join("inner")).unwrap(), Path::new("sub/b.txt"));
        // Saturated hard links stay one file in the copy, not the source's.
        assert_eq!(ino(&d.join("h1")), ino(&d.join("sub/h2")));
        assert_ne!(ino(&d.join("h1")), ino(&s.join("h1")));
        // The unsaturated one is copied: its outside name is not followed.
        assert_eq!(std::fs::metadata(d.join("h3")).unwrap().nlink(), 1);
        // The outer link is unrolled by default: its target's contents in its place.
        assert!(std::fs::symlink_metadata(d.join("outer")).unwrap().is_dir());
        assert_eq!(std::fs::read_to_string(d.join("outer/o.txt")).unwrap(), "o");
    }

    #[test]
    fn outer_links_are_spliced_or_cropped_as_asked() {
        let (t, s) = fixture();
        let d = t.path().join("spliced");
        go(TreeOp::SmartCopy, &s, &d, Options { outer: Outer::Splice, relative: true });
        assert_eq!(std::fs::read_link(d.join("outer")).unwrap(), Path::new("../out"));
        let d = t.path().join("absolute");
        go(TreeOp::SmartCopy, &s, &d, Options { outer: Outer::Splice, relative: false });
        assert_eq!(std::fs::read_link(d.join("outer")).unwrap(), t.path().join("out").canonicalize().unwrap());
        let d = t.path().join("cropped");
        go(TreeOp::SmartCopy, &s, &d, Options { outer: Outer::Crop, relative: true });
        assert!(std::fs::symlink_metadata(d.join("outer")).is_err());
    }

    #[test]
    fn links_between_the_dragged_folders_are_inner_links() {
        let t = tempfile::tempdir().unwrap();
        put(&t.path().join("one/f"), "1");
        std::fs::create_dir(t.path().join("two")).unwrap();
        symlink("../one/f", t.path().join("two/to-one")).unwrap();
        let dest = t.path().join("dest");
        let pairs = destinations(&[t.path().join("one"), t.path().join("two")], &dest);
        let r = run(TreeOp::SmartCopy, &pairs, Options { outer: Outer::Crop, relative: false }, &mut |_| true);
        assert!(r.errors.is_empty());
        assert_eq!(std::fs::read_link(dest.join("two/to-one")).unwrap(), dest.join("one/f"));
    }

    #[test]
    fn a_loop_of_outer_links_is_cut() {
        let t = tempfile::tempdir().unwrap();
        put(&t.path().join("src/f"), "f");
        put(&t.path().join("out/g"), "g");
        symlink(t.path().join("out"), t.path().join("src/away")).unwrap();
        symlink(t.path().join("out"), t.path().join("out/again")).unwrap();
        let d = t.path().join("d");
        go(TreeOp::SmartCopy, &t.path().join("src"), &d, Options::default());
        assert_eq!(std::fs::read_to_string(d.join("away/g")).unwrap(), "g");
        assert!(std::fs::symlink_metadata(d.join("away/again")).is_err(), "the loop ends");
    }

    #[test]
    fn smart_copy_copies_only_what_is_newer() {
        let (t, s) = fixture();
        let d = t.path().join("copy");
        go(TreeOp::SmartCopy, &s, &d, Options::default());
        std::fs::write(d.join("a.txt"), "edited in the copy").unwrap();
        let r = go(TreeOp::SmartCopy, &s, &d, Options::default());
        assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), "edited in the copy");
        assert_eq!(r.copied, 0, "{r:?}");
    }

    #[test]
    fn smart_mirror_makes_the_destination_equal() {
        let (t, s) = fixture();
        let d = t.path().join("mirror");
        go(TreeOp::SmartCopy, &s, &d, Options::default());
        put(&d.join("stray.txt"), "not in the source");
        std::fs::remove_file(s.join("a.txt")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        put(&s.join("sub/b.txt"), "b again");
        let r = go(TreeOp::SmartMirror, &s, &d, Options::default());
        assert!(std::fs::symlink_metadata(d.join("stray.txt")).is_err());
        assert!(std::fs::symlink_metadata(d.join("a.txt")).is_err());
        assert_eq!(std::fs::read_to_string(d.join("sub/b.txt")).unwrap(), "b again");
        assert_eq!(r.removed, 2, "{r:?}");
    }

    #[test]
    fn a_hardlink_clone_shares_every_file_with_its_source() {
        let (t, s) = fixture();
        let d = t.path().join("clone");
        go(TreeOp::HardlinkClone, &s, &d, Options::default());
        assert_eq!(ino(&d.join("a.txt")), ino(&s.join("a.txt")));
        assert_eq!(ino(&d.join("sub/b.txt")), ino(&s.join("sub/b.txt")));
        assert_eq!(std::fs::read_link(d.join("inner")).unwrap(), Path::new("sub/b.txt"));
    }

    #[test]
    fn a_symbolic_link_clone_points_every_file_at_its_source() {
        let (t, s) = fixture();
        let d = t.path().join("clone");
        go(TreeOp::SymlinkClone, &s, &d, Options::default());
        assert_eq!(std::fs::read_link(d.join("a.txt")).unwrap(), Path::new("../src/a.txt"));
        assert_eq!(std::fs::read_to_string(d.join("sub/b.txt")).unwrap(), "b");
        assert!(std::fs::symlink_metadata(d.join("sub")).unwrap().is_dir());
    }

    #[test]
    fn delorean_backups_share_unchanged_files_and_keep_old_ones_intact() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("work");
        put(&s.join("same.txt"), "same");
        put(&s.join("changes.txt"), "v1");
        put(&s.join("goes.txt"), "bye");
        let b = t.path().join("backups");
        let (first, r1) = delorean(&s, &b, "2026-11-14 10-00-00", Options::default(), &mut |_| true).unwrap();
        assert!(r1.errors.is_empty());
        assert_eq!(first, b.join("work - 2026-11-14 10-00-00"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        put(&s.join("changes.txt"), "v2");
        std::fs::remove_file(s.join("goes.txt")).unwrap();
        put(&s.join("new.txt"), "new");
        let (second, r2) = delorean(&s, &b, "2026-11-14 11-00-00", Options::default(), &mut |_| true).unwrap();
        assert!(r2.errors.is_empty(), "{:?}", r2.errors);
        // Unchanged: one file shared by both backups.
        assert_eq!(ino(&first.join("same.txt")), ino(&second.join("same.txt")));
        // Changed: copied anew; the earlier backup keeps its own.
        assert_eq!(std::fs::read_to_string(second.join("changes.txt")).unwrap(), "v2");
        assert_eq!(std::fs::read_to_string(first.join("changes.txt")).unwrap(), "v1");
        assert_ne!(ino(&first.join("changes.txt")), ino(&second.join("changes.txt")));
        // Removed only from the new backup.
        assert!(std::fs::symlink_metadata(second.join("goes.txt")).is_err());
        assert_eq!(std::fs::read_to_string(first.join("goes.txt")).unwrap(), "bye");
        assert_eq!(std::fs::read_to_string(second.join("new.txt")).unwrap(), "new");
        assert_eq!(backups(&b, "work"), [first, second]);
        assert!(delorean(&s, &b, "2026-11-14 11-00-00", Options::default(), &mut |_| true).is_err(), "a stamp is used once");
    }

    #[test]
    fn a_run_stops_when_asked() {
        let (t, s) = fixture();
        let mut n = 0;
        let r = run(TreeOp::SmartCopy, &[(s, t.path().join("d"))], Options::default(), &mut |_| {
            n += 1;
            n < 3
        });
        assert!(r.cancelled);
    }

    #[test]
    fn stamps_are_recognised_exactly() {
        assert!(is_stamp("2026-11-14 10-00-00"));
        assert!(!is_stamp("2026-11-14 10:00:00"));
        assert!(!is_stamp("2026-11-14"));
        assert!(is_stamp(&stamp_now()));
    }
}
