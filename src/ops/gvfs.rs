//! GVfs shares exposed by gvfsd-fuse (`$XDG_RUNTIME_DIR/gvfs/smb-share:…`).
//!
//! When a server stops answering, or gvfsd-fuse itself hangs, every access to the share's
//! folder blocks for good. Before a share is listed or written to, [`check`] asks it with a
//! timeout on a separate thread. While such a question is still unanswered, later checks
//! of that share fail at once instead of piling up more stuck threads.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a share may take to answer before it counts as not responding.
pub const TIMEOUT: Duration = Duration::from_secs(8);

pub fn fuse_root() -> PathBuf {
    gtk::glib::user_runtime_dir().join("gvfs")
}

/// Whether `p` lies on a GVfs share exposed by gvfsd-fuse.
pub fn is_gvfs_path(p: &Path) -> bool {
    p.starts_with(fuse_root()) || p.starts_with(gtk::glib::home_dir().join(".gvfs"))
}

/// The share's top folder (`…/gvfs/smb-share:server=…,share=…`) for a path on it.
pub fn share_root(p: &Path) -> Option<PathBuf> {
    for base in [fuse_root(), gtk::glib::home_dir().join(".gvfs")] {
        if let Ok(rest) = p.strip_prefix(&base) {
            return rest.components().next().map(|c| base.join(c));
        }
    }
    None
}

/// A readable name for the share: `share on server` for SMB, the device's name for a phone,
/// else its folder name.
pub fn share_name(root: &Path) -> String {
    if let Some((_, name, _)) = crate::devices::named_root(root).filter(|(r, ..)| r == root) {
        return name;
    }
    let dir = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let field = |key: &str| dir.split([':', ',']).find_map(|kv| kv.strip_prefix(key).map(str::to_string));
    match (field("share="), field("server=")) {
        (Some(share), Some(server)) => format!("{share} on {server}"),
        _ => dir,
    }
}

/// The error of a share that did not answer in time.
#[derive(Debug)]
pub struct NotResponding {
    pub root: PathBuf,
}

impl std::fmt::Display for NotResponding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "“{}” is not responding.", share_name(&self.root))
    }
}

impl std::error::Error for NotResponding {}

/// The error [`check`] gives for a share that did not answer.
pub fn not_responding(root: PathBuf) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, NotResponding { root })
}

/// The share behind a "not responding" error from [`check`].
pub fn not_responding_share(e: &io::Error) -> Option<PathBuf> {
    e.get_ref()?.downcast_ref::<NotResponding>().map(|n| n.root.clone())
}

#[derive(Default)]
struct Share {
    /// A probe thread has not come back yet (with its generation).
    pending: Option<u64>,
    generation: u64,
    free: Option<(u64, Instant)>,
    free_pending: bool,
}

static SHARES: Mutex<Option<HashMap<PathBuf, Share>>> = Mutex::new(None);

fn with_share<T>(root: &Path, f: impl FnOnce(&mut Share) -> T) -> T {
    let mut all = SHARES.lock().unwrap();
    f(all.get_or_insert_with(HashMap::new).entry(root.to_path_buf()).or_default())
}

/// Asks the share holding `p` whether `p` is there, waiting at most `timeout`. Paths not on
/// a GVfs share pass at once. Errors: the file system's own, or a [`NotResponding`] one
/// (see [`not_responding_share`]).
pub fn check(p: &Path, timeout: Duration) -> io::Result<()> {
    check_with(p, timeout, |p| std::fs::metadata(p).map(|_| ()))
}

fn check_with(p: &Path, timeout: Duration, probe: impl FnOnce(&Path) -> io::Result<()> + Send + 'static) -> io::Result<()> {
    let Some(root) = share_root(p) else { return Ok(()) };
    let generation = with_share(&root, |s| {
        if s.pending.is_some() {
            return None;
        }
        s.generation += 1;
        s.pending = Some(s.generation);
        Some(s.generation)
    });
    let Some(generation) = generation else { return Err(not_responding(root)) };
    let (tx, rx) = mpsc::channel();
    let (path, r2) = (p.to_path_buf(), root.clone());
    std::thread::Builder::new()
        .name("fb-share-probe".into())
        .spawn(move || {
            let res = probe(&path);
            with_share(&r2, |s| {
                if s.pending == Some(generation) {
                    s.pending = None;
                }
            });
            let _ = tx.send(res);
        })
        .map_err(io::Error::other)?;
    match rx.recv_timeout(timeout) {
        Ok(res) => res,
        Err(_) => Err(not_responding(root)),
    }
}

/// Forgets that a share hung (after it was mounted again): the next check asks afresh.
pub fn forget(root: &Path) {
    with_share(root, |s| {
        s.pending = None;
        s.free = None;
    });
}

/// Free space on the share holding `dir`, without ever blocking: the last value read (at
/// most a minute old), while a fresh one is read in the background.
pub fn free_space_cached(dir: &Path, read: fn(&Path) -> Option<u64>) -> Option<u64> {
    let root = share_root(dir)?;
    let (cached, start) = with_share(&root, |s| {
        let fresh = s.free.is_some_and(|(_, t)| t.elapsed() < Duration::from_secs(60));
        let start = !fresh && !s.free_pending && s.pending.is_none();
        if start {
            s.free_pending = true;
        }
        (s.free.map(|(v, _)| v), start)
    });
    if start {
        let dir = dir.to_path_buf();
        let _ = std::thread::Builder::new().name("fb-share-free".into()).spawn(move || {
            let v = read(&dir);
            with_share(&root, |s| {
                s.free_pending = false;
                s.free = v.map(|v| (v, Instant::now()));
            });
        });
    }
    cached
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_share(rest: &str) -> PathBuf {
        fuse_root().join("smb-share:server=192.168.1.198,share=gamingcrypt-drive").join(rest)
    }

    #[test]
    fn share_paths_and_names() {
        let p = on_share("Windows Games/GTA4");
        assert!(is_gvfs_path(&p));
        let root = share_root(&p).unwrap();
        assert_eq!(root, on_share(""));
        assert_eq!(share_name(&root), "gamingcrypt-drive on 192.168.1.198");
        assert_eq!(share_root(Path::new("/home/me")), None);
        assert!(check(Path::new("/definitely/not/a/share"), Duration::from_millis(1)).is_ok());
    }

    #[test]
    fn a_hanging_share_times_out_and_later_checks_fail_fast() {
        let p = on_share("hang-test");
        let (release, wait) = mpsc::channel::<()>();
        let e = check_with(&p, Duration::from_millis(100), move |_| {
            let _ = wait.recv();
            Ok(())
        })
        .unwrap_err();
        assert_eq!(not_responding_share(&e), share_root(&p));
        assert_eq!(e.to_string(), "“gamingcrypt-drive on 192.168.1.198” is not responding.");
        // The first probe is still stuck: no second thread, an answer at once.
        let t = Instant::now();
        assert!(check_with(&p, Duration::from_secs(5), |_| Ok(())).is_err());
        assert!(t.elapsed() < Duration::from_secs(1));
        // After a reconnect the share is asked again.
        forget(&share_root(&p).unwrap());
        assert!(check_with(&p, Duration::from_secs(5), |_| Ok(())).is_ok());
        release.send(()).unwrap();
        // Errors of the file system come through as they are.
        let e = check_with(&p, Duration::from_secs(5), |_| Err(io::Error::from(io::ErrorKind::NotFound))).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }
}
