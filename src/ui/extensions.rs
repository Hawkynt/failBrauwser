//! Where features plug into windows and panes: the side panel, the drives page, archives
//! and file operations each add their part here.

use super::clipboard::ClipSource;
use super::model::Item;
use super::pane::Pane;
use super::window::Window;
use super::{archive, dnd, fileops, util};
#[cfg(windows)]
use super::remote;
use failbrauwser::location::Location;
use failbrauwser::ops::copy::Mode;
use gtk::gio;
use gtk::prelude::*;
use std::rc::Rc;

pub fn window_created(w: &Rc<Window>) {
    fileops::install_actions(w);
    w.add_action("extract-here", |w| archive::extract_selected(w, None));
    w.add_action("extract-to", archive::extract_to);
    w.add_action("compress", archive::compress);
    w.add_action("open-as-archive", archive::open_as_archive);
    w.add_action("properties", super::properties::show);
    w.add_action("check-updates", super::update::show);
    w.add_action("find", super::searchbar::open);
    super::customactions::install_actions(w);
    super::trash::install_actions(w);
    super::vcs::install_actions(w);
    super::sidebar::attach(w);
}

pub fn ask_archive_password(w: &Window, p: &Rc<Pane>, loc: &Location) {
    archive::ask_password(w, p, loc);
}

/// Places that are not local folders (network shares mounted through GVfs without a path).
pub fn open_uri(w: &Window, uri: &str) {
    if uri.starts_with("trash:") {
        w.navigate(Location::Trash);
        return;
    }
    if util::is_remote_uri(uri) {
        super::shares::open_remote(&w.me(), uri, false);
        return;
    }
    util::show_error(&w.win, "Cannot open location", &format!("“{uri}” is not a local folder."));
}

pub fn pane_created(w: &Window, p: &Rc<Pane>) {
    dnd::setup(w, p);
    super::drives::attach(w, p);
    super::network::attach(w, p);
    super::trash::attach(p);
    super::searchbar::attach(p);
}

pub fn pane_location_changed(w: &Window, p: &Rc<Pane>) {
    super::vcs::overlay::update(w, p);
    super::drives::location_changed(p);
    super::network::location_changed(p);
    super::trash::update_bar(p);
    super::searchbar::location_changed(p);
}

/// Other rows scrolled into view.
pub fn pane_scrolled(w: &Window, p: &Rc<Pane>) {
    super::thumbnails::update(w, p);
}

pub fn pane_contents_changed(w: &Window, p: &Rc<Pane>) {
    super::dirsize::update(w, p);
    super::vcs::overlay::update(w, p);
    if Rc::ptr_eq(p, &w.current_pane()) {
        super::summary::update(w, true);
    }
    // After the rows are laid out, so the visible range is known.
    let (ww, pw) = (Rc::downgrade(&w.me()), Rc::downgrade(p));
    gtk::glib::idle_add_local_once(move || {
        if let (Some(w), Some(p)) = (ww.upgrade(), pw.upgrade()) {
            super::thumbnails::update(&w, &p);
        }
    });
    super::trash::update_bar(p);
    super::archive::maybe_ask_password(w, p);
    // "Empty Trash" and friends depend on what is there now.
    if Rc::ptr_eq(p, &w.current_pane()) {
        fileops::update_sensitivity(w);
    }
    // Folders created or removed here must show up in the tree too.
    if let (Some(sb), Some(dir)) = (super::sidebar::for_window(w), p.location().local_path()) {
        sb.refresh_dir(dir);
    }
}

/// Total sizes for locations that are not plain folders.
pub fn fill_sizes_non_local(p: &Rc<Pane>) {
    archive::fill_sizes(p);
}

pub fn selection_changed(w: &Window) {
    fileops::update_sensitivity(w);
}

pub fn settings_changed(w: &Window) {
    if let Some(sb) = super::sidebar::for_window(w) {
        sb.apply_settings();
    }
    for p in w.panes() {
        super::dirsize::update(w, &p);
        super::vcs::overlay::update(w, &p);
    }
}

