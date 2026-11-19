//! The Drives page: every volume with its fill level and mount point. Mounting, unmounting
//! and ejecting are asynchronous UDisks2 calls, so the window never freezes while a slow
//! USB stick writes back its cache. The side panel's drive list uses the same calls.

use super::pane::Pane;
use super::util;
use super::window::Window;
use failbrauwser::devices::{self, DeviceKind};
use failbrauwser::drives::{self, Usage, Volume};
use failbrauwser::fs::format::human_size;
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

const UDISKS: &str = "org.freedesktop.UDisks2";

pub struct DrivesPage {
    root: gtk::ScrolledWindow,
    list: gtk::ListBox,
    pane: Weak<Pane>,
    window: Weak<Window>,
    /// The page is on screen (it follows changes only then).
    shown: Cell<bool>,
    /// Volumes shown, in row order, with their fill levels.
    volumes: RefCell<Vec<Volume>>,
    usages: RefCell<Vec<Option<Usage>>>,
    /// The newest reading: an older one that answers late is not shown.
    generation: Cell<u64>,
    weak: RefCell<Weak<DrivesPage>>,
}

thread_local! {
    static PAGES: RefCell<Vec<Rc<DrivesPage>>> = const { RefCell::new(Vec::new()) };
    /// Volumes with an operation running (their buttons are disabled), by [`busy_key`].
    static BUSY: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static LISTENERS: RefCell<Vec<Box<dyn Fn() -> bool>>> = const { RefCell::new(Vec::new()) };
    /// Change notifications from UDisks2 (dropped = unsubscribed).
    static SUBSCRIPTION: RefCell<Option<gio::SignalSubscription>> = const { RefCell::new(None) };
    static PENDING: Cell<bool> = const { Cell::new(false) };
    /// Windows: the drive-letter poll is already installed (see [`connect_changed`]).
    static POLLING: Cell<bool> = const { Cell::new(false) };
    /// Stand-ins for USB sticks in the self-test, answering in place of UDisks2.
    static FAKES: RefCell<Vec<FakeDrive>> = const { RefCell::new(Vec::new()) };
}

/// A stick in the self-test: a folder it "mounts" on, maybe a passphrase it is locked with.
struct FakeDrive {
    label: String,
    dir: PathBuf,
    /// A disk image instead of a stick: it shows only while "mounted".
    image: Option<PathBuf>,
    passphrase: Option<String>,
    /// Encrypted, with LVM volumes inside instead of a file system.
    lvm: bool,
    unlocked: bool,
    mounted: bool,
    /// The file system the volume says it holds and the file systems it says this machine offers for it, as
    /// the scenario names them (`add_fake_volume`); a stand-in disk image names its own where it is built.
    fs: String,
    supported: Vec<String>,
}

impl FakeDrive {
    fn object(&self) -> String {
        format!("/fake/{}", self.label)
    }

    fn volumes(&self) -> Vec<Volume> {
        if let Some(image) = &self.image {
            return if self.mounted {
                vec![Volume {
                    device: format!("/dev/fake-loop-{}", self.label),
                    fs_type: "ntfs".into(),
                    size: 6_000_000,
                    mountable: true,
                    mount_points: vec![self.dir.clone()],
                    block_object: self.object(),
                    loop_object: format!("/fake-loop/{}", self.label),
                    backing_file: Some(image.clone()),
                    rotation_rate: -1,
                    ..Default::default()
                }]
            } else {
                Vec::new()
            };
        }
        let base = Volume {
            drive_name: "Fake Stick".into(),
            drive_object: format!("/fake-drive/{}", self.label),
            removable: true,
            usb: true,
            ejectable: true,
            bus: "usb".into(),
            size: 8_000_000_000,
            rotation_rate: -1,
            ..Default::default()
        };
        let contents = |object: String, backing: String| Volume {
            device: format!("/dev/fake-{}", self.label),
            label: self.label.clone(),
            fs_type: self.fs.clone(),
            supported_file_systems: self.supported.clone(),
            mountable: true,
            block_object: object,
            crypto_backing: backing,
            mount_points: if self.mounted { vec![self.dir.clone()] } else { Vec::new() },
            ..base.clone()
        };
        if self.passphrase.is_none() {
            return vec![contents(self.object(), String::new())];
        }
        let container = Volume { device: format!("/dev/fake-{}-luks", self.label), label: self.label.clone(), fs_type: "crypto_LUKS".into(), block_object: self.object(), unlocked: self.unlocked, lvm_inside: self.unlocked && self.lvm, ..base.clone() };
        if self.lvm {
            // The LVM volume shows once opened ("mounted").
            let lv = Volume { lvm_backing: self.object(), ..contents(format!("{}/lv", self.object()), String::new()) };
            return if self.unlocked && self.mounted { vec![container, lv] } else { vec![container] };
        }
        if self.unlocked { vec![container, contents(format!("{}/clear", self.object()), self.object())] } else { vec![container] }
    }
}

/// A stand-in USB stick (self-test), not mounted; with a passphrase it is encrypted and locked.
pub fn add_fake(label: &str, dir: PathBuf, passphrase: Option<String>, lvm: bool) {
    FAKES.with(|f| f.borrow_mut().push(FakeDrive { label: label.into(), dir, image: None, passphrase, lvm, unlocked: false, mounted: false, fs: "ext4".into(), supported: Vec::new() }));
    changed();
}

/// A stand-in disk image (self-test): mounting it shows `dir`.
pub fn add_fake_image(image: PathBuf, dir: PathBuf) {
    let label = image.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    FAKES.with(|f| f.borrow_mut().push(FakeDrive { label, dir, image: Some(image), passphrase: None, lvm: false, unlocked: false, mounted: false, fs: "ntfs".into(), supported: Vec::new() }));
}

/// A stand-in volume (self-test) whose file system and whose format offer are named by the scenario, so a
/// format window can be driven over a volume this machine does not hold. It is never mounted, so no act of
/// ours reaches a real volume through it: the act's own answer is what the scenario reads.
pub fn add_fake_volume(label: &str, dir: PathBuf, fs: &str, supported: Vec<String>) {
    FAKES.with(|f| f.borrow_mut().push(FakeDrive { label: label.into(), dir, image: None, passphrase: None, lvm: false, unlocked: false, mounted: false, fs: fs.into(), supported }));
    changed();
}

/// Answers a UDisks2 call on a stand-in stick like UDisks2 would.
fn fake_call(object: &str, method: &str, args: &glib::Variant) -> Result<glib::Variant, String> {
    let reply = FAKES.with(|f| {
        let mut f = f.borrow_mut();
        let d = f
            .iter_mut()
            .find(|d| object == d.object() || object == format!("{}/clear", d.object()) || object == format!("/fake-drive/{}", d.label) || object == format!("/fake-loop/{}", d.label))
            .ok_or("No such object")?;
        match method {
            "Delete" => {
                d.mounted = false;
                Ok(glib::Variant::tuple_from_iter(std::iter::empty::<glib::Variant>()))
            }
            "Mount" if d.mounted => Err(format!("Device is already mounted at `{}'.", d.dir.display())),
            "Mount" => {
                d.mounted = true;
                Ok(glib::Variant::tuple_from_iter([d.dir.display().to_string().to_variant()]))
            }
            "Unmount" => {
                d.mounted = false;
                Ok(glib::Variant::tuple_from_iter(std::iter::empty::<glib::Variant>()))
            }
            "Unlock" => {
                let given: String = args.child_value(0).get().unwrap_or_default();
                if d.passphrase.as_deref() != Some(given.as_str()) {
                    return Err("Error unlocking: Failed to activate device: Incorrect passphrase".to_string());
                }
                d.unlocked = true;
                Ok(glib::Variant::tuple_from_iter([format!("{}/clear", d.object()).to_variant()]))
            }
            "Get" => {
                // Properties.Get of the unlocked contents' IdType.
                let kind = if d.lvm { failbrauwser::lvm::PV_TYPE } else { "ext4" };
                Ok(glib::Variant::tuple_from_iter([glib::Variant::from_variant(&kind.to_variant())]))
            }
            "Lock" if d.lvm && d.mounted => Err("Error locking: Device is busy".to_string()),
            "Lock" => {
                d.unlocked = false;
                Ok(glib::Variant::tuple_from_iter(std::iter::empty::<glib::Variant>()))
            }
            _ => Ok(glib::Variant::tuple_from_iter(std::iter::empty::<glib::Variant>())),
        }
    });
    changed();
    reply
}

/// Calls `f` when drives or phones come, go, mount, unmount or get busy (a burst of changes
/// once, 300 ms later), until it returns false.
pub fn connect_changed(f: impl Fn() -> bool + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
    SUBSCRIPTION.with(|sub| {
        if sub.borrow().is_some() {
            return;
        }
        super::phones::watch();
        super::phones::connect_changed(|| {
            changed();
            true
        });
        #[cfg(windows)]
        {
            // Windows has no UDisks2 signals to follow: the drive-letter mask is polled
            // instead — one cheap call, so sticks coming and going move the panel.
            if !POLLING.with(|c| !c.replace(true)) {
                let seen = Rc::new(Cell::new(0u32));
                let _ = glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
                    let mask = failbrauwser::win::storage::drive_mask();
                    if seen.get() != 0 && mask != seen.get() {
                        changed();
                    }
                    seen.set(mask);
                    glib::ControlFlow::Continue
                });
            }
            return;
        }
        #[cfg(not(windows))]
        {
            let Ok(conn) = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) else { return };
            *sub.borrow_mut() = Some(conn.subscribe_to_signal(Some(UDISKS), None, None, None, None, gio::DBusSignalFlags::NONE, |_| changed()));
        }
    });
}

