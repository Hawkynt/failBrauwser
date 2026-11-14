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
//! Built as its own crate (winpack\onefile) on purpose: the launcher links nothing beyond the OS
//! loader itself — no GTK, no C runtime DLL — so the version comparison mirrors (and is tested
//! like) `update::is_newer` instead of linking the GTK stack.

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
    let build = option_env!("FB_BUILD_ID").unwrap_or("");
    let exe = prefix.join("bin").join("failbrauwser.exe");
    let stamp = prefix.join(".payload-version");
    let installed = std::fs::read_to_string(&stamp).map(|s| s.trim().to_string()).unwrap_or_default();
    if !exe.is_file() || needs_unpack((version, build), &installed) {
        let stamp_text = if build.is_empty() { version.to_string() } else { format!("{version} {build}") };
        if let Err(e) = install(PAYLOAD, &prefix).and_then(|()| std::fs::write(&stamp, stamp_text)) {
            report(&install_failure(version, &prefix, &e));
            exit(1);
        }
        refresh_loader_cache(&prefix.join("bin"));
    }
    let status = match Command::new(&exe).args(std::env::args_os().skip(1)).status() {
        Ok(s) => s,
        Err(e) => {
            report(&format!("{} could not start: {e}", exe.display()));
            exit(1);
        }
    };
    exit(status.code().unwrap_or(1));
}

/// Whether the payload (its version and build id) replaces the installed tree the stamp describes.
/// The version alone cannot answer a rebuild — both sides read 0.7.0 — so the build id (`package.ps1`'s
/// packaging time, riding in the tree as `payload-id.txt`) decides whenever the versions are equal.
/// A tree stamped without an id predates them and is unknown, so a payload that can date itself
/// replaces it; one that cannot leaves a stamped install alone.
fn needs_unpack(payload: (&str, &str), installed: &str) -> bool {
    let (a_version, a_build) = payload;
    let mut fields = installed.split(' ');
    let (b_version, b_build) = (fields.next().unwrap_or(""), fields.next().unwrap_or(""));
    if b_version.is_empty() {
        return true; // no stamp at all: the first run
    }
    match (is_newer(a_version, b_version), is_newer(b_version, a_version)) {
        (true, _) => true,
        (false, true) => false,
        _ => match (a_build.is_empty(), b_build.is_empty()) {
            (false, true) => true,
            (true, false) => false,
            (true, true) => false,
            (false, false) => a_build > b_build,
        },
    }
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

/// Where a failure sentence goes. Started from a terminal, the launcher answers there as it always
/// did; started with no console at all — a double-click, a browser download, a link — its standard
/// handles reach nobody, so the same text goes into a window. That is the case where an install
/// that failed read as nothing happening at all.
fn report(text: &str) {
    #[cfg(windows)]
    if !visible::console_attached() {
        visible::window(text);
        return;
    }
    let _ = writeln!(std::io::stderr(), "{text}");
}

/// The launcher's two Windows calls, declared here rather than pulled in as a dependency: the file
/// keeps importing nothing beyond the OS loader, and `kernel32` and `user32` are part of it.
#[cfg(windows)]
mod visible {
    use std::ffi::c_void;

    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut c_void;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    /// Whether a terminal started this process — which is also whether it has a place to print.
    pub fn console_attached() -> bool {
        !unsafe { GetConsoleWindow() }.is_null()
    }

    /// The one window the launcher ever opens: a failure the person who started it has to read.
    pub fn window(text: &str) {
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();
        let (body, caption) = (wide(text), wide("failBrauwser"));
        const MB_OK: u32 = 0x0;
        const MB_ICONWARNING: u32 = 0x30;
        unsafe { MessageBoxW(std::ptr::null_mut(), body.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONWARNING) };
    }
}

/// The sentence a launcher that could not install answers with: the release it carried, the
/// folder it tried to write, and what the system said. The system's half arrives in the language
/// of the machine, so the two sentences this one adds are what a person can always read: nothing
/// was touched, and — for a folder that is in use — what to close.
fn install_failure(version: &str, prefix: &std::path::Path, e: &std::io::Error) -> String {
    let mut text = format!("failBrauwser {version}: could not install into {}: {e}\n\nNothing was changed in that folder.", prefix.display());
    if in_use(e) {
        text.push_str(" Close a failBrauwser that is running from it, then start this file again.");
    }
    text
}

/// Whether the failure is one of a file that will not be written to — a running program file or one
/// another process holds, which for an install folder means an instance running out of it. The
/// system's own codes answer a move that went wrong, and `refused_in_use` marks the one the
/// pre-flight caught before staging anything. The in-app update can swap around a running program
/// file, which is why it is not this launcher's job.
fn in_use(e: &std::io::Error) -> bool {
    e.to_string().contains(IN_USE) || matches!(e.raw_os_error(), Some(32) | Some(5) | Some(1224))
}

/// The marker `refused_in_use` puts in its message, read back by `in_use` because a wrapped error
/// no longer carries the system code that produced it.
const IN_USE: &str = "in use";

fn refused_in_use(rel: &str, e: std::io::Error) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{rel} {IN_USE}: {e}"))
}

