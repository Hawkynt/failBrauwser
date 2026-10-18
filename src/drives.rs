//! Drives and volumes: from UDisks2 (what `udisksctl` and every desktop use) or, without it,
//! from the kernel's mount table. Plus fill levels via `statvfs`.

use gtk::glib;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Volume {
    /// `/dev/sda1`
    pub device: String,
    pub label: String,
    /// ext4, vfat, ntfs, crypto_LUKS, …
    pub fs_type: String,
    pub size: u64,
    pub mount_points: Vec<PathBuf>,
    /// Model of the drive it is on ("SanDisk Ultra").
    pub drive_name: String,
    pub removable: bool,
    pub usb: bool,
    pub optical: bool,
    pub ejectable: bool,
    /// UDisks2 object paths, for mounting and ejecting.
    pub block_object: String,
    pub drive_object: String,
    /// Has a filesystem UDisks2 can mount.
    pub mountable: bool,
    /// A system volume (root, boot, …) as UDisks2 sees it.
    pub system: bool,
    /// An encrypted container that is unlocked: its contents are a volume of their own.
    pub unlocked: bool,
    pub uuid: String,
    /// File system version ("1.0" for ext4, "FAT32", …).
    pub fs_version: String,
    pub read_only: bool,
    pub partition_number: u32,
    pub partition_name: String,
    /// Partition type (GPT GUID or MBR code).
    pub partition_type: String,
    pub serial: String,
    pub revision: String,
    /// Connection: "usb", "sdio", "ieee1394", or empty for internal (SATA/NVMe).
    pub bus: String,
    /// -1 unknown, 0 solid state, otherwise spindle speed.
    pub rotation_rate: i32,
    pub drive_size: u64,
    /// A phone or camera reached through GVfs ("mtp", "gphoto2", "afc"), empty for a block
    /// device. `device` then holds its address, `drive_name` its name and `label` the
    /// storage ("Internal shared storage").
    pub protocol: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub total: u64,
    pub used: u64,
    pub free: u64,
}

impl Usage {
    pub fn fraction(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.used as f64 / self.total as f64 }
    }
}

impl Volume {
    /// What to call it: label, else the folder it is mounted on (unless that is just a
    /// stand-in under `/run/media`), else drive model plus size, else device.
    pub fn title(&self) -> String {
        if self.is_device() {
            return if self.drive_name.is_empty() { "Phone".into() } else { self.drive_name.clone() };
        }
        if !self.label.is_empty() {
            return self.label.clone();
        }
        if self.mount_points.iter().any(|m| m == Path::new("/")) {
            return "File System".into();
        }
        if let Some(m) = self.mount_point() {
            if m == Path::new("/home") {
                return "Home".into();
            }
            let automounted = m.starts_with("/run/media") || m.starts_with("/media");
            if let Some(name) = m.file_name().filter(|_| !automounted) {
                return name.to_string_lossy().into_owned();
            }
        }
        if self.size > 0 {
            let size = crate::fs::format::human_size(self.size);
            if !self.drive_name.is_empty() {
                return format!("{size} {}", self.drive_name);
            }
            return format!("{size} Volume");
        }
        self.device.clone()
    }

    pub fn icon_names(&self) -> Vec<&'static str> {
        if let Some(kind) = self.device_kind() {
            kind.icon_names()
        } else if self.optical {
            vec!["drive-optical", "media-optical", "drive-harddisk"]
        } else if self.usb {
            vec!["drive-removable-media-usb", "drive-removable-media", "drive-harddisk-usb", "drive-harddisk"]
        } else if self.removable {
            vec!["drive-removable-media", "media-flash", "drive-harddisk"]
        } else {
            vec!["drive-harddisk", "drive-harddisk-solidstate"]
        }
    }

    pub fn mount_point(&self) -> Option<&PathBuf> {
        self.mount_points.first()
    }

    /// A phone or camera rather than a block device.
    pub fn is_device(&self) -> bool {
        !self.protocol.is_empty()
    }

    pub fn device_kind(&self) -> Option<crate::devices::DeviceKind> {
        crate::devices::DeviceKind::from_protocol(&self.protocol)
    }
}

