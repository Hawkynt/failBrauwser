//! The list store shared by the detailed list and the icon view.

use super::util;
use failbrauwser::archive::ArchiveNode;
use failbrauwser::fs::format::{human_size, human_time, natural_cmp, permissions};
use failbrauwser::fs::{FileEntry, Kind};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cmp::Ordering;
use std::ffi::OsString;
use std::path::PathBuf;

pub const COL_NAME: u32 = 0;
pub const COL_ICON: u32 = 1;
pub const COL_SIZE: u32 = 2;
pub const COL_SIZE_TEXT: u32 = 3;
pub const COL_MTIME: u32 = 4;
pub const COL_MTIME_TEXT: u32 = 5;
pub const COL_TYPE: u32 = 6;
pub const COL_IS_DIR: u32 = 7;
pub const COL_KEY: u32 = 8;
pub const COL_PERMS: u32 = 9;
pub const COL_OWNER: u32 = 10;
pub const COL_GROUP: u32 = 11;
pub const COL_RANK: u32 = 12;
/// Recursive size in bytes; -1 while unknown.
pub const COL_DIRSIZE: u32 = 13;
pub const COL_DIRSIZE_TEXT: u32 = 14;
/// False renders the row dimmed (hidden files, items cut to the clipboard).
pub const COL_SENSITIVE: u32 = 15;
/// Thumbnail image, if one was found (drawn instead of the icon).
pub const COL_THUMB: u32 = 16;
/// For search results: the folder a result is in.
pub const COL_FOLDER: u32 = 17;

pub fn new_store() -> gtk::ListStore {
    gtk::ListStore::new(&[
        glib::Type::STRING,
        gio::Icon::static_type(),
        glib::Type::U64,
        glib::Type::STRING,
        glib::Type::I64,
        glib::Type::STRING,
        glib::Type::STRING,
        glib::Type::BOOL,
        glib::Type::U64,
        glib::Type::STRING,
        glib::Type::STRING,
        glib::Type::STRING,
        glib::Type::U32,
        glib::Type::I64,
        glib::Type::STRING,
        glib::Type::BOOL,
        gtk::gdk_pixbuf::Pixbuf::static_type(),
        glib::Type::STRING,
    ])
}

/// What a row shows: a file on disk or an entry inside an archive.
#[derive(Debug, Clone)]
pub enum Item {
    Fs(FileEntry),
    Archive(ArchiveNode),
    /// A trashed file: where it came from, and the file as it sits in the trash.
    Trash(failbrauwser::trash::TrashItem, FileEntry),
    /// A search result on disk, with the folder it is in (relative to the search root).
    Found(FileEntry, String),
    /// A search result inside an archive: the archive folder holding it, the entry, and
    /// where that is (for the Folder column).
    FoundInArchive(failbrauwser::location::ArchiveLoc, ArchiveNode, String),
}

