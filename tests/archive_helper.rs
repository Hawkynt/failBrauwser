//! End-to-end tests against the real `fb-archive` helper. They are skipped (with a notice)
//! when the helper has not been built; `make helper` builds it.

use failbrauwser::archive::{ArchiveTree, Helper, client::locate_helper};
use std::fs;
use std::path::Path;
use std::process::Command;

fn helper() -> Option<Helper> {
    let exe = locate_helper();
    if !exe.is_file() {
        eprintln!("fb-archive helper not built, skipping ({})", exe.display());
        return None;
    }
    Some(Helper::new(exe))
}

fn sample_tree(root: &Path) {
    fs::create_dir_all(root.join("sub/deeper")).unwrap();
    fs::write(root.join("a.txt"), "root file").unwrap();
    fs::write(root.join("sub/a.txt"), "nested same name").unwrap();
    fs::write(root.join("sub/deeper/c.bin"), vec![7u8; 10_000]).unwrap();
}

fn names(h: &Helper, archive: &Path) -> Vec<String> {
    let mut n: Vec<String> = h.list(archive, None).unwrap().entries.into_iter().map(|e| e.name).collect();
    n.sort();
    n
}

#[test]
fn create_list_and_extract_roundtrip() {
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    sample_tree(&src);
    let archive = dir.path().join("new.zip");
    h.add(&archive, &[(src.clone(), "src".into())], None).unwrap();

    let listing = h.list(&archive, None).unwrap();
    assert_eq!(listing.format, "Zip");
    assert!(listing.writable);
    let tree = ArchiveTree::build(&listing.entries);
    assert!(tree.is_dir("src/sub/deeper"));
    assert_eq!(tree.get("src/sub/deeper/c.bin").unwrap().size, 10_000);

    let out = dir.path().join("out");
    h.extract(&archive, Some(&["src/sub".to_string()]), &out, None).unwrap();
    assert_eq!(fs::read_to_string(out.join("src/sub/a.txt")).unwrap(), "nested same name");
    assert_eq!(fs::read(out.join("src/sub/deeper/c.bin")).unwrap().len(), 10_000);
}

#[test]
fn new_archives_follow_the_settings() {
    use failbrauwser::archive::options::{CreateSettings, split_file};
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    sample_tree(&src);
    let items = [(src.clone(), "src".to_string())];

    // The formats say what can be chosen.
    let rar = h.options("Rar").unwrap();
    assert_eq!(rar.version().unwrap().values, ["Rar5", "Rar4", "Rar1_5"]);
    let zip = h.options("Zip").unwrap();
    assert!(zip.methods.iter().any(|(id, _)| id == "lzma"));
    assert!(zip.level().is_some());
    assert!(h.options("NoSuchFormat").is_err());

    // RAR 4 (what WinRAR 3.x writes) has the older signature.
    let rar4 = dir.path().join("old.rar");
    let s = CreateSettings { specific: vec![("TargetCompatibility".into(), "Rar4".into())], ..Default::default() };
    h.create_with(&rar4, "Rar", &items, None, Some(&s)).unwrap();
    assert_eq!(&fs::read(&rar4).unwrap()[..7], b"Rar!\x1a\x07\x00");
    let rar5 = dir.path().join("new.rar");
    h.create_with(&rar5, "Rar", &items, None, Some(&CreateSettings::default())).unwrap();
    assert_eq!(&fs::read(&rar5).unwrap()[..8], b"Rar!\x1a\x07\x01\x00");
    assert_eq!(names(&h, &rar4), names(&h, &rar5));

    // ZIP with LZMA: method 14 in the local headers (tiny files that do not shrink are
    // stored); stored: method 0, the 10 000 bytes as they are.
    let methods = |p: &Path| -> Vec<u8> {
        let b = fs::read(p).unwrap();
        b.windows(4).enumerate().filter(|(_, w)| *w == b"PK\x03\x04").map(|(i, _)| b[i + 8]).collect()
    };
    let lzma = dir.path().join("lzma.zip");
    h.create_with(&lzma, "Zip", &items, None, Some(&CreateSettings { method: Some("lzma".into()), level: Some(9), ..Default::default() })).unwrap();
    assert!(methods(&lzma).contains(&14), "{:?}", methods(&lzma));
    let stored = dir.path().join("stored.zip");
    h.create_with(&stored, "Zip", &items, None, Some(&CreateSettings { method: Some("store".into()), ..Default::default() })).unwrap();
    assert!(methods(&stored).iter().all(|&m| m == 0) && fs::metadata(&stored).unwrap().len() > 10_000);
    assert_eq!(names(&h, &lzma), names(&h, &stored));

    // 7z with format settings, split into parts that open as one archive again.
    let seven = dir.path().join("parts.7z");
    let s = CreateSettings { method: Some("copy".into()), specific: vec![("DictSize".into(), "65536".into())], ..Default::default() };
    h.create_with(&seven, "SevenZip", &items, None, Some(&s)).unwrap();
    let whole = fs::read(&seven).unwrap();
    let parts = split_file(&seven, 4096).unwrap();
    assert!(parts.len() >= 3, "{parts:?}");
    let joined: Vec<u8> = parts.iter().flat_map(|p| fs::read(p).unwrap()).collect();
    assert_eq!(joined, whole);
    // The first part opens as the 7z inside, read-only; it extracts like the 7z did.
    let listing = h.list(&parts[0], None).unwrap();
    assert_eq!(listing.format, "SevenZip");
    assert!(!listing.writable);
    let files = |p: &Path| -> Vec<String> {
        let mut v: Vec<String> = h.list(p, None).unwrap().entries.into_iter().filter(|e| !e.is_dir).map(|e| e.name).collect();
        v.sort();
        v
    };
    assert_eq!(files(&parts[0]), files(&rar5));
    let out = dir.path().join("from-parts");
    h.extract(&parts[0], None, &out, None).unwrap();
    assert_eq!(fs::read(out.join("src/sub/deeper/c.bin")).unwrap(), vec![7u8; 10_000]);
    assert!(h.remove(&parts[0], &["src/a.txt".to_string()], None).is_err());
}

