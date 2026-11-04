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
    let unc = remote::unc_as_typed(t);
    if let Some(host) = remote::smb_host(unc.as_deref().unwrap_or(t)) {
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
