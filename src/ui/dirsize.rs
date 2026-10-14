//! Fills in folder sizes for the Size column: they are computed on a small shared pool of
//! worker threads and shown as they finish; leaving the folder cancels the rest. Folders
//! on network shares get a pool of their own, so a slow or hanging share never holds up
//! the local ones.

use super::pane::Pane;
use super::window::Window;
use failbrauwser::fs::dirsize::{DirSize, dir_size};
use failbrauwser::location::Location;
use gtk::glib;
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

const WORKERS: usize = 3;
const SHARE_WORKERS: usize = 2;

enum Msg {
    Progress(OsString, u64),
    Done(OsString, DirSize),
}

struct Task {
    name: OsString,
    path: PathBuf,
    cancel: Arc<AtomicBool>,
    reply: async_channel::Sender<Msg>,
}

fn pool(share: bool) -> &'static Mutex<mpsc::Sender<Task>> {
    static LOCAL: OnceLock<Mutex<mpsc::Sender<Task>>> = OnceLock::new();
    static SHARES: OnceLock<Mutex<mpsc::Sender<Task>>> = OnceLock::new();
    let (cell, workers, name) = if share { (&SHARES, SHARE_WORKERS, "fb-dirsize-share") } else { (&LOCAL, WORKERS, "fb-dirsize") };
    cell.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Task>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..workers {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || loop {
                    let task = { rx.lock().unwrap().recv() };
                    let Ok(t) = task else { return };
                    if t.cancel.load(Ordering::Relaxed) {
                        continue;
                    }
                    // A share that stopped answering would keep this worker for good.
                    if share && failbrauwser::ops::gvfs::check(&t.path, failbrauwser::ops::gvfs::TIMEOUT).is_err() {
                        continue;
                    }
                    let (reply, name) = (t.reply.clone(), t.name.clone());
                    let result = dir_size(&t.path, &t.cancel, |partial| {
                        let _ = reply.try_send(Msg::Progress(name.clone(), partial.bytes));
                    });
                    if let Some(size) = result {
                        let _ = t.reply.try_send(Msg::Done(t.name, size));
                    }
                })
                .expect("spawn size worker");
        }
        Mutex::new(tx)
    })
}

struct PaneSizes {
    pane: Weak<Pane>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    in_flight: Rc<RefCell<HashSet<OsString>>>,
    reply: async_channel::Sender<Msg>,
}

thread_local! {
    static STATES: RefCell<Vec<PaneSizes>> = const { RefCell::new(Vec::new()) };
}

fn cancel_for(pane: &Rc<Pane>) {
    STATES.with(|s| {
        s.borrow_mut().retain(|st| {
            let same = st.pane.upgrade().is_some_and(|p| Rc::ptr_eq(&p, pane));
            let dead = st.pane.strong_count() == 0;
            if same || dead {
                st.cancel.store(true, Ordering::Relaxed);
            }
            !(same || dead)
        })
    });
}

/// Requests sizes for folders that do not have one yet (after loading, changes, or when
/// the column is switched on).
pub fn update(w: &Window, pane: &Rc<Pane>) {
    let visible = w.app.settings.borrow().column_visible("size");
    if !visible || !matches!(pane.location(), Location::Dir(_) | Location::Search(_)) {
        cancel_for(pane);
        if visible {
            super::extensions::fill_sizes_non_local(pane);
        }
        return;
    }
    let generation = pane.generation();
    let current = STATES.with(|s| {
        s.borrow().iter().find(|st| st.pane.upgrade().is_some_and(|p| Rc::ptr_eq(&p, pane)) && st.generation == generation).map(|st| (st.cancel.clone(), st.in_flight.clone(), st.reply.clone()))
    });
    let (cancel, in_flight, reply) = match current {
        Some(x) => x,
        None => {
            cancel_for(pane);
            let cancel = Arc::new(AtomicBool::new(false));
            let in_flight = Rc::new(RefCell::new(HashSet::new()));
            let (tx, rx) = async_channel::unbounded();
            STATES.with(|s| {
                s.borrow_mut().push(PaneSizes { pane: Rc::downgrade(pane), generation, cancel: cancel.clone(), in_flight: in_flight.clone(), reply: tx.clone() })
            });
            let weak = Rc::downgrade(pane);
            let flight = in_flight.clone();
            let c2 = cancel.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = rx.recv().await {
                    let Some(p) = weak.upgrade() else { return };
                    if c2.load(Ordering::Relaxed) || p.generation() != generation {
                        return;
                    }
                    match msg {
                        Msg::Progress(name, bytes) => p.set_dir_size(&name, Some(bytes), true),
                        Msg::Done(name, size) => {
                            flight.borrow_mut().remove(&name);
                            p.set_dir_size(&name, Some(size.bytes), size.incomplete);
                        }
                    }
                }
            });
            (cancel, in_flight, tx)
        }
    };
    for (name, path) in pane.dirs_needing_size() {
        if !in_flight.borrow_mut().insert(name.clone()) {
            continue;
        }
        pane.set_dir_size(&name, None, false);
        let share = failbrauwser::ops::gvfs::is_gvfs_path(&path);
        let _ = pool(share).lock().unwrap().send(Task { name, path, cancel: cancel.clone(), reply: reply.clone() });
    }
}