/// The folder a payload is staged in before a single byte of it reaches the installed tree.
/// Inside the install folder on purpose: moving a file from there into place is a same-volume
/// rename, which is where the all-or-nothing of `install` comes from. It costs room for a second
/// copy of the tree for the moment the unpack runs, and it is a dot-name so the tree's own layout
/// (`bin`, `lib`, `share`) never reads it as part of an install.
const STAGING: &str = ".payload-staging";

/// Unpacks the payload zip (the staging tree under its single `failbrauwser-*` root
/// folder) into `prefix`, replacing every file it carries; extra files a newer install
/// left behind stay.
///
/// Three steps, so that a payload which cannot be applied leaves the installed tree exactly as it
/// was rather than half of it: the entries are read and their names checked, every file they would
/// replace is tried for writability, and only then is the whole payload staged under `STAGING/new`
/// and moved into place — a move that fails partway being undone from the copies of what it replaced
/// under `STAGING/previous`. The writability step is where a running program file is refused: Windows
/// lets a running image be renamed aside, which is how the in-app update swaps one, and this launcher
/// deliberately does not touch one — a folder something is using is refused before it is written to.
fn install(payload: &[u8], prefix: &std::path::Path) -> std::io::Result<()> {
    let planned = planned_files(payload)?; // a payload that lies is refused before anything is written
    check_replaceable(&planned, prefix)?; // and a folder something is using is refused before staging it
    let stage = prefix.join(STAGING);
    let _ = std::fs::remove_dir_all(&stage); // an earlier attempt that died mid-way
    let (fresh, previous) = (stage.join("new"), stage.join("previous"));
    let mut applied = Vec::new();
    let outcome = apply_payload(payload, prefix, &planned, &fresh, &previous, &mut applied);
    if outcome.is_err() {
        rollback(&previous, prefix, &applied); // back to what the tree held before the first move
    }
    let _ = std::fs::remove_dir_all(&stage); // the copies it staged, fresh and previous alike
    outcome
}

/// Undoes the move phase back to front. An entry that replaced a file gives that file back from
/// its copy under `previous`; an entry the tree had never held is simply taken away again. Each
/// step is best-effort on purpose: the reason the install is being undone is the one worth
/// reporting, and a file the tree itself cannot give back is rarer than that failure.
fn rollback(previous: &std::path::Path, prefix: &std::path::Path, applied: &[(String, bool)]) {
    for (rel, replaced) in applied.iter().rev() {
        let target = prefix.join(rel.replace('/', "\\"));
        let _ = std::fs::remove_file(&target); // the file this attempt put there, if it is still there
        if *replaced {
            let _ = std::fs::rename(previous.join(rel.replace('/', "\\")), &target);
        }
    }
}

/// The payload's file entries paired with the path each one names inside the install folder: the
/// single root folder merged away, folder entries dropped, and an entry refused for climbing out of
/// the folder or for naming the staging folder. Their bytes are read and checked later.
fn planned_files(payload: &[u8]) -> std::io::Result<Vec<(Entry, String)>> {
    let root = common_root(payload);
    let mut out = Vec::new();
    for entry in central_directory(payload) {
        let full = entry.name.replace('\\', "/");
        let Some(rel) = full.strip_prefix(&root) else {
            return Err(invalid(format!("payload entry {} is not under the expected root {root}", entry.name)));
        };
        if rel.is_empty() || rel.ends_with('/') {
            continue; // folder entries carry no data
        }
        if rel.starts_with('/') || rel.as_bytes().get(1) == Some(&b':') || rel.split('/').any(|c| c == "..") {
            return Err(invalid(format!("payload entry {} escapes the install folder", entry.name)));
        }
        if rel.split('/').next() == Some(STAGING) {
            return Err(invalid(format!("payload entry {} sits where the payload stages itself", entry.name)));
        }
        out.push((entry, rel.to_string()));
    }
    Ok(out)
}

