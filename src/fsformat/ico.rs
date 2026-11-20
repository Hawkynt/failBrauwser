//! The container a device icon is written into (README: *Device icons*): one icon file holding the four
//! pictures of `super::art` at three depths each — twenty-four-bit colour with an eight-bit alpha, an eight-bit
//! palette with a one-bit transparency mask, and a four-bit palette with the same mask — which is the twelve
//! images the rule names, at 16, 32, 48 and 256 pixels.
//!
//! The bytes are written here rather than produced by an image library: the program carries its own glyphs,
//! needs no codec to put them where the shell reads them, and stays honest about what one of its icon files
//! holds — the depths are a choice of this feature, not whatever a library happened to encode. Every image is a
//! device-independent bitmap inside the container, so the file reads back with the same knowledge that wrote
//! it, which is what the tests below do.

use super::art::{Pixel, pixels};
use gtk::cairo;

/// How one picture of an icon file is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// Twenty-four-bit colour with an eight-bit alpha, held as a thirty-two-bit image.
    True,
    /// A palette of up to 256 colours with a one-bit transparency mask.
    Palette8,
    /// A palette of sixteen colours with a one-bit transparency mask.
    Palette4,
}

/// The three depths every picture of a device icon is written at.
pub const DEPTHS: [Depth; 3] = [Depth::True, Depth::Palette8, Depth::Palette4];

impl Depth {
    /// The bits each pixel costs in this depth.
    pub fn bits(self) -> u16 {
        match self {
            Depth::True => 32,
            Depth::Palette8 => 8,
            Depth::Palette4 => 4,
        }
    }

    /// How many colours this depth can name, which is also how many a picture's colours are cut down to.
    fn slots(self) -> usize {
        match self {
            Depth::True => 0,
            Depth::Palette8 => 256,
            Depth::Palette4 => 16,
        }
    }
}

/// The largest side an icon file can name: its directory holds a size in a byte, with zero meaning 256.
pub const LARGEST: u32 = 256;

fn hole(p: &Pixel) -> bool {
    p[3] < 128
}

/// The palette of a depth and, for every pixel, the entry that stands for it. Index zero is the transparent
/// pixel's own slot: a masked pixel shows no colour, so giving it a colour of its own would show that colour in
/// a reader that looks at the image and not at the mask. A colour the palette cannot name exactly goes to the
/// nearest entry it can name.
fn palette(pixels: &[Pixel], slots: usize) -> (Vec<[u8; 3]>, Vec<u8>) {
    let mut counted: Vec<([u8; 3], usize)> = Vec::new();
    for p in pixels.iter().filter(|p| !hole(p)) {
        let colour = [p[0], p[1], p[2]];
        match counted.iter_mut().find(|(c, _)| *c == colour) {
            Some((_, count)) => *count += 1,
            None => counted.push((colour, 1)),
        }
    }
    // Most frequent first, and the colour's own bytes break a tie, so the same picture always becomes the same
    // palette and the same index.
    counted.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut table: Vec<[u8; 3]> = vec![[0, 0, 0]];
    table.extend(counted.iter().take(slots - 1).map(|(c, _)| *c));
    let index = pixels.iter().map(|p| {
        if hole(p) { return 0; }
        let distance = |i: usize| -> i32 { (0..3).map(|k| (i32::from(p[k]) - i32::from(table[i][k])).pow(2)).sum() };
        (0..table.len()).min_by_key(|&i| distance(i)).unwrap_or(0) as u8
    }).collect();
    (table, index)
}

