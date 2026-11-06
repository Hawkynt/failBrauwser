//! Copying the bytes of one file as fast as the kernel allows.
//!
//! In order of preference: a reflink (`FICLONE`, instant on btrfs/xfs/bcachefs), in-kernel
//! `copy_file_range` (no user-space copy, server-side on NFS 4.2/SMB), and plain read/write.
//!
//! With `throttle` set (slow targets such as USB sticks), data is pushed to the device in
//! fixed windows: the previous window is waited for and dropped from the page cache before
//! the next one starts. That keeps only a few megabytes dirty, so the progress bar shows the
//! real device speed, other programs do not stall behind a huge writeback, and unmounting
//! afterwards is instant instead of freezing for minutes.

use super::job::JobCtx;
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};

/// Writeback window for throttled copies.
const WINDOW: u64 = 8 << 20;
/// Chunk size for unthrottled `copy_file_range` (keeps cancel and progress responsive).
#[cfg(unix)]
const CHUNK: u64 = 32 << 20;
/// Size of the fallback read/write buffer.
pub const BUF_SIZE: usize = 1 << 20;
/// Files at least this large get `fallocate` (less fragmentation, early ENOSPC).
#[cfg(unix)]
const PREALLOC_MIN: u64 = 1 << 20;

pub fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

pub fn is_cancelled_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::Interrupted && e.to_string() == "cancelled"
}

/// Retries after a transient sharing block (an AV scan, an Explorer preview, a laggy
/// close): four attempts at 100/200/400/800 ms before the caller involves the user.
/// The attempt counter is the caller's; `true` means "try again now".
///
/// A Windows ladder only — Unix engines asked the user straight away and still do.
pub fn wait_sharing_retry(e: &io::Error, attempt: &mut u32) -> bool {
    #[cfg(windows)]
    {
        if !crate::osprim::is_sharing_violation(e) || *attempt >= SHARING_TRIES {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100 << *attempt));
        *attempt += 1;
        true
    }
    #[cfg(not(windows))]
    {
        let _ = (e, attempt);
        false
    }
}

#[cfg(windows)]
const SHARING_TRIES: u32 = 4;

#[cfg(unix)]
fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

/// Tracks what has been written and paces writeback for throttled copies.
#[cfg(unix)]
struct Pacer {
    fd: RawFd,
    src: RawFd,
    enabled: bool,
    /// Start of the window currently being filled.
    window_start: u64,
    /// The window handed to writeback, not yet waited for.
    in_flight: Option<(u64, u64)>,
}

#[cfg(unix)]
impl Pacer {
    fn advance(&mut self, pos: u64) {
        if !self.enabled || pos - self.window_start < WINDOW {
            return;
        }
        let len = pos - self.window_start;
        unsafe {
            // Start writing this window out now…
            libc::sync_file_range(self.fd, self.window_start as i64, len as i64, libc::SYNC_FILE_RANGE_WRITE);
            // …and wait for the one before, then forget its pages.
            if let Some((s, l)) = self.in_flight.take() {
                libc::sync_file_range(
                    self.fd,
                    s as i64,
                    l as i64,
                    libc::SYNC_FILE_RANGE_WAIT_BEFORE | libc::SYNC_FILE_RANGE_WRITE | libc::SYNC_FILE_RANGE_WAIT_AFTER,
                );
                libc::posix_fadvise(self.fd, s as i64, l as i64, libc::POSIX_FADV_DONTNEED);
                libc::posix_fadvise(self.src, s as i64, l as i64, libc::POSIX_FADV_DONTNEED);
            }
        }
        self.in_flight = Some((self.window_start, len));
        self.window_start = pos;
    }

    fn finish(&mut self, pos: u64) {
        if !self.enabled {
            return;
        }
        unsafe {
            let flags = libc::SYNC_FILE_RANGE_WAIT_BEFORE | libc::SYNC_FILE_RANGE_WRITE | libc::SYNC_FILE_RANGE_WAIT_AFTER;
            if let Some((s, l)) = self.in_flight.take() {
                libc::sync_file_range(self.fd, s as i64, l as i64, flags);
                libc::posix_fadvise(self.fd, s as i64, l as i64, libc::POSIX_FADV_DONTNEED);
            }
            let len = pos.saturating_sub(self.window_start);
            if len > 0 {
                libc::sync_file_range(self.fd, self.window_start as i64, len as i64, flags);
                libc::posix_fadvise(self.fd, self.window_start as i64, len as i64, libc::POSIX_FADV_DONTNEED);
            }
        }
    }
}

