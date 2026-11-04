//! Archives as folders: listings (cached), nested archives, and edits that are written back
//! through every level of nesting.
//!
//! An archive inside an archive is extracted once into a cache folder and worked on there;
//! after a change it is put back into its container, and that container into its own, up
//! to the file on disk. All methods block; call them from worker threads.

use super::{ArchiveError, ArchiveTree, Helper, Result};
use crate::location::ArchiveLoc;
use crate::osprim;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Identifies a version of a file: when any of these change, cached data is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime_ns: i128,
    size: u64,
    ino: u128,
}

fn stamp(p: &Path) -> Result<Stamp> {
    let m = std::fs::metadata(p).map_err(|e| ArchiveError::new("notfound", format!("{}: {e}", p.display())))?;
    let mt = osprim::times(&m).1;
    Ok(Stamp { mtime_ns: mt.0 as i128 * 1_000_000_000 + mt.1 as i128, size: m.len(), ino: osprim::identity(p, &m).id })
}

#[derive(Debug)]
pub struct Archive {
    pub format: String,
    pub writable: bool,
    pub readonly_reason: Option<String>,
    pub tree: ArchiveTree,
}

type NestedKey = (PathBuf, Vec<String>);

#[derive(Default)]
struct State {
    listings: HashMap<PathBuf, (Stamp, Arc<Archive>)>,
    /// Nested archive → (its extracted copy, the container's stamp when extracted).
    nested: HashMap<NestedKey, (PathBuf, Stamp)>,
    /// Per archive (nested ones have their own), for this session only.
    passwords: HashMap<NestedKey, String>,
}

pub struct Vfs {
    helper: Helper,
    cache: PathBuf,
    state: Mutex<State>,
}

fn hash_key(k: &impl Hash) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    k.hash(&mut h);
    format!("{:016x}", h.finish())
}

impl Vfs {
    pub fn new(helper: Helper, cache: PathBuf) -> Self {
        Vfs { helper, cache, state: Mutex::new(State::default()) }
    }

