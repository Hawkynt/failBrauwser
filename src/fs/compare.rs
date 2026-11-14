//! Comparing two folders, however deep: what is only on one side, what is newer on one
//! side, what differs; and copying the missing and newer ones across (never deleting).

use crate::ops::job::JobCtx;
use crate::osprim;
use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Times this close are the same (FAT keeps two seconds, some copies round).
const SAME_TIME: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    OnlyLeft,
    OnlyRight,
    Same,
    LeftNewer,
    RightNewer,
    /// Same time, different size or contents.
    Differs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub size: u64,
    pub mtime: i64,
    pub dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The path below both folders.
    pub rel: PathBuf,
    pub left: Option<Meta>,
    pub right: Option<Meta>,
    pub state: State,
}

/// Everything below `root`: files (and links) by relative path; a folder is listed and
/// entered.
fn walk(root: &Path, cancel: &AtomicBool, out: &mut BTreeMap<PathBuf, Meta>) -> io::Result<()> {
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let Ok(rd) = std::fs::read_dir(root.join(&rel)) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            let r = rel.join(e.file_name());
            if m.is_dir() {
                stack.push(r.clone());
            }
            out.insert(r, Meta { size: if m.is_dir() { 0 } else { m.len() }, mtime: osprim::times(&m).1.0, dir: m.is_dir() });
        }
    }
    Ok(())
}

