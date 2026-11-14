//! File operations as the user triggers them: menu actions, keyboard, paste, drop.

use super::clipboard::{self, ClipContent, ClipSource};
use super::jobs::Outcome;
use super::model::Item;
use super::pane::Pane;
use super::util;
use super::window::Window;
use failbrauwser::location::Location;
use failbrauwser::ops::copy::{self, Mode};
use failbrauwser::ops::undo::{self, Step, Undoable};
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
    w.add_action("undo", undo_last);
    w.add_action("compare-folders", |w| {
        // Two folders selected: those; else this folder and the one another tab shows.
        let pane = w.current_pane();
        let dirs: Vec<PathBuf> = pane.selected_items().iter().filter(|i| i.is_dir_like()).filter_map(|i| i.path().cloned()).collect();
        let (left, right) = if let [a, b] = dirs.as_slice() {
            (Some(a.clone()), Some(b.clone()))
        } else {
            let here = pane.location().local_path().map(Path::to_path_buf);
            let other = w.panes().iter().filter_map(|p| p.location().local_path().map(Path::to_path_buf)).find(|p| Some(p) != here.as_ref());
            (here, other)
        };
        super::compare::show(w, left, right);
    });
    w.add_action("find-duplicates", |w| {
        let here = w.current_pane().location().local_path().map(Path::to_path_buf);
        super::duplicates::show(w, here);
    });
    w.add_action("mount-image", |w| {
        let image = w.current_pane().selected_items().first().and_then(|i| i.path().cloned());
        if let Some(image) = image {
            super::drives::mount_image(&w.me(), image);
        }
    });
    update_sensitivity(w);
    update_undo(w);
}

/// Enables Ctrl+Z while there is something to take back, and says what in its tooltip.
pub fn update_undo(w: &Window) {
    set_enabled(w, "undo", w.app.next_undo().is_some());
}

/// Takes the last operation back.
fn undo_last(w: &Window) {
    let Some(u) = w.app.take_undo() else { return };
    let weak = Rc::downgrade(&w.me());
    let what = u.what.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Undoing: {}", u.what),
        move |ctx| undo::undo(ctx, &u.step),
        move |outcome| {
            let Some(w) = weak.upgrade() else { return };
            if let Outcome::Done(back) = outcome {
                w.set_status(&format!("Undone: {what}"));
                select_paths(&w, back);
            }
            for p in w.panes() {
                if matches!(p.location(), Location::Search(_)) {
                    p.reload();
                }
            }
        },
    );
}

/// Selects these paths in the tabs showing their folders, once they appear.
pub fn select_paths(w: &Window, paths: &[PathBuf]) {
    for p in w.panes() {
        let Some(dir) = p.location().local_path().map(Path::to_path_buf) else { continue };
        let names: Vec<OsString> = paths.iter().filter(|x| x.parent() == Some(&dir)).filter_map(|x| x.file_name().map(|n| n.to_os_string())).collect();
        if !names.is_empty() {
            p.select_when_present(names);
        }
    }
}

