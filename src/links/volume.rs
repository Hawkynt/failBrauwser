//! Volume Mountpoints (README: Links, *Volume Mountpoints*): a whole local drive shown in a new
//! folder. On Linux the drive's filesystem is bind-mounted there (`mount(2)` with `MS_BIND`, not
//! recursive: the drive, not what is mounted below its other places), as the administrator
//! through the helper. On Windows the folder gets the volume's `\\?\Volume{…}\` name through
//! `SetVolumeMountPointW` and loses it through `DeleteVolumeMountPointW` (Microsoft's
//! documentation, unmeasured here: no Windows machine runs the tests).
//!
//! Windows keeps a volume mount point across restarts; on Linux the mount is kept the same way by
//! a line in `/etc/fstab` (`fstab(5)`, `systemd.mount(5)`: rung 3): the drive by its UUID, mounted
//! on the folder when the folder is first opened, marked with the option `x-failbrauwser` (mount
//! and systemd ignore `x-` options) so that only lines failBrauwser wrote are ever removed.

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
    /// Mounted read-only.
    pub read_only: bool,
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
            (l.len() >= 6 && r.len() >= 2).then(|| Mount {
                source: un(r[1]),
                fs_type: r[0].to_string(),
                root: PathBuf::from(un(l[3])),
                at: PathBuf::from(un(l[4])),
                read_only: l[5].split(',').any(|o| o == "ro"),
            })
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
        let Ok(p) = std::fs::canonicalize(path) else { return false };
        let all = mounts();
        kept(&p).is_some() || volume_at(&all, &p).is_some_and(|m| !other_places(&all, m).is_empty())
    }
}

/// A Volume Mountpoint as its *Link* tab shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The drive: its device, or the way the kept line names it while it is not mounted.
    pub volume: String,
    /// Where else the drive is shown.
    pub others: Vec<PathBuf>,
    /// Kept across restarts.
    pub kept: bool,
}

/// The drive shown in the folder `path` and its other places, for the *Link* tab.
pub fn shown_at(path: &Path) -> Option<Shown> {
    #[cfg(windows)]
    {
        win::mounted_volume(path).map(|volume| Shown { volume, others: Vec::new(), kept: true })
    }
    #[cfg(not(windows))]
    {
        let all = mounts();
        let p = std::fs::canonicalize(path).ok()?;
        let spec = kept(&p);
        match volume_at(&all, &p) {
            Some(m) => {
                let others = other_places(&all, m);
                (!others.is_empty() || spec.is_some()).then(|| Shown { volume: m.source.clone(), others, kept: spec.is_some() })
            }
            None => spec.map(|volume| Shown { volume, others: Vec::new(), kept: true }),
        }
    }
}

/// The fstab failBrauwser reads and writes: `/etc/fstab` (the tests name another one through
/// `FAILBRAUWSER_FSTAB`, which pkexec never passes on).
pub fn fstab_path() -> PathBuf {
    std::env::var_os("FAILBRAUWSER_FSTAB").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/etc/fstab"))
}

/// How failBrauwser's line names the drive kept on the folder `folder`, when it wrote one.
pub fn kept(folder: &Path) -> Option<String> {
    let text = std::fs::read_to_string(fstab_path()).ok()?;
    fstab::ours(&text).into_iter().find(|(_, at)| at == folder).map(|(spec, _)| spec)
}

/// The lines of fstab failBrauwser writes and reads (`fstab(5)`).
pub mod fstab {
    use std::path::{Path, PathBuf};

    /// The option marking failBrauwser's lines.
    pub const MARK: &str = "x-failbrauwser";

    /// A field of fstab: space, tab, newline and backslash as octal escapes.
    pub fn escape(field: &str) -> String {
        let mut out = String::with_capacity(field.len());
        for c in field.chars() {
            match c {
                ' ' => out.push_str("\\040"),
                '\t' => out.push_str("\\011"),
                '\n' => out.push_str("\\012"),
                '\\' => out.push_str("\\134"),
                c => out.push(c),
            }
        }
        out
    }

    /// The line keeping the drive `spec` (`UUID=…`) of type `fs_type` on `folder`.
    pub fn line(spec: &str, folder: &Path, fs_type: &str, read_only: bool) -> String {
        let rw = if read_only { "ro" } else { "rw" };
        format!("{} {} {} {rw},nofail,x-systemd.automount,x-systemd.device-timeout=3s,{MARK} 0 0", escape(spec), escape(&folder.to_string_lossy()), escape(fs_type))
    }

