//! Drawing file icons at any size: GTK 3's cell renderers only know a few fixed icon sizes,
//! so icons are looked up in the theme at the exact pixel size (cached) and drawn as images.

use super::model;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::gio;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

thread_local! {
    static CACHE: RefCell<HashMap<(String, i32), Option<Pixbuf>>> = RefCell::new(HashMap::new());
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

pub fn clear_cache() {
    CACHE.with(|c| c.borrow_mut().clear());
    super::vcs::icons::clear_cache();
    super::linkicons::clear_cache();
}

/// The theme's image for `icon` at `size` pixels.
pub fn pixbuf_for(icon: &gio::Icon, size: i32) -> Option<Pixbuf> {
    let theme = gtk::IconTheme::default()?;
    if !WATCHING.with(|w| w.replace(true)) {
        // A new icon theme draws everything anew.
        theme.connect_changed(|_| clear_cache());
    }
    let key = (IconExt::to_string(icon).map(|s| s.to_string()).unwrap_or_default(), size);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let pixbuf = theme
        .lookup_by_gicon(icon, size, gtk::IconLookupFlags::FORCE_SIZE)
        .or_else(|| theme.lookup_icon("text-x-generic", size, gtk::IconLookupFlags::FORCE_SIZE))
        .and_then(|info| info.load_icon().ok());
    CACHE.with(|c| c.borrow_mut().insert(key, pixbuf.clone()));
    pixbuf
}

/// What a row shows: its thumbnail if there is one, else its icon at `size`.
/// With the version-control overlay (bottom left) and the link overlay (bottom right) of the
/// row, if it has them.
pub fn row_image(model: &gtk::TreeModel, iter: &gtk::TreeIter, size: i32) -> Option<Pixbuf> {
    let git = model.value(iter, model::COL_GIT as i32).get::<u32>().unwrap_or(0);
    let link = model.value(iter, model::COL_LINK as i32).get::<u32>().unwrap_or(0);
    if let Ok(Some(thumb)) = model.value(iter, model::COL_THUMB as i32).get::<Option<Pixbuf>>() {
        let pb = super::thumbnails::fit(&thumb, size);
        let pb = if git == 0 { pb } else { super::vcs::icons::with_overlay(&pb, git, None) };
        return Some(if link == 0 { pb } else { super::linkicons::with_overlay(&pb, link, None) });
    }
    let icon = model.value(iter, model::COL_ICON as i32).get::<Option<gio::Icon>>().ok().flatten()?;
    let pb = pixbuf_for(&icon, size)?;
    if git == 0 && link == 0 {
        return Some(pb);
    }
    let key = IconExt::to_string(&icon).map(|s| s.to_string()).unwrap_or_default();
    let pb = if git == 0 { pb } else { super::vcs::icons::with_overlay(&pb, git, Some(&key)) };
    Some(if link == 0 { pb } else { super::linkicons::with_overlay(&pb, link, Some(&format!("{key}|{git}"))) })
}

/// The picture a volume names for itself: the image file its own marked files point at, decoded and scaled to
/// the pixels `size` draws at (README: *Device icons*). Nothing where it names none, where the file is no icon
/// file or bigger than one ever is, where its bytes do not decode, or where the decoder answers more pixels than
/// an icon file can name — and then the themed icon of its kind stands, so a damaged file costs a row its picture
/// and never its place in the listing. The bytes come from the lib's own bounded reader, so nothing here ever
/// hands the loader a file it would have to hold whole in memory or a header that would have to be drawn.
pub fn volume_picture(v: &failbrauwser::drives::Volume, size: gtk::IconSize) -> Option<Pixbuf> {
    let bytes = failbrauwser::fsformat::icon::picture_bytes(v.picture_file.as_ref()?)?;
    let picture = Pixbuf::from_read(std::io::Cursor::new(bytes)).ok()?;
    if picture.width() > failbrauwser::fsformat::ico::LARGEST as i32 || picture.height() > failbrauwser::fsformat::ico::LARGEST as i32 { return None; }
    // The surface names its icon size; the pixels behind that name are the theme's answer, which is what keeps
    // this picture the same height as the drive icons drawn beside it.
    Some(super::thumbnails::fit(&picture, size.lookup().map_or(48, |(w, _)| w)))
}

/// The image a drive row shows: the volume's own picture where its files name one that decodes, else the
/// themed icon of its kind. One answer for the drives page, the side panel and a properties window.
pub fn volume_image(v: &failbrauwser::drives::Volume, size: gtk::IconSize) -> gtk::Image {
    let image = gtk::Image::new();
    match volume_picture(v, size) {
        Some(pb) => image.set_from_pixbuf(Some(&pb)),
        None => image.set_from_gicon(&gio::ThemedIcon::from_names(&v.icon_names()), size),
    }
    image
}

/// Makes `renderer` draw the row image at the size `size` holds.
pub fn bind(layout: &impl IsA<gtk::CellLayout>, renderer: &gtk::CellRendererPixbuf, size: Rc<Cell<i32>>) {
    let r2 = renderer.clone();
    CellLayoutExt::set_cell_data_func(
        layout,
        renderer,
        Some(Box::new(move |_, _, model, iter| {
            let px = size.get();
            r2.set_pixbuf(row_image(model, iter, px).as_ref());
        })),
    );
}
