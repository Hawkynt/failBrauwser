//! Formatting a volume, naming it, and reading what a device is (README: *Formatting a volume*,
//! *Renaming a volume*, *Device icons*). The option table here **is** the README's option table: a
//! test reads that table out of the README and compares it item for item, the way `vcs::caps.rs`
//! pins the Version control table, so the window and the specification cannot tell two different
//! stories. Nothing here needs a display, and the only program it starts is the format tool asked for its
//! own usage text — a read that writes nothing and ends with this instance, like every reading client
//! (docs\TOOL-STARTS.md). The windows are what refuse what
//! must not be refused, and `format_argv` is the closed gate the command is built through — an
//! option a file system has no place for, a value outside the list the tool's own usage text gives,
//! or a combination that text names as a reason to stop is refused here rather than handed to the
//! tool to be found out.

pub mod act;
pub mod art;
pub mod ico;
pub mod icon;

use crate::drives::Volume;

/// A file system the installed format tool can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSystem {
    Fat,
    Fat32,
    ExFat,
    Ntfs,
    ReFs,
    Udf,
}

/// The words `format` takes after `/FS:`, measured from its own usage text on this build.
const FS_WORDS: [(FileSystem, &str); 6] = [
    (FileSystem::Fat, "FAT"),
    (FileSystem::Fat32, "FAT32"),
    (FileSystem::ExFat, "exFAT"),
    (FileSystem::Ntfs, "NTFS"),
    (FileSystem::ReFs, "ReFS"),
    (FileSystem::Udf, "UDF"),
];

/// Below this size nothing is offered at all: no file system in the table is known to hold a volume
/// that small, and the tool's own refusal stays the authority for anything near the line.
pub const SMALLEST_VOLUME: u64 = 1_000_000;

impl FileSystem {
    /// Every file system the tool names, in the order its usage text lists them.
    pub fn all() -> &'static [FileSystem] {
        &[FileSystem::Fat, FileSystem::Fat32, FileSystem::ExFat, FileSystem::Ntfs, FileSystem::ReFs, FileSystem::Udf]
    }

    /// The word this file system is named by: the tool's, and the one a volume reports back.
    pub fn word(self) -> &'static str {
        FS_WORDS.iter().find(|(fs, _)| *fs == self).map(|(_, word)| *word).unwrap_or_default()
    }

    /// Which file system a word names, in any case and as the tool, the volume or the partition
    /// table spells it. `FAT12` and `FAT16` are one file system here, because one switch writes both
    /// and nothing asks which.
    pub fn parse(word: &str) -> Option<FileSystem> {
        match word.trim().to_uppercase().as_str() {
            "FAT" | "FAT12" | "FAT16" => Some(FileSystem::Fat),
            "FAT32" => Some(FileSystem::Fat32),
            "EXFAT" => Some(FileSystem::ExFat),
            "NTFS" => Some(FileSystem::Ntfs),
            "REFS" => Some(FileSystem::ReFs),
            "UDF" => Some(FileSystem::Udf),
            _ => None,
        }
    }

    /// The allocation units this file system accepts, in bytes, exactly as the tool's usage text
    /// lists them (measured 2026-11-18). FAT and FAT32 also take 128 KiB and 256 KiB, but only above
    /// a 512-byte sector — a fact about the disk rather than the volume, which this window never
    /// reads — so those two sizes are left out rather than offered on a sector size nobody checked.
    pub fn allocation_units(self) -> &'static [u32] {
        match self {
            // Not the numbers of the usage text but the ones this build's tool takes: asked on a letter
            // that holds no volume, 512 to 8192 reach its drive lookup while 12 288 and every larger value
            // are answered as an invalid parameter before a drive is opened — for every file system alike,
            // which leaves ReFS with the 4096 its text also names. README carries both answers.
            FileSystem::Fat | FileSystem::Fat32 | FileSystem::ExFat | FileSystem::Ntfs => &[512, 1024, 2048, 4096, 8192],
            FileSystem::ReFs => &[4096],
            FileSystem::Udf => &[],
        }
    }

    /// How many characters a label of this file system may hold. FAT's 11 is what its drivers have always
    /// taken and exFAT's 11 is what this Windows answers for it (see the row below); NTFS' 32, ReFS' 32 and
    /// UDF's 32 are the file systems' own documented fields, of which only NTFS' has been answered back on
    /// this machine (`docs\WINDOWS.md` names which). A name over the limit is shortened by `name_plan`
    /// rather than refused, and where the machine itself answers, its words are shown.
    pub fn label_limit(self) -> usize {
        match self {
            FileSystem::Fat | FileSystem::Fat32 => 11,
            // Measured here, not read off the file system: eleven characters are written and a twelfth is
            // refused by Windows (win32 154, “the entered volume label exceeds the limit of the target file
            // system”), although exFAT's own field would hold fifteen. README carries both facts.
            FileSystem::ExFat => 11,
            FileSystem::Ntfs | FileSystem::ReFs | FileSystem::Udf => 32,
        }
    }

    /// The count limit the tool states as a stop condition: FAT at most 65 526 clusters.
    fn cluster_ceiling(self) -> Option<u64> {
        match self { FileSystem::Fat => Some(65_526), FileSystem::Fat32 => Some(4_177_917), _ => None }
    }

    /// …and FAT32 above 65 526 of them, which is exactly where FAT stops.
    fn cluster_floor(self) -> Option<u64> {
        match self { FileSystem::Fat32 => Some(65_527), _ => None }
    }
}

/// One knob of a format, item for item the README's option table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatOption {
    VolumeName,
    AllocationUnit,
    Quick,
    Overwrite,
    Compression,
    BiggerFileRecords,
    LogSize,
    ShortNames,
    RepairLogs,
    Trim,
    DirectAccess,
    TransactionalNames,
    Integrity,
    DevDrive,
    Sha256,
    UdfRevision,
    UdfDuplicatedMetadata,
    DeviceIcon,
}

/// The values a knob accepts on one file system, as the tool's usage text names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Values {
    /// On or off, where on is what the switch asks for.
    Toggle,
    /// On or off, where **off** is what the switch asks for (`/NoTrim`, `/NoRepairLogs`).
    InvertedToggle,
    /// A size out of this list, in bytes.
    Sizes(&'static [u32]),
    /// A whole number within these bounds, inclusive.
    Count(u32, u32),
    /// One of these words, spelled exactly so.
    States(&'static [&'static str]),
    /// A volume name, held to the file system's own length and characters.
    Label,
    /// A choice the command never carries: the device icon is written by the act itself.
    Nothing,
}

/// A chosen value of one knob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    On,
    Off,
    Bytes(u32),
    Count(u32),
    State(&'static str),
    Text(String),
}

const ENABLE_DISABLE: [&str; 2] = ["enable", "disable"];
const UDF_REVISIONS: [&str; 5] = ["1.02", "1.50", "2.00", "2.01", "2.50"];