/// Which mechanism ended up copying a file (for tests and statistics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Reflink,
    CopyFileRange,
    ReadWrite,
    Empty,
}

/// Copies `size` bytes from `src` to the freshly created, empty `dst`.
/// `buf` is the caller's reusable buffer for the read/write fallback.
pub fn copy_contents(
    ctx: &JobCtx,
    src: &std::fs::File,
    dst: &std::fs::File,
    size: u64,
    throttle: bool,
    buf: &mut Vec<u8>,
    counted: &mut u64,
) -> io::Result<Method> {
    #[cfg(unix)]
    return unix_copy_contents(ctx, src, dst, size, throttle, buf, counted);
    #[cfg(windows)]
    return windows_copy_contents(ctx, src, dst, size, throttle, buf, counted);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (ctx, src, dst, size, throttle, buf, counted);
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(unix)]
fn unix_copy_contents(
    ctx: &JobCtx,
    src: &std::fs::File,
    dst: &std::fs::File,
    size: u64,
    throttle: bool,
    buf: &mut Vec<u8>,
    counted: &mut u64,
) -> io::Result<Method> {
    let (sfd, dfd) = (src.as_raw_fd(), dst.as_raw_fd());
    if size == 0 {
        return Ok(Method::Empty);
    }
    if unsafe { libc::ioctl(dfd, libc::FICLONE, sfd) } == 0 {
        ctx.add_bytes(size);
        *counted += size;
        return Ok(Method::Reflink);
    }
    if size >= PREALLOC_MIN {
        unsafe {
            libc::posix_fadvise(sfd, 0, 0, libc::POSIX_FADV_SEQUENTIAL);
            // Best effort: not every filesystem can preallocate.
            libc::fallocate(dfd, 0, 0, size as i64);
        }
    }
    let mut pacer = Pacer { fd: dfd, src: sfd, enabled: throttle, window_start: 0, in_flight: None };
    let mut pos: u64 = 0;
    let mut method = Method::CopyFileRange;
    let step = if throttle { WINDOW } else { CHUNK };
    loop {
        ctx.wait_while_paused();
        if ctx.is_cancelled() {
            return Err(cancelled());
        }
        let want = step.min(size.saturating_sub(pos).max(1)) as usize;
        let n = if method == Method::CopyFileRange {
            let r = unsafe { libc::copy_file_range(sfd, std::ptr::null_mut(), dfd, std::ptr::null_mut(), want, 0) };
            if r < 0 {
                let e = io::Error::last_os_error();
                match e.raw_os_error() {
                    Some(libc::EINTR) => continue,
                    // Not possible between these files: fall back, continuing at the same offsets.
                    Some(libc::EXDEV | libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP | libc::EBADF | libc::EPERM | libc::ETXTBSY) => {
                        method = Method::ReadWrite;
                        continue;
                    }
                    _ => return Err(e),
                }
            }
            r as usize
        } else {
            if buf.len() < BUF_SIZE {
                buf.resize(BUF_SIZE, 0);
            }
            let want = want.min(buf.len());
            let r = unsafe { libc::read(sfd, buf.as_mut_ptr().cast(), want) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(e);
            }
            let r = r as usize;
            let mut off = 0;
            while off < r {
                let w = unsafe { libc::write(dfd, buf[off..r].as_ptr().cast(), r - off) };
                if w < 0 {
                    let e = io::Error::last_os_error();
                    if e.raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    return Err(e);
                }
                off += w as usize;
            }
            r
        };
        if n == 0 {
            break; // End of file (it may have shrunk while we copied).
        }
        pos += n as u64;
        ctx.add_bytes(n as u64);
        *counted += n as u64;
        pacer.advance(pos);
    }
    // Preallocation may have left the file longer than what was actually copied.
    if pos != size {
        cvt(unsafe { libc::ftruncate(dfd, pos as i64) })?;
    }
    pacer.finish(pos);
    Ok(method)
}

