//! The acts themselves: naming a volume, formatting it, and writing or taking away the four files of a
//! device icon (README: *Renaming a volume*, *Formatting a volume*, *Device icons*). Nothing here runs where
//! `refusal` or `icon_refusal` says it must not, and no act needs the window that
//! asked for it to hold any rights: the privileged half of every one of them is this program's own file
//! started again as the administrator, told by `--outcome <path>` which file to write its answer into; the
//! window waits for that run and reads the file back (docs\WINDOWS.md). A consent prompt that is closed without
//! agreeing is a no-op: the privileged run never starts and the volume stays as it was.
//!
//! The format tool started by the privileged run is deliberately **not** a reading client: a format that
//! was half-written must not be cut short because some window of ours closed (docs\TOOL-STARTS.md), so
//! that one start goes through no funnel at all — only through the hidden-window rule.

#[cfg(any(windows, test))]
use super::{Target, format_argv};
use super::{FormatOption, FileSystem, NamePlan, Shape, Value, accepted, carried_name, holds_our_own_image, icon, name_plan, refusal, tool_file_systems};
use crate::drives::Volume;
use std::path::{Path, PathBuf};

/// The flag this program's own file is started with to write a volume's name as the administrator.
pub const RENAME: &str = "--rename-volume";
/// …and to format one.
pub const FORMAT: &str = "--format";
/// …to write the four files of a device icon onto one volume,
pub const ICON: &str = "--device-icon";
/// …and to take them away again.
pub const UNICON: &str = "--remove-device-icon";
/// The flag that names, to a format run, the volume name its tool's own switch refuses to take — the one
/// argument of the format act that is no switch, because `/V:` answers *invalid parameter* for a value
/// holding a space (measured; README). The run writes that name itself once the format has worked.
pub const NAME: &str = "--volume-name";
/// The flag naming the file the privileged run writes its answer into.
pub const OUTCOME: &str = "--outcome";
/// The seam `src\links\helper.rs` already uses: it names another program to start this one through, so a
/// scenario never asks for rights and never shows a consent dialog.
pub const SEAM: &str = "FAILBRAUWSER_ELEVATE";

/// What naming a volume answers: the name the volume's own label carries from then on, and the words the act
/// tells — which say whether the longer name the volume is *called* stands pinned in the two text files.
#[derive(Debug)]
pub struct Named {
    pub label: String,
    pub words: String,
}

/// The name of one volume: the gate first, then the plan that settles what the file system can hold, then the
/// label, then the pair of text files that carries a name the label cannot hold. The plan is what the caller
/// needs afterwards, because the window shows the short variant beside the entry before anything is written.
pub fn rename(volume: &Volume, name: &str) -> Result<Named, String> {
    if let Some(why) = refusal(volume) { return Err(format!("This volume cannot be renamed: {why}.")); }
    let fs = FileSystem::parse(&volume.fs_type).ok_or_else(|| format!("failBrauwser names no volume label for the file system {} of “{}”.", volume.fs_type, volume.title()))?;
    let plan = name_plan(fs, name)?;
    // The label first, then the pair that carries the name the label is too short for. Where the rights are
    // what is missing, the one privileged run is handed both halves, so naming a volume answers one consent
    // dialog and not two (docs\TOOL-STARTS.md).
    #[cfg(windows)]
    let words = match crate::win::storage::set_label(&volume.device, &plan.label) {
        Ok(()) => named_words(&plan.label, &name_pins_at(volume, &plan)),
        Err(5) => elevated(RENAME, &admin_rename_arguments(volume, fs, name, &plan.label))?,
        Err(code) => return Err(format!("Windows refused the new name (win32 {code}).")),
    };
    #[cfg(not(windows))]
    let words = elevated(RENAME, &admin_rename_arguments(volume, fs, name, &plan.label))?;
    Ok(Named { label: plan.label, words })
}

/// The words one naming act answers with: the name the volume carries, and what became of the pair that
/// carries the longer one. Nothing is appended where there was nothing to say.
fn named_words(label: &str, pin: &str) -> String {
    let head = if label.trim().is_empty() { "The volume carries no name of its own".to_string() } else { format!("The volume's name is now {label}") };
    if pin.is_empty() { format!("{head}.") } else { format!("{head}. {pin}.") }
}

/// The pair of text files a name needs, at the folder this volume names: written, refreshed, taken back, or
/// named as impossible where the volume names no folder at all (README: *Renaming a volume*).
fn name_pins_at(volume: &Volume, plan: &NamePlan) -> String {
    pin_sentence(root_of(volume).map(|root| root.display().to_string()).unwrap_or_default().as_str(), &volume.drive_name, plan)
}

/// The same sentence from the words a privileged run was handed, so both halves of one rename tell it the
/// same way: the folder it may write into (empty where the volume names none), the model the device
/// description is drawn from, and the plan the name settled to.
fn pin_sentence(folder: &str, model: &str, plan: &NamePlan) -> String {
    if folder.trim().is_empty() {
        return if plan.pinned { "The volume names no folder to write the pin into, so nothing carries the longer name".into() } else { String::new() };
    }
    let both = format!("{} and {}", icon::AUTORUN, icon::DESKTOP);
    match icon::name_pins(Path::new(folder), model, plan) {
        Ok(icon::Pins::Stood { .. }) if plan.pinned => format!("The longer name it is called stands pinned in {both}"),
        Ok(icon::Pins::Stood { .. }) => format!("{both} were refreshed to the name the volume carries"),
        Ok(icon::Pins::Taken { .. }) => format!("{both} of ours were taken back, since nothing of ours pins a longer name any more"),
        Ok(icon::Pins::Nothing) => String::new(),
        Err(why) => format!("The name it is called could not be pinned: {why}"),
    }
}

/// What the privileged rename is handed: the target the label is written at, the label the plan settled on,
/// the file system's word, the folder the pin goes into (empty where the volume names none), the name as it
/// was typed, and the model the device description names. One run, both halves.
fn admin_rename_arguments(volume: &Volume, fs: FileSystem, typed: &str, label: &str) -> Vec<String> {
    vec![volume.device.clone(), label.into(), fs.word().into(), root_of(volume).map(|root| root.display().to_string()).unwrap_or_default(), typed.into(), volume.drive_name.clone()]
}

/// The format of one volume: the gate first, then the closed command, then the privileged run of this
/// program's own file. The words that come back are the format tool's own, so a refusal is the machine's
/// and never an invented one.
pub fn format(volume: &Volume, fs: FileSystem, chosen: &[(FormatOption, Value)]) -> Result<String, String> {
    if let Some(why) = refusal(volume) { return Err(format!("This volume cannot be formatted: {why}.")); }
    let image = std::env::current_exe().map_err(|e| format!("failBrauwser cannot name its own file: {e}"))?;
    if holds_our_own_image(volume, &image) { return Err("This volume holds the file failBrauwser runs from, so formatting it here would end this program mid-format.".into()); }
    let named = tool_file_systems().iter().map(|known| known.word().to_string()).collect::<Vec<_>>();
    if !accepted(volume, &named).contains(&fs) { return Err(format!("{} is not offered for “{}”.", fs.word(), volume.title())); }
    #[cfg(windows)]
    let arguments = {
        let Some(target) = Target::of(volume) else { return Err("This volume has no drive letter to format.".into()) };
        carry_name(format_argv(target, fs, chosen)?, fs, chosen)?
    };
    #[cfg(not(windows))]
    let arguments = {
        if chosen.iter().any(|(option, _)| !matches!(option, FormatOption::VolumeName | FormatOption::DeviceIcon)) {
            // The switches of the option table are this system's format tool's own words. What the tools
            // over there take for each file system has never been read here (docs\TOOL-STARTS.md), so an
            // option that would need one is refused rather than quietly dropped.
            return Err("the options of the format table belong to this system's format tool; only the file system itself is carried over there".into());
        }
        carry_name(linux_format_arguments(volume, fs)?, fs, chosen)?
    };
    let answer = elevated(FORMAT, &arguments)?;
    Ok(format_words(volume, fs.word(), chosen, &answer))
}

