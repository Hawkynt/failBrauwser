//! A working copy as the windows read it, whatever tool owns it. Each tool answers the same
//! questions — the state of a folder's entries, the changes to commit, the history, blame, a file's
//! text at a revision — through its own client, in the words of `state`. What a tool cannot answer
//! at all is a capability it lacks (`caps`), so the windows never ask.

use super::state::{BlameLine, Change, Commit, Refs, Status};
use super::{Tool, WorkingCopy};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait Backend: Send + Sync + std::fmt::Debug {
    fn tool(&self) -> Tool;

    /// The top folder of the working copy.
    fn root(&self) -> &Path;

    /// `path` relative to the top of the working copy, with '/' ("" for the top itself).
    fn relative(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(self.root()).ok()?;
        Some(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
    }

    /// Where the working copy is, in the tool's own words: the "branch" of *Commit -> "branch"…*.
    fn branch_label(&self) -> String;

    /// The changes under `paths` (all of it when empty), with line counts where the tool gives
    /// them; unversioned and ignored files when asked.
    fn changes(&self, paths: &[String], unversioned: bool, ignored: bool) -> Result<Vec<Change>, String>;

    /// The overlay state of each entry of `dir`: files by their own state, folders by the most
    /// important state of what they hold.
    fn folder_status(&self, dir: &Path) -> Result<HashMap<OsString, Status>, String>;

    /// Whether the working copy as a whole is clean, changed or in conflict (unversioned files do
    /// not count): what a folder that is a working copy of its own shows from outside.
    fn summary(&self) -> Result<Status, String>;

    /// The history, newest first; only revisions that touched `paths` when any are given.
    fn log(&self, paths: &[String], max: usize) -> Result<Vec<Commit>, String>;

    /// The kinds of change of each revision ('A', 'M', 'D', 'R'), for the log's Actions column.
    fn log_actions(&self, _paths: &[String], _max: usize) -> HashMap<String, Vec<char>> {
        HashMap::new()
    }

    /// What a revision changed, against the revision before it.
    fn revision_changes(&self, rev: &str) -> Result<Vec<Change>, String>;

    /// The revision before `rev` (None for the first one).
    fn parent(&self, rev: &str) -> Option<String>;

    /// Who last changed each line of `path`.
    fn blame(&self, path: &str) -> Result<Vec<BlameLine>, String>;

    /// The text the working file `path` is compared with by *Diff* ("" for a new file).
    fn base_text(&self, path: &str) -> Vec<u8>;

    /// The text of `path` at `rev` ("" where it did not exist).
    fn text_at(&self, rev: &str, path: &str) -> Vec<u8>;

    /// The two revisions that last changed `path`: (the latest, the one before it, "" for none).
    fn last_two_versions(&self, path: &str) -> Option<(String, String)>;

    /// Whether conflicts wait to be resolved.
    fn has_conflicts(&self) -> bool;

    /// The branches and tags, and the labels next to the revisions they name.
    fn refs(&self) -> Refs {
        Refs::default()
    }

    /// The folder the tool keeps its own data in, watched so the overlays follow commits and
    /// checkouts made elsewhere.
    fn admin_dir(&self) -> Option<PathBuf>;

    /// The Git repository behind this working copy, for the items only Git has (Reflog, the
    /// Pull and Push options).
    fn as_git(&self) -> Option<&super::git::Repo> {
        None
    }
}

/// The backend of a working copy (None where a Git marker names no repository).
pub fn open(wc: &WorkingCopy) -> Option<Arc<dyn Backend>> {
    match wc.tool {
        Tool::Git => super::git::repo_at(&wc.root).map(|r| Arc::new(r) as Arc<dyn Backend>),
        Tool::Mercurial => Some(Arc::new(super::hg::Hg { root: wc.root.clone() })),
        Tool::Subversion => Some(Arc::new(super::svn::Svn { root: wc.root.clone() })),
        Tool::Cvs => Some(Arc::new(super::cvs::Cvs { root: wc.root.clone() })),
        Tool::VisualSourceSafe => Some(Arc::new(super::vss::Vss { root: wc.root.clone() })),
    }
}

/// The backend of the working copy holding `path` (a file or folder). What lies inside a tool's
/// own data folder belongs to no working copy.
pub fn discover(path: &Path) -> Option<Arc<dyn Backend>> {
    if path.components().any(|c| [".git", ".hg", ".svn", "CVS"].iter().any(|a| c.as_os_str() == *a)) {
        return None;
    }
    let dir = if path.is_dir() { path } else { path.parent()? };
    open(&super::find(dir)?)
}
