//! Backup Mode (README: Links, *Backup Mode*): what a copy keeps beyond the bytes. On Linux the
//! owner and group, the permission bits (setuid, setgid and sticky too), every extended attribute
//! (access control lists live in `system.posix_acl_*`, SELinux labels and file capabilities in
//! `security.*`), the access and modification times, and the holes of sparse files. Run as the
//! administrator (see `helper`), it reads what the user cannot. On Windows the backup and restore
//! privileges are enabled and the security descriptor (owner, group, DACL, SACL) is copied;
//! `CopyFileExW` already carries alternate streams, extended attributes and attributes
//! (Microsoft's documentation, unmeasured here: no Windows machine runs the tests).

use std::io;
use std::path::Path;

/// Copies the bytes of the file `src` to the new file `dst`, keeping holes: only the data
/// regions `SEEK_DATA` / `SEEK_HOLE` report are written, the length is set at the end.
#[cfg(unix)]
pub fn copy_data(src: &Path, dst: &Path) -> io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::io::AsRawFd;
    let mut from = std::fs::File::open(src)?;
    let len = from.metadata()?.len();
    let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
    let fd = from.as_raw_fd();
    let mut pos: i64 = 0;
    let mut buf = vec![0u8; 1 << 20];
    while (pos as u64) < len {
        // SAFETY: a file descriptor this function owns.
        let data = unsafe { libc::lseek(fd, pos, libc::SEEK_DATA) };
        if data < 0 {
            let e = io::Error::last_os_error();
            match e.raw_os_error() {
                // No data after `pos`: the rest is a hole.
                Some(libc::ENXIO) => break,
                // A filesystem without hole reporting: everything is data.
                Some(libc::EINVAL) | Some(libc::EOPNOTSUPP) if pos == 0 => return plain(&mut from, &mut to),
                _ => return Err(e),
            }
        }
        // SAFETY: as above.
        let hole = unsafe { libc::lseek(fd, data, libc::SEEK_HOLE) };
        let end = if hole < 0 { len as i64 } else { hole };
        from.seek(SeekFrom::Start(data as u64))?;
        to.seek(SeekFrom::Start(data as u64))?;
        let mut left = (end - data) as u64;
        while left > 0 {
            let want = left.min(buf.len() as u64) as usize;
            let n = from.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            to.write_all(&buf[..n])?;
            left -= n as u64;
        }
        pos = end;
    }
    to.set_len(len)?;
    Ok(())
}

#[cfg(unix)]
fn plain(from: &mut std::fs::File, to: &mut std::fs::File) -> io::Result<()> {
    use std::io::Seek;
    from.rewind()?;
    to.rewind()?;
    io::copy(from, to).map(|_| ())
}

#[cfg(windows)]
pub fn copy_data(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::copy(src, dst).map(|_| ())
}

/// Gives `dst` what `src` carries besides its bytes (owner, group, permissions, extended
/// attributes, times); `src` and `dst` are taken as themselves when they are links.
pub fn copy_meta(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::copy_meta(src, dst)
    }
    #[cfg(windows)]
    {
        win::copy_security(src, dst)?;
        let meta = std::fs::symlink_metadata(src)?;
        let (a, m) = crate::osprim::times(&meta);
        crate::osprim::set_times(dst, a, m, false);
        Ok(())
    }
}

/// The extended attributes of `path` itself (not followed), sorted by name.
#[cfg(unix)]
pub fn xattrs(path: &Path) -> io::Result<Vec<(std::ffi::OsString, Vec<u8>)>> {
    unix::xattrs(path)
}

