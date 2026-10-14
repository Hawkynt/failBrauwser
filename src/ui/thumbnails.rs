//! Thumbnails for the rows in view. The shared cache is read on worker threads; what is
//! missing goes to the desktop's thumbnailer service (Tumbler, as in Thunar) over D-Bus, or,
//! without one, image thumbnails are made here. Only rows on screen are asked for.

use super::pane::Pane;
use super::window::Window;
use failbrauwser::location::Location;
use failbrauwser::thumbs::{self, Flavor};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, mpsc};

const TUMBLER: &str = "org.freedesktop.thumbnails.Thumbnailer1";
const TUMBLER_PATH: &str = "/org/freedesktop/thumbnails/Thumbnailer1";

#[derive(Clone)]
struct Request {
    pane: Weak<Pane>,
    generation: u64,
    name: OsString,
    path: PathBuf,
    mtime: i64,
    mime: String,
    flavor: Flavor,
    /// The thumbnailer service already reported this one as made.
    from_service: bool,
}

enum Outcome {
    /// With "smaller than wanted" (a 128 px thumbnail where 256 px are shown).
    Found(Pixbuf, bool),
    /// Not in the cache: the thumbnailer service should make it.
    Missing,
    Failed,
}

/// Worker job: look in the cache (and, when `generate`, make image thumbnails).
struct Job {
    id: u64,
    path: PathBuf,
    mtime: i64,
    mime: String,
    flavor: Flavor,
    generate: bool,
}

/// Pixel data crossing from a worker (images themselves stay on the UI thread).
struct Raw {
    bytes: glib::Bytes,
    width: i32,
    height: i32,
    stride: i32,
    alpha: bool,
}

impl Raw {
    fn from(pb: &Pixbuf) -> Raw {
        Raw { bytes: pb.read_pixel_bytes(), width: pb.width(), height: pb.height(), stride: pb.rowstride(), alpha: pb.has_alpha() }
    }
    fn into_pixbuf(self) -> Pixbuf {
        Pixbuf::from_bytes(&self.bytes, gtk::gdk_pixbuf::Colorspace::Rgb, self.alpha, 8, self.width, self.height, self.stride)
    }
}

/// (id, may generate, Ok((thumb, smaller than wanted)) | Err(missing?))
type Reply = (u64, bool, Result<(Raw, bool), bool>);

struct Service {
    jobs: mpsc::Sender<Job>,
    pending: RefCell<HashMap<u64, (Request, bool)>>,
    next_id: Cell<u64>,
    /// Tumbler requests by URI.
    by_uri: RefCell<HashMap<String, Vec<Request>>>,
    tumbler_ok: Cell<Option<bool>>,
    batch: RefCell<Vec<Request>>,
    batch_scheduled: Cell<bool>,
    _signals: RefCell<Option<gio::SignalSubscription>>,
}

thread_local! {
    static SERVICE: RefCell<Option<Rc<Service>>> = const { RefCell::new(None) };
    /// Per pane: what was already asked for, and which rows hold a thumbnail (oldest first).
    static ASKED: RefCell<HashMap<usize, (u64, HashSet<(OsString, i64, Flavor)>)>> = RefCell::new(HashMap::new());
    static FIT: RefCell<HashMap<(usize, i32), Pixbuf>> = RefCell::new(HashMap::new());
}