impl Item {
    pub fn display_name(&self) -> String {
        match self {
            Item::Fs(e) => e.display_name(),
            Item::Archive(n) => n.name.clone(),
            Item::Trash(t, _) => t.original_name(),
            Item::Found(e, _) => e.display_name(),
            Item::FoundInArchive(_, n, _) => n.name.clone(),
        }
    }
    pub fn os_name(&self) -> OsString {
        match self {
            Item::Fs(e) => e.name.clone(),
            Item::Archive(n) => OsString::from(&n.name),
            // Unique within the trash, unlike original names.
            Item::Trash(t, _) => OsString::from(&t.name),
            // Names repeat across folders: results are keyed by their full path.
            Item::Found(e, _) => e.path.clone().into_os_string(),
            Item::FoundInArchive(a, n, _) => OsString::from(format!("{}\0{}", failbrauwser::location::Location::Archive(a.clone()).display(), n.path)),
        }
    }
    pub fn is_dir_like(&self) -> bool {
        match self {
            Item::Fs(e) => e.is_dir_like(),
            Item::Archive(n) => n.is_dir,
            Item::Trash(_, e) | Item::Found(e, _) => e.is_dir_like(),
            Item::FoundInArchive(_, n, _) => n.is_dir,
        }
    }
    pub fn is_hidden(&self) -> bool {
        match self {
            Item::Fs(e) => e.is_hidden(),
            Item::Archive(n) => n.name.starts_with('.'),
            // Everything in the trash and every search result is shown.
            Item::Trash(..) | Item::Found(..) | Item::FoundInArchive(..) => false,
        }
    }
    /// The file on disk (for trashed items: the file inside the trash).
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            Item::Fs(e) | Item::Trash(_, e) | Item::Found(e, _) => Some(&e.path),
            Item::Archive(_) | Item::FoundInArchive(..) => None,
        }
    }
    pub fn size(&self) -> u64 {
        match self {
            Item::Fs(e) | Item::Trash(_, e) | Item::Found(e, _) => e.size,
            Item::Archive(n) | Item::FoundInArchive(_, n, _) => n.size,
        }
    }
    pub fn content_type(&self) -> String {
        match self {
            Item::Fs(e) | Item::Trash(_, e) | Item::Found(e, _) => e.content_type.clone(),
            Item::Archive(n) | Item::FoundInArchive(_, n, _) => {
                if n.is_dir {
                    "inode/directory".into()
                } else {
                    gio::content_type_guess(Some(std::path::Path::new(&n.name)), None).0.to_string()
                }
            }
        }
    }
    /// Identity for change detection: two readings with equal keys show the same row.
    pub fn same_as(&self, other: &Item) -> bool {
        match (self, other) {
            (Item::Fs(a), Item::Fs(b)) => a == b,
            (Item::Archive(a), Item::Archive(b)) => a == b,
            (Item::Trash(a, x), Item::Trash(b, y)) => a == b && x == y,
            (Item::Found(a, _), Item::Found(b, _)) => a == b,
            (Item::FoundInArchive(_, a, _), Item::FoundInArchive(_, b, _)) => a == b,
            _ => false,
        }
    }
}

/// Writes every column of a row.
pub fn fill_row(store: &gtk::ListStore, iter: &gtk::TreeIter, key: u64, item: &Item, rank: u32, now: i64) {
    let name = item.display_name();
    let dir = item.is_dir_like();
    let (icon, size, mtime, ct, perms, owner, group) = match item {
        // The date shown for trashed items is when they were deleted.
        Item::Trash(t, e) => (
            util::icon_for_entry(e),
            e.size,
            t.deleted_unix(),
            if e.broken { "inode/symlink".to_string() } else { e.content_type.clone() },
            permissions(e.mode),
            util::user_name(e.uid),
            util::group_name(e.gid),
        ),
        Item::Fs(e) | Item::Found(e, _) => (
            util::icon_for_entry(e),
            e.size,
            e.mtime,
            if e.broken { "inode/symlink".to_string() } else { e.content_type.clone() },
            permissions(e.mode),
            util::user_name(e.uid),
            util::group_name(e.gid),
        ),
        Item::Archive(n) | Item::FoundInArchive(_, n, _) => {
            let ct = item.content_type();
            (util::icon_for_type(&ct), n.size, n.mtime.unwrap_or(0), ct, String::new(), String::new(), String::new())
        }
    };
    let size_text = if dir { String::new() } else { human_size(size) };
    let mtime_text = if mtime == 0 { String::new() } else { human_time(mtime, now) };
    let is_special = matches!(item, Item::Fs(e) if e.kind == Kind::Special);
    let (dirsize, dirsize_text) = if dir { (-1i64, String::new()) } else if is_special { (0, String::new()) } else { (size as i64, human_size(size)) };
    store.set(
        iter,
        &[
            (COL_NAME, &name),
            (COL_ICON, &icon),
            (COL_SIZE, &(if dir { 0 } else { size })),
            (COL_SIZE_TEXT, &size_text),
            (COL_MTIME, &mtime),
            (COL_MTIME_TEXT, &mtime_text),
            (COL_TYPE, &util::type_description(&ct)),
            (COL_IS_DIR, &dir),
            (COL_KEY, &key),
            (COL_PERMS, &perms),
            (COL_OWNER, &owner),
            (COL_GROUP, &group),
            (COL_RANK, &rank),
            (COL_DIRSIZE, &dirsize),
            (COL_DIRSIZE_TEXT, &dirsize_text),
            (COL_SENSITIVE, &!item.is_hidden()),
            // A changed file needs a new thumbnail.
            (COL_THUMB, &None::<gtk::gdk_pixbuf::Pixbuf>),
            (COL_FOLDER, &match item {
                Item::Found(_, f) | Item::FoundInArchive(_, _, f) => f.clone(),
                _ => String::new(),
            }),
        ],
    );
}