fn same_contents(a: &Path, b: &Path, cancel: &AtomicBool) -> io::Result<bool> {
    let (mut fa, mut fb) = (std::fs::File::open(a)?, std::fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let na = read_full(&mut fa, &mut ba)?;
        let nb = read_full(&mut fb, &mut bb)?;
        if na != nb || ba[..na] != bb[..nb] {
            return Ok(false);
        }
        if na == 0 {
            return Ok(true);
        }
    }
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// Compares `left` with `right`. A folder on one side only is one row (its contents are
/// not listed). With `by_content`, files of the same size are compared byte by byte.
pub fn compare(left: &Path, right: &Path, by_content: bool, cancel: &AtomicBool) -> io::Result<Vec<Row>> {
    let (mut l, mut r) = (BTreeMap::new(), BTreeMap::new());
    walk(left, cancel, &mut l)?;
    walk(right, cancel, &mut r)?;
    let mut keys: Vec<&PathBuf> = l.keys().chain(r.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut rows = Vec::new();
    let mut skip_below: Option<PathBuf> = None;
    for rel in keys {
        if skip_below.as_ref().is_some_and(|s| rel.starts_with(s)) {
            continue;
        }
        let (a, b) = (l.get(rel).copied(), r.get(rel).copied());
        let state = match (a, b) {
            (Some(a), None) => {
                if a.dir {
                    skip_below = Some(rel.clone());
                }
                State::OnlyLeft
            }
            (None, Some(b)) => {
                if b.dir {
                    skip_below = Some(rel.clone());
                }
                State::OnlyRight
            }
            (Some(a), Some(b)) if a.dir && b.dir => continue,
            (Some(a), Some(b)) => {
                let same_time = (a.mtime - b.mtime).abs() <= SAME_TIME;
                let same_size = a.size == b.size && a.dir == b.dir;
                let same = if same_size && by_content && !a.dir {
                    same_contents(&left.join(rel), &right.join(rel), cancel)?
                } else {
                    same_size && same_time
                };
                if same {
                    State::Same
                } else if same_time {
                    State::Differs
                } else if a.mtime > b.mtime {
                    State::LeftNewer
                } else {
                    State::RightNewer
                }
            }
            (None, None) => continue,
        };
        rows.push(Row { rel: rel.clone(), left: a, right: b, state });
    }
    Ok(rows)
}

/// Whether copying `row` from one side to the other makes sense: it is missing there or
/// older there (`to_right`: from left to right).
pub fn copies(row: &Row, to_right: bool) -> bool {
    match row.state {
        State::OnlyLeft | State::LeftNewer => to_right,
        State::OnlyRight | State::RightNewer => !to_right,
        State::Differs => true,
        State::Same => false,
    }
}

/// Copies `rels` from `from` to `to`, replacing what is there (in one step: a copy next to
/// it, renamed over it) and keeping the modification times. Folders go with everything in
/// them. Returns what was written.
pub fn copy_across(ctx: &JobCtx, from: &Path, to: &Path, rels: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
    for rel in rels {
        let src = from.join(rel);
        let m = std::fs::symlink_metadata(&src)?;
        if m.is_dir() {
            let mut all = BTreeMap::new();
            walk(&src, &AtomicBool::new(false), &mut all)?;
            std::fs::create_dir_all(to.join(rel))?;
            for (r, meta) in all {
                if meta.dir {
                    std::fs::create_dir_all(to.join(rel).join(&r))?;
                } else {
                    files.push((src.join(&r), to.join(rel).join(&r)));
                }
            }
        } else {
            files.push((src, to.join(rel)));
        }
    }
    let total: u64 = files.iter().filter_map(|(s, _)| std::fs::metadata(s).ok()).map(|m| m.len()).sum();
    ctx.bytes_total.fetch_add(total, Ordering::Relaxed);
    ctx.files_total.fetch_add(files.len() as u64, Ordering::Relaxed);
    ctx.set_phase(crate::ops::job::Phase::Working);
    let mut written = Vec::new();
    for (src, dst) in files {
        if ctx.is_cancelled() {
            return Err(crate::ops::fastcopy::cancelled());
        }
        ctx.wait_while_paused();
        ctx.set_current(dst.file_name().unwrap_or_default().to_string_lossy());
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = dst.with_file_name(format!(".fb-compare-{}-{}", std::process::id(), written.len()));
        let res = (|| -> io::Result<()> {
            let meta = std::fs::symlink_metadata(&src)?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&src)?;
                osprim::make_symlink(&target, &tmp, std::fs::metadata(&target).is_ok_and(|m| m.is_dir()))?;
            } else {
                let mut r = std::fs::File::open(&src)?;
                let mut w = std::fs::File::create(&tmp)?;
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let n = r.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    io::Write::write_all(&mut w, &buf[..n])?;
                    ctx.add_bytes(n as u64);
                }
                w.set_permissions(meta.permissions())?;
                w.set_modified(meta.modified()?)?;
            }
            std::fs::rename(&tmp, &dst)
        })();
        if let Err(e) = res {
            let _ = std::fs::remove_file(&tmp);
            ctx.record_error(format!("{}: {e}", dst.display()));
            continue;
        }
        ctx.files_done.fetch_add(1, Ordering::Relaxed);
        written.push(dst);
    }
    ctx.set_phase(crate::ops::job::Phase::Done);
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    // Stable separator for the expected lists; Windows displays backslashes.
    fn rel_key(rel: &Path) -> String {
        rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
    }

    fn put(p: &Path, text: &str, age_secs: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_secs)).unwrap();
    }

    #[test]
    fn differences_and_copying_across() {
        let dir = tempfile::tempdir().unwrap();
        let (l, r) = (dir.path().join("l"), dir.path().join("r"));
        put(&l.join("same.txt"), "same", 1000);
        put(&r.join("same.txt"), "same", 1000);
        put(&l.join("only-left.txt"), "l", 10);
        put(&r.join("only-right.txt"), "r", 10);
        put(&l.join("sub/newer-left.txt"), "new", 10);
        put(&r.join("sub/newer-left.txt"), "old", 1000);
        put(&l.join("newer-right.txt"), "old", 1000);
        put(&r.join("newer-right.txt"), "newer", 10);
        put(&l.join("photos/a.jpg"), "a", 10);
        put(&l.join("photos/b.jpg"), "b", 10);
        // Same size and time, other bytes: only a content check finds it.
        put(&l.join("sneaky.bin"), "AAAA", 500);
        put(&r.join("sneaky.bin"), "BBBB", 500);
        let no = AtomicBool::new(false);
        let rows = compare(&l, &r, false, &no).unwrap();
        let show: Vec<(String, State)> = rows.iter().map(|x| (rel_key(&x.rel), x.state)).collect();
        assert_eq!(
            show,
            [
                ("newer-right.txt".into(), State::RightNewer),
                ("only-left.txt".into(), State::OnlyLeft),
                ("only-right.txt".into(), State::OnlyRight),
                ("photos".into(), State::OnlyLeft),
                ("same.txt".into(), State::Same),
                ("sneaky.bin".into(), State::Same),
                ("sub/newer-left.txt".into(), State::LeftNewer),
            ]
        );
        let by_content = compare(&l, &r, true, &no).unwrap();
        assert_eq!(by_content.iter().find(|x| x.rel == Path::new("sneaky.bin")).unwrap().state, State::Differs);

        // Copy what is missing or newer on the left over to the right.
        let to_right: Vec<PathBuf> = rows.iter().filter(|x| copies(x, true)).map(|x| x.rel.clone()).collect();
        let ctx = JobCtx::new();
        copy_across(&ctx, &l, &r, &to_right).unwrap();
        assert!(ctx.errors().is_empty());
        assert_eq!(std::fs::read_to_string(r.join("sub/newer-left.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(r.join("photos/b.jpg")).unwrap(), "b");
        assert_eq!(std::fs::read_to_string(r.join("newer-right.txt")).unwrap(), "newer");
        // Now only the right side has anything the left lacks or has older.
        let after = compare(&l, &r, false, &no).unwrap();
        assert!(after.iter().all(|x| !copies(x, true) || x.state == State::Same), "{after:?}");
        assert_eq!(after.iter().filter(|x| x.state != State::Same).count(), 2);
    }
}