fn changed() {
    if PENDING.with(|p| p.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(std::time::Duration::from_millis(300), || {
        PENDING.with(|p| p.set(false));
        let listeners = LISTENERS.with(|l| std::mem::take(&mut *l.borrow_mut()));
        let kept: Vec<_> = listeners.into_iter().filter(|f| f()).collect();
        LISTENERS.with(|l| {
            let mut l = l.borrow_mut();
            let added = std::mem::take(&mut *l);
            *l = kept;
            l.extend(added);
        });
    });
}

/// Whether an operation runs on this volume.
pub fn is_busy(v: &Volume) -> bool {
    BUSY.with(|b| b.borrow().contains(&busy_key(v)))
}

fn set_busy(key: &str, on: bool) {
    BUSY.with(|b| {
        let mut b = b.borrow_mut();
        b.retain(|x| x != key);
        if on {
            b.push(key.to_string());
        }
    });
    changed();
}

/// Every volume, phone and network share, with its fill level, in two steps. `first` gets at
/// once what needs no other device: the computer's own drives with their fill levels, and the
/// network shares with the fill level they told last. What is returned adds the phones and the fill levels the shares tell now, asked at the
/// same time: an SMB server can take seconds, and the drives of this computer do not wait for it.
pub async fn gather_staged(first: impl FnOnce(Vec<(Volume, Option<Usage>)>)) -> Vec<(Volume, Option<Usage>)> {
    let ctx = glib::MainContext::default();
    let local = gather_local().await;
    let mut quick = local.clone();
    quick.extend(super::shares::volumes_known());
    first(quick);
    let phones = ctx.spawn_local(super::phones::volumes());
    let shares = ctx.spawn_local(super::shares::volumes());
    let mut all = local;
    // Phones next to the USB sticks (their fill levels asked of GVfs, not statvfs), and the
    // network shares.
    all.extend(phones.await.unwrap_or_default());
    all.extend(shares.await.unwrap_or_default());
    all
}

/// The computer's own drives (UDisks2, else the mount table), with their fill levels.
async fn gather_local() -> Vec<(Volume, Option<Usage>)> {
    #[cfg(windows)]
    let vols = gio::spawn_blocking(failbrauwser::win::storage::volumes).await.unwrap_or_default();
    #[cfg(not(windows))]
    let vols = match query_udisks().await {
        Some(v) => v,
        None => gio::spawn_blocking(drives::read_mountinfo).await.unwrap_or_default(),
    };
    // statvfs can hang on a dead network mount: never on the UI thread.
    let mut with_usage = gio::spawn_blocking(move || {
        vols.into_iter().map(|v| {
            let u = v.mount_point().and_then(|m| drives::usage(m));
            (v, u)
        }).collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    FAKES.with(|f| {
        for d in f.borrow().iter() {
            with_usage.extend(d.volumes().into_iter().map(|v| {
                let u = v.mount_point().and_then(|m| drives::usage(m));
                (v, u)
            }));
        }
    });
    // What a volume's own files say about it — the long name pinned there and the picture named there — is read
    // of its folder, off the UI thread like the fill levels: a root that does not answer is left to answer for
    // itself (README: *Device icons*). A share is not asked at all, because a dead one hangs a read of its root
    // and the picture of a share belongs to the machine that serves it.
    gio::spawn_blocking(move || {
        with_usage
            .into_iter()
            .map(|(mut v, u)| {
                if let (false, Some(root)) = (v.network, v.mount_point().cloned()) {
                    let said = failbrauwser::fsformat::icon::shown(&root);
                    v.pinned_name = said.pinned;
                    v.picture_file = said.picture;
                }
                (v, u)
            })
            .collect()
    })
    .await
    .unwrap_or_else(|_| Vec::new())
}

fn page_for(pane: &Rc<Pane>) -> Option<Rc<DrivesPage>> {
    PAGES.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|x| x.pane.strong_count() > 0);
        p.iter().find(|x| x.pane.upgrade().is_some_and(|q| Rc::ptr_eq(&q, pane))).cloned()
    })
}

pub fn attach(w: &Window, pane: &Rc<Pane>) {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    list.set_header_func(Some(Box::new(|row, before| {
        let group = |r: &gtk::ListBoxRow| r.widget_name().to_string();
        let g = group(row);
        if before.is_none_or(|b| group(b) != g) {
            let label = gtk::Label::new(None);
            label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(if g == "removable" { "Removable Drives" } else { "This Computer" })));
            label.set_xalign(0.0);
            label.set_margin_start(12);
            label.set_margin_top(12);
            label.set_margin_bottom(4);
            label.style_context().add_class("dim-label");
            label.show();
            row.set_header(Some(&label));
        } else {
            row.set_header(None::<&gtk::Widget>);
        }
    })));
    let root = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    root.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    root.add(&list);
    root.show_all();
    pane.stack.add_named(&root, "drives");
    let page = Rc::new(DrivesPage {
        root,
        list,
        pane: Rc::downgrade(pane),
        window: Rc::downgrade(&w.me()),
        shown: Cell::new(false),
        volumes: RefCell::new(Vec::new()),
        generation: Cell::new(0),
        usages: RefCell::new(Vec::new()),
        weak: RefCell::new(Weak::new()),
    });
    *page.weak.borrow_mut() = Rc::downgrade(&page);
    let weak = Rc::downgrade(&page);
    connect_changed(move || {
        let Some(p) = weak.upgrade() else { return false };
        if p.shown.get() {
            p.refresh();
        }
        true
    });
    let weak = Rc::downgrade(&page);
    page.list.connect_row_activated(move |_, row| {
        if let Some(p) = weak.upgrade() {
            if let Some(v) = p.volumes.borrow().get(row.index() as usize).cloned() {
                p.open(&v);
            }
        }
    });
    let weak = Rc::downgrade(&page);
    page.list.connect_button_press_event(move |list, ev| {
        let Some(p) = weak.upgrade() else { return glib::Propagation::Proceed };
        // A plain click below the drives clears the selection, like in a folder.
        if ev.button() == 1 && ev.event_type() == gdk::EventType::ButtonPress && p.press_left(ev.position().1 as i32) {
            return glib::Propagation::Stop;
        }
        if ev.button() != 3 {
            return glib::Propagation::Proceed;
        }
        if let Some(row) = list.row_at_y(ev.position().1 as i32) {
            list.select_row(Some(&row));
            p.context_menu(row.index() as usize, ev);
        }
        glib::Propagation::Stop
    });
    PAGES.with(|p| p.borrow_mut().push(page));
}

/// Properties of the drive selected on this pane's page (Alt+Enter).
pub fn show_selected_properties(pane: &Rc<Pane>) {
    let Some(page) = page_for(pane) else { return };
    let idx = page.list.selected_row().map(|r| r.index() as usize).unwrap_or(0);
    let v = page.volumes.borrow().get(idx).cloned();
    let u = page.usages.borrow().get(idx).copied().flatten();
    if let Some(v) = v {
        page.show_properties(&v, u);
    }
}

/// Opens the properties of the volume with this title (tests).
pub fn show_properties_of(pane: &Rc<Pane>, title: &str) -> bool {
    let Some(page) = page_for(pane) else { return false };
    let found = page.volumes.borrow().iter().position(|v| v.title() == title);
    match found {
        Some(i) => {
            let v = page.volumes.borrow()[i].clone();
            let u = page.usages.borrow().get(i).copied().flatten();
            page.show_properties(&v, u);
            true
        }
        None => false,
    }
}

/// Shows or hides the page as the pane's location changes.
pub fn location_changed(pane: &Rc<Pane>) {
    let Some(page) = page_for(pane) else { return };
    if pane.location() == Location::Drives {
        pane.stack.set_visible_child(&page.root);
        page.shown.set(true);
        page.refresh();
        page.list.grab_focus();
    } else {
        page.shown.set(false);
        if pane.stack.visible_child().as_ref() == Some(page.root.upcast_ref()) {
            pane.apply_view_mode();
        }
    }
}

/// The volumes on the page of this pane (tests).
pub fn shown_volumes(pane: &Rc<Pane>) -> Vec<Volume> {
    page_for(pane).map(|p| p.volumes.borrow().clone()).unwrap_or_default()
}

/// Clicks below the last drive of this pane's page (tests); false without a page.
pub fn click_empty(pane: &Rc<Pane>) -> bool {
    let Some(page) = page_for(pane) else { return false };
    let bottom = page.list.children().last().map(|r| r.allocation().y() + r.allocation().height()).unwrap_or(0);
    page.press_left(bottom + 5)
}

/// The texts of the rows of drives with this title, one string per row (tests).
pub fn row_texts(pane: &Rc<Pane>, title: &str) -> Vec<String> {
    let Some(page) = page_for(pane) else { return Vec::new() };
    let vols = page.volumes.borrow();
    page.list
        .children()
        .iter()
        .enumerate()
        .filter(|(i, _)| vols.get(*i).is_some_and(|v| v.title() == title))
        .map(|(_, row)| labels_in(row).join(" | "))
        .collect()
}

fn labels_in(w: &gtk::Widget) -> Vec<String> {
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        return vec![l.text().to_string()];
    }
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        return b.label().map(|l| vec![l.replace('_', "")]).unwrap_or_default();
    }
    w.downcast_ref::<gtk::Container>().map(|c| c.children().iter().flat_map(labels_in).collect()).unwrap_or_default()
}

/// Clicks a button (by its text) in the first row of the drive with this title whose texts
/// contain `row_text` (tests).
pub fn click_button(pane: &Rc<Pane>, title: &str, row_text: &str, button: &str) -> Result<(), String> {
    let page = page_for(pane).ok_or("no drives page")?;
    let rows = page.list.children();
    let i = {
        let vols = page.volumes.borrow();
        (0..rows.len())
            .find(|&i| vols.get(i).is_some_and(|v| v.title() == title) && labels_in(&rows[i]).join(" | ").contains(row_text))
            .ok_or_else(|| format!("no drive {title:?} showing {row_text:?}"))?
    };
    fn find(w: &gtk::Widget, label: &str) -> Option<gtk::Button> {
        if let Some(b) = w.downcast_ref::<gtk::Button>() {
            if b.label().is_some_and(|l| l.replace('_', "") == label) {
                return Some(b.clone());
            }
        }
        w.downcast_ref::<gtk::Container>()?.children().iter().find_map(|c| find(c, label))
    }
    let b = find(&rows[i], button).ok_or_else(|| format!("no button {button:?}"))?;
    if !b.is_sensitive() {
        return Err(format!("button {button:?} is disabled"));
    }
    b.clicked();
    Ok(())
}

