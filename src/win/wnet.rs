//! Network shares the way Windows sees them: WNet connections (mapped drives and the
//! credential sessions behind bare `\\server\share` paths), and asking a computer for its
//! shared folders as a guest. Unlike GVfs there is no FUSE layer to hang — but a dead
//! server can still block a call, so callers wrap these in their own timeout thread
//! (`ops::gvfs::check` style).

use std::io;
use windows::core::{PWSTR, PCWSTR};
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_ACCOUNT_DISABLED, ERROR_ACCOUNT_RESTRICTION, ERROR_ALREADY_ASSIGNED, ERROR_BAD_NETPATH, ERROR_FILE_NOT_FOUND, ERROR_HOST_UNREACHABLE, ERROR_LOGON_FAILURE, ERROR_NO_MORE_ITEMS, ERROR_SEM_TIMEOUT, HANDLE, WIN32_ERROR};
use windows::Win32::NetworkManagement::WNet::{CONNECT_UPDATE_PROFILE, NETRESOURCEW, RESOURCETYPE_ANY, RESOURCETYPE_DISK, RESOURCE_CONNECTED, RESOURCE_GLOBALNET, WNET_OPEN_ENUM_USAGE, WNetAddConnection2W, WNetCancelConnection2W, WNetCloseEnum, WNetEnumResourceW, WNetOpenEnumW};

/// A connection this machine has: the drive letter it is mapped to (none for a bare
/// credential session behind a UNC path), its UNC target, and the server's comment.
pub struct Connection {
    pub local: Option<String>,
    pub remote: String,
    pub comment: String,
}

/// The Win32 codes that mean "this wants credentials", not "go away".
const LOCKED: [WIN32_ERROR; 4] = [ERROR_ACCESS_DENIED, ERROR_LOGON_FAILURE, ERROR_ACCOUNT_DISABLED, ERROR_ACCOUNT_RESTRICTION];
/// The codes that mean "there is no file server there (right now)".
const NOTHING: [WIN32_ERROR; 5] = [ERROR_BAD_NETPATH, ERROR_FILE_NOT_FOUND, ERROR_NETWORK_UNREACHABLE_ALT, ERROR_HOST_UNREACHABLE, ERROR_SEM_TIMEOUT];
/// `ERROR_NETWORK_UNREACHABLE` (123) under a local name — the crate's constant list for
/// Foundation keeps the number free of collisions with `ERROR_INVALID_PARAMETER`.
const ERROR_NETWORK_UNREACHABLE_ALT: WIN32_ERROR = WIN32_ERROR(123);

pub fn is_locked(e: &io::Error) -> bool {
    e.raw_os_error().is_some_and(|c| LOCKED.iter().any(|l| c == l.0 as i32))
}

pub fn is_gone(e: &io::Error) -> bool {
    e.raw_os_error().is_some_and(|c| NOTHING.iter().any(|n| c == n.0 as i32))
}

/// The string a UTF-16 pointer inside an enumeration buffer points at (the buffer outlives
/// the read — the caller frees it only after every entry was copied out).
unsafe fn wstr(p: PCWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while unsafe { *p.0.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p.0, len) })
}

/// `PWSTR` (out/in, what the struct fields carry) seen as a read-only wide string.
fn ro(p: PWSTR) -> PCWSTR {
    PCWSTR(p.0 as *const u16)
}

/// The shared dance: hand WNet a buffer for `SLOTS` entries and read the batch back.
/// `visit` sees every entry while the buffer is still intact.
const SLOTS: usize = 32;
fn enumerate(open: impl FnOnce(&mut HANDLE) -> WIN32_ERROR, mut visit: impl FnMut(&NETRESOURCEW)) -> WIN32_ERROR {
    let mut handle = HANDLE::default();
    let r = open(&mut handle);
    // Nothing to enumerate is an answer, not a failure to ask.
    if r == ERROR_NO_MORE_ITEMS {
        return ERROR_NO_MORE_ITEMS;
    }
    if r.is_err() {
        return r;
    }
    // One buffer for the entry array and the strings they point into; WNet places the
    // strings inside it, so every string is copied out before the buffer is reused.
    let mut buf: Vec<u8> = vec![0; SLOTS * std::mem::size_of::<NETRESOURCEW>()];
    let (mut count, mut size) = (SLOTS as u32, buf.len() as u32);
    let r = unsafe { WNetEnumResourceW(handle, &mut count, buf.as_mut_ptr().cast::<std::ffi::c_void>(), &mut size) };
    if r.is_ok() {
        for e in unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<NETRESOURCEW>(), count as usize) } {
            visit(e);
        }
    }
    unsafe { let _ = WNetCloseEnum(handle); };
    // One batch read: a host with more than SLOTS shares shows a page of them on the
    // Network page — revisit with a grow loop if a real server ever hits the cap.
    r
}

