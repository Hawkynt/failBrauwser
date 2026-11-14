//! Keeps the version-control overlays of a tab up to date, whatever tool owns the folder: the
//! state of its entries is read on a worker thread whenever the folder changes, the tool's own
//! data changes (a commit, a checkout, …: its `.git`, `.hg`, `.svn` or `CVS` folder is watched),
//! the window comes back into use or a command of ours finished.

use super::super::pane::Pane;
use super::super::window::Window;
use super::icons::status_code;
use failbrauwser::vcs::{self, backend, Tool};
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

/// Folders checked for working trees of their own in one folder, at most.
const MAX_NESTED: usize = 300;

struct Watch {
    pane: Weak<Pane>,
    running: bool,
    again: bool,
    /// The repository data watched, and the watch.
    monitor: Option<(PathBuf, gio::FileMonitor)>,
    debounce: Option<glib::SourceId>,
}

thread_local! {
    static WATCHES: RefCell<Vec<Watch>> = const { RefCell::new(Vec::new()) };
}

fn with_watch<R>(pane: &Rc<Pane>, f: impl FnOnce(&mut Watch) -> R) -> R {
    WATCHES.with(|ws| {
        let mut ws = ws.borrow_mut();
        ws.retain(|w| w.pane.strong_count() > 0);
        let i = match ws.iter().position(|w| w.pane.upgrade().is_some_and(|p| Rc::ptr_eq(&p, pane))) {
            Some(i) => i,
            None => {
                ws.push(Watch { pane: Rc::downgrade(pane), running: false, again: false, monitor: None, debounce: None });
                ws.len() - 1
            }
        };
        f(&mut ws[i])
    })
}

/// The folders a tool keeps its own data in: never a working copy's content.
const ADMIN: &[&str] = &[".git", ".hg", ".svn", "CVS"];

/// The state of the entries of `dir`: from the working copy holding it, and for folders that are
/// working copies of their own, their summary. Answers the folder to watch for the tool's data.
pub fn compute(dir: &Path, dirs: &[OsString]) -> (Option<PathBuf>, HashMap<OsString, u32>) {
    let wc = backend::discover(dir);
    let mut out: HashMap<OsString, u32> = match &wc {
        Some(b) => b.folder_status(dir).unwrap_or_default().into_iter().map(|(k, v)| (k, status_code(Some(v)))).collect(),
        None => HashMap::new(),
    };
    let tool = wc.as_ref().map(|b| b.tool());
    for name in dirs.iter().filter(|n| !ADMIN.iter().any(|a| n.as_os_str() == *a)).take(MAX_NESTED) {
        let sub = dir.join(name);
        let Some(t) = vcs::marker_in(&sub) else { continue };
        // CVS (and Subversion before 1.7) leave their marker in every folder they track: below a
        // folder of the same tool it is the same working copy, not one of its own.
        if tool == Some(t) && matches!(t, Tool::Cvs | Tool::Subversion) {
            continue;
        }
        if let Some(s) = backend::open(&vcs::WorkingCopy { tool: t, root: sub.clone() }).and_then(|b| b.summary().ok()) {
            out.insert(name.clone(), status_code(Some(s)));
        }
    }
    // CVS keeps its data per folder: the one shown is what a commit or update rewrites.
    let watch = wc.as_ref().and_then(|b| if b.tool() == Tool::Cvs { Some(dir.join("CVS")) } else { b.admin_dir() });
    (watch, out)
}