/// Picks an item of the right-click menu of the first row of the drive with this title
/// whose texts contain `row_text` (tests).
pub fn menu_item(pane: &Rc<Pane>, title: &str, row_text: &str, item: &str) -> Result<(), String> {
    let page = page_for(pane).ok_or("no drives page")?;
    let rows = page.list.children();
    let i = {
        let vols = page.volumes.borrow();
        (0..rows.len())
            .find(|&i| vols.get(i).is_some_and(|v| v.title() == title) && labels_in(&rows[i]).join(" | ").contains(row_text))
            .ok_or_else(|| format!("no drive {title:?} showing {row_text:?}"))?
    };
    let menu = page.menu(i).ok_or("no menu")?;
    let found = menu
        .children()
        .into_iter()
        .filter_map(|c| c.downcast::<gtk::MenuItem>().ok())
        .find(|m| super::menuicons::item_text(&m).is_some_and(|l| l.replace('_', "") == item))
        .ok_or_else(|| format!("no menu item {item:?}"))?;
    found.activate();
    Ok(())
}

/// Selects the drive with this title (tests).
pub fn select(pane: &Rc<Pane>, title: &str) -> bool {
    let Some(page) = page_for(pane) else { return false };
    let Some(i) = page.volumes.borrow().iter().position(|v| v.title() == title) else { return false };
    page.list.select_row(page.list.row_at_index(i as i32).as_ref());
    true
}

/// The title of the selected drive, if one is (tests).
pub fn selected(pane: &Rc<Pane>) -> Option<String> {
    let page = page_for(pane)?;
    let i = page.list.selected_row()?.index() as usize;
    page.volumes.borrow().get(i).map(Volume::title)
}

impl DrivesPage {
    /// A left click at `y` in the list; true when it hit no drive and was handled.
    fn press_left(&self, y: i32) -> bool {
        if self.list.row_at_y(y).is_some() {
            return false;
        }
        self.clicked_empty();
        true
    }

    fn clicked_empty(&self) {
        self.list.unselect_all();
        self.list.grab_focus();
        if let Some(pane) = self.pane.upgrade() {
            pane.emit(super::pane::PaneEvent::ClickedEmpty);
        }
    }

    fn me(&self) -> Rc<DrivesPage> {
        self.weak.borrow().upgrade().expect("page alive")
    }

    /// Reads the volume list from UDisks2 (or the mount table) and redraws: this computer's
    /// drives at once, again once the phones and network shares have answered.
    pub fn refresh(&self) {
        let me = self.me();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        glib::spawn_future_local(async move {
            let first = me.clone();
            let vols = gather_staged(move |quick| {
                if first.generation.get() == generation {
                    first.rebuild(quick);
                }
            })
            .await;
            if me.generation.get() == generation {
                me.rebuild(vols);
            }
        });
    }

    fn rebuild(&self, vols: Vec<(Volume, Option<Usage>)>) {
        let selected = self.list.selected_row().map(|r| r.index());
        for child in self.list.children() {
            self.list.remove(&child);
        }
        *self.volumes.borrow_mut() = vols.iter().map(|(v, _)| v.clone()).collect();
        *self.usages.borrow_mut() = vols.iter().map(|(_, u)| *u).collect();
        for (v, u) in &vols {
            let row = self.make_row(v, u.as_ref());
            self.list.add(&row);
        }
        self.list.show_all();
        if let Some(r) = selected.and_then(|i| self.list.row_at_index(i)) {
            self.list.select_row(Some(&r));
        }
        if let (Some(p), Some(w)) = (self.pane.upgrade(), self.window.upgrade()) {
            if Rc::ptr_eq(&w.current_pane(), &p) {
                w.update_status();
            }
        }
    }

    fn make_row(&self, v: &Volume, usage: Option<&Usage>) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(if v.removable || v.usb { "removable" } else { "fixed" });
        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(3);
        grid.set_margin_start(12);
        grid.set_margin_end(12);
        grid.set_margin_top(8);
        grid.set_margin_bottom(8);

        let icon = super::icons::volume_image(v, gtk::IconSize::Dialog);
        icon.set_valign(gtk::Align::Center);
        grid.attach(&icon, 0, 0, 1, 4);

        let title = gtk::Label::new(None);
        title.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&v.title())));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        grid.attach(&title, 1, 0, 1, 1);

        // A phone: which storage and what kind of device, not its address.
        let mut sub = if v.is_device() { vec![v.label.clone(), drives::drive_kind(v)] } else { vec![v.device.clone()] };
        sub.retain(|s| !s.is_empty());
        if !v.fs_type.is_empty() {
            sub.push(v.fs_type.clone());
        }
        if !v.drive_name.is_empty() && !v.is_device() {
            sub.push(v.drive_name.clone());
        }
        let subtitle = gtk::Label::new(Some(&sub.join(" · ")));
        subtitle.set_xalign(0.0);
        subtitle.style_context().add_class("dim-label");
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        grid.attach(&subtitle, 1, 1, 1, 1);

        let bar = gtk::LevelBar::for_interval(0.0, 1.0);
        // Theme colors: normal up to 90 % full, the warning color beyond.
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, 0.9);
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 1.0);
        bar.set_size_request(-1, 8);
        match usage {
            Some(u) => bar.set_value(u.fraction()),
            None => bar.set_sensitive(false),
        }
        grid.attach(&bar, 1, 2, 1, 1);

        let text = match (usage, v.mount_point()) {
            (Some(u), Some(_)) if v.is_device() => format!("{} free of {} — {:.0} % used", human_size(u.free), human_size(u.total), u.fraction() * 100.0),
            _ if no_storage(v) => devices::no_storage_hint(DeviceKind::Phone).to_string(),
            (None, Some(_)) if v.is_device() => "Connected".to_string(),
            (None, None) if v.is_device() => "Connected — open it to see its files".to_string(),
            (Some(u), Some(m)) => format!("{} free of {} — {:.0} % used — {}", human_size(u.free), human_size(u.total), u.fraction() * 100.0, m.display()),
            (None, Some(m)) => format!("Mounted at {}", m.display()),
            _ if v.lvm_closed() => format!("Encrypted, unlocked, LVM volumes not open — {}", human_size(v.size)),
            _ if v.unlocked => format!("Encrypted, unlocked — {}", human_size(v.size)),
            _ if v.fs_type.starts_with("crypto") => format!("Encrypted, locked — {}", human_size(v.size)),
            _ => format!("Not mounted — {}", human_size(v.size)),
        };
        let info = gtk::Label::new(Some(&text));
        info.set_xalign(0.0);
        info.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        grid.attach(&info, 1, 3, 1, 1);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_valign(gtk::Align::Center);
        let busy = is_busy(v);
        let add = |label: &str, tip: &str, f: Box<dyn Fn(&DrivesPage)>| {
            let b = gtk::Button::with_mnemonic(label);
            b.set_tooltip_text(Some(tip));
            b.set_sensitive(!busy);
            let weak = self.weak.borrow().clone();
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    f(&p);
                }
            });
            buttons.pack_start(&b, false, false, 0);
        };
        let vol = v.clone();
        if no_storage(v) {
            add("_Retry", "Connect again, after unlocking the phone", Box::new(move |p| p.reconnect(&vol)));
        } else if v.mount_point().is_some() {
            add("_Open", "Show the files on this drive", Box::new(move |p| p.open(&vol)));
        } else if v.is_device() {
            add("_Open", "Connect and show the files on this device", Box::new(move |p| p.open(&vol)));
        } else if v.mountable && !v.block_object.is_empty() {
            add("_Mount", "Mount and open this drive", Box::new(move |p| p.open(&vol)));
        } else if v.is_locked() {
            add("_Unlock", "Unlock with the passphrase, mount and open this drive", Box::new(move |p| p.open(&vol)));
        } else if v.lvm_closed() {
            add("_Open", "Open the LVM volumes on this drive (asks for the administrator password)", Box::new(move |p| p.open(&vol)));
        }
        if can_unmount(v) {
            let vol = v.clone();
            add("_Unmount", "Unmount this drive", Box::new(move |p| p.unmount(&vol, false)));
        }
        if can_eject(v) {
            let vol = v.clone();
            let tip = if v.is_device() { "Close the connection, safe to unplug" } else { "Unmount and power off, safe to unplug" };
            add("_Eject", tip, Box::new(move |p| p.unmount(&vol, true)));
        }
        let info = gtk::Button::from_icon_name(Some("document-properties-symbolic"), gtk::IconSize::Button);
        info.set_tooltip_text(Some("Properties"));
        let (vol, u) = (v.clone(), usage.copied());
        let weak = self.weak.borrow().clone();
        info.connect_clicked(move |_| {
            if let Some(p) = weak.upgrade() {
                p.show_properties(&vol, u);
            }
        });
        buttons.pack_start(&info, false, false, 0);
        if busy {
            let spinner = gtk::Spinner::new();
            spinner.start();
            buttons.pack_start(&spinner, false, false, 0);
        }
        grid.attach(&buttons, 2, 0, 1, 4);
        row.add(&grid);
        row
    }

    fn context_menu(&self, index: usize, ev: &gtk::gdk::EventButton) {
        if let Some(menu) = self.menu(index) {
            menu.popup_at_pointer(Some(ev));
        }
    }

    /// The right-click menu of the drive in row `index`.
    fn menu(&self, index: usize) -> Option<gtk::Menu> {
        let v = self.volumes.borrow().get(index).cloned()?;
        let u = self.usages.borrow().get(index).copied().flatten();
        let menu = gtk::Menu::new();
        let add = |label: &str, f: Box<dyn Fn(&DrivesPage)>| {
            let item = super::menuicons::item(label);
            let weak = self.weak.borrow().clone();
            item.connect_activate(move |_| {
                if let Some(p) = weak.upgrade() {
                    f(&p);
                }
            });
            menu.append(&item);
        };
        let vol = v.clone();
        if no_storage(&v) {
            add("_Retry", Box::new(move |p| p.reconnect(&vol)));
        } else if v.mount_point().is_some() || v.is_device() {
            add("_Open", Box::new(move |p| p.open(&vol)));
        } else if v.mountable {
            add("_Mount", Box::new(move |p| p.open(&vol)));
        } else if v.is_locked() {
            add("_Unlock", Box::new(move |p| p.open(&vol)));
        } else if v.lvm_closed() {
            add("_Open", Box::new(move |p| p.open(&vol)));
        }
        if can_unmount(&v) {
            let vol = v.clone();
            add("_Unmount", Box::new(move |p| p.unmount(&vol, false)));
        }
        if can_eject(&v) {
            let vol = v.clone();
            add("_Eject", Box::new(move |p| p.unmount(&vol, true)));
        }
        // The format of the volume, wherever the act could reach it at all; where it cannot, the item is
        // shut and its tooltip says why (README: *Formatting a volume*).
        let vol = v.clone();
        match failbrauwser::fsformat::refusal(&v) {
            None => add("_Format…", Box::new(move |p| p.format(&vol, u))),
            Some(why) => {
                let item = super::menuicons::item("_Format…");
                item.set_sensitive(false);
                item.set_tooltip_text(Some(&format!("This volume cannot be formatted: {why}.")));
                menu.append(&item);
            }
        }
        // The two device-icon verbs write files and nothing else. Where the volume cannot take them the items
        // are shut and the reason the act gives stands on their tooltip (README: *Device icons*).
        match failbrauwser::fsformat::act::icon_refusal(&v) {
            None => {
                menu.append(&gtk::SeparatorMenuItem::new());
                let vol = v.clone();
                add("_Create device icon…", Box::new(move |p| p.create_icon_window(&vol)));
                let vol = v.clone();
                add("_Remove device icon", Box::new(move |p| p.remove_icon(&vol)));
            }
            Some(why) => {
                menu.append(&gtk::SeparatorMenuItem::new());
                for label in ["_Create device icon…", "_Remove device icon"] {
                    let item = super::menuicons::item(label);
                    item.set_sensitive(false);
                    item.set_tooltip_text(Some(&why));
                    menu.append(&item);
                }
            }
        }
        if let Some(link) = super::desktoplink::for_volume(&v) {
            menu.append(&gtk::SeparatorMenuItem::new());
            add("Create _Link on Desktop", Box::new(move |p| {
                if let Some((w, _)) = p.context() {
                    super::desktoplink::create(&w, std::slice::from_ref(&link));
                }
            }));
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        let vol = v.clone();
        add("_Properties…", Box::new(move |p| p.show_properties(&vol, u)));
        menu.set_attach_widget(Some(&self.list));
        menu.show_all();
        Some(menu)
    }

    fn context(&self) -> Option<(Rc<Window>, Rc<Pane>)> {
        Some((self.window.upgrade()?, self.pane.upgrade()?))
    }

    fn open(&self, v: &Volume) {
        if let Some((w, p)) = self.context() {
            open_volume(&w, &p, v, false);
        }
    }

    fn reconnect(&self, v: &Volume) {
        if let Some((w, p)) = self.context() {
            reconnect_device(&w, &p, v);
        }
    }

    fn unmount(&self, v: &Volume, eject: bool) {
        if let Some((w, _)) = self.context() {
            unmount_volume(&w, v, eject, &self.volumes.borrow());
        }
    }

    fn show_properties(&self, v: &Volume, usage: Option<Usage>) {
        if let Some((w, p)) = self.context() {
            show_properties(&w, &p, v, usage);
        }
    }

    /// The format window of this volume, whose *Format* hands the choice to the act (README: *Formatting a
    /// volume*). The page keeps no handle of its own: the act marks the volume busy, and the page follows
    /// that.
    fn format(&self, v: &Volume, usage: Option<Usage>) {
        if let Some((w, _)) = self.context() {
            let (win, vol) = (Rc::downgrade(&w), v.clone());
            super::format::show(&w, v, usage, move |fs, chosen| format_volume(win.clone(), &vol, fs, chosen));
        }
    }

    /// The window naming the shape a device icon is drawn for, which then hands that shape to the act
    /// (README: *Device icons*).
    fn create_icon_window(&self, v: &Volume) {
        if let Some((w, _)) = self.context() {
            icon_window(&w, v);
        }
    }

    /// Taking a device icon away: the two text files of ours and the image files they name, nothing else.
    fn remove_icon(&self, v: &Volume) {
        if let Some((w, _)) = self.context() {
            remove_device_icon(&w, v);
        }
    }
}

