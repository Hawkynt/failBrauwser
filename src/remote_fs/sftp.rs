//! SFTP over russh: the connection (host-key TOFU, keys then a prompted password), the
//! subsystem channel, and the folder operations on the pooled session. Every call here
//! runs on the runtime `remote_fs` owns and blocks the caller's worker thread while it
//! waits — the UI never touches these directly.

use super::auth;
use super::{Entry, Target};
use russh::client::Handle;
use russh::keys::{PrivateKey, PublicKeyOrCertificate};
use std::io;
use std::path::Path;

/// The handler answers only what cannot wait for a result: the server key check.
struct FbHandler {
    host: String,
    port: u16,
}

#[derive(Debug)]
struct FbError(io::Error);

impl From<russh::Error> for FbError {
    fn from(e: russh::Error) -> Self {
        FbError(io::Error::other(e.to_string()))
    }
}

impl russh::client::Handler for FbHandler {
    type Error = FbError;

    fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> impl std::future::Future<Output = Result<bool, Self::Error>> + Send {
        // TOFU: known key passes, unknown key asks through the global prompter.
        let (host, port) = (self.host.clone(), self.port);
        let shown = key.public_key().to_string();
        async move { Ok(auth::trust_host(&host, port, &shown).unwrap_or(false)) }
    }
}

/// Signs in and opens the sftp subsystem on a fresh connection.
pub fn open(t: &Target) -> io::Result<russh_sftp::client::SftpSession> {
    // russh's own client id — a hand-written one buys nothing and can trip old servers.
    let config = std::sync::Arc::new(russh::client::Config::default());
    let port = t.port();
    // Sign-in runs its own awaits one block_on at a time: nesting a block_on inside a
    // block_on is fatal for tokio, and the prompts land between those blocks anyway.
    let mut handle = runtime_block(async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), russh::client::connect(config, (&t.host[..], port), FbHandler { host: t.host.clone(), port }))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("{} did not answer within 10 s.", t.key())))?
            .map_err(|e| e.0)
    })?;
    sign_in(&mut handle, t)?;
    let session = runtime_block(async move {
        let channel = handle.channel_open_session().await.map_err(io_err)?;
        channel.request_subsystem(true, "sftp").await.map_err(io_err)?;
        russh_sftp::client::SftpSession::new(channel.into_stream()).await.map_err(sftp_io)
    })?;
    session.set_timeout(20);
    Ok(session)
}

/// Keys the user already has, then one prompted password (three tries).
fn sign_in(handle: &mut Handle<FbHandler>, t: &Target) -> io::Result<()> {
    let user = if t.user.is_empty() { std::env::var("USERNAME").unwrap_or_else(|_| "anonymous".into()) } else { t.user.clone() };
    // The hash is only ever consulted for RSA keys; ed25519/ecdsa ignore it.
    let hash = runtime_block(handle.best_supported_rsa_hash()).ok().flatten().flatten();
    for key in default_keys()? {
        let signed = russh::keys::PrivateKeyWithHashAlg::new(std::sync::Arc::new(key), hash);
        if matches!(runtime_block(handle.authenticate_publickey(user.clone(), signed)), Ok(russh::client::AuthResult::Success)) {
            return Ok(());
        }
    }
    for attempt in 0..3 {
        let pass = auth::ask_password(t, attempt > 0)?;
        match runtime_block(handle.authenticate_password(user.clone(), pass)).map_err(io_err)? {
            russh::client::AuthResult::Success => return Ok(()),
            russh::client::AuthResult::Failure { .. } => continue,
        }
    }
    Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{user}@{} refused the credentials.", t.host)))
}

/// Private keys at their usual places; a protected key asks for its passphrase once.
fn default_keys() -> io::Result<Vec<PrivateKey>> {
    let home = gtk::glib::home_dir();
    let mut out = Vec::new();
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let path = home.join(".ssh").join(name);
        if !path.is_file() {
            continue;
        }
        // Open it plain, or unlock it once; a key that stays closed is left out of the
        // loop (OpenSSH likewise walks on to the next method instead of giving up).
        let key = match russh::keys::load_secret_key(&path, None) {
            Ok(k) => k,
            Err(_) => match auth::ask_passphrase(&path) {
                Ok(pass) => match russh::keys::load_secret_key(&path, Some(&pass)) {
                    Ok(k) => k,
                    Err(_) => continue,
                },
                Err(_) => continue,
            },
        };
        out.push(key);
    }
    Ok(out)
}

