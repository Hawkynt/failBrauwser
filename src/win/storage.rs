//! Windows volumes as the drives page shows them: every drive letter with its label, file
//! system, size and drive kind, plus the two calls the drive buttons make — an optical
//! eject and the safe removal Explorer's "Eject" performs on sticks. Everything Win32 for
//! drives lives here; the shared `drives` model only ever sees plain `Volume`s.

use crate::drives::Volume;
use std::ffi::c_void;
use std::path::PathBuf;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Parent, CM_Query_And_Remove_SubTreeW, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, PNP_VetoDevice, PNP_VetoInsufficientRights, PNP_VetoNonDisableable, PNP_VetoOutstandingOpen, PNP_VetoPendingClose, PNP_VETO_TYPE, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, OPEN_EXISTING, BusTypeMmc, BusTypeSd, BusTypeUsb,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    DEVICE_SEEK_PENALTY_DESCRIPTOR, GET_LENGTH_INFORMATION, GUID_DEVINTERFACE_DISK, IOCTL_DISK_GET_LENGTH_INFO, IOCTL_STORAGE_EJECT_MEDIA, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_DESCRIPTOR_HEADER, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY, StorageDeviceProperty, StorageDeviceSeekPenaltyProperty,
};
use windows::Win32::System::WindowsProgramming::{DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE};

/// The drive letters present, as `GetLogicalDrives`'s A..Z bitmask (the drives page polls
/// it on Windows where there is no UDisks2 signal to subscribe to).
pub fn drive_mask() -> u32 {
    unsafe { GetLogicalDrives() }
}

/// Every mounted volume with a drive letter, in drive-letter order: the twin of what
/// UDisks2 answers on Linux. Never call it on the UI thread.
pub fn volumes() -> Vec<Volume> {
    let mask = drive_mask();
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let kind = match unsafe { GetDriveTypeW(PCWSTR(wide_text(&root).as_ptr())) } {
            DRIVE_REMOVABLE => ("Removable Disk", true, false),
            DRIVE_FIXED => ("Local Disk", false, false),
            DRIVE_CDROM => ("CD Drive", false, true),
            DRIVE_REMOTE => ("Network Drive", false, false),
            DRIVE_RAMDISK => ("RAM Drive", false, false),
            // DRIVE_UNKNOWN / DRIVE_NO_ROOT_DIR: nothing to show.
            _ => continue,
        };
        let (mut label_buf, mut fs_buf) = ([0u16; 261], [0u16; 261]);
        let (mut serial, mut comp, mut flags) = (0u32, 0u32, 0u32);
        let have_media = unsafe { GetVolumeInformationW(PCWSTR(wide_text(&root).as_ptr()), Some(&mut label_buf), Some(&raw mut serial), Some(&raw mut comp), Some(&raw mut flags), Some(&mut fs_buf)) }.is_ok();
        let (label, fs) = (wide_nul(&label_buf), wide_nul(&fs_buf));
        let disk = disk_info(letter).unwrap_or_default();
        // Mapped shares carry their UNC origin and get the Disconnect button (the drives
        // dispatch routes a volume with a share_uri to shares::eject → WNetCancelConnection);
        // the system's own fixed drive gets no unmount button; an externally attached
        // fixed disk does.
        let network = kind.0 == "Network Drive";
        let share_uri = if network { crate::win::wnet::connection_for(&root).as_deref().and_then(|unc| crate::win::wnet::smb_for_unc(std::path::Path::new(unc))).unwrap_or_default() } else { String::new() };
        let internal_fixed = kind.1 == false && kind.2 == false && letter_is_fixed(&root) && disk.bus != "usb";
        out.push(Volume {
            device: root.clone(),
            // Without a label Explorer's name for the kind: "Local Disk", "Removable Disk"…
            label: if label.is_empty() { kind.0.to_string() } else { label },
            fs_type: fs,
            size: disk.length.unwrap_or(0),
            mount_points: vec![PathBuf::from(&root)],
            drive_name: disk.model,
            removable: kind.1 || disk.removable,
            usb: disk.bus == "usb",
            optical: kind.2,
            ejectable: kind.2 || kind.1 || disk.removable || disk.bus == "usb",
            block_object: root.clone(),
            drive_object: root.clone(),
            share_uri,
            // An empty drive (no media in a tray or reader) has nothing to mount.
            mountable: have_media,
            // The system's own drive and mapped network drives get no unmount button
            // (disconnecting a share comes with the network milestone).
            system: internal_fixed,
            uuid: volume_guid(&root),
            read_only: kind.2,
            serial: disk.serial,
            revision: disk.revision,
            bus: disk.bus,
            rotation_rate: disk.rotation,
            ..Default::default()
        });
    }
    crate::drives::sort_volumes(&mut out);
    out
}