#[test]
fn remove_is_exact_and_folders_go_completely() {
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    sample_tree(&src);
    let archive = dir.path().join("t.tar.gz");
    let st = Command::new("tar").arg("czf").arg(&archive).arg("-C").arg(&src).arg("a.txt").arg("sub").status().unwrap();
    assert!(st.success());

    h.remove(&archive, &["a.txt".into()], None).unwrap();
    assert_eq!(names(&h, &archive), vec!["sub", "sub/a.txt", "sub/deeper", "sub/deeper/c.bin"]);

    h.remove(&archive, &["sub/deeper".into()], None).unwrap();
    assert_eq!(names(&h, &archive), vec!["sub", "sub/a.txt"]);
}

#[test]
fn mkdir_rename_and_replace() {
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    sample_tree(&src);
    let archive = dir.path().join("t.7z");
    h.add(&archive, &[(src.join("a.txt"), "a.txt".into()), (src.join("sub"), "sub".into())], None).unwrap();

    h.mkdir(&archive, "empty", None).unwrap();
    assert!(names(&h, &archive).contains(&"empty".to_string()));

    h.rename(&archive, "sub", "moved/inner", None).unwrap();
    let n = names(&h, &archive);
    assert!(n.contains(&"moved/inner/deeper/c.bin".to_string()), "{n:?}");
    assert!(!n.iter().any(|x| x.starts_with("sub")), "{n:?}");

    // Adding over an existing name replaces it instead of duplicating it.
    let newer = dir.path().join("a2.txt");
    fs::write(&newer, "replaced").unwrap();
    h.add(&archive, &[(newer, "a.txt".into())], None).unwrap();
    let listing = h.list(&archive, None).unwrap();
    let a: Vec<_> = listing.entries.iter().filter(|e| e.name == "a.txt").collect();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].size, 8);
}

#[test]
fn errors_are_reported_not_fatal() {
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.zip");
    let err = h.list(&missing, None).unwrap_err();
    assert_eq!(err.code, "notfound");
    // The helper survives a failed request.
    let bogus = dir.path().join("plain.txt");
    fs::write(&bogus, "not an archive").unwrap();
    let (is_archive, _, _) = h.probe(&bogus).unwrap();
    assert!(!is_archive);
    assert!(h.is_running());
    h.shutdown();
    assert!(!h.is_running());
}

