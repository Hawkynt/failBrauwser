//! Where a tab is: a folder, a folder inside an archive (possibly nested), or the drives page.
//!
//! Archive locations read like paths that continue through the archive file:
//! `/home/me/backup.tar.gz/photos/2024` or, nested, `/data/outer.zip/inner.7z/docs`.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ArchiveLoc {
    /// The archive file on disk.
    pub file: PathBuf,
    /// Archives inside archives: entry paths, each inside the previous archive.
    pub nested: Vec<String>,
    /// Folder inside the innermost archive, "" for its root.
    pub inner: String,
}

impl ArchiveLoc {
    pub fn root(file: PathBuf) -> Self {
        ArchiveLoc { file, nested: Vec::new(), inner: String::new() }
    }

    /// The same archive, a different folder in it.
    pub fn with_inner(&self, inner: &str) -> Self {
        ArchiveLoc { file: self.file.clone(), nested: self.nested.clone(), inner: inner.trim_matches('/').to_string() }
    }

    /// The path of `name` inside the current folder of the innermost archive.
    pub fn entry_path(&self, name: &str) -> String {
        if self.inner.is_empty() { name.to_string() } else { format!("{}/{}", self.inner, name) }
    }

    /// The archive containing this one's innermost archive, for nested archives.
    pub fn container(&self) -> Option<(ArchiveLoc, String)> {
        let mut nested = self.nested.clone();
        let last = nested.pop()?;
        Some((ArchiveLoc { file: self.file.clone(), nested, inner: String::new() }, last))
    }

    /// Enters an archive stored at `entry` inside this one.
    pub fn enter_nested(&self, entry: &str) -> Self {
        let mut nested = self.nested.clone();
        nested.push(entry.trim_matches('/').to_string());
        ArchiveLoc { file: self.file.clone(), nested, inner: String::new() }
    }

    /// Identifies the innermost archive regardless of the folder shown in it.
    pub fn archive_key(&self) -> (PathBuf, Vec<String>) {
        (self.file.clone(), self.nested.clone())
    }
}

/// Search results below a folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchLoc {
    pub root: PathBuf,
    pub query: String,
    /// Look inside archives too.
    pub archives: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Dir(PathBuf),
    Archive(ArchiveLoc),
    Drives,
    /// The trash (freedesktop.org spec; shared with other file managers).
    Trash,
    Search(SearchLoc),
}

pub const DRIVES_URI: &str = "drives:///";
pub const TRASH_URI: &str = "trash:///";

impl Location {
    pub fn home() -> Location {
        Location::Dir(gtk::glib::home_dir())
    }

    /// The text shown in the location bar; [`Location::parse`] reads it back.
    pub fn display(&self) -> String {
        match self {
            Location::Dir(p) => p.to_string_lossy().into_owned(),
            Location::Archive(a) => {
                let mut s = a.file.to_string_lossy().into_owned();
                for n in &a.nested {
                    s.push('/');
                    s.push_str(n);
                }
                if !a.inner.is_empty() {
                    s.push('/');
                    s.push_str(&a.inner);
                }
                s
            }
            Location::Drives => DRIVES_URI.into(),
            Location::Trash => TRASH_URI.into(),
            Location::Search(s) => format!("{} — search “{}”", s.root.display(), s.query),
        }
    }

    /// Short name for tab and window titles.
    pub fn title(&self) -> String {
        match self {
            Location::Dir(p) => {
                // A phone's folder is called by the phone's name.
                if let Some((_, name, _)) = crate::devices::named_root(p).filter(|(r, ..)| r == p) {
                    return name;
                }
                match p.file_name() {
                    Some(n) => n.to_string_lossy().into_owned(),
                    None => p.to_string_lossy().into_owned(),
                }
            }
            Location::Archive(a) => {
                if let Some(leaf) = a.inner.rsplit('/').next().filter(|s| !s.is_empty()) {
                    leaf.to_string()
                } else if let Some(n) = a.nested.last() {
                    n.rsplit('/').next().unwrap_or(n).to_string()
                } else {
                    a.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                }
            }
            Location::Drives => "Drives".into(),
            Location::Trash => "Trash".into(),
            Location::Search(s) => format!("Search “{}”", s.query),
        }
    }

    pub fn parent(&self) -> Option<Location> {
        match self {
            Location::Dir(p) => p.parent().map(|p| Location::Dir(p.to_path_buf())),
            Location::Archive(a) => {
                if !a.inner.is_empty() {
                    let up = a.inner.rfind('/').map(|i| &a.inner[..i]).unwrap_or("");
                    return Some(Location::Archive(a.with_inner(up)));
                }
                if let Some((outer, entry)) = a.container() {
                    let up = entry.rfind('/').map(|i| &entry[..i]).unwrap_or("");
                    return Some(Location::Archive(outer.with_inner(up)));
                }
                a.file.parent().map(|p| Location::Dir(p.to_path_buf()))
            }
            Location::Drives | Location::Trash => None,
            Location::Search(s) => Some(Location::Dir(s.root.clone())),
        }
    }

