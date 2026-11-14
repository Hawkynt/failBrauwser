//! The local network as the Network page shows it: the computers sharing files (found by
//! asking every address for SMB, announced by GVfs's network browsing, or opened before) and
//! the folders each one shares.

use crate::remote::{self, Host};
use gtk::glib;

/// A computer sharing files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Computer {
    /// What to call it: its own name if known, else its address.
    pub name: String,
    /// What to connect to (`nas.local`, `192.168.1.198`).
    pub address: String,
    /// Its IP address, when known and different from the name.
    pub ip: Option<String>,
}

impl Computer {
    /// `smb://address/`, the folders it shares.
    pub fn uri(&self) -> String {
        format!("smb://{}/", self.address)
    }
}

/// A folder a computer shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    pub comment: String,
}

/// The short name of a host: `nas.fritz.box` is `nas`; addresses stay as they are.
pub fn short_name(host: &str) -> String {
    // A port stays with an address (`10.0.0.5:8080`) and goes with a name (`nas:5005`: nas).
    let base = host.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).map_or(host, |(b, _)| b);
    if base.parse::<std::net::IpAddr>().is_ok() || host.parse::<std::net::IpAddr>().is_ok() {
        return host.to_string();
    }
    let host = base;
    host.split('.').next().filter(|s| !s.is_empty()).unwrap_or(host).to_string()
}

/// One list from everything found: SMB servers that answered (with their reverse DNS names),
/// computers GVfs announces (name, address), and servers of addresses opened before. Each
/// computer once (by address or name), sorted by name.
pub fn merge(scanned: &[Host], announced: &[(String, String)], history: &[String]) -> Vec<Computer> {
    let mut out: Vec<Computer> = Vec::new();
    let mut add = |name: String, address: String, ip: Option<String>| {
        let same = |c: &Computer| {
            c.address.eq_ignore_ascii_case(&address)
                || c.ip.as_deref().is_some_and(|i| ip.as_deref() == Some(i) || i == address)
                || ip.as_deref().is_some_and(|i| i == c.address)
                || (!name.is_empty() && c.name.eq_ignore_ascii_case(&name))
        };
        match out.iter_mut().find(|c| same(c)) {
            Some(c) => {
                // A name beats an address.
                if c.name.parse::<std::net::IpAddr>().is_ok() && name.parse::<std::net::IpAddr>().is_err() && !name.is_empty() {
                    c.name = name;
                }
                if c.ip.is_none() {
                    c.ip = ip.filter(|i| *i != c.address);
                }
            }
            None => out.push(Computer { name: if name.is_empty() { address.clone() } else { name }, ip: ip.filter(|i| *i != address), address }),
        }
    };
    for (name, address) in announced {
        add(short_name(name), address.clone(), None);
    }
    for h in scanned {
        let ip = h.addr.to_string();
        match &h.name {
            Some(n) => add(short_name(n), ip.clone(), Some(ip)),
            None => add(String::new(), ip.clone(), Some(ip)),
        }
    }
    for uri in history {
        if let Some(host) = remote::smb_host(uri).or_else(|| uri.strip_prefix("smb://").map(|h| h.trim_end_matches('/')).filter(|h| !h.is_empty() && !h.contains('/'))) {
            let ip = host.parse::<std::net::IpAddr>().is_ok().then(|| host.to_string());
            add(short_name(host), host.to_string(), ip);
        }
    }
    out.sort_by(|a, b| crate::fs::format::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase()));
    out
}

/// The host of a GVfs network entry's target (`smb://NAS/`), for announced computers.
pub fn announced_host(target_uri: &str) -> Option<String> {
    let rest = target_uri.strip_prefix("smb://")?;
    let host = rest.split('/').next()?.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// Shares that are meant to be seen: not the hidden ones (`IPC$`, `C$`, `print$`).
pub fn visible_share(name: &str) -> bool {
    !name.is_empty() && !name.ends_with('$')
}

/// The shares in `smbclient -g -L` output (`Disk|name|comment` lines), folders only.
pub fn parse_smbclient(text: &str) -> Vec<Share> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '|');
            let (kind, name) = (f.next()?, f.next()?);
            (kind == "Disk" && visible_share(name)).then(|| Share { name: name.to_string(), comment: f.next().unwrap_or_default().to_string() })
        })
        .collect()
}

/// What asking a computer for its shared folders as a guest found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// The folders it shares (maybe none).
    Shares(Vec<Share>),
    /// It wants a user name and password before it lists anything.
    Locked,
    /// It did not answer as a file server.
    Nothing,
    /// Could not ask (no `smbclient`).
    Unknown,
}

impl Probe {
    /// Whether the Network page lists the computer: only the ones with folders to open,
    /// or that may have some behind a password.
    pub fn worth_showing(&self) -> bool {
        match self {
            Probe::Shares(s) => !s.is_empty(),
            Probe::Locked | Probe::Unknown => true,
            Probe::Nothing => false,
        }
    }
}

