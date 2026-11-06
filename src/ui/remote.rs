//! Suggestions for network addresses in the location bar: addresses opened before, SMB
//! servers on the local network and the shares of the server being typed.

use failbrauwser::remote::{self, Host};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long a search of the network counts as current.
const SCAN_AGAIN_AFTER: Duration = Duration::from_secs(120);

const COL_URI: u32 = 0;
const COL_NOTE: u32 = 1;

#[derive(Default)]
struct State {
    model: Option<gtk::ListStore>,
    history: Option<Vec<String>>,
    hosts: Vec<Host>,
    scanned: Option<Instant>,
    scanning: bool,
    /// Share names by server, as GVfs lists them.
    shares: HashMap<String, Vec<String>>,
    shares_asked: HashSet<String>,
    /// Entries to show the new suggestions in once they arrive.
    entries: Vec<glib::WeakRef<gtk::Entry>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn model() -> gtk::ListStore {
    STATE.with(|s| {
        s.borrow_mut()
            .model
            .get_or_insert_with(|| gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]))
            .clone()
    })
}

pub fn history() -> Vec<String> {
    STATE.with(|s| s.borrow_mut().history.get_or_insert_with(|| remote::load_history_from(&remote::history_path())).clone())
}

/// Fills the shared suggestion list and pops it up again in entries being typed in.
fn rebuild() {
    let model = model();
    model.clear();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |uri: String, note: &str| {
        if seen.insert(uri.to_lowercase()) {
            model.insert_with_values(None, &[(COL_URI, &uri), (COL_NOTE, &note)]);
        }
    };
    for uri in history() {
        add(uri, "opened before");
    }
    let (hosts, shares) = STATE.with(|s| {
        let s = s.borrow();
        (s.hosts.clone(), s.shares.clone())
    });
    for (host, names) in &shares {
        for name in names {
            add(format!("smb://{host}/{name}"), &format!("share on {host}"));
        }
    }
    for h in &hosts {
        let note = match &h.name {
            Some(n) => format!("SMB server · {n}"),
            None => "SMB server".to_string(),
        };
        add(h.uri(), &note);
        if let Some(n) = &h.name {
            add(format!("smb://{n}"), &format!("SMB server · {}", h.addr));
        }
    }
    let entries: Vec<gtk::Entry> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.entries.retain(|e| e.upgrade().is_some());
        s.entries.iter().filter_map(|e| e.upgrade()).collect()
    });
    for e in entries {
        if e.has_focus() && !e.text().is_empty() {
            if let Some(c) = e.completion() {
                c.complete();
            }
        }
    }
}

/// Searches the local network for SMB servers in the background, unless that was done lately.
fn scan() {
    let start = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let fresh = s.scanned.is_some_and(|t| t.elapsed() < SCAN_AGAIN_AFTER);
        if s.scanning || fresh {
            return false;
        }
        s.scanning = true;
        true
    });
    if !start {
        return;
    }
    glib::spawn_future_local(async {
        let hosts = gio::spawn_blocking(remote::find_smb_servers).await.unwrap_or_default();
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.scanning = false;
            s.scanned = Some(Instant::now());
            s.hosts = hosts;
        });
        rebuild();
    });
}

