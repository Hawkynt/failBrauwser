//! The questions a sign-in asks when nobody is there to answer: a UI registers a prompter
//! (or the self-test registers a scripted one), and the worker threads call into it. With
//! no prompter at all, signing in fails honestly instead of hanging.
//!
//! Host keys follow trust-on-first-use in the app's own store
//! (`<config>/failbrauwser/known_hosts`, `host:port <openssh line>`). The shared
//! `~/.ssh/known_hosts` is deliberately left alone: its hashing and revocation rules are
//! OpenSSH's, and half-implementing them would be worse than a small separate store.

use super::Target;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

/// What the worker thread needs to know.
#[derive(Debug, Clone)]
pub enum Ask {
    /// The password for `target` (`again`: the last one was refused).
    Password { target: String, again: bool },
    /// The passphrase protecting the private key at this path.
    Passphrase { path: String },
    /// A host key nobody has seen before: accept it for good, or refuse?
    HostKey { host: String, port: u16, key: String },
}

/// The answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Cancel,
    Secret(String),
    Trust,
    Refuse,
}

type Prompter = Box<dyn Fn(&Ask) -> Answer + Send + Sync>;

static PROMPTER: Mutex<Option<Prompter>> = Mutex::new(None);

/// The UI installs (and removes) its prompter. While none is installed, prompts fail.
pub fn set_prompter(p: Option<Prompter>) {
    *PROMPTER.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

fn ask(a: &Ask) -> io::Result<Answer> {
    let guard = PROMPTER.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(p) => Ok(p(a)),
        None => Err(io::Error::new(io::ErrorKind::PermissionDenied, "nobody is here to answer the sign-in questions")),
    }
}

pub fn ask_password(t: &Target, again: bool) -> io::Result<String> {
    match ask(&Ask::Password { target: t.key(), again })? {
        Answer::Secret(s) => Ok(s),
        Answer::Cancel => Err(io::Error::new(io::ErrorKind::PermissionDenied, "sign-in was cancelled")),
        _ => Err(io::Error::other("the prompter answered a password question wrongly")),
    }
}

pub fn ask_passphrase(path: &std::path::Path) -> io::Result<String> {
    match ask(&Ask::Passphrase { path: path.to_string_lossy().into_owned() })? {
        Answer::Secret(s) => Ok(s),
        Answer::Cancel => Err(io::Error::new(io::ErrorKind::PermissionDenied, "the key was not unlocked")),
        _ => Err(io::Error::other("the prompter answered a passphrase question wrongly")),
    }
}

/// The TOFU gate: a known key must match exactly; an unknown one is asked about and — on
/// Trust — remembered for good.
pub fn trust_host(host: &str, port: u16, key: &str) -> io::Result<bool> {
    let known = known_hosts().into_iter().find(|(h, _)| *h == format!("{host}:{port}")).map(|(_, k)| k);
    match known.as_deref() {
        Some(k) if k == key => Ok(true),
        Some(_) => Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{}'s host key changed — refusing to sign in through it.", host))),
        None => match ask(&Ask::HostKey { host: host.to_string(), port, key: key.to_string() })? {
            Answer::Trust => {
                remember_host(&format!("{host}:{port}"), key)?;
                Ok(true)
            }
            _ => Ok(false),
        },
    }
}

fn store_path() -> PathBuf {
    gtk::glib::user_config_dir().join("failbrauwser").join("known_hosts")
}

fn known_hosts() -> Vec<(String, String)> {
    std::fs::read_to_string(store_path()).unwrap_or_default().lines().filter_map(|l| Some((l.split_once(' ')?.0.to_string(), l.split_once(' ')?.1.trim().to_string()))).collect()
}

fn remember_host(what: &str, key: &str) -> io::Result<()> {
    use std::io::Write;
    let path = store_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{what} {key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test: the prompter is a process-wide slot, and tests run in parallel.
    #[test]
    fn prompts_need_a_prompter_and_answers_arrive() {
        let t = Target { kind: super::super::Kind::Sftp, user: "me".into(), host: "nowhere.example".into(), port: 22 };
        set_prompter(None);
        let e = ask_password(&t, false).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        set_prompter(Some(Box::new(|a| match a {
            Ask::Password { .. } => Answer::Secret("hunter2".into()),
            Ask::HostKey { .. } => Answer::Cancel,
            _ => Answer::Cancel,
        })));
        assert_eq!(ask_password(&t, true).unwrap(), "hunter2");
        // Unknown host + a prompter that refuses: no sign-in, and nothing remembered.
        assert!(!trust_host("nowhere.example.invalid", 22, "ssh-ed25519 AAAA").unwrap());
        set_prompter(None);
        assert!(!trust_host("nowhere.example.invalid", 22, "ssh-ed25519 AAAA").is_ok_and(|t| t));
    }
}
