//! Reading locations. Runs on worker threads; returns plain data.

use super::model::Item;
use failbrauwser::archive::is_browsable_name;
use failbrauwser::archive::vfs::Vfs;
use failbrauwser::fs::list_dir;
use failbrauwser::location::Location;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct LoadError {
    pub message: String,
    /// The archive is encrypted: ask for its password and read again.
    pub needs_password: bool,
}

impl From<String> for LoadError {
    fn from(message: String) -> Self {
        LoadError { message, needs_password: false }
    }
}

/// Reads everything at a location (hidden entries included).
pub fn read_location(loc: &Location) -> Result<Vec<Item>, LoadError> {
    match loc {
        Location::Dir(p) => list_dir(p)
            .map(|v| v.into_iter().map(Item::Fs).collect())
            .map_err(|e| format!("Cannot open “{}”: {}", p.display(), e).into()),
        Location::Archive(a) => {
            let archive = Vfs::global().archive(a).map_err(|e| LoadError {
                needs_password: e.needs_password(),
                message: format!("Cannot open “{}”: {}", Location::Archive(a.clone()).title(), e),
            })?;
            match archive.tree.children(&a.inner) {
                Some(children) => Ok(children.into_iter().cloned().map(Item::Archive).collect()),
                None => Err(format!("“{}” does not exist in this archive.", a.inner).into()),
            }
        }
        // SFTP and FTP folders are read by the app's own stack (the pane's worker thread
        // blocks on the pooled connection; the UI never does).
        #[cfg(windows)]
        Location::Remote(r) => failbrauwser::remote_fs::list(&r.target, &r.inner)
            .map(|v| v.into_iter().map(Item::Remote).collect())
            .map_err(|e| format!("Cannot open “{}”: {e}", r.target.key()).into()),
        #[cfg(not(windows))]
        // Linux hands those addresses to GVfs; a Remote location is never produced here.
        Location::Remote(_) => Ok(Vec::new()),
        // Search results stream in while searching (see the pane).
        Location::Drives | Location::Search(_) | Location::Network(_) => Ok(Vec::new()),
        Location::Trash => Ok(failbrauwser::trash::list(&failbrauwser::trash::trash_dirs())
            .into_iter()
            .filter_map(|t| failbrauwser::fs::FileEntry::stat(&t.path).ok().map(|e| Item::Trash(t, e)))
            .collect()),
    }
}

/// The file or directory whose changes should trigger a re-read of the location.
pub fn watch_path(loc: &Location) -> Option<PathBuf> {
    match loc {
        Location::Dir(p) => Some(p.clone()),
        // Any change to the archive file (ours or another program's) re-reads it.
        Location::Archive(a) => Some(a.file.clone()),
        // New arrivals and removals show up in the home trash's files folder.
        Location::Trash => {
            let files = gtk::glib::user_data_dir().join("Trash/files");
            let _ = std::fs::create_dir_all(&files);
            Some(files)
        }
        // A remote folder has nothing local to watch; reloads are manual or navigational.
        Location::Drives | Location::Search(_) | Location::Network(_) | Location::Remote(_) => None,
    }
}

/// If activating this file should open it as a folder (an archive), the location to go to.
pub fn enter_as_folder(current: &Location, item: &Item) -> Option<Location> {
    if item.is_dir_like() || !is_browsable_name(&item.display_name()) {
        return None;
    }
    match (current, item) {
        (_, Item::Fs(e)) => Some(Location::Archive(failbrauwser::location::ArchiveLoc::root(e.path.clone()))),
        (Location::Archive(a), Item::Archive(n)) => Some(Location::Archive(a.enter_nested(&n.path))),
        _ => None,
    }
}