/// Ejects an optical tray (or loads it again from a tray-less drive).
pub fn eject_media(root: &str) -> Result<(), String> {
    let Some(letter) = root.chars().next() else { return Err("Not a drive letter.".into()) };
    let h = unsafe { CreateFileW(PCWSTR(wide_text(&format!("\\\\.\\{letter}:")).as_ptr()), GENERIC_READ.0 | GENERIC_WRITE.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.map_err(|e| e.to_string())?;
    let r = unsafe { DeviceIoControl(h, IOCTL_STORAGE_EJECT_MEDIA, None, 0, None, 0, None, None) };
    unsafe {
        let _ = CloseHandle(h);
    }
    r.map_err(|e| e.to_string())
}

/// Removes a drive the way Explorer's "Eject" does: ask PnP to take the whole device
/// subtree out (flushing its caches), which is what makes it "safe to unplug". `root` is
/// the volume root ("C:\\").
pub fn safe_remove(root: &str) -> Result<(), String> {
    let Some(letter) = root.chars().next() else { return Err("Not a drive letter.".into()) };
    let disk = disk_number(letter).ok_or_else(|| format!("There is no disk behind “{root}” to remove."))?;
    let dev = disk_devinst(disk).ok_or_else(|| format!("The drive behind “{root}” is not in the device tree."))?;
    // Take the parent out, not only the disk: for a stick that is the whole device; the
    // disk alone would leave the rest behind. Without a parent, the disk itself.
    let mut target = dev;
    let mut parent = 0u32;
    if unsafe { CM_Get_Parent(&raw mut parent, dev, 0) } == CR_SUCCESS {
        target = parent;
    }
    let mut veto = PNP_VETO_TYPE(0);
    let mut name = [0u16; 261];
    let r = unsafe { CM_Query_And_Remove_SubTreeW(target, Some(&raw mut veto), Some(&mut name), 0) };
    if r == CR_SUCCESS {
        return Ok(());
    }
    Err(remove_message(veto, &name))
}

/// Why PnP refused, in the user's terms, with the vetoing program's name when PnP names one.
fn remove_message(veto: PNP_VETO_TYPE, name: &[u16]) -> String {
    let why = if veto == PNP_VetoOutstandingOpen {
        "Files on this drive are still open somewhere."
    } else if veto == PNP_VetoPendingClose {
        "Files on the drive are still closing."
    } else if veto == PNP_VetoDevice {
        "This drive cannot be removed."
    } else if veto == PNP_VetoNonDisableable {
        "This drive cannot be disabled."
    } else if veto == PNP_VetoInsufficientRights {
        "There are not enough rights to remove it."
    } else {
        "Windows refused to remove it."
    };
    let who = wide_nul(name);
    if who.is_empty() {
        why.to_string()
    } else {
        format!("{why} ({who})")
    }
}

/// What the drive behind a volume answers to the storage queries: model, serial, firmware,
/// whether it is removable media, its bus, whether it has spinning platters, and its size.
#[derive(Debug, Default, PartialEq)]
struct DiskInfo {
    model: String,
    serial: String,
    revision: String,
    removable: bool,
    bus: String,
    rotation: i32,
    length: Option<u64>,
}

fn disk_info(letter: char) -> Option<DiskInfo> {
    let h = open_volume(letter)?;
    let mut out = DiskInfo { rotation: -1, ..Default::default() };
    unsafe {
        if let Some(raw) = storage_property(h, StorageDeviceProperty.0).filter(|raw| raw.len() >= std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>()) {
            // Safe after the length check: the header says the buffer is this big.
            let desc = (raw.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR).read_unaligned();
            out.model = [ansi_at(&raw, desc.VendorIdOffset), ansi_at(&raw, desc.ProductIdOffset)].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
            out.serial = ansi_at(&raw, desc.SerialNumberOffset);
            out.revision = ansi_at(&raw, desc.ProductRevisionOffset);
            out.removable = desc.RemovableMedia;
            out.bus = bus_name(desc.BusType.0);
        }
        // A drive that never seeks gets solid: the same 0 the UDisks2 rotation rate uses.
        // A drive that does seek keeps -1: Windows says nothing about its rpm, and a made-up
        // number would be worse than none.
        if let Some(raw) = storage_property(h, StorageDeviceSeekPenaltyProperty.0).filter(|raw| raw.len() >= std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()) {
            let penalty = (raw.as_ptr() as *const DEVICE_SEEK_PENALTY_DESCRIPTOR).read_unaligned();
            if !penalty.IncursSeekPenalty {
                out.rotation = 0;
            }
        }
        let mut len = GET_LENGTH_INFORMATION::default();
        let mut got = 0u32;
        if DeviceIoControl(h, IOCTL_DISK_GET_LENGTH_INFO, None, 0, Some(&raw mut len as *mut _ as *mut c_void), std::mem::size_of::<GET_LENGTH_INFORMATION>() as u32, Some(&raw mut got), None).is_ok() && len.Length > 0 {
            out.length = Some(len.Length as u64);
        }
        let _ = CloseHandle(h);
    }
    Some(out)
}

/// Opens the volume device (`\\.\C:`) for queries; no access rights needed for those.
fn open_volume(letter: char) -> Option<windows::Win32::Foundation::HANDLE> {
    unsafe { CreateFileW(PCWSTR(wide_text(&format!("\\\\.\\{letter}:")).as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.ok()
}

fn storage_property(h: windows::Win32::Foundation::HANDLE, property: i32) -> Option<Vec<u8>> {
    let query = STORAGE_PROPERTY_QUERY { PropertyId: windows::Win32::System::Ioctl::STORAGE_PROPERTY_ID(property), QueryType: PropertyStandardQuery, AdditionalParameters: [0] };
    let mut header = STORAGE_DESCRIPTOR_HEADER::default();
    let mut got = 0u32;
    unsafe {
        DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, Some(&query as *const _ as *const c_void), std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32, Some(&mut header as *mut _ as *mut c_void), std::mem::size_of::<STORAGE_DESCRIPTOR_HEADER>() as u32, Some(&raw mut got), None).ok()?;
        if header.Size <= 0 {
            return None;
        }
        let mut buf = vec![0u8; header.Size as usize];
        DeviceIoControl(h, IOCTL_STORAGE_QUERY_PROPERTY, Some(&query as *const _ as *const c_void), std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32, Some(buf.as_mut_ptr() as *mut c_void), header.Size, Some(&raw mut got), None).ok()?;
        buf.truncate(got as usize);
        Some(buf)
    }
}

/// A storage bus in the drives model's words ("usb", "sdio", empty for internal).
fn bus_name(bus: i32) -> String {
    if bus == BusTypeUsb.0 { "usb".into() } else if bus == BusTypeSd.0 || bus == BusTypeMmc.0 { "sdio".into() } else { String::new() }
}

/// The drive letter's own type again (the `kind` name alone cannot tell a USB-HDD from an
/// internal one — the bus can, but a query can fail; this stays true for what GetDriveType saw).
fn letter_is_fixed(root: &str) -> bool {
    let kind = unsafe { GetDriveTypeW(PCWSTR(wide_text(root).as_ptr())) };
    kind == DRIVE_FIXED
}

/// The physical disk number behind a volume (`IOCTL_STORAGE_GET_DEVICE_NUMBER`).
fn disk_number(letter: char) -> Option<u32> {
    let h = open_volume(letter)?;
    let mut dn = STORAGE_DEVICE_NUMBER::default();
    let mut got = 0u32;
    let ok = unsafe { DeviceIoControl(h, IOCTL_STORAGE_GET_DEVICE_NUMBER, None, 0, Some(&mut dn as *mut _ as *mut c_void), std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32, Some(&raw mut got), None) }.is_ok();
    unsafe {
        let _ = CloseHandle(h);
    }
    ok.then_some(dn.DeviceNumber)
}

/// The PnP device node of the disk with this number: walk the disk device interfaces and
/// ask each which physical disk it answers to.
fn disk_devinst(number: u32) -> Option<u32> {
    unsafe {
        let set = SetupDiGetClassDevsW(Some(&GUID_DEVINTERFACE_DISK), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE).ok()?;
        let found = disk_devinst_in(set, number);
        let _ = SetupDiDestroyDeviceInfoList(set);
        found
    }
}

fn disk_devinst_in(set: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO, number: u32) -> Option<u32> {
    for i in 0.. {
        let mut iface = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, InterfaceClassGuid: GUID::zeroed(), Flags: 0, Reserved: 0 };
        if unsafe { SetupDiEnumDeviceInterfaces(set, None, &GUID_DEVINTERFACE_DISK, i, &raw mut iface) }.is_err() {
            return None;
        }
        let mut need = 0u32;
        let _ = unsafe { SetupDiGetDeviceInterfaceDetailW(set, &raw const iface, None, 0, Some(&raw mut need), None) };
        if need < std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
            continue;
        }
        // cbSize first, then the path: the API wants the struct size at the buffer's start.
        let mut buf = vec![0u8; need as usize];
        unsafe { std::ptr::write_unaligned(buf.as_mut_ptr() as *mut u32, std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32) };
        let mut devinfo = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ClassGuid: GUID::zeroed(), DevInst: 0, Reserved: 0 };
        if unsafe { SetupDiGetDeviceInterfaceDetailW(set, &raw const iface, Some(buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W), need, None, Some(&raw mut devinfo)) }.is_err() {
            continue;
        }
        let path = String::from_utf16_lossy(&buf[std::mem::size_of::<u32>()..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect::<Vec<_>>());
        let Ok(h) = (|| unsafe { CreateFileW(PCWSTR(wide_text(&path).as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) })() else { continue };
        let mut dn = STORAGE_DEVICE_NUMBER::default();
        let mut got = 0u32;
        let ok = unsafe { DeviceIoControl(h, IOCTL_STORAGE_GET_DEVICE_NUMBER, None, 0, Some(&mut dn as *mut _ as *mut c_void), std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32, Some(&raw mut got), None) }.is_ok();
        let _ = unsafe { CloseHandle(h) };
        if ok && dn.DeviceNumber == number {
            return Some(devinfo.DevInst);
        }
    }
    None
}

/// The volume's GUID (`\\?\Volume{…}\`), the Windows twin of the Linux UUID shown in
/// properties.
fn volume_guid(root: &str) -> String {
    let mut buf = [0u16; 64];
    if unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(wide_text(root).as_ptr()), &mut buf) }.is_err() {
        return String::new();
    }
    let name = wide_nul(&buf);
    name.find('{').and_then(|s| name[s..].find('}').map(|e| name[s + 1..s + e].to_string())).unwrap_or(name)
}

/// A NUL-terminated UTF-16 buffer as a trimmed String.
fn wide_nul(buf: &[u16]) -> String {
    let end = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}

/// A plain string as UTF-16 with the terminating NUL (no `\\?\` making — device paths go
/// through as they are).
fn wide_text(text: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
}

/// The NUL-terminated ASCII/ANSI string at `offset` in a storage descriptor's raw buffer.
fn ansi_at(raw: &[u8], offset: u32) -> String {
    if offset == 0 || offset as usize >= raw.len() {
        return String::new();
    }
    let s = &raw[offset as usize..];
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_strings_are_found_trimmed_and_absent_offsets_stay_empty() {
        // A descriptor as a driver would answer it: the struct, then its strings behind it.
        let head = std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>();
        let mut desc = STORAGE_DEVICE_DESCRIPTOR { VendorIdOffset: head as u32, ProductIdOffset: (head + 10) as u32, ProductRevisionOffset: (head + 24) as u32, SerialNumberOffset: 0, BusType: BusTypeUsb, RemovableMedia: true, ..Default::default() };
        let mut raw = vec![0u8; head + 40];
        unsafe { std::ptr::write_unaligned(raw.as_mut_ptr() as *mut STORAGE_DEVICE_DESCRIPTOR, std::mem::take(&mut desc)) };
        let put = |raw: &mut [u8], off: usize, text: &str| raw[off..off + text.len()].copy_from_slice(text.as_bytes());
        put(&mut raw, head, "SanDisk ");
        put(&mut raw, head + 10, "Ultra Fit");
        put(&mut raw, head + 24, "  rev 2.0 ");
        let info = parse_test_descriptor(&raw);
        assert_eq!(info.model, "SanDisk Ultra Fit");
        assert_eq!(info.revision, "rev 2.0");
        assert!(info.serial.is_empty());
        assert!(info.removable);
        assert_eq!(info.bus, "usb");
    }

    /// The same parsing `disk_info` does, over a caller-built buffer (the test's window in).
    fn parse_test_descriptor(raw: &[u8]) -> DiskInfo {
        let desc = unsafe { (raw.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR).read_unaligned() };
        DiskInfo {
            model: [ansi_at(raw, desc.VendorIdOffset), ansi_at(raw, desc.ProductIdOffset)].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" "),
            serial: ansi_at(raw, desc.SerialNumberOffset),
            revision: ansi_at(raw, desc.ProductRevisionOffset),
            removable: desc.RemovableMedia,
            bus: bus_name(desc.BusType.0),
            ..Default::default()
        }
    }

    #[test]
    fn bus_names_match_the_drives_model() {
        assert_eq!(bus_name(BusTypeUsb.0), "usb");
        assert_eq!(bus_name(BusTypeSd.0), "sdio");
        assert_eq!(bus_name(3), "");
    }

    #[test]
    fn every_lettered_volume_is_listed_as_a_mounted_one() {
        let vols = volumes();
        // This machine's own drive must be in the list, named and mounted.
        let here = std::path::PathBuf::from(std::env::current_exe().unwrap().components().next().unwrap().as_os_str());
        let here_root = format!("{}\\", here.to_string_lossy());
        let here = vols.iter().find(|v| v.device == here_root).unwrap_or_else(|| panic!("{} not in {:?}", here_root, vols.iter().map(|v| (&v.device, &v.label)).collect::<Vec<_>>()));
        assert!(!here.title().is_empty());
        assert_eq!(here.mount_points.len(), 1);
        assert!(!here.device_kind().is_some() && !here.is_locked());
        for v in &vols {
            assert_eq!(v.mount_point().map(PathBuf::as_path), Some(std::path::Path::new(&v.device)));
        }
    }

    #[test]
    fn refuse_reasons_read_in_users_terms() {
        let m = remove_message(PNP_VetoOutstandingOpen, &[0u16; 261]);
        assert_eq!(m, "Files on this drive are still open somewhere.");
        let mut name = [0u16; 261];
        for (i, c) in "explorer.exe".encode_utf16().enumerate() {
            name[i] = c;
        }
        let unknown = windows::Win32::Devices::DeviceAndDriverInstallation::PNP_VetoTypeUnknown;
        assert_eq!(remove_message(unknown, &name), "Windows refused to remove it. (explorer.exe)");
    }
}