    /// A folder below this one.
    pub fn child(&self, name: &str) -> Option<Location> {
        match self {
            Location::Dir(p) => Some(Location::Dir(p.join(name))),
            Location::Archive(a) => Some(Location::Archive(a.with_inner(&a.entry_path(name)))),
            Location::Drives | Location::Trash | Location::Search(_) => None,
        }
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Location::Dir(p) => Some(p),
            _ => None,
        }
    }

    /// The real directory on disk this location lives in: itself, or the archive's folder.
    pub fn nearest_dir(&self) -> Option<PathBuf> {
        match self {
            Location::Dir(p) => Some(p.clone()),
            Location::Archive(a) => a.file.parent().map(Path::to_path_buf),
            Location::Search(s) => Some(s.root.clone()),
            Location::Drives | Location::Trash => None,
        }
    }

    /// Reads what the user typed: an absolute path, `~/…`, a `file://` URI or `drives:///`.
    /// A path running through an archive file becomes an archive location. `is_archive` decides
    /// whether a path component inside an archive is itself an archive.
    pub fn parse(text: &str, is_archive: impl Fn(&str) -> bool) -> Option<Location> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        if t == "drives:" || t.starts_with("drives:/") {
            return Some(Location::Drives);
        }
        if t == "trash:" || t.starts_with("trash:/") {
            return Some(Location::Trash);
        }
        let path: PathBuf = if let Some(rest) = t.strip_prefix("file://") {
            gtk::glib::filename_from_uri(&format!("file://{rest}")).ok()?.0
        } else if t == "~" {
            gtk::glib::home_dir()
        } else if let Some(rest) = t.strip_prefix("~/") {
            gtk::glib::home_dir().join(rest)
        } else if t.starts_with('/') {
            PathBuf::from(t)
        } else {
            return None;
        };
        let path = normalize(&path);
        // On a share that stopped answering every look at the path hangs: take it as a
        // folder, and the folder view reports that the share does not respond.
        if crate::ops::gvfs::check(&path, std::time::Duration::from_secs(2)).is_err_and(|e| crate::ops::gvfs::not_responding_share(&e).is_some()) {
            return Some(Location::Dir(path));
        }
        if path.is_dir() {
            return Some(Location::Dir(path));
        }
        // Find the archive file the path runs through.
        let mut file = path.clone();
        let mut rest: Vec<String> = Vec::new();
        loop {
            if file.is_file() {
                break;
            }
            let name = file.file_name()?.to_string_lossy().into_owned();
            rest.push(name);
            file = file.parent()?.to_path_buf();
        }
        rest.reverse();
        let mut loc = ArchiveLoc::root(file);
        let mut inner: Vec<String> = Vec::new();
        // A component that looks like an archive is entered as one, the last one too:
        // that is how `display` writes a nested archive's root.
        for part in &rest {
            inner.push(part.clone());
            if is_archive(part) {
                loc = loc.enter_nested(&inner.join("/"));
                inner.clear();
            }
        }
        loc.inner = inner.join("/");
        Some(Location::Archive(loc))
    }
}

/// Resolves `.` and `..` lexically (symlinks stay as typed, like a shell's `cd`).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from("/") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::is_browsable_name;

    #[test]
    fn dir_navigation() {
        let l = Location::Dir("/usr/share".into());
        assert_eq!(l.parent(), Some(Location::Dir("/usr".into())));
        assert_eq!(l.child("doc"), Some(Location::Dir("/usr/share/doc".into())));
        assert_eq!(l.title(), "share");
        assert_eq!(Location::Dir("/".into()).parent(), None);
        assert_eq!(Location::Dir("/".into()).title(), "/");
    }

    #[test]
    fn archive_navigation_climbs_out_of_nested_archives() {
        let root = ArchiveLoc::root("/d/outer.zip".into());
        let nested = root.with_inner("a").enter_nested("a/inner.tar").with_inner("x/y");
        let l = Location::Archive(nested.clone());
        assert_eq!(l.display(), "/d/outer.zip/a/inner.tar/x/y");
        assert_eq!(l.title(), "y");
        let up1 = l.parent().unwrap();
        assert_eq!(up1.display(), "/d/outer.zip/a/inner.tar/x");
        let up2 = up1.parent().unwrap();
        assert_eq!(up2.display(), "/d/outer.zip/a/inner.tar");
        assert_eq!(up2.title(), "inner.tar");
        let up3 = up2.parent().unwrap();
        assert_eq!(up3, Location::Archive(root.with_inner("a")));
        let up4 = up3.parent().unwrap().parent().unwrap();
        assert_eq!(up4, Location::Dir("/d".into()));
    }

    #[test]
    fn parse_paths_and_archives() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/a.zip"), b"PK").unwrap();

        let p = |s: &str| Location::parse(s, is_browsable_name);
        assert_eq!(p(&d.join("sub/../sub").to_string_lossy()), Some(Location::Dir(d.join("sub"))));
        assert_eq!(p("drives:///"), Some(Location::Drives));
        assert_eq!(p("trash:///"), Some(Location::Trash));
        assert_eq!(Location::Trash.display(), "trash:///");
        assert_eq!(Location::Trash.parent(), None);
        assert_eq!(p("relative/path"), None);
        assert_eq!(p("~"), Some(Location::home()));

        let zip = d.join("sub/a.zip");
        assert_eq!(p(&zip.to_string_lossy()), Some(Location::Archive(ArchiveLoc::root(zip.clone()))));
        let inside = format!("{}/docs/b.7z/deep/er", zip.display());
        let expected = ArchiveLoc::root(zip.clone()).enter_nested("docs/b.7z").with_inner("deep/er");
        assert_eq!(p(&inside), Some(Location::Archive(expected)));
        let nested_root = format!("{}/docs/b.7z", zip.display());
        assert_eq!(p(&nested_root), Some(Location::Archive(ArchiveLoc::root(zip.clone()).enter_nested("docs/b.7z"))));
        let uri = gtk::glib::filename_to_uri(d.join("sub"), None).unwrap();
        assert_eq!(p(&uri), Some(Location::Dir(d.join("sub"))));
    }

    #[test]
    fn parse_rejects_missing_paths() {
        assert_eq!(Location::parse("/definitely/not/here", |_| true), None);
    }
}
