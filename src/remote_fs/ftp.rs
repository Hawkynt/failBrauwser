//! FTP through suppaftp's sync client: passive transfers, the same entry shape as SFTP.
//! FTP has no locking and no metadata beyond what LIST says — what the server tells is
//! what the folder is; the write-back rules of the UI are the guard.

use super::auth;
use super::{Entry, Target};
use std::io;
use std::path::Path;
use suppaftp::FtpError;
use suppaftp::FtpStream;

/// One connected, logged-in, passive-mode control channel.
pub struct Ftp {
    stream: std::sync::Mutex<FtpStream>,
}

pub fn open(t: &Target) -> io::Result<Ftp> {
    let user = if t.user.is_empty() { "anonymous".to_string() } else { t.user.clone() };
    let mut last = String::new();
    for attempt in 0..3 {
        let pass = auth::ask_password(t, attempt > 0).unwrap_or_default();
        let opened = (|| -> io::Result<FtpStream> {
            let mut c = suppaftp::FtpStream::connect((t.host.as_str(), t.port())).map_err(|e| io::Error::new(io::ErrorKind::NotConnected, e.to_string()))?;
            c.set_mode(suppaftp::Mode::Passive);
            c.login(&user, &pass).map_err(|e| io::Error::new(io::ErrorKind::PermissionDenied, e.to_string()))?;
            Ok(c)
        })();
        match opened {
            Ok(c) => return Ok(Ftp { stream: std::sync::Mutex::new(c) }),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempt < 2 => last = e.to_string(),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{user}@{} refused the credentials ({last}).", t.host))),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("unreachable loop"))
}

fn with<T>(f: &Ftp, op: impl FnOnce(&mut FtpStream) -> Result<T, FtpError>) -> io::Result<T> {
    let mut c = f.stream.lock().unwrap_or_else(|e| e.into_inner());
    op(&mut c).map_err(|e| {
        // A lost control channel must read as "dead" so the pool reconnects; a refused
        // command (UnexpectedResponse) is the server saying no, which the caller reports.
        let kind = match &e {
            FtpError::ConnectionError(_) | FtpError::BadResponse => io::ErrorKind::NotConnected,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(kind, e.to_string())
    })
}

/// `/a/b/` → `/a/b`; "" is the login folder ".".
fn path(p: &str) -> String {
    let p = p.trim_end_matches('/');
    if p.is_empty() { ".".into() } else { p.to_string() }
}

pub fn list(f: &Ftp, p: &str) -> io::Result<Vec<Entry>> {
    // suppaftp hands back raw LIST lines — servers print in one of two dialects, and
    // both are parsed below (unit-tested); anything unparsable is not a file.
    let lines = with(f, |c| c.list(Some(&path(p))))?;
    Ok(lines.iter().filter_map(|l| parse_list_line(l)).collect())
}

pub fn mkdir(f: &Ftp, p: &str) -> io::Result<()> {
    with(f, |c| c.mkdir(path(p)))
}

pub fn remove(f: &Ftp, p: &str, is_dir: bool) -> io::Result<()> {
    with(f, |c| if is_dir { c.rmdir(path(p)) } else { c.rm(path(p)) })
}

pub fn rename(f: &Ftp, from: &str, to: &str) -> io::Result<()> {
    with(f, |c| c.rename(path(from), path(to)))
}

pub fn read_to(f: &Ftp, p: &str, local_out: &Path) -> io::Result<u64> {
    let mut file = std::fs::File::create(local_out)?;
    with(f, |c| {
        c.retr(&path(p), |r| {
            let copied = std::io::copy(r, &mut file).map_err(FtpError::ConnectionError)?;
            Ok(copied)
        })
    })
}

pub fn write_from(f: &Ftp, local_in: &Path, p: &str) -> io::Result<u64> {
    let mut reader = std::fs::File::open(local_in)?;
    with(f, |c| c.put_file(path(p), &mut reader))
}

/// FTP has no stat: the parent's listing is the only answer the server gives.
pub fn exists(f: &Ftp, p: &str) -> bool {
    let p = path(p);
    let name = p.rsplit('/').next().unwrap_or("").to_string();
    let parent = match p.rsplit_once('/') {
        None => ".".to_string(),
        Some((base, _)) => if base.is_empty() { "/".into() } else { base.to_string() },
    };
    list(f, &parent).unwrap_or_default().iter().any(|e| e.name == name)
}

impl Ftp {
    pub fn quit(&self) {
        let mut c = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let _ = c.quit();
    }
}

/// One raw LIST line — the DOS/IIS dialect first (it starts with a date), then the
/// un*x ls dialect. LIST prints wall-clock times with no timezone on the wire; they
/// are read here as UTC, so dates carry and only a few hours can drift. The trailing
/// rest of the line is the name, which may itself contain spaces.
fn parse_list_line(line: &str) -> Option<Entry> {
    let line = line.trim_end_matches(['\r', '\n']);
    parse_dos_line(line).or_else(|| parse_unix_line(line))
}

/// The n first whitespace-run-separated fields, plus the untouched rest of the line.
fn take_fields(line: &str, n: usize) -> Option<(&str, Vec<&str>)> {
    let mut rest = line;
    let mut fields = Vec::with_capacity(n);
    for _ in 0..n {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    Some((rest.trim(), fields))
}

fn parse_dos_line(line: &str) -> Option<Entry> {
    let (name, f) = take_fields(line, 3)?;
    let (date, clock, kind) = (*f.get(0)?, *f.get(1)?, *f.get(2)?);
    if !looks_dos_date(date) || (!clock.ends_with("AM") && !clock.ends_with("PM")) {
        return None;
    }
    let is_dir = kind == "<DIR>";
    let size = if is_dir { 0 } else { kind.parse().unwrap_or(0) };
    let mtime = dos_epoch(date, clock)?;
    (!name.is_empty()).then(|| Entry { name: name.to_string(), size, mtime, is_dir, is_link: false })
}

fn looks_dos_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) && parts[0].len() == 2 && parts[1].len() == 2 && (parts[2].len() == 2 || parts[2].len() == 4)
}

fn dos_epoch(date: &str, clock: &str) -> Option<i64> {
    let mut d = date.split('-');
    let (mo, day) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let short: i64 = d.next()?.parse().ok()?;
    let year = if short >= 100 { short } else if short < 70 { 2000 + short } else { 1900 + short };
    let pm = clock[clock.len() - 2..].eq_ignore_ascii_case("PM");
    let am = clock[clock.len() - 2..].eq_ignore_ascii_case("AM");
    if !(am || pm) {
        return None;
    }
    let (h, m) = clock[..clock.len() - 2].split_once(':')?;
    let mut hour: i64 = h.parse().ok()?;
    let minute: i64 = m.split(':').next()?.parse().ok()?;
    hour = if pm && hour != 12 { hour + 12 } else if am && hour == 12 { 0 } else { hour };
    Some(epoch_secs(year, mo, day, hour, minute))
}

fn parse_unix_line(line: &str) -> Option<Entry> {
    let (name, f) = take_fields(line, 8)?;
    let mode = *f.get(0)?;
    let m = mode.as_bytes();
    if m.len() != 10 || !matches!(m[0], b'-' | b'd' | b'l' | b'c' | b'p' | b's' | b'b') || m[1..].iter().any(|b| !matches!(b, b'r' | b'w' | b'x' | b's' | b't' | b'T' | b'l' | b'?' | b'@' | b'-')) {
        return None;
    }
    let name = name.split(" -> ").next().unwrap_or(name);
    if name == "." || name == ".." {
        return None;
    }
    let mo = month(f[5])?;
    let day: i64 = f[6].parse().ok()?;
    let (year, hour, minute) = if let Some((h, m)) = f[7].split_once(':') {
        // "Mon DD HH:MM" means this year by protocol folklore — the only clock to take it from.
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        (1970 + secs / 31_556_952, h.parse().ok()?, m.parse().ok()?)
    } else {
        (f[7].parse().ok()?, 0, 0)
    };
    Some(Entry {
        name: name.to_string(),
        size: if mode.starts_with('d') { 0 } else { f[4].parse().unwrap_or(0) }, // a folder's block count is not its size
        mtime: epoch_secs(year, mo, day, hour, minute),
        is_dir: mode.starts_with('d'),
        is_link: mode.starts_with('l'),
    })
}

fn month(m: &str) -> Option<i64> {
    if let Ok(v @ 1..=12) = m.parse::<i64>() {
        return Some(v);
    }
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|x| m.eq_ignore_ascii_case(x)).map(|p| p as i64 + 1)
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's closed form), plus the
/// clock time — no calendar dependency for a LIST parser.
fn epoch_secs(year: i64, mo: i64, day: i64, hour: i64, minute: i64) -> i64 {
    let y = year - i64::from(mo <= 2);
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86_400 + hour * 3600 + minute * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_listing_lines_parse() {
        let d = parse_list_line("drwxr-xr-x  9 owner group    4096 Jan 15  2023 Documents").unwrap();
        assert_eq!((d.name.as_str(), d.is_dir, d.is_link, d.size), ("Documents", true, false, 0));
        let f = parse_list_line("-rw-r--r--  1 owner group    1024 Feb  3 10:30 two words.txt").unwrap();
        assert_eq!((f.name.as_str(), f.is_dir, f.size), ("two words.txt", false, 1024));
        let l = parse_list_line("lrwxrwxrwx  1 owner group       7 Jan  1  2020 link -> real").unwrap();
        assert_eq!((l.name.as_str(), l.is_link), ("link", true));
        for junk in ["total 40", "", "drwxr-xr-x 2 a b 4096 Jan  1  2020 .", "drwxr-xr-x 2 a b 4096 Jan  1  2020 ..", "-rw-r--r-- not a listing"] {
            assert!(parse_list_line(junk).is_none(), "{junk:?} must not parse as a file");
        }
    }

    #[test]
    fn dos_listing_lines_parse() {
        let d = parse_list_line("11-05-25  10:30AM       <DIR>          My Photos").unwrap();
        assert_eq!((d.name.as_str(), d.is_dir, d.is_link, d.size), ("My Photos", true, false, 0));
        assert!(d.mtime > 0);
        let f = parse_list_line("11-05-25  12:05AM            12345 movie.mkv").unwrap();
        assert_eq!((f.name.as_str(), f.is_dir, f.size), ("movie.mkv", false, 12345));
    }

    #[test]
    fn listing_times_are_epoch_seconds() {
        // The parsers read wall-clock LIST times as UTC: 2023-01-15 00:00 = 1673740800.
        assert_eq!(parse_list_line("drwxr-xr-x 2 a b 4096 Jan 15  2023 X").unwrap().mtime, 1_673_740_800);
        let late = parse_list_line("01-15-23  11:59PM                 1 x").unwrap().mtime;
        assert_eq!(late, 1_673_827_140, "PM must add twelve hours, 12AM must not");
        let midnight = parse_list_line("01-15-23  12:00AM                 1 x").unwrap().mtime;
        assert_eq!(midnight, 1_673_740_800);
    }
}
