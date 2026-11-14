//! The images of the Git integration, drawn for it (as SVG, built into the program): the
//! overlay icons on files and folders, in TortoiseGit's colors and places, and the icons of
//! its menu items and dialogs.

use failbrauwser::vcs::git::Status;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;

/// A white rim so an overlay stands out on any icon.
const RIM: &str = r##"stroke="#ffffff" stroke-width="1.2""##;

/// The overlay drawn on files and folders in this state (16×16).
fn overlay_svg(s: Status) -> String {
    let body = match s {
        // Green circle with a white tick.
        Status::Normal => format!(r##"<circle cx="8" cy="8" r="7" fill="#2a9d3c" {RIM}/><path d="M4.3 8.3l2.5 2.6 5-5.6" fill="none" stroke="#fff" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"/>"##),
        // Red circle with a white exclamation mark.
        Status::Modified => format!(r##"<circle cx="8" cy="8" r="7" fill="#d9302c" {RIM}/><rect x="6.9" y="3.4" width="2.2" height="6" rx="1" fill="#fff"/><circle cx="8" cy="11.9" r="1.2" fill="#fff"/>"##),
        // Blue circle with a white plus.
        Status::Added | Status::Renamed => format!(r##"<circle cx="8" cy="8" r="7" fill="#2c6fd6" {RIM}/><path d="M8 4v8M4 8h8" stroke="#fff" stroke-width="2.3" stroke-linecap="round"/>"##),
        // Red circle with a white cross.
        Status::Deleted => format!(r##"<circle cx="8" cy="8" r="7" fill="#c62020" {RIM}/><path d="M5.2 5.2l5.6 5.6M10.8 5.2l-5.6 5.6" stroke="#fff" stroke-width="2.2" stroke-linecap="round"/>"##),
        // Yellow warning triangle.
        Status::Conflicted => format!(r##"<path d="M8 1.2L15 14.3H1z" fill="#f4c20d" stroke="#fff" stroke-width="1.2" stroke-linejoin="round"/><rect x="7" y="5.2" width="2" height="5" rx=".9" fill="#5a3a00"/><circle cx="8" cy="12.2" r="1.1" fill="#5a3a00"/>"##),
        // Grey circle with a white bar.
        Status::Ignored => format!(r##"<circle cx="8" cy="8" r="7" fill="#8c8c8c" {RIM}/><path d="M4.4 8h7.2" stroke="#fff" stroke-width="2.3" stroke-linecap="round"/>"##),
        // Purple circle with a white question mark.
        Status::Unversioned => format!(r##"<circle cx="8" cy="8" r="7" fill="#7b55c7" {RIM}/><path d="M5.8 6.1a2.3 2.2 0 1 1 3.3 2c-.8.4-1.1.9-1.1 1.7" fill="none" stroke="#fff" stroke-width="1.9" stroke-linecap="round"/><circle cx="8" cy="12.3" r="1.1" fill="#fff"/>"##),
    };
    svg(&body)
}

fn svg(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">{body}</svg>"#)
}

/// Shapes the menu icons share.
const REPO: &str = r##"<ellipse cx="6" cy="4" rx="4.6" ry="1.8" fill="#9bb5d6" stroke="#3b5b85"/><path d="M1.4 4v7.5c0 1 2.1 1.8 4.6 1.8s4.6-.8 4.6-1.8V4" fill="#7c9cc7" stroke="#3b5b85"/><path d="M1.4 7.7c0 1 2.1 1.8 4.6 1.8s4.6-.8 4.6-1.8" fill="none" stroke="#3b5b85"/>"##;
const PAGE: &str = r##"<path d="M2.5 1.5h7l3.5 3.5v9.5h-10.5z" fill="#fff" stroke="#6b6b6b"/><path d="M9.5 1.5v3.5h3.5" fill="#e6e6e6" stroke="#6b6b6b"/>"##;
const LINES: &str = r##"<path d="M4.5 7h6M4.5 9h6M4.5 11h4.5" stroke="#9a9a9a"/>"##;

fn arrow_up(color: &str, x: f32) -> String {
    format!(r##"<path d="M{x} 15V9.5h-2.3L{a} 5.6l3.8 3.9H{b}V15z" fill="{color}" stroke="#fff" stroke-width=".8" stroke-linejoin="round"/>"##, x = x - 1.3, a = x, b = x + 1.3)
}

fn arrow_down(color: &str, x: f32) -> String {
    format!(r##"<path d="M{l} 5.6v5.5h-2.3l3.8 3.9 3.8-3.9H{r}V5.6z" fill="{color}" stroke="#fff" stroke-width=".8" stroke-linejoin="round"/>"##, l = x - 1.3, r = x + 1.3)
}

/// The SVG of a menu or dialog icon by its name (see [`menu_icon_name`]).
fn menu_svg(name: &str) -> Option<String> {
    let body = match name {
        // A turtle: TortoiseGit's submenu.
        "tgit" => r##"<ellipse cx="7.5" cy="9" rx="5.6" ry="4.2" fill="#3f9b3a" stroke="#22561f"/><path d="M4.2 8.4l2-1.6h2.6l2 1.6-2 1.6H6.2zM7.5 4.9v1.9M7.5 10v3M3 6.6l1.2 1.8M12 6.6l-1.2 1.8" fill="#6cc463" stroke="#22561f" stroke-width=".8"/><circle cx="14.2" cy="8" r="1.6" fill="#8fcf6a" stroke="#22561f" stroke-width=".8"/><path d="M3.2 12.3l-1 1.6M11.8 12.3l1 1.6" stroke="#22561f" stroke-width="1.4" stroke-linecap="round"/>"##.to_string(),
        "commit" => format!(r##"{REPO}{}"##, arrow_up("#2a9d3c", 12.3)),
        "pull" => format!(r##"{REPO}{}"##, arrow_down("#2c6fd6", 12.3)),
        "fetch" => format!(r##"{REPO}{}"##, arrow_down("#26a6a6", 12.3)),
        "push" => format!(r##"{REPO}{}"##, arrow_up("#e07b00", 12.3)),
        "sync" => r##"<path d="M13.2 6.2A5.5 5.5 0 0 0 3 5" fill="none" stroke="#2c6fd6" stroke-width="2"/><path d="M1.4 2.4l.4 4.4 4.2-1.4z" fill="#2c6fd6"/><path d="M2.8 9.8A5.5 5.5 0 0 0 13 11" fill="none" stroke="#2a9d3c" stroke-width="2"/><path d="M14.6 13.6l-.4-4.4-4.2 1.4z" fill="#2a9d3c"/>"##.to_string(),
        "clone" => format!(r##"<g transform="translate(4 2) scale(.75)">{REPO}</g><g transform="translate(-1 -1) scale(.75)" opacity=".85">{REPO}</g><path d="M3 13.5h4.5" stroke="#2a9d3c" stroke-width="1.6"/><path d="M7 11.4l2.6 2.1L7 15.6z" fill="#2a9d3c"/>"##),
        "createrepo" => format!(r##"{REPO}<circle cx="12" cy="12" r="3.4" fill="#2a9d3c" stroke="#fff" stroke-width=".8"/><path d="M12 10v4M10 12h4" stroke="#fff" stroke-width="1.5"/>"##),
        "diff" => r##"<path d="M1.5 1.5h7v10h-7z" fill="#fff" stroke="#6b6b6b"/><path d="M3 4h4M3 6h4" stroke="#d9302c"/><path d="M7.5 4.5h7v10h-7z" fill="#fff" stroke="#6b6b6b"/><path d="M9 7.5h4M9 9.5h4M9 11.5h3" stroke="#2a9d3c"/>"##.to_string(),
        "diffprev" => r##"<path d="M1.5 1.5h7v10h-7z" fill="#fff" stroke="#6b6b6b"/><path d="M3 4h4M3 6h4" stroke="#d9302c"/><path d="M7.5 4.5h7v10h-7z" fill="#fff" stroke="#6b6b6b"/><circle cx="11" cy="10.5" r="2.8" fill="#fff6d5" stroke="#b07a00"/><path d="M11 9v1.6h1.3" fill="none" stroke="#b07a00"/>"##.to_string(),
        "log" => r##"<path d="M2.5 1.5h11v13h-11z" fill="#fff" stroke="#6b6b6b"/><path d="M5 4v8.5" stroke="#2c6fd6"/><circle cx="5" cy="4.5" r="1.3" fill="#d9302c"/><circle cx="5" cy="8" r="1.3" fill="#2c6fd6"/><circle cx="5" cy="11.5" r="1.3" fill="#2a9d3c"/><path d="M7.5 4.5h4M7.5 8h4M7.5 11.5h4" stroke="#7a7a7a"/>"##.to_string(),
        "reflog" => r##"<path d="M2.5 1.5h11v13h-11z" fill="#fff" stroke="#6b6b6b"/><path d="M5 4.5h6M5 7h6M5 9.5h3" stroke="#7a7a7a"/><path d="M12.6 12.2a2.6 2.6 0 1 1-1-2.1" fill="none" stroke="#e07b00" stroke-width="1.3"/><path d="M12.8 8.6v2.2h-2.2" fill="none" stroke="#e07b00" stroke-width="1.3"/>"##.to_string(),
        "checkmods" => format!(r##"{PAGE}{LINES}<circle cx="10" cy="10" r="3" fill="#dfeeff" fill-opacity=".8" stroke="#3b5b85" stroke-width="1.3"/><path d="M12.2 12.2l2.6 2.6" stroke="#3b5b85" stroke-width="1.8" stroke-linecap="round"/>"##),
        "stashsave" => r##"<path d="M1.5 8.5h13v6h-13z" fill="#c9a26b" stroke="#7a5523"/><path d="M1.5 8.5l2-2.5h9l2 2.5" fill="#e3c597" stroke="#7a5523"/><path d="M8 1v6M5.6 4.6L8 7l2.4-2.4" fill="none" stroke="#2c6fd6" stroke-width="1.6"/>"##.to_string(),
        "stashpop" => r##"<path d="M1.5 8.5h13v6h-13z" fill="#c9a26b" stroke="#7a5523"/><path d="M1.5 8.5l2-2.5h9l2 2.5" fill="#e3c597" stroke="#7a5523"/><path d="M8 7V1M5.6 3.4L8 1l2.4 2.4" fill="none" stroke="#2a9d3c" stroke-width="1.6"/>"##.to_string(),
        "resolve" => r##"<path d="M7 1.2L13.4 13H.6z" fill="#f4c20d" stroke="#a07800" stroke-linejoin="round"/><circle cx="11.5" cy="11.5" r="3.6" fill="#2a9d3c" stroke="#fff" stroke-width=".8"/><path d="M9.8 11.6l1.2 1.2 2.3-2.5" fill="none" stroke="#fff" stroke-width="1.4"/>"##.to_string(),
        "revert" => r##"<path d="M4 5.5h6a4 4 0 0 1 0 8H5" fill="none" stroke="#e07b00" stroke-width="2.2" stroke-linecap="round"/><path d="M5.5 1.5L1.2 5.5l4.3 4z" fill="#e07b00"/>"##.to_string(),
        "cleanup" => r##"<path d="M12.8 1.2L7.6 8" stroke="#8a5a2b" stroke-width="1.8" stroke-linecap="round"/><path d="M5.4 7l4 3-1.4 4.8c-2.6.3-5.6-1.3-6.8-3.6z" fill="#f2c94c" stroke="#a07800"/><path d="M4 10.5l2.5 2M3.2 11.8l2 1.6" stroke="#a07800"/>"##.to_string(),
        "switch" => r##"<circle cx="4" cy="3" r="1.8" fill="#2c6fd6"/><circle cx="4" cy="13" r="1.8" fill="#2c6fd6"/><circle cx="12" cy="5" r="1.8" fill="#2a9d3c"/><path d="M4 4.5v7M12 6.5c0 3-8 2-8 5" fill="none" stroke="#3b5b85" stroke-width="1.4"/><path d="M9 12.5h5M12.2 10.7l1.8 1.8-1.8 1.8" fill="none" stroke="#e07b00" stroke-width="1.4"/>"##.to_string(),
        "merge" => r##"<circle cx="4" cy="2.8" r="1.8" fill="#2c6fd6"/><circle cx="12" cy="2.8" r="1.8" fill="#2a9d3c"/><circle cx="8" cy="13.2" r="1.8" fill="#7b55c7"/><path d="M4 4.4c0 4 4 4 4 7M12 4.4c0 4-4 4-4 7" fill="none" stroke="#3b5b85" stroke-width="1.5"/>"##.to_string(),
        "branch" => r##"<circle cx="4.5" cy="3" r="1.8" fill="#2c6fd6"/><circle cx="4.5" cy="13" r="1.8" fill="#2c6fd6"/><circle cx="11.5" cy="5" r="1.8" fill="#2a9d3c"/><path d="M4.5 4.6v6.8M11.5 6.6c0 3-7 2.4-7 4.8" fill="none" stroke="#3b5b85" stroke-width="1.5"/><path d="M13.5 10.5v4M11.5 12.5h4" stroke="#2a9d3c" stroke-width="1.4"/>"##.to_string(),
        "tag" => r##"<path d="M1.5 2.5v5l7 7 6-6-7-7h-5z" fill="#f4c20d" stroke="#a07800" stroke-linejoin="round"/><circle cx="4.6" cy="4.6" r="1.3" fill="#fff" stroke="#a07800"/>"##.to_string(),
        "export" => format!(r##"{PAGE}{LINES}<path d="M8 10.5h5.5" stroke="#2a9d3c" stroke-width="1.8"/><path d="M12.4 7.8l3 2.7-3 2.7z" fill="#2a9d3c"/>"##),
        "add" => format!(r##"{PAGE}<circle cx="11" cy="11" r="3.8" fill="#2c6fd6" stroke="#fff" stroke-width=".8"/><path d="M11 9v4M9 11h4" stroke="#fff" stroke-width="1.6"/>"##),
        "delete" => r##"<path d="M3 3l10 10M13 3L3 13" stroke="#c62020" stroke-width="3" stroke-linecap="round"/>"##.to_string(),
        "deletekeep" => format!(r##"{PAGE}<path d="M8.5 8.5l5.5 5.5M14 8.5l-5.5 5.5" stroke="#c62020" stroke-width="2" stroke-linecap="round"/>"##),
        "rename" => format!(r##"{PAGE}<path d="M4 12.5l1-3 6.6-6.6 2 2-6.6 6.6z" fill="#f2c94c" stroke="#7a5523" stroke-linejoin="round"/>"##),
        "ignore" => format!(r##"{PAGE}<circle cx="10.8" cy="10.8" r="3.8" fill="#fff" stroke="#8c8c8c" stroke-width="1.5"/><path d="M8.2 13.4l5.2-5.2" stroke="#8c8c8c" stroke-width="1.5"/>"##),
        "blame" => r##"<circle cx="5" cy="4" r="2.6" fill="#e0a370" stroke="#7a5523"/><path d="M.8 11.5c.4-2.6 2-4 4.2-4s3.8 1.4 4.2 4z" fill="#2c6fd6"/><path d="M9.5 5h5.5M9.5 8h5.5M10 11h5M1 14h14" stroke="#7a7a7a"/>"##.to_string(),
        "patchcreate" => format!(r##"{PAGE}<path d="M4.5 7h3M6 5.5v3" stroke="#2a9d3c" stroke-width="1.3"/><path d="M4.5 11h3" stroke="#d9302c" stroke-width="1.3"/><path d="M9 11h4.5" stroke="#e07b00" stroke-width="1.6"/><path d="M12.6 8.6l2.8 2.4-2.8 2.4z" fill="#e07b00"/>"##),
        "patchapply" => format!(r##"{PAGE}<path d="M4.5 7h3M6 5.5v3" stroke="#2a9d3c" stroke-width="1.3"/><path d="M4.5 11h3" stroke="#d9302c" stroke-width="1.3"/><path d="M15 11h-4.5" stroke="#2c6fd6" stroke-width="1.6"/><path d="M11.4 8.6L8.6 11l2.8 2.4z" fill="#2c6fd6"/>"##),
        "settings" => r##"<path d="M6.8 1h2.4l.4 2 1.5.7 1.7-1.2 1.7 1.7-1.2 1.7.7 1.5 2 .4v2.4l-2 .4-.7 1.5 1.2 1.7-1.7 1.7-1.7-1.2-1.5.7-.4 2H6.8l-.4-2-1.5-.7-1.7 1.2-1.7-1.7 1.2-1.7-.7-1.5-2-.4V6.8l2-.4.7-1.5-1.2-1.7 1.7-1.7 1.7 1.2 1.5-.7z" fill="#8a96a8" stroke="#4a5568" stroke-width=".7"/><circle cx="8" cy="8" r="2.4" fill="#fff" stroke="#4a5568"/>"##.to_string(),
        "about" => r##"<circle cx="8" cy="8" r="7" fill="#2c6fd6"/><circle cx="8" cy="4.6" r="1.2" fill="#fff"/><path d="M8 7v5.2" stroke="#fff" stroke-width="2.2" stroke-linecap="round"/>"##.to_string(),
        "update" => format!(r##"{PAGE}{LINES}<path d="M11 6.5v6" stroke="#2c6fd6" stroke-width="1.8"/><path d="M8.2 10.4l2.8 3.2 2.8-3.2z" fill="#2c6fd6"/>"##),
        "lock" => r##"<path d="M4.5 7V5a3.5 3.5 0 0 1 7 0v2" fill="none" stroke="#6b6b6b" stroke-width="1.6"/><path d="M2.5 7h11v7.5h-11z" fill="#f4c20d" stroke="#a07800"/><circle cx="8" cy="10.5" r="1.2" fill="#7a5523"/>"##.to_string(),
        "unlock" => r##"<path d="M4.5 7V5a3.5 3.5 0 0 1 6.8-1.2" fill="none" stroke="#6b6b6b" stroke-width="1.6"/><path d="M2.5 7h11v7.5h-11z" fill="#f4c20d" stroke="#a07800"/><circle cx="8" cy="10.5" r="1.2" fill="#7a5523"/>"##.to_string(),
        "refresh" => r##"<path d="M13 8a5 5 0 1 1-1.6-3.7" fill="none" stroke="#2a9d3c" stroke-width="2"/><path d="M13.8 1.2v4.6H9.2z" fill="#2a9d3c"/>"##.to_string(),
        _ => return None,
    };
    Some(svg(&body))
}

/// Every menu icon there is (tests).
#[cfg(test)]
const MENU_ICONS: &[&str] = &[
    "tgit", "commit", "pull", "fetch", "push", "sync", "clone", "createrepo", "diff", "diffprev", "log", "reflog", "checkmods", "stashsave", "stashpop", "resolve",
    "revert", "cleanup", "switch", "merge", "branch", "tag", "export", "add", "delete", "deletekeep", "rename", "ignore", "blame", "patchcreate", "patchapply",
    "settings", "about", "refresh", "update", "lock", "unlock",
];

thread_local! {
    static PIXBUFS: RefCell<HashMap<(String, i32), Option<Pixbuf>>> = RefCell::new(HashMap::new());
    static COMPOSED: RefCell<HashMap<(String, i32, u8), Pixbuf>> = RefCell::new(HashMap::new());
}

fn load(svg: &str, size: i32) -> Option<Pixbuf> {
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(svg.as_bytes()));
    Pixbuf::from_stream_at_scale(&stream, size, size, true, gio::Cancellable::NONE).ok()
}

/// The overlay for `s` at `size` pixels.
pub fn overlay(s: Status, size: i32) -> Option<Pixbuf> {
    let key = (format!("overlay-{s:?}"), size);
    PIXBUFS.with(|c| c.borrow_mut().entry(key).or_insert_with(|| load(&overlay_svg(s), size)).clone())
}

/// A menu or dialog icon at `size` pixels.
pub fn pixbuf(name: &str, size: i32) -> Option<Pixbuf> {
    let key = (name.to_string(), size);
    PIXBUFS.with(|c| c.borrow_mut().entry(key).or_insert_with(|| menu_svg(name).and_then(|s| load(&s, size))).clone())
}

/// A menu icon as an icon menus show (and models keep).
pub fn gicon(name: &str) -> Option<gio::Icon> {
    menu_svg(name).map(|s| gio::BytesIcon::new(&glib::Bytes::from_owned(s.into_bytes())).upcast())
}

/// The icon of a Git submenu by its text ("TortoiseGit", "Add to ignore list").
pub fn submenu_icon(text: &str) -> Option<gio::Icon> {
    match text {
        "Repository" => gicon("tgit"),
        "Add to ignore list" => gicon("ignore"),
        _ => None,
    }
}

pub fn image(name: &str, size: i32) -> gtk::Image {
    match pixbuf(name, size) {
        Some(pb) => gtk::Image::from_pixbuf(Some(&pb)),
        None => gtk::Image::new(),
    }
}

/// The status a row's overlay column holds (0: none), and back.
pub fn status_code(s: Option<Status>) -> u32 {
    match s {
        None => 0,
        Some(s) => s as u32 + 1,
    }
}

pub fn status_from_code(c: u32) -> Option<Status> {
    const ALL: [Status; 8] = [Status::Normal, Status::Ignored, Status::Unversioned, Status::Added, Status::Renamed, Status::Deleted, Status::Modified, Status::Conflicted];
    c.checked_sub(1).and_then(|i| ALL.get(i as usize).copied())
}

/// `base` with the overlay for `code` in its lower left corner, where TortoiseGit puts it.
/// `cache_key` names the base image when it can be reused (icons, not thumbnails).
pub fn with_overlay(base: &Pixbuf, code: u32, cache_key: Option<&str>) -> Pixbuf {
    let Some(st) = status_from_code(code) else { return base.clone() };
    let (w, h) = (base.width(), base.height());
    let key = cache_key.map(|k| (k.to_string(), w.max(h), code as u8));
    if let Some(k) = &key {
        if let Some(hit) = COMPOSED.with(|c| c.borrow().get(k).cloned()) {
            return hit;
        }
    }
    let side = w.min(h);
    let size = ((side as f64 * 0.55).round() as i32).clamp(8, 48);
    let Some(ov) = overlay(st, size) else { return base.clone() };
    let Some(out) = base.copy() else { return base.clone() };
    let out = if out.has_alpha() { out } else { out.add_alpha(false, 0, 0, 0).unwrap_or(out) };
    let (x, y) = (0, h - size);
    ov.composite(&out, x, y, size, size, x as f64, y as f64, 1.0, 1.0, gtk::gdk_pixbuf::InterpType::Bilinear, 255);
    if let Some(k) = key {
        COMPOSED.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() > 2000 {
                c.clear();
            }
            c.insert(k, out.clone());
        });
    }
    out
}

pub fn clear_cache() {
    COMPOSED.with(|c| c.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for s in [Status::Normal, Status::Ignored, Status::Unversioned, Status::Added, Status::Renamed, Status::Deleted, Status::Modified, Status::Conflicted] {
            assert_eq!(status_from_code(status_code(Some(s))), Some(s));
        }
        assert_eq!(status_from_code(0), None);
        assert_eq!(status_from_code(99), None);
    }

    #[test]
    fn every_image_is_well_formed_svg() {
        for name in MENU_ICONS {
            let s = menu_svg(name).unwrap_or_else(|| panic!("no icon {name}"));
            assert!(s.starts_with("<svg") && s.ends_with("</svg>"), "{name}");
            assert_eq!(s.matches('<').count(), s.matches('>').count(), "{name}");
        }
        for s in [Status::Normal, Status::Modified, Status::Added, Status::Deleted, Status::Conflicted, Status::Ignored, Status::Unversioned] {
            assert!(overlay_svg(s).contains("viewBox"));
        }
        assert!(menu_svg("nothing").is_none());
    }
}
