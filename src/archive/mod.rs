//! Archives as directories: typed requests to the helper and the directory tree of a listing.

pub mod client;
pub mod formats;
pub mod options;
pub mod tree;
pub mod vfs;

pub use client::{ArchiveError, Helper, Result};
pub use tree::{ArchiveNode, ArchiveTree};

use serde_json::{Value, json};
use std::path::Path;

/// One entry as the helper lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ArchiveEntry {
    /// The name exactly as stored; needed nowhere but kept for diagnostics.
    pub raw: String,
    /// Normalized: relative, forward slashes, no trailing slash.
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub compressed: u64,
    pub mtime: Option<i64>,
    pub encrypted: bool,
    pub method: String,
}

#[derive(Debug, Clone)]
pub struct Listing {
    pub format: String,
    pub writable: bool,
    /// Why a normally editable format is read-only here (hybrid ISO images).
    pub readonly_reason: Option<String>,
    pub entries: Vec<ArchiveEntry>,
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn with_password(mut req: Value, password: Option<&str>) -> Value {
    if let Some(pw) = password {
        req["password"] = json!(pw);
    }
    req
}

fn parse_listing(v: &Value) -> Listing {
    let entries = v["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| ArchiveEntry {
                    raw: e["raw"].as_str().unwrap_or_default().to_string(),
                    name: e["name"].as_str().unwrap_or_default().to_string(),
                    is_dir: e["dir"].as_bool().unwrap_or(false),
                    size: e["size"].as_i64().unwrap_or(0).max(0) as u64,
                    compressed: e["csize"].as_i64().unwrap_or(0).max(0) as u64,
                    mtime: e["mtime"].as_i64(),
                    encrypted: e["enc"].as_bool().unwrap_or(false),
                    method: e["method"].as_str().unwrap_or_default().to_string(),
                })
                .filter(|e| !e.name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Listing {
        format: v["format"].as_str().unwrap_or("Unknown").to_string(),
        writable: v["writable"].as_bool().unwrap_or(false),
        readonly_reason: v["readonly_reason"].as_str().map(str::to_string),
        entries,
    }
}

impl Helper {
    pub fn list(&self, archive: &Path, password: Option<&str>) -> Result<Listing> {
        let v = self.request(with_password(json!({"cmd": "list", "archive": path_str(archive)}), password))?;
        Ok(parse_listing(&v))
    }

    /// Lists the file as the format with this id (see [`formats`]), skipping detection: that kind's
    /// own reader runs over the bytes whatever the name suggests (M14 *Open as*). A kind the helper
    /// cannot open is refused by name.
    pub fn list_as(&self, archive: &Path, format: &str, password: Option<&str>) -> Result<Listing> {
        let v = self.request(with_password(json!({"cmd": "list", "archive": path_str(archive), "format": format}), password))?;
        Ok(parse_listing(&v))
    }

    /// Is this file something the helper can open as a folder?
    pub fn probe(&self, path: &Path) -> Result<(bool, String, bool)> {
        let v = self.request(json!({"cmd": "probe", "path": path_str(path)}))?;
        Ok((
            v["archive"].as_bool().unwrap_or(false),
            v["format"].as_str().unwrap_or_default().to_string(),
            v["writable"].as_bool().unwrap_or(false),
        ))
    }

    /// Extracts the given entries (or everything for `None`) below `dest`, keeping their
    /// archive-relative paths. A folder brings its whole subtree.
    pub fn extract(&self, archive: &Path, entries: Option<&[String]>, dest: &Path, password: Option<&str>) -> Result<()> {
        let mut req = json!({"cmd": "extract", "archive": path_str(archive), "dest": path_str(dest)});
        if let Some(e) = entries {
            req["entries"] = json!(e);
        }
        self.request(with_password(req, password)).map(|_| ())
    }

    /// Extracts through the reader of the format with this id, skipping detection (M14 *Open as*).
    /// A wrong kind answers with that reader's own error and leaves nothing behind.
    pub fn extract_as(&self, archive: &Path, entries: Option<&[String]>, dest: &Path, format: &str, password: Option<&str>) -> Result<()> {
        let mut req = json!({"cmd": "extract", "archive": path_str(archive), "dest": path_str(dest), "format": format});
        if let Some(e) = entries {
            req["entries"] = json!(e);
        }
        self.request(with_password(req, password)).map(|_| ())
    }

    /// Adds files or folders from disk under the given entry names. A missing archive is created.
    pub fn add(&self, archive: &Path, items: &[(std::path::PathBuf, String)], password: Option<&str>) -> Result<()> {
        let items: Vec<Value> = items.iter().map(|(src, name)| json!({"src": path_str(src), "name": name})).collect();
        self.request(with_password(json!({"cmd": "add", "archive": path_str(archive), "items": items}), password)).map(|_| ())
    }

    /// Creates a new archive in the format with this id (see [`formats`]) from files or
    /// folders on disk, whatever its name suggests.
    pub fn create(&self, archive: &Path, format: &str, items: &[(std::path::PathBuf, String)], password: Option<&str>) -> Result<()> {
        self.create_with(archive, format, items, password, None)
    }

