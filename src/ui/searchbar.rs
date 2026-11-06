//! Ctrl+F: a search bar above the list. Typing filters the current folder; with "In
//! subfolders" it searches below it (results stream in), "Inside archives" looks into
//! archives too. Closing the bar (Esc) returns to the folder.

use super::pane::Pane;
use super::window::Window;
use failbrauwser::location::{Location, SearchLoc};
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

struct Bar {
    pane: Weak<Pane>,
    bar: gtk::SearchBar,
    entry: gtk::SearchEntry,
    deep: gtk::CheckButton,
    archives: gtk::CheckButton,
    /// The folder the search started in.
    /// The folder searched below: on disk, or on a server or device the app browses itself.
    base: RefCell<Option<Location>>,
    /// Set while the bar itself changes the location.
    steering: Cell<bool>,
    /// What was applied last: the same text and options do not search again.
    applied: RefCell<Option<(String, bool, bool)>>,
}

thread_local! {
    static BARS: RefCell<Vec<Rc<Bar>>> = const { RefCell::new(Vec::new()) };
}

fn bar_for(pane: &Rc<Pane>) -> Option<Rc<Bar>> {
    BARS.with(|b| {
        let mut b = b.borrow_mut();
        b.retain(|x| x.pane.strong_count() > 0);
        b.iter().find(|x| x.pane.upgrade().is_some_and(|p| Rc::ptr_eq(&p, pane))).cloned()
    })
}

pub fn attach(pane: &Rc<Pane>) {
    let entry = gtk::SearchEntry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("Filter this folder — or search below it (* and ? work too)"));
    let deep = gtk::CheckButton::with_mnemonic("In _subfolders");
    let archives = gtk::CheckButton::with_mnemonic("Inside _archives");
    archives.set_sensitive(false);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.pack_start(&entry, true, true, 0);
    row.pack_start(&deep, false, false, 0);
    row.pack_start(&archives, false, false, 0);
    row.set_size_request(520, -1);
    let bar = gtk::SearchBar::new();
    bar.set_show_close_button(true);
    bar.add(&row);
    bar.connect_entry(&entry);
    row.show_all();
    bar.show();
    pane.root.pack_start(&bar, false, false, 0);
    pane.root.reorder_child(&bar, 0);

    let b = Rc::new(Bar { pane: Rc::downgrade(pane), bar, entry, deep, archives, base: RefCell::new(None), steering: Cell::new(false), applied: RefCell::new(None) });
    let w = Rc::downgrade(&b);
    b.entry.connect_search_changed(move |_| {
        if let Some(b) = w.upgrade() {
            b.apply();
        }
    });
    let w = Rc::downgrade(&b);
    b.deep.connect_toggled(move |d| {
        if let Some(b) = w.upgrade() {
            // archives on a server are not opened while searching (each would be downloaded)
            let remote = matches!(b.base.borrow().as_ref(), Some(Location::Remote(_)));
            b.archives.set_sensitive(d.is_active() && !remote);
            b.apply();
        }
    });
    let w = Rc::downgrade(&b);
    b.archives.connect_toggled(move |_| {
        if let Some(b) = w.upgrade() {
            b.apply();
        }
    });
    let w = Rc::downgrade(&b);
    b.bar.connect_search_mode_enabled_notify(move |bar| {
        if let Some(b) = w.upgrade() {
            if !bar.is_search_mode() {
                b.closed();
            }
        }
    });
    BARS.with(|x| x.borrow_mut().push(b));
}

impl Bar {
    fn apply(&self) {
        let Some(pane) = self.pane.upgrade() else { return };
        // The entry reports changes a moment late; once the bar is closed (a result was
        // opened, or Esc) they must not start a search again.
        if !self.bar.is_search_mode() {
            return;
        }
        let state = (self.entry.text().to_string(), self.deep.is_active(), self.archives.is_active());
        if self.applied.borrow().as_ref() == Some(&state) {
            return;
        }
        *self.applied.borrow_mut() = Some(state);
        let text = self.entry.text().to_string();
        let base = self.base.borrow().clone();
        self.steering.set(true);
        match (&base, self.deep.is_active() && !text.trim().is_empty()) {
            (Some(root), true) => {
                pane.set_filter("");
                let target = Location::Search(match root {
                    Location::Remote(r) => SearchLoc { root: PathBuf::new(), query: text, archives: false, remote: Some(r.clone()) },
                    other => SearchLoc { root: other.local_path().map(Path::to_path_buf).unwrap_or_default(), query: text, archives: self.archives.is_active(), remote: None },
                });
                if matches!(pane.location(), Location::Search(_)) {
                    pane.replace_location(target);
                } else {
                    // The first search step is kept in the history: Back returns to the folder.
                    pane.navigate(target);
                }
            }
            _ => {
                if let (Some(root), Location::Search(_)) = (&base, pane.location()) {
                    pane.replace_location(root.clone());
                }
                pane.set_filter(&text);
            }
        }
        self.steering.set(false);
    }

    fn closed(&self) {
        let Some(pane) = self.pane.upgrade() else { return };
        self.applied.borrow_mut().take();
        self.steering.set(true);
        pane.set_filter("");
        if let (Some(root), Location::Search(_)) = (self.base.borrow().clone(), pane.location()) {
            pane.replace_location(root);
        }
        self.steering.set(false);
        self.entry.set_text("");
        pane.focus_view();
    }
}

/// Opens the search bar of the current tab (Ctrl+F).
pub fn open(w: &Window) {
    let pane = w.current_pane();
    let Some(b) = bar_for(&pane) else { return };
    let base = match pane.location() {
        Location::Search(s) => Some(s.base()),
        Location::Dir(p) => Some(Location::Dir(p)),
        Location::Remote(r) => Some(Location::Remote(r)),
        _ => None,
    };
    // Searching below works from folders on disk and on servers or devices the app browses
    // itself; elsewhere the bar filters only.
    b.deep.set_sensitive(base.is_some());
    b.archives.set_sensitive(b.deep.is_active() && !matches!(base, Some(Location::Remote(_))));
    *b.base.borrow_mut() = base;
    b.bar.set_search_mode(true);
    b.entry.grab_focus();
}

/// Going to another place (not by searching) closes the bar and its filter.
pub fn location_changed(pane: &Rc<Pane>) {
    let Some(b) = bar_for(pane) else { return };
    if b.steering.get() || !b.bar.is_search_mode() {
        return;
    }
    let base = b.base.borrow().clone();
    let loc = pane.location();
    if matches!(loc, Location::Search(_)) || Some(&loc) == base.as_ref() {
        return;
    }
    b.bar.set_search_mode(false);
}

/// Types into the search bar of the current tab (tests).
pub fn type_text(w: &Window, text: &str) {
    if let Some(b) = bar_for(&w.current_pane()) {
        b.entry.set_text(text);
        // The entry reports changes after a short delay; apply at once.
        b.apply();
    }
}

pub fn set_options(w: &Window, deep: Option<bool>, archives: Option<bool>) {
    if let Some(b) = bar_for(&w.current_pane()) {
        if let Some(d) = deep {
            b.deep.set_active(d);
        }
        if let Some(a) = archives {
            b.archives.set_active(a);
        }
    }
}

pub fn close(w: &Window) {
    if let Some(b) = bar_for(&w.current_pane()) {
        b.bar.set_search_mode(false);
    }
}
