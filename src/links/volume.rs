//! Volume Mountpoints (README: Links, *Volume Mountpoints*): a whole local drive shown in a new
//! folder. On Linux the drive's filesystem is bind-mounted there (`mount(2)` with `MS_BIND`, not
//! recursive: the drive, not what is mounted below its other places), as the administrator
//! through the helper. On Windows the folder gets the volume's `\\?\Volume{…}\` name through
//! `SetVolumeMountPointW` and loses it through `DeleteVolumeMountPointW` (Microsoft's
//! documentation, unmeasured here: no Windows machine runs the tests).

use crate::ops::names::free_name;
use std::io;
use std::path::{Path, PathBuf};

/// One line of `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// What is mounted: `/dev/sdb1`, `host:/export`, `//server/share`, …
    pub source: String,
    pub fs_type: String,
    /// The folder of the filesystem shown there (`/` for the whole filesystem).
    pub root: PathBuf,
    /// Where it is shown.
    pub at: PathBuf,
}

impl Mount {
    /// A local drive: a block device, never a network share or a FUSE filesystem.
    pub fn is_local(&self) -> bool {
        self.source.starts_with("/dev/")
    }

    /// A local drive as a whole.
    pub fn is_volume(&self) -> bool {
        self.is_local() && self.root == Path::new("/")
    }
}

/// The mounts of `/proc/self/mountinfo` text, in the order they were made.
pub fn parse(text: &str) -> Vec<Mount> {
    use crate::drives::unescape_mount as un;
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let l: Vec<&str> = left.split(' ').collect();
            let r: Vec<&str> = right.split(' ').collect();
            (l.len() >= 5 && r.len() >= 2).then(|| Mount { source: un(r[1]), fs_type: r[0].to_string(), root: PathBuf::from(un(l[3])), at: PathBuf::from(un(l[4])) })
        })
        .collect()
}

/// The mounts of this system now (none off Linux).
pub fn mounts() -> Vec<Mount> {
    std::fs::read_to_string("/proc/self/mountinfo").map(|t| parse(&t)).unwrap_or_default()
}

/// What is shown at `path`: the mount made there last, when one is.
pub fn mount_at<'a>(mounts: &'a [Mount], path: &Path) -> Option<&'a Mount> {
    mounts.iter().rev().find(|m| m.at == path)
}

/// The local drive whose top folder `path` (a real path) is.
pub fn volume_at<'a>(mounts: &'a [Mount], path: &Path) -> Option<&'a Mount> {
    mount_at(mounts, path).filter(|m| m.is_volume())
}

/// The other places the drive mounted as `m` is shown at.
pub fn other_places(mounts: &[Mount], m: &Mount) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = mounts.iter().filter(|o| o.source == m.source && o.root == m.root && o.at != m.at && mount_at(mounts, &o.at) == Some(o)).map(|o| o.at.clone()).collect();
    out.sort();
    out.dedup();
    out
}

/// Whether `path` is the top folder of a local drive, which can be dropped as a Volume
/// Mountpoint.
pub fn is_volume_root(path: &Path) -> bool {
    #[cfg(windows)]
    {
        win::volume_name(path).is_some()
    }
    #[cfg(not(windows))]
    {
        std::fs::canonicalize(path).is_ok_and(|p| volume_at(&mounts(), &p).is_some())
    }
}

/// Whether the folder `path` is a Volume Mountpoint that can be unmounted: a local drive shown
/// there that is shown somewhere else too (its last place is the drive's own to unmount).
pub fn is_mountpoint(path: &Path) -> bool {
    #[cfg(windows)]
    {
        win::is_mountpoint(path)
    }
    #[cfg(not(windows))]
    {
        let all = mounts();
        std::fs::canonicalize(path).ok().and_then(|p| volume_at(&all, &p).cloned()).is_some_and(|m| !other_places(&all, &m).is_empty())
    }
}

/// The drive shown in the folder `path` and its other places, for the *Link* tab.
pub fn shown_at(path: &Path) -> Option<(String, Vec<PathBuf>)> {
    #[cfg(windows)]
    {
        win::mounted_volume(path).map(|v| (v, Vec::new()))
    }
    #[cfg(not(windows))]
    {
        let all = mounts();
        let p = std::fs::canonicalize(path).ok()?;
        let m = volume_at(&all, &p)?;
        let others = other_places(&all, m);
        (!others.is_empty()).then(|| (m.source.clone(), others))
    }
}

