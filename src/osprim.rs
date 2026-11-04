//! The syscall-level primitives the copy, delete, undo and trash engines share.
//!
//! Error-shape travels through `io::Error::raw_os_error` on both platforms, so the
//! `is_*` predicates below are the only place errno codes are matched. Identity uses
//! (volume, file id) pairs: dev/ino on Unix, volume serial + FileIdInfo on Windows.
//! The `\\?\` long-path prefix is applied only inside these calls (see `win::longpath`).

use std::io;
use std::path::Path;

/// Identity of a file within its filesystem: enough to spot hardlinks, to stop
/// directory walks at mount boundaries, and to detect same-file renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
    pub dev: u64,
    pub id: u128,
    pub nlink: u64,
}

// the trailing fallback is only reachable on targets that are neither Unix nor Windows
#[cfg_attr(any(unix, windows), allow(unreachable_code))]
pub fn identity(path: &Path, meta: &std::fs::Metadata) -> Identity {
    let _ = meta;
    #[cfg(unix)]
    return unix_identity(meta);
    #[cfg(windows)]
    return win_identity(path).unwrap_or(Identity { dev: 0, id: 0, nlink: 0 });
    #[cfg(not(any(unix, windows)))]
    let _ = (path, meta);
    Identity { dev: 0, id: 0, nlink: 0 }
}

/// The filesystem id holding `path`, when it can be told (queue keys, trash dedup).
pub fn device_of(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    return unix_device_of(path);
    #[cfg(windows)]
    return win_device_of(path);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// dev of the entry itself (no symlink following): moving a symlink is decided by
/// the filesystem the link lives on, not the one its target points to.
pub fn entry_device(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    return std::fs::symlink_metadata(path).ok().map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.dev()
    });
    #[cfg(windows)]
    return std::fs::symlink_metadata(path).ok().map(|m| identity(path, &m).dev);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// Bytes available on the filesystem holding `path`; `None` when the filesystem
/// reports no size at all (a server that does not tell, not a full drive).
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    return unix_free_space(path);
    #[cfg(windows)]
    return win_free_space(path);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// rename(2) that refuses to replace an existing target.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return unix_renameat(from, to, libc::RENAME_NOREPLACE);
    #[cfg(windows)]
    return win_movefile(from, to, false);
    #[cfg(not(any(unix, windows)))]
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Swap two directory entries atomically. Filesystems without swap support answer
/// unsupported (see `is_rename_flag_unsupported`), so callers can fall back to delete-first.
pub fn rename_swap(from: &Path, to: &Path) -> io::Result<()> {
    let _ = (&from, &to);
    #[cfg(unix)]
    return unix_renameat(from, to, libc::RENAME_EXCHANGE);
    #[cfg(windows)]
    return Err(io::Error::from(io::ErrorKind::Unsupported));
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (from, to);
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// (atime, mtime) as (seconds, nanoseconds) since the Unix epoch, from metadata.
pub fn times(meta: &std::fs::Metadata) -> ((i64, i64), (i64, i64)) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ((meta.atime(), meta.atime_nsec()), (meta.mtime(), meta.mtime_nsec()))
    }
    #[cfg(windows)]
    {
        let at = |t: std::io::Result<std::time::SystemTime>| -> (i64, i64) {
            let Ok(t) = t else { return (0, 0) };
            match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => (d.as_secs() as i64, d.subsec_nanos() as i64),
                Err(e) => {
                    let d = e.duration();
                    (-(d.as_secs() as i64), -(d.subsec_nanos() as i64))
                }
            }
        };
        (at(meta.accessed()), at(meta.modified()))
    }
}

/// Best-effort timestamp restore (a failed mtime never fails a copy).
/// `atime`/`mtime` are (seconds, nanoseconds) since the Unix epoch, as MetadataExt gives.
pub fn set_times(path: &Path, atime: (i64, i64), mtime: (i64, i64), follow: bool) {
    #[cfg(unix)]
    return unix_set_times(path, atime, mtime, follow);
    #[cfg(windows)]
    return win_set_times(path, atime, mtime, follow);
    #[cfg(not(any(unix, windows)))]
    let _ = (path, atime, mtime, follow);
}