fn service() -> Rc<Service> {
    SERVICE.with(|s| {
        if let Some(x) = s.borrow().as_ref() {
            return x.clone();
        }
        let (jtx, jrx) = mpsc::channel::<Job>();
        let (rtx, rrx) = async_channel::unbounded::<Reply>();
        let jrx = Arc::new(Mutex::new(jrx));
        for i in 0..2 {
            let (jrx, rtx) = (jrx.clone(), rtx.clone());
            std::thread::Builder::new()
                .name(format!("fb-thumb-{i}"))
                .spawn(move || {
                    let root = thumbs::cache_root();
                    loop {
                        let job = { jrx.lock().unwrap().recv() };
                        let Ok(j) = job else { return };
                        let can_make = j.generate && j.mime.starts_with("image/");
                        let result = match thumbs::load(&root, &j.path, j.mtime, j.flavor) {
                            // Only the small one while a large one is wanted: make that.
                            Some((small, f)) if f != j.flavor && f == Flavor::Normal && can_make => {
                                let pb = thumbs::generate(&root, &j.path, j.mtime, j.flavor).unwrap_or(small);
                                Ok((Raw::from(&pb), false))
                            }
                            Some((pb, f)) => Ok((Raw::from(&pb), f != j.flavor && f == Flavor::Normal)),
                            None if thumbs::failed_before(&root, &j.path, j.mtime) => Err(false),
                            None if can_make => thumbs::generate(&root, &j.path, j.mtime, j.flavor).map(|pb| (Raw::from(&pb), false)).ok_or(false),
                            None => Err(true),
                        };
                        if rtx.send_blocking((j.id, j.generate, result)).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn thumbnail worker");
        }
        let svc = Rc::new(Service {
            jobs: jtx,
            pending: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
            by_uri: RefCell::new(HashMap::new()),
            // FB_NO_TUMBLER=1 uses the built-in image thumbnailer only (tests).
            tumbler_ok: Cell::new(if std::env::var_os("FB_NO_TUMBLER").is_some() { Some(false) } else { None }),
            batch: RefCell::new(Vec::new()),
            batch_scheduled: Cell::new(false),
            _signals: RefCell::new(None),
        });
        let weak = Rc::downgrade(&svc);
        glib::spawn_future_local(async move {
            while let Ok((id, generated, result)) = rrx.recv().await {
                let Some(svc) = weak.upgrade() else { return };
                let Some((req, _)) = svc.pending.borrow_mut().remove(&id) else { continue };
                let outcome = match result {
                    Ok((raw, smaller)) => Outcome::Found(raw.into_pixbuf(), smaller),
                    Err(true) => Outcome::Missing,
                    Err(false) => Outcome::Failed,
                };
                svc.handle(req, generated, outcome);
            }
        });
        *s.borrow_mut() = Some(svc.clone());
        svc
    })
}

impl Service {
    fn submit(&self, req: Request, generate: bool) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let job = Job { id, path: req.path.clone(), mtime: req.mtime, mime: req.mime.clone(), flavor: req.flavor, generate };
        self.pending.borrow_mut().insert(id, (req, generate));
        let _ = self.jobs.send(job);
    }

    fn handle(self: &Rc<Self>, req: Request, generated: bool, outcome: Outcome) {
        match outcome {
            Outcome::Found(pb, smaller) => {
                debug(|| format!("found {:?}{}", req.name, if smaller { " (smaller than wanted)" } else { "" }));
                deliver(&req, pb);
                // Shown for now; the wanted size is asked for once (never again for an answer
                // of the service itself: no request loops).
                if smaller && !generated && !req.from_service {
                    match self.tumbler_ok.get() {
                        Some(false) => self.submit(req, true),
                        _ => self.queue_tumbler(req),
                    }
                }
            }
            Outcome::Failed => {}
            Outcome::Missing if generated => {}
            // Reported ready but not usable: do not ask again (no request loops).
            Outcome::Missing if req.from_service => debug(|| format!("unusable thumbnail for {:?}", req.name)),
            Outcome::Missing => match self.tumbler_ok.get() {
                // No service: make image thumbnails ourselves.
                Some(false) => self.submit(req, true),
                _ => self.queue_tumbler(req),
            },
        }
    }

    /// Collects requests for a moment and sends them to Tumbler in one call.
    fn queue_tumbler(self: &Rc<Self>, req: Request) {
        self.batch.borrow_mut().push(req);
        if self.batch_scheduled.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
            if let Some(s) = weak.upgrade() {
                s.batch_scheduled.set(false);
                s.send_batch();
            }
        });
    }

    fn send_batch(self: &Rc<Self>) {
        let reqs: Vec<Request> = std::mem::take(&mut *self.batch.borrow_mut());
        if reqs.is_empty() {
            return;
        }
        let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
            self.tumbler_ok.set(Some(false));
            for r in reqs {
                self.submit(r, true);
            }
            return;
        };
        self.ensure_signals(&conn);
        let flavor = reqs[0].flavor;
        let mut uris = Vec::new();
        let mut mimes = Vec::new();
        for r in &reqs {
            let Some(uri) = thumbs::file_uri(&r.path) else { continue };
            self.by_uri.borrow_mut().entry(uri.clone()).or_default().push(r.clone());
            uris.push(uri);
            mimes.push(r.mime.clone());
        }
        let args = (uris, mimes, flavor.name().to_string(), "foreground".to_string(), 0u32).to_variant();
        debug(|| format!("queue {} uris ({})", uris_len(&args), flavor.name()));
        let weak = Rc::downgrade(self);
        conn.call(Some(TUMBLER), TUMBLER_PATH, TUMBLER, "Queue", Some(&args), None, gio::DBusCallFlags::NONE, 10_000, gio::Cancellable::NONE, move |res| {
            let Some(s) = weak.upgrade() else { return };
            match res {
                Ok(_) => s.tumbler_ok.set(Some(true)),
                Err(e) => {
                    debug(|| format!("queue failed: {e}"));
                    // No thumbnailer service: fall back to our own image thumbnails.
                    s.tumbler_ok.set(Some(false));
                    let waiting: Vec<Request> = s.by_uri.borrow_mut().drain().flat_map(|(_, v)| v).collect();
                    for r in waiting {
                        s.submit(r, true);
                    }
                }
            }
        });
    }

    fn ensure_signals(self: &Rc<Self>, conn: &gio::DBusConnection) {
        if self._signals.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        // Matched by interface and path, not by the service's name: it is started by our
        // first call, and signals from a name owned only later would not match.
        let sub = conn.subscribe_to_signal(None, Some(TUMBLER), None, Some(TUMBLER_PATH), None, gio::DBusSignalFlags::NONE, move |sig| {
            let Some(s) = weak.upgrade() else { return };
            let uris: Vec<String> = match sig.signal_name {
                "Ready" | "Error" => sig.parameters.child_value(1).get::<Vec<String>>().unwrap_or_default(),
                _ => return,
            };
            let ready = sig.signal_name == "Ready";
            debug(|| format!("{} for {} uris", sig.signal_name, uris.len()));
            for uri in uris {
                let Some(reqs) = s.by_uri.borrow_mut().remove(&uri) else { continue };
                if ready {
                    // Written to the cache by Tumbler: read it from there.
                    for mut r in reqs {
                        r.from_service = true;
                        s.submit(r, false);
                    }
                }
            }
        });
        *self._signals.borrow_mut() = Some(sub);
    }
}