    /// The process-wide instance; its cache lives in `$XDG_RUNTIME_DIR/failbrauwser`.
    pub fn global() -> &'static Vfs {
        static V: OnceLock<Vfs> = OnceLock::new();
        V.get_or_init(|| Vfs::new(Helper::global().clone(), runtime_dir()))
    }

    pub fn helper(&self) -> &Helper {
        &self.helper
    }

    /// A private scratch folder below the cache (removed by the caller).
    pub fn scratch_dir(&self, what: &str) -> Result<PathBuf> {
        let base = self.cache.join(what);
        std::fs::create_dir_all(&base).map_err(io_err)?;
        let dir = tempfile::Builder::new().prefix("fb-").tempdir_in(&base).map_err(io_err)?;
        Ok(dir.keep())
    }

    /// Remembers the password of the innermost archive of `loc` (until the program ends).
    pub fn set_password(&self, loc: &ArchiveLoc, password: &str) {
        self.state.lock().unwrap().passwords.insert(loc.archive_key(), password.to_string());
    }

    pub fn has_password(&self, loc: &ArchiveLoc) -> bool {
        self.state.lock().unwrap().passwords.contains_key(&loc.archive_key())
    }

    /// The password of the innermost archive of `loc`: its own, else that of the file on disk.
    fn password_for(&self, loc: &ArchiveLoc) -> Option<String> {
        let st = self.state.lock().unwrap();
        st.passwords.get(&loc.archive_key()).or_else(|| st.passwords.get(&(loc.file.clone(), Vec::new()))).cloned()
    }

    /// The file on disk holding the innermost archive of `loc` (extracting nested ones).
    pub fn backing(&self, loc: &ArchiveLoc) -> Result<PathBuf> {
        let mut path = loc.file.clone();
        for depth in 1..=loc.nested.len() {
            let container = ArchiveLoc { file: loc.file.clone(), nested: loc.nested[..depth - 1].to_vec(), inner: String::new() };
            let pw = self.password_for(&container);
            let key: NestedKey = (loc.file.clone(), loc.nested[..depth].to_vec());
            let parent_stamp = stamp(&path)?;
            let known = self.state.lock().unwrap().nested.get(&key).cloned();
            if let Some((copy, st)) = known {
                if st == parent_stamp && copy.exists() {
                    path = copy;
                    continue;
                }
            }
            let entry = &loc.nested[depth - 1];
            let dir = self.cache.join("nested").join(hash_key(&key));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(io_err)?;
            self.helper.extract(&path, Some(std::slice::from_ref(entry)), &dir, pw.as_deref())?;
            let copy = dir.join(entry);
            if !copy.is_file() {
                return Err(ArchiveError::new("notfound", format!("“{entry}” could not be extracted")));
            }
            self.state.lock().unwrap().nested.insert(key, (copy.clone(), parent_stamp));
            path = copy;
        }
        Ok(path)
    }

    /// The listing of the innermost archive of `loc`.
    pub fn archive(&self, loc: &ArchiveLoc) -> Result<Arc<Archive>> {
        let backing = self.backing(loc)?;
        let st = stamp(&backing)?;
        if let Some((s, a)) = self.state.lock().unwrap().listings.get(&backing) {
            if *s == st {
                return Ok(a.clone());
            }
        }
        let listing = self.helper.list(&backing, self.password_for(loc).as_deref())?;
        let archive = Arc::new(Archive { tree: ArchiveTree::build(&listing.entries), format: listing.format, writable: listing.writable, readonly_reason: listing.readonly_reason });
        self.state.lock().unwrap().listings.insert(backing, (st, archive.clone()));
        Ok(archive)
    }

    /// The cached listing, without any I/O beyond a stat (for the UI thread).
    pub fn cached(&self, loc: &ArchiveLoc) -> Option<Arc<Archive>> {
        let st = self.state.lock().unwrap();
        let backing = if loc.nested.is_empty() {
            loc.file.clone()
        } else {
            st.nested.get(&(loc.file.clone(), loc.nested.clone()))?.0.clone()
        };
        let (_, a) = st.listings.get(&backing)?;
        Some(a.clone())
    }

    /// After the innermost archive changed, puts it back into its containers, innermost first.
    fn write_back(&self, loc: &ArchiveLoc) -> Result<()> {
        for depth in (1..=loc.nested.len()).rev() {
            let child_key: NestedKey = (loc.file.clone(), loc.nested[..depth].to_vec());
            let child = self.state.lock().unwrap().nested.get(&child_key).map(|x| x.0.clone()).ok_or_else(|| ArchiveError::new("error", "nested archive vanished"))?;
            let parent_loc = ArchiveLoc { file: loc.file.clone(), nested: loc.nested[..depth - 1].to_vec(), inner: String::new() };
            let parent = self.backing(&parent_loc)?;
            let pw = self.password_for(&parent_loc);
            self.helper.add(&parent, &[(child, loc.nested[depth - 1].clone())], pw.as_deref())?;
            // Our copy is what the container now holds: no need to extract it again.
            let new_stamp = stamp(&parent)?;
            if let Some(e) = self.state.lock().unwrap().nested.get_mut(&child_key) {
                e.1 = new_stamp;
            }
        }
        Ok(())
    }

    fn edit(&self, loc: &ArchiveLoc, f: impl FnOnce(&Helper, &Path, Option<&str>) -> Result<()>) -> Result<()> {
        let backing = self.backing(loc)?;
        let a = self.archive(loc)?;
        if !a.writable {
            return Err(ArchiveError::new("unsupported", format!("{} archives cannot be changed.", a.format)));
        }
        f(&self.helper, &backing, self.password_for(loc).as_deref())?;
        self.write_back(loc)
    }

    /// Adds files or folders from disk; names are full paths inside the archive.
    pub fn add(&self, loc: &ArchiveLoc, items: &[(PathBuf, String)]) -> Result<()> {
        self.edit(loc, |h, b, pw| h.add(b, items, pw))
    }

    /// Removes entries (full paths inside the archive; folders with their contents).
    pub fn remove(&self, loc: &ArchiveLoc, names: &[String]) -> Result<()> {
        self.edit(loc, |h, b, pw| h.remove(b, names, pw))
    }

    pub fn rename(&self, loc: &ArchiveLoc, from: &str, to: &str) -> Result<()> {
        self.edit(loc, |h, b, pw| h.rename(b, from, to, pw))
    }

    pub fn mkdir(&self, loc: &ArchiveLoc, path: &str) -> Result<()> {
        self.edit(loc, |h, b, pw| h.mkdir(b, path, pw))
    }

    /// Extracts entries (full paths) below `dest`, keeping their paths inside the archive.
    /// Returns where each requested entry ended up.
    pub fn extract(&self, loc: &ArchiveLoc, entries: &[String], dest: &Path) -> Result<Vec<PathBuf>> {
        let backing = self.backing(loc)?;
        self.helper.extract(&backing, Some(entries), dest, self.password_for(loc).as_deref())?;
        Ok(entries.iter().map(|e| dest.join(e)).collect())
    }

    /// Extracts everything into `dest_dir`: directly when the archive holds a single top
    /// folder or file (a compressed file becomes that file), else into a new folder named
    /// after the archive. Returns what was created.
    pub fn extract_all(&self, loc: &ArchiveLoc, dest_dir: &Path) -> Result<PathBuf> {
        let a = self.archive(loc)?;
        let tops = a.tree.children("").unwrap_or_default();
        let name = loc.nested.last().map(|n| n.rsplit('/').next().unwrap_or(n).to_string()).unwrap_or_else(|| loc.file.file_name().unwrap_or_default().to_string_lossy().into_owned());
        let stem = strip_archive_extension(&name);
        let backing = self.backing(loc)?;
        let pw = self.password_for(loc);
        // Everything goes into a folder that did not exist before; a failed extraction (a
        // wrong password, say) removes it again, so a retry does not land in "name (2)".
        let (target, extract_into) = if tops.len() == 1 && std::fs::symlink_metadata(dest_dir.join(&tops[0].name)).is_err() {
            (dest_dir.join(&tops[0].name), dest_dir.to_path_buf())
        } else {
            let folder = if dest_dir.join(&stem).exists() {
                dest_dir.join(crate::ops::names::free_name(dest_dir, stem.as_ref(), None))
            } else {
                dest_dir.join(&stem)
            };
            std::fs::create_dir_all(&folder).map_err(io_err)?;
            (folder.clone(), folder)
        };
        if let Err(e) = self.helper.extract(&backing, None, &extract_into, pw.as_deref()) {
            if target.is_dir() {
                let _ = std::fs::remove_dir_all(&target);
            } else {
                let _ = std::fs::remove_file(&target);
            }
            return Err(e);
        }
        Ok(target)
    }

    /// Forgets cached listings (e.g. after an external change).
    pub fn invalidate(&self, file: &Path) {
        let mut st = self.state.lock().unwrap();
        st.listings.retain(|k, _| k != file);
    }
}

fn io_err(e: std::io::Error) -> ArchiveError {
    ArchiveError::new("error", e.to_string())
}

/// "photos.tar.gz" → "photos".
pub fn strip_archive_extension(name: &str) -> String {
    let (stem, ext) = crate::ops::names::split_extension(name.as_bytes());
    if ext.is_empty() || stem.is_empty() { name.to_string() } else { String::from_utf8_lossy(stem).into_owned() }
}

/// `$XDG_RUNTIME_DIR/failbrauwser` (private to the user, cleared at logout).
pub fn runtime_dir() -> PathBuf {
    gtk::glib::user_runtime_dir().join("failbrauwser")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_extensions_are_stripped() {
        assert_eq!(strip_archive_extension("a.tar.gz"), "a");
        assert_eq!(strip_archive_extension("b.zip"), "b");
        assert_eq!(strip_archive_extension("noext"), "noext");
    }
}