/// Can files be created, changed or removed here?
pub fn location_writable(loc: &Location) -> bool {
    match loc {
        // Not asked on a network share (this runs on the UI thread, and a share that stopped
        // answering would hang it); jobs report what the share refuses.
        Location::Dir(p) if failbrauwser::ops::gvfs::is_gvfs_path(p) => true,
        Location::Dir(p) => failbrauwser::osprim::can_write(p),
        // Known once the archive was listed (always the case while it is shown).
        Location::Archive(a) => failbrauwser::archive::vfs::Vfs::global().cached(a).is_some_and(|x| x.writable),
        // A server folder is taken as writable (asking would mean a round trip per draw);
        // the job reports whatever the server refuses.
        Location::Remote(_) => true,
        // Nothing can be put into the trash view; items there are restored or deleted.
        Location::Drives | Location::Trash | Location::Search(_) | Location::Network(_) => false,
    }
}

/// Items that are not plain local files (entries inside archives or on servers).
pub fn open_non_local(w: &Window, item: &Item) {
    match item {
        #[cfg(windows)]
        Item::Remote(_) => remote::view::open_entry(w, item),
        _ => archive::open_entry(w, item),
    }
}

pub fn transfer_non_local(w: &Window, sources: Vec<ClipSource>, dest: Location, mode: Mode, after: Box<dyn FnOnce()>) {
    if matches!(dest, Location::Drives | Location::Network(_)) {
        util::show_error(&w.win, "Cannot paste here", "Open a folder to paste into.");
        return;
    }
    archive::transfer(w, sources, dest, mode, after);
}

pub fn delete_non_local(w: &Window, pane: &Rc<Pane>) {
    match pane.location() {
        #[cfg(windows)]
        Location::Remote(_) => remote::view::delete_selection(w, pane),
        _ => archive::delete_selection(w, pane),
    }
}

pub fn rename_non_local(w: &Window, pane: &Rc<Pane>, item: &Item, new: &str) {
    match pane.location() {
        #[cfg(windows)]
        Location::Remote(_) => remote::view::rename(w, pane, item, new),
        _ => archive::rename(w, pane, item, new),
    }
}

/// Rename onto a name that is there (asked first — see `fileops::rename`).
pub fn rename_replacing_non_local(w: &Window, pane: &Rc<Pane>, item: &Item, new: &str) {
    match pane.location() {
        #[cfg(windows)]
        Location::Remote(_) => remote::view::rename_replacing(w, pane, item, new),
        _ => archive::rename_replacing(w, pane, item, new),
    }
}

pub fn create_non_local(w: &Window, loc: &Location, name: &str, folder: bool) {
    match loc {
        #[cfg(windows)]
        Location::Remote(_) => remote::view::create(w, loc, name, folder),
        _ => archive::create(w, loc, name, folder),
    }
}

fn section(menu: &gio::Menu, items: &[(&str, &str)]) {
    let s = gio::Menu::new();
    for (label, action) in items {
        s.append_item(&super::menuicons::model_item(label, action));
    }
    menu.append_section(None, &s);
}