/// Everything russh-async wants goes through one entry point: block this worker thread on
/// the shared runtime (never called from a runtime thread).
pub(crate) fn runtime_block<T>(f: impl std::future::Future<Output = T>) -> T {
    super::runtime().block_on(f)
}

fn io_err(e: russh::Error) -> io::Error {
    io::Error::other(e.to_string())
}

/// The server's answer and the client's own complaints become io::Errors with the kind
/// the pool's reconnect rule and the UI's messages need.
pub(crate) fn sftp_io(e: russh_sftp::client::error::Error) -> io::Error {
    use russh_sftp::client::error::Error as E;
    use russh_sftp::protocol::StatusCode as S;
    match e {
        E::Status(st) => match st.status_code {
            S::NoSuchFile => io::Error::new(io::ErrorKind::NotFound, "not found"),
            S::PermissionDenied => io::Error::new(io::ErrorKind::PermissionDenied, "the server refused this operation"),
            S::Eof => io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected end"),
            S::ConnectionLost | S::NoConnection => io::Error::new(io::ErrorKind::NotConnected, "the connection was lost"),
            S::OpUnsupported => io::Error::new(io::ErrorKind::Unsupported, "the server does not support this operation"),
            other if !st.error_message.is_empty() => io::Error::other(format!("{other}: {}", st.error_message)),
            other => io::Error::other(other.to_string()),
        },
        E::Timeout => io::Error::new(io::ErrorKind::TimedOut, "the server did not answer"),
        E::IO(m) | E::Limited(m) | E::UnexpectedBehavior(m) => io::Error::other(m),
        E::UnexpectedPacket => io::Error::other("the server sent a packet we did not expect"),
    }
}

pub fn list(s: &russh_sftp::client::SftpSession, path: &str) -> io::Result<Vec<Entry>> {
    runtime_block(async {
        // read_dir hands back the whole folder fetched (the "." and ".." rows are
        // already dropped), so listing is one await and a plain walk after it.
        let rd = s.read_dir(path_display(path)).await.map_err(sftp_io)?;
        Ok(rd
            .map(|e| {
                let m = e.metadata();
                Entry {
                    name: e.file_name(),
                    size: m.size.unwrap_or(0),
                    mtime: i64::from(m.mtime.unwrap_or(0)),
                    is_dir: m.is_dir(),
                    is_link: m.is_symlink(),
                }
            })
            .collect())
    })
}

/// `/a/b/` → `/a/b` (the server speaks its own paths; "" means its home).
fn path_display(path: &str) -> String {
    let p = path.trim_end_matches('/');
    if p.is_empty() { ".".into() } else { p.to_string() }
}

pub fn mkdir(s: &russh_sftp::client::SftpSession, path: &str) -> io::Result<()> {
    runtime_block(s.create_dir(path_display(path))).map_err(sftp_io)
}

pub fn remove(s: &russh_sftp::client::SftpSession, path: &str, is_dir: bool) -> io::Result<()> {
    if is_dir {
        runtime_block(s.remove_dir(path_display(path))).map_err(sftp_io)
    } else {
        runtime_block(s.remove_file(path_display(path))).map_err(sftp_io)
    }
}

pub fn rename(s: &russh_sftp::client::SftpSession, from: &str, to: &str) -> io::Result<()> {
    runtime_block(s.rename(path_display(from), path_display(to))).map_err(sftp_io)
}

pub fn read_to(s: &russh_sftp::client::SftpSession, path: &str, local_out: &Path) -> io::Result<u64> {
    use tokio::io::AsyncReadExt;
    runtime_block(async {
        let mut remote = s.open(path_display(path)).await.map_err(sftp_io)?;
        let mut file = std::fs::File::create(local_out)?;
        let mut buf = vec![0u8; 64 << 10];
        let mut done = 0u64;
        loop {
            // russh-sftp's File speaks tokio's read/write traits: those errors are already io::Errors.
            let n = remote.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut file, &buf[..n])?;
            done += n as u64;
        }
        Ok(done)
    })
}

pub fn write_from(s: &russh_sftp::client::SftpSession, local_in: &Path, path: &str) -> io::Result<u64> {
    use tokio::io::AsyncWriteExt;
    runtime_block(async {
        let mut remote = s.create(path_display(path)).await.map_err(sftp_io)?;
        let data = std::fs::read(local_in)?;
        remote.write_all(&data).await?;
        remote.flush().await?;
        remote.shutdown().await?;
        Ok(data.len() as u64)
    })
}

pub fn exists(s: &russh_sftp::client::SftpSession, path: &str) -> bool {
    runtime_block(s.try_exists(path_display(path))).unwrap_or(false)
}
