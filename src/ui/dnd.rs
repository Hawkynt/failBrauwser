//! Drag and drop between tabs, windows and other applications (text/uri-list).
//!
//! Without modifier keys a drop moves within one filesystem and copies across
//! filesystems, like other file managers; Shift forces move, Ctrl copy, Ctrl+Shift link.

use super::clipboard::ClipSource;
use super::fileops;
use super::model;
use super::pane::Pane;
use super::window::Window;
use failbrauwser::clipformat::uri_to_path;
use failbrauwser::location::Location;
use failbrauwser::ops::copy::Mode;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

const URI_LIST: &str = "text/uri-list";
/// In-process drags of archive entries.
const INTERNAL: &str = "application/x-failbrauwser-items";

thread_local! {
    /// What the drag started here carries, for choosing the default action and for archive entries.
    static DRAGGING: RefCell<Vec<ClipSource>> = const { RefCell::new(Vec::new()) };
}

/// The size of each item's image in the drag image of several items.
const DRAG_ICON_PX: i32 = 48;
/// How far each image behind lies from the one in front.
const STACK_STEP: i32 = 8;

/// The drag image of `count` items: `images` (at most four shown) stacked, the first in
/// front, with a badge showing `count` in the selection colors `bg` and `fg`. The pointer
/// holds it at the middle of the front image.
pub fn stacked_icon(images: &[gtk::gdk_pixbuf::Pixbuf], count: usize, bg: (f64, f64, f64), fg: (f64, f64, f64)) -> Option<gtk::cairo::ImageSurface> {
    use gtk::cairo;
    let shown = &images[..images.len().min(4)];
    let behind = shown.len() as i32 - 1;
    let badge = 22;
    let width = DRAG_ICON_PX + behind * STACK_STEP + badge / 2;
    let height = DRAG_ICON_PX + behind * STACK_STEP + badge / 2;
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).ok()?;
    let cr = cairo::Context::new(&surface).ok()?;
    // The ones behind first, up and to the right; the first one in front, bottom left.
    for (i, img) in shown.iter().enumerate().rev() {
        let x = i as i32 * STACK_STEP + (DRAG_ICON_PX - img.width()) / 2;
        let y = badge / 2 + (behind - i as i32) * STACK_STEP + (DRAG_ICON_PX - img.height()) / 2;
        cr.set_source_pixbuf(img, x as f64, y as f64);
        cr.paint_with_alpha(if i == 0 { 1.0 } else { 0.85 }).ok()?;
    }
    // The count, in a round badge at the top right.
    let text = if count > 999 { "999+".to_string() } else { count.to_string() };
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    cr.set_font_size(12.0);
    let ext = cr.text_extents(&text).ok()?;
    let r = badge as f64 / 2.0;
    let w = (ext.width() + 10.0).max(badge as f64);
    let (bx, by) = (width as f64 - w, 0.0);
    cr.new_sub_path();
    cr.arc(bx + w - r, by + r, r, -std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    cr.arc(bx + r, by + r, r, std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
    cr.set_source_rgb(bg.0, bg.1, bg.2);
    cr.fill().ok()?;
    cr.set_source_rgb(fg.0, fg.1, fg.2);
    cr.move_to(bx + (w - ext.width()) / 2.0 - ext.x_bearing(), by + r - ext.height() / 2.0 - ext.y_bearing());
    cr.show_text(&text).ok()?;
    drop(cr);
    let front_y = badge / 2 + behind * STACK_STEP;
    surface.set_device_offset(-(DRAG_ICON_PX as f64 / 2.0), -((front_y + DRAG_ICON_PX / 2) as f64));
    Some(surface)
}

fn targets() -> Vec<gtk::TargetEntry> {
    vec![
        gtk::TargetEntry::new(INTERNAL, gtk::TargetFlags::SAME_APP, 1),
        gtk::TargetEntry::new(URI_LIST, gtk::TargetFlags::empty(), 0),
    ]
}

pub fn setup(w: &Window, pane: &Rc<Pane>) {
    let actions = gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK;
    enable_model_drag_source(pane.tree.upcast_ref(), actions);
    enable_model_drag_source(pane.icons.upcast_ref(), actions);
    for view in [pane.tree.upcast_ref::<gtk::Widget>(), pane.icons.upcast_ref()] {
        view.drag_dest_set(gtk::DestDefaults::empty(), &targets(), actions);
        connect_source(view, pane);
        connect_dest(view, w, pane);
    }
}

/// `gtk_{tree,icon}_view_enable_model_drag_source`, which the Rust bindings do not cover.
/// Unlike a plain drag source it keeps rubber-band selection and multi-row drags working.
fn enable_model_drag_source(view: &gtk::Widget, actions: gdk::DragAction) {
    use glib::translate::{IntoGlib, ToGlibPtr};
    let owned: Vec<(std::ffi::CString, u32, u32)> = targets()
        .iter()
        .map(|t| (std::ffi::CString::new(t.target().to_string()).unwrap(), t.flags().bits(), t.info()))
        .collect();
    let entries: Vec<gtk::ffi::GtkTargetEntry> = owned
        .iter()
        .map(|(name, flags, info)| gtk::ffi::GtkTargetEntry { target: name.as_ptr() as *mut _, flags: *flags, info: *info })
        .collect();
    let mask = gdk::ModifierType::BUTTON1_MASK.into_glib();
    unsafe {
        if let Some(tree) = view.downcast_ref::<gtk::TreeView>() {
            gtk::ffi::gtk_tree_view_enable_model_drag_source(tree.to_glib_none().0, mask, entries.as_ptr(), entries.len() as i32, actions.into_glib());
        } else if let Some(icons) = view.downcast_ref::<gtk::IconView>() {
            gtk::ffi::gtk_icon_view_enable_model_drag_source(icons.to_glib_none().0, mask, entries.as_ptr(), entries.len() as i32, actions.into_glib());
        }
    }
}

fn connect_source(view: &gtk::Widget, pane: &Rc<Pane>) {
    let p = Rc::downgrade(pane);
    view.connect_drag_begin(move |_, _| {
        if let Some(p) = p.upgrade() {
            let sources = fileops::selection_sources(&p);
            DRAGGING.with(|d| *d.borrow_mut() = sources);
        }
    });
    // Several items: their icons stacked with how many there are, instead of GTK's image of
    // the one row pressed (set by the view's own handler, so this one runs after it).
    let p = Rc::downgrade(pane);
    view.connect_closure(
        "drag-begin",
        true,
        glib::closure_local!(move |view: gtk::Widget, ctx: gdk::DragContext| {
            let Some(p) = p.upgrade() else { return };
            let (images, count) = p.selected_images(DRAG_ICON_PX, 4);
            if count < 2 || images.is_empty() {
                return;
            }
            let style = view.style_context();
            let bg = style.lookup_color("theme_selected_bg_color").unwrap_or_else(|| gdk::RGBA::new(0.21, 0.52, 0.89, 1.0));
            let fg = style.lookup_color("theme_selected_fg_color").unwrap_or_else(|| gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));
            if let Some(icon) = stacked_icon(&images, count, (bg.red(), bg.green(), bg.blue()), (fg.red(), fg.green(), fg.blue())) {
                ctx.drag_set_icon_surface(&icon);
            }
        }),
    );
    view.connect_drag_end(|_, _| DRAGGING.with(|d| d.borrow_mut().clear()));
    view.connect_drag_data_get(|_, _, sel, info, _| {
        let sources = DRAGGING.with(|d| d.borrow().clone());
        if info == 1 {
            // The receiving side reads DRAGGING directly; the payload only marks the drag as ours.
            sel.set(&gdk::Atom::intern(INTERNAL), 8, b"1");
            return;
        }
        let uris: Vec<String> = sources
            .iter()
            .filter_map(|s| match s {
                ClipSource::Local(p) => glib::filename_to_uri(p, None).ok().map(|u| u.to_string()),
                _ => None,
            })
            .collect();
        let refs: Vec<&str> = uris.iter().map(String::as_str).collect();
        sel.set_uris(&refs);
    });
}