/// The Windows engine: reflink by `FSCTL_DUPLICATE_EXTENTS_TO_FILE` (instant on ReFS,
/// refused elsewhere and then never mentioned again), and a read/write loop with the
/// same cancel/pause/progress hooks. Throttled (slow, removable) targets get a
/// `FlushFileBuffers` every window so the progress bar tracks the device and little
/// stays dirty; fixed targets run from the cache — no `sync_file_range` twin exists,
/// and stating that honestly beats faking it.
#[cfg(windows)]
fn windows_copy_contents(
    ctx: &JobCtx,
    src: &std::fs::File,
    dst: &std::fs::File,
    size: u64,
    throttle: bool,
    buf: &mut Vec<u8>,
    counted: &mut u64,
) -> io::Result<Method> {
    use std::io::{Read, Write};
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::FlushFileBuffers;
    if size == 0 {
        return Ok(Method::Empty);
    }
    if size >= 4096 && refs_extents(src, dst, size) {
        ctx.add_bytes(size);
        *counted += size;
        return Ok(Method::Reflink);
    }
    if buf.len() < BUF_SIZE {
        buf.resize(BUF_SIZE, 0);
    }
    // The Read/Write traits want &mut; the handles behind these clones are the same ones.
    let mut r = src.try_clone()?;
    let mut w = dst.try_clone()?;
    let dh = HANDLE(dst.as_raw_handle());
    let flush = || {
        let _ = unsafe { FlushFileBuffers(dh) };
    };
    let mut pos: u64 = 0;
    let mut window_start = 0u64;
    loop {
        ctx.wait_while_paused();
        if ctx.is_cancelled() {
            return Err(cancelled());
        }
        let want = buf.len().min((size - pos) as usize);
        let n = match r.read(&mut buf[..want]) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            break; // End of file (it may have shrunk while we copied).
        }
        let mut off = 0;
        while off < n {
            match w.write(&buf[off..n]) {
                Ok(k) if k > 0 => off += k,
                Ok(_) => return Err(io::Error::from(io::ErrorKind::WriteZero)),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        pos += n as u64;
        ctx.add_bytes(n as u64);
        *counted += n as u64;
        if throttle && pos - window_start >= WINDOW {
            flush();
            window_start = pos;
        }
    }
    // The source may have been shorter than promised: end where we actually got to.
    if pos != size {
        dst.set_len(pos)?;
    }
    if throttle {
        flush();
    }
    Ok(Method::ReadWrite)
}

/// Asks ReFS (and only ReFS — both volumes must be on it) to share the source's
/// clusters with the freshly created empty destination. Every refusal (wrong
/// filesystem, compressed or hard-linked source, …) answers false and the
/// read/write loop takes over; nothing is retried or logged.
#[cfg(windows)]
fn refs_extents(src: &std::fs::File, dst: &std::fs::File, size: u64) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Ioctl::{DUPLICATE_EXTENTS_DATA, FSCTL_DUPLICATE_EXTENTS_TO_FILE};
    if !fs_is_refs(src) || !fs_is_refs(dst) {
        return false;
    }
    let data = DUPLICATE_EXTENTS_DATA { FileHandle: HANDLE(src.as_raw_handle()), SourceFileOffset: 0, TargetFileOffset: 0, ByteCount: size as i64 };
    unsafe {
        windows::Win32::System::IO::DeviceIoControl(
            HANDLE(dst.as_raw_handle()),
            FSCTL_DUPLICATE_EXTENTS_TO_FILE,
            Some(&data as *const DUPLICATE_EXTENTS_DATA as *const core::ffi::c_void),
            std::mem::size_of::<DUPLICATE_EXTENTS_DATA>() as u32,
            None,
            0,
            None,
            None,
        )
    }
    .is_ok()
}