/// Enables the backup and restore privileges of this process where the system has them (a
/// no-op on Linux, where running as the administrator is what counts).
pub fn enable_privileges() -> io::Result<()> {
    #[cfg(windows)]
    {
        win::enable_privileges()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

/// Whether `src` and `dst` have the same owner and group (always on Windows: the security
/// descriptor is not compared).
pub fn same_owner(src: &std::fs::Metadata, dst: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        src.uid() == dst.uid() && src.gid() == dst.gid()
    }
    #[cfg(not(unix))]
    {
        let _ = (src, dst);
        true
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::{CString, OsString};
    use std::io;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    fn c(p: &Path) -> io::Result<CString> {
        CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
    }

    pub fn copy_meta(src: &Path, dst: &Path) -> io::Result<()> {
        let meta = std::fs::symlink_metadata(src)?;
        let link = meta.file_type().is_symlink();
        let d = c(dst)?;
        let now = std::fs::symlink_metadata(dst)?;
        if now.uid() != meta.uid() || now.gid() != meta.gid() {
            // SAFETY: a NUL-terminated path.
            if unsafe { libc::lchown(d.as_ptr(), meta.uid(), meta.gid()) } != 0 {
                return Err(with_path(io::Error::last_os_error(), "cannot set owner of", dst));
            }
        }
        // After the owner: a change of owner clears setuid and setgid.
        if !link {
            // SAFETY: as above.
            if unsafe { libc::chmod(d.as_ptr(), meta.mode() & 0o7777) } != 0 {
                return Err(with_path(io::Error::last_os_error(), "cannot set permissions of", dst));
            }
        }
        // After the owner too: a change of owner drops `security.capability`.
        let want = xattrs(src)?;
        for (name, _) in xattrs(dst)? {
            if !want.iter().any(|(n, _)| *n == name) {
                let n = CString::new(name.as_bytes()).unwrap_or_default();
                // SAFETY: NUL-terminated path and name.
                unsafe { libc::lremovexattr(d.as_ptr(), n.as_ptr()) };
            }
        }
        for (name, value) in &want {
            let n = CString::new(name.as_bytes()).unwrap_or_default();
            // SAFETY: NUL-terminated path and name, a value buffer of the length given.
            if unsafe { libc::lsetxattr(d.as_ptr(), n.as_ptr(), value.as_ptr().cast(), value.len(), 0) } != 0 {
                let e = io::Error::last_os_error();
                return Err(with_path(e, &format!("cannot set {} of", name.to_string_lossy()), dst));
            }
        }
        let (a, m) = crate::osprim::times(&meta);
        crate::osprim::set_times(dst, a, m, false);
        Ok(())
    }

    fn with_path(e: io::Error, what: &str, p: &Path) -> io::Error {
        io::Error::new(e.kind(), format!("{what} {}: {e}", p.display()))
    }

    pub fn xattrs(path: &Path) -> io::Result<Vec<(OsString, Vec<u8>)>> {
        let p = c(path)?;
        let mut names = vec![0u8; 4096];
        let n = loop {
            // SAFETY: a NUL-terminated path and a buffer of the length given.
            let n = unsafe { libc::llistxattr(p.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
            if n >= 0 {
                break n as usize;
            }
            let e = io::Error::last_os_error();
            match e.raw_os_error() {
                Some(libc::ERANGE) => names.resize(names.len() * 4, 0),
                Some(libc::ENOTSUP) => return Ok(Vec::new()),
                _ => return Err(e),
            }
        };
        let mut out = Vec::new();
        for name in names[..n].split(|b| *b == 0).filter(|s| !s.is_empty()) {
            let cn = CString::new(name).unwrap_or_default();
            let mut value = vec![0u8; 256];
            loop {
                // SAFETY: NUL-terminated path and name, a buffer of the length given.
                let got = unsafe { libc::lgetxattr(p.as_ptr(), cn.as_ptr(), value.as_mut_ptr().cast(), value.len()) };
                if got >= 0 {
                    value.truncate(got as usize);
                    out.push((OsString::from_vec(name.to_vec()), value));
                    break;
                }
                let e = io::Error::last_os_error();
                match e.raw_os_error() {
                    Some(libc::ERANGE) => value.resize(value.len().max(1) * 16, 0),
                    // Gone meanwhile.
                    Some(libc::ENODATA) => break,
                    _ => return Err(e),
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree, LUID};
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, GetSecurityDescriptorDacl, GetSecurityDescriptorGroup, GetSecurityDescriptorOwner, GetSecurityDescriptorSacl, LookupPrivilegeValueW, ACL, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, LUID_AND_ATTRIBUTES, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SACL_SECURITY_INFORMATION, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    /// SeBackupPrivilege and SeRestorePrivilege, which an administrator holds but has disabled.
    pub fn enable_privileges() -> io::Result<()> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).map_err(crate::win::err::to_io)?;
            let mut r = Ok(());
            for name in ["SeBackupPrivilege", "SeRestorePrivilege", "SeSecurityPrivilege"] {
                let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
                let mut luid = LUID::default();
                if let Err(e) = LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(w.as_ptr()), &mut luid) {
                    r = Err(crate::win::err::to_io(e));
                    continue;
                }
                let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
                if let Err(e) = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None) {
                    r = Err(crate::win::err::to_io(e));
                }
            }
            let _ = CloseHandle(token);
            r
        }
    }

    /// The owner, group, DACL and SACL of `src` set on `dst`.
    pub fn copy_security(src: &Path, dst: &Path) -> io::Result<()> {
        let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | SACL_SECURITY_INFORMATION;
        let (s, d) = (wide(src), wide(dst));
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let got = GetNamedSecurityInfoW(PCWSTR(s.as_ptr()), SE_FILE_OBJECT, info, None, None, None, None, &raw mut sd);
            if got != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(got.0 as i32));
            }
            let (mut owner, mut group) = (PSID::default(), PSID::default());
            let (mut dacl, mut sacl): (*mut ACL, *mut ACL) = (std::ptr::null_mut(), std::ptr::null_mut());
            let (mut f1, mut f2, mut p1, mut p2) = Default::default();
            let read = GetSecurityDescriptorOwner(sd, &raw mut owner, &raw mut f1)
                .and(GetSecurityDescriptorGroup(sd, &raw mut group, &raw mut f2))
                .and(GetSecurityDescriptorDacl(sd, &raw mut p1, &raw mut dacl, &raw mut f1))
                .and(GetSecurityDescriptorSacl(sd, &raw mut p2, &raw mut sacl, &raw mut f2));
            let set = read.map_err(crate::win::err::to_io).and_then(|()| {
                let r = SetNamedSecurityInfoW(PCWSTR(d.as_ptr()), SE_FILE_OBJECT, info, owner, group, Some(dacl), Some(sacl));
                if r == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(r.0 as i32)) }
            });
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            set
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn a_sparse_file_keeps_its_holes_and_its_bytes() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("sparse"), t.path().join("copy"));
        {
            use std::io::{Seek, SeekFrom, Write};
            let mut f = std::fs::File::create(&src).unwrap();
            f.write_all(b"head").unwrap();
            f.seek(SeekFrom::Start(64 << 20)).unwrap();
            f.write_all(b"tail").unwrap();
        }
        copy_data(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&dst).unwrap());
        let (s, d) = (std::fs::metadata(&src).unwrap(), std::fs::metadata(&dst).unwrap());
        // Only where the source itself is sparse can the copy be measured against it.
        if s.blocks() * 512 < s.len() / 2 {
            assert!(d.blocks() * 512 < d.len() / 2, "the copy filled its holes: {} blocks", d.blocks());
        }
    }

    #[test]
    fn permissions_attributes_and_times_go_with_the_copy() {
        let t = tempfile::tempdir().unwrap();
        let (src, dst) = (t.path().join("a"), t.path().join("b"));
        std::fs::write(&src, "x").unwrap();
        std::fs::write(&dst, "x").unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o2741)).unwrap();
        let set = |p: &Path, n: &str, v: &[u8]| {
            let (p, n) = (std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap(), std::ffi::CString::new(n).unwrap());
            unsafe { libc::lsetxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0) == 0 }
        };
        let xattr_fs = set(&src, "user.fb.kept", b"yes");
        if xattr_fs {
            assert!(set(&dst, "user.fb.stale", b"gone"));
        }
        crate::osprim::set_times(&src, (1_000_000, 5), (2_000_000, 7), false);
        copy_meta(&src, &dst).unwrap();
        let (s, d) = (std::fs::metadata(&src).unwrap(), std::fs::metadata(&dst).unwrap());
        assert_eq!(d.mode() & 0o7777, s.mode() & 0o7777);
        assert_eq!((d.mtime(), d.mtime_nsec()), (2_000_000, 7));
        assert_eq!((d.atime(), d.atime_nsec()), (1_000_000, 5));
        if xattr_fs {
            assert_eq!(xattrs(&dst).unwrap(), [("user.fb.kept".into(), b"yes".to_vec())]);
        }
    }
}
