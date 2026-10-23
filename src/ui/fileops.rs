//! File operations as the user triggers them: menu actions, keyboard, paste, drop.

use super::clipboard::{self, ClipContent, ClipSource};
use super::jobs::Outcome;
use super::model::Item;
use super::pane::Pane;
use super::util;
use super::window::Window;
use failbrauwser::location::Location;
use failbrauwser::ops::copy::{self, Mode};
use failbrauwser::ops::{delete, names};
use gtk::prelude::*;
use gtk::gio;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub fn install_actions(w: &Window) {
    w.add_action("copy", |w| copy_selection(w, false));
    w.add_action("cut", |w| copy_selection(w, true));
    w.add_action("paste", |w| {
        let dest = w.current_location();
        paste(w, dest);
    });
    w.add_action("paste-into", |w| {
        let p = w.current_pane();
        if let Some(dir) = p.selected_items().first().filter(|i| i.is_dir_like()).and_then(|i| p.location().child(&i.display_name())) {
            paste(w, dir);
        }
    });
    w.add_action("trash", trash_selection);
    w.add_action("delete", delete_selection);
    w.add_action("rename", rename_selection);
    w.add_action("new-folder", new_folder);
    w.add_action("new-file", new_file);
    w.add_action("link-on-desktop", link_on_desktop);
    update_sensitivity(w);
}

fn set_enabled(w: &Window, name: &str, on: bool) {
    if let Some(a) = w.win.lookup_action(name).and_then(|a| a.downcast::<gio::SimpleAction>().ok()) {
        a.set_enabled(on);
    }
}

/// Enables what makes sense for the current selection and location.
pub fn update_sensitivity(w: &Window) {
    let pane = w.current_pane();
    let loc = pane.location();
    let n = pane.selected_items().len();
    let writable = super::extensions::location_writable(&loc);
    let browsable = !matches!(loc, Location::Drives);
    set_enabled(w, "copy", n > 0);
    set_enabled(w, "cut", n > 0 && writable);
    set_enabled(w, "paste", writable && browsable);
    set_enabled(w, "paste-into", n == 1 && writable);
    set_enabled(w, "rename", n == 1 && writable);
    set_enabled(w, "trash", n > 0 && writable && matches!(loc, Location::Dir(_)));
    let in_trash = loc == Location::Trash;
    set_enabled(w, "delete", n > 0 && (writable || in_trash));
    set_enabled(w, "copy", n > 0 && !in_trash);
    super::trash::update_sensitivity(w, |name, on| set_enabled(w, name, on));
    set_enabled(w, "new-folder", writable && browsable);
    set_enabled(w, "new-file", writable && browsable);
    let items = pane.selected_items();
    let linkable = if items.is_empty() { loc.local_path().is_some() } else { !in_trash && items.iter().all(|i| i.path().is_some()) };
    set_enabled(w, "link-on-desktop", linkable);
    if matches!(loc, Location::Search(_)) {
        // Results on disk can be handled like files anywhere; nothing is created here.
        let on_disk = n > 0 && pane.selected_items().iter().all(|i| matches!(i, Item::Found(..)));
        set_enabled(w, "cut", on_disk);
        set_enabled(w, "trash", on_disk);
        set_enabled(w, "delete", on_disk);
        set_enabled(w, "rename", on_disk && n == 1);
    }
}

/// Links the selection (nothing selected: the folder shown) on the desktop.
fn link_on_desktop(w: &Window) {
    let pane = w.current_pane();
    let items = pane.selected_items();
    let links: Vec<_> = if items.is_empty() {
        pane.location().local_path().and_then(|p| super::desktoplink::for_dir(p, None)).into_iter().collect()
    } else {
        items.iter().filter_map(super::desktoplink::for_item).collect()
    };
    super::desktoplink::create(w, &links);
}

/// The selection as clipboard sources.
pub fn selection_sources(pane: &Pane) -> Vec<ClipSource> {
    let loc = pane.location();
    pane.selected_items()
        .iter()
        .filter_map(|i| match (i, &loc) {
            (Item::Fs(e) | Item::Found(e, _), _) => Some(ClipSource::Local(e.path.clone())),
            (Item::FoundInArchive(a, n, _), _) => Some(ClipSource::Archive(a.clone(), n.path.clone())),
            (Item::Archive(n), Location::Archive(a)) => Some(ClipSource::Archive(a.clone(), n.path.clone())),
            _ => None,
        })
        .collect()
}