/// The bytes of one image, and how many colours its header names. A device-independent bitmap starts at its
/// bottom row, so the rows are written in reverse, and every image is followed by its one-bit transparency
/// mask, which the header counts in its height. The paletted depths need that mask for their transparency; the
/// true-colour one carries it as well, because that is the spelling this Windows was measured to read back: an
/// entry naming the image's planes and bits and an image drawn at twice its own height is a file
/// `ExtractIconExW` counts an icon in.
fn payload(size: u32, depth: Depth, pixels: &[Pixel]) -> (Vec<u8>, usize) {
    let side = size as usize;
    let (table, index) = match depth {
        Depth::True => (Vec::new(), Vec::new()),
        d => palette(pixels, d.slots()),
    };
    // A row is padded to a whole four bytes, and a four-bit pixel takes half a byte.
    let xor_row = match depth {
        Depth::True => side * 4,
        Depth::Palette8 => side.div_ceil(4) * 4,
        Depth::Palette4 => side.div_ceil(2).div_ceil(4) * 4,
    };
    let and_row = side.div_ceil(32) * 4;
    let (xor_len, and_len) = (xor_row * side, and_row * side);
    let height = size * 2; // the header counts image and mask as one height
    let mut out = Vec::with_capacity(40 + table.len() * 4 + xor_len + and_len);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&depth.bits().to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // no compression: the pixels are written as they are
    out.extend_from_slice(&((xor_len + and_len) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 8]); // no resolution is named: an icon is a shape, not a printed size
    out.extend_from_slice(&(table.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for c in &table {
        out.extend_from_slice(&[c[2], c[1], c[0], 0]); // blue, green, red, reserved
    }
    for y in (0..side).rev() {
        let row = match depth {
            Depth::True => pixels[y * side..][..side].iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect::<Vec<_>>(),
            Depth::Palette8 => index[y * side..][..side].to_vec(),
            // Two pixels to a byte, high nibble first; the nibble an odd side leaves over names the transparent
            // slot rather than running past the picture's own pixels.
            Depth::Palette4 => (0..side.div_ceil(2)).map(|i| index[y * side + i * 2] << 4 | index.get(y * side + i * 2 + 1).copied().unwrap_or_default()).collect::<Vec<_>>(),
        };
        out.extend_from_slice(&row);
        out.resize(out.len() + xor_row - row.len(), 0);
    }
    for y in (0..side).rev() {
        let row = (0..side).fold(vec![0u8; and_row], |mut row, x| {
            if hole(&pixels[y * side + x]) { row[x / 8] |= 0x80 >> (x % 8); } // a set bit is a hole
            row
        });
        out.extend_from_slice(&row);
    }
    (out, table.len())
}

/// Whether an icon file already open in `source` says something a reader can be given at all (README: *Device
/// icons*): an icon file rather than a cursor one, at least one image, a directory the file has room for, every
/// image placed behind that directory and inside the file, every image's own header standing where its entry
/// says it does, and every image within [`LARGEST`] pixels — the picture's own header being the authority, so a
/// directory naming a small image in front of a header naming an enormous one is refused rather than believed.
/// Nothing beyond those headers is read, so the size of a picture file costs this answer nothing, and a file that
/// lies about its own pixels never reaches a decoder that would allocate them.
pub fn pictures_are_ones_a_file_can_hold<S: std::io::Read + std::io::Seek>(source: &mut S, len: u64) -> bool {
    let mut head = [0u8; 6];
    if source.read_exact(&mut head).is_err() || head[..4] != [0, 0, 1, 0] { return false; }
    let count = usize::from(u16::from_le_bytes([head[4], head[5]]));
    let room = 6 + 16 * count;
    if count == 0 || room as u64 > len { return false; }
    let mut directory = vec![0u8; 16 * count];
    if source.read_exact(&mut directory).is_err() { return false; }
    for i in 0..count {
        let e = i * 16;
        let (length, place) = (u32::from_le_bytes(directory[e + 8..][..4].try_into().unwrap()), u64::from(u32::from_le_bytes(directory[e + 12..][..4].try_into().unwrap())));
        if place < room as u64 || place + u64::from(length) > len { return false; }
        // The header of the image itself, which is what a decoder reads its pixels from: a device-independent
        // bitmap names its width and height as words (the height counting the transparency mask twice), and a
        // PNG — the other spelling an entry may hold — names them in its own header, past its signature.
        let mut header = [0u8; 32];
        if source.seek(std::io::SeekFrom::Start(place)).is_err() || source.read_exact(&mut header).is_err() { return false; }
        let png = header[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let (wide, high) = if png {
            (i64::from(u32::from_be_bytes(header[16..][..4].try_into().unwrap())), i64::from(u32::from_be_bytes(header[20..][..4].try_into().unwrap())))
        } else {
            if u32::from_le_bytes(header[..4].try_into().unwrap()) < 40 { return false; }
            (i64::from(i32::from_le_bytes(header[4..][..4].try_into().unwrap())), i64::from(i32::from_le_bytes(header[8..][..4].try_into().unwrap())))
        };
        if wide <= 0 || high <= 0 { return false; }
        // A bitmap's one-bit transparency mask is what makes its header name twice the picture's own rows; a PNG
        // header names the picture alone, so halving its height would hide a picture twice the size of an icon.
        // The arithmetic is wider than the words in the header, because a header naming the largest number it
        // can name is exactly the file this answer exists to refuse.
        if wide.max(if png { high } else { (high + 1) / 2 }) > i64::from(LARGEST) { return false; }
    }
    true
}

/// One image of the container: its size, its depth, the colours its header names, and its bytes.
struct Written {
    size: u32,
    depth: Depth,
    colours: usize,
    bytes: Vec<u8>,
}

/// The bytes of an icon file: the directory and, behind it, every picture at every depth. The pictures go in the
/// order they are given, and each of them is written once per depth. A picture is borrowed mutably because
/// cairo hands out a surface's pixels only through a mutable borrow of it — nothing is changed.
///
/// An entry names its image's length before its place. Measured here: a file whose two words are written the
/// other way round holds images that sit where nothing is and an image count of nothing — `ExtractIconExW`
/// answers zero for it and GDI+ refuses to open it (docs\WINDOWS.md).
pub fn container(pictures: &mut [(u32, cairo::ImageSurface)]) -> Result<Vec<u8>, String> {
    if pictures.is_empty() { return Err("an icon with no picture is no icon".to_string()); }
    let mut written = Vec::with_capacity(pictures.len() * DEPTHS.len());
    for (size, surface) in pictures {
        if *size == 0 || *size > LARGEST { return Err(format!("{} is not a size an icon file can name", size)); }
        if surface.width() != *size as i32 || surface.height() != *size as i32 { return Err(format!("a picture for {} pixels is not that many pixels square", size)); }
        let pixels = pixels(surface)?;
        for depth in DEPTHS {
            let (bytes, colours) = payload(*size, depth, &pixels);
            written.push(Written { size: *size, depth, colours, bytes });
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // an icon file, not a cursor file
    out.extend_from_slice(&(written.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * written.len();
    for w in &written {
        let side = if w.size == LARGEST { 0 } else { w.size as u8 }; // the byte names 256 as zero
        out.extend_from_slice(&[side, side, if w.colours >= 256 { 0 } else { w.colours as u8 }, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&w.depth.bits().to_le_bytes());
        out.extend_from_slice(&(w.bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += w.bytes.len();
    }
    for w in &written {
        out.extend_from_slice(&w.bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsformat::{Shape, art};
    use gtk::cairo;

    fn u16(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([b[at], b[at + 1]])
    }

    fn u32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }

    /// The four pictures of one shape, which is what a device icon is made of.
    fn pictures(shape: Shape) -> Vec<(u32, cairo::ImageSurface)> {
        art::images(shape, "SanDisk Ultra", 64_160_400_896).expect("the four pictures")
    }

    /// Every entry of the directory, read back out of the file's own bytes: size, colours, bits, the place of
    /// its image and how many bytes that image is. A size of 256 reads back as 256, however its byte spells it,
    /// and the two words of an entry are its length and then its place — the order the format defines, not the
    /// order that reads plausibly either way.
    fn directory(bytes: &[u8]) -> Vec<(u32, u8, u16, usize, usize)> {
        (0..usize::from(u16(bytes, 4))).map(|i| {
            let e = 6 + i * 16;
            let side = u32::from(bytes[e]).max(u32::from(bytes[e + 1]));
            (if side == 0 { LARGEST } else { side }, bytes[e + 2], u16(bytes, e + 6), u32(bytes, e + 12) as usize, u32(bytes, e + 8) as usize)
        }).collect()
    }

    /// The pixels of the image behind a directory entry, and the mask behind those: where the palette starts,
    /// and what a row of each costs.
    fn layout(size: u32, depth: Depth, colours: usize) -> (usize, usize, usize, usize) {
        let side = size as usize;
        let xor_row = match depth {
            Depth::True => side * 4,
            Depth::Palette8 => side.div_ceil(4) * 4,
            Depth::Palette4 => side.div_ceil(2).div_ceil(4) * 4,
        };
        let and_row = side.div_ceil(32) * 4;
        (40 + colours * 4, xor_row, and_row, 40 + colours * 4 + (xor_row + and_row) * side)
    }

    #[test]
    fn one_device_icon_holds_the_twelve_images_the_rule_names() {
        let mut all = pictures(Shape::UsbStick);
        let bytes = container(&mut all).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 1, 0], "the file is neither empty nor an icon file");
        assert_eq!(u16(&bytes, 4), 12, "an icon of a device holds one image per size and per depth");
        let want: Vec<(u32, Depth)> = art::SIZES.iter().flat_map(|&s| DEPTHS.iter().map(move |&d| (s, d))).collect();
        let entries = directory(&bytes);
        for (i, &(size, depth)) in want.iter().enumerate() {
            let (side, colours, bits, offset, length) = entries[i];
            assert_eq!(side, size, "entry {i} is not the {size} pixel image its slot says");
            assert_eq!(bits, depth.bits(), "entry {i} is not a {} bit image", depth.bits());
            assert_eq!(u32(&bytes, offset), 40, "the image behind entry {i} is no device-independent bitmap");
            assert_eq!(u32(&bytes, offset + 4), size);
            // Every image names its one-bit mask in the header's height, the true-colour image included: an
            // image without one is an image this Windows cannot extract (docs\WINDOWS.md).
            assert_eq!(u32(&bytes, offset + 8), size * 2, "entry {i} names no room for its own transparency mask");
            assert_eq!(u16(&bytes, offset + 12), 1);
            assert_eq!(u16(&bytes, offset + 14), depth.bits(), "the header of entry {i} disagrees with its directory entry");
            assert_eq!(offset + length, if i + 1 < want.len() { entries[i + 1].3 } else { bytes.len() }, "the images of the file are not packed one behind the other");
            // The directory's colour byte cannot name more than 255, so a full palette is written there as zero
            // and the image's own header is the authority on how many entries follow it.
            let palette = if depth == Depth::True { 0 } else { u32(&bytes, offset + 32) as usize };
            let (pixels_at, _, _, whole) = layout(size, depth, palette);
            assert_eq!(length, whole, "entry {i} counts its image differently than it is written");
            assert!(pixels_at >= 40, "entry {i} has no room for its own header");
            if depth == Depth::True {
                assert_eq!(colours, 0, "a true-colour image names a palette");
                assert_eq!(palette, 0, "a true-colour image holds a palette after all");
            } else {
                assert!((2..=depth.slots()).contains(&palette), "a {} bit image names {palette} colours", depth.bits());
                assert!(colours == 0 || usize::from(colours) == palette, "entry {i} names {colours} colours in its directory and {palette} in its header");
            }
        }
    }

    /// The mask is what makes a shaped icon a shape: the corner where the stick is not is a hole, and the band
    /// the words sit on is not. The bottom row of the file is the top row of the picture.
    #[test]
    fn a_paletted_image_marks_its_holes_in_its_mask_and_its_words_in_its_pixels() {
        let mut all = pictures(Shape::UsbStick);
        let bytes = container(&mut all[1..2]).unwrap(); // one picture, three depths
        let (size, _, _, offset, _) = directory(&bytes)[1];
        let (side, depth) = (usize::try_from(size).unwrap(), Depth::Palette8);
        let palette = u32(&bytes, offset + 32) as usize;
        let (pixels_at, xor_row, and_row, _) = layout(size, depth, palette);
        let (xor, and) = (offset + pixels_at, offset + pixels_at + xor_row * side);
        assert_ne!(bytes[and + and_row * (side - 1)] & 0x80, 0, "the corner where the shape is not is no hole in the mask");
        assert!(bytes[and..and + 6].iter().all(|b| *b == 0), "the band the words sit on is a hole");
        // The band sits at the bottom of the picture, which is where a device-independent bitmap starts, and
        // its words are the lightest thing in it. So the palette holds a light entry — the ink colour — and the
        // band's own rows use it: a band of one colour alone fails here however many colours the shape has.
        let light = (0..palette).find(|&i| bytes[offset + 40 + i * 4 + 2] > 150).expect("no ink colour in the palette");
        let rows = side / 4; // the band's rows, which the file writes first
        assert!(bytes[xor..xor + xor_row * rows].iter().any(|&i| usize::from(i) == light), "no word is drawn on the band");
    }

    #[test]
    fn a_true_colour_image_keeps_the_alpha_it_was_drawn_with() {
        let mut all = pictures(Shape::UsbStick);
        let bytes = container(&mut all[1..2]).unwrap();
        let (size, _, _, offset, _) = directory(&bytes)[0];
        let (side, row) = (usize::try_from(size).unwrap(), usize::try_from(size).unwrap() * 4);
        let alpha_at = |x: usize, y: usize| bytes[offset + 40 + (side - 1 - y) * row + x * 4 + 3];
        assert_eq!(alpha_at(1, 1), 0, "the corner outside the shape is not transparent");
        assert_eq!(alpha_at(24, 40), 255, "the band is not opaque");
    }

    /// A picture whose every pixel is one colour is the one case a palette has no choices left to make.
    #[test]
    fn a_picture_of_one_colour_alone_still_names_a_palette_that_holds_it() {
        // The drawing context is gone before the picture is read: cairo lends its pixels only while nothing
        // else references the surface, and a live context holds one reference.
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 16, 16).unwrap();
        cairo::Context::new(&surface)
            .and_then(|cr| { cr.set_source_rgb(0.0, 1.0, 0.0); cr.paint() })
            .unwrap();
        let mut one = [(16u32, surface)];
        let bytes = container(&mut one).unwrap();
        // The first entry is the true-colour image, which names no palette at all.
        for (i, entry) in directory(&bytes).iter().enumerate().skip(1) {
            let used = u32(&bytes, entry.3 + 32) as usize;
            assert_eq!(used, 2, "entry {i} of a one-colour picture names {used} colours");
            assert_eq!(&bytes[entry.3 + 40..entry.3 + 48], &[0, 0, 0, 0, 0, 255, 0, 0], "the green of the picture is not in its palette");
            assert_eq!(entry.1, 2, "entry {i} hides the palette its image holds");
        }
    }

    /// The two words of an entry are the length of its image and then its place, in that order. Exchange them
    /// and the file places images inside its own directory and names lengths that reach past its end, which is
    /// a file this Windows counts no icons in (measured here, docs\WINDOWS.md). Every image begins behind the
    /// directory, and the image's own header says how many bytes of pixels and mask follow it.
    #[test]
    fn an_entry_names_the_length_of_its_image_before_its_place() {
        let mut all = pictures(Shape::UsbStick);
        let bytes = container(&mut all).unwrap();
        let count = usize::from(u16(&bytes, 4));
        for i in 0..count {
            let e = 6 + i * 16;
            let (length, place) = (usize::try_from(u32(&bytes, e + 8)).unwrap(), usize::try_from(u32(&bytes, e + 12)).unwrap());
            assert!(place >= 6 + 16 * count, "entry {i} places its image inside the directory itself");
            assert!(place + length <= bytes.len(), "entry {i} names {} bytes of image where only {} are", length, bytes.len() - place);
            assert_eq!(u32(&bytes, place), 40, "entry {i} does not point where its image begins");
            let pixels = u32(&bytes, place + 20) as usize; // the pixels and the mask, as the image counts them
            assert_eq!(40 + u32(&bytes, place + 32) as usize * 4 + pixels, length, "entry {i} counts its image differently than the image does");
        }
    }

    /// A side that is no even number leaves one nibble over at the end of every four-bit row. The picture fills
    /// the pairs it has pixels for and the padding nibble names the transparent slot, which its mask covers
    /// anyway; a row that ran off the picture's own pixels instead would read past them.
    #[test]
    fn a_picture_of_an_odd_side_still_writes_every_row_of_its_four_bit_image() {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 17, 17).unwrap();
        cairo::Context::new(&surface)
            .and_then(|cr| { cr.set_source_rgb(0.0, 1.0, 0.0); cr.paint() })
            .unwrap();
        let mut one = [(17u32, surface)];
        let bytes = container(&mut one).unwrap();
        let entry = directory(&bytes).into_iter().find(|&(_, _, bits, _, _)| bits == 4).expect("a file with no four-bit image");
        let (place, length) = (entry.3, entry.4);
        assert_eq!(u32(&bytes, place + 4), 17, "the four-bit image of a 17 pixel picture names another width");
        let (pixels_at, xor_row, and_row, whole) = layout(17, Depth::Palette4, u32(&bytes, place + 32) as usize);
        assert_eq!(length, whole, "the file counts the four-bit image differently than it is written");
        assert_eq!(xor_row, 12, "a row of 17 four-bit pixels is not nine bytes padded to a whole four");
        assert_eq!(and_row, 4);
        let first = &bytes[place + pixels_at..][..12]; // the file's first row is the picture's last one
        assert!(first[..8].iter().all(|&b| b == 0x11), "a pair of the row is not the picture's colour twice: {:#06b}", first[0]);
        assert_eq!(first[8] & 0xf0, 0x10, "the odd pixel out of the row is missing: {:#06b}", first[8]);
        assert_eq!(first[8] & 0x0f, 0, "the nibble over the picture's own pixels names a colour: {:#06b}", first[8]);
        assert_eq!(&first[9..], &[0, 0, 0], "the row is not padded with nothing to its own four-byte width");
    }

    #[test]
    fn a_picture_that_is_not_the_size_it_names_is_refused() {
        let square = || cairo::ImageSurface::create(cairo::Format::ARgb32, 32, 32).unwrap();
        let mut other = [(48u32, square())];
        assert!(container(&mut other).is_err());
        let mut too_big = [(257u32, square())];
        assert!(container(&mut too_big).is_err(), "a size the directory cannot name is accepted");
        let mut no_side = [(0u32, square())];
        assert!(container(&mut no_side).is_err());
        let mut none: Vec<(u32, cairo::ImageSurface)> = Vec::new();
        assert!(container(&mut none).is_err());
    }
}