/// The computers the network browse (workgroup or domain master browser) announces:
/// single-component entries (`\\NAME`). Domains come out too; they answer nothing to a
/// share probe and the page filters them out. A dead master browser can block the call —
/// callers wrap it in their timeout like everywhere else.
pub fn browse_network() -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    let r = enumerate(
        |h| unsafe { WNetOpenEnumW(RESOURCE_GLOBALNET, RESOURCETYPE_ANY, WNET_OPEN_ENUM_USAGE(0), None, h) },
        |e| {
            let remote = unsafe { wstr(ro(e.lpRemoteName)) };
            let name = remote.trim_start_matches('\\');
            if !name.is_empty() && !name.contains('\\') {
                out.push(name.to_string());
            }
        },
    );
    if r.is_ok() || r == ERROR_NO_MORE_ITEMS {
        Ok(out)
    } else {
        Err(io::Error::from_raw_os_error(r.0 as i32))
    }
}

/// The connections this machine has as its own user: mapped network drives and the
/// credential sessions behind UNC paths.
pub fn connections() -> io::Result<Vec<Connection>> {
    let mut out = Vec::new();
    let r = enumerate(
        |h| unsafe { WNetOpenEnumW(RESOURCE_CONNECTED, RESOURCETYPE_DISK, WNET_OPEN_ENUM_USAGE(0), None, h) },
        |e| {
            let remote = unsafe { wstr(ro(e.lpRemoteName)) };
            if !remote.is_empty() {
                out.push(Connection { local: (!e.lpLocalName.is_null()).then(|| unsafe { wstr(ro(e.lpLocalName)) }), remote, comment: unsafe { wstr(ro(e.lpComment)) } });
            }
        },
    );
    if r.is_ok() || r == ERROR_NO_MORE_ITEMS {
        Ok(out)
    } else {
        Err(io::Error::from_raw_os_error(r.0 as i32))
    }
}

/// The UNC target a mapped drive letter points at (`W:` → `\\server\share`); None for a
/// local drive.
pub fn connection_for(local: &str) -> Option<String> {
    let name = crate::win::err::wide(local);
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32 - 1;
    let r = unsafe { windows::Win32::NetworkManagement::WNet::WNetGetConnectionW(PCWSTR(name.as_ptr()), Some(PWSTR(buf.as_mut_ptr())), &mut len) };
    r.is_ok().then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// Signs in to `unc` (`\\server\share`) with the current user unless a name is given.
/// Already-connected answers count as success: the session is there either way.
pub fn connect(unc: &str, user: Option<&str>, password: Option<&str>) -> io::Result<()> {
    let remote = crate::win::err::wide(unc);
    let (u, p) = (user.map(crate::win::err::wide), password.map(crate::win::err::wide));
    let nr = NETRESOURCEW { dwType: RESOURCETYPE_DISK, lpRemoteName: PWSTR(remote.as_ptr().cast_mut()), ..Default::default() };
    let r = unsafe { WNetAddConnection2W(&nr, p.as_deref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())), u.as_deref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())), Default::default()) };
    if r.is_ok() || r == ERROR_ALREADY_ASSIGNED {
        return Ok(());
    }
    Err(io::Error::from_raw_os_error(r.0 as i32))
}

