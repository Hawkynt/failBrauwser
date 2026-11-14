//! The volume a path lives on (Windows view): the mount root and its serial.

use std::path::{Path, PathBuf};

/// The volume root for any path on the volume holding `path` ("C:\\", "\\\\server\\share\\").
pub fn volume_root_string(path: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::GetVolumePathNameW;
    let w = crate::win::longpath::wide(path);
    let mut root = vec![0u16; 32768];
    if unsafe { GetVolumePathNameW(windows::core::PCWSTR(w.as_ptr()), &mut root) }.is_err() {
        return None;
    }
    let end = root.iter().position(|c| *c == 0).unwrap_or(root.len());
    Some(String::from_utf16_lossy(&root[..end]))
}

pub fn volume_root_path(path: &Path) -> Option<PathBuf> {
    volume_root_string(path).map(PathBuf::from)
}