/// What `smbclient -g -N -L` printed: its folders, or why there are none.
pub fn classify_smbclient(stdout: &str, stderr: &str) -> Probe {
    let shares = parse_smbclient(stdout);
    if !shares.is_empty() {
        return Probe::Shares(shares);
    }
    let all = format!("{stdout}\n{stderr}");
    const LOCKED: [&str; 4] = ["NT_STATUS_ACCESS_DENIED", "NT_STATUS_LOGON_FAILURE", "NT_STATUS_ACCOUNT_DISABLED", "NT_STATUS_ACCOUNT_RESTRICTION"];
    if LOCKED.iter().any(|c| all.contains(c)) {
        Probe::Locked
    } else if all.lines().any(|l| l.starts_with("Disk|") || l.starts_with("IPC|") || l.starts_with("Printer|")) {
        // It answered, with nothing but hidden folders and printers.
        Probe::Shares(Vec::new())
    } else {
        Probe::Nothing
    }
}

/// Asks `host` for its shared folders as a guest (never shows a password dialog); gives up
/// after `timeout`.
#[cfg(windows)]
pub fn probe_shares(host: &str, timeout: std::time::Duration) -> Probe {
    // The redirector answers for the whole workgroup; a dead host can block the call, so
    // the ask runs on its own thread with the same fail-fast shape as the smbclient arm.
    let (tx, rx) = std::sync::mpsc::channel();
    let host = host.to_string();
    if std::thread::Builder::new().name("fb-share-probe".into()).spawn(move || tx.send(crate::win::wnet::probe_shares(&host))).is_err() {
        return Probe::Unknown;
    }
    match rx.recv_timeout(timeout) {
        Ok(Ok(shares)) => Probe::Shares(shares.into_iter().filter(|(n, _)| visible_share(n)).map(|(name, comment)| Share { name, comment }).collect()),
        Ok(Err(e)) => {
            if crate::win::wnet::is_locked(&e) {
                Probe::Locked
            } else {
                Probe::Nothing
            }
        }
        Err(_) => Probe::Nothing,
    }
}

/// Asks `host` for its shared folders as a guest (never shows a password dialog); gives up
/// after `timeout`.
#[cfg(not(windows))]
pub fn probe_shares(host: &str, timeout: std::time::Duration) -> Probe {
    use std::process::{Command, Stdio};
    let child = Command::new("smbclient")
        .args(["-g", "-N", "-L", &format!("//{host}"), "-t", &timeout.as_secs().max(1).to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return Probe::Unknown };
    let deadline = std::time::Instant::now() + timeout + std::time::Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Probe::Nothing;
            }
        }
    }
    match child.wait_with_output() {
        Ok(out) => classify_smbclient(&String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr)),
        Err(_) => Probe::Unknown,
    }
}

/// Address schemes of network shares GVfs mounts (phones and cameras are not among them).
pub fn is_share_scheme(uri: &str) -> bool {
    let scheme = uri.split_once("://").map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
    matches!(scheme.as_str(), "smb" | "sftp" | "ssh" | "ftp" | "ftps" | "dav" | "davs" | "webdav" | "webdavs" | "nfs" | "afp")
}

/// What to do for a share after the computer woke up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterResume {
    /// It was mounted before sleeping and is gone: mount it again.
    Mount(String),
    /// It is mounted but does not answer: unmount it and mount it again.
    Reconnect(String),
}

/// The shares mounted before sleeping (`before`) and now, with whether each answers:
/// what to mount again and what to reconnect.
pub fn after_resume(before: &[String], now: &[(String, bool)]) -> Vec<AfterResume> {
    let norm = |u: &str| u.trim_end_matches('/').to_ascii_lowercase();
    let mut out: Vec<AfterResume> = now.iter().filter(|(_, answers)| !answers).map(|(u, _)| AfterResume::Reconnect(u.clone())).collect();
    for b in before {
        if !now.iter().any(|(u, _)| norm(u) == norm(b)) {
            out.push(AfterResume::Mount(b.clone()));
        }
    }
    out
}

/// The address of a shared folder: `smb://nas/My%20Files`.
pub fn share_uri(address: &str, share: &str) -> String {
    format!("smb://{}/{}", address, glib::uri_escape_string(share, None::<&str>, false))
}

