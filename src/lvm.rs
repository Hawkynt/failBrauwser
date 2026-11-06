//! LVM volumes inside an encrypted drive (a whole Linux installation on a USB disk: LUKS,
//! LVM inside, file systems on its logical volumes).
//!
//! UDisks2 unlocks the drive but does not open LVM volumes on its own, and LVM refuses to
//! when this computer already uses a volume group of the same name (`ArchinstallVg` on both).
//! So failBrauwser maps the volumes itself, under names of its own (`fb-root-1a2b3c4d`), with
//! the device mapper: LVM's metadata on the drive is only read (`lvs --readonly`, limited to
//! that one device), never written. Mapping and mounting need root: the program runs itself
//! through `pkexec` as `failbrauwser --lvm open|close <encrypted partition>`.

use serde_json::Value;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What the device mapper volumes failBrauwser makes are recognized by.
pub const DM_UUID_PREFIX: &str = "FBLVM-";
/// The partition type LVM puts in a physical volume's signature (as `blkid` reports it).
pub const PV_TYPE: &str = "LVM2_member";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Where in the logical volume it starts, and how long it is, in extents.
    pub start: u64,
    pub size: u64,
    /// Where on the physical volume it starts, in extents.
    pub pv_start: u64,
    /// `linear`, or something failBrauwser cannot map (`thin`, `raid1`, …).
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalVolume {
    pub vg: String,
    pub name: String,
    pub uuid: String,
    /// Extent size in 512-byte sectors.
    pub extent: u64,
    pub segments: Vec<Segment>,
}

impl LogicalVolume {
    /// The device mapper name: the volume's name and the start of its UUID, so that it
    /// never collides with this computer's own (`ArchinstallVg-root`).
    pub fn dm_name(&self) -> String {
        let id: String = self.uuid.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
        let name: String = self.name.chars().map(|c| if c.is_ascii_alphanumeric() || "+_.".contains(c) { c } else { '_' }).collect();
        format!("fb-{name}-{id}")
    }

    pub fn dm_uuid(&self) -> String {
        format!("{DM_UUID_PREFIX}{}", self.uuid.replace('-', ""))
    }

    /// The device mapper table: each segment mapped onto the physical volume `pv`, whose
    /// data starts `pe_start` sectors in.
    pub fn dm_table(&self, pv: &str, pe_start: u64) -> Result<String, String> {
        let mut segs = self.segments.clone();
        segs.sort_by_key(|s| s.start);
        let mut out = String::new();
        let mut next = 0;
        for s in &segs {
            if s.kind != "linear" {
                return Err(format!("the volume “{}” is a {} volume, which failBrauwser cannot open", self.name, s.kind));
            }
            if s.start != next || s.size == 0 {
                return Err(format!("the volume “{}” has a gap", self.name));
            }
            next = s.start + s.size;
            out.push_str(&format!("{} {} linear {pv} {}\n", s.start * self.extent, s.size * self.extent, pe_start + s.pv_start * self.extent));
        }
        if out.is_empty() {
            return Err(format!("the volume “{}” is empty", self.name));
        }
        Ok(out)
    }
}

/// The rows of an LVM JSON report (`{"report": [{"seg": [{…}, …]}]}`).
fn report_rows(json: &str) -> Result<Vec<serde_json::Map<String, Value>>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("cannot read the LVM report: {e}"))?;
    let mut rows = Vec::new();
    for r in v["report"].as_array().into_iter().flatten() {
        for list in r.as_object().into_iter().flat_map(|o| o.values()) {
            rows.extend(list.as_array().into_iter().flatten().filter_map(|x| x.as_object().cloned()));
        }
    }
    Ok(rows)
}

fn field<'a>(row: &'a serde_json::Map<String, Value>, k: &str) -> &'a str {
    row.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

fn number(row: &serde_json::Map<String, Value>, k: &str) -> Result<u64, String> {
    let t = field(row, k);
    // Whole numbers, sometimes written "8192.00".
    t.parse()
        .ok()
        .or_else(|| t.parse::<f64>().ok().filter(|f| f.fract() == 0.0 && *f >= 0.0).map(|f| f as u64))
        .ok_or_else(|| format!("unexpected {k} in the LVM report: {t:?}"))
}