#[cfg(windows)]
fn fs_is_refs(f: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
    let mut name = [0u16; 26];
    let h = windows::Win32::Foundation::HANDLE(f.as_raw_handle());
    unsafe { GetVolumeInformationByHandleW(h, None, None, None, None, Some(&mut name)) }
        .map(|()| String::from_utf16_lossy(&name).trim_end_matches('\0').eq_ignore_ascii_case("refs"))
        .unwrap_or(false)
}

/// Reads a file back through `FILE_FLAG_NO_BUFFERING`, i.e. from the device: the
/// verification pass then cannot be answered by the very cache the copy just filled.
/// Only sector-aligned amounts are read; the sub-sector tail is read normally.
#[cfg(windows)]
pub struct UncachedReader {
    h: windows::Win32::Foundation::HANDLE,
    buf: Vec<u8>,
    off: usize,
}

#[cfg(windows)]
impl UncachedReader {
    pub const CAP: usize = 1 << 20;

    pub fn open(path: &std::path::Path) -> io::Result<Self> {
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_NO_BUFFERING, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        };
        let w = crate::win::longpath::wide(path);
        let h = unsafe {
            CreateFileW(
                windows::core::PCWSTR(w.as_ptr()),
                FILE_GENERIC_READ.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_NO_BUFFERING.0),
                None,
            )
        }
        .map_err(crate::win::err::to_io)?;
        // malloc gives 8-byte pointers; NO_BUFFERING wants the read buffer sector-aligned.
        let buf = vec![0u8; Self::CAP + 4096];
        let off = (buf.as_ptr() as usize).next_multiple_of(4096) - buf.as_ptr() as usize;
        Ok(Self { h, buf, off })
    }

    /// Reads `out.len()` bytes (a sector multiple, within one sector of the cap) into `out`.
    pub fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        use windows::Win32::Storage::FileSystem::ReadFile;
        debug_assert!(out.len() <= Self::CAP);
        let block = unsafe { std::slice::from_raw_parts_mut(self.buf.as_mut_ptr().add(self.off), out.len()) };
        let mut done = 0u32;
        match unsafe { ReadFile(self.h, Some(&mut *block), Some(&raw mut done), None) } {
            Ok(()) => {
                let n = done as usize;
                out[..n].copy_from_slice(&block[..n]);
                Ok(n)
            }
            // ERROR_HANDLE_EOF: no whole sector left at this offset.
            Err(e) if e.code().0 as u32 & 0xFFFF == 0x26 => Ok(0),
            Err(e) => Err(io::Error::other(format!("uncached read failed: {e}"))),
        }
    }
}

#[cfg(windows)]
impl Drop for UncachedReader {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.h);
        }
    }
}

/// The physical sector size of the volume holding `path` (NO_BUFFERING granularity);
/// 512 when the query fails, which is the minimum NTFS answer.
#[cfg(windows)]
pub fn sector_size(path: &std::path::Path) -> u32 {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceW;
    let w = crate::win::longpath::wide(path);
    let mut bytes = 0u32;
    if unsafe { GetDiskFreeSpaceW(windows::core::PCWSTR(w.as_ptr()), None, Some(&raw mut bytes), None, None) }.is_ok() && bytes > 0 {
        bytes
    } else {
        512
    }
}

/// Flushes everything cached for the filesystem holding `dir` (after copies to slow drives).
#[cfg_attr(any(unix, windows), allow(unreachable_code))]
pub fn syncfs(dir: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    return unix_syncfs(dir);
    #[cfg(windows)]
    return win_syncfs(dir);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(unix)]
fn unix_syncfs(dir: &std::path::Path) -> io::Result<()> {
    let f = std::fs::File::open(dir)?;
    cvt(unsafe { libc::syncfs(f.as_raw_fd()) }).map(|_| ())
}