/// The names the side panel and links use for a network address: a share is "Music on nas",
/// a computer its name.
pub fn uri_name(uri: &str) -> String {
    if let Some(host) = uri.strip_prefix(crate::location::NETWORK_URI) {
        return short_name(host.trim_matches('/'));
    }
    let Some(rest) = uri.split_once("://").map(|(_, r)| r) else { return uri.to_string() };
    let rest = rest.trim_end_matches('/');
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = short_name(host.rsplit('@').next().unwrap_or(host));
    let share = path.rsplit('/').next().unwrap_or_default();
    let share = glib::uri_unescape_string(share, None::<&str>).map(|s| s.to_string()).unwrap_or_else(|| share.to_string());
    if share.is_empty() { host } else { format!("{share} on {host}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn computers_are_listed_once_by_name() {
        let scanned = [
            Host { addr: Ipv4Addr::new(192, 168, 1, 198), name: Some("nas.fritz.box".into()) },
            Host { addr: Ipv4Addr::new(192, 168, 1, 20), name: None },
        ];
        let announced = [("NAS".to_string(), "NAS.local".to_string()), ("printer-box".to_string(), "printer-box.local".to_string())];
        let history = ["smb://192.168.1.198/GamingCrypt-Drive".to_string(), "smb://192.168.1.197/Old".to_string(), "sftp://x/y".to_string()];
        let all = merge(&scanned, &announced, &history);
        let names: Vec<&str> = all.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["192.168.1.20", "192.168.1.197", "NAS", "printer-box"]);
        // The announced NAS and the scanned nas.fritz.box are the same computer by name.
        let nas = all.iter().find(|c| c.name == "NAS").unwrap();
        assert_eq!(nas.address, "NAS.local");
        assert_eq!(nas.ip.as_deref(), Some("192.168.1.198"));
        assert_eq!(nas.uri(), "smb://NAS.local/");
        assert_eq!(all.iter().find(|c| c.name == "192.168.1.20").unwrap().ip, None);
    }

    #[test]
    fn shares_after_waking_up() {
        let before = vec!["smb://nas/Music/".to_string(), "smb://nas/Photos".to_string(), "sftp://pi/home".to_string()];
        // Music answers, Photos hangs, the SFTP one was dropped while sleeping.
        let now = vec![("smb://NAS/Music".to_string(), true), ("smb://nas/Photos/".to_string(), false)];
        assert_eq!(after_resume(&before, &now), [AfterResume::Reconnect("smb://nas/Photos/".into()), AfterResume::Mount("sftp://pi/home".into())]);
        assert!(after_resume(&[], &[]).is_empty());
        assert!(is_share_scheme("smb://nas/x") && is_share_scheme("davs://cloud/dav") && is_share_scheme("SFTP://pi/"));
        assert!(!is_share_scheme("mtp://Pixel/") && !is_share_scheme("file:///tmp") && !is_share_scheme("/tmp"));
    }

    #[test]
    fn guest_listings() {
        let listed = "Disk|Music|Songs\nIPC|IPC$|IPC Service (Samba)\n";
        assert_eq!(classify_smbclient(listed, ""), Probe::Shares(vec![Share { name: "Music".into(), comment: "Songs".into() }]));
        // A router that answers SMB without sharing anything.
        let empty = "IPC|IPC$|IPC Service\nDisk|print$|Printer Drivers\n";
        assert_eq!(classify_smbclient(empty, ""), Probe::Shares(Vec::new()));
        // Windows wants a password before listing.
        assert_eq!(classify_smbclient("", "session setup failed: NT_STATUS_ACCESS_DENIED\n"), Probe::Locked);
        assert_eq!(classify_smbclient("", "do_connect: Connection to 10.0.0.9 failed (Error NT_STATUS_CONNECTION_REFUSED)\n"), Probe::Nothing);
        assert!(!Probe::Nothing.worth_showing());
        assert!(!Probe::Shares(Vec::new()).worth_showing());
        assert!(Probe::Locked.worth_showing() && Probe::Unknown.worth_showing());
        assert!(classify_smbclient(listed, "").worth_showing());
    }

    #[test]
    fn share_lists_and_names() {
        let out = "Disk|Music|Songs\nDisk|IPC$|IPC Service\nDisk|My Files|\nPrinter|Laser|\nWorkgroup|WORKGROUP|NAS\n";
        assert_eq!(parse_smbclient(out).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Music", "My Files"]);
        assert_eq!(parse_smbclient(out)[0].comment, "Songs");
        assert_eq!(share_uri("nas", "My Files"), "smb://nas/My%20Files");
        assert_eq!(uri_name("smb://nas.fritz.box/My%20Files"), "My Files on nas");
        assert_eq!(uri_name("smb://me@192.168.1.5/data/"), "data on 192.168.1.5");
        assert_eq!(uri_name("network:///nas"), "nas");
        assert_eq!(announced_host("smb://NAS/"), Some("NAS".into()));
        assert_eq!(announced_host("dnssd://x/"), None);
        assert!(!visible_share("print$"));
        assert_eq!(short_name("10.0.0.1"), "10.0.0.1");
        assert_eq!(short_name("127.0.0.1:8099"), "127.0.0.1:8099");
        assert_eq!(short_name("nas.local:5005"), "nas");
        assert_eq!(uri_name("davs://cloud.example.org/remote.php/dav/files/me/"), "me on cloud");
    }
}