/// The right-click menu for the current selection (or the folder background).
pub fn context_menu_model(w: &Window, pane: &Rc<Pane>, items: &[Item]) -> gio::Menu {
    let menu = gio::Menu::new();
    if pane.location() == Location::Trash {
        if items.is_empty() {
            section(&menu, &[("_Empty Trash", "win.empty-trash")]);
            section(&menu, &[("Select _All", "win.select-all"), ("_Reload", "win.reload")]);
        } else {
            section(&menu, &[("_Restore", "win.restore")]);
            section(&menu, &[("_Delete Permanently", "win.delete")]);
            section(&menu, &[("_Properties…", "win.properties")]);
        }
        return menu;
    }
    let local = matches!(pane.location(), Location::Dir(_));
    if items.is_empty() {
        section(&menu, &[("Create _Folder…", "win.new-folder"), ("Create _Document…", "win.new-file")]);
        section(&menu, &[("_Paste", "win.paste")]);
        section(&menu, &[("Select _All", "win.select-all"), ("Show _Hidden Files", "win.show-hidden"), ("_Reload", "win.reload")]);
        // The version-control items, where Explorer has TortoiseGit's: after the folder's own.
        if let Some(g) = super::vcs::menu_section(w, pane, items) {
            menu.append_section(None, &g);
        }
        if local {
            section(&menu, &[("Create _Link on Desktop", "win.link-on-desktop")]);
            if let Some(s) = super::customactions::menu_section(w) {
                menu.append_section(None, &s);
            }
        }
        section(&menu, &[("_Properties…", "win.properties")]);
        return menu;
    }
    let open = gio::Menu::new();
    open.append_item(&super::menuicons::model_item("_Open", "win.open"));
    if items.iter().all(Item::is_dir_like) {
        open.append_item(&super::menuicons::model_item("Open in New _Tab", "win.open-in-tab"));
    }
    if let Some(sub) = open_with_menu(items) {
        open.append_item(&super::menuicons::model_submenu("Open _With", &sub));
    }
    menu.append_section(None, &open);
    // The version-control items, as in Explorer: right after opening.
    if local || matches!(pane.location(), Location::Search(_)) {
        if let Some(g) = super::vcs::menu_section(w, pane, items) {
            menu.append_section(None, &g);
        }
    }
    let mut edit = vec![("Cu_t", "win.cut"), ("_Copy", "win.copy")];
    if items.len() == 1 && items[0].is_dir_like() {
        edit.push(("_Paste Into Folder", "win.paste-into"));
    }
    section(&menu, &edit);
    let mut del = Vec::new();
    if local {
        del.push(("Move to T_rash", "win.trash"));
    }
    del.push(("_Delete", "win.delete"));
    del.push(("_Rename…", "win.rename"));
    section(&menu, &del);
    let mut arch = Vec::new();
    if local && !archive::selected_archives(pane).is_empty() {
        arch.push(("E_xtract Here", "win.extract-here"));
        arch.push(("Extract _To…", "win.extract-to"));
    }
    if let [a, b] = items {
        if a.is_dir_like() && b.is_dir_like() && a.path().is_some() && b.path().is_some() {
            arch.push(("Co_mpare", "win.compare-folders"));
        }
    }
    if let [Item::Fs(e) | Item::Found(e, _)] = items {
        if !e.is_dir_like() && failbrauwser::drives::is_mountable_image_name(&items[0].display_name()) {
            arch.push(("_Mount as Drive", "win.mount-image"));
        }
    }
    if local {
        arch.push(("C_ompress…", "win.compress"));
    }
    section(&menu, &arch);
    if let Some(sub) = open_as_menu(items) {
        let s = gio::Menu::new();
        s.append_item(&super::menuicons::model_submenu("Open _as", &sub));
        menu.append_section(None, &s);
    }
    if items.iter().all(|i| i.path().is_some()) {
        section(&menu, &[("Create _Link on Desktop", "win.link-on-desktop")]);
    }
    if local || matches!(pane.location(), Location::Search(_)) {
        if let Some(s) = super::customactions::menu_section(w) {
            menu.append_section(None, &s);
        }
    }
    section(&menu, &[("_Properties…", "win.properties")]);
    menu
}

/// A container kind a file can be opened as whatever its name says (M14 *Open as*): the helper's
/// format id, and the menu label with its mnemonic. The choices are the user's because a VeraCrypt
/// header is random-looking bytes, a dm-crypt volume has no header at all, and a renamed container
/// says nothing about what is inside it.
pub const VOLUME_KINDS: [(&str, &str); 4] = [("VeraCrypt", "_VeraCrypt / TrueCrypt"), ("BitLocker", "_BitLocker"), ("Luks", "_LUKS"), ("DmCryptPlain", "dm-crypt (_plain)")];

/// One *Open as* entry.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenAs {
    /// Enter whatever the library detects, instead of launching the program that owns the suffix.
    Archive,
    /// Run this kind's own reader over the bytes: (format id, menu label).
    Volume(&'static str, &'static str),
}

/// Is this helper format id one of the encrypted container kinds the menu offers? Its ids are the
/// single source, so an entry and the ask that belongs to it cannot drift apart.
pub fn kind_needs_passphrase(id: &str) -> bool {
    VOLUME_KINDS.iter().any(|(kind, _)| *kind == id)
}

impl OpenAs {
    /// Must the passphrase arrive before the file is opened (M14)? Every volume kind the menu
    /// offers is an encrypted one, and its reader is worth running only with the answer in hand:
    /// a search started without it can only answer with the shell of a locked volume. *Archive*
    /// keeps the ask where the library puts it — on the reader that needs one.
    pub fn needs_passphrase(&self) -> bool {
        match self { OpenAs::Volume(id, _) => kind_needs_passphrase(id), OpenAs::Archive => false }
    }
}