/// The folder a drop of the drive `source` into `dest_dir` makes: named like the drive's top
/// folder ("File System" for `/`), "(mountpoint)" added when the name is taken.
pub fn folder_for(source: &Path, dest_dir: &Path) -> PathBuf {
    let name = source.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| volume_label(source).into());
    let at = dest_dir.join(&name);
    if std::fs::symlink_metadata(&at).is_ok() {
        return dest_dir.join(free_name(dest_dir, &name, Some(super::LinkKind::Mountpoint.name_tag())));
    }
    at
}

fn volume_label(source: &Path) -> String {
    let s = source.to_string_lossy();
    let letter = s.trim_end_matches(['\\', '/']).trim_end_matches(':');
    if letter.len() == 1 { format!("Drive {letter}") } else { "File System".into() }
}

/// Makes the Volume Mountpoint of the drive whose top folder is `source` in `dest_dir` (a new
/// folder) and gives the folder; `admin` runs what needs the administrator. The folder is
/// removed again when the drive cannot be mounted there.
pub fn create(source: &Path, dest_dir: &Path, admin: &super::helper::Elevation) -> io::Result<PathBuf> {
    if !is_volume_root(source) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not the top folder of a local drive", source.display())));
    }
    let folder = folder_for(source, dest_dir);
    std::fs::create_dir(&folder)?;
    let r = mount(source, &folder, admin);
    if r.is_err() {
        let _ = std::fs::remove_dir(&folder);
    }
    r.map(|()| folder)
}

fn mount(source: &Path, folder: &Path, admin: &super::helper::Elevation) -> io::Result<()> {
    #[cfg(windows)]
    {
        let _ = admin;
        win::mount(source, folder)
    }
    #[cfg(not(windows))]
    {
        admin.run(vec!["mount".into(), source.into(), folder.into()], &mut |_| true)?.result()
    }
}

/// *Unmount Volume*: takes the drive out of the Volume Mountpoint `folder`, which stays, empty.
pub fn remove(folder: &Path, admin: &super::helper::Elevation) -> io::Result<()> {
    #[cfg(windows)]
    {
        let _ = admin;
        win::unmount(folder)
    }
    #[cfg(not(windows))]
    {
        admin.run(vec!["unmount".into(), folder.into()], &mut |_| true)?.result()
    }
}