/// The logical volumes in the segment report of `lvs --segments` with the fields
/// [`LVS_FIELDS`] (units in sectors, no suffix).
pub fn parse_segments(json: &str) -> Result<Vec<LogicalVolume>, String> {
    let mut out: Vec<LogicalVolume> = Vec::new();
    for row in report_rows(json)? {
        let uuid = field(&row, "lv_uuid").to_string();
        let kind = match field(&row, "segtype") {
            // One stripe is the same as linear.
            "striped" if field(&row, "stripes") == "1" => "linear".to_string(),
            k => k.to_string(),
        };
        // "/dev/dm-3:0-12799": the extents on the physical volume.
        let ranges = field(&row, "seg_pe_ranges");
        let pv_start = match ranges.rsplit_once(':').and_then(|(dev, r)| Some((dev, r.split_once('-')?.0.parse::<u64>().ok()?))) {
            Some((dev, start)) if !dev.contains("unknown") && !dev.contains("missing") && !ranges.contains(' ') => start,
            _ if kind != "linear" => 0,
            _ => return Err(format!("the volume “{}” is not entirely on this drive", field(&row, "lv_name"))),
        };
        let seg = Segment { start: number(&row, "seg_start_pe")?, size: number(&row, "seg_size_pe")?, pv_start, kind };
        match out.iter_mut().find(|l| l.uuid == uuid) {
            Some(l) => l.segments.push(seg),
            None => out.push(LogicalVolume {
                vg: field(&row, "vg_name").to_string(),
                name: field(&row, "lv_name").to_string(),
                uuid,
                extent: number(&row, "vg_extent_size")?,
                segments: vec![seg],
            }),
        }
    }
    Ok(out)
}

pub const LVS_FIELDS: &str = "vg_name,vg_extent_size,lv_name,lv_uuid,segtype,stripes,seg_start_pe,seg_size_pe,seg_pe_ranges";

/// Where the physical volume's data starts (`pvs -o pe_start`, in sectors).
pub fn parse_pe_start(json: &str) -> Result<u64, String> {
    let rows = report_rows(json)?;
    let row = rows.first().ok_or("the drive holds no LVM physical volume")?;
    number(row, "pe_start")
}

/// The kernel's name for a device (`/dev/sda2` → `sda2`, `/dev/mapper/x` → `dm-3`).
fn kernel_name(dev: &Path) -> Option<String> {
    let real = std::fs::canonicalize(dev).unwrap_or_else(|_| dev.to_path_buf());
    real.file_name().map(|n| n.to_string_lossy().into_owned())
}

fn sys_list(sys: &Path, name: &str, what: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(sys.join("class/block").join(name).join(what))
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn dm_uuid(sys: &Path, name: &str) -> String {
    std::fs::read_to_string(sys.join("class/block").join(name).join("dm/uuid")).unwrap_or_default().trim().to_string()
}

/// The unlocked contents of an encrypted partition: the device mapper device holding it
/// whose UUID says it is dm-crypt's.
pub fn cleartext_of(sys: &Path, partition: &str) -> Option<String> {
    sys_list(sys, partition, "holders").into_iter().find(|h| dm_uuid(sys, h).starts_with("CRYPT-"))
}

/// The devices built on top of `dev` (its LVM volumes), by kernel name.
pub fn holders(sys: &Path, dev: &str) -> Vec<String> {
    sys_list(sys, dev, "holders")
}

/// The devices `dev` is built on (an LVM volume's physical volume).
pub fn slaves(sys: &Path, dev: &str) -> Vec<String> {
    sys_list(sys, dev, "slaves")
}

/// For volumes the drive list shows: the encrypted partition (UDisks2 object) whose LVM
/// volumes they are. `containers` are (UDisks2 object, device) of unlocked, removable ones.
pub fn lvm_containers(sys: &Path, devices: &[String], containers: &[(String, String)]) -> Vec<Option<String>> {
    let clear: Vec<(String, String)> = containers.iter().filter_map(|(obj, dev)| Some((cleartext_of(sys, &kernel_name(Path::new(dev))?)?, obj.clone()))).collect();
    devices
        .iter()
        .map(|d| {
            let name = kernel_name(Path::new(d))?;
            if !name.starts_with("dm-") {
                return None;
            }
            let s = slaves(sys, &name);
            clear.iter().find(|(c, _)| s.contains(c)).map(|(_, obj)| obj.clone())
        })
        .collect()
}

/// A free folder for a volume below `base`: its name, or with a number after it.
pub fn mount_dir(base: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let clean: String = name.chars().map(|c| if c == '/' || c.is_control() { '_' } else { c }).collect();
    let clean = if clean.trim().is_empty() || clean == "." || clean == ".." { "disk".to_string() } else { clean };
    let mut p = base.join(&clean);
    let mut n = 1;
    while taken(&p) {
        p = base.join(format!("{clean}{n}"));
        n += 1;
    }
    p
}

// ── The part that runs as root ───────────────────────────────────────────────────────────

fn run(cmd: &str, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut c = Command::new(cmd);
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).env("LC_ALL", "C");
    c.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = c.spawn().map_err(|e| format!("cannot run {cmd}: {e}"))?;
    if let (Some(text), Some(mut i)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        i.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err: Vec<&str> = err.lines().filter(|l| !l.trim_start().starts_with("WARNING")).collect();
        Err(format!("{cmd} {}: {}", args.first().unwrap_or(&""), err.join(" ").trim()))
    }
}