    /// Like [`Helper::create`], written with these settings (method, level, version, …).
    pub fn create_with(&self, archive: &Path, format: &str, items: &[(std::path::PathBuf, String)], password: Option<&str>, settings: Option<&options::CreateSettings>) -> Result<()> {
        let items: Vec<Value> = items.iter().map(|(src, name)| json!({"src": path_str(src), "name": name})).collect();
        let mut req = json!({"cmd": "add", "archive": path_str(archive), "items": items, "format": format});
        if let Some(s) = settings {
            req["settings"] = s.to_json();
        }
        self.request(with_password(req, password)).map(|_| ())
    }

    /// The methods and settings a new archive of this format offers.
    pub fn options(&self, format: &str) -> Result<options::FormatOptions> {
        self.request(json!({"cmd": "options", "format": format})).map(|v| options::FormatOptions::from_json(&v))
    }

    pub fn remove(&self, archive: &Path, names: &[String], password: Option<&str>) -> Result<()> {
        self.request(with_password(json!({"cmd": "remove", "archive": path_str(archive), "names": names}), password)).map(|_| ())
    }

    pub fn rename(&self, archive: &Path, from: &str, to: &str, password: Option<&str>) -> Result<()> {
        self.request(with_password(json!({"cmd": "rename", "archive": path_str(archive), "from": from, "to": to}), password)).map(|_| ())
    }

    pub fn mkdir(&self, archive: &Path, name: &str, password: Option<&str>) -> Result<()> {
        self.request(with_password(json!({"cmd": "mkdir", "archive": path_str(archive), "name": name}), password)).map(|_| ())
    }
}

/// Archive types that always open as a folder on double click, whatever else claims them.
const BROWSABLE_SUFFIXES: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz", ".tar.lzma", ".tar.lz4", ".tar.z", ".tgz", ".tbz",
    ".tbz2", ".txz", ".tzst", ".tlz", ".zip", ".7z", ".rar", ".tar", ".cpio", ".ar", ".deb", ".rpm", ".cab", ".lzh",
    ".lha", ".arj", ".cbz", ".cbr", ".cb7", ".cbt", ".xar", ".wim", ".zpaq", ".ace", ".zoo", ".arc", ".sit", ".sitx",
    ".gz", ".bz2", ".xz", ".zst", ".lz", ".lzma", ".lz4",
    // Disc images (ISO 9660 with Rock Ridge / Joliet names).
    ".iso",
];

/// Suffixes the archive library understands but whose files belong to another program:
/// documents, e-books, pictures, fonts, programs and installers, databases and data files,
/// ROMs, and names too generic to guess from (`.bin`, `.dat`, `.txt`). They open with their
/// application; *Open as Archive* still shows what is inside.
const NOT_BY_NAME: &[&str] = &[
    // Office documents, e-books, mail, drawings.
    ".doc", ".docx", ".docm", ".dot", ".dotx", ".dotm", ".xls", ".xlsx", ".xlsm", ".xlt", ".xltx", ".xltm", ".ppt",
    ".pptx", ".pptm", ".pps", ".ppsx", ".ppsm", ".pot", ".potx", ".potm", ".odt", ".ods", ".odp", ".vsdx", ".vsdm",
    ".vssx", ".vssm", ".vstx", ".vstm", ".one", ".onetoc2", ".pub", ".wpd", ".wp", ".wp5", ".wp6", ".wp7", ".pmd",
    ".xps", ".oxps", ".pdf", ".epub", ".mobi", ".azw", ".azw3", ".fb2", ".lit", ".chm", ".prc", ".msg", ".eml",
    ".mbox", ".mbx", ".pst", ".ost", ".sketch", ".ai", ".fla", ".swf", ".swc", ".fig", ".dxf", ".dae", ".3ds", ".obj",
    ".ply", ".stl", ".tnef",
    // Pictures, fonts, media.
    ".gif", ".apng", ".mng", ".tif", ".tiff", ".bigtiff", ".ico", ".cur", ".ani", ".icns", ".jxl", ".qoi", ".ktx2",
    ".mpo", ".dpx", ".dcm", ".dicom", ".dcmdir", ".fits", ".fts", ".fit", ".flc", ".fli", ".ttf", ".otf", ".ttc",
    ".otc", ".ts", ".m2ts", ".mts", ".flac", ".mp3", ".umx", ".psf", ".psf2", ".minipsf", ".minipsf2", ".2sf", ".gsf",
    ".ncsf", ".snsf", ".ssf", ".dsf", ".usf", ".sup", ".idx", ".m3u", ".m3u8", ".cue", ".fsb", ".bnk", ".awb", ".acb",
    // Programs, libraries, installers, packages that install.
    ".exe", ".dll", ".so", ".o", ".a", ".ko", ".elf", ".dylib", ".macho", ".ocx", ".cpl", ".sys", ".com", ".mui",
    ".mun", ".resource.dll", ".wasm", ".jar", ".war", ".ear", ".apk", ".apks", ".aab", ".ipa", ".xpi", ".crx",
    ".msi", ".msp", ".mst", ".msix", ".msixbundle", ".appx", ".appimage", ".snap", ".sh", ".lnk", ".reg", ".efi",
    // Databases and data files.
    ".db", ".db3", ".sqlite", ".sqlite3", ".mdb", ".accdb", ".ldb", ".mdt", ".hdf", ".hdf4", ".hdf5", ".h4", ".h5",
    ".nc", ".cdf", ".parquet", ".feather", ".arrow", ".avro", ".orc", ".onnx", ".pkl", ".pickle", ".npy", ".npz",
    ".msgpack", ".mat", ".nrbf", ".storable", ".pcap", ".pcapng", ".json", ".xml", ".txt", ".cwb.json", ".cwb.xml",
    ".mo", ".dtb", ".dtbo", ".hex", ".ihex", ".ihx", ".srec", ".s19", ".s28", ".s37", ".mot", ".h86", ".tfrecord",
    ".tfrecords", ".p12", ".pfx", ".hive", ".hiv", ".pol", ".nii", ".nii.gz", ".kmz", ".par2", ".cap", ".vdf",
    ".aa",
    // Source code and translations that share a suffix with a format.
    ".pas", ".po", ".ovl", ".ufo", ".bundle",
    // Game ROMs and snapshots (emulators open them).
    ".nes", ".sfc", ".smc", ".gb", ".gbc", ".nds", ".z80", ".sna", ".tap", ".tzx", ".nsp", ".lnx",
    // Too generic.
    ".bin", ".dat", ".f", ".do", ".as", ".001", ".sst", ".pp", ".mem", ".smart", ".share", ".sea", ".win",
];