/// "“a.txt”" for one item, "3 items" for more.
fn what(paths: &[PathBuf]) -> String {
    match paths {
        [one] => format!("“{}”", one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        many => count_label(many.len(), "item"),
    }
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
    let browsable = !matches!(loc, Location::Drives | Location::Network(_));
    set_enabled(w, "copy", n > 0);
    set_enabled(w, "cut", n > 0 && writable);
    set_enabled(w, "paste", writable && browsable);
    set_enabled(w, "paste-into", n == 1 && writable);
    // Several files on disk are renamed together.
    let all_local = n > 0 && pane.selected_items().iter().all(|i| matches!(i, Item::Fs(_) | Item::Found(..)));
    set_enabled(w, "rename", writable && (n == 1 || all_local));
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
        set_enabled(w, "rename", on_disk);
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
            (Item::Remote(n), Location::Remote(r)) => Some(ClipSource::Remote(r.at.clone(), r.entry_path(&n.name), n.is_dir)),
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
            let key = failbrauwser::osprim::device_of(&dir);
            // What went where, for Ctrl+Z.
            let pairs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let p2 = pairs.clone();
            w.app.jobs.start_queued(
                w.win.upcast_ref(),
                title,
                key,
                move |ctx| {
                    let made = copy::transfer_pairs(ctx, &paths, &dir, mode, &opts)?;
                    *p2.lock().unwrap() = made.clone();
                    Ok(made.into_iter().map(|(_, dst)| dst).collect())
                },
                move |outcome| {
                    after();
                    let Some(w) = weak.upgrade() else { return };
                    // Even a cancelled copy made what it made: Ctrl+Z takes that back.
                    let pairs = std::mem::take(&mut *pairs.lock().unwrap());
                    let made: Vec<PathBuf> = pairs.iter().map(|(_, d)| d.clone()).filter(|d| d.symlink_metadata().is_ok()).collect();
                    if !made.is_empty() {
                        let (verb, step) = match mode {
                            Mode::Move => ("Move", Step::Moved(pairs)),
                            Mode::Copy => ("Copy", Step::Copied(made.clone())),
                        };
                        w.app.remember(Undoable { what: format!("{verb} {}", what(&made)), step });
                    }
                    if let Outcome::Done(created) = outcome {
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
    let weak = Rc::downgrade(&w.me());
    let p2 = paths.clone();
    w.app.jobs.start(w.win.upcast_ref(), title, move |ctx| delete::trash(ctx, &paths).map(|_| Vec::new()), move |outcome| {
        if let Some(w) = weak.upgrade() {
            // Those that are gone are in the trash (or were deleted when asked to).
            let gone: Vec<PathBuf> = p2.into_iter().filter(|p| p.symlink_metadata().is_err()).collect();
            if !gone.is_empty() {
                w.app.remember(Undoable { what: format!("Move {} to the trash", what(&gone)), step: Step::Trashed(gone) });
            }
        }
        done(outcome);
    });
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
    let items = pane.selected_items();
    if items.len() > 1 {
        super::batchrename::show(w, items);
        return;
    }
    let Some(item) = items.into_iter().next() else { return };
    let initial = item.display_name();
    ask_rename(w, item, initial);
}

/// Asks for the new name of `item`, starting from `initial`; a name that is taken asks
/// whether to overwrite, choose another name (back here) or leave it.
fn ask_rename(w: &Window, item: Item, initial: String) {
    let old = item.display_name();
    let weak = Rc::downgrade(&w.me());
    let title = format!("Rename “{old}”");
    util::ask_text(&w.win, &title, "New name:", &initial, "_Rename", !item.is_dir_like(), move |new| {
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
                    Ok(()) => {
                        pane.select_when_present(vec![OsString::from(&new)]);
                        w.app.remember(Undoable { what: format!("Rename “{old}”"), step: Step::Moved(vec![(e.path.clone(), target.clone())]) });
                        w.app.refresh_shares();
                    }
                    Err(err) if failbrauwser::osprim::is_exists(&err) => {
                        let folder = target.symlink_metadata().is_ok_and(|m| m.is_dir());
                        let place = target.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
                        name_taken(&w, item.clone(), new, &place, folder, {
                            let (from, pane) = (e.path.clone(), Rc::downgrade(&pane));
                            move |w, new| overwrite_local(w, &pane, from.clone(), target.clone(), new)
                        });
                    }
                    Err(err) => util::show_error(&w.win, "Cannot rename", &err.to_string()),
                }
            }
            Item::Archive(_) if pane.has_name(&new) => {
                let folder = pane.item_named(&new).is_some_and(|i| i.is_dir_like());
                let place = pane.location().title();
                let (p, it) = (Rc::downgrade(&pane), item.clone());
                name_taken(&w, item.clone(), new, &place, folder, move |w, new| {
                    if let Some(pane) = p.upgrade() {
                        super::archive::rename_replacing(w, &pane, &it, &new);
                    }
                });
            }
            Item::Remote(_) if pane.has_name(&new) => {
                // The server answers with its own "already exists" (FTP) or replaces (SFTP);
                // renaming onto a name shown in the pane always asks first.
                let folder = pane.item_named(&new).is_some_and(|i| i.is_dir_like());
                let place = pane.location().title();
                let (p, it) = (Rc::downgrade(&pane), item.clone());
                name_taken(&w, item.clone(), new, &place, folder, move |w, new| {
                    if let Some(pane) = p.upgrade() {
                        super::extensions::rename_replacing_non_local(w, &pane, &it, &new);
                    }
                });
            }
            Item::Archive(_) | Item::Remote(_) => super::extensions::rename_non_local(&w, &pane, &item, &new),
            Item::Trash(..) | Item::FoundInArchive(..) => {}
        }
    });
}

/// `new` is taken: overwrite (`overwrite`), ask for another name, or leave it.
fn name_taken(w: &Window, item: Item, new: String, place: &str, folder: bool, overwrite: impl Fn(&Rc<Window>, String) + 'static) {
    let weak = Rc::downgrade(&w.me());
    util::ask_name_taken(&w.win, &new.clone(), place, folder, move |choice| {
        let Some(w) = weak.upgrade() else { return };
        match choice {
            util::NameTaken::Overwrite => overwrite(&w, new.clone()),
            util::NameTaken::Rename => {
                // Back to the name, with one that is free suggested.
                let free = match &item {
                    Item::Fs(e) | Item::Found(e, _) => names::free_name(e.path.parent().unwrap_or(Path::new("/")), new.as_ref(), None),
                    _ => {
                        let pane = w.current_pane();
                        names::free_name_in(&|n| pane.has_name(&n.to_string_lossy()), new.as_ref(), None)
                    }
                };
                ask_rename(&w, item.clone(), free.to_string_lossy().into_owned());
            }
            util::NameTaken::Cancel => {}
        }
    });
}

/// Renames over what is there; a folder in the way is deleted on a worker thread.
fn overwrite_local(w: &Rc<Window>, pane: &std::rc::Weak<Pane>, from: PathBuf, to: PathBuf, new: String) {
    let title = format!("Renaming “{}”", from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let pane = pane.clone();
    w.app.jobs.start(
        w.win.upcast_ref(),
        title,
        move |_| copy::rename_replacing(&from, &to).map(|_| vec![to]),
        move |outcome| {
            if let (Some(p), Outcome::Done(_)) = (pane.upgrade(), outcome) {
                if matches!(p.location(), Location::Search(_)) {
                    p.reload();
                }
                p.select_when_present(vec![OsString::from(&new)]);
            }
        },
    );
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
                    Ok(()) => {
                        w.current_pane().select_when_present(vec![OsString::from(&name)]);
                        w.app.remember(Undoable { what: format!("Create “{name}”"), step: Step::Created(path.clone()) });
                        w.app.refresh_shares();
                    }
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