#[test]
fn long_operations_report_progress() {
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("many");
    std::fs::create_dir_all(&src).unwrap();
    // Random content compresses slowly enough for a few progress samples.
    let mut seed: u64 = 42;
    for i in 0..96 {
        let data: Vec<u8> = (0..256 * 1024)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (seed >> 33) as u8
            })
            .collect();
        fs::write(src.join(format!("f{i:03}.bin")), data).unwrap();
    }
    let archive = dir.path().join("big.tar.gz");
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let s2 = seen.clone();
    failbrauwser::archive::client::with_progress(move |p| s2.borrow_mut().push(p.clone()), || {
        h.add(&archive, &[(src.clone(), "many".into())], None).unwrap();
    });
    let seen = seen.borrow();
    assert!(!seen.is_empty(), "no progress reported");
    for p in seen.iter() {
        assert!(p.total >= 24 << 20, "{p:?}");
        assert!(p.done <= p.total, "{p:?}");
        assert!(p.phase == "adding" || p.phase == "writing", "{p:?}");
    }
}

#[test]
fn encrypted_archives_stay_encrypted_when_edited() {
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
    fs::write(&a, "secret a").unwrap();
    fs::write(&b, "secret b").unwrap();
    let zip = dir.path().join("e.zip");
    h.add(&zip, &[(a.clone(), "a.txt".into())], Some("pw")).unwrap();
    // Adding with the password must not add a plain entry.
    h.add(&zip, &[(b.clone(), "sub/b.txt".into())], Some("pw")).unwrap();
    let l = h.list(&zip, None).unwrap();
    assert!(l.entries.iter().filter(|e| !e.is_dir).all(|e| e.encrypted), "{:?}", l.entries);
    assert_eq!(l.entries.iter().filter(|e| !e.is_dir).count(), 2);
    h.remove(&zip, &["a.txt".into()], Some("pw")).unwrap();
    assert!(h.list(&zip, None).unwrap().entries.iter().filter(|e| !e.is_dir).all(|e| e.encrypted));
    // Wrong or missing passwords are reported as such.
    let out = dir.path().join("out");
    assert!(h.extract(&zip, None, &out, None).unwrap_err().needs_password());
    assert!(h.extract(&zip, None, &out, Some("nope")).unwrap_err().needs_password());
    h.extract(&zip, None, &out, Some("pw")).unwrap();
    assert_eq!(fs::read_to_string(out.join("sub/b.txt")).unwrap(), "secret b");
    let sz = dir.path().join("e.7z");
    h.add(&sz, &[(a, "a.txt".into())], Some("pw")).unwrap();
    assert!(h.extract(&sz, None, &dir.path().join("o7"), None).unwrap_err().needs_password());
}

fn xorriso(args: &[&str]) -> Option<String> {
    let out = Command::new("xorriso").args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr))
}

