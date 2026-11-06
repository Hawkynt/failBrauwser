//! Network addresses offered while typing a location: the ones opened before and the SMB
//! servers answering on the local network.

use gtk::glib;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How many opened addresses are kept.
pub const HISTORY_MAX: usize = 50;
/// The SMB port (SMB over TCP, no NetBIOS).
const SMB_PORT: u16 = 445;

pub fn history_path() -> PathBuf {
    glib::user_config_dir().join("failbrauwser").join("remote-history")
}

/// The same address however it was typed: no surrounding spaces, no trailing slash.
pub fn normalize(uri: &str) -> String {
    uri.trim().trim_end_matches('/').to_string()
}

/// Opened addresses, most recent first.
pub fn load_history_from(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|s| s.lines().map(normalize).filter(|l| !l.is_empty()).take(HISTORY_MAX).collect())
        .unwrap_or_default()
}

/// Puts `uri` first in the history file (once) and keeps the newest [`HISTORY_MAX`].
pub fn remember_in(path: &Path, uri: &str) -> io::Result<Vec<String>> {
    let uri = normalize(uri);
    let mut list = load_history_from(path);
    list.retain(|u| !u.eq_ignore_ascii_case(&uri));
    list.insert(0, uri);
    list.truncate(HISTORY_MAX);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, list.iter().map(|u| format!("{u}\n")).collect::<String>())?;
    Ok(list)
}

/// `\\host\share\dir` as it is being typed, spelled `smb://host/share/dir` (names unescaped,
/// a trailing separator kept): what suggestions are compared with.
pub fn unc_as_typed(text: &str) -> Option<String> {
    let rest = text.trim().strip_prefix(r"\\")?;
    let parts: Vec<&str> = rest.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let end = if rest.ends_with(['\\', '/']) { "/" } else { "" };
    Some(format!("smb://{}{end}", parts.join("/")))
}

/// The address of a Windows-style share name: `\\host\My Share` is `smb://host/My%20Share`.
/// None for anything else.
pub fn unc_to_uri(text: &str) -> Option<String> {
    let typed = unc_as_typed(text)?;
    let Some((host, path)) = typed.strip_prefix("smb://")?.split_once('/') else {
        return Some(typed);
    };
    let path = path.split('/').map(|s| glib::uri_escape_string(s, None::<&str>, false).to_string()).collect::<Vec<_>>().join("/");
    Some(format!("smb://{host}/{path}"))
}

/// The server part of `smb://host/share/…`, when the text has one followed by a slash.
pub fn smb_host(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix("smb://")?;
    let (host, _) = rest.split_once('/')?;
    let host = host.rsplit('@').next().unwrap_or(host);
    (!host.is_empty()).then_some(host)
}

/// Whether a suggestion fits what is typed: from the start, or from the server name on so
/// that typing `192.168` finds `smb://192.168.1.198`. `\\192.168` finds it too.
pub fn suggestion_matches(candidate: &str, typed: &str) -> bool {
    let typed = unc_as_typed(typed).unwrap_or_else(|| typed.to_string()).trim().to_lowercase();
    if typed.is_empty() {
        return false;
    }
    let c = candidate.to_lowercase();
    if c == typed {
        return false;
    }
    c.starts_with(&typed) || c.split_once("://").is_some_and(|(_, rest)| rest.starts_with(&typed))
}

/// An SMB server found on the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub addr: Ipv4Addr,
    /// From reverse DNS, when the network has it.
    pub name: Option<String>,
}

impl Host {
    pub fn uri(&self) -> String {
        format!("smb://{}", self.addr)
    }
}

/// The neighbors the OS saw recently: `/proc/net/arp` on Unix, the IP net table on Windows.
pub fn arp_neighbours() -> Vec<Ipv4Addr> {
    #[cfg(unix)]
    return std::fs::read_to_string("/proc/net/arp").map(|t| parse_arp(&t)).unwrap_or_default();
    #[cfg(windows)]
    return crate::win::netinfo::arp_table();
    #[cfg(not(any(unix, windows)))]
    Vec::new()
}

