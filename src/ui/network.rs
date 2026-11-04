//! The Network page: the computers sharing files on the local network, and on a double click
//! the folders one of them shares. Entering a folder mounts it through GVfs, which asks for a
//! user name and password when the server wants them. Computers and folders can be pinned to
//! the side panel and linked on the desktop.

use super::pane::Pane;
use super::util;
use super::window::Window;
use failbrauwser::location::Location;
use failbrauwser::network::{self, Computer, Probe, Share};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

/// A row of the page.
#[derive(Clone, Debug)]
enum Entry {
    /// A computer and what asking it for its folders found.
    Computer(Computer, Probe),
    /// A folder shared by the computer at this address.
    Share(String, Share),
    /// A server added under a name (SFTP, WebDAV, FTP, SMB).
    Server(failbrauwser::servers::Server),
}

impl Entry {
    fn title(&self) -> String {
        match self {
            Entry::Computer(c, _) => c.name.clone(),
            Entry::Share(_, s) => s.name.clone(),
            Entry::Server(s) => s.name.clone(),
        }
    }

    fn subtitle(&self) -> String {
        match self {
            // Its folders, and the address where it says more than the name.
            Entry::Computer(c, probe) => {
                let address = match (&c.ip, c.address.eq_ignore_ascii_case(&c.name)) {
                    (Some(ip), true) => Some(ip.clone()),
                    (Some(ip), false) => Some(format!("{} — {ip}", c.address)),
                    (None, false) => Some(c.address.clone()),
                    (None, true) => None,
                };
                let what = match probe {
                    Probe::Shares(s) if !s.is_empty() => s.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "),
                    Probe::Locked => "Folders behind a password".into(),
                    _ => "Shares files".into(),
                };
                match address {
                    Some(a) => format!("{what} — {a}"),
                    None => what,
                }
            }
            Entry::Share(_, s) if !s.comment.is_empty() => s.comment.clone(),
            Entry::Share(..) => "Shared folder".into(),
            Entry::Server(s) => failbrauwser::servers::describe(&s.uri),
        }
    }

    /// Its address: `network:///nas` for a computer, `smb://nas/Music` for a folder.
    fn uri(&self) -> String {
        match self {
            Entry::Computer(c, _) => Location::Network(c.address.clone()).display(),
            Entry::Share(host, s) => network::share_uri(host, &s.name),
            Entry::Server(s) => s.uri.clone(),
        }
    }
}

struct NetworkPage {
    root: gtk::Box,
    list: gtk::ListBox,
    heading: gtk::Label,
    status: gtk::Label,
    spinner: gtk::Spinner,
    pane: Weak<Pane>,
    window: Weak<Window>,
    entries: RefCell<Vec<Entry>>,
    /// Bumped on every refresh: answers for an older one are dropped.
    generation: Cell<u64>,
    weak: RefCell<Weak<NetworkPage>>,
}

/// A computer in the self-test: its folders are local folders, maybe behind a password.
struct FakeHost {
    name: String,
    shares: Vec<(String, PathBuf)>,
    password: Option<String>,
}

thread_local! {
    static PAGES: RefCell<Vec<Rc<NetworkPage>>> = const { RefCell::new(Vec::new()) };
    /// The computers with folders found last, and when.
    static FOUND: RefCell<Option<(Instant, Vec<(Computer, Probe)>)>> = const { RefCell::new(None) };
    static FAKES: RefCell<Vec<FakeHost>> = const { RefCell::new(Vec::new()) };
}

/// How long a search of the network is good for before the page searches again.
const FRESH: Duration = Duration::from_secs(60);

fn page_for(pane: &Rc<Pane>) -> Option<Rc<NetworkPage>> {
    PAGES.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|x| x.pane.strong_count() > 0);
        p.iter().find(|x| x.pane.upgrade().is_some_and(|q| Rc::ptr_eq(&q, pane))).cloned()
    })
}

