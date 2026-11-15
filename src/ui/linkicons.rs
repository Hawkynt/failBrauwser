//! The overlays of links (README: Links, *Overlay icons*), drawn for failBrauwser as SVG in Link
//! Shell Extension's colors and shapes: a red arrow on hard links, a chain of three links on
//! junctions, a light green arrow on symbolic links (greyed with a red stroke when the target is
//! gone). They sit at the bottom right, where no version-control overlay goes.

use failbrauwser::links::Overlay;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static PIXBUFS: RefCell<HashMap<(u32, i32), Option<Pixbuf>>> = RefCell::new(HashMap::new());
    static COMPOSED: RefCell<HashMap<(String, i32, u32), Pixbuf>> = RefCell::new(HashMap::new());
}

/// Whether link overlays are drawn (Edit → Links → Link Overlays).
pub fn set_enabled(on: bool) {
    ENABLED.with(|e| e.set(on));
    clear_cache();
}

pub fn enabled() -> bool {
    ENABLED.with(Cell::get)
}

pub fn clear_cache() {
    COMPOSED.with(|c| c.borrow_mut().clear());
}

/// A white plate under the sign, so it reads on any icon.
const PLATE: &str = r##"<rect x=".6" y=".6" width="14.8" height="14.8" rx="3" fill="#ffffff" fill-opacity=".92" stroke="#7a7a7a" stroke-width=".8"/>"##;

fn arrow(color: &str) -> String {
    // The shortcut arrow: up from the bottom left, bending to the top right.
    format!(r##"<path d="M3.2 13V9.3c0-2.9 2.1-4.6 5.2-4.6h1.6V2.4l4.4 4.2-4.4 4.2V8.4H8.6c-1.8 0-2.6.8-2.6 2.3V13z" fill="{color}" stroke="#ffffff" stroke-width=".7" stroke-linejoin="round"/>"##)
}

pub fn svg(o: Overlay) -> String {
    let body = match o {
        Overlay::Hard => arrow("#d42020"),
        Overlay::Symbolic => arrow("#3cc43c"),
        Overlay::Broken => format!(r##"{}<path d="M2.5 13.5L13.5 2.5" stroke="#d42020" stroke-width="2.2" stroke-linecap="round"/>"##, arrow("#9a9a9a")),
        // Three oval links, slanted, one after the other.
        Overlay::Junction => r##"<g fill="none" stroke="#2f5f9e" stroke-width="1.7"><rect x="1.6" y="9" width="5.6" height="3.4" rx="1.7" transform="rotate(-35 4.4 10.7)"/><rect x="5.2" y="6.3" width="5.6" height="3.4" rx="1.7" transform="rotate(-35 8 8)"/><rect x="8.8" y="3.6" width="5.6" height="3.4" rx="1.7" transform="rotate(-35 11.6 5.3)"/></g>"##.to_string(),
    };
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">{PLATE}{body}</svg>"#)
}

fn overlay(o: Overlay, size: i32) -> Option<Pixbuf> {
    PIXBUFS.with(|c| {
        c.borrow_mut()
            .entry((o.code(), size))
            .or_insert_with(|| {
                let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(svg(o).into_bytes()));
                Pixbuf::from_stream_at_scale(&stream, size, size, true, gio::Cancellable::NONE).ok()
            })
            .clone()
    })
}

/// `base` with the overlay of link code `code` at its bottom right (when overlays are on).
pub fn with_overlay(base: &Pixbuf, code: u32, cache_key: Option<&str>) -> Pixbuf {
    let Some(o) = Overlay::from_code(code).filter(|_| enabled()) else { return base.clone() };
    let (w, h) = (base.width(), base.height());
    let key = cache_key.map(|k| (k.to_string(), w.max(h), code));
    if let Some(k) = &key {
        if let Some(hit) = COMPOSED.with(|c| c.borrow().get(k).cloned()) {
            return hit;
        }
    }
    let size = ((w.min(h) as f64 * 0.45).round() as i32).clamp(8, 40);
    let Some(ov) = overlay(o, size) else { return base.clone() };
    let Some(out) = base.copy() else { return base.clone() };
    let out = if out.has_alpha() { out } else { out.add_alpha(false, 0, 0, 0).unwrap_or(out) };
    let (x, y) = (w - size, h - size);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_overlay_is_well_formed_svg_that_loads() {
        for o in [Overlay::Hard, Overlay::Junction, Overlay::Symbolic, Overlay::Broken] {
            let s = svg(o);
            assert!(s.starts_with("<svg") && s.ends_with("</svg>"), "{o:?}");
            assert!(overlay(o, 16).is_some(), "{o:?} does not load");
        }
    }

    #[test]
    fn the_overlay_lands_at_the_bottom_right() {
        let base = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 32, 32).unwrap();
        base.fill(0);
        let out = with_overlay(&base, Overlay::Hard.code(), None);
        let px = |x: i32, y: i32| {
            let bytes = out.read_pixel_bytes();
            let i = (y * out.rowstride() + x * 4) as usize;
            bytes[i + 3]
        };
        assert!(px(28, 28) > 0, "something drawn at the bottom right");
        assert_eq!(px(2, 2), 0, "nothing at the top left");
        set_enabled(false);
        assert_eq!(with_overlay(&base, Overlay::Hard.code(), None).read_pixel_bytes(), base.read_pixel_bytes());
        set_enabled(true);
    }
}