fn blkid(dev: &str, tag: &str) -> String {
    run("blkid", &["-p", "-o", "value", "-s", tag, dev], None).unwrap_or_default().trim().to_string()
}

/// Where the device with this device number is mounted, deepest first.
fn mounts_of(rdev: u64) -> Vec<PathBuf> {
    let (maj, min) = (libc::major(rdev), libc::minor(rdev));
    let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut v: Vec<PathBuf> = text
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(' ').collect();
            (f.len() > 4 && f[2] == format!("{maj}:{min}")).then(|| PathBuf::from(crate::drives::unescape_mount(f[4])))
        })
        .collect();
    v.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    v
}

fn rdev(dev: &str) -> Result<u64, String> {
    let m = std::fs::metadata(dev).map_err(|e| format!("{dev}: {e}"))?;
    if !m.file_type().is_block_device() {
        return Err(format!("{dev} is not a drive"));
    }
    Ok(m.rdev())
}

/// The user who asked (pkexec says who): name, uid and gid.
fn asking_user() -> Result<(String, u32, u32), String> {
    let uid: u32 = std::env::var("PKEXEC_UID").or_else(|_| std::env::var("SUDO_UID")).ok().and_then(|u| u.parse().ok()).ok_or("run through pkexec")?;
    // SAFETY: getpwuid returns a pointer to static storage, read right away.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return Err(format!("no user {uid}"));
        }
        let name = std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        Ok((name, uid, (*pw).pw_gid))
    }
}

/// The unlocked contents of the encrypted partition `partition`, checked to be an LVM
/// physical volume: (kernel name, /dev path).
fn physical_volume(partition: &str) -> Result<(String, String), String> {
    if !partition.starts_with("/dev/") {
        return Err(format!("{partition} is not a drive"));
    }
    rdev(partition)?;
    let sys = Path::new("/sys");
    let name = kernel_name(Path::new(partition)).ok_or("no such drive")?;
    let clear = cleartext_of(sys, &name).ok_or("the drive is not unlocked")?;
    let dev = format!("/dev/{clear}");
    if blkid(&dev, "TYPE") != PV_TYPE {
        return Err("the drive holds no LVM volumes".into());
    }
    Ok((clear, dev))
}

