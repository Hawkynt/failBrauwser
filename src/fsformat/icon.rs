//! The four files a device icon consists of (README: *Device icons*): the image file `DevIcon.fil`, the
//! folder's own `folder.ico`, and the two text files that point at them and pin the volume's long name.
//!
//! The bytes of the two icon files are what `super::ico::container` writes for this shape and these words;
//! the text files are written here, in the encodings this machine was measured to read (UTF-16 with a
//! byte-order mark for `desktop.ini`, plain ASCII for `autorun.inf`) and with a section of our own that
//! *Remove device icon* removes by. Removal never sweeps: it reads the two text files, and only what a
//! marked file names is deleted, which is what keeps somebody else's `autorun.inf` intact. The same reading
//! answers what those files *say* — [`shown`] names the volume by the long name a marked file pins and gives
//! it the picture that file names — so a volume is listed as its own files describe it (README: *Device
//! icons*), and a file without the section of ours still says nothing at all. What that reading is given has a
//! bound, and it is a bound of this module rather than a hope about the file: a text file longer than the two of
//! ours ever are is not read at all — by the listing or by the removal that reads the same names — a pinned name
//! no row could show pins nothing, and a picture that is no icon file, or that lies about where its own images
//! sit or how large they are, names nothing. Reading stops at those bounds, so neither the size nor the shape of
//! a file in somebody's volume root decides how much memory or time goes into a row, and no decoder is handed a
//! container whose header names pixels no icon holds. Each answer falls back on its own: a nonsense name costs a
//! volume its pinned name alone, and a nonsense picture its picture alone.
//!
//! The attributes — the two bits a file is hidden and system by, and the root's own mark that makes the
//! shell read `desktop.ini` at all — belong to this machine, so the tests that speak of them speak of it.

use super::{NamePlan, Shape, art, ico, decimal_size, vendor_token};
use crate::osprim;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

/// The four files one device icon consists of.
pub const AUTORUN: &str = "autorun.inf";
pub const DESKTOP: &str = "desktop.ini";
pub const FOLDER_ICON: &str = "folder.ico";
pub const DEVICE_IMAGE: &str = "DevIcon.fil";

/// The section both text files carry, and the keys of it that removal reads.
pub const SECTION: &str = "failBrauwser";
const MARK_KEY: &str = "deviceicon";
const IMAGE_KEY: &str = "image";
const ROOT_KEY: &str = "rootmarked";

/// How far a text file of ours is read at all. Measured 2026-11-19 on a real volume, the two of them are 219 and
/// 300 bytes; anything past this bound is no file this feature wrote, carries no marker this feature obeys, and
/// is not read by the listing or by the removal that reads the same two names — so neither the reading nor the
/// taking-away spends time or memory on the length of a file somebody else left in a volume root.
const TEXT_LIMIT: u64 = 64 * 1024;

/// How far a picture file is read at all: the two icon files of one written device icon measure 408 966 bytes
/// each, and a named picture past this bound names nothing this listing draws.
const PICTURE_LIMIT: u64 = 8 * 1024 * 1024;

/// The longest a pinned name can be: one component of a path is never longer than this many characters, and a
/// file naming more pins nothing rather than filling a row with what no volume is called.
const PIN_LIMIT: usize = 255;

/// What a create wrote, and whether marking the root came from here.
pub struct Written {
    pub files: Vec<String>,
    pub marked_root: bool,
}

/// What a removal took away, what it left because nothing of ours marked it, and whether the root's mark
/// came back off.
pub struct Removed {
    pub deleted: Vec<String>,
    pub kept: Vec<String>,
    pub unmarked_root: bool,
}

/// How the two text files that carry a name came out (README: *Renaming a volume*): standing, taken back, or
/// never needed. The naming act tells its answer by these three, because a volume whose name fits its file
/// system outright has nothing to pin.
#[derive(Debug)]
pub enum Pins {
    /// The two text files stand — written now, or refreshed where a device icon already wrote them — naming
    /// which, and whether marking the root came from here.
    Stood { files: Vec<String>, marked_root: bool },
    /// The marked text files of ours were taken back, naming which, and whether the root's mark came off.
    Taken { files: Vec<String>, unmarked_root: bool },
    /// Nothing of ours stands there, and the name needs no pin.
    Nothing,
}

/// Why the four files did not stand. The words tell what happened, in the file's own name; `denied` says
/// whether the rights were what was missing, which is the one thing that tells the act whether to ask the
/// administrator for the same write (README: *Device icons*).
#[derive(Debug)]
pub struct Failure {
    pub text: String,
    pub denied: bool,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<String> for Failure {
    fn from(text: String) -> Self {
        Self { text, denied: false }
    }
}

/// The machine's own answer for a name another program stands on: 32 and 33 on Windows (a sharing conflict,
/// a lock violation) and 11 elsewhere (a resource that is temporarily unavailable).
#[cfg(windows)]
const HELD: [Option<i32>; 2] = [Some(32), Some(33)];
#[cfg(not(windows))]
const HELD: [Option<i32>; 1] = [Some(11)];

/// The machine's own answer for a name whose rights refuse the write.
#[cfg(windows)]
const OUTSIDE: [Option<i32>; 1] = [Some(5)];
#[cfg(not(windows))]
const OUTSIDE: [Option<i32>; 2] = [Some(1), Some(13)];

/// Whether another program stands at that name right now: an open that permits no sharing at all is answered
/// by the machine's own sharing conflict for as long as any handle of another program holds the name.
#[cfg(windows)]
fn stands_held(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    matches!(std::fs::OpenOptions::new().read(true).share_mode(0).open(path), Err(e) if HELD.contains(&e.raw_os_error()))
}

/// Where this host cannot ask, the machine's own code is the only answer there is; recorded, never run here
/// (docs/WINDOWS.md).
#[cfg(not(windows))]
fn stands_held(_: &Path) -> bool { false }

/// The refusal of one named file: this machine's own words, told apart by the code behind them and, where that
/// code is the one a held name shares with a name that lacks rights, by asking the machine whether a handle
/// stands at the name. Only the rights answer sends the act to the administrator — a name another program
/// holds open is no question of rights, and an elevated run cannot take that name away from whoever holds it
/// (README: *Device icons*). Measured here: the move of a staged file onto a name held with no sharing at all
/// answers access denied, the same code a read-only name answers, so the code alone would guess.
fn refused(path: &Path, name: &str, what: &str, e: std::io::Error) -> Failure {
    let code = e.raw_os_error();
    let (hint, denied) = if HELD.contains(&code) || stands_held(path) { ("another program holds that name open", false) }
        else if OUTSIDE.contains(&code) { ("this folder refuses the write", true) }
        else { ("the machine answered so", false) };
    Failure { text: format!("{name} {what}: {e} ({hint})"), denied }
}

/// The words of `autorun.inf` are plain ASCII, because that file is a single-byte one: the shown label of
/// a µSD card or a 5.25″ floppy holds characters it cannot carry, and the shape's data name is what the
/// file says instead. A value left with nothing of itself by that rule is left out of the file entirely.
fn ascii(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii() && !matches!(c, '\r' | '\n')).collect::<String>().trim().to_string()
}

/// The section of our own, which says that the file is this feature's, which image file belongs to it, and
/// whether the root's system attribute was already there before this feature put it there. The image key
/// stands only where a picture of ours stands with the file: a naming act that wrote no picture names none,
/// so removal never takes a file it did not write (README: *Renaming a volume*).
fn section(marked_root: bool, picture: bool) -> Vec<String> {
    let mut lines = vec![String::new(), format!("[{SECTION}]"), "DeviceIcon=1".to_string()];
    if picture { lines.push(format!("Image={DEVICE_IMAGE}")); }
    lines.push(format!("RootMarked={}", usize::from(marked_root)));
    lines
}

/// The brand the machine reported, in the single-byte words that file holds.
fn brand(model: &str) -> Option<String> {
    let token = ascii(&vendor_token(model));
    (!token.is_empty()).then_some(token)
}