/// The folder a drop at (x, y) goes into: the folder row under the pointer, else the tab's location.
fn drop_target(view: &gtk::Widget, pane: &Pane, x: i32, y: i32) -> (Location, Option<gtk::TreePath>) {
    let path = if let Some(tree) = view.downcast_ref::<gtk::TreeView>() {
        let (bx, by) = tree.convert_widget_to_bin_window_coords(x, y);
        tree.path_at_pos(bx, by).and_then(|(p, ..)| p)
    } else if let Some(icons) = view.downcast_ref::<gtk::IconView>() {
        icons.path_at_pos(x, y)
    } else {
        None
    };
    if let Some(path) = path {
        if let Some(iter) = pane.store.iter(&path) {
            let is_dir: bool = pane.store.value(&iter, model::COL_IS_DIR as i32).get().unwrap_or(false);
            let name: String = pane.store.value(&iter, model::COL_NAME as i32).get().unwrap_or_default();
            if is_dir {
                if let Some(loc) = pane.location().child(&name) {
                    return (loc, Some(path));
                }
            }
        }
    }
    (pane.location(), None)
}

fn highlight(view: &gtk::Widget, path: Option<&gtk::TreePath>) {
    if let Some(tree) = view.downcast_ref::<gtk::TreeView>() {
        tree.set_drag_dest_row(path, gtk::TreeViewDropPosition::IntoOrAfter);
    } else if let Some(icons) = view.downcast_ref::<gtk::IconView>() {
        icons.set_drag_dest_item(path, gtk::IconViewDropPosition::DropInto);
    }
}

