//! Copying and moving files and folder trees.
//!
//! A transfer first plans the whole tree (so progress has a total), creates all folders,
//! then copies files with several workers at once — many small files are bound by
//! per-file latency, not bandwidth, so parallel requests help a lot on SSDs and network
//! shares. Folders get their final permissions and times at the end, because writing
//! into a folder changes its time.
//!
//! GVfs shares (`smb://`, `sftp://`, … under `$XDG_RUNTIME_DIR/gvfs`) are written through
//! GIO, one file at a time: gvfsd-fuse deadlocks under parallel writes to a slow backend
//! such as SMB, and then every access to the share hangs.

use super::device::{DeviceClass, classify};
use super::fastcopy::{self, copy_contents, is_cancelled_error};
use super::job::{Answer, JobCtx, Phase, Question};
use super::names::free_name;
use crate::osprim;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
#[cfg(unix)]
use std::sync::OnceLock;
use gtk::gio;
use gtk::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Copy,
    Move,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Pace writes to slow (removable, rotational) targets.
    pub smooth_writes: bool,
    /// Override the automatic number of workers.
    pub workers: Option<usize>,
    /// Read every copied file back from the device and compare it with the original.
    pub verify: bool,
}

#[derive(Debug)]
struct FileTask {
    src: PathBuf,
    dst: PathBuf,
    size: u64,
    mode: u32,
    /// Windows HIDDEN|SYSTEM|READONLY mask of the source (0 on Unix).
    attrs: u32,
    atime: (i64, i64),
    mtime: (i64, i64),
    ino: u64,
}

#[derive(Debug)]
struct DirTask {
    dst: PathBuf,
    /// Only Unix applies mode bits to folders; Windows carries `attrs`.
    #[cfg(unix)]
    mode: u32,
    attrs: u32,
    atime: (i64, i64),
    mtime: (i64, i64),
    /// We created it (so we own its metadata); false when merging into an existing folder.
    created: bool,
}

#[derive(Default)]
struct Plan {
    dirs: Vec<DirTask>,
    files: Vec<FileTask>,
    /// (source link, its target, destination)
    links: Vec<(PathBuf, PathBuf, PathBuf)>,
    /// Source folders to remove after a cross-device move, deepest first.
    move_leftovers: Vec<PathBuf>,
}

/// Sort key giving roughly sequential read order in the plan.
#[cfg(unix)]
fn inode_of(_src: &Path, meta: &fs::Metadata) -> u64 {
    meta.ino()
}

#[cfg(windows)]
fn inode_of(src: &Path, meta: &fs::Metadata) -> u64 {
    osprim::identity(src, meta).id as u64
}

#[cfg(unix)]
fn umask() -> u32 {
    static UMASK: OnceLock<u32> = OnceLock::new();
    *UMASK.get_or_init(|| {
        fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Umask:").map(|v| v.trim().to_string())))
            .and_then(|v| u32::from_str_radix(&v, 8).ok())
            .unwrap_or(0o022)
    })
}

fn times(m: &fs::Metadata) -> ((i64, i64), (i64, i64)) {
    osprim::times(m)
}

/// rename(2) that refuses to replace an existing target.
pub use crate::osprim::rename_noreplace;

/// Renames `from` to `to`, replacing what is there: a file in one atomic step, a folder (or
/// a file over a folder) by swapping the two and deleting the old one afterwards.
pub fn rename_replacing(from: &Path, to: &Path) -> io::Result<()> {
    let target = match fs::symlink_metadata(to) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return fs::rename(from, to),
        Err(e) => return Err(e),
    };
    let source = fs::symlink_metadata(from)?;
    // "a.txt" to "A.txt" on a case-insensitive drive: the same file, just a new spelling.
    if osprim::identity(from, &source) == osprim::identity(to, &target) {
        return fs::rename(from, to);
    }
    if !source.is_dir() && !target.is_dir() {
        return fs::rename(from, to);
    }
    match osprim::rename_swap(from, to) {
        Ok(()) => return remove_any(from), // the old one now sits under the source's name
        Err(err) if osprim::is_rename_flag_unsupported(&err) => {}
        Err(err) => return Err(err),
    }
    // Filesystems that cannot swap (FUSE, some network ones): delete first.
    remove_any(to)?;
    fs::rename(from, to)
}