/// The body of `autorun.inf`: the drive's picture where one stands, the label the volume carries itself
/// where a single-byte file can hold it, and the two sections that describe the device.
fn autorun_body(name: &NamePlan, device: &str, maker: Option<&str>, picture: bool, marked_root: bool) -> String {
    let mut lines = vec!["[autorun]".to_string()];
    if picture { lines.push(format!("icon={FOLDER_ICON},0")); }
    if !name.label.is_empty() && name.label.is_ascii() { lines.push(format!("label={}", ascii(&name.label))); }
    if !device.is_empty() {
        lines.extend(["".to_string(), "[DeviceDescription]".to_string(), format!("ModelName={device}")]);
        if let Some(maker) = maker.filter(|maker| !maker.is_empty()) {
            lines.extend(["".to_string(), "[DeviceManufacturerInformation]".to_string(), format!("Manufacturer={maker}")]);
        }
    }
    lines.extend(section(marked_root, picture));
    lines.join("\r\n") + "\r\n"
}

/// The body of `desktop.ini`: the folder's picture where one stands, and the name the volume is called
/// where it is worth pinning.
fn desktop_body(pin: Option<&str>, picture: bool, marked_root: bool) -> String {
    let mut lines = vec!["[.ShellClassInfo]".to_string()];
    if picture { lines.push(format!("IconResource={FOLDER_ICON},0")); }
    if let Some(pin) = pin.filter(|pin| !pin.trim().is_empty()) { lines.push(format!("LocalizedResourceName={}", pin.trim())); }
    lines.extend(section(marked_root, picture));
    lines.join("\r\n") + "\r\n"
}

/// The drive's own `autorun.inf`: the icon for the drive, the device as the machine described it, and the
/// label the volume carries itself where a single-byte file can hold it.
pub fn autorun_text(shape: Shape, model: &str, bytes: u64, name: &NamePlan, marked_root: bool) -> String {
    autorun_body(name, &format!("{} {}", ascii(shape.data_label()), ascii(&decimal_size(bytes))), brand(model).as_deref(), true, marked_root)
}

/// The folder's own `desktop.ini`: its picture, and the long name the volume's own label may be too short
/// for.
pub fn desktop_text(name: &NamePlan, marked_root: bool) -> String {
    desktop_body(Some(&name.called), true, marked_root)
}

/// The bytes of `desktop.ini`: UTF-16 with its mark, which is what this machine renders rather than mangles
/// (measured 2026-11-18, docs\WINDOWS.md).
fn desktop_bytes(text: &str) -> Vec<u8> {
    let mut out = vec![0xFFu8, 0xFE];
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// The text of a file written on this machine or on another one: UTF-16 either way of its mark, and
/// otherwise the bytes as they are, which is enough to read a marker made of ASCII.
pub fn decode(bytes: &[u8]) -> String {
    let big = bytes.starts_with(&[0xFE, 0xFF]);
    if !big && !bytes.starts_with(&[0xFF, 0xFE]) { return String::from_utf8_lossy(bytes).into_owned(); }
    let units = bytes[2..].as_chunks::<2>().0.iter().map(|c| if big { u16::from_be_bytes(*c) } else { u16::from_le_bytes(*c) });
    String::from_utf16_lossy(&units.collect::<Vec<_>>())
}

/// The keys of one section, lower-cased as the shell's own readers spell them.
fn keys(text: &str, section: &str) -> Vec<(String, String)> {
    let mut inside = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line.trim_matches(|c| c == '[' || c == ']').eq_ignore_ascii_case(section);
            continue;
        }
        if !inside { continue; }
        if line.starts_with(';') { continue; }
        if let Some((key, value)) = line.split_once('=') { out.push((key.trim().to_lowercase(), value.trim().to_string())); }
    }
    out
}

/// Whether this text file is one this feature wrote: the section of ours, saying the icon stands here.
pub fn marked(text: &str) -> bool {
    keys(text, SECTION).iter().any(|(key, value)| key == MARK_KEY && value == "1")
}

/// The file opened to be read as far as `limit` bytes at most: an ordinary file of that size or smaller, and
/// nothing otherwise — a folder, a device, or a file somebody made enormous says nothing to this feature, and
/// nothing of it is read to find that out.
fn open_within(path: &Path, limit: u64) -> Option<(std::fs::File, u64)> {
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > limit { return None; }
    Some((file, meta.len()))
}

