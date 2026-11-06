//! The trash view's operations: restore, delete permanently, empty. The trash itself is the
//! freedesktop.org one, shared with Thunar and other file managers.

use super::jobs::Outcome;
use super::model::Item;
use super::util;
use super::window::Window;
use failbrauwser::location::Location;
use failbrauwser::ops::fastcopy;
use failbrauwser::ops::job::{Answer, Question};
use failbrauwser::ops::names::free_name;
use failbrauwser::trash::{self, TrashItem};
use gtk::prelude::*;
use std::rc::Rc;

/// The bar at the top of the trash view: how much is in it, Restore and Empty Trash.
struct TrashBar {
    pane: std::rc::Weak<super::pane::Pane>,
    bar: gtk::ActionBar,
    count: gtk::Label,
    restore: gtk::Button,
    empty: gtk::Button,
}

thread_local! {
    static BARS: std::cell::RefCell<Vec<TrashBar>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Adds the (hidden) trash bar to a tab; it shows while the tab shows the trash.
pub fn attach(pane: &Rc<super::pane::Pane>) {
    let bar = gtk::ActionBar::new();
    let count = gtk::Label::new(None);
    count.style_context().add_class("dim-label");
    bar.pack_start(&count);
    let empty = gtk::Button::with_mnemonic("_Empty Trash");
    empty.set_action_name(Some("win.empty-trash"));
    empty.style_context().add_class("destructive-action");
    empty.set_tooltip_text(Some("Delete everything in the trash permanently"));
    bar.pack_end(&empty);
    let restore = gtk::Button::with_mnemonic("_Restore");
    restore.set_action_name(Some("win.restore"));
    restore.set_tooltip_text(Some("Put the selected items back where they were deleted from"));
    bar.pack_end(&restore);
    // The contents are always shown; the bar itself only in the trash (and "show all" on
    // the window must not reveal it elsewhere).
    for w in [count.upcast_ref::<gtk::Widget>(), restore.upcast_ref(), empty.upcast_ref()] {
        w.show();
    }
    bar.set_no_show_all(true);
    pane.root.pack_start(&bar, false, false, 0);
    // Directly above the file list, below the error bar.
    pane.root.reorder_child(&bar, 1);
    BARS.with(|b| {
        let mut b = b.borrow_mut();
        b.retain(|x| x.pane.strong_count() > 0);
        b.push(TrashBar { pane: Rc::downgrade(pane), bar, count, restore, empty });
    });
}

/// Shows the bar in the trash, hides it elsewhere, and keeps its count current.
pub fn update_bar(pane: &Rc<super::pane::Pane>) {
    BARS.with(|b| {
        for tb in b.borrow().iter() {
            if !tb.pane.upgrade().is_some_and(|p| Rc::ptr_eq(&p, pane)) {
                continue;
            }
            if pane.location() != Location::Trash {
                tb.bar.hide();
                continue;
            }
            let n = pane.item_count();
            tb.count.set_text(&match n {
                0 => "The trash is empty".to_string(),
                1 => "1 item in the trash".to_string(),
                n => format!("{n} items in the trash"),
            });
            tb.bar.show();
        }
    });
}

/// Clicks a button of the trash bar ("Restore" or "Empty Trash"); false if it is not
/// shown or not clickable (tests).
pub fn click_bar(pane: &Rc<super::pane::Pane>, label: &str) -> bool {
    BARS.with(|b| {
        for tb in b.borrow().iter() {
            if !tb.pane.upgrade().is_some_and(|p| Rc::ptr_eq(&p, pane)) || !tb.bar.is_visible() {
                continue;
            }
            let button = if label == "Restore" { &tb.restore } else { &tb.empty };
            if button.is_sensitive() && button.is_visible() && tb.bar.allocated_height() > 1 {
                button.clicked();
                return true;
            }
        }
        false
    })
}

fn selected(w: &Window) -> Vec<TrashItem> {
    w.current_pane()
        .selected_items()
        .into_iter()
        .filter_map(|i| match i {
            Item::Trash(t, _) => Some(t),
            _ => None,
        })
        .collect()
}

pub fn install_actions(w: &Window) {
    w.add_action("restore", restore_selection);
    w.add_action("empty-trash", empty);
}

pub fn update_sensitivity(w: &Window, set: impl Fn(&str, bool)) {
    let in_trash = w.current_location() == Location::Trash;
    set("restore", in_trash && !selected(w).is_empty());
    set("empty-trash", !trash::is_empty(&trash::trash_dirs()));
}

pub fn restore_selection(w: &Window) {
    let items = selected(w);
    if items.is_empty() {
        return;
    }
    let n = items.len();
    let weak = Rc::downgrade(&w.me());
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Restoring {n} item{}", if n == 1 { "" } else { "s" }),
        move |ctx| {
            let mut done = Vec::new();
            for item in &items {
                if ctx.is_cancelled() {
                    return Err(fastcopy::cancelled());
                }
                ctx.set_current(item.original_name());
                match trash::restore(item, None) {
                    Ok(p) => done.push(p),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        let there = std::fs::symlink_metadata(&item.original).ok();
                        let here = std::fs::symlink_metadata(&item.path).ok();
                        let q = Question::Conflict {
                            src: item.path.clone(),
                            dst: item.original.clone(),
                            src_size: here.as_ref().map(|m| m.len()).unwrap_or(0),
                            dst_size: there.as_ref().map(|m| m.len()).unwrap_or(0),
                            src_mtime: here.as_ref().map(|m| failbrauwser::osprim::times(m).1.0).unwrap_or(0),
                            dst_mtime: there.as_ref().map(|m| failbrauwser::osprim::times(m).1.0).unwrap_or(0),
                            dst_is_dir: there.is_some_and(|m| m.is_dir()),
                        };
                        let parent = item.original.parent().unwrap_or(std::path::Path::new("/")).to_path_buf();
                        let name = item.original.file_name().unwrap_or_default().to_os_string();
                        match ctx.ask(q) {
                            Answer::KeepBoth => done.push(trash::restore(item, Some(&parent.join(free_name(&parent, &name, None))))?),
                            Answer::Replace => {
                                // What is in the way goes to the trash instead of being lost.
                                if let Err(e) = trash::trash_file(&item.original) {
                                    ctx.record_error(format!("{}: {e}", item.original.display()));
                                    continue;
                                }
                                done.push(trash::restore(item, None)?);
                            }
                            Answer::Cancel => return Err(fastcopy::cancelled()),
                            _ => {}
                        }
                    }
                    Err(e) => ctx.record_error(format!("{}: {e}", item.original_name())),
                }
            }
            Ok(done)
        },
        move |outcome| {
            if let (Some(w), Outcome::Done(paths)) = (weak.upgrade(), outcome) {
                let n = paths.len();
                w.set_status(&format!("Restored {n} item{}.", if n == 1 { "" } else { "s" }));
            }
        },
    );
}