/// Everything known about a volume, its drive and how full it is.
pub fn show_properties(w: &Rc<Window>, pane: &Rc<Pane>, v: &Volume, usage: Option<Usage>) {
    let d = gtk::Dialog::with_buttons(Some(&format!("{} — Properties", v.title())), Some(&w.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[]);
    d.set_resizable(false);
    let area = d.content_area();
    area.set_border_width(12);
    area.set_spacing(12);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.pack_start(&super::icons::volume_image(v, gtk::IconSize::Dialog), false, false, 0);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::new(None);
    title.set_markup(&format!("<big><b>{}</b></big>", glib::markup_escape_text(&v.title())));
    title.set_xalign(0.0);
    let mut sub: Vec<String> = Vec::new();
    let kind = drives::drive_kind(v);
    if !kind.is_empty() {
        sub.push(kind);
    }
    if v.size > 0 {
        sub.push(human_size(v.size));
    }
    if !v.fs_type.is_empty() {
        sub.push(v.fs_type.clone());
    }
    let subtitle = gtk::Label::new(Some(&sub.join(" · ")));
    subtitle.set_xalign(0.0);
    subtitle.style_context().add_class("dim-label");
    titles.pack_start(&title, false, false, 0);
    titles.pack_start(&subtitle, false, false, 0);
    header.pack_start(&titles, true, true, 0);
    area.add(&header);

    // How full: a bar with used / free / total underneath.
    if let Some(u) = usage {
        let bar = gtk::LevelBar::for_interval(0.0, 1.0);
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, 0.9);
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 1.0);
        bar.set_value(u.fraction());
        bar.set_size_request(360, 12);
        area.add(&bar);
        let counts = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        counts.set_homogeneous(true);
        for (label, value) in [
            ("Used", format!("{} ({:.0} %)", human_size(u.used), u.fraction() * 100.0)),
            ("Free", human_size(u.free)),
            ("Total", human_size(u.total)),
        ] {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let k = gtk::Label::new(Some(label));
            k.style_context().add_class("dim-label");
            let val = gtk::Label::new(None);
            val.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&value)));
            b.pack_start(&k, false, false, 0);
            b.pack_start(&val, false, false, 0);
            counts.pack_start(&b, true, true, 0);
        }
        area.add(&counts);
    }

    // A row of the properties grid: words the volume answers, or the volume's own name as it is written
    // here.
    enum Row {
        Text(String),
        Named(gtk::Box),
    }

    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(4);
    let mut y = 0;
    let mut section = |name: &str, rows: Vec<(&str, Row)>| {
        let rows: Vec<_> = rows.into_iter().filter(|(_, row)| !matches!(row, Row::Text(v) if v.is_empty())).collect();
        if rows.is_empty() {
            return;
        }
        let h = gtk::Label::new(None);
        h.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(name)));
        h.set_xalign(0.0);
        h.set_margin_top(if y == 0 { 0 } else { 8 });
        grid.attach(&h, 0, y, 2, 1);
        y += 1;
        for (k, row) in rows {
            let kl = gtk::Label::new(Some(k));
            kl.set_xalign(1.0);
            kl.set_yalign(0.0);
            kl.style_context().add_class("dim-label");
            grid.attach(&kl, 0, y, 1, 1);
            match row {
                Row::Text(val) => {
                    let vl = gtk::Label::new(Some(&val));
                    vl.set_xalign(0.0);
                    vl.set_selectable(true);
                    vl.set_line_wrap(true);
                    vl.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
                    vl.set_max_width_chars(44);
                    grid.attach(&vl, 1, y, 1, 1);
                }
                // The name of the volume is written from this window, so it is typed here (README:
                // *Renaming a volume*).
                Row::Named(root) => grid.attach(&root, 1, y, 1, 1),
            }
            y += 1;
        }
    };
    let fs = match (v.fs_type.as_str(), v.fs_version.as_str()) {
        (t, "") => t.to_string(),
        (t, ver) => format!("{t} {ver}"),
    };
    let mounted = v.mount_points.iter().map(|m| m.display().to_string()).collect::<Vec<_>>().join("\n");
    let options = v.mount_point().and_then(|m| drives::mount_options(m)).unwrap_or_default();
    let yes_no = |b: bool| if b { "Yes".to_string() } else { "No".to_string() };
    // The name of the volume is written from this window wherever it could be written at all: the file
    // system is one this program names a label for, the gate lets the volume through, and no act of ours is
    // running on it. Elsewhere the row is the name as the volume says it (README: *Renaming a volume*).
    let named = match (failbrauwser::fsformat::FileSystem::parse(&v.fs_type), failbrauwser::fsformat::refusal(v)) {
        (Some(fs), None) if !is_busy(v) => Some(name_row(v, fs, Rc::downgrade(w))),
        _ => None,
    };
    section(
        "Volume",
        vec![
            ("Label:", match &named { Some(root) => Row::Named(root.clone()), None => Row::Text(v.label.clone()) }),
            ("Device:", Row::Text(v.device.clone())),
            ("File system:", Row::Text(fs)),
            ("UUID:", Row::Text(v.uuid.clone())),
            ("Mounted at:", Row::Text(if mounted.is_empty() { "Not mounted".into() } else { mounted })),
            ("Mount options:", Row::Text(options)),
            ("Read-only:", Row::Text(yes_no(v.read_only || v.mount_point().and_then(|m| drives::mount_options(m)).is_some_and(|o| o.split(',').any(|x| x == "ro"))))),
        ],
    );
    section(
        "Partition",
        vec![
            ("Number:", Row::Text(if v.partition_number > 0 { v.partition_number.to_string() } else { String::new() })),
            ("Name:", Row::Text(v.partition_name.clone())),
            ("Type:", Row::Text(if v.partition_type.is_empty() { String::new() } else { drives::partition_type_name(&v.partition_type) })),
        ],
    );
    section(
        "Drive",
        vec![
            ("Model:", Row::Text(v.drive_name.clone())),
            ("Serial number:", Row::Text(v.serial.clone())),
            ("Firmware:", Row::Text(v.revision.clone())),
            ("Kind:", Row::Text(drives::drive_kind(v))),
            ("Connection:", Row::Text(match v.bus.as_str() {
                "usb" if v.is_device() => format!("USB ({})", v.protocol.to_uppercase()),
                "usb" => "USB".into(),
                "sdio" | "mmc" => "SD card reader".into(),
                "ieee1394" => "FireWire".into(),
                "nvme" => "NVMe".into(),
                "sata" => "SATA".into(),
                "ata" | "atapi" => "ATA".into(),
                "raid" => "RAID".into(),
                "virtual" => "Virtual disk".into(),
                "" if v.device.starts_with("/dev/nvme") => "NVMe".into(),
                "" if !v.drive_object.is_empty() => "Internal".into(),
                other => other.to_string(),
            })),
            ("Drive size:", Row::Text(if v.drive_size > 0 { human_size(v.drive_size) } else { String::new() })),
            ("Removable:", Row::Text(if v.drive_object.is_empty() { String::new() } else { yes_no(v.removable || v.usb) })),
        ],
    );
    area.add(&grid);

    // *Format…* opens the format window of this volume. Where the gate refuses the volume it stays shut,
    // and the reason the act gives is the words on its tooltip: the refusal exists before a command does,
    // so the window has nothing to offer that volume (README: *Formatting a volume*).
    let acts = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let format = gtk::Button::with_mnemonic("_Format…");
    format.set_widget_name("format-button");
    match failbrauwser::fsformat::refusal(v) {
        None => {
            format.connect_clicked({
                let (win, vol, u) = (Rc::downgrade(w), v.clone(), usage);
                move |_| {
                    let Some(w) = win.upgrade() else { return };
                    let (act_on, act_vol) = (win.clone(), vol.clone());
                    super::format::show(&w, &vol, u, move |fs, chosen| format_volume(act_on.clone(), &act_vol, fs, chosen));
                }
            });
        }
        Some(why) => {
            format.set_sensitive(false);
            format.set_tooltip_text(Some(&format!("This volume cannot be formatted: {why}.")));
        }
    }
    acts.pack_start(&format, false, false, 0);

    // The two device-icon verbs write files and nothing else: they never dismount, never format, never touch a
    // file system. Where the volume cannot take them the buttons stay shut, with the act's own reason on their
    // tooltip (README: *Device icons*).
    let create = gtk::Button::with_mnemonic("_Create device icon…");
    let remove = gtk::Button::with_mnemonic("_Remove device icon");
    match failbrauwser::fsformat::act::icon_refusal(v) {
        None => {
            create.connect_clicked({
                let (win, vol) = (Rc::downgrade(w), v.clone());
                move |_| {
                    if let Some(w) = win.upgrade() {
                        icon_window(&w, &vol);
                    }
                }
            });
            remove.connect_clicked({
                let (win, vol) = (Rc::downgrade(w), v.clone());
                move |_| {
                    if let Some(w) = win.upgrade() {
                        remove_device_icon(&w, &vol);
                    }
                }
            });
        }
        Some(why) => {
            for button in [&create, &remove] {
                button.set_sensitive(false);
                button.set_tooltip_text(Some(&why));
            }
        }
    }
    acts.pack_start(&create, false, false, 0);
    acts.pack_start(&remove, false, false, 0);
    area.add(&acts);

    if v.mount_point().is_some() && !no_storage(v) || v.is_device() {
        d.add_button("_Open", gtk::ResponseType::Accept);
    } else if v.mountable && !v.block_object.is_empty() {
        d.add_button("_Mount and Open", gtk::ResponseType::Accept);
    }
    d.add_button("_Close", gtk::ResponseType::Close);
    d.set_default_response(gtk::ResponseType::Close);
    let (win, pane) = (Rc::downgrade(w), Rc::downgrade(pane));
    let vol = v.clone();
    d.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Accept {
            if let (Some(w), Some(p)) = (win.upgrade(), pane.upgrade()) {
                open_volume(&w, &p, &vol, false);
            }
        }
        d.close();
    });
    d.show_all();
}

