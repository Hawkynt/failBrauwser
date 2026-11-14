//! The right side of the status bar: what the current folder holds and how much space it
//! needs with all its subfolders. The total is measured in the background (symlink-safe,
//! hard links once, not across mounts) and replaced when the folder changes.

use super::window::Window;
use failbrauwser::fs::dirsize::{DirSize, dir_size};
use failbrauwser::fs::format::human_size;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Summary {
    window: Weak<Window>,
    label: gtk::Label,
    cancel: RefCell<Option<Arc<AtomicBool>>>,
    /// Bumped for every new measurement; stale results are dropped.
    token: Cell<u64>,
    pending: Cell<bool>,
    /// What was last measured (location and load); unchanged means nothing to redo.
    last: RefCell<Option<(String, u64)>>,
    force: Cell<bool>,
    /// Recent totals, so moving back and forth does not measure again at once.
    recent: RefCell<HashMap<PathBuf, (Instant, DirSize)>>,
}

thread_local! {
    static SUMMARIES: RefCell<Vec<Rc<Summary>>> = const { RefCell::new(Vec::new()) };
}

fn for_window(w: &Window) -> Option<Rc<Summary>> {
    SUMMARIES.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|x| x.window.strong_count() > 0);
        s.iter().find(|x| x.window.upgrade().is_some_and(|y| std::ptr::eq(&*y, w))).cloned()
    })
}

pub fn attach(w: &Rc<Window>, statusbar: &gtk::Statusbar) {
    let label = gtk::Label::new(None);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    label.set_margin_end(6);
    statusbar.pack_end(&label, false, false, 0);
    label.show();
    SUMMARIES.with(|s| {
        s.borrow_mut().push(Rc::new(Summary {
            window: Rc::downgrade(w),
            label,
            cancel: RefCell::new(None),
            token: Cell::new(0),
            pending: Cell::new(false),
            last: RefCell::new(None),
            force: Cell::new(false),
            recent: RefCell::new(HashMap::new()),
        }))
    });
}

/// The summary text shown (tests).
pub fn text(w: &Window) -> String {
    for_window(w).map(|s| s.label.text().to_string()).unwrap_or_default()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "34 files, 12 folders" — what is shown in the current folder.
fn counts(files: usize, dirs: usize) -> String {
    format!("{}, {}", plural(files, "file", "files"), plural(dirs, "folder", "folders"))
}

fn total_text(size: &DirSize, done: bool) -> String {
    let prefix = if done && !size.incomplete { "" } else { "≥ " };
    let tail = if done { "" } else { "…" };
    format!("{prefix}{}{tail} with subfolders", human_size(size.bytes))
}

fn tooltip(size: &DirSize) -> String {
    let mut t = format!("{} and {} in total", plural(size.files as usize, "file", "files"), plural(size.dirs.saturating_sub(1) as usize, "subfolder", "subfolders"));
    if size.incomplete {
        t.push_str("\nSome folders could not be read.");
    }
    if size.skipped_mounts {
        t.push_str("\nDrives mounted inside are not counted.");
    }
    t
}

/// Refreshes the summary when the tab shows another folder; `changed` when the folder's
/// contents changed (measure again). Selections do not change it.
pub fn update(w: &Window, changed: bool) {
    let Some(s) = for_window(w) else { return };
    let pane = w.current_pane();
    let sig = (pane.location().display(), pane.generation());
    if !changed && s.last.borrow().as_ref() == Some(&sig) {
        return;
    }
    *s.last.borrow_mut() = Some(sig);
    if changed {
        s.force.set(true);
    }
    if s.pending.replace(true) {
        return;
    }
    let weak = Rc::downgrade(&s);
    glib::timeout_add_local_once(Duration::from_millis(200), move || {
        if let Some(s) = weak.upgrade() {
            s.pending.set(false);
            s.refresh();
        }
    });
}

impl Summary {
    fn refresh(self: &Rc<Self>) {
        let Some(w) = self.window.upgrade() else { return };
        if let Some(c) = self.cancel.borrow_mut().take() {
            c.store(true, Ordering::Relaxed);
        }
        let token = self.token.get() + 1;
        self.token.set(token);
        let pane = w.current_pane();
        let (files, dirs) = pane.counts();
        let here = counts(files, dirs);
        self.label.set_tooltip_text(None);
        match pane.location() {
            Location::Drives | Location::Network(_) => self.label.set_text(""),
            Location::Search(_) => {
                let n = files + dirs;
                let state = if pane.is_searching() { " · searching…" } else { "" };
                self.label.set_text(&format!("{} found{state}", plural(n, "item", "items")));
            }
            Location::Archive(a) => {
                let total = failbrauwser::archive::vfs::Vfs::global().cached(&a).map(|x| x.tree.total_size(&a.inner));
                match total {
                    Some(t) => self.label.set_text(&format!("{here} · {} unpacked with subfolders", human_size(t))),
                    None => self.label.set_text(&here),
                }
            }
            Location::Dir(p) => self.measure(token, here, vec![p]),
            Location::Trash => {
                let dirs: Vec<PathBuf> = failbrauwser::trash::trash_dirs().iter().map(|d| d.files()).collect();
                self.measure(token, here, dirs);
            }
            // A server's totals would mean a recursive listing over the wire — not asked for.
            Location::Remote(_) => self.label.set_text(&here),
        }
    }

    fn measure(self: &Rc<Self>, token: u64, here: String, roots: Vec<PathBuf>) {
        let key = roots.first().cloned().unwrap_or_default();
        if self.force.replace(false) {
            self.recent.borrow_mut().remove(&key);
        }
        if let Some((when, size)) = self.recent.borrow().get(&key) {
            if when.elapsed() < Duration::from_secs(5) {
                self.label.set_text(&format!("{here} · {}", total_text(size, true)));
                self.label.set_tooltip_text(Some(&tooltip(size)));
                return;
            }
        }
        self.label.set_text(&format!("{here} · measuring…"));
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded::<(DirSize, bool)>();
        std::thread::Builder::new()
            .name("fb-summary".into())
            .spawn(move || {
                let mut total = DirSize::default();
                for root in &roots {
                    let base = total;
                    let progress_tx = tx.clone();
                    let r = dir_size(root, &cancel, |part| {
                        let mut t = base;
                        t.bytes += part.bytes;
                        let _ = progress_tx.try_send((t, false));
                    });
                    let Some(r) = r else { return };
                    total.bytes += r.bytes;
                    total.files += r.files;
                    total.dirs += r.dirs;
                    total.incomplete |= r.incomplete;
                    total.skipped_mounts |= r.skipped_mounts;
                }
                let _ = tx.try_send((total, true));
            })
            .expect("spawn summary thread");
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok((size, done)) = rx.recv().await {
                let Some(s) = weak.upgrade() else { return };
                if s.token.get() != token {
                    return;
                }
                s.label.set_text(&format!("{here} · {}", total_text(&size, done)));
                if done {
                    s.label.set_tooltip_text(Some(&tooltip(&size)));
                    s.recent.borrow_mut().insert(key.clone(), (Instant::now(), size));
                    return;
                }
            }
        });
    }
}