/// Reads the overlays of the tab's folder again (soon, when asked often).
pub fn update(w: &Window, pane: &Rc<Pane>) {
    let enabled = {
        let s = w.app.settings.borrow();
        s.vcs_integration && s.vcs_overlays
    };
    let dir = match pane.location() {
        Location::Dir(d) if enabled && !failbrauwser::ops::gvfs::is_gvfs_path(&d) => d,
        _ => {
            pane.set_git_status(&HashMap::new());
            with_watch(pane, |wt| wt.monitor = None);
            return;
        }
    };
    if with_watch(pane, |wt| {
        if wt.running {
            wt.again = true;
        } else {
            wt.running = true;
        }
        wt.again
    }) {
        return;
    }
    let generation = pane.generation();
    let dirs = pane.dir_names();
    let (tx, rx) = async_channel::bounded(1);
    let d2 = dir.clone();
    std::thread::Builder::new()
        .name("fb-vcs-status".into())
        .spawn(move || {
            let _ = tx.send_blocking(compute(&d2, &dirs));
        })
        .expect("spawn vcs status");
    let weak = Rc::downgrade(pane);
    let ww = Rc::downgrade(&w.me());
    glib::spawn_future_local(async move {
        let Ok((watch, codes)) = rx.recv().await else { return };
        let Some(p) = weak.upgrade() else { return };
        let again = with_watch(&p, |wt| {
            wt.running = false;
            std::mem::take(&mut wt.again)
        });
        if p.generation() == generation && p.location() == Location::Dir(dir.clone()) {
            p.set_git_status(&codes);
            watch_repo(&p, ww.clone(), watch);
        }
        if again {
            if let Some(w) = ww.upgrade() {
                update(&w, &p);
            }
        }
    });
}

/// Watches the tool's data folder (Git's index, HEAD and refs, Mercurial's dirstate, Subversion's
/// wc.db, CVS's Entries: what commits, checkouts and merges write) to read the overlays again.
fn watch_repo(pane: &Rc<Pane>, window: Weak<Window>, want: Option<PathBuf>) {
    let current = with_watch(pane, |wt| wt.monitor.as_ref().map(|(p, _)| p.clone()));
    if current == want {
        return;
    }
    let monitor = want.as_ref().and_then(|dir| {
        let m = gio::File::for_path(dir).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE).ok()?;
        let pw = Rc::downgrade(pane);
        m.connect_changed(move |_, file, _, _| {
            // Lock and scratch files come and go while a client works; what they become counts.
            if file.basename().is_some_and(|n| failbrauwser::vcs::is_transient(&n.to_string_lossy())) {
                return;
            }
            let (Some(p), Some(w)) = (pw.upgrade(), window.upgrade()) else { return };
            soon(&w, &p);
        });
        Some((dir.clone(), m))
    });
    with_watch(pane, |wt| wt.monitor = monitor);
}

/// Reads the overlays again in a moment, once for a burst of changes.
pub fn soon(w: &Window, pane: &Rc<Pane>) {
    if let Some(id) = with_watch(pane, |wt| wt.debounce.take()) {
        id.remove();
    }
    let (ww, pw) = (Rc::downgrade(&w.me()), Rc::downgrade(pane));
    let id = glib::timeout_add_local_once(Duration::from_millis(300), move || {
        let (Some(w), Some(p)) = (ww.upgrade(), pw.upgrade()) else { return };
        with_watch(&p, |wt| wt.debounce = None);
        update(&w, &p);
    });
    with_watch(pane, |wt| wt.debounce = Some(id));
}

/// Every tab of every window reads its overlays again (after a Git command of ours).
pub fn refresh_all(w: &Window) {
    w.app.for_each_window(|win| {
        for p in win.panes() {
            soon(win, &p);
        }
    });
}

/// The window came back into use: what changed meanwhile (files edited elsewhere) shows.
pub fn window_created(w: &Rc<Window>) {
    let weak = Rc::downgrade(w);
    w.win.connect_is_active_notify(move |win| {
        if !win.is_active() {
            return;
        }
        if let Some(w) = weak.upgrade() {
            for p in w.panes() {
                soon(&w, &p);
            }
        }
    });
}

/// Whether reading the overlays of the tab is done (tests).
pub fn idle(pane: &Rc<Pane>) -> bool {
    with_watch(pane, |wt| !wt.running && wt.debounce.is_none())
}