/// Which *Open as* entries one selected file gets (M14). Decided apart from the menu so it can be
/// tested without a pane: a folder gets none, and a file the archive viewer already opens by name
/// needs no *Archive* entry, because that entry is for what the name sends somewhere else.
pub fn open_as_choices(display_name: &str, dir_like: bool) -> Vec<OpenAs> {
    if dir_like {
        return Vec::new();
    }
    let mut choices = Vec::new();
    if !failbrauwser::archive::is_browsable_name(display_name) {
        choices.push(OpenAs::Archive);
    }
    choices.extend(VOLUME_KINDS.iter().map(|(id, label)| OpenAs::Volume(id, label)));
    choices
}

fn open_as_menu(items: &[Item]) -> Option<gio::Menu> {
    let [item] = items else { return None };
    let choices = open_as_choices(&item.display_name(), item.is_dir_like());
    if choices.is_empty() {
        return None;
    }
    let sub = gio::Menu::new();
    for choice in choices {
        match choice {
            OpenAs::Archive => sub.append_item(&super::menuicons::model_item("_Archive", "win.open-as-archive")),
            OpenAs::Volume(id, label) => {
                let entry = super::menuicons::model_item(label, "win.open-as-volume");
                entry.set_action_and_target_value(Some("win.open-as-volume"), Some(&id.to_variant()));
                sub.append_item(&entry);
            }
        }
    }
    Some(sub)
}

/// Applications registered for the selection's type, plus "Other Application…".
pub fn open_with_menu(items: &[Item]) -> Option<gio::Menu> {
    let first = items.iter().find(|i| i.path().is_some() && !i.is_dir_like())?;
    let ct = first.content_type();
    let sub = gio::Menu::new();
    let apps = gio::AppInfo::recommended_for_type(&ct);
    for app in apps.iter().take(12) {
        let Some(id) = app.id() else { continue };
        let item = gio::MenuItem::new(Some(&app.display_name()), None);
        item.set_action_and_target_value(Some("win.open-with"), Some(&id.to_variant()));
        if let Some(icon) = app.icon().or_else(|| super::menuicons::themed(&["application-x-executable"]).map(|n| gio::ThemedIcon::new(&n).upcast())) {
            item.set_icon(&icon);
        }
        sub.append_item(&item);
    }
    let other = gio::Menu::new();
    other.append_item(&super::menuicons::model_item("Other _Application…", "win.open-with-other"));
    sub.append_section(None, &other);
    Some(sub)
}

#[cfg(test)]
mod tests {
    use super::{OpenAs, VOLUME_KINDS, kind_needs_passphrase, open_as_choices};

    #[test]
    fn a_folder_gets_no_open_as_choices() {
        assert!(open_as_choices("photos", true).is_empty());
    }

    /// The kinds that a name cannot tell apart stay offered for every single file (M14).
    #[test]
    fn one_file_of_any_name_gets_the_four_volume_kinds() {
        for name in ["disk.bin", "notes.txt", "movie.mkv", "setup.exe", "photo.iso", "photos.zip"] {
            let kinds = open_as_choices(name, false).into_iter().filter(|c| matches!(c, OpenAs::Volume(_, _))).count();
            assert_eq!(kinds, 4, "{name}");
        }
    }

    /// A chosen encrypted kind reads its volume with the passphrase, so the ask comes first:
    /// a search that runs before the answer can only end in the shell of a locked volume (M14).
    #[test]
    fn a_chosen_volume_asks_its_passphrase_before_opening() {
        for (id, label) in VOLUME_KINDS {
            assert!(OpenAs::Volume(id, label).needs_passphrase(), "{id} is encrypted: it asks before anything is read");
        }
        assert!(!OpenAs::Archive.needs_passphrase(), "Archive asks only when a reader of its own asks");
        assert!(!kind_needs_passphrase("Zip"), "a forced ordinary archive keeps the reader's own ask");
    }

    #[test]
    fn what_the_archive_viewer_already_reaches_by_name_gets_no_archive_entry() {
        assert!(!open_as_choices("photos.zip", false).contains(&OpenAs::Archive), "the entry would repeat what the name does already");
        for name in ["movie.mkv", "setup.exe", "photo.jpg", "disk.bin"] {
            assert!(open_as_choices(name, false).contains(&OpenAs::Archive), "{name} should answer with its contents instead of its program");
        }
    }
}