/// True when a file with this name opens as a folder on activation: a known archive,
/// compressed file or disk image that no other program owns (see [`NOT_BY_NAME`]).
pub fn is_browsable_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    // The first part of a split archive ("x.7z.001") opens as the archive.
    if let Some(stem) = lower.strip_suffix(".001") {
        return stem.contains('.') && is_browsable_name(stem);
    }
    let ends = |s: &&str| lower.len() > s.len() && lower.ends_with(*s);
    if BROWSABLE_SUFFIXES.iter().any(ends) {
        return true;
    }
    formats::by_name(&lower).is_some() && !NOT_BY_NAME.iter().any(ends)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsable_names() {
        assert!(is_browsable_name("a.ZIP"));
        assert!(is_browsable_name("backup.7z.001"));
        assert!(is_browsable_name("photos.tar.gz.001"));
        assert!(!is_browsable_name("film.001"));
        assert!(!is_browsable_name("notes.txt.001"));
        assert!(is_browsable_name("x.tar.gz"));
        assert!(is_browsable_name("ubuntu-26.04-desktop-amd64.ISO"));
        assert!(!is_browsable_name(".zip"));
        assert!(!is_browsable_name("doc.odt"));
        assert!(!is_browsable_name("zip"));
        // Everything else the library knows, unless another program owns it.
        assert!(is_browsable_name("disk.vhdx"));
        assert!(is_browsable_name("notes.txt.br"));
        assert!(is_browsable_name("old.lzh"));
        assert!(is_browsable_name("floppy.d64"));
        assert!(!is_browsable_name("report.docx"));
        assert!(!is_browsable_name("setup.exe"));
        assert!(!is_browsable_name("firmware.bin"));
        assert!(!is_browsable_name("readme.txt"));
        assert!(!is_browsable_name("font.ttf"));
        assert!(!is_browsable_name("de.po"));
    }

    #[test]
    fn formats_by_name_prefer_the_longest_suffix() {
        assert_eq!(formats::by_name("a.tar.gz").map(|f| f.kind), Some(formats::Kind::Tar));
        assert_eq!(formats::by_name("a.gz").map(|f| f.kind), Some(formats::Kind::Stream));
        assert_eq!(formats::by_name("A.ZIP").map(|f| f.id), Some("Zip"));
        assert!(formats::by_name("zip").is_none());
    }

    #[test]
    fn listing_is_parsed_and_empty_names_dropped() {
        let v = json!({"format": "Zip", "writable": true, "entries": [
            {"raw": "a/", "name": "a", "dir": true, "size": 0, "csize": 0, "enc": false, "method": "Store"},
            {"raw": "/", "name": "", "dir": true, "size": 0, "csize": 0, "enc": false, "method": ""},
            {"raw": "a/b", "name": "a/b", "dir": false, "size": 5, "csize": 3, "enc": true, "method": "Deflate", "mtime": 7}
        ]});
        let l = parse_listing(&v);
        assert_eq!(l.format, "Zip");
        assert!(l.writable);
        assert_eq!(l.entries.len(), 2);
        assert_eq!(l.entries[1].mtime, Some(7));
        assert!(l.entries[1].encrypted);
    }
}
