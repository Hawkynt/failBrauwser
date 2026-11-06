//! Disk images as drives on Windows: the twin of the UDisks2 loop mount on Linux. Windows
//! mounts ISO, IMG, VHD and VHDX files itself; this asks the shell for its own "Mount" verb
//! first — what Explorer's double-click does, allowed for standard users — and, where that
//! verb is missing (Server Core, stripped shells), attaches the image through the virtual
//! disk API, which needs administrator rights. Either way Windows gives the volume a drive
//! letter; the new letter is what the caller opens. Eject on the drives page takes it away
//! again, as Explorer's Eject does.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WIN32_ERROR};
use windows::Win32::Storage::Vhd::{
    ATTACH_VIRTUAL_DISK_FLAG_PERMANENT_LIFETIME, ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY, AttachVirtualDisk, OPEN_VIRTUAL_DISK_FLAG_NONE, OPEN_VIRTUAL_DISK_PARAMETERS, OPEN_VIRTUAL_DISK_VERSION_1, OpenVirtualDisk,
    VIRTUAL_DISK_ACCESS_ATTACH_RO, VIRTUAL_DISK_ACCESS_ATTACH_RW, VIRTUAL_DISK_ACCESS_DETACH, VIRTUAL_DISK_ACCESS_GET_INFO, VIRTUAL_STORAGE_TYPE, VIRTUAL_STORAGE_TYPE_DEVICE_UNKNOWN, VIRTUAL_STORAGE_TYPE_VENDOR_UNKNOWN,
};
use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

/// The image kinds Windows mounts by itself.
pub const MOUNTABLE: &[&str] = &[".iso", ".img", ".vhd", ".vhdx"];

/// Optical images attach read-only (they are read-only media); disk containers read-write,
/// like a loop device over a raw image on Linux.
pub fn is_optical(image: &Path) -> bool {
    let lower = image.to_string_lossy().to_lowercase();
    lower.ends_with(".iso") || lower.ends_with(".img")
}

/// Mounts `image` and answers the root of the drive it became. Blocks for up to the
/// settle time below, so callers run it off the UI thread.
pub fn mount(image: &Path) -> Result<PathBuf, String> {
    let before = super::storage::drive_mask();
    let shell = shell_mount(image);
    if let Err(shell_err) = shell {
        // Without the shell's verb the API route remains; report both if it fails too.
        attach(image).map_err(|api_err| format!("{shell_err}; {api_err}"))?;
    }
    wait_new_letter(before, Duration::from_secs(15)).ok_or_else(|| "Windows mounted the image but gave it no drive letter.".to_string())
}

/// The lowest drive letter that appeared since `before`, as its root folder.
fn wait_new_letter(before: u32, settle: Duration) -> Option<PathBuf> {
    let start = Instant::now();
    loop {
        if let Some(root) = new_letter(before, super::storage::drive_mask()) {
            return Some(root);
        }
        if start.elapsed() > settle {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The lowest letter set in `after` but not in `before` (the `GetLogicalDrives` masks).
pub fn new_letter(before: u32, after: u32) -> Option<PathBuf> {
    let fresh = after & !before;
    (0..26u32).find(|i| fresh & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The shell's "mount" verb on the file, synchronously and without shell dialogs.
fn shell_mount(image: &Path) -> Result<(), String> {
    let file = wide(image);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("mount"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| format!("The shell cannot mount it ({})", e.message()))
}

/// The virtual disk API route: open the image (type detected from its extension) and attach
/// it for the lifetime of the system, read-only for optical images.
fn attach(image: &Path) -> Result<(), String> {
    let kind = VIRTUAL_STORAGE_TYPE { DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_UNKNOWN, VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_UNKNOWN };
    let optical = is_optical(image);
    let access = if optical { VIRTUAL_DISK_ACCESS_ATTACH_RO } else { VIRTUAL_DISK_ACCESS_ATTACH_RW } | VIRTUAL_DISK_ACCESS_GET_INFO | VIRTUAL_DISK_ACCESS_DETACH;
    let mut params = OPEN_VIRTUAL_DISK_PARAMETERS { Version: OPEN_VIRTUAL_DISK_VERSION_1, ..Default::default() };
    // RWDepth 1: the default chain depth for a plain (non-differencing) image
    params.Anonymous.Version1.RWDepth = 1;
    let path = wide(image);
    let mut handle = HANDLE::default();
    let rc = unsafe { OpenVirtualDisk(&kind, PCWSTR(path.as_ptr()), access, OPEN_VIRTUAL_DISK_FLAG_NONE, Some(&params), &mut handle) };
    check(rc, "open")?;
    let flags = ATTACH_VIRTUAL_DISK_FLAG_PERMANENT_LIFETIME | if optical { ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY } else { Default::default() };
    let rc = unsafe { AttachVirtualDisk(handle, None, flags, 0, None, None) };
    unsafe {
        let _ = CloseHandle(handle);
    }
    check(rc, "attach")
}

fn check(rc: WIN32_ERROR, what: &str) -> Result<(), String> {
    if rc.0 == 0 {
        Ok(())
    } else {
        Err(format!("the virtual disk service could not {what} it ({})", std::io::Error::from_raw_os_error(rc.0 as i32)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_letter_is_the_lowest_fresh_one() {
        // C and D present before; D, E and G after (D stays, E and G are new)
        let before = (1 << 2) | (1 << 3);
        let after = (1 << 3) | (1 << 4) | (1 << 6);
        assert_eq!(new_letter(before, after), Some(PathBuf::from("E:\\")));
        assert_eq!(new_letter(after, after), None, "nothing new, nothing answered");
        assert_eq!(new_letter(0, 1 << 25), Some(PathBuf::from("Z:\\")));
    }

    #[test]
    fn optical_images_attach_read_only() {
        assert!(is_optical(Path::new(r"C:\x\Setup.ISO")));
        assert!(is_optical(Path::new(r"C:\x\floppy.img")));
        assert!(!is_optical(Path::new(r"C:\x\disk.vhdx")));
    }

}
