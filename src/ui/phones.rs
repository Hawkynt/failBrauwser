//! Phones, tablets and cameras on USB (see [`failbrauwser::devices`]) as GVfs's volume
//! monitor sees them: listed with the drives under their own name, one row per storage with
//! its fill level, opened (mounted) and safely removed from there.

use failbrauwser::devices::{self, DeviceKind};
use failbrauwser::drives::{Usage, Volume};
use failbrauwser::location::Location;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

/// A connected device.
#[derive(Clone)]
pub struct Device {
    pub name: String,
    pub kind: DeviceKind,
    /// Its address (`mtp://Google_Pixel_7_…/`), how it is found again later.
    pub uri: String,
    volume: Option<gio::Volume>,
    mount: Option<gio::Mount>,
    /// A stand-in for a phone in the self-test: a local folder, and whether it is open.
    fake: Option<(PathBuf, bool)>,
}

impl Device {
    /// The top of its files once it is open.
    fn root(&self) -> Option<gio::File> {
        if let Some((p, open)) = &self.fake {
            return open.then(|| gio::File::for_path(p));
        }
        self.mount.as_ref().map(|m| m.root())
    }
}

thread_local! {
    static FAKES: RefCell<Vec<(String, PathBuf, bool)>> = const { RefCell::new(Vec::new()) };
    static WATCHERS: RefCell<Vec<Box<dyn Fn() -> bool>>> = const { RefCell::new(Vec::new()) };
    static MONITOR: RefCell<Option<gio::VolumeMonitor>> = const { RefCell::new(None) };
}