/// Every file the payload would replace, opened for writing and closed again — nothing else. Windows
/// answers that with a refusal for a running program file and for one another process holds, so the
/// launcher finds out here: before staging tens of megabytes it has nowhere to put, before any file
/// of a running install is renamed, and with the answer still saying which file.
fn check_replaceable(planned: &[(Entry, String)], prefix: &std::path::Path) -> std::io::Result<()> {
    for (_, rel) in planned {
        let target = prefix.join(rel.replace('/', "\\"));
        if !target.is_file() {
            continue; // nothing to replace, so nothing that can refuse
        }
        if let Err(e) = std::fs::OpenOptions::new().write(true).open(&target) {
            return Err(refused_in_use(rel, e));
        }
    }
    Ok(())
}

fn invalid(text: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, text)
}

fn apply_payload(payload: &[u8], prefix: &std::path::Path, planned: &[(Entry, String)], fresh: &std::path::Path, previous: &std::path::Path, applied: &mut Vec<(String, bool)>) -> std::io::Result<()> {
    let mut placed = Vec::with_capacity(planned.len());
    for (entry, rel) in planned {
        let compressed = payload.get(entry.data..entry.data + entry.csize).ok_or_else(|| invalid("payload is truncated".to_string()))?;
        let mut bytes = Vec::with_capacity(entry.size);
        match entry.method {
            0 => bytes.extend_from_slice(compressed),
            8 => {
                flate2::read::DeflateDecoder::new(compressed).read_to_end(&mut bytes)?;
            }
            m => return Err(invalid(format!("payload entry {} uses unsupported zip method {m}", entry.name))),
        }
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&bytes);
        if bytes.len() != entry.size || hasher.finalize() != entry.crc {
            return Err(invalid(format!("payload entry {} is corrupt", entry.name)));
        }
        let staged = fresh.join(rel.replace('/', "\\"));
        if let Some(parent) = staged.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&staged, &bytes)?;
        placed.push((staged, prefix.join(rel.replace('/', "\\")), rel.clone()));
    }
    for (staged, target, rel) in placed {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let replaced = target.is_file();
        if replaced {
            // what this entry replaces goes aside rather than being overwritten, so a later entry
            // that cannot be put in place gives it back
            let aside = previous.join(rel.replace('/', "\\"));
            if let Some(parent) = aside.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&target, &aside)?; // a refusal here changed nothing in the tree
            applied.push((rel.clone(), true));
        }
        std::fs::rename(&staged, &target)?;
        if !replaced {
            applied.push((rel, false)); // only a file this attempt actually brought in is its own to take away
        }
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

    /// A rebuild at the same version is a newer payload — the build id says so — and an installed
    /// tree stamped before build ids existed names an unknown tree, which gets replaced. An older
    /// payload never touches a newer install, and a payload that cannot date itself never touches
    /// a stamped one.
    #[test]
    fn a_same_version_rebuild_replaces_an_installed_tree() {
        assert!(needs_unpack(("0.7.0", "20261112T0700"), "0.7.0 20261110T1200"), "the same version built later is newer");
        assert!(!needs_unpack(("0.7.0", "20261110T1200"), "0.7.0 20261112T0700"), "an older payload leaves a newer install alone");
        assert!(!needs_unpack(("0.7.0", "20261112T0700"), "0.7.0 20261112T0700"), "the payload that is already installed does not unpack again");
        assert!(needs_unpack(("0.7.0", "20261112T0700"), "0.7.0"), "a stamp with no build id names an unknown tree");
        assert!(!needs_unpack(("0.7.0", ""), "0.7.0 20261112T0700"), "a payload that cannot date itself cannot claim to be newer");
        assert!(!needs_unpack(("0.7.0", ""), "0.7.0"), "with nothing but versions the old rule holds");
        assert!(needs_unpack(("0.8.0", ""), "0.7.0 20261112T0700"), "a higher version is newer whatever the ids say");
        assert!(!needs_unpack(("0.7.0", ""), "0.8.0 20261112T0700"), "and a lower one is not");
        assert!(needs_unpack(("0.7.0", "20261112T0700"), ""), "no stamp at all is a first run");
    }

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

    /// The contract of a refused unpack: nothing of it reaches the installed tree. An entry that
    /// cannot be written — a folder standing where the payload names a file — fails the whole
    /// install, and the folder is left holding exactly what it held: not one entry written ahead
    /// of it, no half-staged copy of the payload behind.
    #[test]
    fn a_payload_that_cannot_be_applied_leaves_the_installed_tree_as_it_was() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        std::fs::write(d.path().join("bin/keep.exe"), b"OLD").unwrap();
        std::fs::write(d.path().join("plain.txt"), b"OLD").unwrap();
        std::fs::create_dir_all(d.path().join("bin/blocked.exe")).unwrap(); // where a file belongs
        let (payload, names) = zip_of(&[("r/README.md", b"NEW".as_slice(), 0), ("r\\bin\\keep.exe", b"NEW".as_slice(), 0), ("r\\bin\\blocked.exe", b"NEW".as_slice(), 0)]);
        let before = tree_of(d.path());

        assert!(install(&payload, d.path()).is_err(), "an entry that cannot be written fails the install");
        assert_eq!(tree_of(d.path()), before, "the install folder holds exactly the files it held before");
        assert_eq!(std::fs::read(d.path().join("bin/keep.exe")).unwrap(), b"OLD", "the file the payload would have replaced keeps its content");
        assert_eq!(std::fs::read(d.path().join("plain.txt")).unwrap(), b"OLD");

        // A corrupt entry is refused the same way, and for the same reason: reading and verifying
        // the payload is finished before the installed tree is touched at all.
        let mut corrupt = payload.clone();
        corrupt[names[2] - 30] ^= 1; // the directory's CRC for the last entry
        std::fs::remove_dir_all(d.path().join("bin/blocked.exe")).unwrap();
        let intact = tree_of(d.path());
        assert!(install(&corrupt, d.path()).is_err(), "an entry failing its CRC fails the install");
        assert_eq!(tree_of(d.path()), intact, "and no file of the payload was written on the way to it");

        // With the blocker gone the same payload applies completely — including over a file that
        // is already there.
        assert!(install(&payload, d.path()).is_ok(), "the same payload applies once the folder is out of the way");
        assert_eq!(std::fs::read(d.path().join("bin/keep.exe")).unwrap(), b"NEW", "an installed file is replaced");
        assert_eq!(std::fs::read(d.path().join("README.md")).unwrap(), b"NEW");
        assert_eq!(std::fs::read(d.path().join("bin/blocked.exe")).unwrap(), b"NEW");
        assert!(tree_of(d.path()).iter().all(|p| !p.contains("staging")), "no staging folder is left behind: {:?}", tree_of(d.path()));
    }

    /// A file the installed tree holds and another process will not let go stops the install before
    /// a byte of the payload is staged: the answer names a file in use, so the message can say what
    /// to close, and the folder reads exactly as it did.
    #[cfg(windows)]
    #[test]
    fn a_file_in_use_stops_the_install_before_it_touches_the_folder() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("bin")).unwrap();
        std::fs::write(d.path().join("bin/keep.exe"), b"OLD").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(d.path().join("bin/keep.exe")).unwrap();
        let (payload, _) = zip_of(&[("r/README.md", b"NEW".as_slice(), 0), ("r\\bin\\keep.exe", b"NEW".as_slice(), 0)]);
        let before = tree_of(d.path());

        let e = install(&payload, d.path()).expect_err("a file another process holds cannot be replaced");
        assert!(in_use(&e), "the answer says a file is in use, so the message can say what to close: {e}");
        drop(held);

        assert_eq!(tree_of(d.path()), before, "nothing was written on the way to that answer");
        assert_eq!(std::fs::read(d.path().join("bin/keep.exe")).unwrap(), b"OLD");
        assert!(!d.path().join("README.md").exists(), "the entry that came before it was never put in place");
    }

    /// The sentence a launcher that could not install answers with names the release, the folder
    /// and the reason, and says plainly that the folder was left alone. A folder in use is told
    /// what to close; a failure with another cause is not blamed on one.
    #[test]
    fn an_install_failure_names_the_folder_and_what_to_do_about_it() {
        let in_use = install_failure("0.7.0", std::path::Path::new("P:\\Programs\\failbrauwser"), &std::io::Error::from_raw_os_error(32));
        assert!(in_use.contains("0.7.0") && in_use.contains("P:\\Programs\\failbrauwser"), "names the release and the folder: {in_use}");
        assert!(in_use.contains("Nothing was changed"), "says the installed tree is untouched: {in_use}");
        assert!(in_use.contains("running from it"), "a folder in use is told what to close: {in_use}");
        let corrupt = install_failure("0.7.0", std::path::Path::new("P:\\Programs\\failbrauwser"), &std::io::Error::new(std::io::ErrorKind::InvalidData, "payload entry bin/x.exe is corrupt"));
        assert!(corrupt.contains("payload entry bin/x.exe is corrupt"), "names the reason it got: {corrupt}");
        assert!(!corrupt.contains("running from it"), "a corrupt payload is not blamed on a running instance: {corrupt}");
    }

    /// Every path under `root`, as root-relative strings, sorted — so two states of the same
    /// folder compare as equal or not, without naming how the install stages its payload.
    fn tree_of(root: &std::path::Path) -> Vec<String> {
        let (mut out, mut dirs) = (Vec::new(), vec![root.to_path_buf()]);
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path.clone());
                }
                out.push(path.strip_prefix(root).unwrap().display().to_string().replace('\\', "/"));
            }
        }
        out.sort();
        out
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