/// The *Label* row of a drive's properties window: an entry you type into, the *Rename* button beside it
/// that appears while the typed name differs from what the volume says and disappears again when it does
/// not, and the words naming the shorter name the file system really carries (README: *Renaming a volume*,
/// *A name too long for the file system is kept, not thrown away*).
///
/// The row answers as long as its own widgets do and holds none of them strongly: what the handlers pass
/// each other is a weak handle and three words of state, so closing the window ends the row with the
/// widgets, and an act that outlives it finds nothing to answer to.
fn name_row(v: &Volume, fs: failbrauwser::fsformat::FileSystem, win: Weak<Window>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let entry = gtk::Entry::new();
    entry.set_widget_name("label-entry");
    entry.set_text(&v.label);
    entry.set_width_chars(26);
    entry.set_max_width_chars(26);
    let rename = gtk::Button::with_mnemonic("_Rename");
    rename.set_widget_name("rename-button");
    rename.set_no_show_all(true);
    rename.set_visible(false);
    let carried = gtk::Label::new(None);
    carried.set_widget_name("label-carried");
    carried.set_xalign(0.0);
    carried.set_line_wrap(true);
    carried.set_max_width_chars(48);
    carried.style_context().add_class("dim-label");
    line.pack_start(&entry, true, true, 0);
    line.pack_start(&rename, false, false, 0);
    root.pack_start(&line, false, false, 0);
    root.pack_start(&carried, false, false, 0);

    let typed = Rc::new(RefCell::new(v.label.clone()));
    let loaded = Rc::new(RefCell::new(v.label.clone()));
    let running = Rc::new(Cell::new(false));
    let mark: Rc<dyn Fn()> = {
        let (rename, carried, typed, loaded, running) = (rename.downgrade(), carried.downgrade(), typed.clone(), loaded.clone(), running.clone());
        Rc::new(move || {
            let (Some(rename), Some(carried)) = (rename.upgrade(), carried.upgrade()) else { return };
            let name = typed.borrow().clone();
            let plan = failbrauwser::fsformat::name_plan(fs, &name);
            carried.set_text(&match &plan {
                Ok(p) if p.pinned => format!("the volume itself carries “{}” — {} holds {} characters at most", p.label, fs.word(), fs.label_limit()),
                Ok(p) if p.label.is_empty() && !p.called.is_empty() => "the volume would hold no name at all".into(),
                Ok(_) => String::new(),
                Err(why) => why.clone(),
            });
            let differs = name.trim() != loaded.borrow().trim();
            rename.set_visible(differs);
            // A name the file system cannot carry is refused where it is typed: the button is there, because
            // the text differs, and shut, because there is nothing to write.
            rename.set_sensitive(differs && plan.is_ok() && !running.get());
        })
    };
    entry.connect_changed({
        let (typed, mark) = (typed.clone(), mark.clone());
        move |e| {
            *typed.borrow_mut() = e.text().to_string();
            mark();
        }
    });
    rename.connect_clicked({
        let (mark, vol, win, typed, loaded, running) = (mark.clone(), v.clone(), win, typed.clone(), loaded.clone(), running.clone());
        move |_| {
            if running.replace(true) {
                return;
            }
            mark();
            let name = typed.borrow().clone();
            rename_volume(win.clone(), &vol, &name, {
                let (loaded, running, mark) = (loaded.clone(), running.clone(), mark.clone());
                move |answer: Result<String, String>| {
                    running.set(false);
                    // What the volume carries is what the row holds as loaded from then on, so a name the file
                    // system shortened is not offered back to the entry as a change of its own.
                    if let Ok(label) = answer {
                        *loaded.borrow_mut() = label;
                    }
                    mark();
                }
            });
        }
    });
    mark();
    root
}

/// Writes a volume's name off the UI thread (README: *Renaming a volume*), the volume marked busy while the
/// act runs. What comes back is the name the volume now carries — the short variant, where the typed name
/// was longer than the file system holds — and a refusal is the act's own words. What became of the longer
/// name is told in a window of its own, like the two device-icon verbs tell theirs: whether it stands pinned
/// in the two text files, was taken back, or could not be pinned at all.
fn rename_volume(win: Weak<Window>, v: &Volume, name: &str, on_answer: impl Fn(Result<String, String>) + 'static) {
    let (key, vol, name) = (busy_key(v), v.clone(), name.to_string());
    set_busy(&key, true);
    glib::spawn_future_local(async move {
        let answer = gio::spawn_blocking(move || failbrauwser::fsformat::act::rename(&vol, &name)).await.map_err(|_| "the rename run ended".to_string()).and_then(|r| r);
        set_busy(&key, false);
        match answer {
            Ok(named) => {
                on_answer(Ok(named.label.clone()));
                if !named.words.is_empty() { notice(&win, "The volume's name", &named.words); }
            }
            Err(e) => {
                error(&win, "Cannot rename the volume", &e);
                on_answer(Err(e));
            }
        }
    });
}

/// Formats a volume off the UI thread (README: *Formatting a volume*) with the choice its format window
/// answered, the volume marked busy while the act runs. What comes back is the machine's own words, because
/// the tool that wrote the file system is the one that refused to.
fn format_volume(win: Weak<Window>, v: &Volume, fs: failbrauwser::fsformat::FileSystem, chosen: Vec<(failbrauwser::fsformat::FormatOption, failbrauwser::fsformat::Value)>) {
    let (key, vol) = (busy_key(v), v.clone());
    set_busy(&key, true);
    glib::spawn_future_local(async move {
        let answer = gio::spawn_blocking(move || failbrauwser::fsformat::act::format(&vol, fs, &chosen)).await.map_err(|_| "the format run ended".to_string()).and_then(|r| r);
        set_busy(&key, false);
        if let Err(e) = answer {
            error(&win, "Cannot format the volume", &e);
        }
    });
}