fn remove_any(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Runs `op` until it succeeds, the user skips it (`Ok(None)`) or cancels (`Err`).
/// Transient sharing blocks retry among themselves first — an AV scan or preview
/// handler releases within a blink and should not interrupt the user.
fn with_retry<T>(ctx: &JobCtx, path: &Path, mut op: impl FnMut() -> io::Result<T>) -> io::Result<Option<T>> {
    let mut sharing = 0u32;
    loop {
        match op() {
            Ok(v) => return Ok(Some(v)),
            Err(e) if is_cancelled_error(&e) => return Err(e),
            Err(e) if fastcopy::wait_sharing_retry(&e, &mut sharing) => continue,
            Err(e) => {
                sharing = 0;
                match ctx.ask(Question::Error { path: path.to_path_buf(), message: e.to_string() }) {
                    Answer::Retry => continue,
                    Answer::Cancel => return Err(fastcopy::cancelled()),
                    _ => {
                        ctx.record_error(format!("{}: {}", path.display(), e));
                        return Ok(None);
                    }
                }
            },
        }
    }
}

/// What to do about an existing destination.
enum Resolve {
    /// Write to this path (the original, or a free name).
    Into(PathBuf),
    /// Replace the existing file at this path once the new one is complete.
    Replace(PathBuf),
    /// Merge into the existing folder.
    Merge,
    Skip,
}

fn resolve_conflict(ctx: &JobCtx, src: &Path, src_meta: &fs::Metadata, dst: &Path) -> io::Result<Resolve> {
    let dst_meta = match fs::symlink_metadata(dst) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Resolve::Into(dst.to_path_buf())),
        Err(e) => return Err(e),
    };
    if src_meta.is_dir() && dst_meta.is_dir() {
        return Ok(Resolve::Merge);
    }
    let q = Question::Conflict {
        src: src.to_path_buf(),
        dst: dst.to_path_buf(),
        src_size: src_meta.len(),
        dst_size: dst_meta.len(),
        src_mtime: osprim::times(&src_meta).1.0,
        dst_mtime: osprim::times(&dst_meta).1.0,
        dst_is_dir: dst_meta.is_dir(),
    };
    match ctx.ask(q) {
        Answer::Replace if src_meta.is_file() && dst_meta.is_file() => Ok(Resolve::Replace(dst.to_path_buf())),
        Answer::Replace => {
            remove_any(dst)?;
            Ok(Resolve::Into(dst.to_path_buf()))
        }
        Answer::KeepBoth => {
            let parent = dst.parent().unwrap_or(Path::new("/"));
            let name = dst.file_name().unwrap_or_default();
            Ok(Resolve::Into(parent.join(free_name(parent, name, None))))
        }
        Answer::Cancel => Err(fastcopy::cancelled()),
        _ => Ok(Resolve::Skip),
    }
}

/// Copies or moves `sources` into `dest_dir`. Returns the top-level destinations written.
pub fn transfer(ctx: &JobCtx, sources: &[PathBuf], dest_dir: &Path, mode: Mode, opts: &Options) -> io::Result<Vec<PathBuf>> {
    transfer_pairs(ctx, sources, dest_dir, mode, opts).map(|pairs| pairs.into_iter().map(|(_, dst)| dst).collect())
}