pub fn attach(w: &Window, pane: &Rc<Pane>) {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    // Like folders: a double click (or Enter) opens.
    list.set_activate_on_single_click(false);
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    scroll.add(&list);
    let heading = gtk::Label::new(None);
    heading.set_xalign(0.0);
    heading.set_hexpand(true);
    heading.style_context().add_class("dim-label");
    let spinner = gtk::Spinner::new();
    let reload = gtk::Button::from_icon_name(Some("view-refresh-symbolic"), gtk::IconSize::Button);
    reload.set_tooltip_text(Some("Search again"));
    reload.set_relief(gtk::ReliefStyle::None);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.set_margin_start(12);
    bar.set_margin_end(6);
    bar.set_margin_top(6);
    bar.set_margin_bottom(6);
    let add = gtk::Button::with_mnemonic("_Add Server…");
    add.set_tooltip_text(Some("An SFTP, WebDAV, FTP or Windows server, kept here under a name"));
    add.set_relief(gtk::ReliefStyle::None);
    bar.pack_start(&heading, true, true, 0);
    bar.pack_start(&spinner, false, false, 0);
    bar.pack_start(&add, false, false, 0);
    bar.pack_start(&reload, false, false, 0);
    let status = gtk::Label::new(None);
    status.set_line_wrap(true);
    status.set_xalign(0.0);
    status.set_margin_start(12);
    status.set_margin_end(12);
    status.set_margin_bottom(12);
    status.style_context().add_class("dim-label");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.pack_start(&bar, false, false, 0);
    root.pack_start(&scroll, true, true, 0);
    root.pack_start(&status, false, false, 0);
    root.show_all();
    status.set_visible(false);
    pane.stack.add_named(&root, "network");
    let page = Rc::new(NetworkPage {
        root,
        list,
        heading,
        status,
        spinner,
        pane: Rc::downgrade(pane),
        window: Rc::downgrade(&w.me()),
        entries: RefCell::new(Vec::new()),
        generation: Cell::new(0),
        weak: RefCell::new(Weak::new()),
    });
    *page.weak.borrow_mut() = Rc::downgrade(&page);
    let weak = Rc::downgrade(&page);
    add.connect_clicked(move |_| {
        if let Some(p) = weak.upgrade() {
            p.server_dialog(None);
        }
    });
    let weak = Rc::downgrade(&page);
    reload.connect_clicked(move |_| {
        if let Some(p) = weak.upgrade() {
            p.refresh(true);
        }
    });
    let weak = Rc::downgrade(&page);
    page.list.connect_row_activated(move |_, row| {
        let Some(p) = weak.upgrade() else { return };
        // Opening lists the next page: the rows must not stay borrowed.
        let e = p.entries.borrow().get(row.index() as usize).cloned();
        if let Some(e) = e {
            p.open(&e, false);
        }
    });
    let weak = Rc::downgrade(&page);
    page.list.connect_button_press_event(move |list, ev| {
        let Some(p) = weak.upgrade() else { return glib::Propagation::Proceed };
        if ev.event_type() != gdk::EventType::ButtonPress {
            return glib::Propagation::Proceed;
        }
        let Some(row) = list.row_at_y(ev.position().1 as i32) else { return glib::Propagation::Proceed };
        let Some(e) = p.entries.borrow().get(row.index() as usize).cloned() else { return glib::Propagation::Proceed };
        match ev.button() {
            3 => {
                list.select_row(Some(&row));
                p.menu(&e).popup_at_pointer(Some(ev));
                glib::Propagation::Stop
            }
            2 => {
                p.open(&e, true);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    PAGES.with(|p| p.borrow_mut().push(page));
}

/// Shows or hides the page as the pane's location changes.
pub fn location_changed(pane: &Rc<Pane>) {
    let Some(page) = page_for(pane) else { return };
    if matches!(pane.location(), Location::Network(_)) {
        pane.stack.set_visible_child(&page.root);
        page.refresh(false);
        page.list.grab_focus();
    } else if pane.stack.visible_child().as_ref() == Some(page.root.upcast_ref()) {
        pane.apply_view_mode();
    }
}

/// The status bar text while the page is shown.
pub fn status_text(pane: &Rc<Pane>) -> String {
    let Some(page) = page_for(pane) else { return String::new() };
    let entries = page.entries.borrow();
    let n = entries.len();
    match (pane.location(), n) {
        (Location::Network(h), 1) if h.is_empty() => "1 computer".into(),
        (Location::Network(h), n) if h.is_empty() => format!("{n} computers"),
        (_, 1) => "1 shared folder".into(),
        (_, n) => format!("{n} shared folders"),
    }
}

impl NetworkPage {
    fn context(&self) -> Option<(Rc<Window>, Rc<Pane>)> {
        Some((self.window.upgrade()?, self.pane.upgrade()?))
    }

    fn host(&self) -> Option<String> {
        match self.pane.upgrade()?.location() {
            Location::Network(h) => Some(h),
            _ => None,
        }
    }

    fn show(&self, entries: Vec<Entry>) {
        for c in self.list.children() {
            self.list.remove(&c);
        }
        for e in &entries {
            self.list.add(&row(e));
        }
        self.list.show_all();
        *self.entries.borrow_mut() = entries;
        if let Some(w) = self.window.upgrade() {
            w.update_status();
        }
    }

    fn say(&self, text: &str) {
        self.status.set_text(text);
        self.status.set_visible(!text.is_empty());
    }

    fn busy(&self, on: bool) {
        if on {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
    }

    /// Lists the computers, or the shared folders of the one shown; `again` searches the
    /// network even when the last search is recent.
    fn refresh(self: &Rc<Self>, again: bool) {
        let Some(host) = self.host() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.say("");
        if host.is_empty() {
            self.heading.set_text("Computers sharing files on this network");
            self.refresh_computers(generation, again);
        } else {
            self.heading.set_text(&format!("Folders shared by {}", network::short_name(&host)));
            self.show(Vec::new());
            self.refresh_shares(generation, host);
        }
    }

    fn refresh_computers(self: &Rc<Self>, generation: u64, again: bool) {
        let history = super::remote::history();
        let fakes: Vec<(String, String)> = FAKES.with(|f| f.borrow().iter().map(|h| (h.name.clone(), h.name.clone())).collect());
        let cached = FOUND.with(|f| f.borrow().clone());
        // What was found last shows at once; a new search replaces it.
        let known = cached.as_ref().map(|(_, list)| list.clone()).unwrap_or_default();
        self.show(with_servers(known.iter().cloned().map(|(c, p)| Entry::Computer(c, p)).collect()));
        let fresh = cached.as_ref().is_some_and(|(at, _)| at.elapsed() < FRESH);
        if fresh && !again {
            self.after_search(&known);
            return;
        }
        self.busy(true);
        if known.is_empty() {
            self.say("Searching for computers that share folders…");
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            // The self-test does not knock on the doors of the network it runs in.
            let all = if super::selftest::active() {
                network::merge(&[], &fakes, &history)
            } else {
                let scan = gio::spawn_blocking(failbrauwser::remote::find_smb_servers);
                let announced = announced().await;
                let scanned = scan.await.unwrap_or_default();
                network::merge(&scanned, &[announced, fakes].concat(), &history)
            };
            // Each is asked for its folders at once; only those with some are listed.
            let probes: Vec<_> = all.iter().map(|c| probe(&c.address)).collect();
            let mut found = Vec::new();
            for (c, p) in all.into_iter().zip(probes) {
                let p = p.await;
                if p.worth_showing() {
                    found.push((c, p));
                }
            }
            FOUND.with(|f| *f.borrow_mut() = Some((Instant::now(), found.clone())));
            let Some(p) = weak.upgrade() else { return };
            if p.generation.get() != generation {
                return;
            }
            p.busy(false);
            p.say("");
            p.show(with_servers(found.iter().cloned().map(|(c, p)| Entry::Computer(c, p)).collect()));
            p.after_search(&found);
        });
    }

    fn after_search(&self, found: &[(Computer, Probe)]) {
        if found.is_empty() {
            self.say("No computer sharing folders answered. A NAS or a computer that shares folders (SMB, Windows file sharing) shows up here; one on another network can be opened by typing its address in the location bar, like smb://192.168.1.10/share.");
        }
    }

    fn refresh_shares(self: &Rc<Self>, generation: u64, host: String) {
        if let Some(shares) = FAKES.with(|f| f.borrow().iter().find(|h| h.name == host).map(|h| h.shares.iter().map(|(n, _)| Share { name: n.clone(), comment: String::new() }).collect::<Vec<_>>())) {
            self.show(shares.into_iter().map(|s| Entry::Share(host.clone(), s)).collect());
            return;
        }
        let Some((w, _)) = self.context() else { return };
        self.busy(true);
        let weak = Rc::downgrade(self);
        let parent: gtk::Window = w.win.clone().upcast();
        glib::spawn_future_local(async move {
            let res = list_shares(&parent, &host).await;
            let Some(p) = weak.upgrade() else { return };
            if p.generation.get() != generation {
                return;
            }
            p.busy(false);
            match res {
                Ok(shares) => {
                    if shares.is_empty() {
                        p.say(&format!("{} shares no folders, or none this user may see.", network::short_name(&host)));
                    }
                    p.show(shares.into_iter().map(|s| Entry::Share(host.clone(), s)).collect());
                }
                Err(e) if e.is_empty() => p.say("Cancelled. Search again to sign in."),
                Err(e) => p.say(&format!("Cannot list the shared folders of {}: {e}", network::short_name(&host))),
            }
        });
    }

    /// Opens a computer (its folders) or a shared folder (mounted first).
    fn open(&self, e: &Entry, new_tab: bool) {
        let Some((w, pane)) = self.context() else { return };
        match e {
            Entry::Computer(c, _) => {
                let loc = Location::Network(c.address.clone());
                if new_tab {
                    w.add_tab(loc, true);
                } else {
                    pane.navigate(loc);
                }
            }
            Entry::Server(s) => super::shares::open_remote(&w, &s.uri, new_tab),
            Entry::Share(host, s) => {
                if let Some((dir, password)) = FAKES.with(|f| f.borrow().iter().find(|h| h.name == *host).and_then(|h| Some((h.shares.iter().find(|(n, _)| *n == s.name)?.1.clone(), h.password.clone())))) {
                    open_fake(&w, &pane, dir, password, new_tab);
                    return;
                }
                super::shares::open_remote(&w, &e.uri(), new_tab);
            }
        }
    }

    fn menu(self: &Rc<Self>, e: &Entry) -> gtk::Menu {
        let menu = gtk::Menu::new();
        let add = |label: &str, f: Box<dyn Fn(&Rc<NetworkPage>)>| {
            let item = gtk::MenuItem::with_mnemonic(label);
            let weak = Rc::downgrade(self);
            item.connect_activate(move |_| {
                if let Some(p) = weak.upgrade() {
                    f(&p);
                }
            });
            menu.append(&item);
        };
        let e2 = e.clone();
        add("_Open", Box::new(move |p| p.open(&e2, false)));
        let e2 = e.clone();
        add("Open in New _Tab", Box::new(move |p| p.open(&e2, true)));
        menu.append(&gtk::SeparatorMenuItem::new());
        let uri = e.uri();
        add("_Pin to Side Panel", Box::new(move |_| super::sidebar::pin(&uri)));
        let (uri, title) = (e.uri(), e.title());
        let is_share = matches!(e, Entry::Share(..));
        add("Create _Link on Desktop", Box::new(move |p| {
            let Some((w, _)) = p.context() else { return };
            let link = if is_share {
                super::desktoplink::for_share(&network::uri_name(&uri), &uri)
            } else {
                super::desktoplink::for_network_page(&title, &Location::Network(uri.trim_start_matches(failbrauwser::location::NETWORK_URI).to_string()))
            };
            super::desktoplink::create(&w, std::slice::from_ref(&link));
        }));
        let uri = e.uri();
        add("_Copy Address", Box::new(move |_| {
            gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&uri);
        }));
        if let Entry::Server(server) = e {
            menu.append(&gtk::SeparatorMenuItem::new());
            let s2 = server.clone();
            add("_Edit…", Box::new(move |p| p.server_dialog(Some(s2.clone()))));
            let s2 = server.clone();
            add("_Remove", Box::new(move |p| {
                let mut list = failbrauwser::servers::load();
                list.retain(|x| *x != s2);
                p.save_servers(&list);
            }));
        }
        menu.show_all();
        menu
    }
}

fn row(e: &Entry) -> gtk::ListBoxRow {
    let icon = match e {
        Entry::Computer(..) => ["network-server", "computer"],
        Entry::Share(..) => ["folder-remote", "folder"],
        Entry::Server(_) => ["network-server", "folder-remote"],
    };
    let image = gtk::Image::from_gicon(&gio::ThemedIcon::from_names(&icon), gtk::IconSize::Dnd);
    let title = gtk::Label::new(Some(&e.title()));
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let sub = gtk::Label::new(Some(&e.subtitle()));
    sub.set_xalign(0.0);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub.style_context().add_class("dim-label");
    let lines = gtk::Box::new(gtk::Orientation::Vertical, 2);
    lines.set_valign(gtk::Align::Center);
    lines.pack_start(&title, false, false, 0);
    lines.pack_start(&sub, false, false, 0);
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(6);
    content.set_margin_bottom(6);
    content.pack_start(&image, false, false, 0);
    content.pack_start(&lines, true, true, 0);
    let row = gtk::ListBoxRow::new();
    row.set_tooltip_text(Some(&format!("{}\nDouble-click to open, right-click to pin it to the side panel", e.uri())));
    row.add(&content);
    row
}

/// The rows of the computers, with the servers added by hand after them.
fn with_servers(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.extend(failbrauwser::servers::load().into_iter().map(Entry::Server));
    entries
}

impl NetworkPage {
    fn save_servers(&self, list: &[failbrauwser::servers::Server]) {
        let Some((w, _)) = self.context() else { return };
        if let Err(e) = failbrauwser::servers::save(list) {
            util::show_error(&w.win, "Cannot save the servers", &e.to_string());
        }
        // Every Network page lists them (taken out first: showing asks for the pages too).
        let pages: Vec<Rc<NetworkPage>> = PAGES.with(|p| p.borrow().clone());
        for page in pages {
            if page.host().is_some_and(|h| h.is_empty()) {
                let rows: Vec<Entry> = page.entries.borrow().iter().filter(|e| matches!(e, Entry::Computer(..))).cloned().collect();
                page.show(with_servers(rows));
            }
        }
    }

    /// Adds a server, or changes one (`existing`).
    fn server_dialog(self: &Rc<Self>, existing: Option<failbrauwser::servers::Server>) {
        use failbrauwser::servers::{self, KINDS, Parts};
        let Some((w, _)) = self.context() else { return };
        let parts = existing.as_ref().and_then(|s| servers::parse_uri(&s.uri)).unwrap_or(Parts { scheme: "sftp".into(), ..Default::default() });
        let d = gtk::MessageDialog::new(
            Some(&w.win),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Other,
            gtk::ButtonsType::None,
            if existing.is_some() { "Edit Server" } else { "Add Server" },
        );
        d.add_button("_Cancel", gtk::ResponseType::Cancel);
        d.add_button(if existing.is_some() { "_Save" } else { "_Add" }, gtk::ResponseType::Ok);
        d.set_default_response(gtk::ResponseType::Ok);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_spacing(8);
        let entry = |name: &str, text: &str, hint: &str| {
            let e = gtk::Entry::new();
            e.set_widget_name(name);
            e.set_text(text);
            e.set_placeholder_text(Some(hint));
            e.set_hexpand(true);
            e.set_activates_default(true);
            e
        };
        let name = entry("server-name", existing.as_ref().map_or("", |s| s.name.as_str()), "Raspberry Pi");
        let kind = gtk::ComboBoxText::new();
        kind.set_widget_name("server-kind");
        for (id, text, _) in KINDS {
            kind.append(Some(id), text);
        }
        kind.set_active_id(Some(&parts.scheme));
        let host = entry("server-host", &parts.host, "pi.local or 192.168.1.20");
        let port = gtk::SpinButton::with_range(0.0, 65535.0, 1.0);
        port.set_widget_name("server-port");
        port.set_value(parts.port as f64);
        port.set_tooltip_text(Some("0: the usual port"));
        let user = entry("server-user", &parts.user, "optional");
        let folder = entry("server-folder", &parts.folder, "optional");
        let address = gtk::Label::new(None);
        address.set_xalign(0.0);
        address.set_selectable(true);
        address.style_context().add_class("dim-label");
        let rows: [(&str, gtk::Widget); 7] = [
            ("_Name:", name.clone().upcast()),
            ("_Type:", kind.clone().upcast()),
            ("_Server:", host.clone().upcast()),
            ("_Port:", port.clone().upcast()),
            ("_User:", user.clone().upcast()),
            ("_Folder:", folder.clone().upcast()),
            ("Address:", address.clone().upcast()),
        ];
        for (y, (text, widget)) in rows.into_iter().enumerate() {
            let l = gtk::Label::with_mnemonic(text);
            l.set_xalign(1.0);
            l.set_mnemonic_widget(Some(&widget));
            grid.attach(&l, 0, y as i32, 1, 1);
            grid.attach(&widget, 1, y as i32, 1, 1);
        }
        if let Ok(area) = d.message_area().downcast::<gtk::Box>() {
            area.pack_start(&grid, false, false, 0);
        }
        let current = {
            let (kind, host, port, user, folder) = (kind.clone(), host.clone(), port.clone(), user.clone(), folder.clone());
            move || Parts {
                scheme: kind.active_id().map(|s| s.to_string()).unwrap_or_else(|| "sftp".into()),
                host: host.text().trim().to_string(),
                port: port.value() as u16,
                user: user.text().trim().to_string(),
                folder: folder.text().to_string(),
            }
        };
        let update = {
            let (current, address, d, name) = (current.clone(), address.clone(), d.clone(), name.clone());
            move || {
                let p = current();
                let ok = !p.host.is_empty() && !name.text().trim().is_empty();
                address.set_text(&if p.host.is_empty() { String::new() } else { servers::build_uri(&p) });
                d.set_response_sensitive(gtk::ResponseType::Ok, ok);
            }
        };
        for e in [&name, &host, &user, &folder] {
            let u = update.clone();
            e.connect_changed(move |_| u());
        }
        let u = update.clone();
        kind.connect_changed(move |_| u());
        let u = update.clone();
        port.connect_value_changed(move |_| u());
        update();
        let weak = Rc::downgrade(self);
        let answered = Cell::new(false);
        let name2 = name.clone();
        d.connect_response(move |d, resp| {
            if answered.replace(true) {
                return;
            }
            let (server_name, parts) = (name2.text().trim().to_string(), current());
            // Not close(): see util::ask_text.
            unsafe { d.destroy() };
            let (gtk::ResponseType::Ok, Some(p)) = (resp, weak.upgrade()) else { return };
            let server = servers::Server { name: server_name, uri: servers::build_uri(&parts) };
            let mut list = servers::load();
            match existing.as_ref().and_then(|e| list.iter().position(|x| x == e)) {
                Some(i) => list[i] = server,
                None => list.push(server),
            }
            p.save_servers(&list);
        });
        d.show_all();
        name.grab_focus();
    }
}

/// Computers the workgroup/domain browse announces (the master browser's list), as
/// (name, name): the Windows twin of GVfs's network browsing.
#[cfg(windows)]
async fn announced() -> Vec<(String, String)> {
    let list = async {
        gio::spawn_blocking(failbrauwser::win::wnet::browse_network)
            .await
            .map(std::result::Result::unwrap_or_default)
            .unwrap_or_default()
            .into_iter()
            .map(|n| (n.clone(), n))
            .collect()
    };
    // A silent master browser must not hold the page: the await is cut off, the worker
    // ends on the redirector's own timeout.
    glib::future_with_timeout(Duration::from_secs(6), list).await.ok().unwrap_or_default()
}

/// Computers GVfs's network browsing announces (DNS-SD, WS-Discovery), as (name, address).
#[cfg(not(windows))]
async fn announced() -> Vec<(String, String)> {
    let dir = gio::File::for_uri("network:///");
    let list = async {
        let e = dir.enumerate_children_future("standard::display-name,standard::target-uri", gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await.ok()?;
        let mut out = Vec::new();
        loop {
            let infos = e.next_files_future(32, glib::Priority::DEFAULT).await.ok()?;
            if infos.is_empty() {
                break;
            }
            for i in infos {
                let target = i.attribute_string("standard::target-uri").map(|s| s.to_string()).unwrap_or_default();
                if let Some(host) = network::announced_host(&target) {
                    out.push((i.display_name().to_string(), host));
                }
            }
        }
        Some(out)
    };
    glib::future_with_timeout(Duration::from_secs(4), list).await.ok().flatten().unwrap_or_default()
}

/// The folders a computer shares, through the redirector's own browsing. When it wants
/// credentials the user is asked for a name and password, the session is signed in, and the
/// answer is read again. An empty error: the user cancelled.
#[cfg(windows)]
async fn list_shares(parent: &gtk::Window, host: &str) -> Result<Vec<Share>, String> {
    let answer = |host: &str| {
        let h = host.to_string();
        async move { gio::spawn_blocking(move || network::probe_shares(&h, Duration::from_secs(6))).await.unwrap_or(Probe::Unknown) }
    };
    match answer(host).await {
        Probe::Shares(shares) => Ok(shares),
        Probe::Nothing | Probe::Unknown => Err(format!("“{host}” did not answer as a file server.")),
        Probe::Locked => {
            let (tx, rx) = async_channel::unbounded();
            let guess = std::env::var("USERNAME").unwrap_or_default();
            util::ask_text(parent, "Sign In", &format!("User name for “{host}”:"), &guess, "Continue", false, move |u| {
                let _ = tx.send_blocking(u);
            });
            let Ok(user) = rx.recv().await else { return Err(String::new()) };
            let (tx, rx) = async_channel::unbounded();
            util::ask_secret(parent, "Sign In", &format!("Password for {user}@{host}:"), move |p| {
                let _ = tx.send_blocking(p);
            });
            let Ok(pass) = rx.recv().await else { return Err(String::new()) };
            if let Err(e) = failbrauwser::win::wnet::connect(&format!("\\\\{host}"), Some(user.as_str()), Some(pass.as_str())) {
                return Err(e.to_string());
            }
            match answer(host).await {
                Probe::Shares(shares) => Ok(shares),
                Probe::Locked => Err("The server refused these credentials.".into()),
                Probe::Nothing | Probe::Unknown => Err(format!("“{host}” did not answer after signing in.")),
            }
        }
    }
}

/// The folders a computer shares, through GVfs (asking for a password if the server wants
/// one), else through `smbclient` as a guest. An empty error: the user cancelled.
#[cfg(not(windows))]
async fn list_shares(parent: &gtk::Window, host: &str) -> Result<Vec<Share>, String> {
    let dir = gio::File::for_uri(&format!("smb://{host}/"));
    let mut first_error = None;
    for attempt in 0..2 {
        match enumerate(&dir).await {
            Ok(v) => return Ok(v),
            Err(e) if attempt == 0 && e.matches(gio::IOErrorEnum::NotMounted) => {
                let op = gtk::MountOperation::new(Some(parent));
                if let Err(e) = dir.mount_enclosing_volume_future(gio::MountMountFlags::NONE, Some(&op)).await {
                    if e.matches(gio::IOErrorEnum::FailedHandled) {
                        return Err(String::new());
                    }
                    if !e.matches(gio::IOErrorEnum::AlreadyMounted) {
                        first_error = Some(e.message().to_string());
                        break;
                    }
                }
            }
            Err(e) => {
                first_error = Some(e.message().to_string());
                break;
            }
        }
    }
    // Without GVfs's SMB browsing: ask as a guest.
    let host = host.to_string();
    let guest = gio::spawn_blocking(move || {
        let out = std::process::Command::new("smbclient").args(["-g", "-N", "-L", &format!("//{host}")]).output().ok()?;
        Some(network::parse_smbclient(&String::from_utf8_lossy(&out.stdout)))
    })
    .await
    .ok()
    .flatten()
    .filter(|v| !v.is_empty());
    guest.ok_or_else(|| first_error.unwrap_or_else(|| "no answer".into()))
}

#[cfg(not(windows))] // only GVfs-backed listings enumerate gio children here
async fn enumerate(dir: &gio::File) -> Result<Vec<Share>, glib::Error> {
    let e = dir.enumerate_children_future("standard::display-name,standard::name", gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await?;
    let mut out = Vec::new();
    loop {
        let infos = e.next_files_future(64, glib::Priority::DEFAULT).await?;
        if infos.is_empty() {
            break;
        }
        for i in infos {
            let name = i.display_name().to_string();
            if network::visible_share(&name) {
                out.push(Share { name, comment: String::new() });
            }
        }
    }
    out.sort_by(|a, b| failbrauwser::fs::format::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase()));
    Ok(out)
}

/// Asks a computer for its folders as a guest, on a worker thread. A stand-in computer of the
/// self-test answers with its folders; others there are not asked.
fn probe(address: &str) -> impl std::future::Future<Output = Probe> + use<> {
    let fake = FAKES.with(|f| {
        f.borrow().iter().find(|h| h.name == address).map(|h| Probe::Shares(h.shares.iter().map(|(n, _)| Share { name: n.clone(), comment: String::new() }).collect()))
    });
    let fake = fake.or_else(|| super::selftest::active().then_some(Probe::Unknown));
    // Started here, so that all computers are asked at the same time.
    // A thread each (not GLib's few pool threads): the slow ones must not hold up the rest.
    let asking = fake.is_none().then(|| {
        let (tx, rx) = async_channel::bounded(1);
        let address = address.to_string();
        std::thread::spawn(move || {
            let _ = tx.send_blocking(network::probe_shares(&address, Duration::from_secs(4)));
        });
        rx
    });
    async move {
        match (fake, asking) {
            (Some(p), _) => p,
            (None, Some(rx)) => rx.recv().await.unwrap_or(Probe::Unknown),
            (None, None) => Probe::Unknown,
        }
    }
}

/// Opens a network share by its address (a pinned one): mounted through GVfs.
pub fn open_share(w: &Rc<Window>, uri: &str, new_tab: bool) {
    let fake = FAKES.with(|f| {
        f.borrow().iter().find_map(|h| h.shares.iter().find(|(n, _)| network::share_uri(&h.name, n) == uri).map(|(_, d)| (d.clone(), h.password.clone())))
    });
    match fake {
        Some((dir, password)) => open_fake(w, &w.current_pane(), dir, password, new_tab),
        None => super::shares::open_remote(w, uri, new_tab),
    }
}

/// A computer for the self-test: `shares` are (name, folder) pairs.
pub fn add_fake(name: &str, shares: Vec<(String, PathBuf)>, password: Option<String>) {
    FAKES.with(|f| f.borrow_mut().push(FakeHost { name: name.into(), shares, password }));
}

fn open_fake(w: &Rc<Window>, pane: &Rc<Pane>, dir: PathBuf, password: Option<String>, new_tab: bool) {
    let (ww, pw) = (Rc::downgrade(w), Rc::downgrade(pane));
    let go = move || {
        let loc = Location::Dir(dir.clone());
        if new_tab {
            if let Some(w) = ww.upgrade() {
                w.add_tab(loc, true);
            }
        } else if let Some(p) = pw.upgrade() {
            p.navigate(loc);
        }
    };
    match password {
        None => go(),
        Some(want) => {
            let parent = w.win.clone();
            util::ask_secret(&w.win, "Password Required", "Password for the shared folder:", move |given| {
                if given == want {
                    go();
                } else {
                    util::show_error(&parent, "Wrong password", "The shared folder stays closed.");
                }
            });
        }
    }
}

/// The titles of the page's rows (tests).
pub fn row_titles(pane: &Rc<Pane>) -> Vec<String> {
    page_for(pane).map(|p| p.entries.borrow().iter().map(Entry::title).collect()).unwrap_or_default()
}

/// The second line of a row (tests).
pub fn row_subtitle(pane: &Rc<Pane>, title: &str) -> Option<String> {
    page_for(pane)?.entries.borrow().iter().find(|e| e.title() == title).map(Entry::subtitle)
}

/// Opens the Add Server dialog (tests).
pub fn add_server(pane: &Rc<Pane>) -> Result<(), String> {
    page_for(pane).ok_or("no network page")?.server_dialog(None);
    Ok(())
}

/// Opens a row by its title as a double click would (tests).
pub fn activate(pane: &Rc<Pane>, title: &str) -> Result<(), String> {
    let page = page_for(pane).ok_or("no network page")?;
    let i = page.entries.borrow().iter().position(|e| e.title() == title).ok_or_else(|| format!("no row {title:?}"))?;
    let row = page.list.row_at_index(i as i32).ok_or("row missing")?;
    page.list.emit_by_name::<()>("row-activated", &[&row]);
    Ok(())
}

/// Picks an item of a row's right-click menu (tests).
pub fn menu_item(pane: &Rc<Pane>, title: &str, item: &str) -> Result<(), String> {
    let page = page_for(pane).ok_or("no network page")?;
    let e = page.entries.borrow().iter().find(|e| e.title() == title).cloned().ok_or_else(|| format!("no row {title:?}"))?;
    let menu = page.menu(&e);
    let found = menu
        .children()
        .into_iter()
        .filter_map(|c| c.downcast::<gtk::MenuItem>().ok())
        .find(|m| m.label().is_some_and(|l| l.replace('_', "") == item))
        .ok_or_else(|| format!("no menu item {item:?}"))?;
    found.activate();
    Ok(())
}
