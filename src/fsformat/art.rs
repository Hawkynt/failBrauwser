//! The picture a device shows with its own folder (README: *Device icons*): the tracked glyph of the
//! device's shape, with the words the machine reported about it composited into the picture — the brand the
//! model string begins with and the size in the units the packaging prints, decimal.
//!
//! The nine masters are drawn by `assets\device-glyphs\make-glyphs.ps1`, live in this repository next to
//! that record, and are carried inside the program's own file, so an icon is written on a machine that has
//! no folder of art beside the program. Nothing of another program's artwork is used or adapted: the shapes
//! are drawn from what the device looks like. The words are composited when an icon is written rather than
//! baked into a master, because the manufacturer and the size are this volume's, not the shape's.

use super::{Shape, decimal_size, vendor_token};
use gtk::cairo;
use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::Pixbuf;

/// The sizes one device icon holds, largest first as the container writes them.
pub const SIZES: [u32; 4] = [256, 48, 32, 16];

/// The side a tracked master is drawn at.
const MASTER: f64 = 256.0;
/// The band the words sit on and the words themselves are the two tones the masters are drawn in, so the
/// band reads as part of the picture and not as a sticker on it.
const BODY: (f64, f64, f64) = (47.0 / 255.0, 58.0 / 255.0, 69.0 / 255.0);
const WORD: (f64, f64, f64) = (215.0 / 255.0, 222.0 / 255.0, 229.0 / 255.0);

/// One pixel of a picture: red, green, blue and alpha — the colour and its alpha apart, which is how an icon
/// file holds them, rather than as cairo stores them.
pub type Pixel = [u8; 4];

/// The tracked master of a shape, as the program's own file carries it.
pub fn master(shape: Shape) -> &'static [u8] {
    match shape {
        Shape::Nvme => include_bytes!("../../assets/device-glyphs/nvme.png"),
        Shape::Ssd => include_bytes!("../../assets/device-glyphs/ssd.png"),
        Shape::Hdd => include_bytes!("../../assets/device-glyphs/hdd.png"),
        Shape::UsbStick => include_bytes!("../../assets/device-glyphs/usbstick.png"),
        Shape::SdCard => include_bytes!("../../assets/device-glyphs/sdcard.png"),
        Shape::MicroSd => include_bytes!("../../assets/device-glyphs/msd.png"),
        Shape::Floppy35 => include_bytes!("../../assets/device-glyphs/floppy35.png"),
        Shape::Floppy525 => include_bytes!("../../assets/device-glyphs/floppy525.png"),
        Shape::Optical => include_bytes!("../../assets/device-glyphs/optical.png"),
    }
}

/// The pixels of a picture, top row first. cairo stores ARgb32 premultiplied and lends its pixels out only
/// while nothing else holds the surface — a live drawing context references it, so a picture is read once it
/// has been drawn and not while it is being drawn. The alpha is divided back out here, which is what an icon
/// file wants and what a test compares against.
pub fn pixels(surface: &mut cairo::ImageSurface) -> Result<Vec<Pixel>, String> {
    let (width, height) = (surface.width(), surface.height());
    let stride = surface.stride() as usize;
    let guard = surface.data().map_err(|_| "the picture's pixels could not be read")?;
    let data: &[u8] = &guard;
    let unfuse = |value: u8, alpha: u8| -> u8 { ((u32::from(value) * 255 + u32::from(alpha / 2)) / u32::from(alpha)).min(255) as u8 };
    let mut out = Vec::with_capacity(usize::try_from(width * height).unwrap_or_default());
    for y in 0..height {
        for x in 0..width {
            let i = y as usize * stride + x as usize * 4;
            let (blue, green, red, alpha) = (data[i], data[i + 1], data[i + 2], data[i + 3]);
            out.push(if alpha == 0 { [0, 0, 0, 0] } else { [unfuse(red, alpha), unfuse(green, alpha), unfuse(blue, alpha), alpha] });
        }
    }
    Ok(out)
}

/// The words an icon carries. The size goes on at every size; the brand goes on only at the largest picture,
/// which is the one that can hold two words at a readable height (README: *Device icons*).
pub fn imprint(model: &str, bytes: u64, with_brand: bool) -> String {
    let size = decimal_size(bytes);
    let brand = vendor_token(model);
    if with_brand && !brand.is_empty() { format!("{} {}", brand.to_uppercase(), size) } else { size }
}

/// Whether a picture of this side is big enough to carry the brand as well as the size. Measured here, not
/// chosen by taste: at 48 pixels a line of both words fits only by shrinking under the height the size word
/// has on its own, which left 17 sampled pixels of ink on the band against 27 for the size word alone — less
/// ink, which is the mush the words are meant to avoid.
fn carries_brand(size: u32) -> bool {
    size >= 256 // the largest of SIZES, and the only one drawn with two words
}

