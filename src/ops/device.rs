//! What kind of storage a path lives on. Copies pick their parallelism and write
//! pacing from this.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    /// SSD / NVMe.
    Fast,
    /// Spinning disk: seeks are expensive, parallel writes hurt.
    Rotational,
    /// USB sticks, SD cards and other hot-pluggable drives.
    Removable,
    /// NFS, SMB, FUSE: latency bound, benefits from parallel requests.
    Network,
    /// tmpfs and friends.
    Memory,
}

impl DeviceClass {
    /// Worker threads for copying many files to or from this device.
    pub fn workers(self) -> usize {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        match self {
            DeviceClass::Fast | DeviceClass::Memory => cpus.clamp(2, 8),
            DeviceClass::Network => 6,
            DeviceClass::Removable => 2,
            DeviceClass::Rotational => 1,
        }
    }

    /// Writes to this device should be paced so the page cache never holds much dirty data.
    pub fn is_slow(self) -> bool {
        matches!(self, DeviceClass::Rotational | DeviceClass::Removable)
    }
}

/// Classifies the device holding `path` (which must exist).
pub fn classify(path: &Path) -> DeviceClass {
    #[cfg(unix)]
    return unix_classify(path);
    #[cfg(windows)]
    return win_classify(path);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        DeviceClass::Fast
    }
}

#[cfg(unix)]
fn unix_classify(path: &Path) -> DeviceClass {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(path) else { return DeviceClass::Fast };
    if let Some(c) = classify_by_fs_type(path) {
        return c;
    }
    let dev = meta.dev();
    let (major, minor) = (libc::major(dev), libc::minor(dev));
    let sys = PathBuf::from(format!("/sys/dev/block/{major}:{minor}"));
    classify_sysfs(&sys).unwrap_or(DeviceClass::Fast)
}

/// First cut at Windows: volume type only. `win::storage` (drives milestone) adds the
/// rotational/SSD answer from the storage device descriptor.
#[cfg(windows)]
fn win_classify(path: &Path) -> DeviceClass {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::Win32::System::WindowsProgramming::{DRIVE_REMOVABLE, DRIVE_REMOTE};
    // GetDriveTypeW wants the volume root; UNC roots answer DRIVE_REMOTE there too.
    let root = crate::win::volume::volume_root_path(path).unwrap_or_else(|| path.to_path_buf());
    let w: Vec<u16> = OsStr::new(&root).encode_wide().chain(std::iter::once(0)).collect();
    match unsafe { GetDriveTypeW(windows::core::PCWSTR(w.as_ptr())) } {
        DRIVE_REMOVABLE => DeviceClass::Removable,
        DRIVE_REMOTE => DeviceClass::Network,
        _ => DeviceClass::Fast,
    }
}

#[cfg(unix)]
fn classify_by_fs_type(path: &Path) -> Option<DeviceClass> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    // Magic numbers from linux/magic.h.
    const NFS: i64 = 0x6969;
    const SMB: i64 = 0x517B;
    const SMB2: i64 = 0xFE534D42;
    const CIFS: i64 = 0xFF534D42;
    const FUSE: i64 = 0x65735546;
    const TMPFS: i64 = 0x01021994;
    const RAMFS: i64 = 0x858458f6;
    let t = st.f_type as i64;
    match t {
        NFS | SMB | SMB2 | CIFS | FUSE => Some(DeviceClass::Network),
        TMPFS | RAMFS => Some(DeviceClass::Memory),
        _ => None,
    }
}

/// Reads `removable`, `queue/rotational` and the bus path of a block device in sysfs.
/// Partitions carry none of these themselves; their parent disk does.
#[cfg(unix)]
pub fn classify_sysfs(dev_link: &Path) -> Option<DeviceClass> {
    let dev = std::fs::canonicalize(dev_link).ok()?;
    let disk = if dev.join("partition").exists() { dev.parent()?.to_path_buf() } else { dev.clone() };
    let read = |p: &Path| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string());
    let on_usb = disk.to_string_lossy().contains("/usb");
    let removable = read(&disk.join("removable")).as_deref() == Some("1");
    if on_usb || removable {
        return Some(DeviceClass::Removable);
    }
    // Device mapper / md stack on something else; use their first slave.
    if !disk.join("queue/rotational").exists() {
        return Some(DeviceClass::Fast);
    }
    if read(&disk.join("queue/rotational")).as_deref() == Some("1") {
        return Some(DeviceClass::Rotational);
    }
    if let Some(slave) = std::fs::read_dir(disk.join("slaves")).ok().and_then(|mut d| d.next()).and_then(Result::ok) {
        if let Some(c) = classify_sysfs(&slave.path()) {
            return Some(c);
        }
    }
    Some(DeviceClass::Fast)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;

    #[test]
    #[cfg(unix)] // sysfs layouts are a Linux notion
    fn classifies_fake_sysfs_trees() {
        let root = tempfile::tempdir().unwrap();
        let usb = root.path().join("devices/pci0/usb2/2-1/host6/block/sdb");
        fs::create_dir_all(usb.join("sdb1")).unwrap();
        fs::create_dir_all(usb.join("queue")).unwrap();
        fs::write(usb.join("sdb1/partition"), "1").unwrap();
        fs::write(usb.join("removable"), "0").unwrap();
        fs::write(usb.join("queue/rotational"), "1").unwrap();
        assert_eq!(classify_sysfs(&usb.join("sdb1")), Some(DeviceClass::Removable));

        let hdd = root.path().join("devices/pci0/ata1/block/sda");
        fs::create_dir_all(hdd.join("queue")).unwrap();
        fs::write(hdd.join("removable"), "0").unwrap();
        fs::write(hdd.join("queue/rotational"), "1").unwrap();
        assert_eq!(classify_sysfs(&hdd), Some(DeviceClass::Rotational));

        let ssd = root.path().join("devices/pci0/nvme/block/nvme0n1");
        fs::create_dir_all(ssd.join("queue")).unwrap();
        fs::write(ssd.join("removable"), "0").unwrap();
        fs::write(ssd.join("queue/rotational"), "0").unwrap();
        assert_eq!(classify_sysfs(&ssd), Some(DeviceClass::Fast));
    }

    #[test]
    #[cfg(unix)] // the probe paths ("/", "/dev/shm") are Linux paths
    fn real_paths_classify_without_panicking() {
        let _ = classify(Path::new("/"));
        assert_eq!(classify(Path::new("/dev/shm")), DeviceClass::Memory);
    }

    #[test]
    fn slow_devices_get_few_workers() {
        assert_eq!(DeviceClass::Rotational.workers(), 1);
        assert!(DeviceClass::Fast.workers() >= 2);
        assert!(DeviceClass::Removable.is_slow());
        assert!(!DeviceClass::Fast.is_slow());
    }
}