/// Like [`transfer`], with each top-level item it made paired with its source (for undo).
pub fn transfer_pairs(ctx: &JobCtx, sources: &[PathBuf], dest_dir: &Path, mode: Mode, opts: &Options) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    ctx.set_phase(Phase::Preparing);
    // A share that stopped answering would hang the whole job (and Cancel with it).
    super::gvfs::check(dest_dir, super::gvfs::TIMEOUT)?;
    if let Some(dir) = sources.first().and_then(|s| s.parent()) {
        super::gvfs::check(dir, super::gvfs::TIMEOUT)?;
    }
    fs::metadata(dest_dir)?; // fail before any work if the destination folder is gone
    let dest_class = classify(dest_dir);
    // The GIO backend copy exists to skip gvfsd-fuse; Windows UNC paths are native to the
    // redirector, so the normal engine (with its own timeout checks) serves them.
    #[cfg(unix)]
    let via_gio = is_gvfs_path(dest_dir);
    #[cfg(windows)]
    let via_gio = false;
    let mut plan = Plan::default();
    let mut tops = Vec::new();

    for src in sources {
        if ctx.is_cancelled() {
            return Err(fastcopy::cancelled());
        }
        let Some(name) = src.file_name() else { continue };
        let Some(src_meta) = with_retry(ctx, src, || fs::symlink_metadata(src))? else { continue };
        if src_meta.is_dir() && dest_dir.starts_with(src) {
            ctx.record_error(format!("Cannot put “{}” into itself.", src.display()));
            continue;
        }
        let mut dst = dest_dir.join(name);
        if dst == *src {
            if mode == Mode::Move {
                continue; // Moving onto itself: nothing to do.
            }
            dst = dest_dir.join(free_name(dest_dir, name, Some("copy")));
        }

        // Moves within one filesystem are a rename, whatever the size.
        if mode == Mode::Move && osprim::same_filesystem(src, dest_dir) {
            ctx.set_current(name.to_string_lossy());
            move_by_rename(ctx, src, &src_meta, &dst)?;
            tops.push((src.clone(), dst));
            continue;
        }

        let dst = match resolve_conflict(ctx, src, &src_meta, &dst)? {
            // The file task meets the existing file again and replaces it then.
            Resolve::Into(p) | Resolve::Replace(p) => p,
            Resolve::Merge => dst,
            Resolve::Skip => continue,
        };
        ctx.set_current(format!("Preparing {}", name.to_string_lossy()));
        plan_tree(ctx, src, &src_meta, &dst, mode, &mut plan)?;
        tops.push((src.clone(), dst));
    }

    let total: u64 = plan.files.iter().map(|f| f.size).sum();
    // Verifying reads everything once more: the progress bar covers both passes.
    ctx.bytes_total.fetch_add(if opts.verify { total * 2 } else { total }, Ordering::Relaxed);
    ctx.files_total.fetch_add(plan.files.len() as u64 + plan.links.len() as u64, Ordering::Relaxed);
    ctx.set_phase(Phase::Working);

    // Folders first, in order, so every file has its parent.
    for d in &mut plan.dirs {
        if ctx.is_cancelled() {
            return Err(fastcopy::cancelled());
        }
        if !d.created {
            continue;
        }
        let res = with_retry(ctx, &d.dst, || match fs::create_dir(&d.dst) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && d.dst.is_dir() => Ok(false),
            Err(e) => Err(e),
            Ok(()) => Ok(true),
        })?;
        d.created = res.unwrap_or(false);
    }

    let throttle = opts.smooth_writes && dest_class.is_slow();
    let src_class = sources.first().and_then(|s| s.parent()).map(classify).unwrap_or(DeviceClass::Fast);
    let workers = if via_gio { 1 } else { opts.workers.unwrap_or_else(|| dest_class.workers().min(src_class.workers())).max(1) };
    if src_class == DeviceClass::Rotational {
        // Reading in inode order keeps a spinning source from seeking back and forth.
        plan.files.sort_by_key(|f| f.ino);
    } else {
        // Spread the workers over different folders: creating files in one folder is
        // serialized by the filesystem, different folders proceed in parallel.
        interleave_by_folder(&mut plan.files);
    }
    copy_files(ctx, &plan.files, mode, throttle, workers, opts.verify, via_gio)?;

    for (src, target, dst) in &plan.links {
        if ctx.is_cancelled() {
            return Err(fastcopy::cancelled());
        }
        let made = if via_gio {
            match gio::File::for_path(dst).make_symbolic_link(target, gio::Cancellable::NONE) {
                Ok(()) => Some(()),
                Err(e) => {
                    // SMB shares hold no links: say so and go on with the rest.
                    ctx.record_error(format!("{}: the link could not be created on the share ({e})", dst.display()));
                    None
                }
            }
        } else {
            with_retry(ctx, dst, || osprim::make_symlink(target, dst, fs::metadata(target).is_ok_and(|m| m.is_dir())))?
        };
        if made.is_some() && mode == Mode::Move {
            let _ = fs::remove_file(src);
        }
        ctx.files_done.fetch_add(1, Ordering::Relaxed);
    }

    // Folder metadata last (deepest first); then clean up after cross-device moves.
    for d in plan.dirs.iter().rev().filter(|d| d.created) {
        #[cfg(unix)]
        let _ = fs::set_permissions(&d.dst, fs::Permissions::from_mode(d.mode & 0o7777));
        osprim::set_attrs(&d.dst, d.attrs); // HIDDEN/SYSTEM/READONLY on Windows, no-op on Unix
        osprim::set_times(&d.dst, d.atime, d.mtime, true);
    }
    if mode == Mode::Move {
        for dir in &plan.move_leftovers {
            // Fails harmlessly where something was skipped and stayed behind.
            let _ = fs::remove_dir(dir);
        }
    }

    if dest_class.is_slow() {
        // Report "done" only when the data is on the device: unplugging is safe right after.
        ctx.set_phase(Phase::Flushing);
        ctx.set_current("Writing cached data to the drive…");
        let _ = fastcopy::syncfs(dest_dir);
    }
    ctx.set_phase(Phase::Done);
    Ok(tops)
}