impl FormatOption {
    /// Every option, in the order the README table lists its rows.
    pub fn all() -> &'static [FormatOption] {
        use FormatOption::*;
        &[VolumeName, AllocationUnit, Quick, Overwrite, Compression, BiggerFileRecords, LogSize, ShortNames, RepairLogs, Trim, DirectAccess, TransactionalNames, Integrity, DevDrive, Sha256, UdfRevision, UdfDuplicatedMetadata, DeviceIcon]
    }

    /// The row of the README's option table this option is.
    pub fn readme_row(self) -> &'static str {
        use FormatOption::*;
        match self {
            VolumeName => "volume name",
            AllocationUnit => "allocation unit (cluster size)",
            Quick => "quick instead of full",
            Overwrite => "overwrite the whole volume first",
            Compression => "compress new files by default",
            BiggerFileRecords => "bigger file records",
            LogSize => "log size",
            ShortNames => "short (8.3) names",
            RepairLogs => "write repair logs",
            Trim => "trim while formatting",
            DirectAccess => "direct access storage (DAX)",
            TransactionalNames => "transactional naming (TxF)",
            Integrity => "integrity",
            DevDrive => "dev drive",
            Sha256 => "SHA-256 checksums",
            UdfRevision => "UDF revision",
            UdfDuplicatedMetadata => "duplicate UDF metadata",
            DeviceIcon => "device icon (see *Device icons*)",
        }
    }

    /// The switches this option contributes to one file system's command — which is the cell of the
    /// README table — and none at all where that table says `—`. The two switches that ask for the
    /// *off* state (`/NoTrim`, `/NoRepairLogs`) are named here as the switches they are; `format_argv`
    /// is where their meaning is turned around, and a test pins both directions.
    pub fn switches_for(self, fs: FileSystem) -> &'static [&'static str] {
        use FileSystem::*;
        use FormatOption::*;
        match (self, fs) {
            (VolumeName, Fat | Fat32 | ExFat | Ntfs | ReFs | Udf) => &["/V:"],
            (AllocationUnit, Fat | Fat32 | ExFat | Ntfs | ReFs) => &["/A:"],
            (Quick, Fat | Fat32 | ExFat | Ntfs | ReFs | Udf) => &["/Q"],
            (Overwrite, Fat | Fat32 | ExFat | Ntfs | ReFs | Udf) => &["/P:"],
            (Compression, Ntfs) => &["/C"],
            (BiggerFileRecords, Ntfs) => &["/L:"],
            (LogSize, Ntfs) => &["/LogSize:"],
            (ShortNames, Ntfs) => &["/S:"],
            (RepairLogs, Ntfs) => &["/NoRepairLogs"],
            (Trim, Fat | Fat32 | ExFat | Ntfs | ReFs | Udf) => &["/NoTrim"],
            (DirectAccess, Ntfs) => &["/DAX:"],
            (TransactionalNames, Ntfs) => &["/TXF:"],
            (Integrity, ReFs) => &["/I:"],
            (DevDrive, ReFs) => &["/DevDrv"],
            (Sha256, ReFs) => &["/SHA256Checksums"],
            (UdfRevision, Udf) => &["/R:"],
            (UdfDuplicatedMetadata, Udf) => &["/D"],
            _ => &[],
        }
    }

    /// What this option may be set to on one file system, or nothing where that file system has no
    /// such option.
    pub fn values(self, fs: FileSystem) -> Option<Values> {
        use FormatOption::*;
        if self != DeviceIcon && self.switches_for(fs).is_empty() { return None; }
        Some(match self {
            VolumeName => Values::Label,
            AllocationUnit => Values::Sizes(fs.allocation_units()),
            Quick => Values::Toggle,
            // The tool states what a pass count means, not a ceiling; four is the most this window
            // offers, and 0 is its own answer (a zero written to every sector).
            Overwrite => Values::Count(0, 4),
            Compression => Values::Toggle,
            BiggerFileRecords => Values::States(&ENABLE_DISABLE),
            // The tool names 2 MB as its floor and 0 as its own size-dependent default; the top is
            // the largest value this window offers and the tool's refusal is the authority.
            LogSize => Values::Count(0, 1_048_576),
            ShortNames => Values::States(&ENABLE_DISABLE),
            RepairLogs => Values::InvertedToggle,
            Trim => Values::InvertedToggle,
            DirectAccess => Values::States(&ENABLE_DISABLE),
            TransactionalNames => Values::States(&ENABLE_DISABLE),
            Integrity => Values::States(&ENABLE_DISABLE),
            DevDrive => Values::Toggle,
            Sha256 => Values::Toggle,
            UdfRevision => Values::States(&UDF_REVISIONS),
            UdfDuplicatedMetadata => Values::Toggle,
            DeviceIcon => Values::Nothing,
        })
    }

    /// The name the format window gives this option's control, which is also the name a scenario
    /// drives it by (`format-set allocation-unit = 4096`). Stable: a scenario names these, so a row
    /// renamed here is a scenario that stops answering, which is the point.
    pub fn field(self) -> &'static str {
        use FormatOption::*;
        match self {
            VolumeName => "volume-name",
            AllocationUnit => "allocation-unit",
            Quick => "quick",
            Overwrite => "overwrite",
            Compression => "compression",
            BiggerFileRecords => "bigger-file-records",
            LogSize => "log-size",
            ShortNames => "short-names",
            RepairLogs => "repair-logs",
            Trim => "trim",
            DirectAccess => "direct-access",
            TransactionalNames => "transactional-names",
            Integrity => "integrity",
            DevDrive => "dev-drive",
            Sha256 => "sha256",
            UdfRevision => "udf-revision",
            UdfDuplicatedMetadata => "udf-duplicated-metadata",
            DeviceIcon => "device-icon",
        }
    }

    /// The words the window shows for this option.
    pub fn label(self) -> &'static str {
        use FormatOption::*;
        match self {
            VolumeName => "Volume name",
            AllocationUnit => "Allocation unit size",
            Quick => "Quick format",
            Overwrite => "Overwrite passes",
            Compression => "Compress new files by default",
            BiggerFileRecords => "Bigger file records",
            LogSize => "Log size (kB)",
            ShortNames => "Short (8.3) names",
            RepairLogs => "Write repair logs",
            Trim => "Trim while formatting",
            DirectAccess => "Direct access storage (DAX)",
            TransactionalNames => "Transactional naming (TxF)",
            Integrity => "Integrity",
            DevDrive => "Dev drive",
            Sha256 => "SHA-256 checksums",
            UdfRevision => "UDF revision",
            UdfDuplicatedMetadata => "Duplicate UDF metadata",
            DeviceIcon => "Write a device icon",
        }
    }
}

/// What a command is for: the volume by its letter and its size, which is what the cluster counts of
/// FAT and FAT32 are measured against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub letter: char,
    pub size: u64,
}

impl Target {
    /// The volume of a `Volume` as a command target: its drive letter, and the size the machine
    /// names for it.
    pub fn of(volume: &Volume) -> Option<Target> {
        let letter = drive_letter(&volume.device).or_else(|| volume.mount_points.first().and_then(|m| drive_letter(&m.to_string_lossy())))?;
        Some(Target { letter, size: if volume.size > 0 { volume.size } else { volume.drive_size } })
    }
}

/// The drive letter out of `C:`, `C:\folder`, `/dev/sda1`-shaped noise or a UNC path — nothing where
/// the text names no letter.
pub(crate) fn drive_letter(text: &str) -> Option<char> {
    let mut chars = text.trim().chars();
    match (chars.next(), chars.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => Some(letter.to_ascii_lowercase()),
        _ => None,
    }
}

/// The file systems to offer for one volume, out of the names the system or the format tool said this
/// machine has. An empty answer means the machine refused to say, in which case every file system the
/// installed tool names is what the window offers. A file system is left out when no allocation unit
/// it accepts keeps this volume's cluster count inside the band the tool states, or when the volume is
/// one that is refused outright.
pub fn accepted(volume: &Volume, offered: &[String]) -> Vec<FileSystem> {
    if refusal(volume).is_some() { return Vec::new(); }
    let size = if volume.size > 0 { volume.size } else { volume.drive_size };
    let named: Vec<FileSystem> = offered.iter().filter_map(|w| FileSystem::parse(w)).collect();
    let pool: Vec<FileSystem> = if named.is_empty() { FileSystem::all().to_vec() } else { named };
    FileSystem::all().iter().copied().filter(|fs| pool.contains(fs) && fits(*fs, size)).collect()
}

/// The offer of one volume in the words a window shows: the machine's answer narrowed to this volume,
/// which is what `Volume.supported_file_systems` holds and what the format page lists.
pub fn offer(volume: &Volume) -> Vec<String> {
    let named = tool_file_systems().iter().map(|fs| fs.word().to_string()).collect::<Vec<_>>();
    accepted(volume, &named).iter().map(|fs| fs.word().to_string()).collect()
}

/// The file systems the installed format tool answers for, in this module's order. Its own usage text is
/// asked for once per run and kept, because that tool is the program which will run and its list is the
/// one matching the switches this table names: a build whose tool answers for fewer of them is offered
/// fewer. Empty where the read fails, which leaves the whole table as the offer instead of inventing a
/// shorter list (README: *Formatting a volume*).
pub fn tool_file_systems() -> Vec<FileSystem> {
    static ONCE: std::sync::OnceLock<Vec<FileSystem>> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| named_by_tool(&tool_usage_text())).clone()
}

/// Which of the table's file systems the tool names in the part of its usage text that explains `/FS:`.
/// The words are compared as whole words, because `FAT32` named on its own is not a build that formats
/// to FAT, and in any case, because the sentences around them are in the user's language while the
/// file-system names are not.
fn named_by_tool(text: &str) -> Vec<FileSystem> {
    let block = fs_block(text).to_lowercase();
    let tokens = block.split(|c: char| !c.is_ascii_alphanumeric()).filter(|token| !token.is_empty()).collect::<Vec<_>>();
    FileSystem::all().iter().copied().filter(|fs| tokens.iter().any(|token| *token == fs.word().to_lowercase())).collect()
}

/// The tool's own explanation of `/FS:`: the paragraph whose first non-blank characters are `/FS:`, with
/// the continuation lines under it. The synopsis line carries `/FS:` too, inside brackets and naming no
/// file system, so a search for the words has to start at the paragraph that explains the option.
fn fs_block(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(at) = lines.iter().position(|line| line.trim_start().starts_with("/FS:")) else { return String::new() };
    let mut end = at + 1;
    while end < lines.len() && !lines[end].trim_start().starts_with('/') && !lines[end].trim().is_empty() {
        end += 1;
    }
    lines[at..end].join("\n")
}

/// The usage text, from the tool itself: `/?` names the usage and writes nothing. The read goes through
/// the funnel every reading client of ours goes through, so it is started without a window and its client
/// ends with this instance.
#[cfg(windows)]
fn tool_usage_text() -> String {
    // Asked by its own file name: a start without an extension gets `.exe` appended, and the tool on this
    // build is `format.com` — measured, asking it as `format` answers that it is not installed.
    crate::vcs::run::ask("format.com", std::path::Path::new("."), &["/?"]).text
}