/// The window a *Create device icon* verb opens (README: *Device icons*): the shape the machine detected with
/// its evidence spelled out beside it — the same dropdown and the same words the format window carries — and a
/// *Create* button that hands that shape to the act. Nothing is written before it is pressed.
fn icon_window(w: &Rc<Window>, v: &Volume) {
    let d = gtk::Dialog::with_buttons(Some(&format!("{} — Device icon", v.title())), Some(&w.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Cancel", gtk::ResponseType::Cancel)]);
    d.set_resizable(false);
    let area = d.content_area();
    area.set_border_width(12);
    area.set_spacing(10);

    let heading = gtk::Label::new(None);
    heading.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&format!("Device icon of “{}”", v.title()))));
    heading.set_xalign(0.0);
    area.add(&heading);
    let files = gtk::Label::new(Some("The four files are written onto this volume: DevIcon.fil and folder.ico, autorun.inf and desktop.ini. Nothing else is touched."));
    files.set_xalign(0.0);
    files.set_line_wrap(true);
    files.set_max_width_chars(52);
    area.add(&files);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(6);
    let shape = gtk::ComboBoxText::new();
    shape.set_widget_name("device-shape");
    for kind in failbrauwser::fsformat::Shape::all() {
        shape.append(Some(kind.asset()), kind.label());
    }
    shape.set_active_id(Some(failbrauwser::fsformat::Shape::detected(v).unwrap_or(failbrauwser::fsformat::Shape::UsbStick).asset()));
    let key = gtk::Label::new(Some("Device shape:"));
    key.set_xalign(1.0);
    key.style_context().add_class("dim-label");
    let evidence = gtk::Label::new(Some(&failbrauwser::fsformat::Shape::evidence(v)));
    evidence.set_widget_name("shape-carried");
    evidence.set_xalign(0.0);
    evidence.set_line_wrap(true);
    evidence.set_max_width_chars(52);
    evidence.style_context().add_class("dim-label");
    grid.attach(&key, 0, 0, 1, 1);
    grid.attach(&shape, 1, 0, 1, 1);
    grid.attach(&evidence, 1, 1, 1, 1);
    area.add(&grid);

    let create = gtk::Button::with_mnemonic("_Create");
    create.set_widget_name("create-icon-button");
    create.style_context().add_class("suggested-action");
    d.add_action_widget(&create, gtk::ResponseType::Accept);
    d.set_default_response(gtk::ResponseType::Cancel);
    d.connect_response({
        let (win, vol) = (Rc::downgrade(w), v.clone());
        move |d, resp| {
            if resp == gtk::ResponseType::Accept {
                let chosen = shape.active_id().and_then(|id| failbrauwser::fsformat::Shape::parse(id.as_str())).unwrap_or(failbrauwser::fsformat::Shape::UsbStick);
                if let Some(w) = win.upgrade() {
                    create_device_icon(&w, &vol, chosen);
                }
            }
            d.close();
        }
    });
    d.show_all();
}

/// Writes the four files off the UI thread with the shape the window named, the volume marked busy while the
/// act runs, and answers in the words the act wrote — which name the files that stand (README: *Device icons*).
fn create_device_icon(w: &Rc<Window>, v: &Volume, shape: failbrauwser::fsformat::Shape) {
    let win = Rc::downgrade(w);
    let (key, vol, fs, name) = (busy_key(v), v.clone(), v.fs_type.clone(), v.label.clone());
    set_busy(&key, true);
    glib::spawn_future_local(async move {
        let answer = gio::spawn_blocking(move || failbrauwser::fsformat::act::create_icon(&vol, &fs, shape, &name)).await.map_err(|_| "the device-icon run ended".to_string()).and_then(|r| r);
        set_busy(&key, false);
        match answer {
            Ok(words) => notice(&win, "Device icon", &words),
            Err(e) => error(&win, "Cannot write the device icon", &e),
        }
    });
}

/// Takes a device icon away off the UI thread, the volume marked busy while the act runs. What was kept — an
/// `autorun.inf` or `desktop.ini` of somebody else — is told, because the verb promises to say so (README:
/// *Device icons*).
fn remove_device_icon(w: &Rc<Window>, v: &Volume) {
    let win = Rc::downgrade(w);
    let (key, vol) = (busy_key(v), v.clone());
    set_busy(&key, true);
    glib::spawn_future_local(async move {
        let answer = gio::spawn_blocking(move || failbrauwser::fsformat::act::remove_icon(&vol)).await.map_err(|_| "the device-icon run ended".to_string()).and_then(|r| r);
        set_busy(&key, false);
        match answer {
            Ok(words) => notice(&win, "Device icon", &words),
            Err(e) => error(&win, "Cannot take the device icon away", &e),
        }
    });
}

/// The words of an act that worked, told in a window of its own: for the two device-icon verbs the answer —
/// which files stand, which were kept and why — is the point of the verb, not a side note.
fn notice(w: &Weak<Window>, title: &str, detail: &str) {
    if let Some(w) = w.upgrade() {
        let d = gtk::MessageDialog::new(Some(&w.win), gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Info, gtk::ButtonsType::Close, title);
        d.set_secondary_text(Some(detail));
        d.connect_response(|d, _| d.close());
        d.show();
    }
}

/// Shows a volume's files in `pane` (or a new tab), mounting it or connecting to the phone
/// first when needed.
pub fn open_volume(w: &Rc<Window>, pane: &Rc<Pane>, v: &Volume, new_tab: bool) {
    let show = shower(w, pane, new_tab);
    if v.is_device() && (v.mount_point().is_none() || no_storage(v)) {
        open_device(w, v, show);
        return;
    }
    if let Some(m) = v.mount_point() {
        show(Location::Dir(m.clone()));
        return;
    }
    if v.is_locked() {
        unlock_and_open(w, v, Rc::new(show));
        return;
    }
    if v.lvm_closed() {
        let win = Rc::downgrade(w);
        let (obj, dev) = (v.block_object.clone(), v.device.clone());
        set_busy(&obj, true);
        glib::spawn_future_local(async move {
            let res = lvm_helper("open", &dev).await;
            set_busy(&obj, false);
            match res {
                Ok(mounts) => {
                    if let Some(m) = first_mount(mounts) {
                        show(Location::Dir(m));
                    }
                }
                Err(e) if e.is_empty() => {}
                Err(e) => error(&win, "Cannot open the drive's LVM volumes", &e),
            }
        });
        return;
    }
    if !v.mountable || v.block_object.is_empty() {
        return;
    }
    let win = Rc::downgrade(w);
    let obj = v.block_object.clone();
    set_busy(&obj, true);
    glib::spawn_future_local(async move {
        let res = mount(&obj).await;
        set_busy(&obj, false);
        match res {
            Ok(path) => show(Location::Dir(path)),
            Err(e) => error(&win, "Cannot mount drive", &e),
        }
    });
}

async fn mount(block_object: &str) -> Result<PathBuf, String> {
    let reply = call(block_object, "org.freedesktop.UDisks2.Filesystem", "Mount", empty_options()).await?;
    Ok(PathBuf::from(reply.child_value(0).get::<String>().unwrap_or_default()))
}

/// An encrypted drive: asks for its passphrase, unlocks it and mounts what is inside. A wrong
/// passphrase asks again.
fn unlock_and_open(w: &Rc<Window>, v: &Volume, show: Rc<dyn Fn(Location)>) {
    let win = Rc::downgrade(w);
    let obj = v.block_object.clone();
    let v2 = v.clone();
    util::ask_secret(&w.win, "Encrypted Drive", &format!("Passphrase for “{}”:", v.title()), move |pass| {
        let (win, obj, show, v2) = (win.clone(), obj.clone(), show.clone(), v2.clone());
        set_busy(&obj, true);
        glib::spawn_future_local(async move {
            let opts = glib::VariantDict::new(None).end();
            let res = call(&obj, "org.freedesktop.UDisks2.Encrypted", "Unlock", glib::Variant::tuple_from_iter([pass.to_variant(), opts])).await;
            let opened = match res {
                Ok(reply) => {
                    let clear = reply.child_value(0).str().unwrap_or_default().to_string();
                    if holds_lvm(&clear).await {
                        // LVM volumes instead of a file system (a Linux installation).
                        match lvm_helper("open", &v2.device).await {
                            Ok(mounts) => Ok(first_mount(mounts)),
                            // The administrator password was not given: it stays unlocked,
                            // listed to open its volumes later.
                            Err(e) if e.is_empty() => Ok(None),
                            Err(e) => {
                                let _ = call(&obj, "org.freedesktop.UDisks2.Encrypted", "Lock", empty_options()).await;
                                Err(("Unlocked, but cannot open its LVM volumes", e))
                            }
                        }
                    } else {
                        // GNOME and others may already have mounted it on unlocking.
                        match mount(&clear).await {
                            Ok(p) => Ok(Some(p)),
                            Err(e) if e.contains("already mounted") => Ok(None),
                            Err(e) => Err(("Unlocked, but cannot mount it", e)),
                        }
                    }
                }
                Err(e) => Err(("Cannot unlock the drive", e)),
            };
            set_busy(&obj, false);
            match opened {
                Ok(Some(p)) => show(Location::Dir(p)),
                Ok(None) => {}
                Err((_, e)) if e.to_lowercase().contains("passphrase") || e.contains("No key available") => {
                    if let Some(w) = win.upgrade() {
                        util::show_error(&w.win, "Wrong passphrase", "The drive stays locked. Try again.");
                        unlock_and_open(&w, &v2, show);
                    }
                }
                Err((title, e)) => error(&win, title, &e),
            }
        });
    });
}

/// Whether the unlocked contents of an encrypted drive are an LVM physical volume.
async fn holds_lvm(cleartext: &str) -> bool {
    let args = glib::Variant::tuple_from_iter(["org.freedesktop.UDisks2.Block".to_variant(), "IdType".to_variant()]);
    match call(cleartext, "org.freedesktop.DBus.Properties", "Get", args).await {
        Ok(r) => r.child_value(0).as_variant().and_then(|v| v.str().map(str::to_string)).as_deref() == Some(failbrauwser::lvm::PV_TYPE),
        Err(_) => false,
    }
}

/// The folder to show of the LVM volumes just mounted: `home` if there is one.
fn first_mount(mounts: Vec<PathBuf>) -> Option<PathBuf> {
    mounts.iter().find(|m| m.file_name().is_some_and(|n| n == "home")).cloned().or_else(|| mounts.into_iter().next())
}