/// Addresses the kernel saw recently (`/proc/net/arp`), complete entries only.
pub fn parse_arp(text: &str) -> Vec<Ipv4Addr> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // Flags 0x0 is an incomplete entry: nobody answered.
            (f.len() >= 4 && f[2] != "0x0").then(|| f[0].parse().ok()).flatten()
        })
        .collect()
}

/// This machine's IPv4 addresses on private networks, with their prefix length.
pub fn local_networks() -> Vec<(Ipv4Addr, u32)> {
    #[cfg(unix)]
    return unix_local_networks();
    #[cfg(windows)]
    return crate::win::netinfo::local_networks();
    #[cfg(not(any(unix, windows)))]
    Vec::new()
}

#[cfg(unix)]
fn unix_local_networks() -> Vec<(Ipv4Addr, u32)> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a list we only read and free with freeifaddrs.
    unsafe {
        if libc::getifaddrs(&mut ifap) != 0 {
            return out;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            cur = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_netmask.is_null() || i32::from((*ifa.ifa_addr).sa_family) != libc::AF_INET {
                continue;
            }
            let addr = Ipv4Addr::from(u32::from_be((*(ifa.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr));
            let mask = u32::from_be((*(ifa.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr);
            if addr.is_private() {
                out.push((addr, mask.count_ones()));
            }
        }
        libc::freeifaddrs(ifap);
    }
    out
}

/// The addresses worth asking: the neighbours first, then the rest of each local network.
/// Networks wider than /24 are only searched in this machine's /24.
pub fn scan_targets(networks: &[(Ipv4Addr, u32)], neighbours: &[Ipv4Addr]) -> Vec<Ipv4Addr> {
    let own: Vec<Ipv4Addr> = networks.iter().map(|(a, _)| *a).collect();
    let mut out: Vec<Ipv4Addr> = Vec::new();
    let in_networks = |a: &Ipv4Addr| networks.iter().any(|(n, p)| same_net(*n, *a, (*p).max(24)));
    for a in neighbours.iter().filter(|a| in_networks(a)) {
        if !out.contains(a) && !own.contains(a) {
            out.push(*a);
        }
    }
    for (addr, prefix) in networks {
        let prefix = (*prefix).max(24);
        let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
        let base = u32::from(*addr) & mask;
        for host in 1..(!mask) {
            let a = Ipv4Addr::from(base | host);
            if !out.contains(&a) && !own.contains(&a) {
                out.push(a);
            }
        }
    }
    out
}

fn same_net(a: Ipv4Addr, b: Ipv4Addr, prefix: u32) -> bool {
    let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
    u32::from(a) & mask == u32::from(b) & mask
}

/// The name reverse DNS gives an address, if any.
fn reverse_name(addr: Ipv4Addr) -> Option<String> {
    #[cfg(unix)]
    return unix_reverse_name(addr);
    #[cfg(windows)]
    return crate::win::netinfo::reverse_name(addr);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = addr;
        None
    }
}

#[cfg(unix)]
fn unix_reverse_name(addr: Ipv4Addr) -> Option<String> {
    // SAFETY: plain C call on a stack sockaddr_in and buffer.
    unsafe {
        let mut sa: libc::sockaddr_in = std::mem::zeroed();
        sa.sin_family = libc::AF_INET as libc::sa_family_t;
        sa.sin_addr.s_addr = u32::from(addr).to_be();
        let mut buf = [0 as libc::c_char; 256];
        let r = libc::getnameinfo(
            &sa as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            buf.as_mut_ptr(),
            buf.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        );
        if r != 0 {
            return None;
        }
        let name = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
        Some(name.trim_end_matches('.').to_string()).filter(|n| !n.is_empty())
    }
}

/// The NetBIOS node status request (RFC 1002 4.2.17): "what are your names?" for `*`.
pub fn nbstat_request(id: u16) -> Vec<u8> {
    let mut p = Vec::with_capacity(50);
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    // "*" padded with zeros to 16 bytes, each byte as two letters A..P.
    p.push(32);
    for b in std::iter::once(b'*').chain(std::iter::repeat_n(0u8, 15)) {
        p.push(b'A' + (b >> 4));
        p.push(b'A' + (b & 0x0f));
    }
    p.push(0);
    p.extend_from_slice(&[0, 0x21, 0, 1]);
    p
}

/// The computer's own name in a node status answer: the first unique name of type 00
/// (workstation) or 20 (file server), not a group (workgroup) name.
pub fn parse_nbstat(answer: &[u8], id: u16) -> Option<String> {
    if answer.len() < 12 || answer[..2] != id.to_be_bytes() || answer[2] & 0x80 == 0 {
        return None;
    }
    // Header, the name (as sent, or a pointer to it), type, class, TTL, data length.
    let mut i = 12;
    i = skip_dns_name(answer, i)?;
    i += 10;
    let count = *answer.get(i)? as usize;
    i += 1;
    (0..count).find_map(|n| {
        let e = answer.get(i + n * 18..i + n * 18 + 18)?;
        let group = e[16] & 0x80 != 0;
        let name = String::from_utf8_lossy(&e[..15]).trim_end_matches([' ', '\0']).to_string();
        (!group && matches!(e[15], 0x00 | 0x20) && !name.is_empty()).then_some(name)
    })
}

/// The position after the DNS name starting at `i`.
fn skip_dns_name(p: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let len = *p.get(i)? as usize;
        if len == 0 {
            return Some(i + 1);
        }
        if len & 0xc0 == 0xc0 {
            return Some(i + 2);
        }
        i += 1 + len;
    }
}

/// The DNS name at `i`, following compression pointers.
fn read_dns_name(p: &[u8], mut i: usize) -> Option<String> {
    let mut labels: Vec<String> = Vec::new();
    for _ in 0..64 {
        let len = *p.get(i)? as usize;
        if len == 0 {
            return Some(labels.join("."));
        }
        if len & 0xc0 == 0xc0 {
            i = ((len & 0x3f) << 8) | *p.get(i + 1)? as usize;
            continue;
        }
        labels.push(String::from_utf8_lossy(p.get(i + 1..i + 1 + len)?).into_owned());
        i += 1 + len;
    }
    None
}

/// A DNS question for the name of `addr` (`4.3.2.1.in-addr.arpa`, PTR), as mDNS asks it.
pub fn ptr_request(id: u16, addr: Ipv4Addr) -> Vec<u8> {
    let mut p = Vec::with_capacity(48);
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    let o = addr.octets();
    for label in [o[3].to_string(), o[2].to_string(), o[1].to_string(), o[0].to_string(), "in-addr".into(), "arpa".into()] {
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&[0, 12, 0, 1]);
    p
}

/// The name in the first PTR record of a DNS answer.
pub fn parse_ptr(answer: &[u8], id: u16) -> Option<String> {
    if answer.len() < 12 || answer[..2] != id.to_be_bytes() || answer[2] & 0x80 == 0 {
        return None;
    }
    let questions = u16::from_be_bytes([answer[4], answer[5]]);
    let answers = u16::from_be_bytes([answer[6], answer[7]]);
    let mut i = 12;
    for _ in 0..questions {
        i = skip_dns_name(answer, i)? + 4;
    }
    for _ in 0..answers {
        i = skip_dns_name(answer, i)?;
        let kind = u16::from_be_bytes([*answer.get(i)?, *answer.get(i + 1)?]);
        let len = u16::from_be_bytes([*answer.get(i + 8)?, *answer.get(i + 9)?]) as usize;
        i += 10;
        if kind == 12 {
            return read_dns_name(answer, i).map(|n| n.trim_end_matches('.').to_string()).filter(|n| !n.is_empty());
        }
        i += len;
    }
    None
}

/// Sends `request` to `to` over UDP and hands the answer to `parse`; None after `timeout`.
fn ask_udp(to: SocketAddr, request: &[u8], timeout: Duration, parse: impl Fn(&[u8]) -> Option<String>) -> Option<String> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.set_read_timeout(Some(timeout)).ok()?;
    sock.send_to(request, to).ok()?;
    let deadline = std::time::Instant::now() + timeout;
    let mut buf = [0u8; 1500];
    while std::time::Instant::now() < deadline {
        let (n, from) = sock.recv_from(&mut buf).ok()?;
        if from.ip() == to.ip() {
            if let Some(name) = parse(&buf[..n]) {
                return Some(name);
            }
        }
    }
    None
}

