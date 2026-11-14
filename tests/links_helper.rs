//! The administrator helper of Backup Mode and Volume Mountpoints (README: Links), run as root
//! for real: inside a user namespace (`unshare -r`, `-rm` with a mount namespace of its own) the
//! helper is uid 0 with the powers root has over the files it maps, so it reads folders nobody
//! may read and mounts drives, without a password and without touching the system.
#![cfg(target_os = "linux")]

use failbrauwser::links::helper::{self, Elevation};
use failbrauwser::links::tree::{Options, Outer, TreeOp};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_failbrauwser");

fn namespaces() -> bool {
    Command::new("unshare").args(["-rm", "true"]).status().is_ok_and(|s| s.success())
}

/// A folder under `target/`, on the same drive as the sources (the system's temporary folder may
/// be a tmpfs, which is no drive).
fn scratch() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/links-helper");
    std::fs::create_dir_all(&base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn set_xattr(p: &Path, n: &str, v: &[u8]) -> bool {
    let (p, n) = (std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap(), std::ffi::CString::new(n).unwrap());
    unsafe { libc::lsetxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0) == 0 }
}

#[test]
fn backup_mode_copies_what_the_user_cannot_read_and_keeps_what_files_carry() {
    if !namespaces() {
        eprintln!("skipped: no user namespaces here");
        return;
    }
    let t = scratch();
    let src = t.path().join("src");
    std::fs::create_dir_all(src.join("locked/deeper")).unwrap();
    std::fs::write(src.join("locked/deeper/secret"), "only root").unwrap();
    std::fs::write(src.join("tool"), "x").unwrap();
    std::fs::set_permissions(src.join("tool"), std::fs::Permissions::from_mode(0o2751)).unwrap();
    let xattr_fs = set_xattr(&src.join("tool"), "user.fb.origin", b"src");
    std::os::unix::fs::symlink("tool", src.join("tool-link")).unwrap();
    std::fs::set_permissions(src.join("locked/deeper/secret"), std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::set_permissions(src.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(std::fs::read_dir(src.join("locked")).is_err(), "the fixture is unreadable for the user");

    let dst = t.path().join("dst");
    let opts = Options { outer: Outer::Splice, relative: true, backup: true };
    let mut seen = Vec::new();
    let reply = Elevation::with(vec!["unshare".into(), "-r".into(), BIN.into()])
        .run(helper::tree_args(TreeOp::SmartCopy, opts, &[(src.clone(), dst.clone())]), &mut |p| {
            seen.push(p.to_string());
            true
        })
        .unwrap();
    // Readable again for the checks (and for the clean-up).
    for p in [src.join("locked"), dst.join("locked")] {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(reply.errors.is_empty(), "{:?}", reply.errors);
    assert!(!reply.cancelled);
    assert_eq!(reply.copied, 2, "{reply:?}");
    assert!(seen.iter().any(|p| p.ends_with("locked/deeper/secret")), "progress names each entry: {seen:?}");
    let secret = dst.join("locked/deeper/secret");
    assert_eq!(std::fs::metadata(&secret).unwrap().mode() & 0o7777, 0, "permissions kept, also none at all");
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "only root");
    assert_eq!(std::fs::metadata(dst.join("tool")).unwrap().mode() & 0o7777, 0o2751, "setgid kept");
    assert_eq!(std::fs::read_link(dst.join("tool-link")).unwrap(), Path::new("tool"));
    if xattr_fs {
        assert_eq!(failbrauwser::links::backup::xattrs(&dst.join("tool")).unwrap(), [("user.fb.origin".into(), b"src".to_vec())]);
    }
}

#[test]
fn a_stopped_helper_run_says_so() {
    if !namespaces() {
        return;
    }
    let t = scratch();
    let src = t.path().join("src");
    for i in 0..2000 {
        std::fs::create_dir_all(src.join(format!("d{i}"))).unwrap();
        std::fs::write(src.join(format!("d{i}/f")), "x").unwrap();
    }
    let opts = Options { backup: true, ..Options::default() };
    let reply = Elevation::with(vec!["unshare".into(), "-r".into(), BIN.into()]).run(helper::tree_args(TreeOp::SmartCopy, opts, &[(src, t.path().join("dst"))]), &mut |_| false).unwrap();
    assert!(reply.cancelled, "{reply:?}");
}

#[test]
fn the_helper_refuses_to_run_as_the_user() {
    let out = Command::new(BIN).args(["--links", "unmount", "/"]).output().unwrap();
    if unsafe { libc::geteuid() } != 0 {
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("administrator"));
    }
    let e = Elevation::with(vec![BIN.into()]).run(vec!["unmount".into(), "/".into()], &mut |_| true).unwrap_err();
    assert!(e.to_string().contains("administrator"), "{e}");
}

/// The top folder of the drive holding the sources, when it is one a user namespace may bind
/// (not `/`, whose mounts below it are locked there).
fn drive_of_sources() -> Option<PathBuf> {
    let here = std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).ok()?;
    let mounts = failbrauwser::links::volume::mounts();
    let m = mounts.iter().filter(|m| here.starts_with(&m.at)).max_by_key(|m| m.at.as_os_str().len())?;
    (m.is_volume() && m.at != Path::new("/") && failbrauwser::links::volume::mount_at(&mounts, &m.at) == Some(m)).then(|| m.at.clone())
}