/// `FB_DEBUG_THUMBS=1` traces the thumbnail traffic on stderr.
fn debug(msg: impl FnOnce() -> String) {
    if std::env::var_os("FB_DEBUG_THUMBS").is_some() {
        eprintln!("thumbs: {}", msg());
    }
}

fn deliver(req: &Request, pb: Pixbuf) {
    let Some(p) = req.pane.upgrade() else { return };
    if p.generation() == req.generation {
        p.set_thumbnail(&req.name, pb);
    }
}

fn uris_len(args: &glib::Variant) -> usize {
    args.child_value(0).n_children()
}

/// Scales a thumbnail to fit `size` × `size` (cached; thumbnails are drawn often).
pub fn fit(pb: &Pixbuf, size: i32) -> Pixbuf {
    let (w, h) = (pb.width(), pb.height());
    if w <= size && h <= size {
        return pb.clone();
    }
    let key = (pb.as_ptr() as usize, size);
    if let Some(hit) = FIT.with(|f| f.borrow().get(&key).cloned()) {
        return hit;
    }
    let scale = size as f64 / w.max(h) as f64;
    let (nw, nh) = (((w as f64 * scale).round() as i32).max(1), ((h as f64 * scale).round() as i32).max(1));
    let scaled = pb.scale_simple(nw, nh, gtk::gdk_pixbuf::InterpType::Bilinear).unwrap_or_else(|| pb.clone());
    FIT.with(|f| {
        let mut f = f.borrow_mut();
        if f.len() > 1500 {
            f.clear();
        }
        f.insert(key, scaled.clone());
    });
    scaled
}

pub fn clear_cache() {
    FIT.with(|f| f.borrow_mut().clear());
}

/// Thumbnails make sense here: enabled, icons big enough, local files.
fn wanted(w: &Window, pane: &Pane) -> Option<Flavor> {
    let s = w.app.settings.borrow();
    if !s.show_thumbnails {
        return None;
    }
    let px = s.icon_size();
    if px < 32 {
        return None;
    }
    match pane.location() {
        Location::Dir(p) => {
            // Network shares: reading every image for a thumbnail would be slow.
            if failbrauwser::ops::device::classify(&p) == failbrauwser::ops::device::DeviceClass::Network {
                return None;
            }
        }
        Location::Trash | Location::Search(_) => {}
        _ => return None,
    }
    Some(Flavor::for_size(px))
}

/// Asks for thumbnails of the rows in view (after loading, scrolling, zooming).
pub fn update(w: &Window, pane: &Rc<Pane>) {
    let Some(flavor) = wanted(w, pane) else {
        debug(|| "not wanted here".into());
        return;
    };
    let generation = pane.generation();
    let key = Rc::as_ptr(pane) as usize;
    let svc = service();
    let items = pane.visible_thumbnail_candidates();
    debug(|| format!("update: {} rows in view", items.len()));
    ASKED.with(|a| {
        let mut a = a.borrow_mut();
        a.retain(|_, _| true);
        let entry = a.entry(key).or_insert_with(|| (generation, HashSet::new()));
        if entry.0 != generation {
            *entry = (generation, HashSet::new());
        }
        for (name, path, mtime, mime) in items {
            if !thumbs::is_candidate(&mime) || !entry.1.insert((name.clone(), mtime, flavor)) {
                continue;
            }
            svc.submit(Request { pane: Rc::downgrade(pane), generation, name, path, mtime, mime, flavor, from_service: false }, false);
        }
    });
}

/// Forget what a pane asked for (its rows were rebuilt, or thumbnails were switched on).
pub fn reset(pane: &Rc<Pane>) {
    let key = Rc::as_ptr(pane) as usize;
    ASKED.with(|a| {
        a.borrow_mut().remove(&key);
    });
}