fn same_filesystem(sources: &[ClipSource], dest: &Location) -> bool {
    let Some(dir) = dest.local_path() else { return false };
    let Some(d) = failbrauwser::osprim::entry_device(dir) else { return false };
    !sources.is_empty()
        && sources.iter().all(|s| match s {
            ClipSource::Local(p) => failbrauwser::osprim::entry_device(p) == Some(d),
            _ => false,
        })
}

fn choose_action(view: &gtk::Widget, ctx: &gdk::DragContext, dest: &Location) -> gdk::DragAction {
    let allowed = ctx.actions();
    let mods = view
        .window()
        .zip(Some(ctx.device()))
        .map(|(win, dev)| win.device_position(&dev).3)
        .unwrap_or_else(gdk::ModifierType::empty);
    let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
    let want = if ctrl && shift {
        gdk::DragAction::LINK
    } else if shift {
        gdk::DragAction::MOVE
    } else if ctrl {
        gdk::DragAction::COPY
    } else {
        let ours = DRAGGING.with(|d| d.borrow().clone());
        if same_filesystem(&ours, dest) { gdk::DragAction::MOVE } else { gdk::DragAction::COPY }
    };
    if allowed.contains(want) {
        want
    } else if allowed.contains(gdk::DragAction::COPY) {
        gdk::DragAction::COPY
    } else {
        ctx.suggested_action()
    }
}