/// Opens (`open`) or closes (`close`) the LVM volumes inside the unlocked encrypted
/// partition `device`, as root through pkexec (see `failbrauwser::lvm`). Gives the folders
/// they are mounted on; an empty error when the administrator password was not given.
async fn lvm_helper(action: &str, device: &str) -> Result<Vec<PathBuf>, String> {
    if device.starts_with("/dev/fake-") {
        return fake_lvm(action, device);
    }
    if std::env::var_os("FB_NO_AUTH_PROMPTS").is_some() {
        return Err("Opening LVM volumes needs the administrator password (prompts are turned off).".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let argv: Vec<&std::ffi::OsStr> = vec!["pkexec".as_ref(), exe.as_os_str(), "--lvm".as_ref(), action.as_ref(), device.as_ref()];
    if gtk::glib::find_program_in_path("pkexec").is_none() {
        return Err(failbrauwser::lvm::NO_PKEXEC.into());
    }
    let proc = gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE).map_err(|e| format!("cannot run pkexec: {e}"))?;
    let (out, err) = proc.communicate_utf8_future(None).await.map_err(|e| e.to_string())?;
    let out = out.map(|s| s.to_string()).unwrap_or_default();
    let err = err.map(|s| s.to_string()).unwrap_or_default();
    // pkexec: 126 the dialog was dismissed, 127 not authorized.
    match proc.exit_status() {
        0 => Ok(out.lines().filter_map(|l| l.strip_prefix("MOUNT ")).map(PathBuf::from).collect()),
        126 => Err(String::new()),
        127 => Err("Not authorized.".into()),
        _ => Err(Some(err.trim().to_string()).filter(|e| !e.is_empty()).unwrap_or_else(|| "The LVM volumes could not be opened.".into())),
    }
}

/// The self-test's stand-in for the LVM helper on a fake encrypted stick.
fn fake_lvm(action: &str, device: &str) -> Result<Vec<PathBuf>, String> {
    let res = FAKES.with(|f| {
        let mut f = f.borrow_mut();
        let d = f.iter_mut().find(|d| format!("/dev/fake-{}-luks", d.label) == device && d.lvm && d.unlocked).ok_or("the drive is not unlocked")?;
        d.mounted = action == "open";
        Ok(if d.mounted { vec![d.dir.clone()] } else { Vec::new() })
    });
    changed();
    res
}

/// Where opened files go: the pane, or a new tab of the window.
fn shower(w: &Rc<Window>, pane: &Rc<Pane>, new_tab: bool) -> impl Fn(Location) + 'static {
    let (win, pane) = (Rc::downgrade(w), Rc::downgrade(pane));
    move |loc| {
        if new_tab {
            if let Some(w) = win.upgrade() {
                w.add_tab(loc, true);
            }
        } else if let Some(p) = pane.upgrade() {
            p.navigate(loc);
        }
    }
}

fn error(w: &Weak<Window>, title: &str, msg: &str) {
    if let Some(w) = w.upgrade() {
        util::show_error(&w.win, title, msg);
    }
}

/// Connects to a phone or camera and shows its files.
fn open_device(w: &Rc<Window>, v: &Volume, show: impl Fn(Location) + 'static) {
    let win = Rc::downgrade(w);
    let key = busy_key(v);
    set_busy(&key, true);
    super::phones::open(w.win.upcast_ref(), &v.device, move |res| {
        set_busy(&key, false);
        match res {
            Ok(loc) => show(loc),
            Err(msg) if msg.is_empty() => {}
            Err(msg) => error(&win, "Cannot open the device", &msg),
        }
    });
}

/// Closes the connection to a device that showed no storage and opens it again (once
/// the phone is unlocked, its storages show up).
pub fn reconnect_device(w: &Rc<Window>, pane: &Rc<Pane>, v: &Volume) {
    let (win, p) = (Rc::downgrade(w), Rc::downgrade(pane));
    let vol = v.clone();
    set_busy(&busy_key(v), true);
    super::phones::eject(w.win.upcast_ref(), &v.device, move |_| {
        set_busy(&busy_key(&vol), false);
        if let (Some(w), Some(p)) = (win.upgrade(), p.upgrade()) {
            open_device(&w, &vol, shower(&w, &p, false));
        }
    });
}

/// Unmounts (and for `eject`, powers off / ejects the whole drive, with its other volumes
/// among `all`). Tabs showing the drive are moved to the drives page first, so they do not
/// keep it busy.
#[cfg_attr(windows, allow(unreachable_code))]
pub fn unmount_volume(w: &Rc<Window>, v: &Volume, eject: bool, all: &[Volume]) {
    if v.is_device() {
        eject_device(w, v);
        return;
    }
    if eject && !v.loop_object.is_empty() {
        eject_image(w, v, all);
        return;
    }
    if !v.share_uri.is_empty() {
        let w2 = w.clone();
        super::shares::eject(w, v, move |mounts| leave(&w2, mounts));
        return;
    }
    #[cfg(windows)]
    return win_unmount_volume(w, v, eject);
    // Encrypted containers with LVM volumes inside: all their volumes are unmounted and
    // closed together (by the LVM helper), then the container is locked.
    let lvm: Vec<(String, String)> = all
        .iter()
        .filter(|x| x.lvm_inside && x.unlocked && if eject { x.drive_object == v.drive_object } else { x.block_object == v.lvm_backing })
        .map(|x| (x.block_object.clone(), x.device.clone()))
        .collect();
    let mut mounts: Vec<PathBuf> = if eject {
        all.iter().filter(|x| x.drive_object == v.drive_object).flat_map(|x| x.mount_points.clone()).collect()
    } else {
        v.mount_points.clone()
    };
    mounts.extend(all.iter().filter(|x| lvm.iter().any(|(o, _)| o == &x.lvm_backing)).flat_map(|x| x.mount_points.clone()));
    leave(w, &mounts);
    // Unmounted, and encrypted ones locked again (as other file managers do).
    let targets: Vec<(String, String)> = if eject {
        all.iter().filter(|x| x.drive_object == v.drive_object && !x.mount_points.is_empty() && x.lvm_backing.is_empty()).map(|x| (x.block_object.clone(), x.crypto_backing.clone())).collect()
    } else if v.lvm_backing.is_empty() {
        vec![(v.block_object.clone(), v.crypto_backing.clone())]
    } else {
        Vec::new()
    };
    // Unlocked but not mounted: still to be locked before powering off.
    let mut lock: Vec<String> = if eject {
        all.iter().filter(|x| x.drive_object == v.drive_object && x.unlocked && !targets.iter().any(|(_, b)| b == &x.block_object)).map(|x| x.block_object.clone()).collect()
    } else {
        lvm.iter().map(|(o, _)| o.clone()).collect()
    };
    let win = Rc::downgrade(w);
    let v = v.clone();
    set_busy(&v.block_object, true);
    glib::spawn_future_local(async move {
        for (_, dev) in &lvm {
            match lvm_helper("close", dev).await {
                Ok(_) => {}
                Err(e) => {
                    set_busy(&v.block_object, false);
                    if !e.is_empty() {
                        error(&win, "Cannot unmount drive", &e);
                    }
                    return;
                }
            }
        }
        for (obj, backing) in &targets {
            if let Err(e) = call(obj, "org.freedesktop.UDisks2.Filesystem", "Unmount", empty_options()).await {
                set_busy(&v.block_object, false);
                error(&win, "Cannot unmount drive", &e);
                return;
            }
            if !backing.is_empty() {
                lock.push(backing.clone());
            }
        }
        for obj in &lock {
            if let Err(e) = call(obj, "org.freedesktop.UDisks2.Encrypted", "Lock", empty_options()).await {
                set_busy(&v.block_object, false);
                error(&win, "Unmounted, but the encrypted drive could not be locked", &e);
                return;
            }
        }
        if eject {
            let method = if v.usb { "PowerOff" } else { "Eject" };
            if let Err(e) = call(&v.drive_object, "org.freedesktop.UDisks2.Drive", method, empty_options()).await {
                // Unmounted but not powered off: still safe, but say so.
                error(&win, "Drive unmounted, but could not be powered off", &e);
            }
        }
        set_busy(&v.block_object, false);
    });
}

/// Windows has no per-volume unmount: like Explorer's "Eject", a stick has its whole device
/// taken out by PnP (caches flushed first — safe to unplug afterwards) and an optical drive
/// ejects its disc; both buttons mean that here, so `eject` only picks the wording. The
/// self-test stand-ins answer in the UDisks2 shape on either system.
#[cfg(windows)]
fn win_unmount_volume(w: &Rc<Window>, v: &Volume, eject: bool) {
    leave(w, &v.mount_points);
    let win = Rc::downgrade(w);
    let v = v.clone();
    set_busy(&v.block_object, true);
    glib::spawn_future_local(async move {
        let unmounted = if v.block_object.starts_with("/fake") {
            call(&v.block_object, "org.freedesktop.UDisks2.Filesystem", "Unmount", empty_options()).await.map(|_| ())
        } else {
            Ok(())
        };
        let res = if v.block_object.starts_with("/fake") {
            match unmounted {
                Ok(()) if !v.crypto_backing.is_empty() => call(&v.crypto_backing, "org.freedesktop.UDisks2.Encrypted", "Lock", empty_options()).await.map(|_| ()),
                other => other,
            }
        } else if v.optical {
            gio::spawn_blocking(move || failbrauwser::win::storage::eject_media(&v.device)).await.map_err(|_| "the drive went away during the eject".to_string()).and_then(|r| r)
        } else {
            let device = v.device.clone();
            gio::spawn_blocking(move || failbrauwser::win::storage::safe_remove(&device)).await.map_err(|_| "the drive went away during the eject".to_string()).and_then(|r| r)
        };
        set_busy(&v.block_object, false);
        if let Err(e) = res {
            error(&win, if v.optical || eject { "Cannot eject the drive" } else { "Cannot unmount the drive" }, &e);
        }
    });
}

/// Unmounts every file system of a mounted disk image and detaches it from its loop device.
fn eject_image(w: &Rc<Window>, v: &Volume, all: &[Volume]) {
    let parts: Vec<Volume> = all.iter().filter(|x| x.loop_object == v.loop_object).cloned().collect();
    leave(w, &parts.iter().flat_map(|x| x.mount_points.clone()).collect::<Vec<_>>());
    let win = Rc::downgrade(w);
    let v = v.clone();
    set_busy(&v.block_object, true);
    glib::spawn_future_local(async move {
        for p in parts.iter().filter(|p| !p.mount_points.is_empty()) {
            if let Err(e) = call(&p.block_object, "org.freedesktop.UDisks2.Filesystem", "Unmount", empty_options()).await {
                set_busy(&v.block_object, false);
                error(&win, "Cannot unmount the image", &e);
                return;
            }
        }
        // Set to clear itself when unmounted; detached here in case it was not.
        let _ = call(&v.loop_object, "org.freedesktop.UDisks2.Loop", "Delete", empty_options()).await;
        set_busy(&v.block_object, false);
    });
}