/// The words one format answers: the format tool's own, joined with those of the device-icon act when the
/// page had its box ticked. That box is a second act on the volume that now holds the new file system, so it
/// runs after the format worked, against the name typed on the page and the file system just written — the
/// record of the volume still names what it was before. A failure of the icon is told beside a format that did
/// work instead of hiding it. An unticked box leaves the tool's words at the head of the answer and writes the
/// pair of text files that carries a name the label the tool wrote is too short for — pinning is the naming
/// act's work, not the icon's (README: *Formatting a volume*, *Renaming a volume*).
fn format_words(volume: &Volume, fs: &str, chosen: &[(FormatOption, Value)], tool: &str) -> String {
    let typed = chosen.iter().find(|(option, _)| *option == FormatOption::VolumeName).map(|(_, value)| match value { Value::Text(name) => name.as_str(), _ => "" }).unwrap_or("");
    let Some(shape) = chosen.iter().find(|(option, _)| *option == FormatOption::DeviceIcon).and_then(|(_, value)| match value { Value::Text(asset) => Shape::parse(asset), _ => None }) else {
        let pin = name_pins_at(volume, &name_words(fs, typed));
        return if pin.is_empty() { tool.into() } else { format!("{tool}\n{pin}") };
    };
    match create_icon(volume, fs, shape, typed) {
        Ok(words) => format!("{tool}\n{words}"),
        Err(why) => format!("{tool}\nThe volume was formatted; its device icon was not written: {why}"),
    }
}

/// The command of a format with the one name its tool's own switch cannot carry. The switches of the table
/// stay as they were built, and a name the tool would refuse is handed over as [`NAME`] instead, so the tool
/// is never started with an argument it rejects and the same privileged run writes that name afterwards.
fn carry_name(arguments: Vec<String>, fs: FileSystem, chosen: &[(FormatOption, Value)]) -> Result<Vec<String>, String> {
    let Some(label) = carried_name(fs, chosen)? else { return Ok(arguments) };
    let mut carried = arguments;
    carried.extend([NAME.into(), label]);
    Ok(carried)
}

/// The command the Linux half runs: the `mkfs.<type>` program of the chosen file system and the volume it
/// writes. Never run on this machine, and recorded as such.
pub fn linux_format_arguments(volume: &Volume, fs: FileSystem) -> Result<Vec<String>, String> {
    let program = mkfs_program(fs).ok_or_else(|| format!("no mkfs program is named for {} over there", fs.word()))?;
    if volume.device.is_empty() { return Err("this volume names no device to format".into()); }
    Ok(vec![program.into(), volume.device.clone()])
}

/// The program that writes a new file system of this kind there. Never run here, so this is the tools'
/// documented naming rather than a measured answer; a distribution without one gets its own refusal.
pub fn mkfs_program(fs: FileSystem) -> Option<&'static str> {
    match fs {
        FileSystem::Fat => Some("mkfs.fat"),
        FileSystem::Fat32 => Some("mkfs.vfat"),
        FileSystem::ExFat => Some("mkfs.exfat"),
        FileSystem::Ntfs => Some("mkfs.ntfs"),
        FileSystem::Udf => Some("mkfs.udf"),
        FileSystem::ReFs => None,
    }
}

/// The program that writes a volume's name there, and where the name goes among its arguments. The tools'
/// documented shape, never run here; a file system with no named tool is refused rather than guessed at.
pub fn label_arguments(fs: FileSystem, device: &str, label: &str) -> Option<Vec<String>> {
    match fs {
        FileSystem::Ntfs => Some(vec!["ntfslabel".into(), device.into(), label.into()]),
        FileSystem::ExFat => Some(vec!["exfatlabel".into(), device.into(), label.into()]),
        FileSystem::Fat | FileSystem::Fat32 => Some(vec!["mlabel".into(), format!("-i{device}"), format!("::{label}")]),
        FileSystem::Udf | FileSystem::ReFs => None,
    }
}

/// The words that stand when this volume gets no device icon, or nothing where it could. The verbs write
/// files and nothing else, so what is refused is a volume that refuses writes and a volume naming no folder
/// to write into (README: *Device icons*).
pub fn icon_refusal(volume: &Volume) -> Option<String> {
    if volume.read_only { return Some("This volume gets no device icon: the volume is read-only.".into()); }
    if root_of(volume).is_none() { return Some(NO_FOLDER.to_string()); }
    None
}

/// The words naming a volume with nowhere to write.
const NO_FOLDER: &str = "This volume gets no device icon: it names no folder to write the four files into.";

/// The four files of one device icon on a volume (README: *Device icons*): the shape the window named, the
/// model and size the machine reported, and the name the volume is called. The two text files pin that name
/// only where the file system holds something shorter, which is the same rule the rename follows. Where the
/// rights are what is missing, the same write is asked of this program's own file run as the administrator,
/// and a prompt that is closed without agreeing leaves the volume as it was. `fs` names the file system the
/// name is settled against: the record's own word for the two verbs, and the one just written for a format,
/// whose record still says what the volume was before it.
pub fn create_icon(volume: &Volume, fs: &str, shape: Shape, name: &str) -> Result<String, String> {
    if let Some(why) = icon_refusal(volume) { return Err(why); }
    let Some(root) = root_of(volume) else { return Err(NO_FOLDER.into()) };
    match icon::create(&root, shape, &volume.drive_name, device_size(volume), &name_words(fs, name)) {
        Ok(written) => Ok(creation_words(&written, &root)),
        Err(why) if why.denied => elevated(ICON, &icon_arguments(volume, fs, shape, name)),
        Err(why) => Err(why.text),
    }
}

/// Taking a device icon away from a volume: the two text files of ours and the image files they name, and
/// nothing else. An `autorun.inf` or `desktop.ini` carrying no marker of ours keeps every byte, and the
/// answer names what was kept (README: *Device icons*).
pub fn remove_icon(volume: &Volume) -> Result<String, String> {
    if let Some(why) = icon_refusal(volume) { return Err(why); }
    let Some(root) = root_of(volume) else { return Err(NO_FOLDER.into()) };
    match icon::remove(&root) {
        Ok(gone) => Ok(removal_words(&gone, &root)),
        Err(why) if why.denied => elevated(UNICON, &unicon_arguments(volume)),
        Err(why) => Err(why.text),
    }
}

/// What the privileged create is handed: the folder, the shape by the name its picture carries, the model and
/// the size the words are drawn from, the name to pin, and the file system that bounds that name. Every word
/// the child needs to draw the same picture is here, so it asks no window anything.
pub fn icon_arguments(volume: &Volume, fs: &str, shape: Shape, name: &str) -> Vec<String> {
    vec![root_of(volume).map(|root| root.display().to_string()).unwrap_or_default(), shape.asset().to_string(), volume.drive_name.clone(), device_size(volume).to_string(), name.to_string(), fs.to_string()]
}

/// …and what the privileged removal is handed: the folder whose two text files are read.
pub fn unicon_arguments(volume: &Volume) -> Vec<String> {
    vec![root_of(volume).map(|root| root.display().to_string()).unwrap_or_default()]
}