/// Lists the shares of an SMB server through GVfs (without asking for a password: servers
/// that want one show no shares here).
fn list_shares(host: &str) {
    let first = STATE.with(|s| s.borrow_mut().shares_asked.insert(host.to_lowercase()));
    if !first {
        return;
    }
    let host = host.to_string();
    let server = gio::File::for_uri(&format!("smb://{host}/"));
    glib::spawn_future_local(async move {
        match server.mount_enclosing_volume_future(gio::MountMountFlags::NONE, gio::MountOperation::NONE).await {
            Ok(()) => {}
            Err(e) if e.matches(gio::IOErrorEnum::AlreadyMounted) => {}
            Err(_) => return,
        }
        let Ok(children) = server.enumerate_children_future("standard::name,standard::display-name", gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await else {
            return;
        };
        let mut names = Vec::new();
        while let Ok(batch) = children.next_files_future(64, glib::Priority::DEFAULT).await {
            if batch.is_empty() {
                break;
            }
            names.extend(batch.iter().map(|i| i.display_name().to_string()));
        }
        // The server's hidden administrative shares (C$, IPC$) are not for browsing.
        names.retain(|n| !n.ends_with('$'));
        names.sort_by_key(|n| n.to_lowercase());
        STATE.with(|s| s.borrow_mut().shares.insert(host, names));
        rebuild();
    });
}

/// Starts what the text being typed could use: the network search once something address-like
/// is typed, the share list once a server is complete.
fn on_typed(text: &str) {
    let t = text.trim();
    if t.is_empty() || t.starts_with('/') || t.starts_with('~') {
        return;
    }
    // A test does not go looking through the real network.
    if !super::selftest::active() {
        scan();
    }
    if cfg!(unix) {
        // Suggestions are smb:// here; a typed \\host\share matches them spelled as such.
        // On Windows the share list would come over GVfs, which does not serve this - the
        // typed name is a native path there and the network page is its own story.
        let unc = remote::unc_as_typed(t);
        if let Some(host) = remote::smb_host(unc.as_deref().unwrap_or(t)) {
            list_shares(host);
        }
    } else if let Some(host) = remote::smb_host(t) {
        list_shares(host);
    }
}

/// Adds the suggestions to a location entry.
pub fn attach(entry: &gtk::Entry) {
    let completion = gtk::EntryCompletion::new();
    completion.set_model(Some(&model()));
    completion.set_text_column(COL_URI as i32);
    completion.set_match_func(|c, typed, iter| {
        let Some(model) = c.model() else { return false };
        let uri: String = model.value(iter, COL_URI as i32).get().unwrap_or_default();
        // GTK hands over a normalized, lower-case key; take what is really typed.
        let typed = c.entry().map(|e| e.text().to_string()).unwrap_or_else(|| typed.to_string());
        remote::suggestion_matches(&uri, &typed)
    });
    let note = gtk::CellRendererText::new();
    note.set_property("foreground-rgba", gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
    note.set_property("xpad", 12u32);
    completion.pack_end(&note, false);
    completion.add_attribute(&note, "text", COL_NOTE as i32);
    STATE.with(|s| s.borrow_mut().entries.push(entry.downgrade()));
    // A chosen server gets its slash, which lists its shares.
    completion.connect_match_selected(|c, model, iter| {
        let uri: String = model.value(iter, COL_URI as i32).get().unwrap_or_default();
        let is_server = uri.split_once("://").is_some_and(|(_, rest)| !rest.contains('/'));
        let Some(e) = c.entry().and_then(|e| e.downcast::<gtk::Entry>().ok()) else { return glib::Propagation::Proceed };
        e.set_text(&if is_server { format!("{uri}/") } else { uri });
        e.set_position(-1);
        glib::Propagation::Stop
    });
    entry.set_completion(Some(&completion));
    STATE.with(|s| s.borrow_mut().entries.push(entry.downgrade()));
    entry.connect_changed(|e| on_typed(&e.text()));
    rebuild();
}

/// Records an address that was opened, first in the suggestions from now on.
pub fn remember(uri: &str) {
    match remote::remember_in(&remote::history_path(), uri) {
        Ok(list) => STATE.with(|s| s.borrow_mut().history = Some(list)),
        Err(e) => eprintln!("failbrauwser: cannot save the address history: {e}"),
    }
    rebuild();
}

// ── The sign-in questions of the app's own remote stack (Windows) ──────────

thread_local! {
    /// The application, kept where GTK objects may live (the UI thread); the sign-in
    /// answer dialogs attach to its active window.
    #[cfg(windows)]
    static APP: std::cell::RefCell<Option<gtk::Application>> = const { std::cell::RefCell::new(None) };
}

/// Installs the prompter the SFTP/FTP sign-in asks through: the worker thread parks on
/// an mpsc channel while the answer dialog runs on the UI thread. With no window up
/// (or no scripted reply in a self-test) the answer is an honest Cancel — the engine
/// then fails the sign-in instead of hanging.
#[cfg(windows)]
pub fn install_prompts(app: &gtk::Application) {
    use failbrauwser::remote_fs::auth::{self, Answer, Ask};
    use std::sync::mpsc::{channel, Sender};
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    auth::set_prompter(Some(Box::new(|ask| {
        let (tx, rx) = channel();
        let ask = ask.clone();
        // A main context that is gone drops the sender unsent: the worker takes that as
        // a Cancel instead of waiting forever. (The application object itself is never
        // sent to this thread — GTK objects stay on the UI thread.)
        glib::idle_add_local_once(move || answer_prompt(&ask, tx));
        rx.recv().unwrap_or(Answer::Cancel)
    })));

    fn answer_prompt(ask: &Ask, tx: Sender<Answer>) {
        // The self-test's script lives in this thread's locals, so it is read here.
        if super::selftest::active() {
            let a = match ask {
                Ask::HostKey { .. } => Answer::Trust,
                _ => super::selftest::scripted_text().map_or(Answer::Cancel, Answer::Secret),
            };
            let _ = tx.send(a);
            return;
        }
        // The application's own window, reached only on the thread that holds it.
        let Some(parent) = APP.with(|a| a.borrow().clone()).and_then(|a| a.active_window()) else {
            let _ = tx.send(Answer::Cancel);
            return;
        };
        match ask {
            Ask::Password { target, again } => secret(&parent, &format!("Sign in to {target}"), if *again { "Wrong password — try again:" } else { "Password:" }, "_Sign in", tx),
            Ask::Passphrase { path } => secret(&parent, "Private key", &format!("Passphrase for {path}:"), "_Unlock", tx),
            Ask::HostKey { host, port, key } => {
                let d = gtk::MessageDialog::new(Some(&parent), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Warning, gtk::ButtonsType::None, &format!("Unknown host key for {host}:{port}"));
                d.set_secondary_text(Some(&format!("Nobody has seen this key before. Accept it for good?\n\n{key}")));
                d.add_button("_Refuse", gtk::ResponseType::Cancel);
                if let Some(b) = d.add_button("_Accept key", gtk::ResponseType::Other(0)).downcast_ref::<gtk::Button>() {
                    b.style_context().add_class("suggested-action");
                }
                // Closing the dialog answers once more (as a delete event).
                let answered = std::cell::Cell::new(false);
                d.connect_response(move |d, resp| {
                    if answered.replace(true) {
                        return;
                    }
                    unsafe { d.destroy() };
                    let _ = tx.send(if resp == gtk::ResponseType::Other(0) { Answer::Trust } else { Answer::Refuse });
                });
                d.show_all();
            }
        }
    }

    /// Like `util::ask_secret`, but every answer (cancel and close included) reaches the
    /// worker that is waiting on it.
    fn secret(parent: &impl IsA<gtk::Window>, title: &str, label: &str, ok_label: &str, tx: Sender<Answer>) {
        let d = gtk::Dialog::with_buttons(Some(title), Some(parent), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Cancel", gtk::ResponseType::Cancel), (ok_label, gtk::ResponseType::Ok)]);
        d.set_default_response(gtk::ResponseType::Ok);
        d.set_resizable(false);
        let area = d.content_area();
        area.set_spacing(6);
        area.set_border_width(12);
        let l = gtk::Label::new(Some(label));
        l.set_xalign(0.0);
        let entry = gtk::Entry::new();
        entry.set_visibility(false);
        entry.set_input_purpose(gtk::InputPurpose::Password);
        entry.set_activates_default(true);
        entry.set_width_chars(32);
        area.add(&l);
        area.add(&entry);
        d.show_all();
        entry.grab_focus();
        let answered = std::cell::Cell::new(false);
        d.connect_response(move |d, resp| {
            if answered.replace(true) {
                return;
            }
            let text = entry.text().to_string();
            unsafe { d.destroy() };
            let _ = tx.send(if resp == gtk::ResponseType::Ok && !text.is_empty() { Answer::Secret(text) } else { Answer::Cancel });
        });
    }
}

// ── Remote folders in the panes (Windows) ──────────────────────────────────

/// Open (with write-back while the application holds the file), delete, rename and
/// create in a server folder. Every engine call runs inside a job; the panes showing
/// that server reload when it is done.
#[cfg(windows)]
pub mod view {
    use super::super::archive::io as arc_io;
    use super::super::jobs::Outcome;
    use super::super::model::Item;
    use super::super::pane::Pane;
    use super::super::util;
    use super::super::window::Window;
    use failbrauwser::archive::vfs::Vfs;
    use failbrauwser::location::{Location, RemoteLoc};
    use failbrauwser::ops::staging;
    use failbrauwser::remote_fs;
    use gtk::prelude::*;
    use gtk::{gio, glib};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::Duration;

    thread_local! {
        /// Files extracted for an application; their monitors save edits back.
        static OPENED: RefCell<Vec<gio::FileMonitor>> = const { RefCell::new(Vec::new()) };
    }

    pub fn open_entry(w: &Window, item: &Item) {
        let Some((loc, node)) = item.remote_of(&w.current_location()) else { return };
        if node.is_dir {
            return;
        }
        let (target, remote) = (loc.at.clone(), loc.entry_path(&node.name));
        let (name, remote2) = (node.name.clone(), remote.clone());
        let weak = Rc::downgrade(&w.me());
        w.app.jobs.start(
            w.win.upcast_ref(),
            format!("Opening “{name}”"),
            move |_ctx| {
                let dir = Vfs::global().scratch_dir("remote-open").map_err(arc_io)?;
                staging::created(&dir);
                let local = dir.join(&name);
                remote_fs::read_to(&target, &remote, &local)?;
                Ok(vec![local])
            },
            move |outcome| {
                let (Some(w), Outcome::Done(paths)) = (weak.upgrade(), outcome) else { return };
                let Some(path) = paths.first().cloned() else { return };
                util::open_with_default(w.win.upcast_ref(), &path);
                watch_saved(&w, loc, remote2, path);
            },
        );
    }

    /// Saves an opened file back to the server whenever an application writes it
    /// (debounced the way editor bursts settle, exactly like archive edits).
    fn watch_saved(w: &Window, loc: RemoteLoc, remote: String, path: PathBuf) {
        let Ok(monitor) = gio::File::for_path(&path).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else { return };
        let pending = Rc::new(std::cell::Cell::new(false));
        let weak = Rc::downgrade(&w.me());
        monitor.connect_changed(move |_, _, _, ev| {
            use gio::FileMonitorEvent as E;
            if !matches!(ev, E::ChangesDoneHint | E::Created | E::MovedIn | E::Renamed | E::Changed) || pending.replace(true) {
                return;
            }
            let (weak, loc, remote, path, pending) = (weak.clone(), loc.clone(), remote.clone(), path.clone(), pending.clone());
            glib::timeout_add_local_once(Duration::from_millis(800), move || {
                pending.set(false);
                let Some(w) = weak.upgrade() else { return };
                if !path.is_file() {
                    return;
                }
                let (target, remote2, path2) = (loc.at.clone(), remote.clone(), path.clone());
                let name = remote.rsplit('/').next().unwrap_or(&remote).to_string();
                let w2 = Rc::downgrade(&w);
                w.app.jobs.start(
                    w.win.upcast_ref(),
                    format!("Saving “{name}” to the server"),
                    move |_ctx| remote_fs::write_from(&target, &path2, &remote2).map(|_| Vec::new()),
                    move |outcome| {
                        if let Some(w) = w2.upgrade() {
                            if matches!(outcome, Outcome::Done(_)) {
                                w.set_status(&format!("Sent the changes to “{name}” to the server."));
                            }
                        }
                    },
                );
            });
        });
        OPENED.with(|o| o.borrow_mut().push(monitor));
    }

    pub fn delete_selection(w: &Window, pane: &Rc<Pane>) {
        let Location::Remote(loc) = pane.location() else { return };
        let entries: Vec<(String, bool)> = pane
            .selected_items()
            .iter()
            .filter_map(|i| match i {
                Item::Remote(n) => Some((loc.entry_path(&n.name), n.is_dir)),
                _ => None,
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        let target = loc.at.clone();
        let n = entries.len();
        let pw = Rc::downgrade(pane);
        w.app.jobs.start(
            w.win.upcast_ref(),
            format!("Deleting {n} item{} from the server", if n == 1 { "" } else { "s" }),
            move |_ctx| {
                // Removal at the server is final: the other machine's folder has no trash.
                for (path, is_dir) in &entries {
                    remote_fs::remove(&target, path, *is_dir)?;
                }
                Ok(Vec::new())
            },
            move |outcome| {
                if let (Some(pane), Outcome::Done(_)) = (pw.upgrade(), outcome) {
                    pane.reload();
                }
            },
        );
    }

    pub fn rename(w: &Window, pane: &Rc<Pane>, item: &Item, new: &str) {
        let (Location::Remote(loc), Item::Remote(node)) = (pane.location(), item) else { return };
        let (from, to, new) = (loc.entry_path(&node.name), loc.entry_path(new), new.to_string());
        let target = loc.at.clone();
        let pw = Rc::downgrade(pane);
        w.app.jobs.start(
            w.win.upcast_ref(),
            format!("Renaming “{}”", node.name),
            move |_ctx| remote_fs::rename(&target, &from, &to).map(|_| Vec::new()),
            move |outcome| {
                if let (Some(pane), Outcome::Done(_)) = (pw.upgrade(), outcome) {
                    pane.reload();
                    pane.select_when_present(vec![new.into()]);
                }
            },
        );
    }

    /// Renames onto a name the pane shows: what is in the way is removed first (the ask
    /// already happened in `fileops::rename`).
    pub fn rename_replacing(w: &Window, pane: &Rc<Pane>, item: &Item, new: &str) {
        let (Location::Remote(loc), Item::Remote(node)) = (pane.location(), item) else { return };
        let (from, to, new) = (loc.entry_path(&node.name), loc.entry_path(new), new.to_string());
        let (target, new_in_job) = (loc.at.clone(), new.clone());
        let pw = Rc::downgrade(pane);
        w.app.jobs.start(
            w.win.upcast_ref(),
            format!("Renaming “{}”", node.name),
            move |_ctx| {
                let folder = to.rfind('/').map(|i| to[..i].to_string()).unwrap_or_default();
                if let Some(e) = remote_fs::list(&target, &folder).unwrap_or_default().into_iter().find(|e| e.name == new_in_job) {
                    remote_fs::remove(&target, &to, e.is_dir)?;
                }
                remote_fs::rename(&target, &from, &to).map(|_| Vec::new())
            },
            move |outcome| {
                if let (Some(pane), Outcome::Done(_)) = (pw.upgrade(), outcome) {
                    pane.reload();
                    pane.select_when_present(vec![new.into()]);
                }
            },
        );
    }

    pub fn create(w: &Window, loc: &Location, name: &str, folder: bool) {
        let Location::Remote(r) = loc.clone() else { return };
        let (target, path, name2) = (r.at.clone(), r.entry_path(name), name.to_string());
        let shown_on = target.clone();
        let weak = Rc::downgrade(&w.me());
        w.app.jobs.start(
            w.win.upcast_ref(),
            format!("Creating “{name}” on the server"),
            move |_ctx| {
                if folder {
                    remote_fs::mkdir(&target, &path)?;
                } else {
                    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
                    let file = std::env::temp_dir().join(format!("fb-empty-{stamp}"));
                    std::fs::write(&file, b"")?;
                    let r = remote_fs::write_from(&target, &file, &path);
                    let _ = std::fs::remove_file(&file);
                    r?;
                }
                Ok(Vec::new())
            },
            move |outcome| {
                let Some(w) = weak.upgrade() else { return };
                for p in w.panes() {
                    if let Location::Remote(x) = p.location() {
                        if x.at == shown_on {
                            p.reload();
                        }
                    }
                }
                if matches!(outcome, Outcome::Done(_)) {
                    w.current_pane().select_when_present(vec![name2.into()]);
                }
            },
        );
    }
}