/// How much of a picture of this side the band the words sit on takes. A 16 pixel icon needs a proportionally
/// taller band than a 256 pixel one to hold its word at all, and the smallest ones need the shape's room for
/// the shape.
fn band_of(size: u32) -> f64 {
    (f64::from(size) * if size >= 256 { 0.28 } else if size >= 48 { 0.24 } else { 0.42 }).max(6.0)
}

/// The composited picture at one size: the shape scaled into the room above the band, the band, and the words
/// fitted into it. A shape scaled into the whole square would put words over the device, and words over a
/// device cannot be read.
pub fn image(shape: Shape, model: &str, bytes: u64, size: u32) -> Result<cairo::ImageSurface, String> {
    let side = i32::try_from(size).map_err(|_| "the size is beyond what an icon holds".to_string())?;
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, side, side).map_err(|_| "the picture could not be made".to_string())?;
    let cr = cairo::Context::new(&surface).map_err(|_| "the picture could not be drawn".to_string())?;
    let side = f64::from(size);
    let words = imprint(model, bytes, carries_brand(size));
    let band = band_of(size);
    let room = side - band;

    let master = Pixbuf::from_read(master(shape)).map_err(|_| "the tracked glyph of this shape could not be read")?;
    let scale = room / MASTER;
    cr.save().map_err(|_| "the picture could not be drawn".to_string())?;
    cr.translate((side - MASTER * scale) / 2.0, 0.0);
    cr.scale(scale, scale);
    cr.set_source_pixbuf(&master, 0.0, 0.0);
    cr.source().set_filter(cairo::Filter::Good); // a glyph scaled to nine pixels is still a glyph, not noise
    cr.paint().map_err(|_| "the tracked glyph could not be drawn")?;
    cr.restore().map_err(|_| "the picture could not be drawn".to_string())?;

    cr.set_source_rgb(BODY.0, BODY.1, BODY.2);
    cr.rectangle(0.0, room, side, band);
    cr.fill().map_err(|_| "the band could not be drawn")?;
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
    let mut point = band * 0.66;
    loop {
        cr.set_font_size(point);
        let ink = cr.text_extents(&words).map_err(|_| "the words could not be measured")?;
        if ink.width() <= side - side * 0.06 || point <= 1.0 {
            // Centred on the band, whatever the font's own bearing puts above and below the baseline.
            cr.move_to((side - ink.width()) / 2.0 - ink.x_bearing(), room + band / 2.0 - (ink.y_bearing() + ink.height() / 2.0));
            cr.set_source_rgb(WORD.0, WORD.1, WORD.2);
            cr.show_text(&words).map_err(|_| "the words could not be written")?;
            break;
        }
        point *= 0.9;
    }
    Ok(surface)
}

