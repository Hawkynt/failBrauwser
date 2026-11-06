//! Copy / cut / paste through the system clipboard, in the format other file managers use,
//! so files can be pasted between failBrauwser, Thunar, Nautilus and friends. Entries inside
//! archives have no path other programs could use; they live in-process only.

use failbrauwser::clipformat::{COPIED_FILES, format_copied_files, parse_copied_files, uri_to_path};
use failbrauwser::location::ArchiveLoc;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum ClipSource {
    Local(PathBuf),
    /// An entry (file or folder) inside an archive: the archive's folder and the entry path.
    Archive(ArchiveLoc, String),
    /// An entry on a remote server: the server, the path from its sign-in folder, and what
    /// kind it is (so the walk does not have to ask).
    Remote(failbrauwser::location::RemoteAt, String, bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClipContent {
    pub cut: bool,
    pub items: Vec<ClipSource>,
}

impl ClipContent {
    pub fn local_paths(&self) -> Vec<PathBuf> {
        self.items
            .iter()
            .filter_map(|i| match i {
                ClipSource::Local(p) => Some(p.clone()),
                _ => None,
            })
            .collect()
    }
}

thread_local! {
    static OWNED: RefCell<Option<(u64, ClipContent)>> = const { RefCell::new(None) };
    static SERIAL: Cell<u64> = const { Cell::new(0) };
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
}

/// Lives inside the clipboard's data callback; GTK drops it when another program takes
/// over the clipboard, which is how we learn we no longer own it.
struct OwnerGuard(u64);

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        let serial = self.0;
        let cleared = OWNED.with(|o| {
            let mut o = o.borrow_mut();
            if o.as_ref().is_some_and(|(s, _)| *s == serial) {
                *o = None;
                true
            } else {
                false
            }
        });
        if cleared {
            // Not from inside GTK's clipboard code: listeners may touch the clipboard.
            glib::idle_add_local_once(notify);
        }
    }
}

fn notify() {
    LISTENERS.with(|l| {
        for f in l.borrow().iter() {
            f();
        }
    });
}

/// Called whenever the clipboard content changes (to dim cut files).
pub fn connect_changed(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

fn clipboard() -> gtk::Clipboard {
    gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD)
}

pub fn set(content: ClipContent) {
    let serial = SERIAL.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let paths = content.local_paths();
    let special = format_copied_files(&paths, content.cut);
    let uris: Vec<String> = paths.iter().filter_map(|p| glib::filename_to_uri(p, None).ok().map(|u| u.to_string())).collect();
    let text = content
        .items
        .iter()
        .map(|i| match i {
            ClipSource::Local(p) => p.to_string_lossy().into_owned(),
            ClipSource::Archive(a, e) => format!("{}/{}", failbrauwser::location::Location::Archive(a.clone()).display(), e.rsplit('/').next().unwrap_or(e)),
            ClipSource::Remote(t, e, _) => t.uri(e),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let targets = [
        gtk::TargetEntry::new(COPIED_FILES, gtk::TargetFlags::empty(), 0),
        gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 1),
        gtk::TargetEntry::new("UTF8_STRING", gtk::TargetFlags::empty(), 2),
        gtk::TargetEntry::new("text/plain;charset=utf-8", gtk::TargetFlags::empty(), 2),
        gtk::TargetEntry::new("text/plain", gtk::TargetFlags::empty(), 2),
    ];
    // Owned before the call: GTK drops the previous owner's guard inside set_with_data.
    let guard = OwnerGuard(serial);
    let ok = clipboard().set_with_data(&targets, move |_, sel, info| {
        let _keep = &guard;
        match info {
            0 => sel.set(&gdk::Atom::intern(COPIED_FILES), 8, special.as_bytes()),
            1 => {
                let refs: Vec<&str> = uris.iter().map(String::as_str).collect();
                sel.set_uris(&refs);
            }
            _ => {
                sel.set_text(&text);
            }
        }
    });
    if ok {
        OWNED.with(|o| *o.borrow_mut() = Some((serial, content)));
    }
    notify();
}

/// What we put on the clipboard, while it is still ours.
pub fn owned() -> Option<ClipContent> {
    OWNED.with(|o| o.borrow().as_ref().map(|(_, c)| c.clone()))
}

/// Paths currently cut to the clipboard (shown dimmed).
pub fn cut_paths() -> Vec<PathBuf> {
    owned().filter(|c| c.cut).map(|c| c.local_paths()).unwrap_or_default()
}

/// After a cut-and-paste the clipboard must not offer the moved files again.
pub fn clear_if_cut() {
    if owned().is_some_and(|c| c.cut) {
        OWNED.with(|o| *o.borrow_mut() = None);
        clipboard().clear();
        notify();
    }
}

/// Reads the clipboard: ours directly, otherwise files copied in another program.
pub fn get(cb: impl FnOnce(Option<ClipContent>) + 'static) {
    if let Some(c) = owned() {
        cb(Some(c));
        return;
    }
    let cb = RefCell::new(Some(cb));
    clipboard().request_contents(&gdk::Atom::intern(COPIED_FILES), move |clip, sel| {
        let data = sel.data();
        if let Some((cut, paths)) = std::str::from_utf8(&data).ok().and_then(parse_copied_files) {
            if let Some(f) = cb.borrow_mut().take() {
                f(Some(ClipContent { cut, items: paths.into_iter().map(ClipSource::Local).collect() }));
            }
            return;
        }
        let cb = RefCell::new(cb.borrow_mut().take());
        clip.request_uris(move |clip, uris| {
            let paths: Vec<PathBuf> = uris.iter().filter_map(|u| uri_to_path(u)).collect();
            if !paths.is_empty() {
                if let Some(f) = cb.borrow_mut().take() {
                    f(Some(ClipContent { cut: false, items: paths.into_iter().map(ClipSource::Local).collect() }));
                }
                return;
            }
            let cb = RefCell::new(cb.borrow_mut().take());
            clip.request_text(move |_, text| {
                let paths: Vec<PathBuf> = text.map(|t| t.lines().filter_map(uri_to_path).filter(|p| p.exists()).collect()).unwrap_or_default();
                if let Some(f) = cb.borrow_mut().take() {
                    f(if paths.is_empty() { None } else { Some(ClipContent { cut: false, items: paths.into_iter().map(ClipSource::Local).collect() }) });
                }
            });
        });
    });
}