/// Nothing measurable to ask on the other side: the file systems there come from `mkfs.<type>` programs,
/// which of them a distribution carries is not something this machine can answer, and inventing their
/// names is what this module refuses to do. The list stays the table's own, recorded as unverified.
#[cfg(not(windows))]
fn tool_usage_text() -> String {
    String::new()
}

/// Whether this file system can hold this volume: inside the size the window offers at all, and with
/// one of its own allocation units inside the cluster band the tool states as a stop condition.
fn fits(fs: FileSystem, size: u64) -> bool {
    if size < SMALLEST_VOLUME { return false; }
    let units = fs.allocation_units();
    units.is_empty() || units.iter().any(|u| cluster_count_ok(fs, size, *u).is_ok())
}

/// The tool's own stop condition, stated here as a reason to refuse before a command exists.
fn cluster_count_ok(fs: FileSystem, size: u64, unit: u32) -> Result<(), String> {
    if size == 0 || unit == 0 { return Ok(()); }
    let clusters = size / unit as u64;
    if let Some(ceiling) = fs.cluster_ceiling().filter(|c| clusters > *c) { return Err(format!("{} takes at most {ceiling} clusters; {} clusters of {unit} B would be {clusters} on a volume of {size} B", fs.word(), fs.word())); }
    if let Some(floor) = fs.cluster_floor().filter(|f| clusters < *f) { return Err(format!("{} takes at least {floor} clusters; {} clusters of {unit} B would be {clusters} on a volume of {size} B", fs.word(), fs.word())); }
    Ok(())
}

/// Why this volume is refused outright, in words the window shows as they are. Whether a volume is
/// busy belongs to the windows, which hold the operations that make it busy.
pub fn refusal(volume: &Volume) -> Option<String> {
    if volume.optical { return Some("an optical drive holds no file system of its own to write".into()); }
    // The volume Windows runs from is answered by the machine on this platform: the drives model's
    // `system` flag names an internal fixed volume there — the one that gets no unmount button — and says
    // nothing about where Windows lives (docs\WINDOWS.md, measured). Elsewhere that flag is the hint the
    // machine itself gives for the system's own volume.
    #[cfg(windows)]
    if let Some(system) = crate::win::storage::system_volume_letter()
        && drive_letter(&volume.device) == Some(system) {
        return Some("this is the volume Windows runs from".into());
    }
    #[cfg(not(windows))]
    if volume.system { return Some("this is a system volume — the volume the system runs from or boots from".into()); }
    if volume.read_only { return Some("this volume is read-only".into()); }
    if !volume.share_uri.is_empty() { return Some("this is a folder on another computer, not a volume of its own".into()); }
    // A drive the machine itself calls remote has no volume of its own to write, whether or not a folder
    // on it is mounted here: measured on this machine, two mapped drives answer it, and the lookup of
    // their UNC origin answers nothing for them.
    if volume.network { return Some("this is a drive on another computer, not a volume this machine holds".into()); }
    if !volume.protocol.is_empty() { return Some("this is a phone or a camera, whose storage is not formatted from here".into()); }
    if (if volume.size > 0 { volume.size } else { volume.drive_size }) < SMALLEST_VOLUME { return Some("the machine names no usable size for this volume, so no file system can be chosen for it".into()); }
    None
}

/// Whether formatting this volume would pull the file this program runs from out from under itself.
pub fn holds_our_own_image(volume: &Volume, image: &std::path::Path) -> bool {
    let (Some(own), Some(own_letter)) = (image.to_str().and_then(drive_letter), Target::of(volume).map(|t| t.letter)) else { return false };
    own == own_letter
}

/// The label a file system can hold, and the name the volume is called: the same string wherever the
/// file system has room for it, and a shortened label with the name kept in `autorun.inf` and
/// `desktop.ini` wherever it does not (README: *A name too long for the file system is kept*).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamePlan {
    /// What goes onto the volume: the tool's `/V:` word, and what `e2label` and friends take.
    pub label: String,
    /// The name as its owner typed it, which is what the two files pin.
    pub called: String,
    /// Whether the two files have to pin anything, which is exactly where they differ.
    pub pinned: bool,
}

pub fn name_plan(fs: FileSystem, name: &str) -> Result<NamePlan, String> {
    let called = name.trim();
    if called.is_empty() { return Ok(NamePlan { label: String::new(), called: String::new(), pinned: false }); }
    if let Some(c) = called.chars().find(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) { return Err(format!("a volume name cannot hold {c:?}")); }
    let fat = matches!(fs, FileSystem::Fat | FileSystem::Fat32);
    if let Some(c) = called.chars().find(|c| fat && matches!(c, '+' | ',' | ';' | '=' | '[' | ']')) { return Err(format!("{} cannot hold {c:?} in a volume name", fs.word())); }
    let collapsed = called.split_whitespace().collect::<Vec<_>>().join(" ");
    let whole = if fat { collapsed.to_uppercase() } else { collapsed };
    let label = if whole.chars().count() > fs.label_limit() { shorten_to_word(&whole, fs.label_limit())? } else { whole };
    if label.is_empty() { return Err(format!("nothing of {called:?} is left in the {} characters {} takes", fs.label_limit(), fs.word())); }
    Ok(NamePlan { label: label.clone(), called: called.to_string(), pinned: label != called })
}

/// Whether this machine's format tool takes this label on its `/V:` switch. Measured on 2026-11-19 by
/// handing the tool a name on a letter that holds no volume, so the answer came from its own parameter
/// check: a value holding a space is refused (`invalid parameter`, exit 4) whether the space is leading,
/// trailing, single or several, while a value without one reaches the drive lookup. A `/` inside the value
/// is answered as a switch of its own and a `"` is refused outright — both are characters `name_plan`
/// forbids already, so the space is the whole of what is left to settle here.
pub fn tool_takes_name(label: &str) -> bool {
    !label.contains(' ')
}

/// The name of the chosen set, settled by the plan exactly as the command would have carried it, and
/// refused by the same `name_plan` for a name no file system can hold.
fn chosen_plan(fs: FileSystem, chosen: &[(FormatOption, Value)]) -> Result<Option<NamePlan>, String> {
    let Some(Value::Text(name)) = chosen.iter().find(|(option, _)| *option == FormatOption::VolumeName).map(|(_, value)| value) else { return Ok(None) };
    name_plan(fs, name).map(Some)
}

/// The label the privileged run has to write itself, because the tool's switch cannot carry it. `None` for
/// a name the switch does take, so nothing is ever named twice: the command carries a name or the run is
/// told it, never both.
pub fn carried_name(fs: FileSystem, chosen: &[(FormatOption, Value)]) -> Result<Option<String>, String> {
    Ok(chosen_plan(fs, chosen)?.filter(|plan| !plan.label.is_empty() && !tool_takes_name(&plan.label)).map(|plan| plan.label))
}

/// Cut a name down to what the file system holds, pulling a cut that would land inside a word back to
/// the word in front of it. A single word longer than the limit keeps its own head, which is the last
/// answer rather than the first one: it is still the same brand or the same word, cut short.
fn shorten_to_word(name: &str, limit: usize) -> Result<String, String> {
    let mut kept = String::new();
    for word in name.split_whitespace() {
        let with = if kept.is_empty() { word.to_string() } else { format!("{kept} {word}") };
        if with.chars().count() <= limit { kept = with; continue; }
        if kept.is_empty() {
            let head: String = word.chars().take(limit).collect();
            return Ok(head.trim_end_matches(['-', '.', ' ']).to_string());
        }
        break;
    }
    Ok(kept.trim_end_matches(['-', '.', ' ']).to_string())
}