/// Drops the connection to `unc` (mapped drive or credential session) and, when
/// `forget_profile`, the auto-reconnect entry as well.
pub fn disconnect(unc: &str, forget_profile: bool) -> io::Result<()> {
    let remote = crate::win::err::wide(unc);
    let flags = if forget_profile { CONNECT_UPDATE_PROFILE } else { Default::default() };
    let r = unsafe { WNetCancelConnection2W(PCWSTR(remote.as_ptr()), flags, true) };
    // ERROR_CONNECTION_UNAVAILABLE (2250): already disconnected — the wanted state.
    if r.is_ok() || r == WIN32_ERROR(2250) {
        return Ok(());
    }
    Err(io::Error::from_raw_os_error(r.0 as i32))
}

/// The folders `host` shares, as a guest (never shows a dialog): `(name, comment)` pairs;
/// hidden (`…$`) ones are left for the caller (`network::visible_share`). Errors carry the
/// Win32 code for [`is_locked`]/[`is_gone`] classification.
pub fn probe_shares(host: &str) -> io::Result<Vec<(String, String)>> {
    let server = crate::win::err::wide(&format!("\\\\{host}"));
    let filter = NETRESOURCEW { dwType: RESOURCETYPE_ANY, lpRemoteName: PWSTR(server.as_ptr().cast_mut()), ..Default::default() };
    let mut out = Vec::new();
    let r = enumerate(
        |h| unsafe { WNetOpenEnumW(RESOURCE_GLOBALNET, RESOURCETYPE_ANY, WNET_OPEN_ENUM_USAGE(0), Some(&filter), h) },
        |e| {
            // `\\host\share` → share; only real disk shares (print queues and IPC stay out).
            if e.dwType != RESOURCETYPE_DISK {
                return;
            }
            let remote = unsafe { wstr(ro(e.lpRemoteName)) };
            let mut parts = remote.trim_start_matches('\\').splitn(2, '\\');
            if let (Some(_host), Some(share)) = (parts.next(), parts.next()) {
                if !share.is_empty() {
                    out.push((share.to_string(), unsafe { wstr(ro(e.lpComment)) }));
                }
            }
        },
    );
    if r.is_ok() || r == ERROR_NO_MORE_ITEMS {
        Ok(out)
    } else {
        Err(io::Error::from_raw_os_error(r.0 as i32))
    }
}

/// `\\server\share\sub` for an `smb://server/share/sub` address; None for other schemes.
/// Percent-escapes are undone (the GVfs fuse view had unescaped the same way).
pub fn unc_for_smb(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("smb://")?.trim_end_matches('/');
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.rsplit('@').next()?;
    (!host.is_empty()).then(|| {
        let un = |s: &str| gtk::glib::uri_unescape_string(s, None::<&str>).map(|s| s.to_string()).unwrap_or_else(|| s.to_string());
        let path = path.split('/').filter(|s| !s.is_empty() && *s != ".").map(un).collect::<Vec<_>>().join("\\");
        if path.is_empty() {
            format!("\\\\{host}")
        } else {
            format!("\\\\{host}\\{path}")
        }
    })
}

/// `\\server[@SSL][@port]\path` for a `dav(s)://server[:port]/path` address: the WebDAV
/// redirector's own spelling, so WebDAV shares walk the same code as SMB ones. The path
/// sits under the server's DAV root (servers that publish WebDAV under a sub-path, like
/// `/remote.php/dav`, need that prefix spelled into the address). None for other schemes.
pub fn unc_for_webdav(uri: &str) -> Option<String> {
    let (scheme, rest) = uri.split_once("://").filter(|(s, _)| matches!(*s, "dav" | "davs" | "webdav" | "webdavs"))?;
    let rest = rest.trim_end_matches('/');
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = authority.rsplit_once(':').filter(|(_, p)| p.parse::<u16>().is_ok()).unwrap_or((authority, ""));
    let host = host.rsplit('@').next()?;
    (!host.is_empty()).then(|| {
        let un = |s: &str| gtk::glib::uri_unescape_string(s, None::<&str>).map(|s| s.to_string()).unwrap_or_else(|| s.to_string());
        // The port tag is what tells the redirector this is WebDAV and not SMB: https
        // spells @SSL (@port only when one was given), http always carries its port.
        let ssl = scheme == "davs" || scheme == "webdavs";
        let tag = if ssl { "@SSL".to_string() } else { format!("@{}", if port.is_empty() { "80".into() } else { port.to_string() }) };
        let port = if ssl && !port.is_empty() { format!("@{port}") } else { String::new() };
        let path = path.split('/').filter(|s| !s.is_empty() && *s != ".").map(un).collect::<Vec<_>>().join("\\");
        let head = format!("\\\\{host}{tag}{port}");
        if path.is_empty() {
            head
        } else {
            format!("{head}\\{path}")
        }
    })
}