/// The folder the four files go into: where the volume is mounted, or on this machine the root of the letter
/// it is named by — which is what a volume is right after it was formatted, before its record is read again.
fn root_of(volume: &Volume) -> Option<PathBuf> {
    if let Some(mount) = volume.mount_point() { return Some(mount.clone()); }
    #[cfg(windows)]
    if let Some(target) = Target::of(volume) { return Some(PathBuf::from(format!("{}:\\", target.letter.to_uppercase()))); }
    None
}

/// The size the picture prints: the device's own, which is what the packaging states, and the volume's where
/// the machine names no device of its own.
fn device_size(volume: &Volume) -> u64 {
    if volume.drive_size > 0 { volume.drive_size } else { volume.size }
}

/// The name the two text files carry, settled by the same rule the rename follows. Where the file system is
/// one this program names no limit for, the name is carried as it stands, because nothing shortens it; where
/// the name is one no file system would hold, nothing is pinned rather than a name that would be lost.
fn name_words(fs: &str, name: &str) -> NamePlan {
    match FileSystem::parse(fs).and_then(|fs| name_plan(fs, name).ok()) {
        Some(plan) => plan,
        None => NamePlan { label: String::new(), called: String::new(), pinned: false },
    }
}

/// The words a finished create answers with: the folder and the files that stand there.
fn creation_words(written: &icon::Written, root: &Path) -> String {
    format!("The device icon stands on {}: {}.", root.display(), written.files.join(", "))
}

/// The words a finished removal answers with, naming what was kept and why.
fn removal_words(gone: &icon::Removed, root: &Path) -> String {
    let taken = if gone.deleted.is_empty() { "Nothing".into() } else { gone.deleted.join(", ") };
    let kept = if gone.kept.is_empty() { "nothing of ours was there".into() } else { format!("{} kept, because nothing of ours marks them", gone.kept.join(", ")) };
    format!("{taken} taken away from {}; {kept}.", root.display())
}

/// The four files as the administrator: the folder, the shape by its asset name, the model, the size, the
/// name and the file system's word. The name is settled here by the same rule as in the window, so the child
/// writes the files the window showed.
fn run_icon(arguments: &[String]) -> Result<String, String> {
    let (Some(root), Some(asset), Some(model), Some(bytes), Some(name), Some(fs)) = (arguments.first(), arguments.get(1), arguments.get(2), arguments.get(3), arguments.get(4), arguments.get(5)) else {
        return Err("A folder, a shape, a model, a size, a name and a file system are needed to write a device icon.".into());
    };
    let Some(shape) = Shape::parse(asset) else { return Err(format!("{asset:?} names no device shape.")) };
    let Ok(bytes) = bytes.parse::<u64>() else { return Err(format!("{bytes:?} is no size in bytes.")) };
    icon::create(Path::new(root), shape, model, bytes, &name_words(fs, name)).map(|written| creation_words(&written, Path::new(root))).map_err(|why| why.text)
}

/// The removal as the administrator, reading the same marker and deleting by nothing else.
fn run_unicon(arguments: &[String]) -> Result<String, String> {
    let Some(root) = arguments.first() else { return Err("The folder of a device icon is needed.".into()) };
    icon::remove(Path::new(root)).map(|gone| removal_words(&gone, Path::new(root))).map_err(|why| why.text)
}

/// The file one act of one instance answers into: named for the act and for this process, so two windows
/// cannot answer into each other's file.
pub fn outcome_path(act: &str) -> PathBuf {
    std::env::temp_dir().join(format!("failbrauwser-{act}-{}.out", std::process::id()))
}

/// The answer of a privileged run: whether the act worked, and the words that go with it.
pub fn outcome_text(ok: bool, text: &str) -> String {
    format!("{}\n{}", if ok { "ok" } else { "no" }, text)
}

/// The same answer read back: the first line is the verdict, everything after it the words.
pub fn read_outcome(text: &str) -> (bool, String) {
    let (head, rest) = text.split_once('\n').unwrap_or((text, ""));
    (head.trim() == "ok", rest.trim_matches('\r').to_string())
}

/// The arguments the privileged run is started with: the act first, then the file its answer goes into,
/// then what the act is to do.
pub fn elevated_arguments(act: &str, outcome: &Path, arguments: &[String]) -> Vec<String> {
    let mut out = vec![act.into(), OUTCOME.into(), outcome.display().to_string()];
    out.extend(arguments.iter().cloned());
    out
}

/// Where an argument list the privileged run was started with splits into the answer file and the act.
/// Started without the flag, the act still runs — it simply has nowhere to write, and the window says so
/// rather than guessing at an answer.
pub fn split_arguments(args: &[String]) -> (Option<String>, Vec<String>) {
    match args.iter().position(|arg| arg == OUTCOME) {
        Some(at) if at + 1 < args.len() => (args.get(at + 1).cloned(), args.iter().enumerate().filter(|(n, _)| *n != at && *n != at + 1).map(|(_, arg)| arg.clone()).collect()),
        _ => (None, args.to_vec()),
    }
}

/// Whether this program's own file was started to do one of the acts: the start routes exactly these flags to
/// [`execute`] before a window, a setting or a reading client exists, and a flag answered there but missed at
/// the start opens the program on screen instead of doing the act — which leaves the waiting window reading
/// back an answer file that never gets written.
pub fn is_act(argument: Option<&str>) -> bool {
    matches!(argument, Some(RENAME) | Some(FORMAT) | Some(ICON) | Some(UNICON))
}

/// The privileged half, run by this program's own file as the administrator: the act is done here and its
/// answer written to the file `--outcome` named. `main` routes every flag of [`is_act`] here before any window
/// is built, so this run holds no display, no settings and no reading clients.
pub fn execute(args: &[String]) -> i32 {
    let (outcome, rest) = split_arguments(args);
    let answer = match rest.first().map(String::as_str) {
        Some(RENAME) => write_label_as_admin(&rest[1..]),
        Some(FORMAT) => run_format(&rest[1..]),
        Some(ICON) => run_icon(&rest[1..]),
        Some(UNICON) => run_unicon(&rest[1..]),
        other => Err(format!("Nothing to answer for {:?}.", other.unwrap_or_default())),
    };
    let (ok, text) = match answer {
        Ok(text) => (true, text),
        Err(text) => (false, text),
    };
    if let Some(path) = outcome.filter(|path| !path.is_empty()) {
        // The file is the whole answer: a refusal is written into it as well, so that the window never has
        // to tell a refused act apart from a run that never started.
        let _ = std::fs::write(&path, outcome_text(ok, &text));
    }
    if ok { 0 } else { 1 }
}

/// The name as the administrator: the target the label is written at, the label, the file system's word, the
/// folder the pin of a long name goes into (empty where the volume names none), the name as it was typed, and
/// the model the device description is drawn from. The label and the pin are one run, so a naming act never
/// asks for consent twice (README: *Renaming a volume*).
fn write_label_as_admin(arguments: &[String]) -> Result<String, String> {
    let (Some(target), Some(label)) = (arguments.first(), arguments.get(1)) else { return Err("A volume and its new name are needed.".into()) };
    let (folder, typed, model) = (arguments.get(3).map(String::as_str).unwrap_or_default(), arguments.get(4).map(String::as_str).unwrap_or_default(), arguments.get(5).map(String::as_str).unwrap_or_default());
    let pin = |fs: FileSystem| pin_sentence(folder, model, &name_words(fs.word(), typed));
    #[cfg(windows)]
    {
        let fs = FileSystem::parse(arguments.get(2).map(String::as_str).unwrap_or_default()).ok_or_else(|| "The file system whose label limit bounds the name was not named.".to_string())?;
        crate::win::storage::set_label(target, label).map(|()| named_words(label, &pin(fs))).map_err(|code| format!("Windows refused the new name (win32 {code})."))
    }
    #[cfg(not(windows))]
    {
        let fs = FileSystem::parse(arguments.get(2).map(String::as_str).unwrap_or_default()).ok_or_else(|| "The file system whose name tool to use was not named.".to_string())?;
        let tool = label_arguments(fs, target, label).ok_or_else(|| format!("no program is named that writes the name of a {} volume there.", fs.word()))?;
        run_tool_as_read(&tool).map(|words| format!("{words}\n{}", named_words(label, &pin(fs))))
    }
}

