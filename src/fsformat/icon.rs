//! The four files a device icon consists of (README: *Device icons*): the image file `DevIcon.fil`, the
//! folder's own `folder.ico`, and the two text files that point at them and pin the volume's long name.
//!
//! The bytes of the two icon files are what `super::ico::container` writes for this shape and these words;
//! the text files are written here, in the encodings this machine was measured to read (UTF-16 with a
//! byte-order mark for `desktop.ini`, plain ASCII for `autorun.inf`) and with a section of our own that
//! *Remove device icon* removes by. Removal never sweeps: it reads the two text files, and only what a
//! marked file names is deleted, which is what keeps somebody else's `autorun.inf` intact.
//!
//! The attributes — the two bits a file is hidden and system by, and the root's own mark that makes the
//! shell read `desktop.ini` at all — belong to this machine, so the tests that speak of them speak of it.

use super::{NamePlan, Shape, art, ico, decimal_size, vendor_token};
use crate::osprim;
use std::path::Path;

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

/// The refusal of one named file: this machine's own words, and whether they mean the rights are missing.
fn refused(name: &str, what: &str, e: std::io::Error) -> Failure {
    Failure { text: format!("{name} {what}: {e}"), denied: e.kind() == std::io::ErrorKind::PermissionDenied }
}

/// The words of `autorun.inf` are plain ASCII, because that file is a single-byte one: the shown label of
/// a µSD card or a 5.25″ floppy holds characters it cannot carry, and the shape's data name is what the
/// file says instead. A value left with nothing of itself by that rule is left out of the file entirely.
fn ascii(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii() && !matches!(c, '\r' | '\n')).collect::<String>().trim().to_string()
}

/// The section of our own, which says that the file is this feature's, which image file belongs to it, and
/// whether the root's system attribute was already there before this feature put it there.
fn section(marked_root: bool) -> Vec<String> {
    vec![String::new(), format!("[{SECTION}]"), format!("{}=1", "DeviceIcon"), format!("Image={DEVICE_IMAGE}"), format!("RootMarked={}", usize::from(marked_root))]
}

/// The drive's own `autorun.inf`: the icon for the drive, the device as the machine described it, and the
/// label the volume carries itself where a single-byte file can hold it.
pub fn autorun_text(shape: Shape, model: &str, bytes: u64, name: &NamePlan, marked_root: bool) -> String {
    let brand = ascii(&vendor_token(model));
    let mut lines = vec!["[autorun]".to_string(), format!("icon={FOLDER_ICON},0")];
    if !name.label.is_empty() && name.label.is_ascii() { lines.push(format!("label={}", ascii(&name.label))); }
    lines.extend(["".to_string(), "[DeviceDescription]".to_string(), format!("ModelName={} {}", ascii(shape.data_label()), ascii(&decimal_size(bytes)))]);
    if !brand.is_empty() {
        lines.extend(["".to_string(), "[DeviceManufacturerInformation]".to_string(), format!("Manufacturer={brand}")]);
    }
    lines.extend(section(marked_root));
    lines.join("\r\n") + "\r\n"
}

/// The folder's own `desktop.ini`: its picture, and the long name the volume's own label may be too short
/// for.
pub fn desktop_text(name: &NamePlan, marked_root: bool) -> String {
    let mut lines = vec!["[.ShellClassInfo]".to_string(), format!("IconResource={FOLDER_ICON},0")];
    if !name.called.is_empty() { lines.push(format!("LocalizedResourceName={}", name.called.trim())); }
    lines.extend(section(marked_root));
    lines.join("\r\n") + "\r\n"
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

/// The plain file names one marked text file points at. A name that walks out of the root is no name this
/// removal obeys, whatever a damaged file says about it.
fn named_files(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (section, key) in [("autorun", "icon"), (".shellclassinfo", "iconresource"), (SECTION, IMAGE_KEY)] {
        for (found, value) in keys(text, section) {
            if found != key { continue; }
            let name = value.split(',').next().unwrap_or_default().trim().trim_matches('"').to_string();
            if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') { continue; }
            if !out.contains(&name) { out.push(name); }
        }
    }
    out
}

/// Whether marking the root falls to this feature: the pin shows only under the system attribute, and where
/// the root carries it already, nobody of ours has to put it there.
fn marks_root(root: &Path) -> bool {
    osprim::attrs_of(root) & osprim::SYSTEM == 0
}

fn put(root: &Path, name: &str, bytes: &[u8], hidden: bool) -> Result<(), Failure> {
    let path = root.join(name);
    std::fs::write(&path, bytes).map_err(|e| refused(name, "could not be written", e))?;
    osprim::set_attr_bits(&path, if hidden { osprim::HIDDEN | osprim::SYSTEM } else { osprim::HIDDEN }, 0);
    Ok(())
}

fn undo(root: &Path, written: &mut Vec<String>) {
    while let Some(name) = written.pop() {
        let _ = std::fs::remove_file(root.join(name));
    }
}

/// The four files of one device icon, in one act: what cannot be written is not written at all, and what
/// was written before the refusal goes away again.
pub fn create(root: &Path, shape: Shape, model: &str, bytes: u64, name: &NamePlan) -> Result<Written, Failure> {
    let marked_root = marks_root(root);
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
        if let Err(e) = put(root, name, &body, hidden) {
            undo(root, &mut written);
            return Err(e);
        }
        written.push(name.to_string());
    }
    if marked_root { osprim::set_attr_bits(root, osprim::SYSTEM | osprim::HIDDEN, 0); }
    Ok(Written { files: written, marked_root })
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
        let Ok(bytes) = std::fs::read(root.join(name)) else { continue };
        let text = decode(&bytes);
        if !marked(&text) { continue; }
        marked_files.push(name.to_string());
        unmark |= keys(&text, SECTION).iter().any(|(key, value)| key == ROOT_KEY && value == "1");
        for file in named_files(&text) {
            if !named.contains(&file) { named.push(file); }
        }
    }
    for name in named {
        let path = root.join(&name);
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|e| refused(name.as_str(), "could not be taken away", e))?;
            deleted.push(name);
        }
    }
    for name in marked_files {
        std::fs::remove_file(root.join(&name)).map_err(|e| refused(&name, "could not be taken away", e))?;
        deleted.push(name);
    }
    if unmark { osprim::set_attr_bits(root, 0, osprim::SYSTEM | osprim::HIDDEN); }
    let kept = [AUTORUN, DESKTOP, FOLDER_ICON, DEVICE_IMAGE].iter().filter(|n| !deleted.iter().any(|d| d == *n) && root.join(n).exists()).map(|n| (*n).to_string()).collect();
    Ok(Removed { deleted, kept, unmarked_root: unmark })
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
}
