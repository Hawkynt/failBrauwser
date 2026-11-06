//! The single-file Windows release: one executable holding the entire application —
//! the program, the archive helper, every GTK DLL and the runtime data — as an embedded
//! payload (the staging zip from winpack\package.ps1, staged in by winpack\onefile).
//! Running it unpacks that payload into the install folder (default
//! `%LOCALAPPDATA%\Programs\failbrauwser`, `FB_PORTABLE_PREFIX` overrides) whenever the
//! embedded version (FB_VERSION) is newer than the installed stamp — the first run
//! always — refreshes the pixbuf loader cache for the new location, and starts the
//! program from there; later runs only check the stamp and start it. Nothing is
//! registered in the system, and nothing lands next to the file itself.
//!
//! Built as its own crate (winpack\onefile) on purpose: the launcher must carry no DLL
//! imports of its own, so the version comparison mirrors (and is tested like)
//! `update::is_newer` instead of linking the GTK stack.

#![windows_subsystem = "windows"]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, exit};

/// The packaged payload: the same staging zip `winpack\package.ps1` produces, copied
/// into OUT_DIR by that crate's build.rs (FB_PAYLOAD names it).
const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.zip"));

fn main() {
    let version = option_env!("FB_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    let prefix = match std::env::var_os("FB_PORTABLE_PREFIX") {
        Some(p) => PathBuf::from(p),
        None => {
            let local = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("AppData\\Local").into_os_string())).expect("no per-user folder to install into");
            PathBuf::from(local).join("Programs\\failbrauwser")
        }
    };
    let exe = prefix.join("bin").join("failbrauwser.exe");
    let stamp = prefix.join(".payload-version");
    let installed = std::fs::read_to_string(&stamp).map(|s| s.trim().to_string()).unwrap_or_default();
    if !exe.is_file() || is_newer(version, &installed) {
        if let Err(e) = install(PAYLOAD, &prefix).and_then(|()| std::fs::write(&stamp, version)) {
            let _ = writeln!(std::io::stderr(), "failBrauwser {version}: could not install into {}: {e}", prefix.display());
            exit(1);
        }
        refresh_loader_cache(&prefix.join("bin"));
    }
    let status = match Command::new(&exe).args(std::env::args_os().skip(1)).status() {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "{} could not start: {e}", exe.display());
            exit(1);
        }
    };
    exit(status.code().unwrap_or(1));
}

/// Whether version `a` is newer than `b` — the rule of `update::is_newer`, duplicated on
/// purpose so this launcher stays free of the library crate (see the module header).
fn is_newer(a: &str, b: &str) -> bool {
    let numbers = |v: &str| v.trim().trim_start_matches(['v', 'V']).split(['.', '-']).map_while(|p| p.parse::<u64>().ok()).collect::<Vec<_>>();
    let (mut x, mut y) = (numbers(a), numbers(b));
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    x > y
}

/// Unpacks the payload zip (the staging tree under its single `failbrauwser-*` root
/// folder) into `prefix`, replacing every file it carries; extra files a newer install
/// left behind stay.
fn install(payload: &[u8], prefix: &std::path::Path) -> std::io::Result<()> {
    let root = common_root(payload);
    for entry in central_directory(payload) {
        let full = entry.name.replace('\\', "/");
        let Some(rel) = full.strip_prefix(&root) else {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("payload entry {} is not under the expected root {root}", entry.name)));
        };
        if rel.is_empty() || rel.ends_with('/') {
            continue; // folder entries carry no data
        }
        if rel.starts_with('/') || rel.as_bytes().get(1) == Some(&b':') || rel.split('/').any(|c| c == "..") {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("payload entry {} escapes the install folder", entry.name)));
        }
        let compressed = payload.get(entry.data..entry.data + entry.csize).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "payload is truncated"))?;
        let mut bytes = Vec::with_capacity(entry.size);
        match entry.method {
            0 => bytes.extend_from_slice(compressed),
            8 => {
                flate2::read::DeflateDecoder::new(compressed).read_to_end(&mut bytes)?;
            }
            m => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("payload entry {} uses unsupported zip method {m}", entry.name))),
        }
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&bytes);
        if bytes.len() != entry.size || hasher.finalize() != entry.crc {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("payload entry {} is corrupt", entry.name)));
        }
        let target = prefix.join(rel.replace('/', "\\"));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &bytes)?;
    }
    Ok(())
}