/// The format itself, run by the privileged instance: the volume by its letter and then only switches of
/// the table. The list is checked here again, because the privileged run is a separate start and its
/// arguments travelled through the machine's command line: a list naming no drive letter, or carrying
/// anything that is no switch, never reaches the tool.
fn run_format(arguments: &[String]) -> Result<String, String> {
    let (command, named) = take_carried_name(arguments)?;
    #[cfg(windows)]
    {
        if command.first().map(String::as_str).filter(|target| is_drive_letter(target)).is_none() {
            return Err(format!("The format command must name the volume by its drive letter, and not {:?}.", command.first().map(String::as_str).unwrap_or_default()));
        }
        for switch in command.iter().skip(1) {
            if !switch.starts_with('/') || switch.contains('"') || switch.chars().any(char::is_control) { return Err(format!("{switch:?} is not a switch of the format table.")); }
        }
        // Asked by its own file name: a start without an extension gets `.exe` appended, and the tool on
        // this build is `format.com` (measured; docs\TOOL-STARTS.md).
        let words = format_tool("format.com", &command)?;
        let root = format!("{}:\\", command[0][..1].to_uppercase());
        name_written_after(words, named.as_deref(), |label| crate::win::storage::set_label(&root, label).map(|()| format!("The volume's name is now {label}.")).map_err(|code| format!("Windows refused the new name (win32 {code}).")))
    }
    #[cfg(not(windows))]
    {
        let Some(program) = command.first() else { return Err("No format command was given.".into()) };
        if !program.starts_with("mkfs.") || command.iter().skip(1).any(|argument| argument.starts_with('-')) { return Err(format!("{program:?} is not a format program this feature names.")); }
        let words = run_tool_as_read(&command)?;
        let device = command.get(1).map(String::as_str).unwrap_or_default();
        name_written_after(words, named.as_deref(), |label| {
            let fs = fs_of_mkfs(program).ok_or_else(|| format!("{program:?} names no file system whose name tool this feature knows."))?;
            label_arguments(fs, device, label).ok_or_else(|| format!("no program is named that writes the name of a {} volume there.", fs.word())).and_then(|tool| run_tool_as_read(&tool))
        })
    }
}

/// The list the privileged run was handed, split into the command the tool is started with and the name it
/// writes itself. `--volume-name` is the one argument of a format that names no switch, and it is checked
/// here again because the arguments travelled through the machine's command line: no name after the flag, a
/// name that reads as a switch, one holding a control character, or the flag more than once, and the whole
/// list is refused before the tool exists.
fn take_carried_name(arguments: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let Some(at) = arguments.iter().position(|argument| argument == NAME) else { return Ok((arguments.to_vec(), None)) };
    let label = arguments.get(at + 1).ok_or_else(|| format!("{NAME} was named without a name after it."))?;
    if label.is_empty() { return Err(format!("{NAME} was named without a name after it.")); }
    if label.starts_with(['-', '/']) { return Err(format!("what {NAME} names reads as a switch: {label:?}.")); }
    if label.chars().any(char::is_control) { return Err(format!("the name {NAME} carries holds a control character: {label:?}.")); }
    if arguments.iter().skip(at + 1).any(|argument| argument == NAME) { return Err(format!("{NAME} was named more than once.")); }
    Ok((arguments.iter().enumerate().filter(|(n, _)| *n != at && *n != at + 1).map(|(_, argument)| argument.clone()).collect(), Some(label.clone())))
}

/// The name the run writes for itself, once the format answered success: this machine's own label call here,
/// the file system's own label program over there. A volume that was formatted and then refused its name is
/// told as the half-act it is, since the format itself did happen and the name is the smaller of the two.
fn name_written_after(words: String, named: Option<&str>, write: impl FnOnce(&str) -> Result<String, String>) -> Result<String, String> {
    let Some(label) = named else { return Ok(words) };
    match write(label) {
        Ok(answer) => Ok(format!("{words}\n{answer}")),
        Err(why) => Ok(format!("{words}\nThe volume was formatted; its name was not written: {why}")),
    }
}

/// The file system whose `mkfs` program this is, which is the only trace of it in the command over there.
/// Never read on this machine.
#[cfg_attr(windows, allow(dead_code))]
fn fs_of_mkfs(program: &str) -> Option<FileSystem> {
    FileSystem::all().iter().copied().find(|fs| mkfs_program(*fs) == Some(program))
}