/// The command for one file system, in the order the README lists its options. Every call carries
/// `/Y` — no prompt, an empty label accepted — and never `/X`: a volume something of this program is
/// using is refused outright rather than torn out from under it.
pub fn format_argv(target: Target, fs: FileSystem, chosen: &[(FormatOption, Value)]) -> Result<Vec<String>, String> {
    for (option, _) in chosen {
        if chosen.iter().filter(|(o, _)| o == option).count() > 1 { return Err(format!("{} was offered twice", option.label())); }
    }
    let quick = chosen.iter().any(|(o, v)| *o == FormatOption::Quick && *v == Value::On);
    let mut argv: Vec<String> = vec![format!("{}:", target.letter.to_ascii_uppercase()), format!("/FS:{}", fs.word())];
    for option in FormatOption::all() {
        if *option == FormatOption::DeviceIcon { continue; }
        let Some(value) = chosen.iter().find(|(o, _)| o == option).map(|(_, v)| v) else { continue };
        let switches = option.switches_for(fs);
        if switches.is_empty() { return Err(format!("{} has no {}", fs.word(), option.label().to_lowercase())); }
        let switch = switches[0];
        let allowed = option.values(fs).unwrap_or(Values::Nothing);
        if *option == FormatOption::Overwrite && quick { return Err("/Q overrides the pass count, so a format cannot overwrite first and be quick at once".into()); }
        if let (FormatOption::AllocationUnit, Value::Bytes(unit)) = (*option, value) {
            if fs == FileSystem::Ntfs && chosen.iter().any(|(o, v)| *o == FormatOption::Compression && *v == Value::On) && *unit > 4096 { return Err("the tool supports NTFS compression only up to a 4096-byte allocation unit".into()); }
            cluster_count_ok(fs, target.size, *unit)?;
        }
        if let (FormatOption::LogSize, Value::Count(kb)) = (*option, value) && *kb != 0 && *kb < 2048 { return Err("the tool's smallest NTFS log size is 2 MB (2048 kB); 0 asks for its own default".into()); }
        if *option == FormatOption::UdfDuplicatedMetadata && *value == Value::On {
            let revision = chosen.iter().find(|(o, _)| *o == FormatOption::UdfRevision).and_then(|(_, v)| if let Value::State(word) = v { Some(*word) } else { None }).unwrap_or("2.01");
            if revision != "2.50" { return Err(format!("the tool duplicates UDF metadata in 2.50 only, and the revision here is {revision}")); }
        }
        match (allowed, value) {
            (Values::Toggle, Value::On) => argv.push(switch.to_string()),
            (Values::InvertedToggle, Value::Off) => argv.push(switch.to_string()),
            (Values::Toggle | Values::InvertedToggle, Value::On | Value::Off) => {}
            (Values::Label, Value::Text(text)) => {
                let plan = name_plan(fs, text)?;
                // The plan is settled here even for a label the switch cannot carry, so a name nothing can
                // hold is refused before a command exists; the label itself then goes over as the act's own
                // argument (`carried_name`) and the tool is started with switches and nothing else.
                if !plan.label.is_empty() && tool_takes_name(&plan.label) { argv.push(format!("{switch}{}", plan.label)); }
            }
            (Values::Sizes(list), Value::Bytes(unit)) if list.contains(unit) => argv.push(format!("{switch}{unit}")),
            (Values::Count(lo, hi), Value::Count(count)) if *count >= lo && *count <= hi => argv.push(format!("{switch}{count}")),
            (Values::States(words), Value::State(word)) if words.contains(word) => argv.push(format!("{switch}{word}")),
            (Values::Nothing, _) => return Err(format!("{} answers {} alone", fs.word(), option.label())),
            (_, _) => return Err(format!("{} does not take {value:?} for {} (allowed: {})", fs.word(), option.label(), describe(&allowed))),
        }
    }
    argv.push("/Y".to_string());
    Ok(argv)
}

/// The allowed values in words, for the refusal that quotes them.
fn describe(values: &Values) -> String {
    match values {
        Values::Toggle | Values::InvertedToggle => "on or off".into(),
        Values::Sizes(list) => list.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
        Values::Count(lo, hi) => format!("{lo} to {hi}"),
        Values::States(words) => words.join(" / "),
        Values::Label => "a volume name".into(),
        Values::Nothing => "nothing".into(),
    }
}

/// The size in the units the packaging prints, which are decimal: 1 GB is 1 000 000 000 bytes, so the
/// 64 160 400 896-byte stick on this machine reads `64 GB`. One decimal below ten of a unit (a 4.7 GB
/// disc), a whole number at or above it.
pub fn decimal_size(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [(1_000_000_000_000, "TB"), (1_000_000_000, "GB"), (1_000_000, "MB"), (1, "B")];
    for (scale, unit) in UNITS {
        if bytes >= scale {
            let value = bytes as f64 / scale as f64;
            let rounded = (value * 10.0).round() / 10.0;
            return if value < 10.0 && rounded.fract() != 0.0 { format!("{rounded:.1} {unit}") } else { format!("{} {unit}", rounded.round() as u64) };
        }
    }
    format!("{bytes} B")
}

/// The brand word a model string gives for the icon: the longest word made of letters alone, with the
/// words that name a connection rather than a maker left out, and the letters a bare model number
/// starts with as the last answer. Empty means the machine named no brand, and then the icon prints
/// the size alone.
pub fn vendor_token(model: &str) -> String {
    const CONNECTION: [&str; 9] = ["nvme", "sata", "scsi", "usb", "ata", "atapi", "ide", "pcie", "ssd"];
    let model = model.trim();
    if model.is_empty() || model.to_lowercase().contains("reader") { return String::new(); }
    let words: Vec<String> = model.split_whitespace().map(|w| w.trim_end_matches(['-', '.', ',']).to_string()).filter(|w| !w.is_empty()).collect();
    let is_connection = |w: &str| CONNECTION.contains(&w.to_lowercase().as_str());
    if let Some(brand) = words.iter().filter(|w| w.chars().all(|c| c.is_alphabetic()) && !is_connection(w)).max_by_key(|w| (w.chars().count(), std::cmp::Reverse(words.iter().position(|x| x == *w).unwrap_or(0)))) {
        return if brand.chars().all(|c| c.is_lowercase()) { brand[..1].to_uppercase() + &brand[1..] } else { brand.clone() };
    }
    let bare = words.iter().find(|w| !is_connection(w)).map(|w| w.chars().take_while(|c| c.is_alphabetic()).take(4).collect::<String>()).unwrap_or_default();
    if bare.chars().all(|c| c.is_lowercase()) { bare[..1].to_uppercase() + &bare[1..] } else { bare }
}

/// The volume name the format window proposes: the brand and the size, which are the same two words
/// the device icon carries.
pub fn proposed_name(model: &str, size: u64) -> String {
    let (brand, size) = (vendor_token(model), decimal_size(size));
    if brand.is_empty() { size } else { format!("{brand} {size}") }
}

/// The shape a device icon is drawn for (README: *Device icons*).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Nvme,
    Ssd,
    Hdd,
    UsbStick,
    SdCard,
    MicroSd,
    Floppy35,
    Floppy525,
    Optical,
}

