//! Archive folders through the VFS, with the real helper: nested archives and write-back.

use failbrauwser::archive::client::locate_helper;
use failbrauwser::archive::vfs::Vfs;
use failbrauwser::archive::Helper;
use failbrauwser::location::ArchiveLoc;
use std::fs;
use std::path::Path;

fn vfs(cache: &Path) -> Option<Vfs> {
    let exe = locate_helper();
    if !exe.is_file() {
        eprintln!("fb-archive helper not built, skipping");
        return None;
    }
    Some(Vfs::new(Helper::new(exe), cache.to_path_buf()))
}

fn names(v: &Vfs, loc: &ArchiveLoc, dir: &str) -> Vec<String> {
    let a = v.archive(loc).unwrap();
    a.tree.children(dir).unwrap_or_default().iter().map(|n| n.name.clone()).collect()
}

/// outer.zip containing docs/inner.tar.gz containing notes/a.txt
fn nested_fixture(d: &Path, v: &Vfs) -> ArchiveLoc {
    fs::create_dir_all(d.join("src/notes")).unwrap();
    fs::write(d.join("src/notes/a.txt"), "inner text").unwrap();
    let inner = d.join("inner.tar.gz");
    v.helper().add(&inner, &[(d.join("src/notes"), "notes".into())], None).unwrap();
    let outer = d.join("outer.zip");
    fs::write(d.join("readme"), "top").unwrap();
    v.helper().add(&outer, &[(inner, "docs/inner.tar.gz".into()), (d.join("readme"), "readme".into())], None).unwrap();
    ArchiveLoc::root(outer)
}

#[test]
fn browses_into_nested_archives() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let outer = nested_fixture(d.path(), &v);
    assert_eq!(names(&v, &outer, ""), vec!["docs", "readme"]);
    let inner = outer.enter_nested("docs/inner.tar.gz");
    assert_eq!(names(&v, &inner, ""), vec!["notes"]);
    assert_eq!(names(&v, &inner, "notes"), vec!["a.txt"]);
    let out = d.path().join("out");
    let got = v.extract(&inner, &["notes/a.txt".into()], &out).unwrap();
    assert_eq!(fs::read_to_string(&got[0]).unwrap(), "inner text");
}

#[test]
fn edits_in_nested_archive_reach_the_outer_file() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let outer = nested_fixture(d.path(), &v);
    let inner = outer.enter_nested("docs/inner.tar.gz");
    fs::write(d.path().join("new.txt"), "added deep").unwrap();
    v.add(&inner, &[(d.path().join("new.txt"), "notes/new.txt".into())]).unwrap();
    v.mkdir(&inner, "fresh").unwrap();
    v.rename(&inner, "notes/a.txt", "notes/b.txt").unwrap();

    // A new VFS with an empty cache sees the change through the outer zip.
    let v2 = vfs(&d.path().join("cache2")).unwrap();
    assert_eq!(names(&v2, &inner, ""), vec!["fresh", "notes"]);
    assert_eq!(names(&v2, &inner, "notes"), vec!["b.txt", "new.txt"]);
    assert_eq!(names(&v2, &outer, ""), vec!["docs", "readme"]);

    v.remove(&inner, &["notes".into()]).unwrap();
    let v3 = vfs(&d.path().join("cache3")).unwrap();
    assert_eq!(names(&v3, &inner, ""), vec!["fresh"]);
}

#[test]
fn listing_is_cached_until_the_file_changes() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let outer = nested_fixture(d.path(), &v);
    let a1 = v.archive(&outer).unwrap();
    let a2 = v.archive(&outer).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a1, &a2));
    assert!(v.cached(&outer).is_some());
    fs::write(d.path().join("more"), "m").unwrap();
    v.add(&outer, &[(d.path().join("more"), "more".into())]).unwrap();
    let a3 = v.archive(&outer).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&a1, &a3));
    assert_eq!(names(&v, &outer, ""), vec!["docs", "more", "readme"]);
}

#[test]
fn extract_all_makes_one_folder() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let outer = nested_fixture(d.path(), &v);
    let dest = d.path().join("dest");
    fs::create_dir(&dest).unwrap();
    // Two top-level entries: into a folder named after the archive.
    let made = v.extract_all(&outer, &dest).unwrap();
    assert_eq!(made, dest.join("outer"));
    assert_eq!(fs::read_to_string(dest.join("outer/readme")).unwrap(), "top");
    // Again: the name is taken, a free one is used.
    let made2 = v.extract_all(&outer, &dest).unwrap();
    assert_eq!(made2, dest.join("outer (2)"));
    // A single top folder is extracted as it is.
    let inner = outer.enter_nested("docs/inner.tar.gz");
    assert_eq!(v.extract_all(&inner, &dest).unwrap(), dest.join("notes"));
}