/// The text of one of the two names of ours, read no further than [`TEXT_LIMIT`] and answered only where it
/// carries the section of ours. Nobody's file is read past that bound, and a file past it carries no marker this
/// feature obeys — which is what keeps a planted giant of a text file out of the listing and out of *Remove
/// device icon* alike, rather than merely out of one of the two.
fn marked_text(root: &Path, name: &str) -> Option<String> {
    let (mut file, len) = open_within(&root.join(name), TEXT_LIMIT)?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.by_ref().take(TEXT_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > TEXT_LIMIT { return None; } // the file grew while it was being read
    let text = decode(&bytes);
    marked(&text).then_some(text)
}

/// Whether the picture a marked file names can be shown at all: an icon file honest about its own images, within
/// [`PICTURE_LIMIT`] bytes and with every image of it within the size such a directory names (README: *Device
/// icons*). Only what [`super::ico`] needs to answer that is read, so the size of the file costs the answer
/// nothing, and bytes that are no icon — a half-written file, an archive's own, an invented one — name no
/// picture and never reach a decoder.
pub fn picture_can_be_shown(path: &Path) -> bool {
    let (mut file, len) = match open_within(path, PICTURE_LIMIT) {
        Some(opened) => opened,
        None => return false,
    };
    super::ico::pictures_are_ones_a_file_can_hold(&mut file, len)
}

/// The bytes of such a picture, read through the very file the answer above was read from, so a file cannot take
/// a different shape between the answer and the read. Nothing where that answer is no, and never more than
/// [`PICTURE_LIMIT`] bytes however long the file has become meanwhile.
pub fn picture_bytes(path: &Path) -> Option<Vec<u8>> {
    let (mut file, len) = open_within(path, PICTURE_LIMIT)?;
    if !super::ico::pictures_are_ones_a_file_can_hold(&mut file, len) { return None; }
    file.rewind().ok()?;
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(PICTURE_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= PICTURE_LIMIT).then_some(bytes)
}

/// The name a file pins, when a row can show it: not empty, no longer than [`PIN_LIMIT`] characters, and carrying
/// neither a control character nor a directional override — the latter would let a file make a row read back
/// from what it says, which is how a name that was never earned looks like one that was. Nothing otherwise, and
/// the volume keeps the name the machine gives it.
fn pin_worth_shown(value: &str) -> Option<String> {
    let pin = value.trim().trim_matches('"').trim();
    if pin.is_empty() || pin.chars().count() > PIN_LIMIT { return None; }
    pin.chars().all(|c| !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')).then(|| pin.to_string())
}

/// The plain file name one key points at, read the way the shell's own readers spell it: without the index
/// suffix behind a comma and without quotes around it. A name that is empty, walks out of the root, hides behind
/// a dot, ends at one, carries a character no path can hold, or is longer than one component of a path ever is,
/// is no name this module obeys, whatever a damaged file says about it.
fn named_here(value: &str) -> Option<String> {
    let name = value.split(',').next().unwrap_or_default().trim().trim_matches('"').to_string();
    (!name.is_empty()
        && !name.contains(['/', '\\'])
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name.len() <= PIN_LIMIT
        && !name.chars().any(char::is_control))
        .then_some(name)
}

/// The plain file names one marked text file points at, in the order its keys name them.
fn named_files(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (section, key) in [("autorun", "icon"), (".shellclassinfo", "iconresource"), (SECTION, IMAGE_KEY)] {
        for (found, value) in keys(text, section) {
            if found != key { continue; }
            if let Some(name) = named_here(&value).filter(|name| !out.contains(name)) { out.push(name); }
        }
    }
    out
}

/// Whether the marking of the root falls to this feature: the pin shows only under the system attribute, and
/// where the root carries it already nobody of ours has to put it there. The answer is the machine's own answer
/// to this very write, not a guess from the absence of a mark — the root of this Windows' exFAT stick refuses
/// the mark outright (measured 2026-11-19, the write itself answered an error) — and the probe is taken back
/// off again, since the marking itself belongs after the files of the act stand: an act that cannot finish
/// leaves no mark of its own behind, and a marker never claims a marking the file system refused.
fn mark_root(root: &Path) -> bool {
    if osprim::attrs_of(root) & osprim::SYSTEM != 0 { return false; }
    osprim::set_attr_bits(root, osprim::SYSTEM | osprim::HIDDEN, 0);
    let stands = osprim::attrs_of(root) & osprim::SYSTEM != 0;
    if stands { osprim::set_attr_bits(root, 0, osprim::SYSTEM | osprim::HIDDEN); }
    stands
}

/// The name one of our files is staged under until its bytes stand: the file's own name with this behind it,
/// which is also what tells a later removal what an act left half-written (README: *Device icons*).
const STAGED: &str = ".fbtmp";

/// One file of this feature, arriving in two moves: written under a name of ours, its length read back where it
/// stands, its attributes put on it, and only then moved into the name the systems read. A slot therefore never
/// holds a file of ours that was opened but not filled — not while the act runs and not after it was refused —
/// and a name somebody else holds open is met at the move, where the machine answers instead of waiting.
fn put(root: &Path, name: &str, bytes: &[u8], hidden: bool) -> Result<(), Failure> {
    let staged = root.join(format!("{name}{STAGED}"));
    std::fs::write(&staged, bytes).map_err(|e| refused(&staged, name, "could not be staged", e))?;
    let stood = std::fs::metadata(&staged).map(|meta| meta.len()).unwrap_or_default();
    if stood != bytes.len() as u64 {
        let _ = std::fs::remove_file(&staged);
        return Err(Failure { text: format!("{name} would carry {} bytes and answers {stood} of them where it stands", bytes.len()), denied: false });
    }
    osprim::set_attr_bits(&staged, if hidden { osprim::HIDDEN | osprim::SYSTEM } else { osprim::HIDDEN }, 0);
    if let Err(e) = std::fs::rename(&staged, root.join(name)) {
        let _ = std::fs::remove_file(&staged);
        return Err(refused(&root.join(name), name, "could not be put in place", e));
    }
    Ok(())
}

/// Taking back what an act wrote, newest first, and naming what would not go away: a file still standing after
/// its own act was refused is told to the person looking at the volume rather than dropped quietly.
fn undo(root: &Path, written: &mut Vec<String>) -> Vec<String> {
    let mut left = Vec::new();
    while let Some(name) = written.pop() {
        if let Err(e) = std::fs::remove_file(root.join(&name)) { left.push(format!("{name} ({e})")); }
    }
    left
}

/// The words of a refusal that could not undo everything of its own.
fn with_residue(why: Failure, left: &[String]) -> Failure {
    if left.is_empty() { return why; }
    Failure { text: format!("{} — and these of ours stand there still: {}", why.text, left.join(", ")), denied: why.denied }
}

/// The four files of one device icon, in one act: what cannot be written is not written at all, and what
/// was written before the refusal goes away again.
pub fn create(root: &Path, shape: Shape, model: &str, bytes: u64, name: &NamePlan) -> Result<Written, Failure> {
    let marked_root = mark_root(root);
    let mut pictures = art::images(shape, model, bytes)?;
    let icon = ico::container(&mut pictures)?;
    let autorun = autorun_text(shape, model, bytes, name, marked_root);
    let desktop = desktop_text(name, marked_root);
    let mut written: Vec<String> = Vec::new();
    for (name, body, hidden) in [
        (DEVICE_IMAGE, icon.clone(), false),
        (FOLDER_ICON, icon, false),
        (AUTORUN, autorun.into_bytes(), true),
        (DESKTOP, desktop_bytes(&desktop), true),
    ] {
        if let Err(why) = put(root, name, &body, hidden) {
            return Err(with_residue(why, &undo(root, &mut written)));
        }
        written.push(name.to_string());
    }
    if marked_root { osprim::set_attr_bits(root, osprim::SYSTEM | osprim::HIDDEN, 0); }
    Ok(Written { files: written, marked_root })
}

/// The value one key of one section carries, as the file stands; the section and the key are read the way
/// the shell's own readers spell them.
fn value(text: &str, section: &str, key: &str) -> Option<String> {
    keys(text, section).into_iter().find(|(found, _)| found == key).map(|(_, value)| value)
}

/// The two text files that carry a name the volume's own label is too short for: written where nothing of
/// ours stands, refreshed where it does, and taken back again when a later name needs no pin (README:
/// *Renaming a volume*). Where no picture was asked for they say nothing about one, since a reference the
/// shell cannot resolve is worse than none; the words of a device description an icon wrote keep their own
/// value, because those words describe the device and not the volume's name. Removal of a pair that names no
/// picture goes through [`remove`], which obeys nothing but the section of ours.
pub fn name_pins(root: &Path, model: &str, name: &NamePlan) -> Result<Pins, Failure> {
    let ours: Vec<(String, String)> = [AUTORUN, DESKTOP].into_iter().filter_map(|file| marked_text(root, file).map(|text| (file.to_string(), text))).collect();
    let picture = ours.iter().flat_map(|(_, text)| named_files(text)).any(|file| file.eq_ignore_ascii_case(FOLDER_ICON) && root.join(file).is_file());
    let said = |section: &str, key: &str| ours.iter().find_map(|(_, text)| value(text, section, key));
    let marked_root = said(SECTION, ROOT_KEY).map(|said| said == "1").unwrap_or_else(|| mark_root(root));
    let pinned = name.pinned && !name.called.trim().is_empty();
    if !pinned && !picture {
        if ours.is_empty() { return Ok(Pins::Nothing); }
        let gone = remove(root)?;
        return Ok(Pins::Taken { files: gone.deleted, unmarked_root: gone.unmarked_root });
    }
    let device = said("devicedescription", "modelname").unwrap_or_else(|| if pinned { ascii(&name.called) } else { String::new() });
    let maker = said("devicemanufacturerinformation", "manufacturer").or_else(|| brand(model));
    let pin = pinned.then_some(name.called.as_str());
    let bodies = [(AUTORUN, autorun_body(name, &device, maker.as_deref(), picture, marked_root).into_bytes()), (DESKTOP, desktop_bytes(&desktop_body(pin, picture, marked_root)))];
    let mut written: Vec<String> = Vec::new();
    for (file, body) in bodies {
        if let Err(why) = put(root, file, &body, true) {
            return Err(with_residue(why, &undo(root, &mut written)));
        }
        written.push(file.to_string());
    }
    // Where the marker of a file that stood says the marking came from here, the mark goes back on a root
    // somebody has taken it off since; where this act marked the root itself it stands already.
    if marked_root { osprim::set_attr_bits(root, osprim::SYSTEM | osprim::HIDDEN, 0); }
    Ok(Pins::Stood { files: written, marked_root })
}

/// Taking a device icon away: only a text file carrying the section of ours is deleted, only the icon files
/// such a file names are deleted beside it, and the root is unmarked only where the marker says the marking
/// came from here. Nothing is matched by a pattern over the volume.
pub fn remove(root: &Path) -> Result<Removed, Failure> {
    let mut deleted: Vec<String> = Vec::new();
    let mut marked_files: Vec<String> = Vec::new();
    let mut named: Vec<String> = Vec::new();
    let mut unmark = false;
    for name in [AUTORUN, DESKTOP] {
        let Some(text) = marked_text(root, name) else { continue };
        marked_files.push(name.to_string());
        unmark |= keys(&text, SECTION).iter().any(|(key, value)| key == ROOT_KEY && value == "1");
        for file in named_files(&text) {
            if !named.contains(&file) { named.push(file); }
        }
    }
    for name in named {
        let path = root.join(&name);
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| refused(&path, name.as_str(), "could not be taken away", e))?;
            deleted.push(name);
        }
    }
    for name in marked_files {
        let path = root.join(&name);
        std::fs::remove_file(&path).map_err(|e| refused(&path, &name, "could not be taken away", e))?;
        deleted.push(name);
    }
    // A staged name of ours is ours by its name alone, so it goes with the four even where no marked text file
    // stands to name it: that is what an act interrupted between its two moves leaves behind.
    for name in [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE] {
        let staged = root.join(format!("{name}{STAGED}"));
        if !staged.is_file() { continue; }
        std::fs::remove_file(&staged).map_err(|e| refused(&staged, &format!("{name}{STAGED}"), "could not be taken away", e))?;
        deleted.push(format!("{name}{STAGED}"));
    }
    if unmark { osprim::set_attr_bits(root, 0, osprim::SYSTEM | osprim::HIDDEN); }
    let kept = [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE].iter().filter(|n| !deleted.iter().any(|d| d == *n) && root.join(n).exists()).map(|n| (*n).to_string()).collect();
    Ok(Removed { deleted, kept, unmarked_root: unmark })
}