/// Recreate a symbolic link (the copy engine replans link targets verbatim).
pub fn make_symlink(target: &Path, link: &Path, target_is_dir: bool) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return win_symlink(target, link, target_is_dir);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link, target_is_dir);
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

/// Permission bits as the views display them: the real mode on Unix; on Windows
/// distilled from the read-only attribute (the ACL truth comes with `win::sidname`).
#[cfg_attr(any(unix, windows), allow(unreachable_code))]
pub fn mode_of(meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    return unix_mode_of(meta);
    #[cfg(not(unix))]
    return distilled_mode_of(meta);
    0
}

#[cfg(unix)]
fn unix_mode_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode()
}

#[cfg(not(unix))]
fn distilled_mode_of(meta: &std::fs::Metadata) -> u32 {
    let ro = meta.permissions().readonly();
    if meta.is_dir() {
        if ro { 0o040555 } else { 0o040755 }
    } else if meta.file_type().is_symlink() {
        0o120777
    } else if ro {
        0o100444
    } else {
        0o100644
    }
}

/// The propagatable file attributes of a source: Windows answers the HIDDEN/SYSTEM/
/// READONLY mask (the ones a copy can safely re-set); Unix answers 0 because its
/// engine carries the mode bits instead.
pub fn attrs_of(path: &Path) -> u32 {
    #[cfg(windows)]
    {
        let w = crate::win::longpath::wide(path);
        let attrs = unsafe { windows::Win32::Storage::FileSystem::GetFileAttributesW(windows::core::PCWSTR(w.as_ptr())) };
        if attrs == windows::Win32::Storage::FileSystem::INVALID_FILE_ATTRIBUTES {
            0
        } else {
            attrs & 0x7 // READONLY | HIDDEN | SYSTEM
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        0
    }
}

/// Applies `attrs_of` output to a fresh copy, keeping whatever else the new file
/// already carries (the archive bit and friends). Best effort, like set_times.
pub fn set_attrs(path: &Path, attrs: u32) {
    #[cfg(windows)]
    {
        if attrs & 0x7 == 0 {
            return;
        }
        let w = crate::win::longpath::wide(path);
        let pcw = windows::core::PCWSTR(w.as_ptr());
        let existing = unsafe { windows::Win32::Storage::FileSystem::GetFileAttributesW(pcw) };
        if existing == windows::Win32::Storage::FileSystem::INVALID_FILE_ATTRIBUTES {
            return;
        }
        let _ = unsafe { windows::Win32::Storage::FileSystem::SetFileAttributesW(pcw, windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES((existing & !0x7) | attrs & 0x7)) };
    }
    #[cfg(not(windows))]
    let _ = (path, attrs);
}

/// A quick "may I write here" hint (permission bits only, like access(2)).
pub fn can_write(path: &Path) -> bool {
    #[cfg(unix)]
    return unix_can_write(path);
    #[cfg(windows)]
    return win_can_write(path);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

// ---- error-shape predicates (errno / Win32 codes as control flow) ----

pub fn is_cross_device(e: &io::Error) -> bool {
    codes(e, &[EXDEV_CROSS])
}

pub fn is_exists(e: &io::Error) -> bool {
    codes(e, &EXISTS_CODES)
}

pub fn is_dir_not_empty(e: &io::Error) -> bool {
    codes(e, &NOT_EMPTY_CODES)
}

pub fn is_rename_flag_unsupported(e: &io::Error) -> bool {
    codes(e, &SWAP_UNSUPPORTED_CODES)
}

/// Another holder is using the file (Explorer previews, running editors, AV scans).
pub fn is_sharing_violation(e: &io::Error) -> bool {
    codes(e, &SHARING_CODES)
}

fn codes(e: &io::Error, allowed: &[i32]) -> bool {
    e.raw_os_error().is_some_and(|c| allowed.contains(&c))
}

#[cfg(unix)]
const EXDEV_CROSS: i32 = libc::EXDEV;
#[cfg(windows)]
const EXDEV_CROSS: i32 = 17; // ERROR_NOT_SAME_DEVICE

#[cfg(unix)]
const EXISTS_CODES: [i32; 1] = [libc::EEXIST];
#[cfg(windows)]
const EXISTS_CODES: [i32; 2] = [80, 183]; // ERROR_FILE_EXISTS, ERROR_ALREADY_EXISTS

#[cfg(unix)]
const NOT_EMPTY_CODES: [i32; 1] = [libc::ENOTEMPTY];
#[cfg(windows)]
const NOT_EMPTY_CODES: [i32; 1] = [145]; // ERROR_DIR_NOT_EMPTY

#[cfg(unix)]
const SWAP_UNSUPPORTED_CODES: [i32; 3] = [libc::EINVAL, libc::ENOSYS, libc::EOPNOTSUPP];
#[cfg(windows)]
const SWAP_UNSUPPORTED_CODES: [i32; 0] = [];

#[cfg(unix)]
const SHARING_CODES: [i32; 1] = [libc::ETXTBSY];
#[cfg(windows)]
const SHARING_CODES: [i32; 2] = [32, 33]; // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION

// ---- Unix bodies (verbatim moves from the engines) ----

#[cfg(unix)]
fn unix_renameat(from: &Path, to: &Path, flags: libc::c_uint) -> io::Result<()> {
    let (f, t) = (cpath(from)?, cpath(to)?);
    let r = unsafe { libc::renameat2(libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), flags) };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn unix_set_times(path: &Path, atime: (i64, i64), mtime: (i64, i64), follow: bool) {
    let Ok(c) = cpath(path) else { return };
    let ts = [
        libc::timespec { tv_sec: atime.0, tv_nsec: atime.1 },
        libc::timespec { tv_sec: mtime.0, tv_nsec: mtime.1 },
    ];
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    unsafe {
        libc::utimensat(libc::AT_FDCWD, c.as_ptr(), ts.as_ptr(), flags);
    }
}

#[cfg(unix)]
fn unix_can_write(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(unix)]
fn unix_free_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    if st.f_blocks == 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(unix)]
fn cpath(p: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

// ---- Windows bodies ----

/// Whether both paths live on the same filesystem (move-by-rename vs copy+delete).
pub fn same_filesystem(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    return unix_same_filesystem(a, b);
    #[cfg(windows)]
    return win_same_filesystem(a, b);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (a, b);
        false
    }
}

#[cfg(unix)]
fn unix_same_filesystem(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

#[cfg(unix)]
fn unix_identity(meta: &std::fs::Metadata) -> Identity {
    use std::os::unix::fs::MetadataExt;
    Identity { dev: meta.dev(), id: meta.ino() as u128, nlink: meta.nlink() }
}

#[cfg(unix)]
fn unix_device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

#[cfg(windows)]
fn win_same_filesystem(a: &Path, b: &Path) -> bool {
    match (volume_root(a), volume_root(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[cfg(windows)]
fn volume_root(path: &Path) -> Option<String> {
    crate::win::volume::volume_root_string(path)
}

#[cfg(windows)]
fn win_movefile(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let (f, t) = (crate::win::longpath::wide(from), crate::win::longpath::wide(to));
    let flags = MOVEFILE_WRITE_THROUGH | if replace { MOVEFILE_REPLACE_EXISTING } else { Default::default() };
    unsafe { MoveFileExW(windows::core::PCWSTR(f.as_ptr()), windows::core::PCWSTR(t.as_ptr()), flags) }
        .map_err(crate::win::err::to_io)
}

#[cfg(windows)]
fn win_device_of(path: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    let w = crate::win::longpath::wide(path);
    let mut serial = 0u32;
    if unsafe { GetVolumeInformationW(windows::core::PCWSTR(w.as_ptr()), None, Some(&raw mut serial), None, None, None) }.is_ok() {
        Some(serial as u64)
    } else {
        None
    }
}

#[cfg(windows)]
fn win_free_space(path: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w = crate::win::longpath::wide(path);
    let mut free = 0u64;
    let mut total = 0u64;
    if unsafe { GetDiskFreeSpaceExW(windows::core::PCWSTR(w.as_ptr()), Some(&raw mut free), Some(&raw mut total), None) }.is_err() {
        return None;
    }
    if total == 0 {
        return None;
    }
    Some(free)
}

/// Open with no access rights at all: metadata queries only, never blocks on
/// sharing, works on directories and reparse points.
#[cfg(windows)]
fn open_query(path: &Path, extra_flags: u32) -> io::Result<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let w = crate::win::longpath::wide(path);
    unsafe {
        CreateFileW(
            windows::core::PCWSTR(w.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(extra_flags),
            None,
        )
    }
    .map_err(crate::win::err::to_io)
}

#[cfg(windows)]
fn win_identity(path: &Path) -> io::Result<Identity> {
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        GetFileInformationByHandle, GetFileInformationByHandleEx,
    };
    let h = open_query(path, FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let nlink = if unsafe { GetFileInformationByHandle(h, &mut info) }.is_ok() { info.nNumberOfLinks as u64 } else { 1 };
    let mut id = FILE_ID_INFO::default();
    let r = unsafe {
        GetFileInformationByHandleEx(
            h,
            windows::Win32::Storage::FileSystem::FileIdInfo,
            &mut id as *mut FILE_ID_INFO as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(h);
    }
    r.map_err(crate::win::err::to_io)?;
    let file_id = u128::from_le_bytes(id.FileId.Identifier);
    Ok(Identity { dev: id.VolumeSerialNumber as u64, id: file_id, nlink })
}

/// FILE_WRITE_ATTRIBUTES is enough to set timestamps and bypasses sharing checks.
#[cfg(windows)]
fn win_set_times(path: &Path, atime: (i64, i64), mtime: (i64, i64), follow: bool) {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, OPEN_EXISTING,
    };
    // FILETIME: 100ns ticks since 1601-01-01; Unix seconds + this offset = Windows epoch.
    const EPOCH_SHIFT: i64 = 11_644_473_600;
    let to_filetime = |t: (i64, i64)| -> Option<windows::Win32::Foundation::FILETIME> {
        let ticks = (t.0.checked_add(EPOCH_SHIFT)?.checked_mul(10_000_000))?.checked_add(t.1 / 100)?;
        Some(windows::Win32::Foundation::FILETIME {
            dwLowDateTime: (ticks as u64 & u32::MAX as u64) as u32,
            dwHighDateTime: (ticks as u64 >> 32) as u32,
        })
    };
    let (a, m) = (to_filetime(atime), to_filetime(mtime));
    let w = crate::win::longpath::wide(path);
    let flags = FILE_FLAGS_AND_ATTRIBUTES(
        FILE_ATTRIBUTE_NORMAL.0 | FILE_FLAG_BACKUP_SEMANTICS.0 | if follow { 0 } else { FILE_FLAG_OPEN_REPARSE_POINT.0 },
    );
    let Ok(h) = (unsafe {
        CreateFileW(
            windows::core::PCWSTR(w.as_ptr()),
            FILE_WRITE_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
    }) else {
        return;
    };
    unsafe {
        let _ = windows::Win32::Storage::FileSystem::SetFileTime(
            h,
            None,
            a.as_ref().map(|f| f as *const windows::Win32::Foundation::FILETIME),
            m.as_ref().map(|f| f as *const windows::Win32::Foundation::FILETIME),
        );
        let _ = windows::Win32::Foundation::CloseHandle(h);
    }
}

#[cfg(windows)]
fn win_symlink(target: &Path, link: &Path, target_is_dir: bool) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::{
        CreateSymbolicLinkW, SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE, SYMBOLIC_LINK_FLAG_DIRECTORY,
    };
    let (t, l) = (crate::win::longpath::wide(target), crate::win::longpath::wide(link));
    let dir = if target_is_dir { SYMBOLIC_LINK_FLAG_DIRECTORY } else { Default::default() };
    let tp = windows::core::PCWSTR(t.as_ptr());
    let lp = windows::core::PCWSTR(l.as_ptr());
    // Without Developer Mode the unprivileged flag fails; fall back to the
    // privileged path, which answers 1314 when neither is available.
    if unsafe { CreateSymbolicLinkW(tp, lp, dir | SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE) } {
        return Ok(());
    }
    if unsafe { CreateSymbolicLinkW(tp, lp, dir) } {
        return Ok(());
    }
    Err(crate::win::err::last())
}

#[cfg(windows)]
fn win_can_write(path: &Path) -> bool {
    use windows::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
    let w = crate::win::longpath::wide(path);
    let attrs = unsafe { GetFileAttributesW(windows::core::PCWSTR(w.as_ptr())) };
    attrs != INVALID_FILE_ATTRIBUTES && (attrs & windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY.0) == 0
}