fn copy_selection(w: &Window, cut: bool) {
    let items = selection_sources(&w.current_pane());
    if items.is_empty() {
        return;
    }
    clipboard::set(ClipContent { cut, items });
}

pub fn paste(w: &Window, dest: Location) {
    let weak = Rc::downgrade(&w.me());
    clipboard::get(move |content| {
        let (Some(w), Some(c)) = (weak.upgrade(), content) else { return };
        let mode = if c.cut { Mode::Move } else { Mode::Copy };
        let cut = c.cut;
        transfer(&w, c.items, dest.clone(), mode, move || {
            if cut {
                clipboard::clear_if_cut();
            }
        });
    });
}

fn count_label(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// Copies or moves sources to a location; everything that crosses into or out of an
/// archive is handled by the archive support.
pub fn transfer(w: &Window, sources: Vec<ClipSource>, dest: Location, mode: Mode, after: impl FnOnce() + 'static) {
    if sources.is_empty() {
        return;
    }
    let all_local = sources.iter().all(|s| matches!(s, ClipSource::Local(_)));
    match (&dest, all_local) {
        (Location::Dir(dir), true) => {
            let paths: Vec<PathBuf> = sources
                .into_iter()
                .filter_map(|s| match s {
                    ClipSource::Local(p) => Some(p),
                    _ => None,
                })
                .collect();
            let verb = if mode == Mode::Move { "Moving" } else { "Copying" };
            let title = format!("{verb} {} to “{}”", count_label(paths.len(), "item"), dest.title());
            let opts = copy::Options { smooth_writes: w.app.settings.borrow().smooth_writes, workers: None, verify: w.app.settings.borrow().verify_copies };
            let dir = dir.clone();
            let weak = Rc::downgrade(&w.me());
            // Copies to the same drive wait for each other.
            let key = std::fs::metadata(&dir).ok().map(|m| std::os::unix::fs::MetadataExt::dev(&m));
            w.app.jobs.start_queued(
                w.win.upcast_ref(),
                title,
                key,
                move |ctx| copy::transfer(ctx, &paths, &dir, mode, &opts),
                move |outcome| {
                    after();
                    if let (Some(w), Outcome::Done(created)) = (weak.upgrade(), outcome) {
                        select_created(&w, &dest, created);
                    }
                },
            );
        }
        _ => super::extensions::transfer_non_local(w, sources, dest, mode, Box::new(after)),
    }
}

/// Selects newly created items in the tabs showing their folder, once they appear.
pub fn select_created(w: &Window, dest: &Location, created: &[PathBuf]) {
    let names: Vec<OsString> = created.iter().filter_map(|p| p.file_name().map(|n| n.to_os_string())).collect();
    for p in w.panes() {
        if &p.location() == dest {
            p.select_when_present(names.clone());
        }
    }
}

fn selected_local(pane: &Pane) -> Vec<PathBuf> {
    pane.selected_items().iter().filter_map(|i| i.path().cloned()).collect()
}

/// Search results are not watched: after removing some, search again.
fn refresh_search_after(w: &Window) -> impl FnOnce(&Outcome) + 'static {
    let pane = Rc::downgrade(&w.current_pane());
    move |_| {
        if let Some(p) = pane.upgrade() {
            if matches!(p.location(), Location::Search(_)) {
                p.reload();
            }
        }
    }
}

fn trash_selection(w: &Window) {
    let pane = w.current_pane();
    if !matches!(pane.location(), Location::Dir(_) | Location::Search(_)) {
        // Archives have no trash: deleting from them is permanent.
        delete_selection(w);
        return;
    }
    let paths = selected_local(&pane);
    if paths.is_empty() {
        return;
    }
    let title = format!("Moving {} to the trash", count_label(paths.len(), "item"));
    let done = refresh_search_after(w);
    w.app.jobs.start(w.win.upcast_ref(), title, move |ctx| delete::trash(ctx, &paths).map(|_| Vec::new()), done);
}