/// Moves within one filesystem: rename, merging into existing folders entry by entry.
fn move_by_rename(ctx: &JobCtx, src: &Path, src_meta: &fs::Metadata, dst: &Path) -> io::Result<()> {
    match rename_noreplace(src, dst) {
        Ok(()) => {
            ctx.files_done.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        Err(e) if osprim::is_exists(&e) || osprim::is_dir_not_empty(&e) => {}
        Err(e) if osprim::is_rename_flag_unsupported(&e) => {
            // Old kernels or filesystems without RENAME_NOREPLACE: check, then rename.
            if fs::symlink_metadata(dst).is_err() {
                return with_retry(ctx, src, || fs::rename(src, dst)).map(|_| ());
            }
        }
        Err(e) => {
            return with_retry(ctx, src, || Err::<(), _>(io::Error::new(e.kind(), e.to_string()))).map(|_| ());
        }
    }
    match resolve_conflict(ctx, src, src_meta, dst)? {
        Resolve::Skip => Ok(()),
        // rename(2) replaces a file atomically.
        Resolve::Into(p) | Resolve::Replace(p) => with_retry(ctx, src, || fs::rename(src, &p)).map(|_| ()),
        Resolve::Merge => {
            let entries: Vec<_> = fs::read_dir(src)?.filter_map(Result::ok).collect();
            for e in entries {
                if ctx.is_cancelled() {
                    return Err(fastcopy::cancelled());
                }
                let child = e.path();
                let Ok(m) = fs::symlink_metadata(&child) else { continue };
                move_by_rename(ctx, &child, &m, &dst.join(e.file_name()))?;
            }
            let _ = fs::remove_dir(src);
            Ok(())
        }
    }
}

fn plan_tree(ctx: &JobCtx, src: &Path, meta: &fs::Metadata, dst: &Path, mode: Mode, plan: &mut Plan) -> io::Result<()> {
    if ctx.is_cancelled() {
        return Err(fastcopy::cancelled());
    }
    let ft = meta.file_type();
    let (atime, mtime) = times(meta);
    if ft.is_symlink() {
        let Some(target) = with_retry(ctx, src, || fs::read_link(src))? else { return Ok(()) };
        plan.links.push((src.to_path_buf(), target, dst.to_path_buf()));
    } else if ft.is_dir() {
        let exists = dst.is_dir();
        plan.dirs.push(DirTask { dst: dst.to_path_buf(), attrs: osprim::attrs_of(src), atime, mtime, created: !exists, #[cfg(unix)] mode: osprim::mode_of(&meta) });
        let Some(entries) = with_retry(ctx, src, || fs::read_dir(src)?.collect::<io::Result<Vec<_>>>())? else { return Ok(()) };
        for e in entries {
            let child = e.path();
            let Some(m) = with_retry(ctx, &child, || fs::symlink_metadata(&child))? else { continue };
            plan_tree(ctx, &child, &m, &dst.join(e.file_name()), mode, plan)?;
        }
        if mode == Mode::Move {
            plan.move_leftovers.push(src.to_path_buf());
        }
    } else if ft.is_file() {
        plan.files.push(FileTask { src: src.to_path_buf(), dst: dst.to_path_buf(), size: meta.len(), mode: osprim::mode_of(&meta), attrs: osprim::attrs_of(src), atime, mtime, ino: inode_of(src, meta) });
    } else {
        ctx.record_error(format!("{}: special files are not copied", src.display()));
    }
    Ok(())
}

/// Reorders files round-robin over their folders: first file of every folder, then the
/// second of every folder, and so on.
fn interleave_by_folder(files: &mut Vec<FileTask>) {
    let mut slot: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();
    let mut keyed: Vec<(u32, usize, FileTask)> = Vec::with_capacity(files.len());
    let mut folder_ids: std::collections::HashMap<PathBuf, usize> = std::collections::HashMap::new();
    for f in files.drain(..) {
        let parent = f.dst.parent().map(Path::to_path_buf).unwrap_or_default();
        let n = folder_ids.len();
        let folder = *folder_ids.entry(parent.clone()).or_insert(n);
        let s = slot.entry(parent).or_insert(0);
        keyed.push((*s, folder, f));
        *s += 1;
    }
    keyed.sort_by_key(|(s, folder, _)| (*s, *folder));
    files.extend(keyed.into_iter().map(|(_, _, f)| f));
}

fn copy_files(ctx: &JobCtx, files: &[FileTask], mode: Mode, throttle: bool, workers: usize, verify: bool, via_gio: bool) -> io::Result<()> {
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<io::Error>> = Mutex::new(None);
    let workers = workers.min(files.len()).max(1);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                let mut buf = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(task) = files.get(i) else { break };
                    ctx.wait_while_paused();
                    if ctx.is_cancelled() {
                        break;
                    }
                    ctx.set_current(task.src.file_name().unwrap_or_default().to_string_lossy());
                    match copy_one(ctx, task, mode, throttle, verify, via_gio, &mut buf) {
                        Ok(()) => {
                            ctx.files_done.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            let mut f = failure.lock().unwrap();
                            if f.is_none() {
                                *f = Some(e);
                            }
                            ctx.cancel();
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }
    if ctx.is_cancelled() {
        return Err(fastcopy::cancelled());
    }
    Ok(())
}

/// Copies one file. Errors the user chose to skip return `Ok`.
fn copy_one(ctx: &JobCtx, t: &FileTask, mode: Mode, throttle: bool, verify: bool, via_gio: bool, buf: &mut Vec<u8>) -> io::Result<()> {
    let src_meta = match fs::symlink_metadata(&t.src) {
        Ok(m) => m,
        Err(e) => {
            ctx.record_error(format!("{}: {}", t.src.display(), e));
            return Ok(());
        }
    };
    let (dst, replace) = match resolve_conflict(ctx, &t.src, &src_meta, &t.dst)? {
        Resolve::Into(p) => (p, None),
        Resolve::Replace(p) => {
            // Write next to it under a temporary name; rename over it when complete.
            let parent = p.parent().unwrap_or(Path::new("/")).to_path_buf();
            let mut tmp = std::ffi::OsString::from(".");
            tmp.push(p.file_name().unwrap_or_default());
            tmp.push(".fbpart");
            let tmp = if fs::symlink_metadata(parent.join(&tmp)).is_ok() { parent.join(free_name(&parent, &tmp, None)) } else { parent.join(tmp) };
            (tmp, Some(p))
        }
        Resolve::Merge | Resolve::Skip => {
            ctx.add_bytes(t.size);
            return Ok(());
        }
    };
    let done = with_retry(ctx, &t.src, || {
        let mut counted = 0;
        let mut r = if via_gio { write_file_gio(ctx, t, &dst, &mut counted) } else { write_file(ctx, t, &dst, throttle, buf, &mut counted) };
        if r.is_ok() && verify {
            r = verify_copy(ctx, &t.src, &dst, &mut counted);
            if r.is_err() {
                let _ = fs::remove_file(&dst);
            }
        }
        if r.is_err() {
            // A retry starts over: take back what the failed attempt counted.
            ctx.bytes_done.fetch_sub(counted, Ordering::Relaxed);
        }
        r
    })?;
    if done.is_none() {
        ctx.add_bytes(t.size);
        return Ok(());
    }
    if let Some(target) = replace {
        if let Err(e) = fs::rename(&dst, &target) {
            let _ = fs::remove_file(&dst);
            ctx.record_error(format!("{}: {}", target.display(), e));
            return Ok(());
        }
    }
    if mode == Mode::Move {
        let _ = with_retry(ctx, &t.src, || fs::remove_file(&t.src))?;
    }
    Ok(())
}

/// Compares a copy with its original, reading the copy from the device rather than from
/// the page cache. On Unix the copy's pages are flushed and dropped first; on Windows
/// the sector-aligned bulk is read back through `FILE_FLAG_NO_BUFFERING`, which cannot
/// be served by cache at all, and only the sub-sector tail reads normally.
pub fn verify_copy(ctx: &JobCtx, src: &Path, dst: &Path, counted: &mut u64) -> io::Result<()> {
    use std::io::Read;
    ctx.set_phase(Phase::Working);
    let mut a = fs::File::open(src)?;
    let mut b = fs::File::open(dst)?;
    // Drop the copy's cached pages so the compare reads from the device.
    #[cfg(unix)]
    unsafe {
        libc::fdatasync(b.as_raw_fd());
        libc::posix_fadvise(b.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
    }
    let mut x = vec![0u8; 1 << 20];
    let mut y = vec![0u8; 1 << 20];

    #[cfg(windows)]
    {
        use std::io::Seek;
        let sector = fastcopy::sector_size(dst) as usize;
        let len = b.metadata()?.len() as usize;
        let bulk = len - len % sector;
        if bulk > 0 {
            let mut uncached = fastcopy::UncachedReader::open(dst)?;
            let mut off = 0usize;
            while off < bulk {
                ctx.wait_while_paused();
                if ctx.is_cancelled() {
                    return Err(fastcopy::cancelled());
                }
                let want = x.len().min(bulk - off);
                let n = read_full(&mut a, &mut x[..want])?;
                let m = uncached.read(&mut y[..want])?;
                if m == 0 {
                    return Err(io::Error::other("verification failed: the copy changed during verification"));
                }
                if n != m || x[..n] != y[..n] {
                    return Err(io::Error::other("verification failed: the copy differs from the original"));
                }
                off += n;
                ctx.add_bytes(n as u64);
                *counted += n as u64;
            }
        }
        // The tail loop continues both files from here; b must skip the bulk it has not read.
        b.seek(std::io::SeekFrom::Start(bulk as u64))?;
    }

    loop {
        ctx.wait_while_paused();
        if ctx.is_cancelled() {
            return Err(fastcopy::cancelled());
        }
        let n = read_full(&mut a, &mut x)?;
        let m = read_full(&mut b, &mut y)?;
        if n != m || x[..n] != y[..m] {
            return Err(io::Error::other("verification failed: the copy differs from the original"));
        }
        if n == 0 {
            return Ok(());
        }
        ctx.add_bytes(n as u64);
        *counted += n as u64;
    }

    fn read_full(f: &mut fs::File, buf: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0;
        while filled < buf.len() {
            match f.read(&mut buf[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        Ok(filled)
    }
}

fn write_file(ctx: &JobCtx, t: &FileTask, dst: &Path, throttle: bool, buf: &mut Vec<u8>, counted: &mut u64) -> io::Result<()> {
    let src = open_unfollowed(&t.src)?;
    let perm = t.mode & 0o7777;
    let out = open_new(dst, perm)?;
    let result = (|| {
        copy_contents(ctx, &src, &out, t.size, throttle, buf, counted)?;
        // Only when the umask changed what we asked for (saves a syscall per file).
        #[cfg(unix)]
        if perm != (perm & 0o777 & !umask()) {
            out.set_permissions(fs::Permissions::from_mode(perm))?;
        }
        // HIDDEN/SYSTEM/READONLY travel with the copy once the bytes are written
        // (read-only first would block the write itself); the Windows twin of the mode bits.
        osprim::set_attrs(dst, t.attrs);
        osprim::set_times(dst, t.atime, t.mtime, true);
        Ok(())
    })();
    drop(out);
    if result.is_err() {
        let _ = fs::remove_file(dst);
    }
    result
}

/// Open a regular file, refusing to follow a symlink at the last component.
#[cfg(unix)]
fn open_unfollowed(p: &Path) -> io::Result<fs::File> {
    OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(p)
}

#[cfg(windows)]
fn open_unfollowed(p: &Path) -> io::Result<fs::File> {
    // FILE_FLAG_OPEN_REPARSE_POINT: a reparse point opens as itself, matching O_NOFOLLOW.
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new().read(true).custom_flags(0x0020_0000).open(p)
}

/// Create a brand-new destination file with the source's permission shape.
#[cfg(unix)]
fn open_new(dst: &Path, perm: u32) -> io::Result<fs::File> {
    OpenOptions::new().write(true).create_new(true).mode(perm & 0o777).open(dst)
}

#[cfg(windows)]
fn open_new(dst: &Path, perm: u32) -> io::Result<fs::File> {
    // The attribute shape (read-only/hidden/system) rides on FileTask.attrs and is
    // applied after the copy; perm is the Unix view and carries nothing to set here.
    let _ = perm;
    OpenOptions::new().write(true).create_new(true).open(dst)
}

pub use super::gvfs::is_gvfs_path;

/// Copies one file onto a GVfs share through GIO, which talks to the share's backend
/// directly instead of through gvfsd-fuse. Keeps the modification time.
///
/// The copy runs on a GIO copy thread (see [`gio_copier`]): GIO waits for a backend that
/// stopped answering even after it was cancelled. A cancelled job then returns after a
/// moment and leaves that thread to end by itself (it removes the partial file when the
/// share answers again), and a copy that moves no data for a minute ends with a "not
/// responding" error.
fn write_file_gio(ctx: &JobCtx, t: &FileTask, dst: &Path, counted: &mut u64) -> io::Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};
    const STALL: Duration = Duration::from_secs(60);
    const GIVE_UP_AFTER_CANCEL: Duration = Duration::from_secs(2);

    let cancel = gio::Cancellable::new();
    let paused = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let copier = gio_copier()?;
    copier
        .send(GioCopy { src: gio::File::for_path(&t.src), out: gio::File::for_path(dst), cancel: cancel.clone(), paused: paused.clone(), tx })
        .map_err(|_| io::Error::other("the copy thread ended"))?;

    let mut seen = 0u64;
    let mut last_data = Instant::now();
    let mut cancelled_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Msg::Progress(done)) => {
                if done > seen {
                    ctx.add_bytes(done - seen);
                    *counted += done - seen;
                    seen = done;
                    last_data = Instant::now();
                }
            }
            Ok(Msg::Done(result)) => {
                IDLE_GIO_COPIERS.lock().unwrap_or_else(|e| e.into_inner()).push(copier);
                // Empty files report no progress; count what is left either way.
                if result.is_ok() && t.size > seen {
                    ctx.add_bytes(t.size - seen);
                    *counted += t.size - seen;
                }
                return result.map_err(|e| {
                    if e.matches(gio::IOErrorEnum::Exists) {
                        io::Error::new(io::ErrorKind::AlreadyExists, e.to_string())
                    } else if e.matches(gio::IOErrorEnum::Cancelled) {
                        fastcopy::cancelled()
                    } else {
                        io::Error::other(e.to_string())
                    }
                });
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(io::Error::other("the copy ended without a result")),
        }
        let is_paused = ctx.is_paused();
        paused.store(is_paused, Ordering::Relaxed);
        if is_paused {
            last_data = Instant::now();
        }
        if ctx.is_cancelled() && cancelled_at.is_none() {
            cancel.cancel();
            cancelled_at = Some(Instant::now());
        }
        if cancelled_at.is_some_and(|c| c.elapsed() > GIVE_UP_AFTER_CANCEL) {
            return Err(fastcopy::cancelled());
        }
        if cancelled_at.is_none() && last_data.elapsed() > STALL {
            cancel.cancel();
            let root = super::gvfs::share_root(dst).unwrap_or_else(|| dst.to_path_buf());
            return Err(super::gvfs::not_responding(root));
        }
    }
}

enum Msg {
    Progress(u64),
    Done(Result<(), gtk::glib::Error>),
}

struct GioCopy {
    src: gio::File,
    out: gio::File,
    cancel: gio::Cancellable,
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx: std::sync::mpsc::Sender<Msg>,
}

/// GIO copy threads waiting for work.
static IDLE_GIO_COPIERS: Mutex<Vec<std::sync::mpsc::Sender<GioCopy>>> = Mutex::new(Vec::new());

/// A thread to copy one file through GIO; give it back to [`IDLE_GIO_COPIERS`] when the copy
/// is done. The threads live on because GVfs opens a D-Bus connection to the share's
/// backend for each thread and never closes it: a thread per file ran out of file
/// descriptors after about a thousand files. A thread that is never given back (its copy
/// hangs) ends once that copy returns.
fn gio_copier() -> io::Result<std::sync::mpsc::Sender<GioCopy>> {
    if let Some(c) = IDLE_GIO_COPIERS.lock().unwrap_or_else(|e| e.into_inner()).pop() {
        return Ok(c);
    }
    let (tx, rx) = std::sync::mpsc::channel::<GioCopy>();
    std::thread::Builder::new()
        .name("fb-gio-copy".into())
        .spawn(move || {
            for job in rx {
                let progress_tx = job.tx.clone();
                let mut progress = |done: i64, _total: i64| {
                    let _ = progress_tx.send(Msg::Progress(done.max(0) as u64));
                    while job.paused.load(Ordering::Relaxed) && !job.cancel.is_cancelled() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                };
                let flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::TARGET_DEFAULT_PERMS;
                let r = job.src.copy(&job.out, flags, Some(&job.cancel), Some(&mut progress));
                if let Err(e) = &r {
                    // Someone else's file stays; a partial copy goes.
                    if !e.matches(gio::IOErrorEnum::Exists) {
                        let _ = job.out.delete(gio::Cancellable::NONE);
                    }
                }
                let _ = job.tx.send(Msg::Done(r));
            }
        })
        .map_err(io::Error::other)?;
    Ok(tx)
}

/// The name of `p` for messages.
pub fn display_name(p: &Path) -> String {
    p.file_name().unwrap_or(OsStr::new("/")).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)] // the attribute twin of the Unix mode-bit scenario in tests/copy_engine.rs
    fn readonly_and_hidden_travel_through_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        let f = src.join("locked.txt");
        fs::write(&f, "secret").unwrap();
        osprim::set_attrs(&f, 0x3); // READONLY | HIDDEN
        let dest = dir.path().join("dest");
        fs::create_dir(&dest).unwrap();
        let ctx = JobCtx::new();
        transfer(&ctx, &[src.clone()], &dest, Mode::Copy, &Options::default()).unwrap();
        let out = dest.join("src").join("locked.txt");
        assert_eq!(fs::read_to_string(&out).unwrap(), "secret");
        assert_eq!(osprim::attrs_of(&out), 0x3);
        assert!(fs::metadata(&out).unwrap().permissions().readonly());
    }

    #[test]
    #[cfg(windows)] // verification must catch a byte change in bulk or tail through NO_BUFFERING
    fn verification_reads_from_the_device() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = JobCtx::new();
        for size in [0usize, 1, 500, 4096, 5000, (1 << 20) + 777] {
            let a = dir.path().join(format!("a{size}"));
            let b = dir.path().join(format!("b{size}"));
            let data: Vec<u8> = (0..size).map(|i| (i * 13 % 251) as u8).collect();
            fs::write(&a, &data).unwrap();
            fs::write(&b, &data).unwrap();
            verify_copy(&ctx, &a, &b, &mut 0).unwrap();
            if size >= 2 {
                let mut bad = data.clone();
                let idx = if size > 4200 { 4200 } else { size - 2 };
                bad[idx] ^= 0xFF;
                fs::write(&b, &bad).unwrap();
                assert!(verify_copy(&ctx, &a, &b, &mut 0).is_err(), "a changed byte at {idx} of {size} must be caught");
            }
        }
    }

    #[test]
    #[cfg(windows)] // measured behaviour: removal ignores READONLY, so moves need no clearing
    fn removal_ignores_the_readonly_attribute() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("ro.txt");
        fs::write(&f, "x").unwrap();
        osprim::set_attrs(&f, 0x1);
        fs::remove_file(&f).unwrap();
        assert!(!f.exists());
    }
}