/// Natural-order rank of each name, so sorting in the view compares integers.
pub fn ranks<'a>(names: impl Iterator<Item = &'a str>) -> Vec<u32> {
    let names: Vec<&str> = names.collect();
    let mut idx: Vec<usize> = (0..names.len()).collect();
    idx.sort_by(|&a, &b| natural_cmp(names[a], names[b]));
    let mut rank = vec![0u32; names.len()];
    for (r, i) in idx.into_iter().enumerate() {
        rank[i] = r as u32;
    }
    rank
}

/// Sort ids: the index of the column id in [`failbrauwser::config::COLUMN_IDS`].
pub fn sort_id_for(column: &str) -> u32 {
    failbrauwser::config::COLUMN_IDS.iter().position(|c| *c == column).unwrap_or(0) as u32
}

/// Installs sort functions for every column: folders first (in both directions, when
/// enabled), then the column's value, then the natural name order.
pub fn install_sorting(store: &gtk::ListStore, folders_first: std::rc::Rc<std::cell::Cell<bool>>) {
    for (i, id) in failbrauwser::config::COLUMN_IDS.iter().enumerate() {
        let id = *id;
        let ff = folders_first.clone();
        let sortable = store.clone();
        store.set_sort_func(gtk::SortColumn::Index(i as u32), move |m, a, b| {
            let descending = matches!(sortable.sort_column_id(), Some((_, gtk::SortType::Descending)));
            if ff.get() {
                let da: bool = m.value(a, COL_IS_DIR as i32).get().unwrap_or_default();
                let db: bool = m.value(b, COL_IS_DIR as i32).get().unwrap_or_default();
                if da != db {
                    // The view reverses the result for descending order; undo that here.
                    let ord = if da { Ordering::Less } else { Ordering::Greater };
                    return if descending { ord.reverse() } else { ord };
                }
            }
            let by_rank = || {
                let ra: u32 = m.value(a, COL_RANK as i32).get().unwrap_or_default();
                let rb: u32 = m.value(b, COL_RANK as i32).get().unwrap_or_default();
                ra.cmp(&rb)
            };
            let by_str = |col: u32| {
                let sa: String = m.value(a, col as i32).get().unwrap_or_default();
                let sb: String = m.value(b, col as i32).get().unwrap_or_default();
                natural_cmp(&sa, &sb)
            };
            let ord = match id {
                // Files by size, folders by their total (not known yet: -1, first).
                "size" => {
                    let x: i64 = m.value(a, COL_DIRSIZE as i32).get().unwrap_or_default();
                    let y: i64 = m.value(b, COL_DIRSIZE as i32).get().unwrap_or_default();
                    x.cmp(&y)
                }
                "modified" => {
                    let x: i64 = m.value(a, COL_MTIME as i32).get().unwrap_or_default();
                    let y: i64 = m.value(b, COL_MTIME as i32).get().unwrap_or_default();
                    x.cmp(&y)
                }
                "type" => by_str(COL_TYPE),
                "permissions" => by_str(COL_PERMS),
                "owner" => by_str(COL_OWNER),
                "group" => by_str(COL_GROUP),
                _ => Ordering::Equal,
            };
            ord.then_with(by_rank).into()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_follow_natural_order() {
        let names = ["b10", "b2", "A"];
        assert_eq!(ranks(names.iter().copied()), vec![2, 1, 0]);
    }
}