/// Whether the text is a bare drive letter with its colon and nothing else, as the format command names
/// its volume.
#[cfg(any(windows, test))]
fn is_drive_letter(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

/// The format tool, started and waited for here. Not a reading client on purpose: the client stays alive
/// when this instance is killed, so a format is never cut in half by a window closing (docs\TOOL-STARTS.md).
#[cfg(windows)]
fn format_tool(program: &str, arguments: &[String]) -> Result<String, String> {
    let mut command = std::process::Command::new(program);
    command.args(arguments);
    // The rule every start of another program answers to: no console window of its own.
    crate::win::console::no_window(&mut command);
    match command.output() {
        Ok(out) => {
            let text = joined(&out.stdout, &out.stderr);
            if out.status.success() { return Ok(text); }
            Err(text)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!("{program} could not be started: it is not installed, or not on the path.")),
        Err(e) => Err(format!("{program} could not be started: {e}")),
    }
}

/// A tool whose whole answer is wanted, for the half over there: its answer is a read of words rather than
/// a write of a volume, so it goes through the funnel every reading client goes through.
#[cfg_attr(windows, allow(dead_code))]
fn run_tool_as_read(arguments: &[String]) -> Result<String, String> {
    let answer = crate::vcs::run::ask(&arguments[0], Path::new("."), &arguments[1..]);
    if answer.ok { return Ok(answer.text); }
    Err(answer.text)
}

/// The two streams of a client, in the order the windows show them.
#[cfg(windows)]
fn joined(out: &[u8], err: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(out).to_string();
    if !text.is_empty() && !text.ends_with('\n') && !err.is_empty() { text.push('\n'); }
    text + &String::from_utf8_lossy(err)
}

/// The act as the administrator: this program's own file started again, its answer read back from the
/// file it was told to write, and that file left nowhere behind.
fn elevated(act: &str, arguments: &[String]) -> Result<String, String> {
    let outcome = outcome_path(act);
    let _ = std::fs::remove_file(&outcome);
    let started = start_elevated(&elevated_arguments(act, &outcome, arguments));
    let answer = std::fs::read_to_string(&outcome);
    let _ = std::fs::remove_file(&outcome);
    started?;
    let text = answer.map_err(|_| format!("The privileged run of {act} wrote no answer."))?;
    let (ok, text) = read_outcome(&text);
    if ok { return Ok(text); }
    Err(text)
}

/// Starting the privileged run: the shell's `runas` verb on this program's own file, or the program
/// `FAILBRAUWSER_ELEVATE` names when a scenario answers for it.
#[cfg(windows)]
fn start_elevated(args: &[String]) -> Result<(), String> {
    if let Some(program) = seam(std::env::var_os(SEAM)) {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut command = std::process::Command::new(&program);
        command.arg(&exe).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        // The rule every start of another program answers to: no console window of its own.
        crate::win::console::no_window(&mut command);
        let status = command.status().map_err(|e| format!("Cannot start {program}: {e}"))?;
        if !status.success() { return Err(format!("The stand-in run answered with code {}.", status.code().unwrap_or_default())); }
        return Ok(());
    }
    crate::win::elevate::run_self_with(args)
}

/// Starting the privileged run there: this program's own file again through `pkexec`, which is how Backup
/// Mode and the LVM volumes reach the administrator (`src\links\helper.rs`); recorded as never run here.
#[cfg(not(windows))]
fn start_elevated(args: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let program = seam(std::env::var_os(SEAM)).unwrap_or_else(|| "pkexec".into());
    let mut argv: Vec<String> = vec![exe.to_string_lossy().to_string()];
    argv.extend(args.iter().cloned());
    let answer = crate::vcs::run::ask(&program, Path::new("."), &argv);
    if answer.ok { return Ok(()); }
    Err(answer.text)
}

/// The stand-in that answers for the privileged run, when the environment names one: it is started with
/// this program's own file name as its first argument, so it can hand the rest of the command straight
/// on. An empty answer names nothing, which is what leaving the variable unset means.
fn seam(value: Option<std::ffi::OsString>) -> Option<String> {
    value.filter(|value| !value.is_empty()).map(|value| value.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use FormatOption::*;
    use Value::*;

    #[test]
    fn an_answer_round_trips_both_verdicts_and_their_words() {
        let written = outcome_text(true, "The volume was formatted.\nEverything on it is gone.");
        let (ok, text) = read_outcome(&written);
        assert!(ok);
        assert_eq!(text, "The volume was formatted.\nEverything on it is gone.");
        let (ok, text) = read_outcome(&outcome_text(false, "Access is denied."));
        assert!(!ok);
        assert_eq!(text, "Access is denied.");
        // A file holding nothing but a verdict answers with no words, and a file that is no answer at all
        // is a refusal rather than a silent success.
        assert_eq!(read_outcome("ok"), (true, String::new()));
        assert_eq!(read_outcome(""), (false, String::new()));
    }

    #[test]
    fn the_answer_file_names_the_act_and_this_instance() {
        let rename = outcome_path(RENAME);
        let format = outcome_path(FORMAT);
        assert_ne!(rename, format, "two acts of one instance must not answer into one file");
        assert!(rename.to_string_lossy().contains(&std::process::id().to_string()), "the file names this instance: {}", rename.display());
        assert_eq!(rename.extension().map(|e| e.to_string_lossy().to_string()), Some("out".into()));
    }

    #[test]
    fn the_arguments_name_the_act_then_the_answer_file_then_the_act_s_own() {
        let args = elevated_arguments(FORMAT, Path::new("/tmp/fb.out"), &["E:".into(), "/FS:NTFS".into(), "/Y".into()]);
        assert_eq!(args, [FORMAT, OUTCOME, "/tmp/fb.out", "E:", "/FS:NTFS", "/Y"]);
        let (outcome, rest) = split_arguments(&args);
        assert_eq!(outcome.as_deref(), Some("/tmp/fb.out"));
        assert_eq!(rest, [FORMAT, "E:", "/FS:NTFS", "/Y"]);
        // An act started without the flag is still the act; the absence is not a crash.
        let (outcome, rest) = split_arguments(&[RENAME.into(), "E:".into()]);
        assert_eq!(outcome, None);
        assert_eq!(rest, [RENAME, "E:"]);
    }

    /// The privileged half answers every act it can be started with, and answers a refusal the same way:
    /// a file that says no, so the window never has to guess what a missing answer meant. No act here
    /// reaches a tool or a volume: each one is refused by the guard that runs before either exists.
    #[test]
    fn the_privileged_half_answers_for_the_act_it_was_started_with() {
        let outcome = outcome_path("test");
        let _ = std::fs::remove_file(&outcome);
        assert_eq!(execute(&[RENAME.into(), OUTCOME.into(), outcome.display().to_string()]), 1, "a name without a volume is refused with a reason");
        assert_eq!(read_outcome(&std::fs::read_to_string(&outcome).expect("the answer file is written even for a refusal")), (false, "A volume and its new name are needed.".to_string()));
        assert_eq!(execute(&[FORMAT.into(), OUTCOME.into(), outcome.display().to_string(), "/FS:NTFS".into(), "/Y".into()]), 1, "a command that names no volume is refused before the tool exists");
        assert!(!read_outcome(&std::fs::read_to_string(&outcome).expect("the answer file is written even for a refusal")).0);
        assert_eq!(execute(&["--elsewhere".into()]), 1, "an argument that names no act is answered, not ignored");
        let _ = std::fs::remove_file(&outcome);
    }

    /// The command that reaches the format tool is the closed table again: no volume letter, or an
    /// argument that is no switch, and the tool is never started at all.
    #[test]
    fn only_a_command_of_the_table_reaches_the_format_tool() {
        for not in ["", "F:\\", "FF:", "1:", "/F:", "F: extra"] {
            let why = run_format(&[not.into(), "/FS:NTFS".into(), "/Y".into()]).unwrap_err();
            assert!(why.contains("drive letter") || why.contains("is not a format program"), "{not:?}: {why}");
        }
        #[cfg(windows)]
        assert!(run_format(&["F:".into(), "/V:a \"b".into()]).unwrap_err().contains("not a switch of the format table"), "a quote cannot travel into the parameter string");
        // What does reach the tool is never run here: the authorized verification step of docs\WINDOWS.md
        // is the only place a format of this feature is measured, on the stick it names.
        #[cfg(not(windows))]
        assert!(run_format(&["mkfs.exfat".into(), "-f".into()]).unwrap_err().contains("is not a format program"), "an option over there is refused, not run");
    }

    /// A name the tool's own switch refuses (measured: any name holding a space) travels to the privileged
    /// run as the act's own argument, so the command the tool is started with keeps to switches.
    #[test]
    fn a_name_the_tool_refuses_travels_as_the_runs_own_argument() {
        let target = Target { letter: 'f', size: 64_160_400_896 };
        let spaced = [(VolumeName, Text("Samsung 64 GB".into())), (AllocationUnit, Bytes(2048)), (Quick, On)];
        assert_eq!(carry_name(format_argv(target, FileSystem::Ntfs, &spaced).unwrap(), FileSystem::Ntfs, &spaced).unwrap(), vec!["F:", "/FS:NTFS", "/A:2048", "/Q", "/Y", NAME, "Samsung 64 GB"]);
        let plain = [(VolumeName, Text("Samsung-64-GB".into()))];
        assert_eq!(carry_name(format_argv(target, FileSystem::Ntfs, &plain).unwrap(), FileSystem::Ntfs, &plain).unwrap(), vec!["F:", "/FS:NTFS", "/V:Samsung-64-GB", "/Y"], "a name the switch takes is never carried as well");
    }

    /// The privileged run takes that name out of the list before the tool is started and hands it back by
    /// itself: the list it runs is only ever the table's switches, and a name that cannot travel as an
    /// argument — nothing there, a switch, another flag, a control character, the flag twice — is refused.
    #[test]
    fn the_privileged_run_splits_the_carried_name_from_the_command() {
        let (command, named) = take_carried_name(&["F:".into(), "/FS:NTFS".into(), "/Q".into(), NAME.into(), "Samsung 64 GB".into()]).unwrap();
        assert_eq!(command, vec!["F:", "/FS:NTFS", "/Q"], "the list the tool is started with holds switches only");
        assert_eq!(named.as_deref(), Some("Samsung 64 GB"));
        let (alone, none) = take_carried_name(&["F:".into(), "/Y".into()]).unwrap();
        assert_eq!(alone, vec!["F:", "/Y"]);
        assert_eq!(none.as_deref(), None, "a command of switches alone carries nothing");
        for bad in [vec!["F:".into(), NAME.into()], vec!["F:".into(), NAME.into(), "/FS:NTFS".into()], vec!["F:".into(), NAME.into(), "-x".into()], vec!["F:".into(), NAME.into(), "a\u{7}b".into()], vec!["F:".into(), NAME.into(), "one".into(), NAME.into(), "two".into()]] {
            assert!(take_carried_name(&bad).is_err(), "{bad:?} is no name to carry");
        }
    }

    #[test]
    fn the_command_shape_the_table_builds_passes_the_privileged_guard() {
        let argv = format_argv(Target { letter: 'f', size: 64_160_400_896 }, FileSystem::Ntfs, &[(VolumeName, Text("Samsung 64 GB".into())), (AllocationUnit, Bytes(8192)), (Quick, On)]).unwrap();
        assert!(is_drive_letter(&argv[0]) && argv[1..].iter().all(|switch| switch.starts_with('/') && !switch.contains('"') && !switch.chars().any(char::is_control)), "{argv:?}");
    }

    #[test]
    fn the_tools_over_there_are_named_for_the_file_systems_that_have_them() {
        assert_eq!(label_arguments(FileSystem::Ntfs, "/dev/sda1", "Data").unwrap(), ["ntfslabel", "/dev/sda1", "Data"]);
        assert_eq!(label_arguments(FileSystem::ExFat, "/dev/sdb1", "Stick").unwrap(), ["exfatlabel", "/dev/sdb1", "Stick"]);
        assert_eq!(label_arguments(FileSystem::Fat32, "/dev/sdc1", "CARD").unwrap(), ["mlabel", "-i/dev/sdc1", "::CARD"]);
        assert_eq!(label_arguments(FileSystem::ReFs, "/dev/sdd1", "X"), None, "no tool is named there for a file system that has none");
        assert_eq!(mkfs_program(FileSystem::Fat32), Some("mkfs.vfat"));
        assert_eq!(mkfs_program(FileSystem::ReFs), None);
        assert_eq!(linux_format_arguments(&Volume { device: "/dev/sdb1".into(), fs_type: "exFAT".into(), ..Default::default() }, FileSystem::ExFat).unwrap(), ["mkfs.exfat", "/dev/sdb1"]);
        assert!(linux_format_arguments(&Volume { fs_type: "exFAT".into(), ..Default::default() }, FileSystem::ExFat).is_err(), "a volume with no device name has nothing to format");
    }

    /// The act is refused before a command exists wherever the gate refuses the volume, and a name the
    /// file system cannot hold is refused without anything being written.
    #[test]
    fn no_act_exists_for_a_volume_the_gate_refuses() {
        let optical = Volume { device: "q:".into(), fs_type: "exFAT".into(), optical: true, drive_size: 64_160_400_896, size: 64_160_400_896, ..Default::default() };
        assert!(rename(&optical, "Name").unwrap_err().contains("optical"));
        assert!(format(&optical, FileSystem::ExFat, &[]).unwrap_err().contains("optical"));
        let tiny = Volume { device: "q:".into(), fs_type: "exFAT".into(), drive_size: 1_000, ..Default::default() };
        assert!(rename(&tiny, "Name").unwrap_err().contains("size"));
        let unnamed = Volume { device: "q:".into(), fs_type: "ext4".into(), drive_size: 64_160_400_896, size: 64_160_400_896, ..Default::default() };
        assert!(rename(&unnamed, "Name").unwrap_err().contains("ext4"), "the machine answers a file system no label rule of ours names");
        // A name the file system cannot carry at all is refused here, so the write is never attempted and
        // a bad name cannot reach a volume through the privileged run either.
        let bad = Volume { device: "q:".into(), fs_type: "NTFS".into(), drive_size: 64_160_400_896, size: 64_160_400_896, ..Default::default() };
        assert!(rename(&bad, "a/b").unwrap_err().contains('/'));
    }

    /// The binding of the label write, without writing a volume: a letter the machine names no drive for
    /// answers with its own refusal, which is the words the act's message is built from.
    #[test]
    #[cfg(windows)]
    fn the_label_write_carries_the_machine_s_own_refusal() {
        assert_eq!(crate::win::storage::drive_mask() & (1 << ('z' as u32 - 'a' as u32)), 0, "Z: is expected to be no drive on this machine");
        let code = crate::win::storage::set_label("Z:\\", "Nothing").unwrap_err();
        assert_ne!(code, 5, "a drive that is not there is a refusal of the name's target, not of the rights");
        assert_ne!(code, 0, "a refusal answers with the machine's code");
    }

    /// An option the Linux half carries no switch for is refused there rather than dropped.
    #[test]
    #[cfg(not(windows))]
    fn an_option_nothing_carries_over_there_is_refused_there_too() {
        let volume = Volume { device: "/dev/sdb1".into(), fs_type: "exFAT".into(), drive_size: 64_160_400_896, size: 64_160_400_896, ..Default::default() };
        let why = format(&volume, FileSystem::ExFat, &[(AllocationUnit, Bytes(4096))]).unwrap_err();
        assert!(why.contains("this system's format tool"), "{why}");
    }

    #[test]
    fn the_stand_in_that_answers_for_the_privileged_run_is_named_by_the_environment() {
        assert_eq!(seam(None), None, "no stand-in unless the environment names one");
        assert_eq!(seam(Some(std::ffi::OsString::new())), None, "an empty answer names nothing");
        assert_eq!(seam(Some(std::ffi::OsString::from("fb-elevate-shim.exe"))).as_deref(), Some("fb-elevate-shim.exe"));
    }

    #[test]
    fn the_drive_letter_of_a_volume_and_a_bare_letter_are_told_apart() {
        assert_eq!(crate::fsformat::drive_letter("F:\\"), Some('f'));
        assert!(!is_drive_letter("F:\\"), "the format command names its volume by the letter alone");
        assert!(is_drive_letter("f:"));
        assert!(!is_drive_letter("/dev/sdb1"));
    }

    /// Both icon verbs write files into a folder. A volume naming no mount point and no drive letter names
    /// nothing to write into, so the act refuses it before one byte exists (README: *Device icons*).
    #[test]
    fn a_volume_naming_no_folder_is_refused_both_icon_acts() {
        let v = Volume { device: "/dev/fake-Photos".into(), fs_type: "NTFS".into(), ..Default::default() };
        for (act, answer) in [("create", create_icon(&v, "NTFS", Shape::UsbStick, "Photos")), ("remove", remove_icon(&v))] {
            let why = answer.err().unwrap_or_else(|| panic!("the {act} icon act wrote into a folder that does not exist"));
            assert!(why.contains("no folder"), "the {act} act refused a nameless volume with other words: {why}");
        }
    }

    /// A read-only volume gets no device icon: the verbs write files, and this volume refuses writes.
    #[test]
    fn a_read_only_volume_is_refused_both_icon_acts() {
        let v = Volume { device: "/dev/fake-Photos".into(), mount_points: vec![PathBuf::from("/media/photos")], read_only: true, ..Default::default() };
        for act in ["create", "remove"] {
            let why = if act == "create" { create_icon(&v, "NTFS", Shape::SdCard, "Photos") } else { remove_icon(&v) }.err().unwrap_or_else(|| panic!("the {act} icon act wrote onto a read-only volume"));
            assert!(why.contains("read-only"), "the {act} act refused a read-only volume with other words: {why}");
        }
    }

    /// What the privileged run is handed: the folder, the shape by the name its picture carries, the words
    /// the machine reported, the size, the name to pin and the file system that bounds it — everything the
    /// child needs to draw the same picture without asking the window again.
    #[test]
    fn the_icon_arguments_name_the_folder_the_shape_and_the_words() {
        let v = Volume { device: "/dev/fake-stick".into(), mount_points: vec![PathBuf::from("/media/stick")], fs_type: "FAT32".into(), drive_name: "SanDisk Ultra".into(), drive_size: 64_160_400_896, size: 64_143_622_144, ..Default::default() };
        assert_eq!(icon_arguments(&v, "FAT32", Shape::MicroSd, "Samsung 64 GB"), ["/media/stick", "msd", "SanDisk Ultra", "64160400896", "Samsung 64 GB", "FAT32"], "the drive size is the size the picture prints, not the partition's");
        assert_eq!(unicon_arguments(&v), ["/media/stick"]);
    }

    /// A volume this machine names by letter but has not mounted — which is exactly what a volume is right
    /// after it was formatted, before its record is read again — still names the folder its four files go
    /// into: the root of that letter.
    #[test]
    #[cfg(windows)]
    fn a_volume_this_machine_names_by_letter_names_the_root_of_that_letter() {
        let v = Volume { device: "F:".into(), ..Default::default() };
        assert_eq!(root_of(&v).map(|p| p.display().to_string()), Some("F:\\".into()));
    }

    /// The privileged half writes what it was asked for: the four files stand in the folder it was given,
    /// and the outcome file says so. This is the child side of the act without any rights being asked for.
    #[test]
    fn the_privileged_icon_run_writes_the_four_files_into_the_folder_it_was_given() {
        let here = tempfile::tempdir().expect("a folder of its own");
        let (root, outcome) = (here.path().to_path_buf(), outcome_path(ICON));
        let code = execute(&[ICON.into(), OUTCOME.into(), outcome.display().to_string(), root.display().to_string(), "usbstick".into(), "SanDisk Ultra".into(), "64160400896".into(), "Samsung 64 GB".into(), "FAT32".into()]);
        assert_eq!(code, 0, "the privileged icon run refused a folder it was given");
        let (ok, text) = read_outcome(&std::fs::read_to_string(&outcome).expect("the answer of the privileged icon run"));
        assert!(ok, "the privileged icon run answered no: {text}");
        for name in [icon::DEVICE_IMAGE, icon::FOLDER_ICON, icon::AUTORUN, icon::DESKTOP] {
            assert!(root.join(name).is_file(), "{name} was not written by the privileged icon run");
        }
        let autorun = icon::decode(&std::fs::read(root.join(icon::AUTORUN)).expect("the file the run wrote"));
        assert!(icon::marked(&autorun), "the file the privileged run wrote does not carry the marker of ours");
        assert!(autorun.contains("ModelName=USB stick 64 GB"), "the run drew the wrong shape or the wrong size: {autorun}");
        assert!(autorun.contains("label=SAMSUNG 64"), "the privileged run did not settle the name by the file system's own rule: {autorun}");
        crate::osprim::set_attr_bits(&root, 0, crate::osprim::SYSTEM | crate::osprim::HIDDEN);
        let _ = std::fs::remove_file(&outcome);
    }

    /// The privileged removal reads the marker and nothing else: a text file of somebody else keeps every
    /// byte it had, and the words of the refusal name what was kept.
    #[test]
    fn the_privileged_removal_takes_only_a_file_the_marker_names() {
        let here = tempfile::tempdir().expect("a folder of its own");
        let (root, outcome) = (here.path().to_path_buf(), outcome_path(UNICON));
        std::fs::write(root.join(icon::AUTORUN), "[autorun]\r\nicon=theirs.ico,0\r\n").expect("an autorun.inf of somebody else");
        std::fs::write(root.join(icon::DESKTOP), "[.ShellClassInfo]\r\nIconResource=theirs.ico,0\r\n").expect("a desktop.ini of somebody else");
        let code = execute(&[UNICON.into(), OUTCOME.into(), outcome.display().to_string(), root.display().to_string()]);
        assert_eq!(code, 0, "the privileged removal refused a folder holding nothing of ours");
        let (ok, text) = read_outcome(&std::fs::read_to_string(&outcome).expect("the answer of the privileged removal"));
        assert!(ok, "the privileged removal answered no: {text}");
        for name in [icon::AUTORUN, icon::DESKTOP] {
            assert!(root.join(name).is_file(), "{name} of somebody else was taken away");
        }
        assert!(text.contains("kept"), "the answer does not say what was kept: {text}");
        let _ = std::fs::remove_file(&outcome);
    }

    /// Every flag `execute` answers has to be one the program's own file recognizes when it is started with
    /// it: a flag that reaches the privileged half but is named to no branch of the start opens the program's
    /// windows instead of doing the act, and the waiting window reads back an answer file that never comes.
    #[test]
    fn the_start_routes_every_act_the_privileged_half_answers() {
        for flag in [RENAME, FORMAT, ICON, UNICON] {
            assert!(is_act(Some(flag)), "{flag} is answered as an act but named to no branch of the start");
        }
        assert!(!is_act(Some("--elsewhere")), "an argument that names no act must not be taken as one");
        assert!(!is_act(None), "a start naming no argument at all is not an act");
    }

    /// The device-icon box of the format page is a second act on the volume the format tool has just written,
    /// so its words join the tool's own answer instead of replacing them: an unticked box answers with the
    /// tool's words exactly as they were, a ticked one names the icon that stands and the files that carry it,
    /// and an icon that could not be written is told beside a format that did work (README: *Device icons*).
    #[test]
    fn the_icon_of_a_format_joins_the_words_of_the_tool_it_follows() {
        let here = tempfile::tempdir().expect("a folder of its own");
        let root = here.path().to_path_buf();
        let v = Volume { device: "E:".into(), mount_points: vec![root.clone()], fs_type: "FAT32".into(), drive_name: "SanDisk Ultra".into(), drive_size: 64_160_400_896, ..Default::default() };
        let tool = "The volume was formatted.\nEverything on it is gone.";
        let named = [(VolumeName, Text("Samsung 64 GB".into()))];
        let unticked = format_words(&v, "FAT32", &named, tool);
        assert!(unticked.starts_with(tool), "the tool's own words are no longer the head of an unticked format: {unticked}");
        assert!(unticked.contains("pinned"), "the unticked box leaves the name it was given unpinned: {unticked}");
        assert!(!root.join(icon::DEVICE_IMAGE).exists(), "an unticked box wrote a picture anyway");
        let mut ticked = named.to_vec();
        ticked.push((DeviceIcon, Text("usbstick".into())));
        let answer = format_words(&v, "FAT32", &ticked, tool);
        assert!(answer.starts_with(tool), "the words of the format tool are no longer the head of the answer: {answer}");
        assert!(answer.contains("The device icon stands on"), "the answer does not name the icon that stands: {answer}");
        assert!(root.join(icon::DEVICE_IMAGE).is_file(), "a ticked box wrote no picture of its own");
        crate::osprim::set_attr_bits(&root, 0, crate::osprim::SYSTEM | crate::osprim::HIDDEN);
        // A volume that names no folder is refused by the icon act, and that refusal cannot undo the format.
        let nameless = Volume { device: "/dev/fake-Photos".into(), ..Default::default() };
        let answer = format_words(&nameless, "NTFS", &ticked, tool);
        assert!(answer.starts_with(tool) && answer.contains("was not written"), "a format that worked is hidden by an icon that failed: {answer}");
    }

    /// The pair of text files a name needs is the naming act's own work: a volume naming a folder gets them,
    /// a volume naming none is told as such instead of being left quietly unpinned, and a name the file system
    /// holds writes nothing of its own (README: *Renaming a volume*).
    #[test]
    fn the_pin_of_a_name_is_written_where_a_folder_stands_and_told_where_none_does() {
        let here = tempfile::tempdir().expect("a folder of its own");
        let v = Volume { device: "E:".into(), mount_points: vec![here.path().to_path_buf()], fs_type: "FAT32".into(), drive_name: "Samsung Flash Drive".into(), ..Default::default() };
        let plan = name_plan(FileSystem::Fat32, "Samsung 64 GB").expect("a name FAT32 is given");
        let words = name_pins_at(&v, &plan);
        assert!(words.contains("pinned"), "the act does not say the name stands pinned: {words}");
        for name in [icon::AUTORUN, icon::DESKTOP] {
            assert!(here.path().join(name).is_file(), "{name} of the pin did not stand");
        }
        assert!(!here.path().join(icon::DEVICE_IMAGE).is_file(), "a naming act drew a picture of its own");

        // The same volume named to nothing: the pair goes with the name it carried, and the root's mark comes
        // back off with it (README: *Renaming a volume*).
        let cleared = name_plan(FileSystem::Fat32, "").expect("an empty name settles to a plan of nothing");
        let words = name_pins_at(&v, &cleared);
        assert!(words.contains("taken back"), "the pair of a name that went away stands on: {words}");
        for name in [icon::AUTORUN, icon::DESKTOP] {
            assert!(!here.path().join(name).is_file(), "{name} outlived the name it pinned");
        }
        crate::osprim::set_attr_bits(here.path(), 0, crate::osprim::SYSTEM | crate::osprim::HIDDEN);

        let nameless = Volume { device: "/dev/fake-Photos".into(), fs_type: "FAT32".into(), ..Default::default() };
        let words = name_pins_at(&nameless, &plan);
        assert!(words.contains("no folder"), "a volume naming no folder is left quietly unpinned: {words}");

        let there = tempfile::tempdir().expect("a folder of its own");
        let holds = Volume { device: "E:".into(), mount_points: vec![there.path().to_path_buf()], fs_type: "NTFS".into(), drive_name: "Samsung Flash Drive".into(), ..Default::default() };
        let plan = name_plan(FileSystem::Ntfs, "Samsung 64 GB").expect("a name NTFS holds outright");
        assert!(name_pins_at(&holds, &plan).is_empty(), "a name NTFS holds wrote words of its own: {}", name_pins_at(&holds, &plan));
        for name in [icon::AUTORUN, icon::DESKTOP, icon::DEVICE_IMAGE, icon::FOLDER_ICON] {
            assert!(!there.path().join(name).exists(), "{name} stands where the name needed no pin at all");
        }
    }

    /// A format whose device-icon box stayed unticked still carries the name it was given, because the label
    /// the tool wrote is the short variant of it (README: *Formatting a volume*, *Renaming a volume*).
    #[test]
    fn a_format_with_an_unticked_box_still_pins_the_name_it_was_given() {
        let here = tempfile::tempdir().expect("a folder of its own");
        let root = here.path().to_path_buf();
        let v = Volume { device: "E:".into(), mount_points: vec![root.clone()], fs_type: "FAT32".into(), drive_name: "SanDisk Ultra".into(), drive_size: 64_160_400_896, ..Default::default() };
        let tool = "The volume was formatted.\nEverything on it is gone.";
        let answer = format_words(&v, "FAT32", &[(VolumeName, Text("Samsung 64 GB".into()))], tool);
        assert!(answer.starts_with(tool), "the tool's own words are no longer the head of the answer: {answer}");
        assert!(answer.contains("pinned"), "the name the tool cannot carry is left unpinned: {answer}");
        for name in [icon::AUTORUN, icon::DESKTOP] {
            assert!(root.join(name).is_file(), "{name} of the pin did not stand after the format");
        }
        assert!(!root.join(icon::DEVICE_IMAGE).is_file(), "an unticked box drew a picture anyway");
        crate::osprim::set_attr_bits(&root, 0, crate::osprim::SYSTEM | crate::osprim::HIDDEN);
    }

    /// The privileged rename is handed the folder, the name as it was typed and the model, so the one run that
    /// takes the label writes the pin too and naming a volume answers one consent dialog (README: *Renaming a
    /// volume*).
    #[test]
    fn the_privileged_rename_carries_what_the_pin_of_the_name_needs() {
        let v = Volume { device: "F:\\".into(), fs_type: "exFAT".into(), drive_name: "Samsung Flash Drive".into(), drive_size: 64_157_253_632, ..Default::default() };
        // A drive letter is a folder only where Windows answers it (`root_of`); elsewhere it names none.
        let folder = if cfg!(windows) { "F:\\" } else { "" };
        assert_eq!(admin_rename_arguments(&v, FileSystem::ExFat, "Samsung 64 GB", "Samsung 64"), ["F:\\", "Samsung 64", "exFAT", folder, "Samsung 64 GB", "Samsung Flash Drive"]);
        let nameless = Volume { device: "/dev/fake-Photos".into(), fs_type: "FAT32".into(), ..Default::default() };
        assert_eq!(admin_rename_arguments(&nameless, FileSystem::Fat32, "Samsung 64 GB", "SAMSUNG 64"), ["/dev/fake-Photos", "SAMSUNG 64", "FAT32", "", "Samsung 64 GB", ""], "a volume naming no folder says so with an empty folder");
    }

    /// A privileged rename run whose target the machine refuses answers the refusal into the outcome file, and
    /// writes no pin it was not allowed to name a volume for.
    #[test]
    #[cfg(windows)]
    fn the_privileged_rename_refuses_a_target_the_machine_does_not_answer() {
        let (here, outcome) = (tempfile::tempdir().expect("a folder of its own"), outcome_path(RENAME));
        let root = here.path().join("nothing");
        let code = execute(&[RENAME.into(), OUTCOME.into(), outcome.display().to_string(), "Q:\\".into(), "Label".into(), "NTFS".into(), root.display().to_string(), "Label".into(), "Samsung Flash Drive".into()]);
        assert_eq!(code, 1, "a privileged rename of a letter this machine has no answered as success");
        let (ok, text) = read_outcome(&std::fs::read_to_string(&outcome).expect("a refusal is answered into the outcome file too"));
        assert!(!ok && text.contains("win32"), "the machine's own refusal is not in the answer: {text}");
        assert!(!root.exists(), "the refused run left a pin of its own behind: {}", root.display());
        let _ = std::fs::remove_file(&outcome);
    }

    /// The words a naming act answers with: the name the volume carries, and nothing invented for a volume
    /// named to nothing.
    #[test]
    fn the_words_of_a_naming_act_name_the_volume_s_own_or_none() {
        assert_eq!(named_words("FAMILY", ""), "The volume's name is now FAMILY.");
        assert_eq!(named_words("FAMILY", "The longer name it is called stands pinned in autorun.inf and desktop.ini"), "The volume's name is now FAMILY. The longer name it is called stands pinned in autorun.inf and desktop.ini.");
        assert_eq!(named_words("", ""), "The volume carries no name of its own.");
    }

    /// A privileged icon run that was given no folder is refused with the outcome file written, so the window
    /// never has to tell a refusal apart from a run that never started.
    #[test]
    fn a_privileged_icon_run_naming_no_folder_answers_a_refusal() {
        let outcome = outcome_path(ICON);
        let _ = std::fs::remove_file(&outcome);
        assert_eq!(execute(&[ICON.into(), OUTCOME.into(), outcome.display().to_string()]), 1, "a privileged icon run with no folder claimed success");
        let (ok, text) = read_outcome(&std::fs::read_to_string(&outcome).expect("a refusal is answered into the outcome file too"));
        assert!(!ok && !text.is_empty(), "the refusal was answered as {ok} with {text:?}");
        let _ = std::fs::remove_file(&outcome);
    }
}