/// Mount points of the system itself rather than of the user's files (and everything below
/// them: `/boot/efi`).
const SYSTEM_MOUNTS: &[&str] = &["/boot", "/efi", "/usr", "/var", "/opt", "/srv", "/tmp", "/root", "/nix", "/gnu", "/snap", "/recovery", "/swap"];

pub fn is_system_mount(m: &Path) -> bool {
    SYSTEM_MOUNTS.iter().any(|s| m.starts_with(s))
}

/// Partition types that never hold the user's files: EFI, BIOS boot, extended boot,
/// Microsoft reserved, Windows recovery, swap, extended partitions.
fn is_system_partition(t: &str) -> bool {
    matches!(
        t.to_ascii_lowercase().as_str(),
        "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
            | "0xef"
            | "21686148-6449-6e6f-744e-656564454649"
            | "bc13c2ff-59e6-4262-a352-b275fd6f7172"
            | "e3c9e316-0b5c-4db8-817d-f92df00215ae"
            | "de94bba4-06d1-4d40-a16a-bfd50179d6ac"
            | "0x27"
            | "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f"
            | "0x82"
            | "0x05"
            | "0x0f"
    )
}

/// The volumes that hold the user's files, for the side panel: without the system's own
/// (`/boot`, EFI, swap…), without `/` when the home folder is on a volume of its own, and
/// without encrypted containers that are unlocked (their contents are listed). Phones stay.
pub fn user_volumes(all: &[Volume], home: &Path) -> Vec<Volume> {
    let root = Path::new("/");
    let home_elsewhere = all.iter().flat_map(|v| &v.mount_points).any(|m| m != root && !is_system_mount(m) && home.starts_with(m));
    all.iter()
        .filter(|v| {
            if v.is_device() {
                return true;
            }
            if v.unlocked {
                return false;
            }
            if v.mount_points.is_empty() {
                return !is_system_partition(&v.partition_type);
            }
            v.mount_points.iter().any(|m| if m == root { !home_elsewhere } else { !is_system_mount(m) })
        })
        .map(|v| {
            let mut v = v.clone();
            if !v.is_device() {
                v.mount_points.retain(|m| if m == root { !home_elsewhere } else { !is_system_mount(m) });
            }
            v
        })
        .collect()
}

