//! The application icon as an artifact: what the painting pictures, what the program files
//! carry, and what the shell can actually find. The window itself is checked by eye against a
//! captured titlebar; these scenarios pin the assets the window and the shell read.

#[path = "support/pe.rs"]
mod pe;

use std::path::{Path, PathBuf};

const SIZES: [u16; 7] = [16, 24, 32, 48, 64, 128, 256];

fn asset(name: &str) -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("data/icons").join(name) }

fn rgb(hex: &str) -> (u16, u16, u16) {
    let n = |r: &str| u16::from_str_radix(r, 16).unwrap_or(0);
    (n(&hex[0..2]), n(&hex[2..4]), n(&hex[4..6]))
}

fn bluish(c: (u16, u16, u16)) -> bool { c.2 > c.0 + 25 && c.2 > c.1 + 20 }

/// The folder is what the icon pictures, and the two gradients are its faces.
#[test]
fn the_app_icon_painting_is_a_blue_folder() {
    let svg = std::fs::read_to_string(asset("org.failbrauwser.FailBrauwser.svg")).expect("the application icon's svg");
    let mut stops = Vec::new();
    for rest in svg.split("stop-color=\"#").skip(1) { stops.push(rgb(&rest[..6])); }
    assert!(stops.len() >= 4, "both folder faces give their gradient stops, found {}", stops.len());
    for (i, c) in stops.iter().enumerate() {
        assert!(bluish(*c), "folder face stop {} is #{:02x}{:02x}{:02x}, not blue", i + 1, c.0, c.1, c.2);
    }
}

/// The shell asks for a small set of sizes; each entry declares one and carries pixels of it.
#[test]
fn the_program_icon_carries_the_sizes_the_shell_asks_for() {
    let ico = std::fs::read(asset("failbrauwser.ico")).expect("data/icons/failbrauwser.ico - make it with winpack\\mkico.ps1");
    assert_eq!(&ico[..4], &[0, 0, 1, 0], "an icon file starts with a reserved zero and type 1");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    assert_eq!(count, SIZES.len(), "one entry per promised size, found {count}");
    let dim = |b: u8| if b == 0 { 256 } else { b as u16 };
    let mut declared = Vec::new();
    for i in 0..count {
        let e = 6 + i * 16;
        let (w, h) = (dim(ico[e]), dim(ico[e + 1]));
        assert_eq!(w, h, "entry {i} is not square: {w}x{h}");
        let (len, off) = (u32::from_le_bytes(ico[e + 8..e + 12].try_into().unwrap()) as usize, u32::from_le_bytes(ico[e + 12..e + 16].try_into().unwrap()) as usize);
        assert!(off + len <= ico.len(), "entry {i} reaches past the file");
        let p = &ico[off..off + len];
        let (pw, ph) = if p.starts_with(b"\x89PNG\r\n\x1a\n") {
            (u32::from_be_bytes(p[16..20].try_into().unwrap()) as u16, u32::from_be_bytes(p[20..24].try_into().unwrap()) as u16)
        } else {
            (i32::from_le_bytes(p[4..8].try_into().unwrap()) as u16, (i32::from_le_bytes(p[8..12].try_into().unwrap()).unsigned_abs() as u16).min(w))
        };
        assert_eq!((pw, ph), (w, h), "entry {i} declares {w}x{h} but its pixels are {pw}x{ph}");
        declared.push(w);
    }
    declared.sort_unstable();
    assert_eq!(declared, SIZES, "the icon carries the sizes the shell asks for");
}

/// The promise is a blue folder, so the shipped pixels have to be mostly blue: read the 32-bit
/// entries and count what a viewer sees.
#[test]
fn the_program_icon_shows_a_blue_folder_in_its_pixels() {
    let ico = std::fs::read(asset("failbrauwser.ico")).expect("data/icons/failbrauwser.ico - make it with winpack\\mkico.ps1");
    let mut checked = 0;
    for i in 0..u16::from_le_bytes([ico[4], ico[5]]) as usize {
        let e = 6 + i * 16;
        let (len, off) = (u32::from_le_bytes(ico[e + 8..e + 12].try_into().unwrap()) as usize, u32::from_le_bytes(ico[e + 12..e + 16].try_into().unwrap()) as usize);
        let p = &ico[off..off + len];
        let Some(px) = dib_pixels(p) else { continue };
        let (side, _) = (i32::from_le_bytes(p[4..8].try_into().unwrap()) as usize, i32::from_le_bytes(p[8..12].try_into().unwrap()));
        let (opaque, blue) = px.iter().fold((0usize, 0usize), |(o, b), &(r, g, bl, a)| if a == 0 { (o, b) } else { (o + 1, b + usize::from(bluish((r as u16, g as u16, bl as u16)))) });
        assert!(opaque > side * side / 3, "entry {i} paints too little of its square: {opaque} of {}", side * side);
        assert!(blue * 5 >= opaque * 3, "entry {i} is a blue folder in {blue} of {opaque} pixels, not in most of them");
        checked += 1;
    }
    assert!(checked >= SIZES.len() - 1, "the 32-bit entries carry readable pixels, checked {checked} of {}", SIZES.len());
}

/// Pixels of a 32-bit uncompressed DIB, top-down, alpha last; `None` for anything else.
fn dib_pixels(dib: &[u8]) -> Option<Vec<(u8, u8, u8, u8)>> {
    if dib.len() < 40 || i32::from_le_bytes(dib[0..4].try_into().unwrap()) != 40 || u16::from_le_bytes(dib[14..16].try_into().unwrap()) != 32 || u32::from_le_bytes(dib[16..20].try_into().unwrap()) != 0 {
        return None;
    }
    let (w, h) = (i32::from_le_bytes(dib[4..8].try_into().unwrap()) as usize, i32::from_le_bytes(dib[8..12].try_into().unwrap()).unsigned_abs() as usize);
    // The header counts the AND mask into the height, so the picture itself is the shorter side.
    let side = w.min(h);
    let mut out = Vec::with_capacity(w * side);
    for y in 0..side {
        let row = 40 + (side - 1 - y) * w * 4;
        if row + w * 4 > dib.len() {
            return None;
        }
        for x in 0..w {
            let p = row + x * 4;
            out.push((dib[p + 2], dib[p + 1], dib[p], dib[p + 3]));
        }
    }
    Some(out)
}

/// What the shell reads is the program file's own resource directory: RT_ICON (3) images and
/// the RT_GROUP_ICON (14) entry that gathers them into the icon a folder view shows.
#[cfg(windows)]
#[test]
fn the_built_program_offers_its_icon_to_the_shell() {
    let exe = env!("CARGO_BIN_EXE_failbrauwser");
    let bytes = std::fs::read(exe).expect("the program this build linked");
    let kinds = pe::resource_kinds(&bytes).unwrap_or_else(|why| panic!("{exe} is not a readable image: {why}"));
    assert!(kinds.contains(&14), "{exe} carries no icon group the shell can read: the build linked no icon resource");
    assert!(kinds.contains(&3), "{exe} carries no icon images behind that group");
}
