//! Servers the user added to the Network page under a name of their own: SFTP, WebDAV, FTP
//! or SMB addresses, kept in `~/.config/failbrauwser/servers` (one `name<TAB>address` a line).

use gtk::glib;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub uri: String,
}

/// The kinds of server, as (address scheme, what to call it, its usual port).
pub const KINDS: &[(&str, &str, u16)] = &[
    ("sftp", "SFTP (SSH)", 22),
    ("davs", "WebDAV (HTTPS)", 443),
    ("dav", "WebDAV (HTTP)", 80),
    ("ftp", "FTP", 21),
    ("smb", "Windows share (SMB)", 445),
];

/// The parts of a server's address, as the Add Server form has them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parts {
    pub scheme: String,
    pub host: String,
    /// 0: the usual one.
    pub port: u16,
    pub user: String,
    pub folder: String,
}

pub fn path() -> PathBuf {
    glib::user_config_dir().join("failbrauwser").join("servers")
}

/// The address made of `p`: `sftp://me@host:2222/srv/files`.
pub fn build_uri(p: &Parts) -> String {
    let user = if p.user.is_empty() { String::new() } else { format!("{}@", glib::uri_escape_string(p.user.as_str(), None::<&str>, false)) };
    let host = p.host.trim();
    // An IPv6 address goes in brackets.
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    let port = if p.port == 0 { String::new() } else { format!(":{}", p.port) };
    let folder = p.folder.trim().trim_start_matches('/');
    let folder = folder.split('/').map(|s| glib::uri_escape_string(s, None::<&str>, false).to_string()).collect::<Vec<_>>().join("/");
    format!("{}://{user}{host}{port}/{folder}", p.scheme)
}

/// The parts of an address (for editing a server).
pub fn parse_uri(uri: &str) -> Option<Parts> {
    let (scheme, rest) = uri.split_once("://")?;
    let (authority, folder) = rest.split_once('/').unwrap_or((rest, ""));
    let (user, hostport) = authority.rsplit_once('@').map_or(("", authority), |(u, h)| (u, h));
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (h, after) = v6.split_once(']')?;
        (h.to_string(), after.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(0))
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap_or(0)),
            _ => (hostport.to_string(), 0),
        }
    };
    let unescape = |s: &str| glib::uri_unescape_string(s, None::<&str>).map(|x| x.to_string()).unwrap_or_else(|| s.to_string());
    Some(Parts { scheme: scheme.to_ascii_lowercase(), host, port, user: unescape(user), folder: unescape(folder) })
}

pub fn load_from(path: &Path) -> Vec<Server> {
    std::fs::read_to_string(path)
        .map(|s| {
            s.lines()
                .filter_map(|l| l.split_once('\t'))
                .filter(|(n, u)| !n.is_empty() && u.contains("://"))
                .map(|(n, u)| Server { name: n.to_string(), uri: u.to_string() })
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_to(path: &Path, servers: &[Server]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text: String = servers.iter().map(|s| format!("{}\t{}\n", s.name.replace(['\t', '\n'], " "), s.uri)).collect();
    std::fs::write(path, text)
}

pub fn load() -> Vec<Server> {
    load_from(&path())
}

pub fn save(servers: &[Server]) -> io::Result<()> {
    save_to(&path(), servers)
}

/// What to say under a server's name: "SFTP — me@host".
pub fn describe(uri: &str) -> String {
    let Some(p) = parse_uri(uri) else { return uri.to_string() };
    let kind = KINDS.iter().find(|k| k.0 == p.scheme).map_or(p.scheme.to_uppercase(), |k| k.1.to_string());
    let who = if p.user.is_empty() { p.host.clone() } else { format!("{}@{}", p.user, p.host) };
    let port = if p.port == 0 { String::new() } else { format!(":{}", p.port) };
    let folder = if p.folder.is_empty() { String::new() } else { format!("/{}", p.folder) };
    // A Host of ~/.ssh/config: where OpenSSH really goes.
    let via = if p.scheme == "sftp" {
        crate::sshconfig::resolve(&p.host).map(|r| format!(" ({}{})", r.user.as_deref().or((!p.user.is_empty()).then_some(p.user.as_str())).map(|u| format!("{u}@")).unwrap_or_default(), r.host_name)).unwrap_or_default()
    } else {
        String::new()
    };
    format!("{kind} — {who}{port}{folder}{via}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_from_their_parts_and_back() {
        let p = Parts { scheme: "sftp".into(), host: "pi.local".into(), port: 2222, user: "me".into(), folder: "/srv/My Files".into() };
        let uri = build_uri(&p);
        assert_eq!(uri, "sftp://me@pi.local:2222/srv/My%20Files");
        assert_eq!(parse_uri(&uri).unwrap(), Parts { folder: "srv/My Files".into(), ..p });
        let dav = Parts { scheme: "davs".into(), host: "cloud.example.org".into(), folder: "remote.php/dav/files/anna".into(), ..Default::default() };
        assert_eq!(build_uri(&dav), "davs://cloud.example.org/remote.php/dav/files/anna");
        let v6 = Parts { scheme: "ftp".into(), host: "fe80::1".into(), port: 2121, ..Default::default() };
        assert_eq!(build_uri(&v6), "ftp://[fe80::1]:2121/");
        assert_eq!(parse_uri("ftp://[fe80::1]:2121/").unwrap().host, "fe80::1");
        assert_eq!(describe("sftp://me@pi.local:2222/srv"), "SFTP (SSH) — me@pi.local:2222/srv");
        assert_eq!(describe("davs://cloud.example.org/dav"), "WebDAV (HTTPS) — cloud.example.org/dav");
        assert!(parse_uri("no address").is_none());
    }

    #[test]
    fn kept_one_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/servers");
        assert!(load_from(&p).is_empty());
        let list = vec![Server { name: "Raspberry\tPi".into(), uri: "sftp://pi@pi.local/".into() }, Server { name: "Cloud".into(), uri: "davs://cloud/dav".into() }];
        save_to(&p, &list).unwrap();
        let back = load_from(&p);
        assert_eq!(back[0].name, "Raspberry Pi");
        assert_eq!(back[1], list[1]);
    }
}