#[test]
fn read_only_formats_refuse_edits() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    // SQLite databases are listed as tables, never written.
    let db = d.path().join("x.sqlite");
    let st = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!("import sqlite3; c = sqlite3.connect({:?}); c.execute('create table t (a)'); c.commit()", db.display().to_string()))
        .status();
    if !st.is_ok_and(|s| s.success()) {
        eprintln!("python3 with sqlite3 missing, skipping");
        return;
    }
    let loc = ArchiveLoc::root(db);
    let a = v.archive(&loc).unwrap();
    assert!(!a.writable);
    let err = v.mkdir(&loc, "nope").unwrap_err();
    assert_eq!(err.code, "unsupported");
}

/// A compressed single file (.gz, .xz, …) shows the file inside, which can be edited and
/// saved back, and extracts to that file itself.
#[test]
fn compressed_files_are_edited_and_extracted() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    fs::write(d.path().join("notes.txt"), "first version\n").unwrap();
    fs::write(d.path().join("edited"), "second version, longer than the first\n").unwrap();
    for ext in ["gz", "bz2", "xz", "zst", "br", "lz4", "lzma", "lz", "z", "uue"] {
        let file = d.path().join(format!("notes.txt.{ext}"));
        v.helper().add(&file, &[(d.path().join("notes.txt"), "notes.txt".into())], None).unwrap();
        let loc = ArchiveLoc::root(file.clone());
        let a = v.archive(&loc).unwrap();
        assert!(a.writable, "{ext} is writable");
        assert_eq!(names(&v, &loc, ""), ["notes.txt"], "{ext}");
        // Saving an edited copy recompresses the file.
        v.add(&loc, &[(d.path().join("edited"), "notes.txt".into())]).unwrap();
        let out = d.path().join(format!("out-{ext}"));
        v.extract(&loc, &["notes.txt".into()], &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("notes.txt")).unwrap(), "second version, longer than the first\n", "{ext}");
        // Nothing else fits in.
        assert!(v.add(&loc, &[(d.path().join("edited"), "other.txt".into())]).is_err(), "{ext}");
        assert!(v.remove(&loc, &["notes.txt".into()]).is_err(), "{ext}");
        assert!(v.rename(&loc, "notes.txt", "x.txt").is_err(), "{ext}");
        assert!(v.mkdir(&loc, "dir").is_err(), "{ext}");
        // Extract Here gives the file, not a folder around it.
        let here = d.path().join(format!("here-{ext}"));
        fs::create_dir(&here).unwrap();
        assert_eq!(v.extract_all(&loc, &here).unwrap(), here.join("notes.txt"), "{ext}");
        assert!(here.join("notes.txt").is_file(), "{ext}");
    }
    // Other programs read what was written.
    if std::process::Command::new("gzip").arg("--version").output().is_ok() {
        let out = std::process::Command::new("gzip").arg("-dc").arg(d.path().join("notes.txt.gz")).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "second version, longer than the first\n");
    }
}

// ── M14 "Open as": a container opened as the kind the user chose ──

fn volume_fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/veracrypt").join(name)
}

fn sha256_of(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(fs::read(path).unwrap());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// One file read two ways answers two different listings, so the listing cache has to keep the
/// chosen kind apart from the detected one instead of serving whichever came first.
#[test]
fn a_chosen_kind_and_the_detected_one_keep_their_own_listing() {
    use std::sync::Arc;
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let lying = d.path().join("disk.bin");
    fs::copy(volume_fixture("test.sha256.hc"), &lying).unwrap();

    let seen = ArchiveLoc::root(lying.clone());
    let chosen = ArchiveLoc::root_as(lying.clone(), "VeraCrypt");
    v.set_password(&chosen, "test");

    let detected = v.archive(&seen).unwrap();
    assert_ne!(detected.format, "VeraCrypt", "neither the name nor the bytes say volume");

    let forced = v.archive(&chosen).unwrap();
    assert_eq!(forced.format, "VeraCrypt");
    assert!(forced.tree.get("volume.img").is_some(), "the chosen reader shows the volume");
    assert!(!Arc::ptr_eq(&detected, &forced), "one file, two answers: the cache keeps them apart");

    // Asking again brings the cached answer: no second search behind it.
    assert!(Arc::ptr_eq(&forced, &v.archive(&chosen).unwrap()));
    assert!(Arc::ptr_eq(&detected, &v.archive(&seen).unwrap()));
}

/// A chosen container opens read-only, and extracting from it runs the chosen reader.
#[test]
fn a_chosen_kind_extracts_the_volume_and_refuses_edits() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let lying = d.path().join("disk.bin");
    fs::copy(volume_fixture("test.sha256.hc"), &lying).unwrap();
    let chosen = ArchiveLoc::root_as(lying, "VeraCrypt");
    v.set_password(&chosen, "test");

    assert!(!v.archive(&chosen).unwrap().writable, "a chosen container opens read-only");
    assert_eq!(v.mkdir(&chosen, "nope").unwrap_err().code, "unsupported");

    let out = d.path().join("out");
    let got = v.extract(&chosen, &["volume.img".into()], &out).unwrap();
    // The data area as recorded when the real VeraCrypt program wrote this container.
    assert_eq!(sha256_of(&got[0]), "2b15dbe4544a853ca15311dd1dc83162c6cc06c6c470d98ca427b26093a9fb41");
}