/// Fill level of a mounted filesystem.
pub fn usage(mount_point: &Path) -> Option<Usage> {
    let c = CString::new(mount_point.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let f = st.f_frsize as u64;
    let total = st.f_blocks as u64 * f;
    Some(Usage { total, used: (st.f_blocks as u64 - st.f_bfree as u64) * f, free: st.f_bavail as u64 * f })
}

fn bytes_to_string(v: &glib::Variant) -> String {
    // UDisks2 byte strings are NUL terminated.
    let bytes: Vec<u8> = v.fixed_array::<u8>().map(|b| b.to_vec()).unwrap_or_default();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

type Props = HashMap<String, glib::Variant>;

fn props(v: &glib::Variant) -> Props {
    let mut m = HashMap::new();
    for i in 0..v.n_children() {
        let entry = v.child_value(i);
        let key: String = entry.child_value(0).get().unwrap_or_default();
        let val = entry.child_value(1);
        let val = val.as_variant().unwrap_or(val);
        m.insert(key, val);
    }
    m
}

fn s(p: &Props, k: &str) -> String {
    p.get(k).and_then(|v| v.get::<String>()).unwrap_or_default()
}

fn b(p: &Props, k: &str) -> bool {
    p.get(k).and_then(|v| v.get::<bool>()).unwrap_or(false)
}

fn u(p: &Props, k: &str) -> u64 {
    p.get(k).and_then(|v| v.get::<u64>()).unwrap_or(0)
}

fn i(p: &Props, k: &str) -> i32 {
    p.get(k).and_then(|v| v.get::<i32>()).unwrap_or(-1)
}

fn n(p: &Props, k: &str) -> u32 {
    p.get(k).and_then(|v| v.get::<u32>()).unwrap_or(0)
}

fn o(p: &Props, k: &str) -> String {
    p.get(k).and_then(|v| v.str().map(str::to_string)).unwrap_or_default()
}

/// Reads the answer of UDisks2's `GetManagedObjects` (`(a{oa{sa{sv}}})`).
pub fn parse_udisks(reply: &glib::Variant) -> Vec<Volume> {
    let objects = if reply.type_().as_str().starts_with('(') { reply.child_value(0) } else { reply.clone() };
    // object path → interface name → properties
    let mut all: HashMap<String, HashMap<String, Props>> = HashMap::new();
    for i in 0..objects.n_children() {
        let entry = objects.child_value(i);
        let path = entry.child_value(0).str().unwrap_or_default().to_string();
        let ifaces = entry.child_value(1);
        let mut map = HashMap::new();
        for j in 0..ifaces.n_children() {
            let ie = ifaces.child_value(j);
            let name: String = ie.child_value(0).get().unwrap_or_default();
            map.insert(name, props(&ie.child_value(1)));
        }
        all.insert(path, map);
    }
    let mut out = Vec::new();
    for (path, ifaces) in &all {
        let Some(block) = ifaces.get("org.freedesktop.UDisks2.Block") else { continue };
        let usage = s(block, "IdUsage");
        let fs = ifaces.get("org.freedesktop.UDisks2.Filesystem");
        if usage != "filesystem" && usage != "crypto" {
            continue;
        }
        let mount_points: Vec<PathBuf> = fs
            .and_then(|f| f.get("MountPoints"))
            .map(|mp| (0..mp.n_children()).map(|i| PathBuf::from(bytes_to_string(&mp.child_value(i)))).filter(|p| !p.as_os_str().is_empty()).collect())
            .unwrap_or_default();
        // Hidden by policy (EFI partitions, recovery…): still shown while mounted.
        if b(block, "HintIgnore") && mount_points.is_empty() {
            continue;
        }
        let drive_object = o(block, "Drive");
        let drive = all.get(&drive_object).and_then(|d| d.get("org.freedesktop.UDisks2.Drive"));
        let empty = Props::new();
        let d = drive.unwrap_or(&empty);
        let part = ifaces.get("org.freedesktop.UDisks2.Partition").unwrap_or(&empty);
        let cleartext = ifaces.get("org.freedesktop.UDisks2.Encrypted").map(|e| o(e, "CleartextDevice")).unwrap_or_default();
        let (drive_name, removable, usb, optical, ejectable) = match drive {
            Some(d) => {
                let name = [s(d, "Vendor"), s(d, "Model")].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" ");
                let media: String = s(d, "Media");
                (name, b(d, "Removable") || b(d, "MediaRemovable"), s(d, "ConnectionBus") == "usb", b(d, "Optical") || media.starts_with("optical"), b(d, "Ejectable"))
            }
            None => (String::new(), false, false, false, false),
        };
        // Loop devices only matter while something is mounted from them.
        let device = block.get("PreferredDevice").or_else(|| block.get("Device")).map(bytes_to_string).unwrap_or_default();
        if device.starts_with("/dev/loop") && mount_points.is_empty() {
            continue;
        }
        out.push(Volume {
            device,
            label: s(block, "IdLabel"),
            fs_type: s(block, "IdType"),
            size: u(block, "Size"),
            mount_points,
            drive_name,
            removable,
            usb,
            optical,
            ejectable,
            block_object: path.clone(),
            drive_object,
            mountable: fs.is_some(),
            system: b(block, "HintSystem"),
            unlocked: !cleartext.is_empty() && cleartext != "/",
            uuid: s(block, "IdUUID"),
            fs_version: s(block, "IdVersion"),
            read_only: b(block, "ReadOnly"),
            partition_number: n(part, "Number"),
            partition_name: s(part, "Name"),
            partition_type: s(part, "Type"),
            serial: s(d, "Serial"),
            revision: s(d, "Revision"),
            bus: s(d, "ConnectionBus"),
            rotation_rate: if drive.is_some() { i(d, "RotationRate") } else { -1 },
            drive_size: u(d, "Size"),
            protocol: String::new(),
        });
    }
    sort_volumes(&mut out);
    out
}

/// Fixed first (root first), then removable; by device name within.
pub fn sort_volumes(v: &mut [Volume]) {
    v.sort_by(|a, b| {
        let root = |x: &Volume| !x.mount_points.iter().any(|m| m == Path::new("/"));
        (root(a), a.removable || a.usb, &a.device).cmp(&(root(b), b.removable || b.usb, &b.device))
    });
}

/// Unescapes the octal escapes of /proc/self/mountinfo ("\040" is a space).
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push(((b[i + 1] - b'0') << 6) | ((b[i + 2] - b'0') << 3) | (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Volumes from the mount table (without UDisks2): real block devices only.
pub fn parse_mountinfo(text: &str) -> Vec<Volume> {
    let mut by_dev: HashMap<String, Volume> = HashMap::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else { continue };
        let l: Vec<&str> = left.split(' ').collect();
        let r: Vec<&str> = right.split(' ').collect();
        if l.len() < 5 || r.len() < 2 {
            continue;
        }
        let (mount, fs_type, source) = (unescape_mount(l[4]), r[0], unescape_mount(r[1]));
        if !source.starts_with("/dev/") || source.starts_with("/dev/loop") {
            continue;
        }
        let v = by_dev.entry(source.clone()).or_insert_with(|| Volume {
            device: source.clone(),
            fs_type: fs_type.to_string(),
            mountable: true,
            rotation_rate: -1,
            ..Default::default()
        });
        v.mount_points.push(PathBuf::from(mount));
    }
    let mut v: Vec<Volume> = by_dev.into_values().collect();
    for vol in &mut v {
        vol.mount_points.sort_by_key(|p| p.as_os_str().len());
    }
    sort_volumes(&mut v);
    v
}

/// Mount options of a mount point, as the kernel reports them ("rw,relatime,…").
pub fn mount_options_in(text: &str, mount_point: &Path) -> Option<String> {
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else { continue };
        let l: Vec<&str> = left.split(' ').collect();
        let r: Vec<&str> = right.split(' ').collect();
        if l.len() < 6 || r.len() < 3 || Path::new(&unescape_mount(l[4])) != mount_point {
            continue;
        }
        let mut opts: Vec<&str> = l[5].split(',').collect();
        for o in r[2].split(',') {
            if !opts.contains(&o) {
                opts.push(o);
            }
        }
        return Some(opts.join(","));
    }
    None
}

pub fn mount_options(mount_point: &Path) -> Option<String> {
    std::fs::read_to_string("/proc/self/mountinfo").ok().and_then(|t| mount_options_in(&t, mount_point))
}

/// A readable name for a GPT type GUID or MBR type code, the raw value in brackets.
pub fn partition_type_name(t: &str) -> String {
    let name = match t.to_ascii_lowercase().as_str() {
        "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" | "0xef" => "EFI system partition",
        "0fc63daf-8483-4772-8e79-3d69d8477de4" | "0x83" => "Linux file system",
        "4f68bce3-e8cd-4db1-96e7-fbcaf984b709" => "Linux root (x86-64)",
        "b921b045-1df0-41c3-af44-4c6f280d3fae" => "Linux root (ARM64)",
        "933ac7e1-2eb4-4f13-b844-0e14e2aef915" => "Linux home",
        "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f" | "0x82" => "Linux swap",
        "e6d6d379-f507-44c2-a23c-238f2a3df928" | "0x8e" => "Linux LVM",
        "a19d880f-05fc-4d3b-a006-743f0f84911e" | "0xfd" => "Linux RAID",
        "ca7d7ccb-63ed-4c53-861c-1742536059cc" => "Linux LUKS",
        "bc13c2ff-59e6-4262-a352-b275fd6f7172" => "Linux extended boot",
        "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7" => "Basic data (Windows)",
        "e3c9e316-0b5c-4db8-817d-f92df00215ae" => "Microsoft reserved",
        "de94bba4-06d1-4d40-a16a-bfd50179d6ac" => "Windows recovery",
        "21686148-6449-6e6f-744e-656564454649" => "BIOS boot",
        "0x07" => "NTFS / exFAT",
        "0x0b" | "0x0c" => "FAT32",
        "0x0e" | "0x06" => "FAT16",
        "0x05" | "0x0f" => "Extended partition",
        _ => return t.to_string(),
    };
    format!("{name} ({t})")
}

/// What kind of drive, in words: "USB drive", "SSD", "Hard disk (7200 rpm)", …
pub fn drive_kind(v: &Volume) -> String {
    if let Some(kind) = v.device_kind() {
        kind.describe().into()
    } else if v.optical {
        "Optical drive".into()
    } else if v.usb {
        "USB drive".into()
    } else if v.bus == "sdio" {
        "Memory card".into()
    } else if v.removable {
        "Removable drive".into()
    } else if v.rotation_rate == 0 || v.device.starts_with("/dev/nvme") {
        "Solid state drive".into()
    } else if v.rotation_rate > 0 {
        format!("Hard disk ({} rpm)", v.rotation_rate)
    } else {
        String::new()
    }
}

pub fn read_mountinfo() -> Vec<Volume> {
    std::fs::read_to_string("/proc/self/mountinfo").map(|t| parse_mountinfo(&t)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_real_devices_only() {
        let text = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
23 22 259:1 / /boot rw - vfat /dev/nvme0n1p1 rw
24 22 0:21 / /proc rw - proc proc rw
25 22 8:17 / /run/media/me/My\\040Stick rw - exfat /dev/sdb1 rw
26 22 7:0 / /snap/x rw - squashfs /dev/loop0 ro
27 22 259:2 /home /bind rw - ext4 /dev/nvme0n1p2 rw
";
        let v = parse_mountinfo(text);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].device, "/dev/nvme0n1p2");
        assert_eq!(v[0].mount_points, vec![PathBuf::from("/"), PathBuf::from("/bind")]);
        assert!(v.iter().any(|x| x.mount_points == vec![PathBuf::from("/run/media/me/My Stick")]));
    }

    #[test]
    fn udisks_reply_is_parsed() {
        let text = "({objectpath '/org/freedesktop/UDisks2/block_devices/sdb1': {'org.freedesktop.UDisks2.Block': {'Device': <b'/dev/sdb1'>, 'IdLabel': <'STICK'>, 'IdType': <'vfat'>, 'IdUsage': <'filesystem'>, 'Size': <uint64 8000000000>, 'HintIgnore': <false>, 'HintSystem': <false>, 'Drive': <objectpath '/org/freedesktop/UDisks2/drives/SanDisk'>}, 'org.freedesktop.UDisks2.Filesystem': {'MountPoints': <[b'/run/media/me/STICK']>}}, objectpath '/org/freedesktop/UDisks2/drives/SanDisk': {'org.freedesktop.UDisks2.Drive': {'Vendor': <'SanDisk'>, 'Model': <'Ultra'>, 'Removable': <true>, 'ConnectionBus': <'usb'>, 'Ejectable': <true>, 'Optical': <false>, 'Media': <''>}}, objectpath '/org/freedesktop/UDisks2/block_devices/sda': {'org.freedesktop.UDisks2.Block': {'Device': <b'/dev/sda'>, 'IdUsage': <''>, 'HintIgnore': <false>}}, objectpath '/org/freedesktop/UDisks2/block_devices/loop0': {'org.freedesktop.UDisks2.Block': {'Device': <b'/dev/loop0'>, 'IdUsage': <'filesystem'>, 'HintIgnore': <false>}, 'org.freedesktop.UDisks2.Filesystem': {'MountPoints': <@aay []>}}},)";
        let v = glib::Variant::parse(Some(glib::VariantTy::new("(a{oa{sa{sv}}})").unwrap()), text).unwrap();
        let vols = parse_udisks(&v);
        assert_eq!(vols.len(), 1);
        let s = &vols[0];
        assert_eq!(s.device, "/dev/sdb1");
        assert_eq!(s.title(), "STICK");
        assert_eq!(s.mount_points, vec![PathBuf::from("/run/media/me/STICK")]);
        assert!(s.usb && s.removable && s.ejectable && s.mountable);
        assert_eq!(s.drive_name, "SanDisk Ultra");
        assert_eq!(s.icon_names()[0], "drive-removable-media-usb");
        assert_eq!(drive_kind(s), "USB drive");
    }

    #[test]
    fn udisks_details_are_read() {
        let text = "({objectpath '/o/b/nvme0n1p2': {'org.freedesktop.UDisks2.Block': {'Device': <b'/dev/nvme0n1p2'>, 'IdUsage': <'filesystem'>, 'IdType': <'ext4'>, 'IdVersion': <'1.0'>, 'IdUUID': <'abcd-1234'>, 'ReadOnly': <false>, 'HintIgnore': <false>, 'Drive': <objectpath '/o/d/ssd'>}, 'org.freedesktop.UDisks2.Filesystem': {'MountPoints': <[b'/']>}, 'org.freedesktop.UDisks2.Partition': {'Number': <uint32 2>, 'Name': <'root'>, 'Type': <'0fc63daf-8483-4772-8e79-3d69d8477de4'>}}, objectpath '/o/d/ssd': {'org.freedesktop.UDisks2.Drive': {'Model': <'Samsung SSD'>, 'Serial': <'S4GV'>, 'Revision': <'1B4Q'>, 'ConnectionBus': <''>, 'RotationRate': <0>, 'Size': <uint64 256060514304>}}},)";
        let v = glib::Variant::parse(Some(glib::VariantTy::new("(a{oa{sa{sv}}})").unwrap()), text).unwrap();
        let vol = &parse_udisks(&v)[0];
        assert_eq!((vol.uuid.as_str(), vol.fs_version.as_str()), ("abcd-1234", "1.0"));
        assert_eq!((vol.partition_number, vol.partition_name.as_str()), (2, "root"));
        assert_eq!((vol.serial.as_str(), vol.revision.as_str()), ("S4GV", "1B4Q"));
        assert_eq!(vol.drive_size, 256060514304);
        assert_eq!(drive_kind(vol), "Solid state drive");
    }

    #[test]
    fn mount_options_merge_both_lists() {
        let text = "22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro\n25 22 8:17 / /run/media/me/My\\040Stick rw,nosuid,nodev - exfat /dev/sdb1 rw,uid=1000\n".replace("\\n", "\n");
        assert_eq!(mount_options_in(&text, Path::new("/")).as_deref(), Some("rw,relatime,errors=remount-ro"));
        assert_eq!(mount_options_in(&text, Path::new("/run/media/me/My Stick")).as_deref(), Some("rw,nosuid,nodev,uid=1000"));
        assert!(mount_options_in(&text, Path::new("/nope")).is_none());
        assert_eq!(partition_type_name("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"), "EFI system partition (C12A7328-F81F-11D2-BA4B-00A0C93EC93B)");
        assert_eq!(partition_type_name("0x83"), "Linux file system (0x83)");
        assert_eq!(partition_type_name("unknown"), "unknown");
        let hdd = Volume { rotation_rate: 7200, ..Default::default() };
        assert_eq!(drive_kind(&hdd), "Hard disk (7200 rpm)");
    }

    #[test]
    fn titles_and_usage() {
        let mut v = Volume { device: "/dev/sdc1".into(), size: 32_000_000_000, drive_name: "Kingston".into(), ..Default::default() };
        assert_eq!(v.title(), "32.0 GB Kingston");
        v.mount_points = vec!["/".into()];
        assert_eq!(v.title(), "File System");
        let u = usage(Path::new("/")).unwrap();
        assert!(u.total > 0 && u.used <= u.total);
        assert!((0.0..=1.0).contains(&u.fraction()));
        assert!(usage(Path::new("/definitely/not/mounted")).is_none());
    }
}
