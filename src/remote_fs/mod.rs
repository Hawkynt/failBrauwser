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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// What kind of remote a target is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Sftp,
    Ftp,
}

/// A server entry point: whose account, which host, which port (0: the kind's usual one).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub kind: Kind,
    pub user: String,
    pub host: String,
    pub port: u16,
}

impl Target {
    /// `sftp://me@host:2222/dir` or `ftp://host` (user empty: the current user's name).
    pub fn from_uri(uri: &str) -> Option<Target> {
        let (scheme, rest) = uri.trim().split_once("://")?;
        let kind = match scheme.to_ascii_lowercase().as_str() {
            "sftp" | "ssh" => Kind::Sftp,
            "ftp" => Kind::Ftp,
            _ => return None,
        };
        let authority = rest.split('/').next()?;
        let (user, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
        let (host, port) = hostport.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map_or((hostport, 0), |(h, p)| (h, p.parse::<u16>().unwrap_or(0)));
        (!host.is_empty() && !host.contains('/')).then(|| Target { kind, user: user.to_string(), host: host.to_string(), port })
    }

    pub fn port(&self) -> u16 {
        if self.port != 0 {
            return self.port;
        }
        match self.kind {
            Kind::Sftp => 22,
            Kind::Ftp => 21,
        }
    }

    /// The pool key and the address shown in prompts.
    pub fn key(&self) -> String {
        format!("{}@{}:{}", if self.user.is_empty() { "?" } else { &self.user }, self.host, self.port())
    }

    /// The address as servers.rs and history spell it, with `path` (a remote path) behind it.
    pub fn uri(&self, path: &str) -> String {
        let scheme = match self.kind {
            Kind::Sftp => "sftp",
            Kind::Ftp => "ftp",
        };
        let user = if self.user.is_empty() { String::new() } else { format!("{}@", gtk::glib::uri_escape_string(self.user.as_str(), None::<&str>, false)) };
        format!("{scheme}://{user}{}:{}/{path}", self.host, self.port())
    }
}

/// One folder entry as the row model needs it: enough to sort, show and stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    /// Modification time as seconds since the Unix epoch (0 when the server hides it).
    pub mtime: i64,
    pub is_dir: bool,
    pub is_link: bool,
}

/// A live connection behind the pool (cheap to hand out: both arms are one Arc).
#[derive(Clone)]
enum Session {
    Sftp(Arc<russh_sftp::client::SftpSession>),
    Ftp(Arc<ftp::Ftp>),
}

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
static EPOCH: AtomicU64 = AtomicU64::new(0);

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
    // Keep the pool key stable across edits so old probes never hit a new connection.
    EPOCH.fetch_add(1, Ordering::Relaxed);
    Ok(s)
}

/// Whether an error means "this connection is gone" (reconnect) rather than "the server
/// said no" (report).
fn is_dead(e: &io::Error) -> bool {
    use io::ErrorKind::*;
    matches!(e.kind(), NotConnected | BrokenPipe | ConnectionReset | ConnectionRefused | ConnectionAborted | TimedOut | UnexpectedEof) || e.to_string().contains("connection lost")
}

/// Lists `path` (the pane asks again on every navigation; there is no listing cache yet).
pub fn list(t: &Target, path: &str) -> io::Result<Vec<Entry>> {
    with_session(t, |s| match s {
        Session::Sftp(s) => sftp::list(s, path),
        Session::Ftp(f) => ftp::list(f, path),
    })
}

pub fn mkdir(t: &Target, path: &str) -> io::Result<()> {
    with_session(t, |s| match s {
        Session::Sftp(s) => sftp::mkdir(s, path),
        Session::Ftp(f) => ftp::mkdir(f, path),
    })
}

pub fn remove(t: &Target, path: &str, is_dir: bool) -> io::Result<()> {
    with_session(t, |s| match s {
        Session::Sftp(s) => sftp::remove(s, path, is_dir),
        Session::Ftp(f) => ftp::remove(f, path, is_dir),
    })
}

pub fn rename(t: &Target, from: &str, to: &str) -> io::Result<()> {
    with_session(t, |s| match s {
        Session::Sftp(s) => sftp::rename(s, from, to),
        Session::Ftp(f) => ftp::rename(f, from, to),
    })
}

pub fn read_to(t: &Target, path: &str, local_out: &std::path::Path) -> io::Result<u64> {
    with_session(t, |s| match s {
        Session::Sftp(s) => sftp::read_to(s, path, local_out),
        Session::Ftp(f) => ftp::read_to(f, path, local_out),
    })
}

pub fn write_from(t: &Target, local_in: &std::path::Path, path: &str) -> io::Result<u64> {
    with_session(t, |s| match s {
        Session::Sftp(s) => sftp::write_from(s, local_in, path),
        Session::Ftp(f) => ftp::write_from(f, local_in, path),
    })
}

pub fn exists(t: &Target, path: &str) -> bool {
    match cached(t) {
        Some(s) => match &s {
            Session::Sftp(s) => sftp::exists(s, path),
            Session::Ftp(f) => ftp::exists(f, path),
        },
        None => false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_and_print() {
        let t = Target::from_uri("sftp://me@nas.local:2222/srv/files").unwrap();
        assert_eq!(t.kind, Kind::Sftp);
        assert_eq!(t.user, "me");
        assert_eq!(t.host, "nas.local");
        assert_eq!(t.port(), 2222);
        assert_eq!(t.uri("a/b"), "sftp://me@nas.local:2222/a/b");
        let bare = Target::from_uri("ftp://10.0.0.9").unwrap();
        assert_eq!(bare.kind, Kind::Ftp);
        assert_eq!(bare.user, "");
        assert_eq!(bare.port(), 21);
        assert_eq!(bare.uri(""), "ftp://10.0.0.9:21/");
        assert!(Target::from_uri("smb://nas/share").is_none());
        assert!(Target::from_uri("sftp://").is_none());
        assert_eq!(Target::from_uri("sftp://me@nas/x").unwrap().key(), "me@nas:22");
    }
}