#[test]
fn a_drive_is_mounted_into_an_empty_folder_and_taken_out_again() {
    let Some(drive) = drive_of_sources() else {
        eprintln!("skipped: the sources are not below a drive of their own");
        return;
    };
    if !namespaces() {
        eprintln!("skipped: no user namespaces here");
        return;
    }
    let t = scratch();
    let folder = t.path().join("Drive");
    std::fs::create_dir(&folder).unwrap();
    let full = t.path().join("full");
    std::fs::create_dir(&full).unwrap();
    std::fs::write(full.join("x"), "").unwrap();
    let sub = t.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let empty = t.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    // One namespace for all steps: a mount lives as long as its namespace.
    let script = r#"
        b="$1"; drive="$2"; folder="$3"; full="$4"; sub="$5"; empty="$6"
        "$b" --links mount "$drive" "$folder"; echo "mounted"
        ls -A "$folder" | head -c 4000 > "$folder.listing"
        "$b" --links mount "$drive" "$full"; echo "onto-full"
        "$b" --links mount "$sub" "$empty"; echo "not-a-drive"
        "$b" --links unmount "$sub"; echo "unmount-plain"
        "$b" --links unmount "$folder"; echo "unmounted"
        ls -A "$folder" | wc -l
    "#;
    let out = Command::new("unshare").args(["-rm", "sh", "-c", script, "sh", BIN]).arg(&drive).arg(&folder).arg(&full).arg(&sub).arg(&empty).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let segments: Vec<&str> = text.split('\n').collect();
    let report_before = |marker: &str| -> helper::Reply {
        let at = segments.iter().position(|l| *l == marker).unwrap_or_else(|| panic!("{marker} missing: {text}"));
        let line = segments[..at].iter().rev().find(|l| l.starts_with("report\t")).unwrap();
        serde_json::from_str(line.trim_start_matches("report\t")).unwrap()
    };
    assert!(report_before("mounted").errors.is_empty(), "{text}");
    let listing = std::fs::read_to_string(t.path().join("Drive.listing")).unwrap();
    let real: Vec<String> = std::fs::read_dir(&drive).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(real.iter().all(|n| listing.lines().any(|l| l == n)) || listing.len() >= 3999, "the drive is shown in the folder: {listing}");
    assert!(report_before("onto-full").errors[0].1.contains("not empty"), "{text}");
    assert!(report_before("not-a-drive").errors[0].1.contains("top folder of a local drive"), "{text}");
    assert!(report_before("unmount-plain").errors[0].1.contains("not a Volume Mountpoint"), "{text}");
    assert!(report_before("unmounted").errors.is_empty(), "{text}");
    assert_eq!(segments.iter().rev().find(|l| !l.is_empty()).map(|l| l.trim()), Some("0"), "the folder is empty again: {text}");
}

#[test]
fn backup_mode_keeps_other_users_as_owners_of_files_and_links() {
    // Subordinate uids (`/etc/subuid`) give the namespace more users than its root.
    if !Command::new("unshare").args(["-r", "--map-auto", "true"]).status().is_ok_and(|s| s.success()) {
        eprintln!("skipped: no subordinate uids here");
        return;
    }
    let t = scratch();
    let src = t.path().join("src");
    std::fs::create_dir_all(src.join("theirs")).unwrap();
    std::fs::write(src.join("theirs/f"), "x").unwrap();
    std::fs::create_dir(t.path().join("a")).unwrap();
    std::fs::create_dir(t.path().join("b")).unwrap();
    std::os::unix::fs::symlink("a", t.path().join("link")).unwrap();
    let script = r#"
        b="$1"; src="$2"; dst="$3"; link="$4"; b2="$5"
        chown 5:7 "$src/theirs/f" "$src/theirs" && chown -h 9:9 "$link" || exit 9
        "$b" --links tree smart-copy splice 1 "$src" "$dst"
        "$b" --links retarget 1 "$link" "$b2"
    "#;
    let out = Command::new("unshare")
        .args(["-r", "--map-auto", "sh", "-c", script, "sh", BIN])
        .arg(&src)
        .arg(t.path().join("dst"))
        .arg(t.path().join("link"))
        .arg(t.path().join("b"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && text.matches("\"errors\":[]").count() == 2, "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    let owner = |p: &Path| {
        let m = std::fs::symlink_metadata(p).unwrap_or_else(|e| panic!("{}: {e}\n{text}", p.display()));
        (m.uid(), m.gid())
    };
    assert_ne!(owner(&src.join("theirs/f")).0, unsafe { libc::getuid() }, "the fixture belongs to someone else");
    assert_eq!(owner(&t.path().join("dst/theirs/f")), owner(&src.join("theirs/f")));
    assert_eq!(owner(&t.path().join("dst/theirs")), owner(&src.join("theirs")));
    assert_eq!(std::fs::read_link(t.path().join("link")).unwrap(), Path::new("b"), "retargeted");
    assert_eq!(owner(&t.path().join("link")).0, owner(&src.join("theirs/f")).0 + 4, "the link keeps its owner (9, not 5)");
}