fn connect_dest(view: &gtk::Widget, w: &Window, pane: &Rc<Pane>) {
    let p: Weak<Pane> = Rc::downgrade(pane);
    view.connect_drag_motion(move |view, ctx, x, y, time| {
        let Some(p) = p.upgrade() else { return false };
        let (dest, path) = drop_target(view, &p, x, y);
        // Dropping a folder onto itself does nothing.
        let onto_self = DRAGGING.with(|d| d.borrow().iter().any(|s| matches!((s, &dest), (ClipSource::Local(a), Location::Dir(b)) if a == b)));
        if onto_self || !super::extensions::location_writable(&dest) {
            highlight(view, None);
            ctx.drag_status(gdk::DragAction::empty(), time);
            return true;
        }
        highlight(view, path.as_ref());
        ctx.drag_status(choose_action(view, ctx, &dest), time);
        true
    });
    view.connect_drag_leave(|view, _, _| highlight(view, None));
    view.connect_drag_drop(|view, ctx, _, _, time| {
        let internal = gdk::Atom::intern(INTERNAL);
        let target = if ctx.list_targets().contains(&internal) { internal } else { gdk::Atom::intern(URI_LIST) };
        view.drag_get_data(ctx, &target, time);
        true
    });
    let p: Weak<Pane> = Rc::downgrade(pane);
    let win = Rc::downgrade(&w.me());
    view.connect_drag_data_received(move |view, ctx, x, y, sel, info, time| {
        highlight(view, None);
        let (Some(p), Some(w)) = (p.upgrade(), win.upgrade()) else {
            ctx.drag_finish(false, false, time);
            return;
        };
        let (dest, _) = drop_target(view, &p, x, y);
        let sources: Vec<ClipSource> = if info == 1 {
            DRAGGING.with(|d| d.borrow().clone())
        } else {
            sel.uris().iter().filter_map(|u| uri_to_path(u)).map(ClipSource::Local).collect()
        };
        if sources.is_empty() {
            ctx.drag_finish(false, false, time);
            return;
        }
        let action = ctx.selected_action();
        ctx.drag_finish(true, false, time);
        if action == gdk::DragAction::LINK {
            make_links(&w, &sources, &dest);
            return;
        }
        let mode = if action == gdk::DragAction::MOVE { Mode::Move } else { Mode::Copy };
        fileops::transfer(&w, sources, dest, mode, || {});
    });
}

fn make_links(w: &Window, sources: &[ClipSource], dest: &Location) {
    let Some(dir) = dest.local_path() else { return };
    let mut made = Vec::new();
    for s in sources {
        let ClipSource::Local(p) = s else { continue };
        let Some(name) = p.file_name() else { continue };
        let mut link = dir.join(name);
        if std::fs::symlink_metadata(&link).is_ok() {
            link = dir.join(failbrauwser::ops::names::free_name(dir, name, Some("link")));
        }
        match failbrauwser::osprim::make_symlink(p, &link, p.is_dir()) {
            Ok(()) => made.push(link),
            Err(e) => super::util::show_error(&w.win, "Cannot create link", &e.to_string()),
        }
    }
    fileops::select_created(w, dest, &made);
}


#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(s: &mut gtk::cairo::ImageSurface, x: i32, y: i32) -> [u8; 4] {
        let stride = s.stride() as usize;
        let data = s.data().unwrap();
        let i = y as usize * stride + x as usize * 4;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    #[test]
    fn several_items_drag_as_a_stack_with_their_count() {
        let solid = |c: u32| {
            let p = gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, DRAG_ICON_PX, DRAG_ICON_PX).unwrap();
            p.fill(c);
            p
        };
        let images: Vec<_> = [0xff0000ff, 0x00ff00ff, 0x0000ffff, 0xffff00ff, 0x00ffffff].into_iter().map(solid).collect();
        let mut s = stacked_icon(&images, 12, (0.0, 0.0, 1.0), (1.0, 1.0, 1.0)).unwrap();
        // Four shown, each 8 px further, and half the badge above.
        assert_eq!((s.width(), s.height()), (48 + 3 * 8 + 11, 48 + 3 * 8 + 11));
        // The first (red) in front at the bottom left; the pointer holds its middle.
        let h = s.height();
        assert_eq!(pixel(&mut s, 2, h - 2), [0, 0, 255, 255]);
        assert_eq!(s.device_offset(), (-24.0, -(11.0 + 24.0 + 24.0)));
        // The last one shown (yellow) behind, at the top right; the fifth is left out.
        assert_eq!(pixel(&mut s, 3 * 8 + 2, 11 + 2), [0, 217, 217, 217]);
        // The badge in the selection color.
        let w = s.width();
        assert_eq!(pixel(&mut s, w - 11, 2), [255, 0, 0, 255]);
        assert!(stacked_icon(&images[..1], 3, (0.0, 0.0, 1.0), (1.0, 1.0, 1.0)).is_some());
    }
}
