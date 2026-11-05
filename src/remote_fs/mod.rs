//! Remote folders (SFTP and FTP) as browsable locations on Windows, where GVfs would
//! otherwise mount them. The decision this module embodies (provisional, user-approved):
//! SFTP/FTP become an in-app VFS rather than drive letters — the whole engine (browse,
//! copy in and out, edit-elsewhere write-back) works on staged local data exactly like
//! the archive VFS does, while SMB and WebDAV stay real UNC paths (see `win::wnet`).
//!
//! Discipline mirrors `archive::vfs`: one tokio runtime owned here (it never leaks out),
//! all calls blocking from the caller's worker thread, sessions pooled per
//! `user@host:port` and dropped the moment a call reports a broken connection.

pub mod auth;
pub mod ftp;
pub mod sftp;

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

// The address and entry data lives in `location` so the Location::Remote variant is data
// on every platform; the engine below only ever runs on Windows.
pub use crate::location::{Kind, RemoteAt, RemoteEntry as Entry, Target};

/// A live connection behind the pool (cheap to hand out: both arms are one Arc).
#[derive(Clone)]
enum Session {
    Sftp(Arc<russh_sftp::client::SftpSession>),
    Ftp(Arc<ftp::Ftp>),
}

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();

/// The one tokio runtime remote work runs on; built once, never handed out.
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("fb-remote")
            .enable_io()
            .enable_time()
            .build()
            .expect("the remote runtime builds once and cannot fail here")
    })
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forgets a broken session (every failing call does; the next one reconnects).
fn drop_session(t: &Target) {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).remove(&t.key());
}

/// Runs `f` on a pooled connection; a connection that died is retried once on a fresh one.
fn with_session<T>(t: &Target, f: impl Fn(&Session) -> io::Result<T>) -> io::Result<T> {
    for attempt in 0..2 {
        let session = {
            let mut pool = sessions().lock().unwrap_or_else(|e| e.into_inner());
            match pool.get(&t.key()) {
                Some(s) => s.clone(),
                None if attempt == 1 => continue,
                None => {
                    let s = connect(t)?;
                    pool.insert(t.key(), s.clone());
                    s
                }
            }
        };
        match f(&session) {
            Ok(v) => return Ok(v),
            Err(e) if attempt == 0 && is_dead(&e) => drop_session(t),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("the remote connection kept failing"))
}

/// A plain read of the pool (no reconnect: cheap probes use it as the cache says).
fn cached(t: &Target) -> Option<Session> {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).get(&t.key()).cloned()
}

fn connect(t: &Target) -> io::Result<Session> {
    let s = match t.kind {
        Kind::Sftp => Session::Sftp(Arc::new(sftp::open(t)?)),
        Kind::Ftp => Session::Ftp(Arc::new(ftp::open(t)?)),
    };
    Ok(s)
}

/// Whether an error means "this connection is gone" (reconnect) rather than "the server
/// said no" (report).
fn is_dead(e: &io::Error) -> bool {
    use io::ErrorKind::*;
    matches!(e.kind(), NotConnected | BrokenPipe | ConnectionReset | ConnectionRefused | ConnectionAborted | TimedOut | UnexpectedEof) || e.to_string().contains("connection lost")
}

// Each engine call dispatches on the address: a server goes through the session pool
// above, a device storage goes to `crate::wpd` (which caches and reconnects on its own).

/// Lists `path` (the pane asks again on every navigation; there is no listing cache yet).
pub fn list(at: &RemoteAt, path: &str) -> io::Result<Vec<Entry>> {
    match at {
        RemoteAt::Server(t) => with_session(t, |s| match s {
            Session::Sftp(s) => sftp::list(s, path),
            Session::Ftp(f) => ftp::list(f, path),
        }),
        RemoteAt::Wpd { pnp, storage } => crate::wpd::list(pnp, storage, path),
    }
}