/// The administrator's half of [`create`]: the drive whose top folder is `source` mounted on
/// the empty folder `folder`, which must belong to `owner` (the user who asked) when given.
#[cfg(target_os = "linux")]
pub fn mount_here(source: &Path, folder: &Path, owner: Option<u32>) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let bad = |m: String| io::Error::new(io::ErrorKind::InvalidInput, m);
    let meta = std::fs::symlink_metadata(folder)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(bad(format!("{} is not a folder", folder.display())));
    }
    if std::fs::read_dir(folder)?.next().is_some() {
        return Err(bad(format!("{} is not empty", folder.display())));
    }
    if owner.is_some_and(|o| o != meta.uid()) {
        return Err(bad(format!("{} is not yours", folder.display())));
    }
    let src = std::fs::canonicalize(source)?;
    if volume_at(&mounts(), &src).is_none() {
        return Err(bad(format!("{} is not the top folder of a local drive", source.display())));
    }
    let folder = std::fs::canonicalize(folder)?;
    let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| bad("NUL in path".into()));
    let (s, f) = (c(&src)?, c(&folder)?);
    // SAFETY: NUL-terminated paths; no filesystem type or data for a bind mount.
    if unsafe { libc::mount(s.as_ptr(), f.as_ptr(), std::ptr::null(), libc::MS_BIND, std::ptr::null()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The administrator's half of [`remove`]: only a drive shown somewhere else as well is taken
/// out of `folder`.
#[cfg(target_os = "linux")]
pub fn unmount_here(folder: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let folder = std::fs::canonicalize(folder)?;
    let all = mounts();
    let ok = volume_at(&all, &folder).is_some_and(|m| !other_places(&all, m).is_empty());
    if !ok {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a Volume Mountpoint", folder.display())));
    }
    let f = std::ffi::CString::new(folder.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::umount2(f.as_ptr(), libc::UMOUNT_NOFOLLOW) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{DeleteVolumeMountPointW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, SetVolumeMountPointW};

    /// `path` with exactly one trailing backslash, NUL-terminated, as the volume calls want it.
    fn slashed(path: &Path) -> Vec<u16> {
        let mut w: Vec<u16> = path.as_os_str().encode_wide().collect();
        while w.last() == Some(&(b'\\' as u16)) || w.last() == Some(&(b'/' as u16)) {
            w.pop();
        }
        w.push(b'\\' as u16);
        w.push(0);
        w
    }

    fn text(buf: &[u16]) -> String {
        String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(buf.len())])
    }

    /// The `\\?\Volume{…}\` name of the volume mounted at `path`, when one is (a drive's root or
    /// a mountpoint folder).
    pub fn volume_name(path: &Path) -> Option<String> {
        let mut buf = [0u16; 64];
        unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(slashed(path).as_ptr()), &mut buf) }.ok()?;
        Some(text(&buf))
    }

    /// A folder (not a drive's root) with a volume mounted on it.
    pub fn is_mountpoint(path: &Path) -> bool {
        let mut root = [0u16; 1024];
        let is_root = unsafe { GetVolumePathNameW(PCWSTR(slashed(path).as_ptr()), &mut root) }.is_ok() && text(&root).len() <= 3;
        !is_root && volume_name(path).is_some()
    }

    pub fn mounted_volume(path: &Path) -> Option<String> {
        is_mountpoint(path).then(|| volume_name(path)).flatten()
    }

    pub fn mount(source: &Path, folder: &Path) -> io::Result<()> {
        let name = volume_name(source).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a volume"))?;
        let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        unsafe { SetVolumeMountPointW(PCWSTR(slashed(folder).as_ptr()), PCWSTR(w.as_ptr())) }.map_err(crate::win::err::to_io)
    }

    pub fn unmount(folder: &Path) -> io::Result<()> {
        unsafe { DeleteVolumeMountPointW(PCWSTR(slashed(folder).as_ptr())) }.map_err(crate::win::err::to_io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "\
32 2 253:1 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
57 32 253:2 / /home rw,relatime shared:113 - ext4 /dev/mapper/home rw
70 32 8:17 / /run/media/me/STICK rw - vfat /dev/sdb1 rw
71 57 8:17 / /home/me/Stick rw - vfat /dev/sdb1 rw
72 57 253:2 /me/src /home/me/bound rw - ext4 /dev/mapper/home rw
73 32 0:50 / /mnt/nas rw - nfs4 nas:/export rw
74 32 0:51 / /mnt/my\\040share rw - cifs //srv/share rw
75 32 0:52 / /run/media/me/STICK rw - tmpfs tmpfs rw
";

    #[test]
    fn only_a_local_drive_as_a_whole_is_a_volume() {
        let m = parse(INFO);
        let at = |p: &str| volume_at(&m, Path::new(p)).map(|v| v.source.as_str());
        assert_eq!(at("/home"), Some("/dev/mapper/home"));
        assert_eq!(at("/home/me/Stick"), Some("/dev/sdb1"));
        assert_eq!(at("/home/me/bound"), None, "a folder of a drive bound elsewhere is not the drive");
        assert_eq!(at("/mnt/nas"), None, "network share");
        assert_eq!(at("/mnt/my share"), None, "network share, escaped name");
        assert_eq!(at("/run/media/me/STICK"), None, "covered by a later mount");
        assert_eq!(at("/home/me"), None, "not a mount");
    }

    #[test]
    fn a_mountpoint_knows_where_else_its_drive_is() {
        let m = parse(INFO);
        let stick = volume_at(&m, Path::new("/home/me/Stick")).unwrap();
        // Its first place is covered by a tmpfs, so the drive is shown nowhere else.
        assert!(other_places(&m, stick).is_empty());
        let m = parse(&INFO.lines().filter(|l| !l.contains("tmpfs")).collect::<Vec<_>>().join("\n"));
        let stick = volume_at(&m, Path::new("/home/me/Stick")).unwrap();
        assert_eq!(other_places(&m, stick), [PathBuf::from("/run/media/me/STICK")]);
    }

    #[test]
    fn the_new_folder_is_named_like_the_drive() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(folder_for(Path::new("/run/media/me/STICK"), t.path()), t.path().join("STICK"));
        std::fs::create_dir(t.path().join("STICK")).unwrap();
        assert_eq!(folder_for(Path::new("/run/media/me/STICK"), t.path()), t.path().join("STICK (mountpoint)"));
        assert_eq!(folder_for(Path::new("/"), t.path()), t.path().join("File System"));
    }
}