fn delete_selection(w: &Window) {
    let pane = w.current_pane();
    let items = pane.selected_items();
    if items.is_empty() {
        return;
    }
    let what = if items.len() == 1 { format!("“{}”", items[0].display_name()) } else { format!("these {} items", items.len()) };
    let weak = Rc::downgrade(&w.me());
    util::confirm(&w.win, &format!("Permanently delete {what}?"), "Deleted items cannot be restored.", "_Delete", move || {
        let Some(w) = weak.upgrade() else { return };
        let pane = w.current_pane();
        if pane.location() == Location::Trash {
            super::trash::delete_selection(&w);
            return;
        }
        if let Location::Archive(_) = pane.location() {
            super::extensions::delete_non_local(&w, &pane);
            return;
        }
        let paths = selected_local(&pane);
        let title = format!("Deleting {}", count_label(paths.len(), "item"));
        let done = refresh_search_after(&w);
        w.app.jobs.start(w.win.upcast_ref(), title, move |ctx| delete::delete_permanently(ctx, &paths).map(|_| Vec::new()), done);
    });
}

fn rename_selection(w: &Window) {
    let pane = w.current_pane();
    let Some(item) = pane.selected_items().into_iter().next() else { return };
    let old = item.display_name();
    let weak = Rc::downgrade(&w.me());
    let prompt = format!("New name for “{old}”:");
    let initial = old.clone();
    util::ask_text(&w.win, "Rename", &prompt, &initial, "_Rename", !item.is_dir_like(), move |new| {
        let Some(w) = weak.upgrade() else { return };
        if new == old {
            return;
        }
        if !util::valid_file_name(&new) {
            util::show_error(&w.win, "Invalid name", "A name cannot be empty or contain “/”.");
            return;
        }
        let pane = w.current_pane();
        match &item {
            Item::Fs(e) | Item::Found(e, _) => {
                let target = e.path.with_file_name(&new);
                match copy::rename_noreplace(&e.path, &target) {
                    Ok(()) => pane.select_when_present(vec![OsString::from(&new)]),
                    Err(err) if err.raw_os_error() == Some(libc::EEXIST) => {
                        util::show_error(&w.win, "Cannot rename", &format!("“{new}” already exists."))
                    }
                    Err(err) => util::show_error(&w.win, "Cannot rename", &err.to_string()),
                }
            }
            Item::Archive(_) => super::extensions::rename_non_local(&w, &pane, &item, &new),
            Item::Trash(..) | Item::FoundInArchive(..) => {}
        }
    });
}

fn create_named(w: &Window, title: &str, default: &str, folder: bool) {
    let pane = w.current_pane();
    let loc = pane.location();
    let initial = if pane.has_name(default) {
        names::free_name_in(&|n| pane.has_name(&n.to_string_lossy()), default.as_ref(), None).to_string_lossy().into_owned()
    } else {
        default.to_string()
    };
    let weak = Rc::downgrade(&w.me());
    let label = if folder { "Folder name:" } else { "File name:" };
    util::ask_text(&w.win, title, label, &initial, "C_reate", !folder, move |name| {
        let Some(w) = weak.upgrade() else { return };
        if !util::valid_file_name(&name) {
            util::show_error(&w.win, "Invalid name", "A name cannot be empty or contain “/”.");
            return;
        }
        match &loc {
            Location::Dir(dir) => {
                let path = dir.join(&name);
                let res = if folder { std::fs::create_dir(&path) } else { create_empty_file(&path) };
                match res {
                    Ok(()) => w.current_pane().select_when_present(vec![OsString::from(&name)]),
                    Err(e) => util::show_error(&w.win, &format!("Cannot create “{name}”"), &e.to_string()),
                }
            }
            _ => super::extensions::create_non_local(&w, &loc, &name, folder),
        }
    });
}

fn create_empty_file(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path).map(|_| ())
}

fn new_folder(w: &Window) {
    create_named(w, "Create New Folder", "New Folder", true);
}

fn new_file(w: &Window) {
    create_named(w, "Create New Document", "New Document.txt", false);
}