pub fn mkdir(at: &RemoteAt, path: &str) -> io::Result<()> {
    match at {
        RemoteAt::Server(t) => with_session(t, |s| match s {
            Session::Sftp(s) => sftp::mkdir(s, path),
            Session::Ftp(f) => ftp::mkdir(f, path),
        }),
        RemoteAt::Wpd { pnp, storage } => crate::wpd::mkdir(pnp, storage, path),
    }
}

pub fn remove(at: &RemoteAt, path: &str, is_dir: bool) -> io::Result<()> {
    match at {
        RemoteAt::Server(t) => with_session(t, |s| match s {
            Session::Sftp(s) => sftp::remove(s, path, is_dir),
            Session::Ftp(f) => ftp::remove(f, path, is_dir),
        }),
        // WPD deletes one object by id and does not need the kind.
        RemoteAt::Wpd { pnp, storage } => crate::wpd::remove(pnp, storage, path),
    }
}

pub fn rename(at: &RemoteAt, from: &str, to: &str) -> io::Result<()> {
    match at {
        RemoteAt::Server(t) => with_session(t, |s| match s {
            Session::Sftp(s) => sftp::rename(s, from, to),
            Session::Ftp(f) => ftp::rename(f, from, to),
        }),
        RemoteAt::Wpd { pnp, storage } => crate::wpd::rename(pnp, storage, from, to),
    }
}

pub fn read_to(at: &RemoteAt, path: &str, local_out: &std::path::Path) -> io::Result<u64> {
    match at {
        RemoteAt::Server(t) => with_session(t, |s| match s {
            Session::Sftp(s) => sftp::read_to(s, path, local_out),
            Session::Ftp(f) => ftp::read_to(f, path, local_out),
        }),
        RemoteAt::Wpd { pnp, storage } => crate::wpd::read_to(pnp, storage, path, local_out),
    }
}

pub fn write_from(at: &RemoteAt, local_in: &std::path::Path, path: &str) -> io::Result<u64> {
    match at {
        RemoteAt::Server(t) => with_session(t, |s| match s {
            Session::Sftp(s) => sftp::write_from(s, local_in, path),
            Session::Ftp(f) => ftp::write_from(f, local_in, path),
        }),
        RemoteAt::Wpd { pnp, storage } => crate::wpd::write_from(pnp, storage, local_in, path),
    }
}

pub fn exists(at: &RemoteAt, path: &str) -> bool {
    match at {
        RemoteAt::Server(t) => match cached(t) {
            Some(s) => match &s {
                Session::Sftp(s) => sftp::exists(s, path),
                Session::Ftp(f) => ftp::exists(f, path),
            },
            None => false,
        },
        RemoteAt::Wpd { pnp, storage } => crate::wpd::exists(pnp, storage, path),
    }
}

// ── Whole-folder moves (the copy engine's remote legs) ──────────────────────

/// Joins a remote folder and an entry name the way the servers spell paths.
fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") }
}

/// (stem, extension) for "name (2)" free-name numbering; hidden and dot-less names keep
/// the whole name as the stem.
fn split_ext(name: &str) -> (String, String) {
    match name.split_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && !ext.contains('.') && ext.len() <= 8 => (stem.to_string(), ext.to_string()),
        _ => (name.to_string(), String::new()),
    }
}

/// `want` is taken in `folder`: the next free "( n)" spelling. Callers reach this only
/// after the user chose KeepBoth.
fn free_name(at: &RemoteAt, folder: &str, want: &str) -> String {
    if !exists(at, &join(folder, want)) {
        return want.to_string();
    }
    let (stem, ext) = split_ext(want);
    let mut n = 2u32;
    loop {
        let cand = if ext.is_empty() { format!("{stem} ({n})") } else { format!("{stem} ({n}).{ext}") };
        if !exists(at, &join(folder, &cand)) {
            return cand;
        }
        n += 1;
    }
}