#[test]
fn iso_edits_keep_rock_ridge_joliet_and_boot_records() {
    let Some(h) = helper() else { return };
    if xorriso(&["-version"]).is_none() {
        eprintln!("xorriso missing, skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("boot")).unwrap();
    fs::create_dir_all(src.join("empty")).unwrap();
    let long = "a name that is much longer than the sixty-four characters Joliet can hold.txt";
    fs::write(src.join(long), "long").unwrap();
    fs::write(src.join("run.sh"), "#!/bin/sh\n").unwrap();
    // The executable bit is what Rock Ridge records on Unix; there is none on Windows.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(src.join("boot/isolinux.bin"), vec![0x5au8; 4096]).unwrap();
    fs::write(src.join("boot/efi.img"), vec![0xa5u8; 32768]).unwrap();
    let iso = dir.path().join("b.iso");
    let made = Command::new("xorriso")
        .args(["-as", "mkisofs", "-R", "-J", "-V", "KEEPME", "-b", "boot/isolinux.bin", "-c", "boot/boot.cat", "-no-emul-boot", "-boot-load-size", "4", "-boot-info-table", "-eltorito-alt-boot", "-e", "boot/efi.img", "-no-emul-boot", "-o"])
        .arg(&iso)
        .arg(&src)
        .output()
        .unwrap();
    assert!(made.status.success());

    let extra = dir.path().join("extra.txt");
    fs::write(&extra, "added").unwrap();
    h.add(&iso, &[(extra, "new folder/extra.txt".into())], None).unwrap();
    h.rename(&iso, "run.sh", "start.sh", None).unwrap();

    let iso_s = iso.to_str().unwrap();
    let rr = xorriso(&["-indev", iso_s, "-find", "/", "-exec", "lsdl"]).unwrap();
    assert!(rr.contains(&format!("'/{long}'")), "{rr}");
    assert!(rr.contains("'/new folder/extra.txt'"), "{rr}");
    assert!(rr.contains("'/empty'"), "{rr}");
    assert!(rr.lines().any(|l| l.starts_with("-rwxr-xr-x") && l.ends_with("'/start.sh'")), "{rr}");
    let joliet = xorriso(&["-rockridge", "off", "-indev", iso_s, "-find", "/"]).unwrap();
    assert!(joliet.contains("'/new folder/extra.txt'"), "{joliet}");
    let boot = xorriso(&["-indev", iso_s, "-report_el_torito", "plain"]).unwrap();
    assert!(boot.contains("El Torito img path :   1  /boot/isolinux.bin"), "{boot}");
    assert!(boot.contains("El Torito img opts :   1  boot-info-table"), "{boot}");
    assert!(boot.contains("El Torito img path :   2  /boot/efi.img"), "{boot}");
    assert!(boot.contains("Volume id    : 'KEEPME'"), "{boot}");
    // And our own listing reads it back with the full names.
    let names: Vec<String> = h.list(&iso, None).unwrap().entries.into_iter().map(|e| e.name).collect();
    assert!(names.contains(&long.to_string()), "{names:?}");
}

/// The format table compiled into the program matches what the helper reports
/// (`FB_REGEN_FORMATS=1` rewrites it).
#[test]
fn formats_table() {
    let Some(h) = helper() else { return };
    let mut out = String::from("// Generated from `fb-archive` by the `formats_table` test in tests/archive_helper.rs.\n// Do not edit; regenerate with FB_REGEN_FORMATS=1.\n\npub static FORMATS: &[Format] = &[\n");
    for f in h.formats().unwrap() {
        let s = |k: &str| f[k].as_str().unwrap().to_string();
        let b = |k: &str| f[k].as_bool().unwrap();
        let kind = match f["kind"].as_str().unwrap() {
            "archive" => "Archive",
            "filesystem" => "Filesystem",
            "tar" => "Tar",
            "stream" => "Stream",
            _ => "Wrapper",
        };
        let exts: Vec<String> = f["ext"].as_array().unwrap().iter().map(|e| format!("{:?}", e.as_str().unwrap())).collect();
        out += &format!(
            "    Format {{ id: {:?}, name: {:?}, kind: Kind::{kind}, create: {}, modify: {}, password: {}, dirs: {}, multi: {}, exts: &[{}] }},\n",
            s("id"), s("name"), b("create"), b("modify"), b("password"), b("dirs"), b("multi"), exts.join(", ")
        );
    }
    out += "];\n";
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/archive/formats_table.rs");
    if std::env::var_os("FB_REGEN_FORMATS").is_some() {
        fs::write(&file, &out).unwrap();
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), out, "format table out of date: FB_REGEN_FORMATS=1 cargo test --test archive_helper formats_table");
}

#[test]
#[cfg(unix)] // the stuck stimulus is a FIFO; the Windows abort twin comes with the harness milestone
fn abort_ends_a_request_that_never_finishes() {
    let Some(h) = helper() else { return };
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    sample_tree(&src);
    let archive = dir.path().join("a.zip");
    h.add(&archive, &[(src.clone(), "src".into())], None).unwrap();
    // Reading a pipe nobody writes to blocks for good, like a share that stopped answering.
    let stuck = dir.path().join("stuck.zip");
    let fifo = std::ffi::CString::new(stuck.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let h2 = h.clone();
    let out = dir.path().join("out");
    let worker = std::thread::spawn(move || h2.extract(&stuck, None, &out, None));
    std::thread::sleep(std::time::Duration::from_secs(2));
    if worker.is_finished() {
        panic!("the request should be stuck on the pipe: {:?}", worker.join().unwrap());
    }
    h.abort();
    let started = std::time::Instant::now();
    while !worker.is_finished() {
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "abort did not end the request");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(worker.join().unwrap().is_err());
    // The next request gets a fresh helper.
    assert!(names(&h, &archive).contains(&"src/a.txt".to_string()));
}