/// The name a Windows or Samba computer answers to (NetBIOS, UDP port 137 at `to`).
pub fn netbios_name(to: SocketAddr, timeout: Duration) -> Option<String> {
    let id = (std::process::id() as u16) ^ 0x4e42;
    ask_udp(to, &nbstat_request(id), timeout, |a| parse_nbstat(a, id))
}

/// The name a computer announces itself under (`nas.local`), asked directly (mDNS, UDP
/// port 5353 at `to`).
pub fn mdns_name(to: SocketAddr, addr: Ipv4Addr, timeout: Duration) -> Option<String> {
    let id = (std::process::id() as u16) ^ 0x6d44;
    ask_udp(to, &ptr_request(id, addr), timeout, |a| parse_ptr(a, id))
}

/// The best name for a computer: from the network's DNS (`nas.fritz.box`), its own mDNS
/// name (`nas.local`), or its NetBIOS name (`NAS`).
pub fn host_name(addr: Ipv4Addr) -> Option<String> {
    let t = Duration::from_millis(700);
    reverse_name(addr)
        .or_else(|| mdns_name(SocketAddr::from((addr, 5353)), addr, t))
        .or_else(|| netbios_name(SocketAddr::from((addr, 137)), t))
}

/// Asks every address on the local networks whether it takes SMB connections, and what the
/// ones that do are called. Blocks for a second or two (many addresses are tried at once).
pub fn find_smb_servers() -> Vec<Host> {
    let neighbours = arp_neighbours();
    let targets = scan_targets(&local_networks(), &neighbours);
    let timeout = Duration::from_millis(400);
    let found: Vec<Ipv4Addr> = std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .chunks(targets.len().div_ceil(64).max(1))
            .map(|chunk| {
                s.spawn(move || {
                    chunk
                        .iter()
                        .copied()
                        .filter(|a| TcpStream::connect_timeout(&SocketAddr::from((*a, SMB_PORT)), timeout).is_ok())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    std::thread::scope(|s| {
        let handles: Vec<_> = found.iter().map(|a| s.spawn(move || Host { addr: *a, name: host_name(*a) })).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_newest_first_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/remote-history");
        assert!(load_history_from(&p).is_empty());
        remember_in(&p, "smb://a/x").unwrap();
        remember_in(&p, " smb://b/y/ ").unwrap();
        let list = remember_in(&p, "SMB://A/X/").unwrap();
        assert_eq!(list, vec!["SMB://A/X", "smb://b/y"]);
        assert_eq!(load_history_from(&p), list);
        for i in 0..HISTORY_MAX + 5 {
            remember_in(&p, &format!("smb://h/{i}")).unwrap();
        }
        assert_eq!(load_history_from(&p).len(), HISTORY_MAX);
    }

    #[test]
    fn hosts_in_typed_addresses() {
        assert_eq!(smb_host("smb://192.168.1.198/"), Some("192.168.1.198"));
        assert_eq!(smb_host("smb://me@nas/share/dir"), Some("nas"));
        assert_eq!(smb_host("smb://nas"), None);
        assert_eq!(smb_host("sftp://nas/"), None);
        assert_eq!(unc_as_typed(r"\\192.168.1.198\").as_deref(), Some("smb://192.168.1.198/"));
        assert_eq!(unc_as_typed(r"\\nas\My Music\dir").as_deref(), Some("smb://nas/My Music/dir"));
        assert_eq!(unc_as_typed(r"\\nas").as_deref(), Some("smb://nas"));
    }

    #[test]
    fn windows_share_names() {
        let uri = |s: &str| unc_to_uri(s);
        assert_eq!(uri(r"\\192.168.1.198").as_deref(), Some("smb://192.168.1.198"));
        assert_eq!(uri(r"\\nas\").as_deref(), Some("smb://nas/"));
        assert_eq!(uri(r"\\nas\share").as_deref(), Some("smb://nas/share"));
        assert_eq!(uri(r" \\nas\share\dir\ ").as_deref(), Some("smb://nas/share/dir/"));
        assert_eq!(uri(r"\\nas/share\dir").as_deref(), Some("smb://nas/share/dir"));
        assert_eq!(uri(r"\\nas\\share").as_deref(), Some("smb://nas/share"));
        assert_eq!(uri(r"\\nas\My Music\50% #1?").as_deref(), Some("smb://nas/My%20Music/50%25%20%231%3F"));
        assert_eq!(uri(r"\\me@nas\share").as_deref(), Some("smb://me@nas/share"));
        for not_one in [r"\\", r"\\\", r"\nas\share", "/home/me", "smb://nas/share", ""] {
            assert_eq!(uri(not_one), None, "{not_one}");
        }
    }

    #[test]
    fn matching_from_start_or_server() {
        assert!(suggestion_matches("smb://192.168.1.198", "sm"));
        assert!(suggestion_matches("smb://192.168.1.198", "192.168"));
        assert!(suggestion_matches("smb://192.168.1.198/GamingCrypt-Drive", "smb://192.168.1.198/gam"));
        assert!(!suggestion_matches("smb://192.168.1.198", "smb://192.168.1.198"));
        assert!(!suggestion_matches("smb://192.168.1.198", "/home"));
        assert!(!suggestion_matches("smb://192.168.1.198", ""));
        assert!(suggestion_matches("smb://192.168.1.198", r"\\192.168"));
        assert!(suggestion_matches("smb://nas/My Music", r"\\NAS\my m"));
        assert!(!suggestion_matches("smb://nas/My Music", r"\\nas\My Music"));
        assert!(!suggestion_matches("sftp://nas", r"\\nas"));
    }

    /// A node status answer from a Samba server called NAS in workgroup WORKGROUP.
    fn nbstat_answer(id: u16) -> Vec<u8> {
        let req = nbstat_request(id);
        let mut a = req[..12].to_vec();
        a[2] = 0x84;
        a[5] = 0;
        a[7] = 1;
        a.extend_from_slice(&req[12..46]);
        a.extend_from_slice(&[0, 0x21, 0, 1, 0, 0, 0, 0]);
        let names: [(&str, u8, u8); 3] = [("WORKGROUP", 0x00, 0x80), ("NAS", 0x00, 0x04), ("NAS", 0x20, 0x04)];
        a.extend_from_slice(&((1 + names.len() * 18 + 46) as u16).to_be_bytes());
        a.push(names.len() as u8);
        for (n, suffix, flags) in names {
            let mut e = format!("{n:<15}").into_bytes();
            e.push(suffix);
            e.extend_from_slice(&[flags, 0]);
            a.extend_from_slice(&e);
        }
        a.extend_from_slice(&[0u8; 46]);
        a
    }

    #[test]
    fn netbios_names() {
        let req = nbstat_request(7);
        assert_eq!(req.len(), 50);
        assert_eq!(&req[12..15], &[32, b'C', b'K']);
        assert_eq!(&req[46..], &[0, 0x21, 0, 1]);
        assert_eq!(parse_nbstat(&nbstat_answer(7), 7).as_deref(), Some("NAS"));
        // Someone else's answer, a request and junk are no names.
        assert_eq!(parse_nbstat(&nbstat_answer(7), 8), None);
        assert_eq!(parse_nbstat(&req, 7), None);
        assert_eq!(parse_nbstat(&[0x00, 0x07, 0x84], 7), None);
    }

    #[test]
    fn mdns_names() {
        let addr = Ipv4Addr::new(192, 168, 1, 198);
        let req = ptr_request(9, addr);
        assert_eq!(&req[12..16], b"\x03198");
        // The answer repeats the question and points back at its name.
        let mut a = req.clone();
        a[2] = 0x84;
        a[7] = 1;
        a.extend_from_slice(&[0xc0, 12, 0, 12, 0, 1, 0, 0, 0, 120]);
        let target = b"\x03nas\x05local\x00";
        a.extend_from_slice(&(target.len() as u16).to_be_bytes());
        a.extend_from_slice(target);
        assert_eq!(parse_ptr(&a, 9).as_deref(), Some("nas.local"));
        assert_eq!(parse_ptr(&a, 10), None);
        assert_eq!(parse_ptr(&req, 9), None);
        assert_eq!(parse_ptr(&a[..40], 9), None);
    }

    #[test]
    fn names_asked_over_udp() {
        // A stand-in computer answering NetBIOS and mDNS questions on this machine.
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = server.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            for _ in 0..2 {
                let (n, from) = server.recv_from(&mut buf).unwrap();
                let id = u16::from_be_bytes([buf[0], buf[1]]);
                let reply = if buf[n - 3] == 0x21 {
                    nbstat_answer(id)
                } else {
                    let mut a = buf[..n].to_vec();
                    a[2] = 0x84;
                    a[7] = 1;
                    a.extend_from_slice(&[0xc0, 12, 0, 12, 0, 1, 0, 0, 0, 120, 0, 9, 3, b'p', b'c', b'1', 3, b'l', b'a', b'n', 0]);
                    a
                };
                server.send_to(&reply, from).unwrap();
            }
        });
        let wait = Duration::from_secs(2);
        assert_eq!(netbios_name(at, wait).as_deref(), Some("NAS"));
        assert_eq!(mdns_name(at, Ipv4Addr::new(127, 0, 0, 1), wait).as_deref(), Some("pc1.lan"));
        t.join().unwrap();
        // Nobody answering: no name, after the timeout.
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        assert_eq!(netbios_name(silent.local_addr().unwrap(), Duration::from_millis(100)), None);
    }

    #[test]
    fn arp_table() {
        let t = "IP address       HW type     Flags       HW address            Mask     Device\n\
                 192.168.1.198    0x1         0x2         00:0c:e7:00:14:c8     *        wlan0\n\
                 192.168.1.50     0x1         0x0         00:00:00:00:00:00     *        wlan0\n";
        assert_eq!(parse_arp(t), vec![Ipv4Addr::new(192, 168, 1, 198)]);
    }

    #[test]
    fn targets_neighbours_first_without_self() {
        let own = Ipv4Addr::new(192, 168, 1, 220);
        let t = scan_targets(&[(own, 24)], &[Ipv4Addr::new(192, 168, 1, 198), Ipv4Addr::new(10, 0, 0, 1)]);
        assert_eq!(t[0], Ipv4Addr::new(192, 168, 1, 198));
        assert_eq!(t.len(), 253);
        assert!(!t.contains(&own));
        assert!(!t.contains(&Ipv4Addr::new(10, 0, 0, 1)));
        assert!(!t.contains(&Ipv4Addr::new(192, 168, 1, 255)));
        // A /16 is only searched around this machine.
        assert_eq!(scan_targets(&[(Ipv4Addr::new(10, 1, 2, 3), 16)], &[]).len(), 253);
    }
}