/// Copies a remote file or subtree into the local `dest` folder (staging): the walk
/// follows the listing, files land through the pooled connection, and the job's cancel
/// button stops it between entries. Returns the top-level local paths created.
pub fn fetch_tree(ctx: &crate::ops::job::JobCtx, at: &RemoteAt, path: &str, is_dir: bool, dest: &std::path::Path) -> io::Result<Vec<std::path::PathBuf>> {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let top = dest.join(&name);
    if is_dir {
        std::fs::create_dir_all(&top)?;
        let mut queue = vec![(path.to_string(), top.clone())];
        while let Some((rp, lp)) = queue.pop() {
            if ctx.is_cancelled() {
                return Err(crate::ops::fastcopy::cancelled());
            }
            for e in list(at, &rp)? {
                let (child_r, child_l) = (join(&rp, &e.name), lp.join(&e.name));
                if e.is_dir {
                    std::fs::create_dir_all(&child_l)?;
                    queue.push((child_r, child_l));
                } else {
                    ctx.set_current(e.name.clone());
                    read_to(at, &child_r, &child_l)?;
                }
            }
        }
    } else {
        ctx.set_current(name.clone());
        read_to(at, path, &top)?;
    }
    Ok(vec![top])
}

/// Pushes local files and folders into the remote folder `dest`, asking the job's
/// conflict question per top-level entry; a folder's own contents then overwrite as
/// plain STOR/write does (matching how the local engine treats files inside a
/// merged folder). Returns the created entry names.
pub fn publish_tree(ctx: &crate::ops::job::JobCtx, locals: &[std::path::PathBuf], at: &RemoteAt, dest: &str) -> io::Result<Vec<std::path::PathBuf>> {
    use crate::ops::job::{Answer, Question};
    let mut created = Vec::new();
    for src in locals {
        if ctx.is_cancelled() {
            return Err(crate::ops::fastcopy::cancelled());
        }
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let existing = list(at, dest).unwrap_or_default().into_iter().find(|e| e.name == name);
        let name = match existing {
            None => name,
            Some(e) => match ctx.ask(Question::Conflict {
                src: src.clone(),
                dst: std::path::PathBuf::from(at.uri(&join(dest, &name))),
                src_size: src.metadata().map_or(0, |m| m.len()),
                dst_size: e.size,
                src_mtime: src.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64),
                dst_mtime: e.mtime,
                dst_is_dir: e.is_dir,
            }) {
                Answer::Replace => {
                    remove(at, &join(dest, &name), e.is_dir)?;
                    name
                }
                Answer::KeepBoth => free_name(at, dest, &name),
                Answer::Skip => continue,
                _ => return Err(crate::ops::fastcopy::cancelled()),
            },
        };
        let rdest = join(dest, &name);
        if src.is_dir() {
            let mut queue = vec![(src.clone(), rdest.clone())];
            while let Some((lp, rp)) = queue.pop() {
                if ctx.is_cancelled() {
                    return Err(crate::ops::fastcopy::cancelled());
                }
                if !exists(at, &rp) {
                    mkdir(at, &rp)?;
                }
                for e in (std::fs::read_dir(&lp).ok()).into_iter().flatten().flatten() {
                    let child_l = e.path();
                    let child_r = join(&rp, &e.file_name().to_string_lossy());
                    if child_l.is_dir() {
                        queue.push((child_l, child_r));
                    } else {
                        ctx.set_current(e.file_name().to_string_lossy().into_owned());
                        write_from(at, &child_l, &child_r)?;
                    }
                }
            }
        } else {
            ctx.set_current(name.clone());
            write_from(at, src, &rdest)?;
        }
        created.push(std::path::PathBuf::from(name));
    }
    Ok(created)
}

/// Closes every pooled connection (logout of the app; tests keep the pool tidy).
pub fn close_all() {
    let pool = sessions().lock().unwrap_or_else(|e| e.into_inner());
    for s in pool.values() {
        match s {
            Session::Sftp(s) => {
                let _ = runtime().block_on(s.close());
            }
            Session::Ftp(f) => f.quit(),
        }
    }
    drop(pool);
    let _ = sessions().lock().unwrap_or_else(|e| e.into_inner()).drain();
}