impl Shape {
    pub fn all() -> &'static [Shape] {
        &[Shape::Nvme, Shape::Ssd, Shape::Hdd, Shape::UsbStick, Shape::SdCard, Shape::MicroSd, Shape::Floppy35, Shape::Floppy525, Shape::Optical]
    }

    pub fn label(self) -> &'static str {
        match self {
            Shape::Nvme => "NVMe drive",
            Shape::Ssd => "Solid-state drive",
            Shape::Hdd => "Hard disk",
            Shape::UsbStick => "USB stick",
            Shape::SdCard => "SD card",
            Shape::MicroSd => "µSD card",
            Shape::Floppy35 => "3.5″ floppy",
            Shape::Floppy525 => "5.25″ floppy",
            Shape::Optical => "Optical drive",
        }
    }

    /// The name the data files carry (`autorun.inf`'s `ModelName`): plain ASCII, because that file is a
    /// single-byte one and the shown label of a µSD card or a 5.25″ floppy holds characters it cannot hold.
    pub fn data_label(self) -> &'static str {
        match self {
            Shape::Nvme => "NVMe drive",
            Shape::Ssd => "solid-state drive",
            Shape::Hdd => "hard disk",
            Shape::UsbStick => "USB stick",
            Shape::SdCard => "SD card",
            Shape::MicroSd => "micro SD card",
            Shape::Floppy35 => "3.5 inch floppy",
            Shape::Floppy525 => "5.25 inch floppy",
            Shape::Optical => "optical drive",
        }
    }

    /// The tracked glyph under `assets\device-glyphs\`.
    pub fn asset(self) -> &'static str {
        match self {
            Shape::Nvme => "nvme",
            Shape::Ssd => "ssd",
            Shape::Hdd => "hdd",
            Shape::UsbStick => "usbstick",
            Shape::SdCard => "sdcard",
            Shape::MicroSd => "msd",
            Shape::Floppy35 => "floppy35",
            Shape::Floppy525 => "floppy525",
            Shape::Optical => "optical",
        }
    }

    /// The shape whose tracked picture carries this asset name, which is how a window hands a chosen shape
    /// back. A word that names no asset answers nothing rather than the closest guess.
    pub fn parse(asset: &str) -> Option<Shape> {
        Self::all().iter().copied().find(|s| s.asset() == asset)
    }

    /// What the machine says the device is, or nothing where it says too little. An SD card and a µSD
    /// card cannot be told apart from across a reader, and one floppy from another only by its
    /// capacity: the window shows what was seen either way and lets the shape be changed.
    pub fn detected(volume: &Volume) -> Option<Shape> {
        if volume.optical { return Some(Shape::Optical); }
        let size = if volume.drive_size > 0 { volume.drive_size } else { volume.size };
        if size > 0 && size <= 1_300_000 { return Some(Shape::Floppy525); }
        if size > 1_300_000 && size <= 2_880_000 { return Some(Shape::Floppy35); }
        match volume.bus.as_str() {
            "nvme" => return Some(Shape::Nvme),
            "sdio" | "mmc" => return Some(Shape::SdCard),
            _ => {}
        }
        if volume.rotation_rate == 0 && !volume.removable { return Some(Shape::Ssd); }
        if volume.rotation_rate > 0 { return Some(Shape::Hdd); }
        if volume.usb || (volume.removable && size >= 4_000_000_000) { return Some(Shape::UsbStick); }
        None
    }

    /// The evidence the window spells out beside the shape, so a shape chosen where the machine could
    /// not tell is seen to be chosen.
    pub fn evidence(volume: &Volume) -> String {
        let size = decimal_size(if volume.drive_size > 0 { volume.drive_size } else { volume.size });
        let seen = match volume.bus.as_str() {
            "" => match volume.rotation_rate {
                0 => "no seek penalty".to_string(),
                r if r > 0 => format!("{r} rpm"),
                _ => "no connection named".to_string(),
            },
            bus => format!("bus {bus}"),
        };
        match Shape::detected(volume) {
            Some(Shape::SdCard) => format!("detected: {seen}, {size} — an µSD card in a reader is indistinguishable from an SD card"),
            Some(Shape::Floppy35) | Some(Shape::Floppy525) => format!("detected: {size} — the capacity says which floppy this is, not the machine"),
            Some(_) => format!("detected: {seen}, {size}"),
            None => format!("nothing detected: {seen}, {size} — choose the shape"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use FormatOption::*;
    use Value::*;

    // The tests spell an allocation unit the way a person reads one; the table, the switch and the
    // machine's answer all carry plain byte counts.
    const KB: u32 = 1024;
    const MB: u32 = 1024 * KB;

    /// The README's option table: row → one cell per file system, in `FileSystem::all` order.
    fn readme_table() -> Vec<(String, Vec<String>)> {
        let readme = include_str!("../../README.md");
        let start = readme.find("| Option | FAT / FAT32 | exFAT | NTFS | ReFS | UDF |").expect("the README has the format option table");
        readme[start..].lines().skip(2).take_while(|l| l.starts_with('|')).map(|line| {
            let cells: Vec<String> = line.trim_matches('|').split('|').map(|c| c.trim().replace('`', "")).collect();
            (cells[0].clone(), cells[1..].to_vec())
        }).collect()
    }

    #[test]
    fn the_format_options_are_the_readme_table() {
        let table = readme_table();
        for option in FormatOption::all() {
            let row = option.readme_row();
            let (_, cells) = table.iter().find(|(item, _)| item == row).unwrap_or_else(|| panic!("the README option table has no row {row:?}"));
            for fs in FileSystem::all() {
                let cell = &cells[match fs { FileSystem::Fat | FileSystem::Fat32 => 0, FileSystem::ExFat => 1, FileSystem::Ntfs => 2, FileSystem::ReFs => 3, FileSystem::Udf => 4 }];
                let switches: Vec<String> = option.switches_for(*fs).iter().map(|s| s.to_string()).collect();
                // `—` means no switch, and `✓` means the option is offered by every file system yet
                // carries none — the device icon — which the other test pins to that option alone.
                let expected: Vec<String> = if cell == "—" || cell == "✓" { Vec::new() } else { cell.split(',').map(|c| c.trim().to_string()).collect() };
                assert_eq!(switches, expected, "{row} for {}: the README says {cell:?}", fs.word());
            }
        }
    }

    #[test]
    fn every_row_of_the_readme_option_table_is_an_option() {
        let rows: Vec<&str> = FormatOption::all().iter().map(|o| o.readme_row()).collect();
        for (item, cells) in readme_table() {
            assert!(rows.contains(&item.as_str()), "the README option table lists {item:?}, which no option answers");
            if cells.iter().all(|c| c == "✓") { assert_eq!(item, FormatOption::DeviceIcon.readme_row(), "only the device icon is ticked everywhere"); }
        }
    }

    /// The device icon is offered by every file system and contributes nothing to the command,
    /// because writing it never formats anything.
    #[test]
    fn the_device_icon_is_offered_everywhere_and_carries_no_switch() {
        for fs in FileSystem::all() {
            assert!(DeviceIcon.switches_for(*fs).is_empty(), "{} carries no switch for the icon", fs.word());
            assert_eq!(DeviceIcon.values(*fs), Some(Values::Nothing));
        }
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ExFat, &[(DeviceIcon, On)]).unwrap(), vec!["F:", "/FS:exFAT", "/Y"]);
    }

    #[test]
    fn the_switches_are_restricted_only_where_the_tool_marks_them() {
        for fs in FileSystem::all() {
            assert_eq!(LogSize.switches_for(*fs) == ["/LogSize:"], *fs == FileSystem::Ntfs);
            assert_eq!(Integrity.switches_for(*fs) == ["/I:"], *fs == FileSystem::ReFs);
            assert_eq!(Sha256.switches_for(*fs) == ["/SHA256Checksums"], *fs == FileSystem::ReFs);
            assert_eq!(DevDrive.switches_for(*fs) == ["/DevDrv"], *fs == FileSystem::ReFs);
            assert_eq!(RepairLogs.switches_for(*fs) == ["/NoRepairLogs"], *fs == FileSystem::Ntfs);
            assert_eq!(UdfRevision.switches_for(*fs) == ["/R:"], *fs == FileSystem::Udf);
            assert_eq!(UdfDuplicatedMetadata.switches_for(*fs) == ["/D"], *fs == FileSystem::Udf);
            assert_eq!(AllocationUnit.switches_for(*fs).is_empty(), *fs == FileSystem::Udf);
            assert_eq!(Trim.switches_for(*fs), ["/NoTrim"], "the tool marks no file system for trim");
            assert_eq!(Overwrite.switches_for(*fs), ["/P:"], "the tool marks no file system for the passes");
        }
    }

    /// The allocation-unit ladders, measured from what the tool takes on this build rather than from what
    /// its usage text lists.
    #[test]
    fn the_allocation_units_are_the_ones_the_tool_actually_takes() {
        // The tool's usage text lists more (2 MiB for NTFS, 32 MiB for exFAT, 64 KiB for FAT, 64 KiB for
        // ReFS), but its own parameter check answers “invalid parameter” for anything above 8192 before it
        // opens a drive, so the offer stops where the tool accepts — README carries both answers.
        assert_eq!(FileSystem::Fat.allocation_units(), FileSystem::Fat32.allocation_units());
        assert_eq!(FileSystem::Fat.allocation_units(), &[512, 1024, 2048, 4096, 8192]);
        assert_eq!(FileSystem::ExFat.allocation_units(), FileSystem::Fat.allocation_units(), "the ceiling is the tool's own, not the file system's");
        assert_eq!(FileSystem::Ntfs.allocation_units(), FileSystem::Fat.allocation_units());
        assert_eq!(FileSystem::ReFs.allocation_units(), &[4096], "64 KiB is what the text names and the tool refuses");
        assert!(FileSystem::Udf.allocation_units().is_empty());
        for fs in [FileSystem::Fat, FileSystem::ExFat, FileSystem::Ntfs, FileSystem::ReFs] {
            assert!(fs.allocation_units().contains(&4096), "{} offers 4096", fs.word());
        }
        assert!(!FileSystem::Ntfs.allocation_units().contains(&16_384), "the tool refuses 16 KiB before it opens a drive");
    }

    #[test]
    fn a_switch_the_file_system_has_no_option_for_is_refused_by_name() {
        for (option, fs) in [(LogSize, FileSystem::ExFat), (Integrity, FileSystem::Ntfs), (AllocationUnit, FileSystem::Udf), (Compression, FileSystem::Fat32)] {
            let error = format_argv(Target { letter: 'F', size: 64_160_400_896 }, fs, &[(option, On)]).unwrap_err();
            assert!(error.contains(fs.word()) && error.contains(&option.label().to_lowercase()), "{option:?} on {}: {error}", fs.word());
        }
    }

    #[test]
    fn an_allocation_unit_outside_the_tools_list_is_refused() {
        let error = format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ExFat, &[(AllocationUnit, Bytes(3 * MB))]).unwrap_err();
        assert!(error.contains("Bytes(3145728)"), "{error}");
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ExFat, &[(AllocationUnit, Bytes(8 * KB))]).unwrap(), vec!["F:", "/FS:exFAT", "/A:8192", "/Y"]);
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ReFs, &[(AllocationUnit, Bytes(512))]).unwrap_err().contains("512"));
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(AllocationUnit, Bytes(16 * KB))]).unwrap_err().contains("16384"), "the tool refuses 16 KiB, so this window does");
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ExFat, &[(AllocationUnit, Bytes(131_072))]).unwrap_err().contains("131072"), "the 128 KiB cluster this machine's own disk tools wrote is not a choice this tool takes");
    }

    /// The command of the authorized verification step, in full. The name is missing from it on purpose: this
    /// name holds a space, which the tool's `/V:` refuses, so the command stays with switches and the run
    /// writes that name itself once the format worked.
    #[test]
    fn the_command_names_the_volume_the_file_system_then_the_options_in_order() {
        let argv = format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[
            (VolumeName, Text("Samsung 64 GB".into())),
            (AllocationUnit, Bytes(8192)),
            (Quick, On),
            (LogSize, Count(0)),
            (ShortNames, State("disable")),
            (RepairLogs, Off),
        ]).unwrap();
        assert_eq!(argv, vec!["F:", "/FS:NTFS", "/A:8192", "/Q", "/LogSize:0", "/S:disable", "/NoRepairLogs", "/Y"]);
    }

    /// Both directions of the two switches that ask for the off state.
    #[test]
    fn the_two_off_switches_appear_only_when_their_option_is_off() {
        let on = format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Trim, On), (RepairLogs, On)]).unwrap();
        assert!(!on.iter().any(|a| a.starts_with("/No")), "on means no switch: {on:?}");
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Trim, Off), (RepairLogs, Off)]).unwrap(), vec!["F:", "/FS:NTFS", "/NoRepairLogs", "/NoTrim", "/Y"]);
    }

    #[test]
    fn the_pass_count_is_refused_once_quick_is_on() {
        let error = format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Quick, On), (Overwrite, Count(2))]).unwrap_err();
        assert!(error.contains("/Q"), "{error}");
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Overwrite, Count(2))]).unwrap(), vec!["F:", "/FS:NTFS", "/P:2", "/Y"]);
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Overwrite, Count(0))]).unwrap(), vec!["F:", "/FS:NTFS", "/P:0", "/Y"]);
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Overwrite, Count(5))]).is_err());
    }

    /// The tool states compression above a 4096-byte allocation unit as unsupported.
    #[test]
    fn ntfs_compression_above_a_4096_byte_allocation_unit_is_refused() {
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Compression, On), (AllocationUnit, Bytes(8192))]).unwrap_err().contains("4096"));
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(Compression, On), (AllocationUnit, Bytes(4096))]).unwrap().contains(&"/C".to_string()));
    }

    /// FAT at most 65 526 clusters, FAT32 from 65 527 to 4 177 917, against this volume's size.
    #[test]
    fn the_cluster_counts_the_tool_stops_at_are_refused_here_first() {
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Fat, &[(AllocationUnit, Bytes(512))]).unwrap_err().contains("65526 clusters"));
        assert!(format_argv(Target { letter: 'A', size: 1_474_560 }, FileSystem::Fat, &[(AllocationUnit, Bytes(512))]).unwrap().contains(&"/A:512".to_string()));
        assert!(format_argv(Target { letter: 'A', size: 1_474_560 }, FileSystem::Fat32, &[(AllocationUnit, Bytes(512))]).unwrap_err().contains("65527"));
        // The band's other side, asked with the largest unit this tool takes: 64 GB at 8192 bytes would be
        // 7 831 836 clusters, over FAT32's ceiling, so the window refuses instead of building a doomed
        // command — which is why no allocation unit this tool accepts can make a volume that size FAT32.
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Fat32, &[(AllocationUnit, Bytes(8 * KB))]).unwrap_err().contains("4177917"));
        assert!(format_argv(Target { letter: 'F', size: 1_000_000_000_000 }, FileSystem::Fat32, &[(AllocationUnit, Bytes(512))]).unwrap_err().contains("4177917"));
    }

    /// UDF duplicates its metadata in 2.50 alone.
    #[test]
    fn duplicated_udf_metadata_needs_the_revision_that_has_it() {
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Udf, &[(UdfRevision, State("2.50")), (UdfDuplicatedMetadata, On)]).unwrap(), vec!["F:", "/FS:UDF", "/R:2.50", "/D", "/Y"]);
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Udf, &[(UdfDuplicatedMetadata, On)]).unwrap_err().contains("2.01"), "the tool's own default revision is the one named");
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Udf, &[(UdfRevision, State("2.60"))]).unwrap_err().contains("2.50"));
    }

    #[test]
    fn the_command_always_asks_with_y_and_never_with_x() {
        let cases: [(FileSystem, Vec<(FormatOption, Value)>); 4] = [(FileSystem::Fat, vec![]), (FileSystem::ExFat, vec![(Quick, On)]), (FileSystem::Ntfs, vec![(LogSize, Count(4096))]), (FileSystem::Udf, vec![(UdfRevision, State("1.02"))])];
        for (fs, chosen) in cases {
            let argv = format_argv(Target { letter: 'F', size: 64_160_400_896 }, fs, &chosen).unwrap();
            assert_eq!(argv.last().unwrap(), "/Y", "{fs:?}: {argv:?}");
            assert!(!argv.iter().any(|a| a.starts_with("/X")), "{fs:?}: {argv:?}");
        }
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(DevDrive, On)]).unwrap_err().contains("NTFS"));
    }

    #[test]
    fn an_option_offered_twice_is_refused() {
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ExFat, &[(Quick, On), (Quick, Off)]).unwrap_err().contains("twice"));
    }

    #[test]
    fn the_log_size_floor_the_tool_names_is_a_refusal_here_too() {
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(LogSize, Count(1024))]).unwrap_err().contains("2 MB"));
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Ntfs, &[(LogSize, Count(2048))]).unwrap().contains(&"/LogSize:2048".to_string()));
    }

    #[test]
    fn the_state_words_are_the_tools_own() {
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ReFs, &[(Integrity, State("disable"))]).unwrap(), vec!["F:", "/FS:ReFS", "/I:disable", "/Y"]);
        assert!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ReFs, &[(Integrity, State("on"))]).unwrap_err().contains("enable"));
    }

    /// The format window and its scenario name a control by these words, so every option needs its own
    /// and no two options may share one.
    #[test]
    fn every_option_names_its_control_in_the_window_alone() {
        let fields: Vec<&str> = FormatOption::all().iter().map(|o| o.field()).collect();
        for field in &fields {
            // A name is words a scenario types whole: lower-case, joined by a dash, and holding the digits
            // of an option whose own word has them ("sha256") — a scenario sets those by name like any other.
            assert!(!field.is_empty() && field.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{field} is a name a scenario can type");
            assert_eq!(fields.iter().filter(|f| *f == field).count(), 1, "{field} names two options");
        }
        assert_eq!(fields.len(), FormatOption::all().len());
    }

    #[test]
    fn the_label_limits_are_the_file_systems_own() {
        assert_eq!(FileSystem::Fat.label_limit(), 11);
        assert_eq!(FileSystem::Fat32.label_limit(), 11);
        assert_eq!(FileSystem::ExFat.label_limit(), 11, "this Windows writes eleven and refuses a twelfth (win32 154), whatever exFAT's own field could hold");
        assert_eq!([FileSystem::Ntfs.label_limit(), FileSystem::ReFs.label_limit(), FileSystem::Udf.label_limit()], [32, 32, 32]);
    }

    /// A name the file system cannot hold is shortened for the volume and kept for the shell.
    #[test]
    fn a_name_too_long_for_the_file_system_gets_a_short_label_and_keeps_its_name() {
        let fat = name_plan(FileSystem::Fat, "Samsung 64 GB").unwrap();
        assert_eq!((fat.label.as_str(), fat.called.as_str(), fat.pinned), ("SAMSUNG 64", "Samsung 64 GB", true));
        let exfat = name_plan(FileSystem::ExFat, "Samsung 64 GB").unwrap();
        assert_eq!((exfat.label.as_str(), exfat.called.as_str(), exfat.pinned), ("Samsung 64", "Samsung 64 GB", true), "eleven is what this Windows honours on exFAT, so the long name is pinned");
        let ntfs = name_plan(FileSystem::Ntfs, "Samsung Flash Drive 64 GB stick here").unwrap();
        assert_eq!((ntfs.label.as_str(), ntfs.pinned), ("Samsung Flash Drive 64 GB stick", true), "31 of the 32 characters, cut between words");
        assert_eq!(name_plan(FileSystem::Ntfs, "").unwrap(), NamePlan { label: String::new(), called: String::new(), pinned: false });
        assert_eq!(name_plan(FileSystem::Fat, "64gb usb").unwrap().label, "64GB USB");
    }

    #[test]
    fn the_short_label_is_cut_at_a_word_boundary() {
        assert_eq!(name_plan(FileSystem::Fat, "SanDisk Ultra USB 3.0").unwrap().label, "SANDISK");
        assert_eq!(name_plan(FileSystem::Fat, "ABCDEFGHIJKLMNOP").unwrap().label, "ABCDEFGHIJK", "one word longer than the limit keeps its head");
        assert_eq!(name_plan(FileSystem::ExFat, "NVMe PC811 SK hynix 1024GB").unwrap().label, "NVMe PC811", "the eleven characters this Windows honours break between words");
        assert_eq!(name_plan(FileSystem::Ntfs, "  spaced    out  ").unwrap().label, "spaced out");
        assert!(!name_plan(FileSystem::Fat, "SAMSUNG 64").unwrap().pinned, "what already fits is typed as it stands");
    }

    #[test]
    fn a_name_no_file_system_can_carry_is_refused_with_the_reason() {
        assert!(name_plan(FileSystem::Fat, "???").unwrap_err().contains('?'));
        assert!(name_plan(FileSystem::Fat, "disk+").unwrap_err().contains('+'));
        assert_eq!(name_plan(FileSystem::Fat, "disk.name").unwrap().label, "DISK.NAME", "the 8.3 label form has one dot in it");
        assert!(name_plan(FileSystem::Ntfs, "a/b").unwrap_err().contains('/'));
        assert!(name_plan(FileSystem::Ntfs, "a\u{7}b").unwrap_err().contains("cannot hold"));
        assert!(name_plan(FileSystem::Ntfs, "disk+").is_ok(), "the 8.3 punctuation is FAT's alone");
        assert!(name_plan(FileSystem::Fat, "").unwrap().label.is_empty());
    }

    /// The command carries the plan's label, so the volume's own name can never exceed what the file
    /// system holds while the window believes it wrote the long one — and it carries it only where the
    /// tool's own switch can take it, which the measured refusal settles for a name holding a space.
    #[test]
    fn the_command_carries_the_plans_label_only_where_the_tool_takes_it() {
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Fat, &[(VolumeName, Text("64gb-usb".into()))]).unwrap(), vec!["F:", "/FS:FAT", "/V:64GB-USB", "/Y"], "a label the plan settles without a space still rides the tool's switch");
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::Fat, &[(VolumeName, Text("Samsung 64 GB".into()))]).unwrap(), vec!["F:", "/FS:FAT", "/Y"], "the plan's SAMSUNG 64 holds a space, which this build's tool refuses as a parameter");
        assert_eq!(format_argv(Target { letter: 'F', size: 64_160_400_896 }, FileSystem::ExFat, &[(VolumeName, Text("   ".into()))]).unwrap(), vec!["F:", "/FS:exFAT", "/Y"]);
    }

    /// The name the tool's switch cannot take is carried to the privileged run instead, settled by the plan
    /// exactly as the command would have carried it, and a name the switch does take is never carried too.
    #[test]
    fn a_name_the_tool_refuses_is_carried_to_the_privileged_run() {
        let spaced = [(VolumeName, Text("Samsung 64 GB".into()))];
        assert_eq!(carried_name(FileSystem::Ntfs, &spaced).unwrap().as_deref(), Some("Samsung 64 GB"), "NTFS holds 32 characters, so the name goes over as typed");
        assert_eq!(carried_name(FileSystem::Fat, &spaced).unwrap().as_deref(), Some("SAMSUNG 64"), "the plan's short variant names the volume while the long one is pinned by the icon files");
        assert_eq!(carried_name(FileSystem::Ntfs, &[(VolumeName, Text("Samsung-64-GB".into()))]).unwrap(), None, "a name the switch takes is not carried as well");
        assert_eq!(carried_name(FileSystem::Ntfs, &[]).unwrap(), None);
        assert!(carried_name(FileSystem::Ntfs, &[(VolumeName, Text("a/b".into()))]).unwrap_err().contains('/'), "a name no plan can carry is refused before anything is asked for");
    }

    #[test]
    fn decimal_size_prints_the_units_the_packaging_prints() {
        assert_eq!(decimal_size(64_160_400_896), "64 GB");
        assert_eq!(decimal_size(1_024_209_543_168), "1 TB");
        assert_eq!(decimal_size(1_000_000_000), "1 GB");
        assert_eq!(decimal_size(1_500_000_000), "1.5 GB");
        assert_eq!(decimal_size(4_700_372_992), "4.7 GB");
        assert_eq!(decimal_size(999_999_999), "1000 MB");
        assert_eq!(decimal_size(1_474_560), "1.5 MB");
        assert_eq!(decimal_size(999), "999 B");
        assert_eq!(decimal_size(0), "0 B");
    }

    /// The data name is what `autorun.inf` carries, and that file is a single-byte one: two of the shown
    /// labels hold a `µ` and a `″` it cannot, and the data names exist for exactly those.
    #[test]
    fn the_data_name_of_every_shape_is_plain_ascii_and_its_own() {
        let names: Vec<&str> = Shape::all().iter().map(|s| s.data_label()).collect();
        assert!(names.iter().all(|n| !n.is_empty() && n.is_ascii()), "a data name no single-byte file can carry: {names:?}");
        for (i, a) in names.iter().enumerate() {
            for b in names.iter().skip(i + 1) { assert_ne!(a, b, "two shapes answer the same data name"); }
        }
        assert_ne!(Shape::MicroSd.label(), Shape::MicroSd.data_label(), "the µSD card's shown label is what the file gets");
        assert_ne!(Shape::Floppy525.label(), Shape::Floppy525.data_label(), "the 5.25 inch floppy's shown label is what the file gets");
    }

    /// The model strings measured on this machine are the fixtures here.
    #[test]
    fn vendor_token_names_the_brand_the_string_gives() {
        assert_eq!(vendor_token("Samsung Flash Drive"), "Samsung");
        assert_eq!(vendor_token("NVMe PC811 SK hynix 1024GB"), "Hynix");
        assert_eq!(vendor_token("SanDisk Ultra USB 3.0"), "SanDisk");
        assert_eq!(vendor_token("WDC WD20EFRX-68JPRT0"), "WDC");
        assert_eq!(vendor_token("ST1000DM003-1SB102"), "ST", "a bare model number answers its own letters");
        assert_eq!(vendor_token("TOSHIBA MQ01ABD050"), "TOSHIBA");
        assert_eq!(vendor_token("Generic- SD Card Reader"), "", "a reader names no card, so the icon prints the size alone");
        assert_eq!(vendor_token(""), "");
    }

    #[test]
    fn the_proposed_name_is_the_brand_and_the_size() {
        assert_eq!(proposed_name("Samsung Flash Drive", 64_160_400_896), "Samsung 64 GB");
        assert_eq!(proposed_name("", 64_160_400_896), "64 GB");
        let proposed = name_plan(FileSystem::ExFat, &proposed_name("Samsung Flash Drive", 64_160_400_896)).unwrap();
        assert_eq!((proposed.label.as_str(), proposed.pinned), ("Samsung 64", true), "this Windows holds eleven on exFAT, so the proposed name is shortened and pinned");
    }

    #[test]
    fn the_shape_is_what_the_machine_describes() {
        let volume = |bus: &str, rotation: i32, removable: bool, size: u64| Volume { bus: bus.into(), rotation_rate: rotation, removable, drive_size: size, ..Default::default() };
        assert_eq!(Shape::detected(&volume("nvme", -1, false, 1_024_209_543_168)), Some(Shape::Nvme));
        assert_eq!(Shape::detected(&volume("ata", 0, false, 512_110_190_592)), Some(Shape::Ssd));
        assert_eq!(Shape::detected(&volume("sata", 7200, false, 4_000_787_030_016)), Some(Shape::Hdd));
        assert_eq!(Shape::detected(&volume("usb", -1, true, 64_160_400_896)), Some(Shape::UsbStick));
        assert_eq!(Shape::detected(&volume("sdio", -1, true, 64_160_400_896)), Some(Shape::SdCard));
        assert_eq!(Shape::detected(&Volume { optical: true, drive_size: 64_160_400_896, ..Default::default() }), Some(Shape::Optical));
        assert_eq!(Shape::detected(&volume("usb", -1, true, 1_474_560)), Some(Shape::Floppy35));
        assert_eq!(Shape::detected(&volume("usb", -1, true, 1_228_800)), Some(Shape::Floppy525));
        assert_eq!(Shape::detected(&volume("", -1, false, 0)), None);
        assert!(Shape::all().iter().all(|s| !s.asset().is_empty() && !s.label().is_empty()));
    }

    /// A window hands the shape back by the name its picture's file carries, so that name has to name the
    /// shape again — and a word that is no asset answers nothing rather than a guess.
    #[test]
    fn the_asset_name_of_a_shape_names_that_shape_again() {
        for shape in Shape::all() {
            assert_eq!(Shape::parse(shape.asset()), Some(*shape), "{} names an asset that answers no shape", shape.label());
        }
        assert_eq!(Shape::parse("floppy8"), None, "a word naming no asset answers a shape anyway");
        assert_eq!(Shape::parse(""), None, "no word at all answers a shape");
    }

    #[test]
    fn the_evidence_spells_out_what_was_seen_and_where_it_cannot_tell() {
        let card = Volume { bus: "sdio".into(), removable: true, drive_size: 64_160_400_896, ..Default::default() };
        let evidence = Shape::evidence(&card);
        assert!(evidence.contains("bus sdio") && evidence.contains("64 GB") && evidence.contains("µSD"), "{evidence}");
        let unknown = Volume { rotation_rate: -1, drive_size: 512_110_190_592, ..Default::default() };
        let evidence = Shape::evidence(&unknown);
        assert!(evidence.contains("no connection named") && evidence.contains("512 GB") && evidence.contains("choose the shape"), "{evidence}");
        assert!(Shape::evidence(&Volume { rotation_rate: 0, drive_size: 512_110_190_592, ..Default::default() }).contains("no seek penalty"));
    }

    #[test]
    fn a_volume_that_cannot_be_formatted_is_refused_with_a_reason() {
        assert!(refusal(&Volume { optical: true, drive_size: 1_000_000_000, ..Default::default() }).unwrap().contains("optical"));
        #[cfg(not(windows))]
        assert!(refusal(&Volume { system: true, drive_size: 1_000_000_000, ..Default::default() }).unwrap().contains("system"));
        assert!(refusal(&Volume { read_only: true, drive_size: 1_000_000_000, ..Default::default() }).unwrap().contains("read-only"));
        assert!(refusal(&Volume { share_uri: "smb://nas/Music".into(), drive_size: 1_000_000_000, ..Default::default() }).unwrap().contains("another computer"));
        // Measured on this machine: two mapped drives answer the machine's own remote-drive fact while the
        // lookup of their origin answers nothing, so the refusal cannot rest on the origin alone.
        assert!(refusal(&Volume { network: true, drive_size: 15_698_757_677_056, ..Default::default() }).unwrap().contains("another computer"));
        assert!(refusal(&Volume { protocol: "mtp".into(), drive_size: 1_000_000_000, ..Default::default() }).unwrap().contains("phone"));
        assert_eq!(refusal(&Volume { drive_size: 64_160_400_896, bus: "usb".into(), ..Default::default() }), None);
    }

    #[test]
    fn a_file_system_is_offered_only_where_this_volumes_size_fits_its_cluster_band() {
        let offered: Vec<String> = FileSystem::all().iter().map(|fs| fs.word().to_string()).collect();
        let names: Vec<&str> = accepted(&Volume { drive_size: 64_160_400_896, ..Default::default() }, &offered).iter().map(|fs| fs.word()).collect();
        assert_eq!(names, ["exFAT", "NTFS", "ReFS", "UDF"], "FAT cannot hold 64 GB inside 65 526 clusters, and at any allocation unit this tool takes FAT32 needs more than its 4 177 917 of them");
        let names: Vec<&str> = accepted(&Volume { drive_size: 1_474_560, ..Default::default() }, &offered).iter().map(|fs| fs.word()).collect();
        assert!(names.contains(&"FAT") && !names.contains(&"FAT32"), "FAT32 needs more clusters than a floppy has: {names:?}");
        assert!(accepted(&Volume { drive_size: 1_000, ..Default::default() }, &offered).is_empty(), "a volume with no usable size offers nothing");
        assert!(accepted(&Volume { drive_size: 64_160_400_896, optical: true, ..Default::default() }, &offered).is_empty(), "a volume that is refused outright is offered nothing");
        #[cfg(not(windows))]
        assert!(accepted(&Volume { drive_size: 64_160_400_896, system: true, ..Default::default() }, &offered).is_empty());
        let names: Vec<&str> = accepted(&Volume { drive_size: 64_160_400_896, ..Default::default() }, &["NTFS".into()]).iter().map(|fs| fs.word()).collect();
        assert_eq!(names, ["NTFS"], "what the machine named is what is offered");
    }

    #[test]
    fn only_the_file_systems_named_in_the_tools_own_explanation_are_offered() {
        // The usage text as this machine's tool answered it on 2026-11-18, its own line break kept: the
        // six words land in two lines, and the words around them are German.
        let here = "FORMAT Volume [/FS:Dateisystem] [/V:Bezeichnung] [/Q]\r\n\r\n  /FS:filesystem  Gibt den Typ des Dateisystems an (FAT, FAT32, exFAT,\r\n                  NTFS, UDF oder ReFS).\r\n  /V:Bezeichnung    Erstellt die Volumebezeichnung.\r\n";
        let names: Vec<&str> = named_by_tool(here).iter().map(|fs| fs.word()).collect();
        assert_eq!(names, ["FAT", "FAT32", "exFAT", "NTFS", "ReFS", "UDF"], "the table's order, in whatever order the tool names them");
        // A tool that explains fewer file systems offers fewer, in the table's order either way.
        let fewer = "FORMAT Volume [/FS:Dateisystem] [/A:Groesse]\r\n  /A:Groesse        Die Groesse der Zuordnungseinheit.\r\n  /FS:filesystem  Gibt den Typ an (UDF, exFAT oder FAT32).\r\n  /Q                Ueberspringt die vollstaendige Ueberpruefung.\r\n";
        let names: Vec<&str> = named_by_tool(fewer).iter().map(|fs| fs.word()).collect();
        assert_eq!(names, ["FAT32", "exFAT", "UDF"], "the paragraph explains them, the synopsis line names none");
        // A file system named only inside another option's sentence is not one this machine formats to.
        let narrower = "FORMAT Volume [/FS:Dateisystem] [/C]\r\n  /FS:filesystem  Gibt den Typ an (FAT oder FAT32).\r\n  /C                Nur NTFS: Dateien werden komprimiert.\r\n";
        let names: Vec<&str> = named_by_tool(narrower).iter().map(|fs| fs.word()).collect();
        assert_eq!(names, ["FAT", "FAT32"], "NTFS is named in the compression option's sentence, not as a format");
        assert!(named_by_tool("FORMAT Volume [/FS:Dateisystem] [/Q]\r\n").is_empty(), "a synopsis line with no explanation names nothing");
        assert!(named_by_tool("Die Datei existiert bereits.").is_empty(), "an answer that is no usage text at all names nothing");
    }

    #[test]
    fn the_offer_of_a_volume_names_the_words_a_window_shows() {
        assert!(offer(&Volume { drive_size: 64_160_400_896, optical: true, ..Default::default() }).is_empty(), "a volume the gate refuses is offered nothing");
        let names = offer(&Volume { device: "F:".into(), size: 64_160_400_896, bus: "usb".into(), ..Default::default() });
        assert!(!names.is_empty(), "a volume the machine names a size for is offered something");
        for word in &names {
            assert!(FileSystem::parse(word).is_some(), "{word} is not a word of the table");
        }
        assert!(!names.contains(&"FAT".into()), "FAT cannot hold 64 GB inside its cluster band, whatever the machine offers");
    }

    /// What the installed tool actually answers, read here rather than assumed: whatever it says, the
    /// offer stays inside this table, and the same read answers the same way twice.
    #[test]
    #[cfg(windows)]
    fn the_installed_tool_s_answer_stays_inside_the_table() {
        let answer = crate::vcs::run::ask("format.com", std::path::Path::new("."), &["/?"]);
        eprintln!("format /? answered ok={} code {:?} in {} bytes, first line {:?}", answer.ok, answer.code, answer.text.len(), answer.text.lines().next().unwrap_or_default());
        let named = tool_file_systems();
        eprintln!("this build's format tool names: {:?}", named.iter().map(|fs| fs.word()).collect::<Vec<_>>());
        assert!(!named.is_empty(), "the installed tool's own text is expected to name at least one file system");
        for fs in &named {
            assert!(FileSystem::all().contains(fs), "{} is not in the table", fs.word());
        }
        assert_eq!(named, named_by_tool(&tool_usage_text()), "the tool answers the same way when asked twice");
    }

    /// The volume the gate refuses as the system's is the one the machine says Windows runs from, which is
    /// not the flag the drives page uses to hide an unmount button: measured on this machine, three lettered
    /// fixed volumes answer that flag and only one of them holds Windows.
    #[test]
    #[cfg(windows)]
    fn the_gate_refuses_the_volume_windows_runs_from_and_not_a_data_volume() {
        let Some(system) = crate::win::storage::system_volume_letter() else { panic!("the machine is expected to answer where Windows runs from") };
        let holds = Volume { device: format!("{system}:"), drive_size: 1_000_000_000, ..Default::default() };
        assert_eq!(refusal(&holds).as_deref(), Some("this is the volume Windows runs from"), "the volume Windows runs from has to be refused before any command exists");
        let other = if system == 'd' { "E:" } else { "D:" };
        let data = Volume { device: other.into(), drive_size: 1_000_000_000, system: true, ..Default::default() };
        assert_eq!(refusal(&data), None, "a data volume is not refused as the system's, whatever the drives page's flag says about it");
    }

    #[test]
    fn every_file_system_word_round_trips_as_itself() {
        for fs in FileSystem::all() {
            assert_eq!(FileSystem::parse(fs.word()), Some(*fs), "{}", fs.word());
        }
        for (word, fs) in [("EXFAT", FileSystem::ExFat), ("ntfs", FileSystem::Ntfs), ("FAT16", FileSystem::Fat), ("FAT12", FileSystem::Fat), ("ReFs", FileSystem::ReFs), (" UDF ", FileSystem::Udf)] {
            assert_eq!(FileSystem::parse(word), Some(fs), "{word}");
        }
        assert_eq!(FileSystem::parse("ext4"), None);
        assert_eq!(FileSystem::parse(""), None);
    }

    #[test]
    fn the_target_is_the_letters_the_machine_names() {
        assert_eq!(Target::of(&Volume { device: "C:".into(), drive_size: 1, ..Default::default() }), Some(Target { letter: 'c', size: 1 }));
        assert_eq!(Target::of(&Volume { device: "\\\\?\\Volume{8953164f-98f3-40d0-8dcc-76ce2e1f6fc0}\\".into(), mount_points: vec![std::path::PathBuf::from("D:\\data")], drive_size: 1, ..Default::default() }), Some(Target { letter: 'd', size: 1 }));
        assert_eq!(Target::of(&Volume { device: "/dev/sda1".into(), drive_size: 1, ..Default::default() }), None);
        assert!(holds_our_own_image(&Volume { device: "C:".into(), drive_size: 1, ..Default::default() }, std::path::Path::new("C:\\Programs\\failbrauwser\\bin\\failbrauwser.exe")));
        assert!(!holds_our_own_image(&Volume { device: "D:".into(), drive_size: 1, ..Default::default() }, std::path::Path::new("C:\\Programs\\failbrauwser\\bin\\failbrauwser.exe")));
    }
}