/// Maps the drive's LVM volumes (unless they are already) and mounts each file system on
/// them below /run/media/<user>; prints `MOUNT <folder>` for each.
fn open(partition: &str) -> Result<(), String> {
    let (clear, pv) = physical_volume(partition)?;
    let sys = Path::new("/sys");
    let mut devices: Vec<String> = holders(sys, &clear).into_iter().map(|h| format!("/dev/{h}")).collect();
    if devices.is_empty() {
        let only = ["--readonly", "--devices", pv.as_str(), "--reportformat", "json", "--units", "s", "--nosuffix"];
        let pe_start = parse_pe_start(&run("pvs", &[&only[..], &["-o", "pe_start"]].concat(), None)?)?;
        let lvs = parse_segments(&run("lvs", &[&only[..], &["--segments", "-o", LVS_FIELDS]].concat(), None)?)?;
        if lvs.is_empty() {
            return Err("the drive's LVM holds no volumes".into());
        }
        let mut problems = Vec::new();
        for lv in &lvs {
            match lv.dm_table(&pv, pe_start) {
                Ok(table) => match run("dmsetup", &["create", &lv.dm_name(), "--uuid", &lv.dm_uuid()], Some(&table)) {
                    Ok(_) => devices.push(format!("/dev/mapper/{}", lv.dm_name())),
                    Err(e) => problems.push(e),
                },
                Err(e) => problems.push(e),
            }
        }
        let _ = run("udevadm", &["settle", "--timeout=10"], None);
        if devices.is_empty() {
            return Err(problems.join("; "));
        }
        for p in problems {
            eprintln!("{p}");
        }
    }
    let (user, uid, gid) = asking_user()?;
    let base = PathBuf::from("/run/media").join(&user);
    if !base.exists() {
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let c = std::ffi::CString::new(base.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: plain C call on a valid path.
        unsafe { libc::chown(c.as_ptr(), 0, gid) };
        let _ = std::fs::set_permissions(&base, std::os::unix::fs::PermissionsExt::from_mode(0o750));
    }
    let mut any = false;
    for dev in &devices {
        let kind = blkid(dev, "TYPE");
        if kind.is_empty() || kind == "swap" || kind == PV_TYPE || kind.starts_with("crypto") {
            continue;
        }
        if let Some(m) = mounts_of(rdev(dev)?).pop() {
            println!("MOUNT {}", m.display());
            any = true;
            continue;
        }
        let label = blkid(dev, "LABEL");
        let name = if label.is_empty() { dev.rsplit('/').next().and_then(|n| n.strip_prefix("fb-")).and_then(|n| n.rsplit_once('-')).map(|(n, _)| n.to_string()).unwrap_or_else(|| dev.rsplit('/').next().unwrap_or("disk").to_string()) } else { label };
        let dir = mount_dir(&base, &name, |p| p.exists());
        std::fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut opts = "nosuid,nodev".to_string();
        if matches!(kind.as_str(), "vfat" | "exfat" | "ntfs" | "ntfs3") {
            opts.push_str(&format!(",uid={uid},gid={gid}"));
        }
        match run("mount", &["-o", &opts, dev, &dir.to_string_lossy()], None) {
            Ok(_) => {
                println!("MOUNT {}", dir.display());
                any = true;
            }
            Err(e) => {
                let _ = std::fs::remove_dir(&dir);
                eprintln!("{e}");
            }
        }
    }
    if any { Ok(()) } else { Err("none of the drive's LVM volumes holds a file system that can be mounted".into()) }
}

/// Unmounts the drive's LVM volumes and removes their device mapper devices, so that the
/// drive can be locked.
fn close(partition: &str) -> Result<(), String> {
    let (clear, _) = physical_volume(partition)?;
    let sys = Path::new("/sys");
    let (user, ..) = asking_user()?;
    let base = PathBuf::from("/run/media").join(user);
    for h in holders(sys, &clear) {
        let dev = format!("/dev/{h}");
        for m in mounts_of(rdev(&dev)?) {
            run("umount", &[&m.to_string_lossy()], None).map_err(|e| if e.contains("busy") { format!("files on {} are still in use", m.display()) } else { e })?;
            if m.starts_with(&base) {
                let _ = std::fs::remove_dir(&m);
            }
        }
        let uuid = dm_uuid(sys, &h);
        if uuid.starts_with(DM_UUID_PREFIX) || uuid.starts_with("LVM-") {
            let name = std::fs::read_to_string(sys.join("class/block").join(&h).join("dm/name")).unwrap_or_default();
            run("dmsetup", &["remove", name.trim()], None)?;
        }
    }
    Ok(())
}

/// `failbrauwser --lvm open|close /dev/sdX`: run as root through pkexec. Returns the exit code.
pub fn helper_main(args: &[String]) -> i32 {
    // SAFETY: plain C call.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("failbrauwser --lvm must run as root (through pkexec)");
        return 2;
    }
    let res = match args {
        [a, dev] if a == "open" => open(dev),
        [a, dev] if a == "close" => close(dev),
        _ => Err("usage: failbrauwser --lvm open|close <encrypted partition>".into()),
    };
    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `lvs --segments --reportformat json` prints for a typical installation: root
    /// in one piece, home in two (grown later), and a thin pool.
    const LVS: &str = r#"{
      "report": [{"seg": [
        {"vg_name":"ArchinstallVg", "vg_extent_size":"8192", "lv_name":"home", "lv_uuid":"Ab12Cd-34Ef-x", "segtype":"linear", "stripes":"1", "seg_start_pe":"0", "seg_size_pe":"1000", "seg_pe_ranges":"/dev/dm-3:12800-13799"},
        {"vg_name":"ArchinstallVg", "vg_extent_size":"8192", "lv_name":"home", "lv_uuid":"Ab12Cd-34Ef-x", "segtype":"striped", "stripes":"1", "seg_start_pe":"1000", "seg_size_pe":"500", "seg_pe_ranges":"/dev/dm-3:20000-20499"},
        {"vg_name":"ArchinstallVg", "vg_extent_size":"8192", "lv_name":"root", "lv_uuid":"Zz99-Yy88", "segtype":"linear", "stripes":"1", "seg_start_pe":"0", "seg_size_pe":"12800", "seg_pe_ranges":"/dev/dm-3:0-12799"},
        {"vg_name":"ArchinstallVg", "vg_extent_size":"8192", "lv_name":"pool", "lv_uuid":"Pp-1", "segtype":"thin-pool", "stripes":"1", "seg_start_pe":"0", "seg_size_pe":"10", "seg_pe_ranges":""}
      ]}],
      "log": []
    }"#;

    #[test]
    fn segments_become_device_mapper_tables() {
        let lvs = parse_segments(LVS).unwrap();
        assert_eq!(lvs.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["home", "root", "pool"]);
        let home = &lvs[0];
        assert_eq!(home.segments.len(), 2);
        assert_eq!(home.dm_table("/dev/dm-3", 2048).unwrap(), format!("0 8192000 linear /dev/dm-3 {}\n8192000 4096000 linear /dev/dm-3 {}\n", 2048 + 12800 * 8192, 2048 + 20000 * 8192));
        assert_eq!(lvs[1].dm_table("/dev/dm-3", 2048).unwrap(), "0 104857600 linear /dev/dm-3 2048\n");
        assert!(lvs[2].dm_table("/dev/dm-3", 2048).unwrap_err().contains("thin-pool"));
        // Never the computer's own names.
        assert_eq!(lvs[1].dm_name(), "fb-root-Zz99Yy88");
        assert_eq!(home.dm_uuid(), "FBLVM-Ab12Cd34Efx");
    }

    #[test]
    fn volumes_partly_elsewhere_are_refused() {
        let missing = LVS.replace("/dev/dm-3:0-12799", "[unknown]:0-12799");
        assert!(parse_segments(&missing).unwrap_err().contains("root"));
        let gap = LogicalVolume { vg: "v".into(), name: "x".into(), uuid: "u".into(), extent: 8, segments: vec![Segment { start: 5, size: 1, pv_start: 0, kind: "linear".into() }] };
        assert!(gap.dm_table("/dev/x", 0).is_err());
        assert!(parse_segments("not json").is_err());
    }

    #[test]
    fn data_start_of_the_physical_volume() {
        assert_eq!(parse_pe_start(r#"{"report":[{"pv":[{"pe_start":"2048"}]}]}"#).unwrap(), 2048);
        assert_eq!(parse_pe_start(r#"{"report":[{"pv":[{"pe_start":"2048.00"}]}]}"#).unwrap(), 2048);
        assert!(parse_pe_start(r#"{"report":[{"pv":[]}]}"#).is_err());
    }

    #[test]
    fn volumes_found_through_sysfs() {
        let sys = tempfile::tempdir().unwrap();
        let blk = sys.path().join("class/block");
        let mk = |name: &str, uuid: &str, holders: &[&str], slaves: &[&str]| {
            let d = blk.join(name);
            std::fs::create_dir_all(d.join("holders")).unwrap();
            std::fs::create_dir_all(d.join("slaves")).unwrap();
            if !uuid.is_empty() {
                std::fs::create_dir_all(d.join("dm")).unwrap();
                std::fs::write(d.join("dm/uuid"), format!("{uuid}\n")).unwrap();
            }
            for h in holders {
                std::fs::write(d.join("holders").join(h), "").unwrap();
            }
            for s in slaves {
                std::fs::write(d.join("slaves").join(s), "").unwrap();
            }
        };
        mk("sda2", "", &["dm-3"], &[]);
        mk("dm-3", "CRYPT-LUKS2-abc-luks", &["dm-4", "dm-5"], &["sda2"]);
        mk("dm-4", "FBLVM-x", &[], &["dm-3"]);
        mk("dm-5", "FBLVM-y", &[], &["dm-3"]);
        mk("dm-1", "LVM-own", &[], &["dm-0"]);
        assert_eq!(cleartext_of(sys.path(), "sda2").as_deref(), Some("dm-3"));
        assert_eq!(holders(sys.path(), "dm-3"), ["dm-4", "dm-5"]);
        let found = lvm_containers(sys.path(), &["/dev/dm-4".into(), "/dev/dm-1".into(), "/dev/sda1".into()], &[("/o/sda2".into(), "/dev/sda2".into())]);
        assert_eq!(found, [Some("/o/sda2".to_string()), None, None]);
    }

    #[test]
    fn mount_folders_are_free_ones() {
        let base = Path::new("/run/media/me");
        assert_eq!(mount_dir(base, "root", |_| false), base.join("root"));
        assert_eq!(mount_dir(base, "root", |p| p.ends_with("root") || p.ends_with("root1")), base.join("root2"));
        assert_eq!(mount_dir(base, "../x", |_| false), base.join(".._x"));
        assert_eq!(mount_dir(base, "..", |_| false), base.join("disk"));
    }
}