    /// failBrauwser's lines in `text`: (drive, folder).
    pub fn ours(text: &str) -> Vec<(String, PathBuf)> {
        text.lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                let ok = f.len() >= 4 && !f[0].starts_with('#') && f[3].split(',').any(|o| o == MARK);
                ok.then(|| (crate::drives::unescape_mount(f[0]), PathBuf::from(crate::drives::unescape_mount(f[1]))))
            })
            .collect()
    }

    /// `text` with `line` added (when no line of failBrauwser's keeps that folder yet).
    pub fn with(text: &str, line: &str, folder: &Path) -> String {
        if ours(text).iter().any(|(_, at)| at == folder) {
            return text.to_string();
        }
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
        out
    }

    /// `text` without failBrauwser's line for `folder`; every other line as it was.
    pub fn without(text: &str, folder: &Path) -> String {
        let keep = |l: &str| {
            let f: Vec<&str> = l.split_whitespace().collect();
            !(f.len() >= 4 && !f[0].starts_with('#') && f[3].split(',').any(|o| o == MARK) && Path::new(&crate::drives::unescape_mount(f[1])) == folder)
        };
        let mut out: String = text.split_inclusive('\n').filter(|l| keep(l)).collect();
        if text.ends_with('\n') && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out
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
    let volume = volume_at(&mounts(), &src).cloned().ok_or_else(|| bad("the drive went away".into()))?;
    // SAFETY: NUL-terminated paths; no filesystem type or data for a bind mount.
    if unsafe { libc::mount(s.as_ptr(), f.as_ptr(), std::ptr::null(), libc::MS_BIND, std::ptr::null()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // Kept across restarts; a mountpoint that cannot be kept is not made.
    let spec = uuid_of(&volume.source).map(|u| format!("UUID={u}")).unwrap_or_else(|| volume.source.clone());
    let line = fstab::line(&spec, &folder, &volume.fs_type, volume.read_only);
    if let Err(e) = edit_fstab(|t| fstab::with(t, &line, &folder)) {
        // SAFETY: a NUL-terminated path.
        unsafe { libc::umount2(f.as_ptr(), libc::UMOUNT_NOFOLLOW) };
        return Err(io::Error::new(e.kind(), format!("cannot keep it in {}: {e}", fstab_path().display())));
    }
    Ok(())
}

/// The UUID of the filesystem on `device`, from `/dev/disk/by-uuid`.
#[cfg(target_os = "linux")]
fn uuid_of(device: &str) -> Option<String> {
    let real = std::fs::canonicalize(device).ok()?;
    std::fs::read_dir("/dev/disk/by-uuid").ok()?.flatten().find(|e| std::fs::canonicalize(e.path()).ok().as_ref() == Some(&real)).map(|e| e.file_name().to_string_lossy().into_owned())
}

/// Rewrites fstab through `change` (a new file beside it renamed over it, its permissions
/// kept), then tells systemd, which reads its mounts from it.
#[cfg(target_os = "linux")]
fn edit_fstab(change: impl FnOnce(&str) -> String) -> io::Result<()> {
    let path = fstab_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let new = change(&text);
    if new == text {
        return Ok(());
    }
    let tmp = path.with_file_name(format!(".{}.failbrauwser", path.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::write(&tmp, &new)?;
    let perms = std::fs::metadata(&path).map(|m| m.permissions()).unwrap_or_else(|_| std::os::unix::fs::PermissionsExt::from_mode(0o644));
    let r = std::fs::set_permissions(&tmp, perms).and_then(|()| std::fs::rename(&tmp, &path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r?;
    if path == Path::new("/etc/fstab") {
        let mut c = std::process::Command::new("systemctl");
        c.arg("daemon-reload").stdin(std::process::Stdio::null());
        #[cfg(windows)]
        crate::win::console::no_window(&mut c); // the rule in tests\source_rules.rs: no start raises a window
        let _ = c.status();
    }
    Ok(())
}

/// The administrator's half of [`remove`]: a folder failBrauwser keeps a drive on, or a drive
/// shown somewhere else as well, is emptied (its fstab line removed, the drive and the systemd
/// automount on it unmounted).
#[cfg(target_os = "linux")]
pub fn unmount_here(folder: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let folder = std::fs::canonicalize(folder)?;
    let is_kept = kept(&folder).is_some();
    let all = mounts();
    let shared = volume_at(&all, &folder).is_some_and(|m| !other_places(&all, m).is_empty());
    if !is_kept && !shared {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a Volume Mountpoint", folder.display())));
    }
    if is_kept {
        edit_fstab(|t| fstab::without(t, &folder))?;
    }
    let f = std::ffi::CString::new(folder.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // The drive, then an automount under it: whatever is mounted on the folder, top first. A
    // kept folder whose drive is absent has nothing to unmount.
    for _ in 0..4 {
        if mount_at(&mounts(), &folder).is_none_or(|m| !(m.is_local() || m.fs_type == "autofs")) {
            break;
        }
        // SAFETY: a NUL-terminated path.
        if unsafe { libc::umount2(f.as_ptr(), libc::UMOUNT_NOFOLLOW) } != 0 {
            return Err(io::Error::last_os_error());
        }
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
    fn a_kept_mountpoint_is_one_line_of_fstab_and_goes_again_alone() {
        let folder = Path::new("/home/me/My Stick");
        let line = fstab::line("UUID=8D1B-C223", folder, "vfat", false);
        assert_eq!(line, "UUID=8D1B-C223 /home/me/My\\040Stick vfat rw,nofail,x-systemd.automount,x-systemd.device-timeout=3s,x-failbrauwser 0 0");
        let before = "# /etc/fstab\nUUID=aa / ext4 rw 0 1\n/data /home/me/My\\040Stick none bind 0 0\n";
        let with = fstab::with(before, &line, folder);
        assert_eq!(with, format!("{before}{line}\n"));
        assert_eq!(fstab::with(&with, &line, folder), with, "added once");
        assert_eq!(fstab::ours(&with), [("UUID=8D1B-C223".to_string(), folder.to_path_buf())], "the user's own bind line is not ours");
        assert_eq!(fstab::without(&with, folder), before, "only failBrauwser's line goes");
        assert_eq!(fstab::without(before, folder), before);
        assert_eq!(fstab::with("UUID=aa / ext4 rw 0 1", &line, folder), format!("UUID=aa / ext4 rw 0 1\n{line}\n"));
        assert!(fstab::line("UUID=x", Path::new("/m"), "ext4", true).contains(" ro,nofail"));
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