/// No syncfs(2) twin: opening the volume root with backup semantics and flushing it
/// pushes everything the filesystem has cached for that volume.
#[cfg(windows)]
fn win_syncfs(dir: &std::path::Path) -> io::Result<()> {
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_SHARE_DELETE, OPEN_EXISTING,
    };
    let root = crate::win::volume::volume_root_path(dir).unwrap_or_else(|| dir.to_path_buf());
    let w = crate::win::longpath::wide(&root);
    let h = (unsafe {
        windows::Win32::Storage::FileSystem::CreateFileW(
            windows::core::PCWSTR(w.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_BACKUP_SEMANTICS.0),
            None,
        )
    })
    .map_err(crate::win::err::to_io)?;
    let r = unsafe { windows::Win32::Storage::FileSystem::FlushFileBuffers(h) };
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(h);
    }
    r.map_err(crate::win::err::to_io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn roundtrip(size: usize, throttle: bool) -> Method {
        let dir = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
        let sp = dir.path().join("s");
        std::fs::File::create(&sp).unwrap().write_all(&data).unwrap();
        let src = std::fs::File::open(&sp).unwrap();
        let dst = std::fs::File::create(dir.path().join("d")).unwrap();
        let ctx = JobCtx::new();
        let mut buf = Vec::new();
        let m = copy_contents(&ctx, &src, &dst, size as u64, throttle, &mut buf, &mut 0).unwrap();
        drop(dst);
        let mut out = Vec::new();
        std::fs::File::open(dir.path().join("d")).unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
        assert_eq!(ctx.snapshot().bytes_done, size as u64);
        m
    }

    #[test]
    fn copies_small_medium_and_large_files() {
        assert_eq!(roundtrip(0, false), Method::Empty);
        roundtrip(1, false);
        roundtrip(100_000, false);
        roundtrip(20 << 20, true);
        roundtrip((3 << 20) + 17, false);
    }

    #[test]
    fn source_shorter_than_expected_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s"), vec![1u8; 10]).unwrap();
        let src = std::fs::File::open(dir.path().join("s")).unwrap();
        let dst = std::fs::File::create(dir.path().join("d")).unwrap();
        let ctx = JobCtx::new();
        copy_contents(&ctx, &src, &dst, 5 << 20, false, &mut Vec::new(), &mut 0).unwrap();
        assert_eq!(std::fs::metadata(dir.path().join("d")).unwrap().len(), 10);
    }

    #[test]
    #[cfg(windows)] // the sharing ladder: four waits (100/200/400/800 ms), then the caller decides
    fn sharing_violations_get_four_fast_retries() {
        let e = io::Error::from_raw_os_error(32);
        let mut a = 0u32;
        for _ in 0..4 {
            assert!(wait_sharing_retry(&e, &mut a));
        }
        assert!(!wait_sharing_retry(&e, &mut a), "after the fourth wait the user is asked");
        assert_eq!(a, 4);
        assert!(!wait_sharing_retry(&io::Error::from_raw_os_error(5), &mut 0), "other errors are asked about right away");
    }

    #[test]
    #[cfg(windows)] // C: here is NTFS: the extent attempt answers false and the plain loop copies
    fn the_extent_attempt_is_quiet_on_other_filesystems() {
        assert_eq!(roundtrip(1 << 12, false), Method::ReadWrite);
    }

    #[test]
    #[cfg(windows)] // NO_BUFFERING reads return exactly the file's bytes, sector by sector
    fn uncached_reader_sees_what_the_device_holds() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("u.bin");
        let data: Vec<u8> = (0..(5000usize)).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let sector = sector_size(&p).max(1) as usize;
        let bulk = data.len() - data.len() % sector;
        let mut r = UncachedReader::open(&p).unwrap();
        let mut got = vec![0u8; bulk];
        let mut off = 0;
        while off < bulk {
            let n = r.read(&mut got[off..off + (UncachedReader::CAP).min(bulk - off)]).unwrap();
            assert!(n > 0, "an uncached read inside the bulk must not stop early");
            off += n;
        }
        assert_eq!(got, data[..bulk]);
    }

    #[test]
    fn cancel_stops_copy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("s"), vec![1u8; 4 << 20]).unwrap();
        let src = std::fs::File::open(dir.path().join("s")).unwrap();
        let dst = std::fs::File::create(dir.path().join("d")).unwrap();
        let ctx = JobCtx::new();
        ctx.cancel();
        let e = copy_contents(&ctx, &src, &dst, 4 << 20, false, &mut Vec::new(), &mut 0).unwrap_err();
        assert!(is_cancelled_error(&e));
    }
}
