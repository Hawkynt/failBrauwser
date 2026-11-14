//! Finding unused file names: "report (copy).pdf", "report (copy 2).pdf", "photo (2).jpg".

use std::ffi::{OsStr, OsString};
use std::path::Path;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

/// Splits "archive.tar.gz" into ("archive", ".tar.gz") and "a.txt" into ("a", ".txt").
/// Hidden files without a further dot keep their whole name as stem.
pub fn split_extension(name: &[u8]) -> (&[u8], &[u8]) {
    let lower = name.to_ascii_lowercase();
    if let Some(i) = find_subslice(&lower, b".tar.") {
        if i > 0 {
            return (&name[..i], &name[i..]);
        }
    }
    match name.iter().rposition(|&b| b == b'.') {
        Some(0) | None => (name, &[]),
        Some(i) => (&name[..i], &name[i..]),
    }
}

fn find_subslice(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// A name for `name` that does not exist in `dir`. With a label ("copy") the candidates are
/// "stem (copy).ext", "stem (copy 2).ext", …; without, "stem (2).ext", "stem (3).ext", ….
pub fn free_name(dir: &Path, name: &OsStr, label: Option<&str>) -> OsString {
    candidates(name, label).find(|c| std::fs::symlink_metadata(dir.join(c)).is_err()).expect("endless candidates")
}

/// Same as [`free_name`] but checks against a set of names instead of the disk.
pub fn free_name_in(existing: &dyn Fn(&OsStr) -> bool, name: &OsStr, label: Option<&str>) -> OsString {
    candidates(name, label).find(|c| !existing(c)).expect("endless candidates")
}

fn candidates<'a>(name: &'a OsStr, label: Option<&'a str>) -> impl Iterator<Item = OsString> + 'a {
    let (stem, ext) = split_os(name);
    (1u64..).map(move |n| {
        let tag = match (label, n) {
            (Some(l), 1) => l.to_string(),
            (Some(l), n) => format!("{l} {n}"),
            (None, n) => (n + 1).to_string(),
        };
        join_os(&stem, &ext, &tag)
    })
}

// The name algebra runs on the bytes the Unix filesystem stores; on Windows an OsString
// is UTF-16 and byte round-trips would corrupt non-ASCII names, so the same split runs
// over the wide units. '.' (0x2E) never appears inside a surrogate pair, so the rules match.
#[cfg(unix)]
fn split_os(name: &OsStr) -> (Vec<u8>, Vec<u8>) {
    let (stem, ext) = split_extension(name.as_bytes());
    (stem.to_vec(), ext.to_vec())
}

#[cfg(unix)]
fn join_os(stem: &[u8], ext: &[u8], tag: &str) -> OsString {
    let mut v = stem.to_vec();
    v.extend_from_slice(format!(" ({tag})").as_bytes());
    v.extend_from_slice(ext);
    OsString::from_vec(v)
}

#[cfg(windows)]
fn split_os(name: &OsStr) -> (Vec<u16>, Vec<u16>) {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = name.encode_wide().collect();
    split_extension_wide(&w)
}

#[cfg(windows)]
fn split_extension_wide(name: &[u16]) -> (Vec<u16>, Vec<u16>) {
    const DOT: u16 = 0x2E;
    const TAR: [u16; 5] = [DOT, 0x74, 0x61, 0x72, DOT]; // ".tar."
    let lower: Vec<u16> = name.iter().map(|c| if (0x41..=0x5A).contains(c) { *c + 32 } else { *c }).collect();
    if let Some(i) = lower.windows(TAR.len()).position(|w| w == TAR) {
        if i > 0 {
            return (name[..i].to_vec(), name[i..].to_vec());
        }
    }
    match name.iter().rposition(|u| *u == DOT) {
        Some(0) | None => (name.to_vec(), Vec::new()),
        Some(i) => (name[..i].to_vec(), name[i..].to_vec()),
    }
}

#[cfg(windows)]
fn join_os(stem: &[u16], ext: &[u16], tag: &str) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    let mut out = OsString::from_wide(stem);
    out.push(format!(" ({tag})"));
    out.push(OsString::from_wide(ext));
    out
}

#[cfg(not(any(unix, windows)))]
fn split_os(name: &OsStr) -> (String, String) {
    let n = name.to_string_lossy();
    let (stem, ext) = split_extension(n.as_bytes());
    (String::from_utf8_lossy(stem).into_owned(), String::from_utf8_lossy(ext).into_owned())
}

#[cfg(not(any(unix, windows)))]
fn join_os(stem: &str, ext: &str, tag: &str) -> OsString {
    OsString::from(format!("{stem} ({tag}){ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(split_extension(b"a.txt"), (&b"a"[..], &b".txt"[..]));
        assert_eq!(split_extension(b"x.TAR.gz"), (&b"x"[..], &b".TAR.gz"[..]));
        assert_eq!(split_extension(b".bashrc"), (&b".bashrc"[..], &b""[..]));
        assert_eq!(split_extension(b"noext"), (&b"noext"[..], &b""[..]));
        assert_eq!(split_extension(b".config.json"), (&b".config"[..], &b".json"[..]));
    }

    #[test]
    #[cfg(windows)] // twin of `extensions` over the wide name view
    fn extensions_wide() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let sp = |s: &str| {
            let (stem, ext) = split_extension_wide(&w(s));
            (String::from_utf16(&stem).unwrap(), String::from_utf16(&ext).unwrap())
        };
        assert_eq!(sp("a.txt"), ("a".into(), ".txt".into()));
        assert_eq!(sp("x.TAR.gz"), ("x".into(), ".TAR.gz".into()));
        assert_eq!(sp(".bashrc"), (".bashrc".into(), String::new()));
        assert_eq!(sp("noext"), ("noext".into(), String::new()));
        assert_eq!(sp("照片.tar.bz2"), ("照片".into(), ".tar.bz2".into()));
    }

    #[test]
    fn free_names_skip_existing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("r (copy).pdf"), "").unwrap();
        assert_eq!(free_name(dir.path(), OsStr::new("r.pdf"), Some("copy")), "r (copy 2).pdf");
        assert_eq!(free_name(dir.path(), OsStr::new("r.pdf"), None), "r (2).pdf");
        let taken = |n: &OsStr| n == "d (2)";
        assert_eq!(free_name_in(&taken, OsStr::new("d"), None), "d (3)");
    }
}