/// The four pictures of one device icon, in the order the container writes them.
pub fn images(shape: Shape, model: &str, bytes: u64) -> Result<Vec<(u32, cairo::ImageSurface)>, String> {
    SIZES.iter().map(|&size| image(shape, model, bytes, size).map(|s| (size, s))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a sampled pixel stops being the dark band and starts being ink: the antialiased edge of a glyph
    /// lies between the two tones, and no shape drawn here is that light anywhere on the band.
    const INK: u8 = 150;

    fn snap(surface: &mut cairo::ImageSurface) -> Vec<Pixel> {
        pixels(surface).expect("the picture's pixels")
    }

    /// The pixels of the band, which is the only part of a picture this module draws itself.
    fn band(side: i32, all: &[Pixel]) -> Vec<Pixel> {
        let room = side as f64 - band_of(side as u32);
        ((room as i32 + 1)..side).flat_map(|y| (0..side).step_by(2).map(move |x| all[(y * side + x) as usize])).collect()
    }

    /// How many sampled columns of the band hold ink, which is how wide the drawn words are.
    fn ink_span(side: i32, all: &[Pixel]) -> usize {
        let room = side as f64 - band_of(side as u32);
        (0..side).step_by(2).filter(|&x| ((room as i32 + 1)..side).any(|y| { let p = all[(y * side + x) as usize]; p[3] > 0 && p[0] > INK })).count()
    }

    /// The master a shape is drawn from has to be the tracked file that shape's own name names, or the program
    /// would print a µSD card where an SD card is and nothing would notice.
    #[test]
    fn every_shape_is_drawn_from_the_tracked_file_its_own_name_names() {
        for &shape in Shape::all() {
            let tracked = std::fs::read(format!("assets/device-glyphs/{}.png", shape.asset())).expect("the tracked master");
            assert_eq!(master(shape), tracked.as_slice(), "{} is carried in the program under another name than its asset", shape.label());
        }
    }

    #[test]
    fn every_shape_has_its_own_master_and_all_nine_are_different() {
        let bytes: Vec<&[u8]> = Shape::all().iter().map(|s| master(*s)).collect();
        assert_eq!(bytes.len(), 9);
        for b in &bytes {
            assert!(b.starts_with(&[0x89, b'P', b'N', b'G']), "a master is no PNG at all");
            assert!(b.len() > 500, "a master is too small to hold a shape");
        }
        for (i, a) in bytes.iter().enumerate() {
            for b in bytes.iter().skip(i + 1) {
                assert_ne!(a, b, "two shapes carry the same master");
            }
        }
    }

    /// The brand goes on only where there is room for it, and the size never leaves.
    #[test]
    fn the_words_name_the_brand_only_where_the_picture_can_hold_it() {
        assert_eq!(imprint("SanDisk Ultra", 64_160_400_896, true), "SANDISK 64 GB");
        assert_eq!(imprint("SanDisk Ultra", 64_160_400_896, false), "64 GB");
        // A machine that names no brand still gets its size, and a card reader names no brand at all.
        assert_eq!(imprint("", 15_698_757_677_056, true), "16 TB");
        assert_eq!(imprint("Realtek Card Reader", 64_160_400_896, true), "64 GB");
        assert!(carries_brand(256) && !carries_brand(48) && !carries_brand(32) && !carries_brand(16));
    }

    #[test]
    fn a_picture_holds_its_shape_its_band_and_its_words() {
        let mut surface = image(Shape::UsbStick, "SanDisk Ultra", 64_160_400_896, 256).unwrap();
        assert_eq!((surface.width(), surface.height()), (256, 256));
        let all = snap(&mut surface);
        assert_eq!(all[2 * 256 + 2][3], 0, "the corner where the stick is not is painted anyway");
        let pixels = band(256, &all);
        assert!(pixels.iter().all(|p| p[3] > 0), "the band is not drawn across the whole width");
        assert!(pixels.iter().any(|p| p[0] > INK), "no word is drawn on the band");
        assert!(pixels.iter().any(|p| p[0] < 100), "the band is no longer the body colour it is drawn in");
    }

    #[test]
    fn the_four_pictures_of_one_device_are_the_four_sizes() {
        let pictures = images(Shape::Nvme, "Samsung SSD 870 QVO", 4_000_787_030_016).unwrap();
        assert_eq!(pictures.iter().map(|(s, _)| *s).collect::<Vec<_>>(), SIZES.to_vec());
        for (size, surface) in &pictures {
            assert_eq!((surface.width(), surface.height()), (i32::try_from(*size).unwrap(), i32::try_from(*size).unwrap()));
        }
    }

    /// The band carries the brand at the largest picture and nowhere else: at 16, 32 and 48 pixels a picture
    /// drawn from a model string is pixel for pixel the picture drawn with no model at all, and at 256 the
    /// brand widens the ink the band holds.
    #[test]
    fn the_band_carries_the_brand_at_the_largest_picture_only() {
        let draw = |model: &str, size: u32| {
            let mut s = image(Shape::UsbStick, model, 64_160_400_896, size).unwrap();
            (i32::try_from(size).unwrap(), snap(&mut s))
        };
        for size in [16u32, 32, 48] {
            let (side, with) = draw("SanDisk Ultra", size);
            let (_, bare) = draw("", size);
            assert_eq!(with, bare, "a {size} pixel picture carries something of the brand");
            assert!(!band(side, &with).is_empty() && band(side, &with).iter().all(|p| p[3] > 0), "the band is missing at {size} pixels");
        }
        let (mid, middle) = draw("SanDisk Ultra", 48);
        assert!(ink_span(mid, &middle) > 0, "no word at all is drawn at 48 pixels");
        let (big, with) = draw("SanDisk Ultra", 256);
        let (_, bare) = draw("", 256);
        assert!(ink_span(big, &with) > ink_span(big, &bare), "the brand adds no ink at 256 pixels: {} against {}", ink_span(big, &with), ink_span(big, &bare));
    }

    /// A picture's pixels are its own: reading one does not change what was drawn.
    #[test]
    fn a_picture_that_is_transparent_outside_its_shape_keeps_that_transparency_when_read() {
        let mut surface = image(Shape::Floppy525, "", 1_474_560, 32).unwrap();
        let all = snap(&mut surface);
        assert_eq!(all[0][3], 0, "the corner outside a 5.25 inch sleeve is not transparent");
        assert!(all.iter().filter(|p| p[3] > 0).count() > 32 * 32 / 3, "the sleeve is barely drawn at all");
    }
}