/// The loader cache lists absolute paths, so the copy from the building machine is
/// wrong here — exactly what winpack\install.ps1 rewrites. The query tool ships in bin
/// for this step; if it cannot run, dropping the stale cache puts gdk-pixbuf back into
/// scan mode, which needs no cache at all.
fn refresh_loader_cache(bin: &std::path::Path) {
    let Some(cache) = bin.parent().map(|p| p.join("lib\\gdk-pixbuf-2.0\\2.10.0\\loaders\\loaders.cache")) else { return };
    let text = match Command::new(bin.join("gdk-pixbuf-query-loaders.exe")).current_dir(bin).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => String::new(),
    };
    if text.contains("libpixbufloader") {
        if let Some(parent) = cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&cache, text.trim_end());
    } else {
        let _ = std::fs::remove_file(&cache);
    }
}

struct Entry {
    name: String,
    method: u16,
    crc: u32,
    csize: usize,
    size: usize,
    data: usize,
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The entries of the zip's central directory, each resolved to its data range through
/// its local header (whose name/extra lengths can differ from the directory's).
fn central_directory(payload: &[u8]) -> Vec<Entry> {
    let end = (payload.len().saturating_sub(65_557)..payload.len()).rev().find(|o| payload.get(*o..*o + 4) == Some(b"PK\x05\x06")).expect("payload is not a zip: no end-of-directory record");
    let (count, cd) = (le16(payload, end + 10) as usize, le32(payload, end + 16) as usize);
    let mut at = cd;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if payload.get(at..at + 4) != Some(b"PK\x01\x02") {
            break;
        }
        let (name_len, extra_len, comment_len) = (le16(payload, at + 28) as usize, le16(payload, at + 30) as usize, le16(payload, at + 32) as usize);
        let name = String::from_utf8_lossy(&payload[at + 46..at + 46 + name_len]).into_owned();
        let local = le32(payload, at + 42) as usize;
        let data = local + 30 + le16(payload, local + 26) as usize + le16(payload, local + 28) as usize;
        entries.push(Entry {
            name,
            method: le16(payload, at + 10),
            crc: le32(payload, at + 16),
            csize: le32(payload, at + 20) as usize,
            size: le32(payload, at + 24) as usize,
            data,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    entries
}

/// The one root folder the staging zip is built with, e.g.
/// `failbrauwser-0.6.0-x86_64-windows/` — the first entry's leading path part when
/// every entry shares it, otherwise empty (a flat payload installs as-is).
fn common_root(payload: &[u8]) -> String {
    let entries = central_directory(payload);
    let Some((head, _)) = entries.first().and_then(|e| e.name.split_once(|c| c == '/' || c == '\\')) else { return String::new() };
    if entries.iter().all(|e| e.name.starts_with(head)) {
        format!("{head}/")
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_like_the_updater() {
        assert!(is_newer("0.7", "0.6.0") && is_newer("v0.10", "0.9.9") && !is_newer("0.6", "0.6.0") && !is_newer("", "0.6.0"));
    }

    fn push16(file: &mut Vec<u8>, v: u16) {
        file.extend_from_slice(&v.to_le_bytes());
    }

    fn push32(file: &mut Vec<u8>, v: u32) {
        file.extend_from_slice(&v.to_le_bytes());
    }

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    /// A hand-written zip the reader must accept (entries of (name, content, method)),
    /// plus the offset of every entry name in the central directory — so a scenario can
    /// tamper with one. Names keep the backslashes the packaging machine's zip writes.
    fn zip_of(items: &[(&str, &[u8], u16)]) -> (Vec<u8>, Vec<usize>) {
        let (mut file, mut written, mut name_offsets) = (Vec::new(), Vec::new(), Vec::new());
        for (name, data, method) in items {
            let body = if *method == 8 { deflate(data) } else { data.to_vec() };
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(data);
            let (crc, local) = (hasher.finalize(), file.len() as u32);
            push32(&mut file, 0x0403_4b50); // local header
            push16(&mut file, 20); // version needed, flags, method
            push16(&mut file, 0);
            push16(&mut file, *method);
            push32(&mut file, 0); // time, date
            push32(&mut file, crc);
            push32(&mut file, body.len() as u32);
            push32(&mut file, data.len() as u32);
            push16(&mut file, name.len() as u16);
            push16(&mut file, 0); // no extra field
            file.extend_from_slice(name.as_bytes());
            file.extend_from_slice(&body);
            written.push((local, *name, *method, crc, body.len() as u32, data.len() as u32));
        }
        let cd = file.len() as u32;
        for (local, name, method, crc, csize, size) in &written {
            push32(&mut file, 0x0201_4b50); // central directory entry
            push16(&mut file, 20); // versions made-by and needed, flags
            push16(&mut file, 20);
            push16(&mut file, 0);
            push16(&mut file, *method);
            push32(&mut file, 0); // time, date
            push32(&mut file, *crc);
            push32(&mut file, *csize);
            push32(&mut file, *size);
            push16(&mut file, name.len() as u16);
            push16(&mut file, 0); // extra, comment, disk, internal attributes
            push16(&mut file, 0);
            push16(&mut file, 0);
            push16(&mut file, 0);
            push32(&mut file, 0); // external attributes
            push32(&mut file, *local);
            file.extend_from_slice(name.as_bytes());
            name_offsets.push(file.len() - name.len());
        }
        push32(&mut file, 0x0605_4b50); // end of central directory
        push16(&mut file, 0);
        push16(&mut file, 0); // disk numbers
        push16(&mut file, written.len() as u16);
        push16(&mut file, written.len() as u16); // entry counts
        let cd_size = (file.len() - cd as usize) as u32;
        push32(&mut file, cd_size);
        push32(&mut file, cd); // directory size and place
        push16(&mut file, 0); // no comment
        (file, name_offsets)
    }

    /// The reader the first run relies on: folder entries, both compression methods and
    /// the packed root folder unpack into the prefix, while a directory entry climbing
    /// out of it and a broken CRC are refused.
    #[test]
    fn unpacks_verifies_and_refuses_a_bad_payload() {
        let d = tempfile::tempdir().unwrap();
        let big = "x".repeat(5000);
        let (payload, names) = zip_of(&[("r/", b"", 0), ("r\\bin\\thing.txt", big.as_bytes(), 8), ("r\\plain.txt", b"stored text", 0)]);
        install(&payload, d.path()).unwrap(); // the root folder itself is merged into the prefix
        assert_eq!(std::fs::read(d.path().join("bin/thing.txt")).unwrap(), big.as_bytes());
        assert_eq!(std::fs::read(d.path().join("plain.txt")).unwrap(), b"stored text");

        let mut climbing = payload.clone();
        climbing[names[2]..names[2] + 11].copy_from_slice(b"..\\..\\x.txt"); // same name length
        assert!(install(&climbing, d.path()).is_err(), "an entry climbing out of the prefix is refused");

        let mut corrupt = payload.clone();
        corrupt[names[2] - 30] ^= 1; // the directory's CRC for the stored entry
        assert!(install(&corrupt, d.path()).is_err(), "a file failing its CRC is refused");
    }

    /// A zip without one shared root folder installs flat, exactly as packed.
    #[test]
    fn a_flat_payload_installs_as_is() {
        let d = tempfile::tempdir().unwrap();
        let (payload, _) = zip_of(&[("bin/x.exe", b"X".as_slice(), 0), ("share/y.txt", b"y".as_slice(), 0)]);
        install(&payload, d.path()).unwrap();
        assert_eq!(std::fs::read(d.path().join("bin/x.exe")).unwrap(), b"X");
        assert_eq!(std::fs::read(d.path().join("share/y.txt")).unwrap(), b"y");
    }
}