fn same_uri(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The device and storage one `wpd://…` row addresses (Windows; see `failbrauwser::wpd`).
#[cfg(windows)]
fn wpd_target(uri: &str) -> Option<(String, String)> {
    match failbrauwser::location::RemoteAt::parse_uri(uri) {
        Some((failbrauwser::location::RemoteAt::Wpd { pnp, storage }, _)) => Some((pnp, storage)),
        _ => None,
    }
}

/// The PnP path behind a WPD row — the drives page sweeps tabs off a device before ejecting.
#[cfg(windows)]
pub fn wpd_pnp(uri: &str) -> Option<String> {
    wpd_target(uri).map(|(pnp, _)| pnp)
}

/// The devices connected now.
pub fn connected() -> Vec<Device> {
    let mon = gio::VolumeMonitor::get();
    let mut out: Vec<Device> = Vec::new();
    for v in mon.volumes() {
        let Some(uri) = v.activation_root().map(|f| f.uri().to_string()) else { continue };
        let Some(kind) = DeviceKind::of_uri(&uri) else { continue };
        out.push(Device { name: v.name().to_string(), kind, uri, mount: v.get_mount(), volume: Some(v), fake: None });
    }
    // Opened by address (`mtp://…` typed in), without a volume of its own.
    for m in mon.mounts() {
        let uri = m.root().uri().to_string();
        let Some(kind) = DeviceKind::of_uri(&uri) else { continue };
        if !out.iter().any(|d| same_uri(&d.uri, &uri)) {
            out.push(Device { name: m.name().to_string(), kind, uri, volume: None, mount: Some(m), fake: None });
        }
    }
    // Windows sees phones and cameras through WPD instead (one row per storage; the
    // walk itself lives in `failbrauwser::wpd`). Their object ids learn friendly names
    // so titles and the path bar can speak human.
    #[cfg(windows)]
    {
        let mut names = Vec::new();
        for d in failbrauwser::wpd::devices().unwrap_or_default() {
            let storages = failbrauwser::wpd::storages(&d.pnp).unwrap_or_default();
            if storages.is_empty() {
                out.push(Device { name: d.name.clone(), kind: DeviceKind::Camera, uri: format!("wpd://{}/", d.pnp), volume: None, mount: None, fake: None });
                continue;
            }
            for s in &storages {
                names.push((s.id.clone(), format!("{} · {}", d.name, s.name)));
                out.push(Device { name: format!("{} · {}", d.name, s.name), kind: DeviceKind::Phone, uri: format!("wpd://{}/{}/", d.pnp, s.id), volume: None, mount: None, fake: None });
            }
        }
        devices::set_named_storages(names);
    }
    FAKES.with(|f| {
        for (name, dir, open) in f.borrow().iter() {
            let uri = gio::File::for_path(dir).uri().to_string();
            out.push(Device { name: name.clone(), kind: DeviceKind::Phone, uri, volume: None, mount: None, fake: Some((dir.clone(), *open)) });
        }
    });
    out
}

fn find(uri: &str) -> Option<Device> {
    connected().into_iter().find(|d| same_uri(&d.uri, uri))
}

/// Tells tab titles and the path bar which folders are devices.
fn name_roots(devs: &[Device]) {
    let roots = devs.iter().filter_map(|d| Some((d.root()?.path()?, d.name.clone(), d.kind))).collect();
    devices::set_named_roots(roots);
}

/// Follows devices coming and going (once per process).
pub fn watch() {
    MONITOR.with(|m| {
        if m.borrow().is_some() {
            return;
        }
        let mon = gio::VolumeMonitor::get();
        mon.connect_volume_added(|_, _| changed());
        mon.connect_volume_removed(|_, _| changed());
        mon.connect_volume_changed(|_, _| changed());
        mon.connect_mount_added(|_, _| changed());
        mon.connect_mount_removed(|_, _| changed());
        *m.borrow_mut() = Some(mon);
    });
    name_roots(&connected());
}

/// Calls `f` whenever devices come, go, open or close, until it returns false.
pub fn connect_changed(f: impl Fn() -> bool + 'static) {
    WATCHERS.with(|w| w.borrow_mut().push(Box::new(f)));
}

fn changed() {
    name_roots(&connected());
    let watchers = WATCHERS.with(|w| std::mem::take(&mut *w.borrow_mut()));
    let kept: Vec<_> = watchers.into_iter().filter(|f| f()).collect();
    WATCHERS.with(|w| {
        let mut w = w.borrow_mut();
        let added = std::mem::take(&mut *w);
        *w = kept;
        w.extend(added);
    });
}

/// A folder standing in for a phone (self-test): its subfolders are the storages.
pub fn add_fake(name: &str, dir: PathBuf) {
    FAKES.with(|f| f.borrow_mut().push((name.to_string(), dir, true)));
    changed();
}

/// The rows for the drives page: per device one per storage (with its fill level), or one
/// for the device itself while it is closed or shows no storage.
pub async fn volumes() -> Vec<(Volume, Option<Usage>)> {
    let mut out = Vec::new();
    for d in connected() {
        let base = Volume {
            device: d.uri.clone(),
            drive_name: d.name.clone(),
            protocol: d.kind.protocol().into(),
            removable: true,
            usb: true,
            ejectable: true,
            bus: "usb".into(),
            rotation_rate: -1,
            ..Default::default()
        };
        // A WPD storage answers for its own fill; no filesystem query can ask a device.
        #[cfg(windows)]
        if let Some((pnp, id)) = wpd_target(&d.uri) {
            match failbrauwser::wpd::storages(&pnp).into_iter().flatten().find(|s| s.id == id) {
                Some(s) => {
                    let usage = Usage { total: s.total, used: s.total.saturating_sub(s.free), free: s.free.min(s.total) };
                    // A read-only storage still browses; its refusals surface when writing.
                    out.push((Volume { label: s.name, size: s.total, mountable: true, ..base }, Some(usage)));
                }
                None => out.push((Volume { mountable: false, ..base }, None)),
            }
            continue;
        }
        let Some(root) = d.root() else {
            out.push((Volume { mountable: d.fake.is_some() || d.volume.as_ref().is_some_and(|v| v.can_mount()), ..base }, None));
            continue;
        };
        let root_path = root.path().unwrap_or_default();
        // An MTP device's top folders are its storages; others show their files right there.
        let storages = if d.kind == DeviceKind::Phone { subfolders(&root).await } else { vec![(String::new(), root.clone())] };
        if storages.is_empty() {
            out.push((Volume { mount_points: vec![root_path], ..base }, None));
            continue;
        }
        for (name, file) in storages {
            let usage = fs_usage(&file).await;
            let path = file.path().unwrap_or_else(|| root_path.clone());
            out.push((Volume { label: name, size: usage.map_or(0, |u| u.total), mount_points: vec![path], ..base.clone() }, usage));
        }
    }
    out
}

/// A device that does not answer must not hold up the page.
const ASK_TIMEOUT: Duration = Duration::from_secs(5);

async fn subfolders(dir: &gio::File) -> Vec<(String, gio::File)> {
    let list = async {
        let e = dir.enumerate_children_future("standard::name,standard::display-name,standard::type", gio::FileQueryInfoFlags::NONE, glib::Priority::DEFAULT).await.ok()?;
        let mut out = Vec::new();
        loop {
            let infos = e.next_files_future(32, glib::Priority::DEFAULT).await.ok()?;
            if infos.is_empty() {
                break;
            }
            for i in infos {
                if i.file_type() == gio::FileType::Directory {
                    out.push((i.display_name().to_string(), dir.child(i.name())));
                }
            }
        }
        Some(out)
    };
    let mut v = glib::future_with_timeout(ASK_TIMEOUT, list).await.ok().flatten().unwrap_or_default();
    v.sort_by(|a, b| failbrauwser::fs::format::natural_cmp(&a.0, &b.0));
    v
}

pub(super) async fn fs_usage(f: &gio::File) -> Option<Usage> {
    let ask = f.query_filesystem_info_future("filesystem::size,filesystem::free,filesystem::used", glib::Priority::DEFAULT);
    let info = glib::future_with_timeout(ASK_TIMEOUT, ask).await.ok()?.ok()?;
    let total = info.attribute_uint64("filesystem::size");
    if total == 0 {
        return None;
    }
    let free = info.attribute_uint64("filesystem::free").min(total);
    let used = if info.has_attribute("filesystem::used") { info.attribute_uint64("filesystem::used") } else { total - free };
    Some(Usage { total, used, free })
}

/// Opens (mounts) a device; gives the folder to show: its only storage, else its top.
/// A WPD device needs no mount — its storage *is* the address the pane browses.
pub fn open(parent: &gtk::Window, uri: &str, done: impl FnOnce(Result<Location, String>) + 'static) {
    #[cfg(windows)]
    if let Some((pnp, storage)) = wpd_target(uri) {
        let at = failbrauwser::location::RemoteAt::Wpd { pnp, storage };
        done(Ok(Location::Remote(failbrauwser::location::RemoteLoc::root(at))));
        return;
    }
    let Some(d) = find(uri) else {
        done(Err("The device is no longer connected.".into()));
        return;
    };
    let kind = d.kind;
    if d.fake.is_some() {
        set_fake_open(uri, true);
    }
    if let Some(root) = find(uri).and_then(|d| d.root()) {
        glib::spawn_future_local(async move { done(folder_to_show(kind, root).await.map(Location::Dir)) });
        return;
    }
    let Some(volume) = d.volume.clone() else {
        done(Err("The device cannot be opened.".into()));
        return;
    };
    let op = gtk::MountOperation::new(Some(parent));
    let v = volume.clone();
    volume.mount(gio::MountMountFlags::NONE, Some(&op), gio::Cancellable::NONE, move |res| match res {
        Ok(()) => match v.get_mount() {
            Some(m) => {
                glib::spawn_future_local(async move { done(folder_to_show(kind, m.root()).await.map(Location::Dir)) });
            }
            None => done(Err("The device was opened but shows nothing.".into())),
        },
        // The user cancelled a question: nothing to say.
        Err(e) if e.matches(gio::IOErrorEnum::FailedHandled) => done(Err(String::new())),
        Err(e) => done(Err(devices::explain_error(kind, e.message()))),
    });
}

async fn folder_to_show(kind: DeviceKind, root: gio::File) -> Result<PathBuf, String> {
    let path = root.path().ok_or("The device was opened but has no local folder (is gvfsd-fuse running?).")?;
    if kind != DeviceKind::Phone {
        return Ok(path);
    }
    match subfolders(&root).await.as_slice() {
        [(_, only)] => Ok(only.path().unwrap_or(path)),
        [] => Err(devices::explain_error(kind, "The phone shows no storage.")),
        _ => Ok(path),
    }
}

/// Closes the connection to a device, so it can be unplugged.
pub fn eject(parent: &gtk::Window, uri: &str, done: impl FnOnce(Result<(), String>) + 'static) {
    // WPD: the device stack itself is asked to stop and remove the device subtree —
    // the same path Explorer's "Safely Remove Hardware" takes. Blocking, off the UI thread.
    #[cfg(windows)]
    if let Some((pnp, _)) = wpd_target(uri) {
        // The device stack may chew on open handles for a while: ask off-thread and check
        // back like the chooser's progress polls do.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(failbrauwser::wpd::eject(&pnp).map_err(|e| e.to_string()));
        });
        let mut done = Some(done);
        glib::timeout_add_local(std::time::Duration::from_millis(150), move || match rx.try_recv() {
            Ok(outcome) => {
                if let Some(done) = done.take() {
                    done(outcome);
                }
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => {
                if let Some(done) = done.take() {
                    done(Err("the eject worker died".into()));
                }
                glib::ControlFlow::Break
            }
        });
        return;
    }
    let Some(d) = find(uri) else {
        done(Ok(()));
        return;
    };
    if d.fake.is_some() {
        set_fake_open(uri, false);
        done(Ok(()));
        return;
    }
    let op = gtk::MountOperation::new(Some(parent));
    let cb = move |res: Result<(), glib::Error>| match res {
        Ok(()) => done(Ok(())),
        Err(e) if e.matches(gio::IOErrorEnum::FailedHandled) => done(Err(String::new())),
        Err(e) => done(Err(e.to_string())),
    };
    match (&d.mount, &d.volume) {
        (Some(m), _) if m.can_eject() => m.eject_with_operation(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, cb),
        (Some(m), _) => m.unmount_with_operation(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, cb),
        (None, Some(v)) if v.can_eject() => v.eject_with_operation(gio::MountUnmountFlags::NONE, Some(&op), gio::Cancellable::NONE, cb),
        _ => cb(Ok(())),
    }
}

fn set_fake_open(uri: &str, open: bool) {
    FAKES.with(|f| {
        for (_, dir, o) in f.borrow_mut().iter_mut() {
            if same_uri(&gio::File::for_path(&*dir).uri(), uri) {
                *o = open;
            }
        }
    });
    changed();
}

/// The folder all of a device's files are below (to move tabs away before it closes).
pub fn root_path(uri: &str) -> Option<PathBuf> {
    find(uri)?.root()?.path()
}
