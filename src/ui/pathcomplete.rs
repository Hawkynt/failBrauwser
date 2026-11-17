//! Path suggestions in the location bar (README: Location bar): while a path is typed, the
//! suggestion list shows the folders (and archives) of the folder being typed in, listed off the
//! UI thread; Tab completes as far as they agree. Network addresses keep their own suggestions
//! (`remote`), and the list switches between the two as the text changes.

use failbrauwser::pathcomplete as pc;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;

#[derive(Default)]
struct State {
    model: Option<gtk::ListStore>,
    /// The folder the offers are of, and how it was typed.
    listed: Option<(PathBuf, String)>,
    /// A folder being listed now.
    asking: Option<(PathBuf, String)>,
    /// The offers (full texts).
    offers: Vec<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The path suggestions, in the columns of the network ones (text, note).
fn model() -> gtk::ListStore {
    STATE.with(|s| s.borrow_mut().model.get_or_insert_with(|| gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING])).clone())
}

/// The text changed: when it is a path, its folder's names become the suggestions (listed once
/// per folder) and `true` is returned; otherwise nothing happens.
pub fn typed(entry: &gtk::Entry, text: &str) -> bool {
    let Some(t) = pc::split(text, &glib::home_dir()) else { return false };
    let store = model();
    if let Some(c) = entry.completion()
        && c.model().as_ref() != Some(store.upcast_ref::<gtk::TreeModel>())
    {
        c.set_model(Some(&store));
    }
    let key = (t.dir.clone(), t.dir_text.clone());
    let known = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.listed.as_ref() == Some(&key) {
            // Back in the folder listed: a listing of another one still running is not wanted.
            s.asking = None;
            return true;
        }
        if s.asking.as_ref() == Some(&key) {
            return true;
        }
        s.asking = Some(key.clone());
        false
    });
    if known {
        return true;
    }
    let weak = entry.downgrade();
    glib::spawn_future_local(async move {
        let dir = key.0.clone();
        let listing = gio::spawn_blocking(move || pc::list(&dir, failbrauwser::archive::is_browsable_name)).await.ok().and_then(|r| r.ok()).unwrap_or_default();
        let offers = pc::texts(&key.1, &listing);
        let current = STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.asking.as_ref() != Some(&key) {
                // Another folder was typed meanwhile; its own listing fills the list.
                return false;
            }
            s.asking = None;
            s.listed = Some(key.clone());
            s.offers = offers.clone();
            true
        });
        if !current {
            return;
        }
        let store = model();
        store.clear();
        for (text, (_, is_dir)) in offers.iter().zip(&listing) {
            let note = if *is_dir { "" } else { "archive" };
            store.insert_with_values(None, &[(0, text), (1, &note.to_string())]);
        }
        // Shown at once where the user is still typing in that folder. GTK opens the list on a
        // change of the text only (complete() just refreshes an open one), so the change is
        // told again; the folder is listed now, so nothing is listed twice.
        if let Some(e) = weak.upgrade()
            && e.is_focus()
            && e.text().starts_with(&key.1)
        {
            e.emit_by_name::<()>("changed", &[]);
        }
    });
    true
}

/// Tab in the location bar: a typed path goes on as far as its folder's names agree (the whole
/// name when one fits, a folder with its slash). `false` when the text is no path, so Tab moves
/// on as usual.
pub fn tab(entry: &gtk::Entry) -> bool {
    let text = entry.text().to_string();
    if !pc::is_path(&text) {
        return false;
    }
    let offers = STATE.with(|s| s.borrow().offers.clone());
    if let Some(longer) = pc::tab(&text, &offers) {
        entry.set_text(&longer);
        entry.set_position(-1);
    }
    true
}

/// The suggestions shown for what is typed now (tests).
pub fn shown(entry: &gtk::Entry) -> Vec<String> {
    let text = entry.text().to_string();
    STATE.with(|s| s.borrow().offers.iter().filter(|o| pc::matches(o, &text)).cloned().collect())
}

/// Whether the folder being typed in is listed (tests).
pub fn listed(entry: &gtk::Entry) -> bool {
    let Some(t) = pc::split(&entry.text(), &glib::home_dir()) else { return true };
    STATE.with(|s| s.borrow().listed.as_ref() == Some(&(t.dir, t.dir_text)))
}