pub fn delete_selection(w: &Window) {
    let items = selected(w);
    if items.is_empty() {
        return;
    }
    let n = items.len();
    w.app.jobs.start(
        w.win.upcast_ref(),
        format!("Deleting {n} item{} for good", if n == 1 { "" } else { "s" }),
        move |ctx| {
            for item in &items {
                ctx.set_current(item.original_name());
                if let Err(e) = trash::delete(item) {
                    ctx.record_error(format!("{}: {e}", item.original_name()));
                }
            }
            Ok(Vec::new())
        },
        |_| {},
    );
}

pub fn empty(w: &Window) {
    let weak = Rc::downgrade(&w.me());
    util::confirm(&w.win, "Empty the trash?", "All items in the trash will be deleted permanently.", "_Empty Trash", move || {
        let Some(w) = weak.upgrade() else { return };
        w.app.jobs.start(w.win.upcast_ref(), "Emptying the trash", |_| trash::empty(&trash::trash_dirs()).map(|_| Vec::new()), |_| {});
    });
}

/// "“report.pdf” (1.2 MB), deleted from /home/me/Documents on 2026-09-29 10:15".
pub fn status_text(t: &TrashItem, size: u64, dir: bool) -> String {
    let from = t.original.parent().map(|p| p.display().to_string()).unwrap_or_default();
    let when = failbrauwser::fs::format::human_time(t.deleted_unix(), gtk::glib::real_time() / 1_000_000);
    let size = if dir { String::new() } else { format!(" ({})", failbrauwser::fs::format::human_size(size)) };
    format!("“{}”{size}, deleted from {from} {when}", t.original_name())
}