/// The share address a local folder came from: `\\server\share[\sub]` to
/// `smb://server/share[/sub]`; None for anything that is not a UNC path.
pub fn smb_for_unc(path: &std::path::Path) -> Option<String> {
    let text = path.to_string_lossy().replace('\\', "/");
    let rest = text.strip_prefix("//")?;
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let host = parts.next()?;
    let esc = |s: &str| gtk::glib::uri_escape_string(s, None::<&str>, false).to_string();
    let share = esc(parts.next()?);
    let sub = parts.map(esc).collect::<Vec<_>>().join("/");
    Some(if sub.is_empty() { format!("smb://{host}/{share}") } else { format!("smb://{host}/{share}/{sub}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_map_both_ways() {
        assert_eq!(unc_for_smb("smb://nas/Music"), Some(r"\\nas\Music".into()));
        assert_eq!(unc_for_smb("smb://nas/"), Some(r"\\nas".into()));
        assert_eq!(unc_for_smb("smb://me@nas/My%20Files/x"), Some(r"\\nas\My Files\x".into()));
        assert_eq!(unc_for_smb("sftp://nas/x"), None);
        assert_eq!(unc_for_smb("smb:///share"), None);
        assert_eq!(unc_for_webdav("davs://cloud.example.org/files"), Some(r"\\cloud.example.org@SSL\files".into()));
        assert_eq!(unc_for_webdav("dav://lan/share"), Some(r"\\lan@80\share".into()));
        assert_eq!(unc_for_webdav("davs://lan:8443/a/b"), Some(r"\\lan@SSL@8443\a\b".into()));
        assert_eq!(unc_for_webdav("smb://nas/x"), None);
        assert_eq!(smb_for_unc(std::path::Path::new(r"\\nas\Music\2026")), Some("smb://nas/Music/2026".into()));
        assert_eq!(smb_for_unc(std::path::Path::new(r"\\nas")), None);
        assert_eq!(smb_for_unc(std::path::Path::new(r"C:\work")), None);
        assert_eq!(smb_for_unc(std::path::Path::new(r"\\nas\My Files")), Some("smb://nas/My%20Files".into()));
    }

    #[test]
    fn win32_classifications() {
        assert!(is_locked(&io::Error::from_raw_os_error(5)));
        assert!(is_locked(&io::Error::from_raw_os_error(1326)));
        assert!(is_gone(&io::Error::from_raw_os_error(53)));
        assert!(is_gone(&io::Error::from_raw_os_error(121)));
        assert!(is_gone(&io::Error::from_raw_os_error(123)));
        assert!(!is_gone(&io::Error::from_raw_os_error(5)));
        assert!(!is_locked(&io::Error::from_raw_os_error(64)));
    }

    #[test]
    #[cfg(windows)]
    #[ignore] // live network state: run by hand to see what the browse list holds
    fn the_network_browse_list_is_sane() {
        for name in browse_network().unwrap_or_default() {
            assert!(!name.contains('\\') && !name.is_empty(), "odd browse entry: {name:?}");
        }
    }

    #[test]
    #[cfg(windows)] // asks the loopback computer over the real redirector
    fn the_local_machine_answers_a_share_probe() {
        // Windows hosts always carry hidden shares (ADMIN$, C$): a local probe either
        // lists nothing visible, or says it wants credentials, or the redirector says
        // the machine is not listening for SMB — never a random failure, never a hang.
        match probe_shares("localhost") {
            Ok(shares) => assert!(shares.iter().all(|(n, _)| !n.ends_with('$')), "hidden shares stay in: {shares:?}"),
            Err(e) => assert!(is_locked(&e) || is_gone(&e), "unexpected local answer: {e}"),
        }
        // Enumerating our own connections must not fail when there are none.
        connections().ok();
    }
}
