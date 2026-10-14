//! Where features plug into windows and panes: the side panel, the drives page, archives
//! and file operations each add their part here.

use super::clipboard::ClipSource;
use super::model::Item;
use super::pane::Pane;
use super::window::Window;
use super::{archive, dnd, fileops, util};
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
    w.add_action("find", super::searchbar::open);
    super::customactions::install_actions(w);
    super::trash::install_actions(w);
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
    super::trash::attach(p);
    super::searchbar::attach(p);
}

pub fn pane_location_changed(_w: &Window, p: &Rc<Pane>) {
    super::drives::location_changed(p);
    super::trash::update_bar(p);
    super::searchbar::location_changed(p);
}

/// Other rows scrolled into view.
pub fn pane_scrolled(w: &Window, p: &Rc<Pane>) {
    super::thumbnails::update(w, p);
}

pub fn pane_contents_changed(w: &Window, p: &Rc<Pane>) {
    super::dirsize::update(w, p);
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
    }
}

/// Can files be created, changed or removed here?
pub fn location_writable(loc: &Location) -> bool {
    match loc {
        // Not asked on a network share (this runs on the UI thread, and a share that stopped
        // answering would hang it); jobs report what the share refuses.
        Location::Dir(p) if failbrauwser::ops::gvfs::is_gvfs_path(p) => true,
        Location::Dir(p) => {
            use std::os::unix::ffi::OsStrExt;
            let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return false };
            unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
        }
        // Known once the archive was listed (always the case while it is shown).
        Location::Archive(a) => failbrauwser::archive::vfs::Vfs::global().cached(a).is_some_and(|x| x.writable),
        // Nothing can be put into the trash view; items there are restored or deleted.
        Location::Drives | Location::Trash | Location::Search(_) => false,
    }
}

/// Items that are not plain local files (entries inside archives).
pub fn open_non_local(w: &Window, item: &Item) {
    archive::open_entry(w, item);
}

pub fn transfer_non_local(w: &Window, sources: Vec<ClipSource>, dest: Location, mode: Mode, after: Box<dyn FnOnce()>) {
    if matches!(dest, Location::Drives) {
        util::show_error(&w.win, "Cannot paste here", "Open a folder to paste into.");
        return;
    }
    archive::transfer(w, sources, dest, mode, after);
}

pub fn delete_non_local(w: &Window, pane: &Rc<Pane>) {
    archive::delete_selection(w, pane);
}

pub fn rename_non_local(w: &Window, pane: &Rc<Pane>, item: &Item, new: &str) {
    archive::rename(w, pane, item, new);
}

pub fn create_non_local(w: &Window, loc: &Location, name: &str, folder: bool) {
    archive::create(w, loc, name, folder);
}

fn section(menu: &gio::Menu, items: &[(&str, &str)]) {
    let s = gio::Menu::new();
    for (label, action) in items {
        s.append(Some(label), Some(action));
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
        if local {
            if let Some(s) = super::customactions::menu_section(w) {
                menu.append_section(None, &s);
            }
        }
        section(&menu, &[("_Properties…", "win.properties")]);
        return menu;
    }
    let open = gio::Menu::new();
    open.append(Some("_Open"), Some("win.open"));
    if items.iter().all(Item::is_dir_like) {
        open.append(Some("Open in New _Tab"), Some("win.open-in-tab"));
    }
    if let Some(sub) = open_with_menu(items) {
        open.append_submenu(Some("Open _With"), &sub);
    }
    menu.append_section(None, &open);
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
    if items.len() == 1 && !items[0].is_dir_like() && !failbrauwser::archive::is_browsable_name(&items[0].display_name()) {
        arch.push(("Open as _Archive", "win.open-as-archive"));
    }
    if local {
        arch.push(("C_ompress…", "win.compress"));
    }
    section(&menu, &arch);
    if local || matches!(pane.location(), Location::Search(_)) {
        if let Some(s) = super::customactions::menu_section(w) {
            menu.append_section(None, &s);
        }
    }
    section(&menu, &[("_Properties…", "win.properties")]);
    menu
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
        if let Some(icon) = app.icon() {
            item.set_icon(&icon);
        }
        sub.append_item(&item);
    }
    let other = gio::Menu::new();
    other.append(Some("Other _Application…"), Some("win.open-with-other"));
    sub.append_section(None, &other);
    Some(sub)
}