/// Mounts a disk image file (`.iso`, `.img`, …) through a loop device, like a drive, and
/// opens it in the current tab. The loop device goes away again when the image is unmounted.
pub fn mount_image(w: &Rc<Window>, image: PathBuf) {
    let name = image.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let win = Rc::downgrade(w);
    let pane = Rc::downgrade(&w.current_pane());
    glib::spawn_future_local(async move {
        let fake = FAKES.with(|f| f.borrow().iter().find(|d| d.image.as_ref() == Some(&image)).map(|d| d.object()));
        let res = match fake {
            Some(obj) => mount(&obj).await,
            None => attach_and_mount(&image).await,
        };
        match res {
            Ok(dir) => {
                if let Some(p) = pane.upgrade() {
                    p.navigate(Location::Dir(dir));
                }
            }
            Err(e) => error(&win, &format!("Cannot mount “{name}”"), &e),
        }
    });
}

#[cfg(unix)] // the udisks2 tracking paths that ask for it are Unix-only until the drives milestone
async fn managed_objects(conn: &gio::DBusConnection) -> Result<glib::Variant, String> {
    conn.call_future(Some(UDISKS), "/org/freedesktop/UDisks2", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", None, Some(glib::VariantTy::new("(a{oa{sa{sv}}})").unwrap()), gio::DBusCallFlags::NONE, 5000)
        .await
        .map_err(|e| e.to_string())
}

/// Mount a disk image as a drive: udisks loop devices on Unix; the Windows twin uses the
/// native virtual-CD API with the drives milestone.
async fn attach_and_mount(image: &Path) -> Result<PathBuf, String> {
    #[cfg(unix)]
    return udisks_attach_and_mount(image).await;
    #[cfg(windows)]
    {
        let _ = image;
        Err("Mounting an image file as a drive is not built for Windows yet.".into())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = image;
        Err("No image mounting on this system.".into())
    }
}

#[cfg(unix)]
async fn udisks_attach_and_mount(image: &Path) -> Result<PathBuf, String> {
    let conn = gio::bus_get_future(gio::BusType::System).await.map_err(|e| e.to_string())?;
    let objects = managed_objects(&conn).await?;
    let loop_object = match drives::loop_for_file(&objects, image) {
        Some(l) => l,
        None => {
            // CD images are read-only anyway; others are written to when the file may be.
            let iso = image.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso"));
            let writable = !iso && std::fs::OpenOptions::new().read(true).write(true).open(image).is_ok();
            let file = std::fs::OpenOptions::new().read(true).write(writable).open(image).map_err(|e| e.to_string())?;
            let fds = gio::UnixFDList::new();
            let index = fds.append(&file).map_err(|e| e.to_string())?;
            let opts = glib::VariantDict::new(None);
            opts.insert("read-only", !writable);
            let args = glib::Variant::tuple_from_iter([glib::variant::Handle(index).to_variant(), opts.end()]);
            let (reply, _) = conn
                .call_with_unix_fd_list_future(Some(UDISKS), "/org/freedesktop/UDisks2/Manager", "org.freedesktop.UDisks2.Manager", "LoopSetup", Some(&args), Some(glib::VariantTy::new("(o)").unwrap()), auth_flags(), -1, Some(&fds))
                .await
                .map_err(|e| gio::DBusError::remote_error(&e).map(|_| strip_dbus_prefix(&e.to_string())).unwrap_or_else(|| e.to_string()))?;
            reply.child_value(0).str().unwrap_or_default().to_string()
        }
    };
    // UDisks2 looks at what is on the device a moment after setting it up.
    let mut found = Vec::new();
    for _ in 0..40 {
        let objects = managed_objects(&conn).await?;
        found = drives::loop_filesystems(&objects, &loop_object);
        if !found.is_empty() {
            break;
        }
        glib::timeout_future(std::time::Duration::from_millis(100)).await;
    }
    if found.is_empty() {
        let _ = call(&loop_object, "org.freedesktop.UDisks2.Loop", "Delete", empty_options()).await;
        return Err("There is no file system in it that Linux can mount. It still opens like a folder with a double click.".into());
    }
    let mut first = None;
    for (obj, mounted) in &found {
        let at = if *mounted {
            let objects = managed_objects(&conn).await?;
            drives::parse_udisks(&objects).into_iter().find(|v| &v.block_object == obj).and_then(|v| v.mount_point().map(|p| p.to_path_buf()))
        } else {
            // A file system the kernel will not write to (a journal it cannot replay, say)
            // still opens read-only.
            match mount(obj).await {
                Ok(p) => Some(p),
                Err(_) => {
                    let opts = glib::VariantDict::new(None);
                    opts.insert("options", "ro");
                    call(obj, "org.freedesktop.UDisks2.Filesystem", "Mount", glib::Variant::tuple_from_iter([opts.end()]))
                        .await
                        .ok()
                        .map(|r| PathBuf::from(r.child_value(0).get::<String>().unwrap_or_default()))
                }
            }
        };
        if first.is_none() {
            first = at;
        }
    }
    // Gone by itself once unmounted. Not earlier: the device would go as soon as UDisks2
    // closed it after looking at it, before anything is mounted.
    let _ = call(&loop_object, "org.freedesktop.UDisks2.Loop", "SetAutoclear", glib::Variant::tuple_from_iter([true.to_variant(), glib::VariantDict::new(None).end()])).await;
    first.ok_or_else(|| "The image is attached, but none of its file systems could be mounted.".to_string())
}

/// May UDisks2 ask for a password? Not when `FB_NO_AUTH_PROMPTS` is set (tests on a desktop
/// in use must not put up dialogs there).
fn auth_flags() -> gio::DBusCallFlags {
    if std::env::var_os("FB_NO_AUTH_PROMPTS").is_some() { gio::DBusCallFlags::NONE } else { gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION }
}

/// Closes the connection to a phone or camera; tabs showing its files go to the drives
/// page first.
fn eject_device(w: &Rc<Window>, v: &Volume) {
    if let Some(root) = super::phones::root_path(&v.device) {
        leave(w, &[root]);
    }
    // A WPD device has no fuse folder to leave — its tabs show wpd:// addresses instead.
    #[cfg(windows)]
    if let Some(pnp) = super::phones::wpd_pnp(&v.device) {
        leave_device_remotes(w, &pnp);
    }
    let win = Rc::downgrade(w);
    let key = busy_key(v);
    set_busy(&key, true);
    super::phones::eject(w.win.upcast_ref(), &v.device, move |res| {
        set_busy(&key, false);
        match res {
            Err(msg) if !msg.is_empty() => error(&win, "Cannot eject the device", &msg),
            _ => {}
        }
    });
}

/// Whether the volume can be unmounted (not the system's own, not `/`).
pub fn can_unmount(v: &Volume) -> bool {
    let is_root = v.mount_points.iter().any(|m| m == Path::new("/"));
    v.mount_point().is_some() && !is_root && !v.system && !v.block_object.is_empty()
}

/// Whether the volume can be ejected: a USB or ejectable drive, or an open phone.
pub fn can_eject(v: &Volume) -> bool {
    (v.usb || v.ejectable) && !v.drive_object.is_empty() || v.is_device() && v.mount_point().is_some() || !v.loop_object.is_empty() || !v.share_uri.is_empty() || wpd_storage(v)
}

/// A WPD row whose storage answered (rows for a storage-less device stay inert — ejecting
/// a phone that only has its screen locked would remove it until the next replug).
#[cfg(windows)]
fn wpd_storage(v: &Volume) -> bool {
    v.device.starts_with("wpd://") && !v.label.is_empty()
}

#[cfg(not(windows))]
fn wpd_storage(_: &Volume) -> bool {
    false
}

/// What marks a volume as busy: its UDisks2 object, or a device's address.
fn busy_key(v: &Volume) -> String {
    if v.is_device() { v.device.clone() } else { v.block_object.clone() }
}

/// A phone that is open but shows no storage (locked, or access not allowed yet).
pub fn no_storage(v: &Volume) -> bool {
    v.device_kind() == Some(DeviceKind::Phone) && v.mount_point().is_some() && v.label.is_empty()
}

/// Moves every tab showing a folder below one of `mounts` to the drives page.
fn leave(w: &Window, mounts: &[PathBuf]) {
    for win in w.app.windows() {
        for p in win.panes() {
            if p.location().nearest_dir().is_some_and(|d| mounts.iter().any(|m| d.starts_with(m))) {
                p.navigate(Location::Drives);
            }
        }
    }
}

/// The Windows twin of `leave`: tabs browsing one WPD device go to the drives page by
/// the device's PnP path instead of by a mount folder (there is no fuse mount to match).
#[cfg(windows)]
fn leave_device_remotes(w: &Window, pnp: &str) {
    for win in w.app.windows() {
        for p in win.panes() {
            if let Location::Remote(r) = p.location() {
                if matches!(&r.at, failbrauwser::location::RemoteAt::Wpd { pnp: device, .. } if device == pnp) {
                    p.navigate(Location::Drives);
                }
            }
        }
    }
}

fn empty_options() -> glib::Variant {
    glib::Variant::tuple_from_iter([glib::VariantDict::new(None).end()])
}

async fn call(object: &str, iface: &str, method: &str, args: glib::Variant) -> Result<glib::Variant, String> {
    if object.starts_with("/fake") {
        return fake_call(object, method, &args);
    }
    let conn = gio::bus_get_future(gio::BusType::System).await.map_err(|e| e.to_string())?;
    conn.call_future(Some(UDISKS), object, iface, method, Some(&args), None, auth_flags(), -1)
        .await
        .map_err(|e| gio::DBusError::remote_error(&e).map(|_| strip_dbus_prefix(&e.to_string())).unwrap_or_else(|| e.to_string()))
}

fn strip_dbus_prefix(msg: &str) -> String {
    // "GDBus.Error:org.freedesktop.UDisks2.Error.DeviceBusy: Error unmounting …: target is busy"
    msg.split_once(": ").map(|(_, rest)| rest.to_string()).unwrap_or_else(|| msg.to_string())
}

#[cfg(not(windows))]
async fn query_udisks() -> Option<Vec<Volume>> {
    let conn = gio::bus_get_future(gio::BusType::System).await.ok()?;
    let reply = conn
        .call_future(Some(UDISKS), "/org/freedesktop/UDisks2", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", None, Some(glib::VariantTy::new("(a{oa{sa{sv}}})").unwrap()), gio::DBusCallFlags::NONE, 5000)
        .await
        .ok()?;
    let mut vols = drives::parse_udisks(&reply);
    drives::link_lvm_volumes(&mut vols, Path::new("/sys"));
    Some(vols)
}