/// A locked volume answers the shell of itself, and that shell is what asks for the passphrase.
/// Keeping that answer after the passphrase arrived would throw the ask away.
#[test]
fn a_passphrase_set_after_a_locked_listing_reopens_the_volume() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let lying = d.path().join("disk.bin");
    fs::copy(volume_fixture("test.sha256.hc"), &lying).unwrap();
    let chosen = ArchiveLoc::root_as(lying, "VeraCrypt");

    let locked = v.archive(&chosen).unwrap();
    assert!(locked.tree.get("volume.img").is_some());
    assert!(locked.tree.get("metadata.ini").is_none(), "with no passphrase yet: the shell alone");

    v.set_password(&chosen, "test");
    let opened = v.archive(&chosen).unwrap();
    assert!(opened.tree.get("metadata.ini").is_some(), "the passphrase that was asked is used");
    assert!(!std::sync::Arc::ptr_eq(&locked, &opened), "the shell answer was replaced, not kept");
}

#[test]
fn a_chosen_kind_that_is_not_a_format_is_refused_by_name() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let lying = d.path().join("disk.bin");
    fs::copy(volume_fixture("test.sha512.hc"), &lying).unwrap();
    let error = v.archive(&ArchiveLoc::root_as(lying, "NoSuchKind")).unwrap_err();
    assert!(error.message.contains("NoSuchKind"), "the refusal names the kind: {error}");
}

/// A dm-crypt (plain) container has no header at all: nothing about the file says what it is,
/// so the choice is the only way in — and it answers without a search.
#[test]
fn a_headerless_volume_reopens_only_through_its_chosen_kind() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    let payload = d.path().join("payload");
    fs::write(&payload, vec![0xABu8; 4096]).unwrap();
    let plain = d.path().join("disk.img");
    v.helper().create(&plain, "DmCryptPlain", &[(payload.clone(), "volume.img".into())], Some("pw")).unwrap();

    let seen = ArchiveLoc::root(plain.clone());
    let blind = v.archive(&seen).unwrap_err();
    assert_eq!(blind.code, "unsupported", "the file carries no marker, so detection has nothing to find: {blind}");

    let chosen = ArchiveLoc::root_as(plain, "DmCryptPlain");
    v.set_password(&chosen, "pw");
    let a = v.archive(&chosen).unwrap();
    assert_eq!(a.format, "DmCryptPlain");
    assert!(a.tree.get("volume.img").is_some());

    let out = d.path().join("out");
    let got = v.extract(&chosen, &["volume.img".into()], &out).unwrap();
    assert_eq!(fs::read(&got[0]).unwrap(), fs::read(&payload).unwrap());
}

#[test]
fn failed_extraction_leaves_nothing_behind() {
    let d = tempfile::tempdir().unwrap();
    let Some(v) = vfs(&d.path().join("cache")) else { return };
    fs::write(d.path().join("f.txt"), "x").unwrap();
    let zip = d.path().join("locked.zip");
    v.helper().add(&zip, &[(d.path().join("f.txt"), "f.txt".into())], Some("pw")).unwrap();
    let dest = d.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let loc = ArchiveLoc::root(zip);
    assert!(v.extract_all(&loc, &dest).unwrap_err().needs_password());
    let left: Vec<_> = fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(left.is_empty(), "left behind: {left:?}");
    v.set_password(&loc, "pw");
    // A single file lands as itself, not in "f (2).txt".
    assert_eq!(v.extract_all(&loc, &dest).unwrap(), dest.join("f.txt"));
}