/// What the files of ours say a volume is called and shown as, read back from the root it is mounted on
/// (README: *Device icons*). Both say nothing where the root holds no text file of ours: the volume is then
/// named by what the machine calls it and drawn with the themed picture of its kind.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    /// The long name a marked `desktop.ini` pins, empty where no marked file pins one.
    pub pinned: String,
    /// The picture a marked file names, as a path under this root; nothing where none is named or the picture
    /// named does not stand.
    pub picture: Option<PathBuf>,
}

/// The picture one marked text file names, in the order worth asking: the image our own section names, which
/// is the one this module wrote, then the folder's own picture the shell's key names. A picture that does not
/// stand, or that stands as something no icon file is, names nothing — so a file somebody took away, or wrote
/// over with nonsense, costs a picture and not the listing, and the next picture that same file names is asked.
fn picture_named(text: &str, root: &Path) -> Option<PathBuf> {
    for (section, key) in [(SECTION, IMAGE_KEY), (".shellclassinfo", "iconresource"), ("autorun", "icon")] {
        let Some(name) = value(text, section, key).as_deref().and_then(named_here) else { continue };
        let path = root.join(name);
        if picture_can_be_shown(&path) { return Some(path); }
    }
    None
}

/// What a volume's own files say about it. Only a text file carrying the section of ours is read at all: an
/// `autorun.inf` or `desktop.ini` of somebody else keeps its bytes for *Remove device icon* to keep and lends
/// this listing neither a name nor a picture, so a drive cannot rename itself into something it was never
/// named by a file an archive or another program left in its root. The name pinned is asked of the file the
/// shell renders a pinned name from, `desktop.ini`; the picture of the file named above. Neither answer is asked
/// of a file past the bounds this module reads, and neither costs the other: what one of the two files cannot
/// say, the machine says instead.
pub fn shown(root: &Path) -> Shown {
    let mut out = Shown::default();
    for name in [DESKTOP, AUTORUN] {
        let Some(text) = marked_text(root, name) else { continue };
        if out.pinned.is_empty() {
            out.pinned = value(&text, ".shellclassinfo", "localizedresourcename").and_then(|pin| pin_worth_shown(&pin)).unwrap_or_default();
        }
        if out.picture.is_none() { out.picture = picture_named(&text, root); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsformat::{FileSystem, NamePlan, Shape, art, ico, name_plan};
    use std::path::Path;

    fn plan(fs: FileSystem, long: &str) -> NamePlan {
        name_plan(fs, long).expect("a name this file system can be given")
    }

    /// A folder of its own whose attributes are taken back off before it is deleted: a leftover
    /// system-and-hidden folder in the temp tree would be a change to the test environment outliving the
    /// test that made it.
    struct Root(tempfile::TempDir);

    impl Root {
        fn fresh() -> Self {
            Self(tempfile::tempdir().expect("a folder of its own"))
        }

        fn path(&self) -> &Path {
            self.0.path()
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let all = osprim::READONLY | osprim::HIDDEN | osprim::SYSTEM;
            osprim::set_attr_bits(self.0.path(), 0, all);
            for name in [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE] {
                osprim::set_attr_bits(&self.0.path().join(name), 0, all);
            }
        }
    }

    fn read(root: &Path, name: &str) -> Vec<u8> {
        std::fs::read(root.join(name)).unwrap_or_else(|e| panic!("{name} could not be read back: {e}"))
    }

    fn text(root: &Path, name: &str) -> String {
        decode(&read(root, name))
    }

    /// The four files of one USB stick icon, in a folder of its own.
    fn create_stick(root: &Path) -> Written {
        create(root, Shape::UsbStick, "SanDisk Ultra", 64_160_400_896, &plan(FileSystem::Fat32, "64GB USB")).expect("the four files")
    }

    #[test]
    fn creating_a_device_icon_leaves_the_four_files_the_rule_names() {
        let here = Root::fresh();
        let wrote = create_stick(here.path());
        for name in [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE] {
            assert!(here.path().join(name).is_file(), "{name} is not there after the create");
        }
        assert_eq!(wrote.files.len(), 4, "the create names {} files instead of four", wrote.files.len());
        let autorun = text(here.path(), AUTORUN);
        assert!(autorun.contains("[autorun]") && autorun.contains("icon=folder.ico,0"), "the drive's own icon is unnamed: {autorun}");
        assert!(autorun.contains("ModelName=USB stick 64 GB"), "the device is not described: {autorun}");
        assert!(autorun.contains("Manufacturer=SanDisk"), "the brand the machine reported is missing: {autorun}");
        assert!(marked(&autorun) && marked(&text(here.path(), DESKTOP)), "neither text file carries the section removal reads");
        assert_eq!(autorun.matches("RootMarked=").count(), 1, "the create says twice what it did to the root");
        #[cfg(windows)]
        {
            assert!(wrote.marked_root, "a plain folder was not marked, and the shell shows no pin without the mark");
            assert!(autorun.contains("RootMarked=1"), "the create does not say that it was who marked the root: {autorun}");
            let bits = osprim::SYSTEM | osprim::HIDDEN;
            assert_eq!(osprim::attrs_of(here.path()) & bits, bits, "the root carries no mark, so the pin will not show");
            for name in [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE] {
                assert_ne!(osprim::attrs_of(&here.path().join(name)) & osprim::HIDDEN, 0, "{name} is left in plain sight");
            }
        }
    }

    /// Both icon files hold the very bytes the container writes for this shape and these words.
    #[test]
    fn the_two_icon_files_carry_the_same_twelve_images_the_container_writes() {
        let here = Root::fresh();
        create_stick(here.path());
        let mut pictures = art::images(Shape::UsbStick, "SanDisk Ultra", 64_160_400_896).expect("the four pictures");
        let want = ico::container(&mut pictures).expect("the container");
        for name in [FOLDER_ICON, DEVICE_IMAGE] {
            let got = read(here.path(), name);
            assert_eq!(got, want, "{name} is not the twelve images of this shape and these words");
            assert_eq!(u16::from_le_bytes([got[4], got[5]]), 12, "{name} holds another number of images");
        }
    }

    /// `autorun.inf` is a single-byte file, so it carries only plain ASCII words — including the shape's
    /// data name, where the shown label of a µSD card or a 5.25″ floppy holds characters that file cannot.
    #[test]
    fn autorun_inf_holds_only_plain_ascii_words_and_describes_the_device() {
        let here = Root::fresh();
        create(here.path(), Shape::MicroSd, "SanDisk Ultra", 64_160_400_896, &plan(FileSystem::Fat32, "64GB USB")).expect("the four files");
        let autorun = read(here.path(), AUTORUN);
        assert!(autorun.iter().all(|&b| b < 0x80), "a byte no single-byte file can carry stands in autorun.inf: {autorun:?}");
        let autorun = decode(&autorun);
        assert!(autorun.contains("ModelName=micro SD card 64 GB"), "the µSD card is described with a character the file cannot hold: {autorun}");

        let there = Root::fresh();
        create(there.path(), Shape::Floppy525, "", 1_474_560, &plan(FileSystem::Fat, "DISKETTE")).expect("the four files");
        let floppy = decode(&read(there.path(), AUTORUN));
        assert!(floppy.contains("ModelName=5.25 inch floppy 1.5 MB"), "the 5.25 inch floppy is described with its shown label: {floppy}");
        assert!(!marked(&decode(b"[autorun]\r\nicon=folder.ico,0\r\n")), "a file of somebody else's reads as ours");
    }

    /// The long name is pinned by `desktop.ini`, in the encoding this machine renders rather than mangles;
    /// a name the volume's own label cannot hold is not written into a single-byte file either.
    #[test]
    fn a_long_name_is_pinned_in_desktop_ini_in_utf16_with_its_mark() {
        let here = Root::fresh();
        let long = format!("Gro{}e 64 GB", char::from(0xF6));
        create(here.path(), Shape::Ssd, "Samsung SSD 870 QVO", 4_000_787_030_016, &plan(FileSystem::Ntfs, &long)).expect("the four files");
        let bytes = read(here.path(), DESKTOP);
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "desktop.ini is no UTF-16 with its mark, which is what this machine renders");
        let desktop = decode(&bytes);
        assert!(desktop.contains(&format!("LocalizedResourceName={long}")), "the long name is not pinned: {desktop}");
        assert!(desktop.contains("IconResource=folder.ico,0"), "the folder's own icon is unnamed: {desktop}");
        let autorun = decode(&read(here.path(), AUTORUN));
        assert!(!autorun.contains("label="), "a label no single-byte file can hold was written into it anyway: {autorun}");
    }

    /// Where the volume's own label holds the name, `autorun.inf` names it as well — in ASCII.
    #[test]
    fn a_name_the_volume_holds_itself_is_named_by_autorun_too() {
        let here = Root::fresh();
        create(here.path(), Shape::UsbStick, "SanDisk Ultra", 64_160_400_896, &plan(FileSystem::Fat32, "64GB USB")).expect("the four files");
        let autorun = text(here.path(), AUTORUN);
        assert!(autorun.contains("label=64GB USB"), "the label the volume carries is not named: {autorun}");
        assert!(autorun.contains("Image=DevIcon.fil"), "the image file goes unnamed, so removal has nothing to read it from: {autorun}");
    }

    /// The root is marked only where it was not marked before, and the marker says which of the two cases
    /// wrote it — so a volume that already carried the attribute keeps it.
    #[cfg(windows)]
    #[test]
    fn the_root_is_marked_only_where_the_marking_did_not_already_stand() {
        let here = Root::fresh();
        osprim::set_attr_bits(here.path(), osprim::SYSTEM, 0);
        let wrote = create_stick(here.path());
        assert!(!wrote.marked_root, "a root that was marked already is recorded as marked here");
        assert_eq!(osprim::attrs_of(here.path()) & osprim::SYSTEM, osprim::SYSTEM, "the root's own mark was taken off");
        assert!(text(here.path(), AUTORUN).contains("RootMarked=0"), "the create claims a mark it did not make: {}", text(here.path(), AUTORUN));

        let there = Root::fresh();
        create_stick(there.path());
        let bits = osprim::SYSTEM | osprim::HIDDEN;
        assert_eq!(osprim::attrs_of(there.path()) & bits, bits, "a plain root was left unmarked");
        remove(there.path()).expect("the removal takes the mark back where it put it");
        assert_eq!(osprim::attrs_of(there.path()) & bits, 0, "the mark the create set stands after the removal");
    }

    /// The marker states what the machine answers about the root, not what this feature attempted: the root of
    /// this Windows' exFAT stick refuses the system-and-hidden mark outright (measured 2026-11-19, the write
    /// itself answered an error), and a pair written there has to say the mark did not come from here rather
    /// than claim one that never stood.
    #[test]
    fn a_root_that_refuses_the_mark_is_recorded_as_never_marked() {
        let here = Root::fresh();
        assert!(!mark_root(&here.path().join("no folder of anybody here")), "a root the machine refuses to mark is recorded as marked by this feature");
        #[cfg(windows)]
        {
            let bits = osprim::SYSTEM | osprim::HIDDEN;
            assert!(mark_root(here.path()), "a plain folder takes the mark, and the marker has to name it");
            assert_eq!(osprim::attrs_of(here.path()) & bits, 0, "asking the machine left a mark of its own behind, which a failed act could never take back");
        }
    }

    /// A name another program holds open is refused **by that name**: the rights are not what is missing, so the
    /// act never asks the administrator for a write elevation cannot win, and nothing of ours stands afterwards —
    /// least of all an opened-but-unfilled file at the name the act could not fill (README: *Device icons*).
    #[cfg(windows)]
    #[test]
    fn a_name_another_program_holds_open_is_refused_by_that_name_and_leaves_nothing_of_ours() {
        use std::os::windows::fs::OpenOptionsExt;
        let here = Root::fresh();
        let theirs: &[u8] = b"a file of another program";
        let held = here.path().join(DEVICE_IMAGE);
        std::fs::write(&held, theirs).expect("the name the icon wants is somebody else's already");
        let guard = std::fs::OpenOptions::new().write(true).share_mode(0).open(&held).expect("a name held with no sharing at all");
        let Err(why) = create(here.path(), Shape::UsbStick, "SanDisk Ultra", 64_160_400_896, &plan(FileSystem::Fat32, "64 Gigabyte USB")) else {
            panic!("a name held by another program was written over anyway");
        };
        assert!(why.text.contains(DEVICE_IMAGE), "the refusal does not name the name that is held: {why}");
        assert!(!why.denied, "a held-open name reads as a question of rights, which would send the act to the administrator for nothing: {why}");
        assert!(why.text.contains("another program holds that name open"), "the refusal does not say what stood in the way: {why}");
        assert_eq!(guard.metadata().expect("the held name as it stands").len(), theirs.len() as u64, "the act left a file of its own where a held name stood");
        drop(guard);
        assert_eq!(std::fs::read(&held).expect("the held name after the refusal").len(), theirs.len(), "the act changed the bytes of the file it could not write");
        for name in [AUTORUN, DESKTOP, FOLDER_ICON] {
            assert!(!here.path().join(name).exists(), "{name} stands after an act that could not fill it");
        }
        for name in [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE] {
            assert!(!here.path().join(format!("{name}{STAGED}")).exists(), "{name}{STAGED} is left of an act that was refused");
        }
    }

    /// Measured on this Windows: putting a staged file of ours onto a name another program holds with no
    /// sharing at all is answered by the very code a name whose rights refuse the write answers, and that code's
    /// kind is the one that says the rights are missing. The code alone would therefore call a held name a
    /// rights problem and ask the administrator for a write no elevation can win — which is why the refusal
    /// asks the machine whether a handle stands at the name (README: *Device icons*).
    #[cfg(windows)]
    #[test]
    fn a_held_name_and_a_name_without_rights_are_answered_by_the_same_code_so_the_refusal_has_to_ask() {
        use std::os::windows::fs::OpenOptionsExt;
        let here = Root::fresh();
        let held = here.path().join(DEVICE_IMAGE);
        std::fs::write(&held, b"a file of another program").expect("the name the icon wants is somebody else's already");
        let guard = std::fs::OpenOptions::new().write(true).share_mode(0).open(&held).expect("a name held with no sharing at all");
        let staged = here.path().join(format!("{DEVICE_IMAGE}{STAGED}"));
        std::fs::write(&staged, b"bytes of ours").expect("the staged file of ours");
        let moved = std::fs::rename(&staged, &held).expect_err("a held name is not taken over by a move").raw_os_error();
        assert!(stands_held(&held), "the open that permits no sharing does not see the handle standing at the name");
        drop(guard);
        let _ = std::fs::remove_file(&staged);
        let readonly = here.path().join(FOLDER_ICON);
        std::fs::write(&readonly, b"").expect("a name that refuses the write by its own attribute");
        osprim::set_attrs(&readonly, osprim::READONLY);
        let written = std::fs::write(&readonly, b"bytes of ours").expect_err("a read-only name refuses the write").raw_os_error();
        assert_eq!(moved, written, "this Windows answers a held name ({moved:?}) and a name without rights ({written:?}) by two different codes, so the refusal could trust the code alone");
        assert!(!stands_held(&readonly), "the open that permits no sharing calls a name that stands unheld a held one");
    }

    /// A staged name of ours is ours by its name alone, so *Remove device icon* takes it with the four — that is
    /// what an act interrupted between its two moves leaves behind.
    #[test]
    fn a_staged_name_of_ours_goes_with_the_four_when_the_icon_is_taken_away() {
        let here = Root::fresh();
        create_stick(here.path());
        let staged = format!("{AUTORUN}{STAGED}");
        std::fs::write(here.path().join(&staged), b"half a file of ours").expect("a staged name left behind");
        let gone = remove(here.path()).expect("the removal");
        assert_eq!(gone.deleted.len(), 5, "the removal took {:?} instead of the four files and the staged one", gone.deleted);
        assert!(gone.deleted.iter().any(|name| *name == staged), "the staged name of ours was left for the next act: {:?}", gone.deleted);
        assert!(!here.path().join(staged).exists(), "the staged name stands after its own removal");
    }

    /// The other order of the two conditions (README: *Renaming a volume*, *Device icons*): the pair a naming act
    /// wrote alone gains the keys of a picture when an icon is written on that volume, so one pair ends up
    /// naming the picture, the device and the name whoever came first.
    #[test]
    fn the_pair_a_naming_act_wrote_gains_the_picture_keys_when_an_icon_is_written_on_it() {
        let here = Root::fresh();
        let long = "64 Gigabyte USB";
        name_pins(here.path(), "SanDisk Ultra", &plan(FileSystem::Fat32, long)).expect("the pair of a naming act");
        create_stick(here.path());
        let autorun = text(here.path(), AUTORUN);
        assert!(autorun.contains("icon=folder.ico,0") && autorun.contains("Image=DevIcon.fil"), "the picture an icon wrote is unnamed in a pair that stood there: {autorun}");
        assert!(autorun.contains("ModelName=USB stick 64 GB") && autorun.contains("Manufacturer=SanDisk"), "the device description did not arrive with the icon: {autorun}");
        let desktop = text(here.path(), DESKTOP);
        assert!(desktop.contains("IconResource=folder.ico,0"), "the folder's own picture went unnamed: {desktop}");
        for name in [FOLDER_ICON, DEVICE_IMAGE] {
            assert!(here.path().join(name).is_file(), "{name} is not there after the icon of that shape");
        }
    }

    #[test]
    fn removing_a_device_icon_takes_exactly_the_files_it_wrote() {
        let here = Root::fresh();
        create_stick(here.path());
        let gone = remove(here.path()).expect("the removal");
        assert_eq!(gone.deleted.len(), 4, "the removal took {:?} instead of the four files", gone.deleted);
        for name in [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE] {
            assert!(!here.path().join(name).exists(), "{name} survived its own removal");
        }
        assert!(gone.kept.is_empty(), "the removal found something to keep in a folder it wrote alone: {:?}", gone.kept);
        #[cfg(windows)]
        assert!(gone.unmarked_root, "the root the create marked stays marked after the removal");
    }

    /// The marker is what makes removal safe: a text file without it keeps every byte, and so does an icon
    /// file no marked file names.
    #[test]
    fn an_autorun_inf_of_somebody_else_keeps_every_byte_it_had() {
        let here = Root::fresh();
        let foreign = b"[autorun]\r\nicon=their.ico\r\nopen=their.exe\r\n";
        std::fs::write(here.path().join(AUTORUN), foreign).expect("a foreign autorun.inf");
        std::fs::write(here.path().join(FOLDER_ICON), b"not ours at all").expect("an icon file of somebody else");
        let left = remove(here.path()).expect("the removal");
        assert_eq!(read(here.path(), AUTORUN), foreign, "a foreign autorun.inf was rewritten or taken away");
        assert_eq!(read(here.path(), FOLDER_ICON), b"not ours at all", "an icon file no marked file names was taken away");
        assert!(left.deleted.is_empty(), "the removal took something no marker covered: {:?}", left.deleted);
        assert_eq!(left.kept, vec![AUTORUN.to_string(), FOLDER_ICON.to_string()], "the removal does not say what it left and why");
        assert_eq!(osprim::attrs_of(here.path()) & (osprim::SYSTEM | osprim::HIDDEN), 0, "the root of a folder nothing was written into was marked anyway");
    }

    /// A marked file names its icon files by plain name; a name that walks out of the root is no name this
    /// removal obeys.
    #[test]
    fn a_marked_file_names_its_icon_files_and_nothing_outside_the_root() {
        let here = Root::fresh();
        let desktop = "[.ShellClassInfo]\r\nIconResource=folder.ico,0\r\n[failBrauwser]\r\nDeviceIcon=1\r\nImage=..\\outside.ico\r\nRootMarked=0\r\n";
        std::fs::write(here.path().join(DESKTOP), desktop.as_bytes()).expect("a marked desktop.ini");
        std::fs::write(here.path().join(FOLDER_ICON), b"ours").expect("the named icon");
        let outside = here.path().join("..").join("outside.ico");
        std::fs::write(&outside, b"not in this root").expect("a file beside the root");
        let gone = remove(here.path()).expect("the removal");
        assert_eq!(gone.deleted, vec![FOLDER_ICON.to_string(), DESKTOP.to_string()], "the removal took {:?}", gone.deleted);
        assert!(outside.exists(), "a name walking out of the root was obeyed and its file deleted");
        std::fs::remove_file(&outside).expect("the file beside the root is left for the next test run");
    }

    /// A create that cannot finish leaves nothing of itself behind: the four files are one act, not four.
    /// One of the four names is held by a directory here, so the two files written in front of it have to be
    /// taken back again — and a name that refused the write is still the name that refused it.
    #[test]
    fn a_write_that_cannot_finish_leaves_nothing_of_itself_behind() {
        let here = Root::fresh();
        std::fs::create_dir(here.path().join(AUTORUN)).expect("a name that no file can be written to");
        let answer = create(here.path(), Shape::UsbStick, "SanDisk Ultra", 64_160_400_896, &plan(FileSystem::Fat32, "64GB USB"));
        assert!(answer.is_err(), "a create that could not write its files claimed success");
        for name in [DEVICE_IMAGE, FOLDER_ICON, DESKTOP] {
            assert!(!here.path().join(name).exists(), "{name} was left behind by a create that failed");
        }
        assert!(here.path().join(AUTORUN).is_dir(), "the name that refused the write was changed by it");
        assert_eq!(osprim::attrs_of(here.path()) & (osprim::SYSTEM | osprim::HIDDEN), 0, "a failed create marked the root");
    }

    /// Where the rights are what is missing, the refusal says so: that is the one thing the act has to know
    /// before it asks the administrator for the same write (README: *Device icons*). This machine's own way
    /// of refusing a write to one of these names is a file of that name carrying the read-only attribute —
    /// a read-only *folder* refuses nothing, which is what an earlier version of this test measured.
    #[cfg(windows)]
    #[test]
    fn a_name_that_refuses_the_write_says_that_the_rights_are_what_is_missing() {
        let here = Root::fresh();
        let held = here.path().join(DEVICE_IMAGE);
        std::fs::write(&held, b"").expect("a file the create wants to write over");
        osprim::set_attrs(&held, osprim::READONLY);
        let Err(why) = create(here.path(), Shape::UsbStick, "SanDisk Ultra", 64_160_400_896, &plan(FileSystem::Fat32, "64GB USB")) else {
            panic!("a name that refuses the write was written over anyway");
        };
        assert!(why.denied, "the refusal does not name the rights as what is missing: {}", why.text);
        assert!(why.text.contains(DEVICE_IMAGE), "the refusal does not name the file it could not write: {}", why.text);
    }

    /// The naming act's own pair: a name the file system cannot hold is pinned without a picture being
    /// asked for, and those two files say nothing about a picture (README: *Renaming a volume*).
    #[test]
    fn a_name_a_naming_act_pins_stands_without_any_word_about_a_picture() {
        let here = Root::fresh();
        let pins = name_pins(here.path(), "Samsung Flash Drive", &plan(FileSystem::Fat32, "Samsung 64 GB")).expect("the two files of the pin");
        let Pins::Stood { files, .. } = &pins else { panic!("the pin of a name came out as {pins:?}") };
        assert_eq!(files.len(), 2, "the pin stands as {} files instead of two", files.len());
        for name in [AUTORUN, DESKTOP] {
            assert!(here.path().join(name).is_file(), "{name} is not there after the pin");
        }
        for name in [DEVICE_IMAGE, FOLDER_ICON] {
            assert!(!here.path().join(name).is_file(), "a pin of a name drew a picture of its own: {name} stands");
        }
        let autorun = text(here.path(), AUTORUN);
        assert!(autorun.contains("ModelName=Samsung 64 GB"), "the name the volume is called is not what the pin says about it: {autorun}");
        assert!(autorun.contains("Manufacturer=Samsung"), "the brand the machine reported is missing: {autorun}");
        assert!(autorun.contains("label=SAMSUNG 64"), "the short label the file system carries is unnamed: {autorun}");
        assert!(!autorun.contains("icon="), "the pin names a picture that was never asked for: {autorun}");
        assert!(!autorun.contains(DEVICE_IMAGE) && !autorun.contains(FOLDER_ICON), "the pin names an icon file that does not stand: {autorun}");
        let desktop = text(here.path(), DESKTOP);
        assert!(desktop.contains(&format!("LocalizedResourceName={}", plan(FileSystem::Fat32, "Samsung 64 GB").called)), "the long name is not pinned: {desktop}");
        assert!(!desktop.contains("IconResource"), "the folder is pointed at a picture that was never asked for: {desktop}");
        assert!(marked(&autorun) && marked(&desktop), "neither file of the pin carries the section removal reads");
    }

    /// A volume that already carries a device icon keeps its picture and its device words under a rename;
    /// only the name changes.
    #[test]
    fn pinning_a_volume_that_carries_an_icon_keeps_every_word_of_that_icon() {
        let here = Root::fresh();
        create_stick(here.path());
        let Pins::Stood { files, .. } = &name_pins(here.path(), "SanDisk Ultra", &plan(FileSystem::Fat32, "Family Pictures of the summer holidays")).expect("the pair refreshed under the icon") else {
            panic!("the pair under a standing icon was not refreshed");
        };
        assert_eq!(files.len(), 2, "the icon's pair came back as {} files", files.len());
        for name in [DEVICE_IMAGE, FOLDER_ICON] {
            assert!(here.path().join(name).is_file(), "the picture of an icon that stands was taken away by a name: {name}");
        }
        let autorun = text(here.path(), AUTORUN);
        assert!(autorun.contains("icon=folder.ico,0"), "the drive's own picture lost its name under the rename: {autorun}");
        assert!(autorun.contains("ModelName=USB stick 64 GB") && autorun.contains("Manufacturer=SanDisk"), "the device the icon described was rewritten as a volume name: {autorun}");
        assert!(autorun.contains("label=FAMILY"), "the label the volume now carries is unnamed: {autorun}");
        let desktop = text(here.path(), DESKTOP);
        assert!(desktop.contains("IconResource=folder.ico,0"), "the folder's own picture lost its name under the rename: {desktop}");
        assert!(desktop.contains("LocalizedResourceName=Family Pictures of the summer"), "the new name is not pinned: {desktop}");
    }

    /// A later name the file system holds outright leaves no stale pin of ours behind; the two files go and
    /// the root's mark comes back off where it came from us.
    #[test]
    fn a_name_the_file_system_holds_itself_takes_the_pin_of_our_own_pair_back() {
        let here = Root::fresh();
        name_pins(here.path(), "Samsung Flash Drive", &plan(FileSystem::Fat32, "Samsung 64 GB")).expect("the pair of the pin");
        let pins = name_pins(here.path(), "Samsung Flash Drive", &plan(FileSystem::Fat32, "HOLIDAY")).expect("the pair taken back");
        let Pins::Taken { files, .. } = &pins else { panic!("a name that needs no pin left the pair standing: {pins:?}") };
        assert_eq!(files.len(), 2, "the pair came back as {} files instead of two", files.len());
        for name in [AUTORUN, DESKTOP] {
            assert!(!here.path().join(name).is_file(), "{name} still pins a name the volume no longer carries");
        }
        #[cfg(windows)]
        assert_eq!(osprim::attrs_of(here.path()) & osprim::SYSTEM, 0, "the root is left marked by a pin that is gone");
    }

    /// Where no name needs a pin and nothing of ours stands, the naming act writes nothing at all.
    #[test]
    fn a_name_the_file_system_holds_writes_no_pair_of_its_own() {
        let here = Root::fresh();
        let pins = name_pins(here.path(), "Samsung Flash Drive", &plan(FileSystem::Ntfs, "Samsung 64 GB")).expect("nothing to do");
        assert!(matches!(pins, Pins::Nothing), "a name NTFS holds outright still wrote something: {pins:?}");
        for name in [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE] {
            assert!(!here.path().join(name).exists(), "{name} stands where the act had nothing to write");
        }
    }

    /// The listing is told its name and its picture by the pair of ours: the long name that pair pins, and the
    /// image our own section names ahead of the one the shell's keys name (README: *Device icons*).
    #[test]
    fn the_pinned_name_and_the_picture_our_own_section_names_are_read_back_from_that_pair() {
        let here = Root::fresh();
        let long = "Samsung 64 GB Family Pictures";
        create(here.path(), Shape::UsbStick, "Samsung Flash Drive", 64_160_400_896, &plan(FileSystem::ExFat, long)).expect("the four files");
        let said = shown(here.path());
        assert_eq!(said.pinned, long, "the listing is not told the long name the pair pins");
        assert_eq!(said.picture, Some(here.path().join(DEVICE_IMAGE)), "the image our own section names is not the one read");
    }

    /// A pair of somebody else's — the same keys, none of them carrying the section of ours — names this
    /// listing no name and lends it no picture, exactly as removal keeps every byte of such a file.
    #[test]
    fn a_pair_of_somebody_elses_names_no_name_and_lends_no_picture() {
        let here = Root::fresh();
        std::fs::write(here.path().join(DESKTOP), "[.ShellClassInfo]\r\nLocalizedResourceName=A name nobody earned\r\nIconResource=folder.ico,0\r\n").expect("a foreign desktop.ini");
        std::fs::write(here.path().join(AUTORUN), "[autorun]\r\nicon=folder.ico\r\nlabel=EVIL\r\n").expect("a foreign autorun.inf");
        std::fs::write(here.path().join(FOLDER_ICON), "whatever an archive left there").expect("a foreign picture");
        let said = shown(here.path());
        assert!(said.pinned.is_empty(), "a name out of a file without the section of ours would be shown: {:?}", said.pinned);
        assert_eq!(said.picture, None, "a picture out of a file without the section of ours would be lent: {:?}", said.picture);
    }

    /// Of the pictures one marked file names, the one our own section names comes first; when somebody takes
    /// that one away, the picture the shell's key of the same file names is what the listing draws.
    #[test]
    fn a_picture_somebody_took_away_leaves_the_other_one_that_marked_file_names() {
        let here = Root::fresh();
        create_stick(here.path());
        assert_eq!(shown(here.path()).picture, Some(here.path().join(DEVICE_IMAGE)), "the create named no picture of ours");
        std::fs::remove_file(here.path().join(DEVICE_IMAGE)).expect("the picture of ours taken away");
        assert_eq!(shown(here.path()).picture, Some(here.path().join(FOLDER_ICON)), "the folder's own picture is not drawn once ours is gone");
    }

    /// A picture standing in the root is not drawn just because it stands: only a marked file naming it makes
    /// it the volume's picture, which is what a naming act's pair — carrying no picture keys — leaves unset.
    #[test]
    fn a_picture_no_marked_file_names_is_not_lent_to_the_listing() {
        let here = Root::fresh();
        name_pins(here.path(), "Samsung Flash Drive", &plan(FileSystem::ExFat, "Samsung 64 GB Family Pictures")).expect("the pair of the pin");
        std::fs::write(here.path().join(FOLDER_ICON), "a picture of somebody else's").expect("a picture that stands unclaimed");
        let said = shown(here.path());
        assert_eq!(said.pinned, "Samsung 64 GB Family Pictures", "the pin of a naming act is not read back");
        assert_eq!(said.picture, None, "a picture no marked file named was drawn anyway");
    }

    /// The name and picture read back say nothing at all where the root holds nothing of ours, so the volume
    /// keeps the name the machine gave it and the themed picture of its kind.
    #[test]
    fn a_root_holding_nothing_of_ours_is_read_as_saying_nothing() {
        let here = Root::fresh();
        assert_eq!(shown(here.path()), Shown::default(), "an empty root was read as naming something");
    }

    /// A text file past the size one of ours ever is says nothing at all: the two of ours on a real volume
    /// measure 219 and 300 bytes, so 64 KiB of text is no file this feature wrote, whoever wrote it and
    /// whatever marker stands inside it. The other answer of the same root is untouched — the second marked
    /// file still names the picture — because each answer falls back on its own.
    #[test]
    fn a_text_file_past_the_size_one_of_ours_ever_is_says_nothing_at_all() {
        let here = Root::fresh();
        create_stick(here.path());
        let mut body = text(here.path(), DESKTOP);
        body.push_str(&format!("\r\n{}", "x".repeat(TEXT_LIMIT as usize))); // one text file of ours plus the bound
        std::fs::write(here.path().join(DESKTOP), body).expect("a text file of a hundred times ours");
        let said = shown(here.path());
        assert!(said.pinned.is_empty(), "a name out of a file no reader of ours would finish was shown: {:?}", said.pinned);
        assert_eq!(said.picture, Some(here.path().join(DEVICE_IMAGE)), "the picture the other marked file names went with a name this feature cannot read");
    }

    /// A pinned name is shown only where a row could show it: no longer than the longest component a path ever
    /// holds, carrying no control character and no directional override that would make the row read as
    /// something its name is not. The long name of ours beside them is shown, so the bound is what refuses here
    /// and not the reading.
    #[test]
    fn a_pinned_name_no_row_could_show_pins_nothing() {
        let here = Root::fresh();
        create_stick(here.path());
        let pin = |value: &str| {
            std::fs::write(here.path().join(DESKTOP), format!("[.ShellClassInfo]\r\nLocalizedResourceName={value}\r\nIconResource=folder.ico,0\r\n[failBrauwser]\r\nDeviceIcon=1\r\nImage=DevIcon.fil\r\nRootMarked=0\r\n")).expect("a desktop.ini of this test's own");
            shown(here.path())
        };
        let long = "x".repeat(PIN_LIMIT + 1); // one character past the longest component of a path
        for (what, value) in [("a name longer than a path component", long.as_str()), ("a name carrying a control character", "USB\u{7}stick"), ("a name carrying a directional override", "stick\u{202E}gnus")] {
            let said = pin(value);
            assert!(said.pinned.is_empty(), "{what} was shown as a volume's name: {:?}", said.pinned);
            assert_eq!(said.picture, Some(here.path().join(DEVICE_IMAGE)), "{what} took the picture of the same volume with it");
        }
        assert_eq!(pin("Samsung 64 GB Family Pictures").pinned, "Samsung 64 GB Family Pictures", "a name a row can show was refused with the nonsense beside it");
    }

    /// Bytes that are no icon at all lend no picture, whatever a damaged write or a planted file left standing
    /// under a name a marked file names.
    #[test]
    fn a_picture_that_is_no_icon_at_all_lends_nothing_to_the_listing() {
        let here = Root::fresh();
        create_stick(here.path());
        for name in [DEVICE_IMAGE, FOLDER_ICON] {
            std::fs::write(here.path().join(name), "no icon: whatever a half-written act or an archive left").expect("a picture of nothing");
        }
        assert_eq!(shown(here.path()).picture, None, "a picture that is no icon was drawn anyway");
    }

    /// An icon file is believed only as far as its own directory and its own headers agree and stay inside it,
    /// and only for images as large as such a directory can name: an entry reaching past the end of its file,
    /// an image header naming more pixels than an icon holds, and an image count the file cannot hold are each a
    /// file that names no picture. The honest spelling beside them is drawn, so the refusal is the lie's doing.
    #[test]
    fn a_picture_whose_own_entries_lie_about_its_size_or_its_place_lends_nothing() {
        let here = Root::fresh();
        create_stick(here.path());
        std::fs::remove_file(here.path().join(FOLDER_ICON)).expect("the second picture out of the way");
        // One image, its directory entry naming a 256 pixel side (spelled as zero) and a header naming another;
        // the header stands at 22, which is where one entry leaves room for itself.
        let one_image_icon = |header_side: i32, length: u32, place: u32| -> Vec<u8> {
            let mut out: Vec<u8> = vec![0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 1, 0, 32, 0];
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(&place.to_le_bytes());
            out.extend_from_slice(&40u32.to_le_bytes());
            out.extend_from_slice(&header_side.to_le_bytes());
            out.extend_from_slice(&(header_side * 2).to_le_bytes());
            out.extend_from_slice(&[1, 0, 32, 0, 0, 0, 0, 0]);
            out.resize(22 + 40, 0);
            out
        };
        let lying = [
            ("an image header naming pixels no icon directory can name", one_image_icon(4096, 40, 22)),
            ("an entry placing its image past the end of its own file", one_image_icon(256, 40, 100_000)),
            ("an entry counting more image than its file holds", one_image_icon(256, 100_000, 22)),
            ("an image count the file has no room for", {
                let mut bytes = one_image_icon(256, 40, 22);
                bytes[4] = 9;
                bytes
            }),
        ];
        for (what, bytes) in &lying {
            std::fs::write(here.path().join(DEVICE_IMAGE), bytes).expect("a picture that lies about itself");
            assert_eq!(shown(here.path()).picture, None, "{what} was drawn as a volume's picture");
        }
        std::fs::write(here.path().join(DEVICE_IMAGE), one_image_icon(256, 40, 22)).expect("an honest picture");
        assert_eq!(shown(here.path()).picture, Some(here.path().join(DEVICE_IMAGE)), "a file whose entries tell the truth was refused with the liars beside it");
    }

    /// A named picture longer than the largest one of ours ever is names no picture, and it is refused from its
    /// own length: what stands past the bound is never read, and neither is the file handed to a reader at all.
    #[test]
    fn a_picture_larger_than_the_largest_one_of_ours_ever_is_names_no_picture() {
        let here = Root::fresh();
        create_stick(here.path());
        std::fs::remove_file(here.path().join(FOLDER_ICON)).expect("the second picture out of the way");
        let mut bytes = vec![0u8; PICTURE_LIMIT as usize + 1]; // the two icon files of ours measure a twentieth of this
        bytes[..6].copy_from_slice(&[0, 0, 1, 0, 1, 0]); // an honest-looking header, which is not what refuses it
        std::fs::write(here.path().join(DEVICE_IMAGE), &bytes).expect("a picture of eight megabytes and one more byte");
        assert_eq!(shown(here.path()).picture, None, "a picture past the size bound was drawn as a volume's picture");
        assert!(picture_bytes(&here.path().join(DEVICE_IMAGE)).is_none(), "the bytes of an oversized picture were read anyway");
    }

    /// A name for a picture file is obeyed only as a plain name: no control character in it, and no trailing
    /// dot a Windows path reads as nothing.
    #[test]
    fn a_picture_name_carrying_anything_a_path_cannot_carry_names_no_picture() {
        assert_eq!(named_here("DevIcon.fil").as_deref(), Some("DevIcon.fil"), "an ordinary name was refused");
        assert_eq!(named_here("Dev\u{1}Icon.fil"), None, "a name carrying a control character was obeyed");
        assert_eq!(named_here("folder.ico."), None, "a name ending in a dot was obeyed");
    }

    /// The bound holds the removal too: a text file this feature cannot read within its own bound is left to
    /// whoever put it there, exactly as an unmarked one is, while the marked files it can read go as always.
    #[test]
    fn a_marker_past_the_bound_removal_can_read_is_left_to_whoever_wrote_it() {
        let here = Root::fresh();
        create_stick(here.path());
        let mut body = text(here.path(), AUTORUN);
        body.push_str(&format!("\r\n{}", "x".repeat(65_536)));
        std::fs::write(here.path().join(AUTORUN), body).expect("a text file of a hundred times ours");
        let gone = remove(here.path()).expect("the removal's answer");
        assert!(here.path().join(AUTORUN).is_file(), "a text file past the bound was deleted as one of ours");
        assert!(gone.kept.iter().any(|name| name == AUTORUN), "the kept files do not name what was left: {:?}", gone.kept);
        assert!(!here.path().join(DESKTOP).is_file(), "the marked file removal can read was left standing too");
    }

    /// The picture this writer puts on a volume reaches pixels through the ICO loader this payload ships,
    /// which is what lets a row draw it at all. Measured 2026-11-19 on both icon files of a real create: the
    /// loader answers the **largest** of the twelve images, 256×256, and a row scales that down to the size it
    /// draws at — so the picture a row gets is the sharpest one the file holds.
    #[test]
    fn the_picture_of_a_written_icon_reaches_pixels_through_the_loader_this_payload_ships() {
        let here = Root::fresh();
        create_stick(here.path());
        for name in [DEVICE_IMAGE, FOLDER_ICON] {
            let bytes = read(here.path(), name);
            let picture = gtk::gdk_pixbuf::Pixbuf::from_read(std::io::Cursor::new(bytes)).unwrap_or_else(|e| panic!("{name} is not a picture the payload's loader reads: {e}"));
            assert_eq!((picture.width(), picture.height()), (256, 256), "{name} was decoded to {}×{} instead of the largest of the twelve", picture.width(), picture.height());
        }
    }
}
