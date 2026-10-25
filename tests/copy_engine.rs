//! Copy / move / delete behaviour on real directories.

use failbrauwser::ops::copy::{Mode, Options, transfer};
use failbrauwser::ops::job::{Answer, JobCtx, Question, Reply};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

fn answer(a: Answer) -> std::sync::Arc<JobCtx> {
    JobCtx::with_answers(move |_| Reply { answer: a, for_all: true })
}

fn opts() -> Options {
    Options { smooth_writes: true, workers: None, verify: false }
}

fn make_tree(root: &Path) {
    fs::create_dir_all(root.join("sub/deep")).unwrap();
    fs::write(root.join("a.txt"), "alpha").unwrap();
    fs::write(root.join("sub/b.bin"), vec![3u8; 3 << 20]).unwrap();
    fs::write(root.join("sub/deep/c"), "").unwrap();
    fs::set_permissions(root.join("a.txt"), fs::Permissions::from_mode(0o751)).unwrap();
    std::os::unix::fs::symlink("../a.txt", root.join("sub/link")).unwrap();
    let t = libc::timespec { tv_sec: 1_600_000_000, tv_nsec: 123_000_000 };
    let c = std::ffi::CString::new(root.join("a.txt").to_str().unwrap()).unwrap();
    unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [t, t].as_ptr(), 0) };
}

#[test]
fn copies_tree_with_metadata_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    make_tree(&src);
    let dest = dir.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let ctx = JobCtx::new();
    let tops = transfer(&ctx, &[src.clone()], &dest, Mode::Copy, &opts()).unwrap();
    assert_eq!(tops, vec![dest.join("src")]);
    let out = dest.join("src");
    assert_eq!(fs::read_to_string(out.join("a.txt")).unwrap(), "alpha");
    assert_eq!(fs::read(out.join("sub/b.bin")).unwrap().len(), 3 << 20);
    assert!(out.join("sub/deep/c").exists());
    let m = fs::metadata(out.join("a.txt")).unwrap();
    assert_eq!(m.mode() & 0o7777, 0o751);
    assert_eq!((m.mtime(), m.mtime_nsec()), (1_600_000_000, 123_000_000));
    assert_eq!(fs::read_link(out.join("sub/link")).unwrap(), Path::new("../a.txt"));
    let snap = ctx.snapshot();
    assert_eq!(snap.bytes_done, snap.bytes_total);
    assert_eq!(snap.files_done, snap.files_total);
    assert!(src.join("a.txt").exists(), "copy keeps the source");
}

#[test]
fn copy_into_same_folder_makes_a_copy_name() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("n.txt"), "1").unwrap();
    let ctx = JobCtx::new();
    transfer(&ctx, &[dir.path().join("n.txt")], dir.path(), Mode::Copy, &opts()).unwrap();
    transfer(&ctx, &[dir.path().join("n.txt")], dir.path(), Mode::Copy, &opts()).unwrap();
    assert!(dir.path().join("n (copy).txt").exists());
    assert!(dir.path().join("n (copy 2).txt").exists());
}

#[test]
fn refuses_to_copy_folder_into_itself() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("x/y")).unwrap();
    let ctx = JobCtx::new();
    transfer(&ctx, &[dir.path().join("x")], &dir.path().join("x/y"), Mode::Copy, &opts()).unwrap();
    assert!(!dir.path().join("x/y/x").exists());
    assert_eq!(ctx.errors().len(), 1);
}

#[test]
fn conflicts_skip_replace_keep_both() {
    for (a, expect_old, expect_extra) in [(Answer::Skip, true, false), (Answer::Replace, false, false), (Answer::KeepBoth, true, true)] {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("s"), dir.path().join("d"));
        fs::create_dir_all(src.join("f")).unwrap();
        fs::create_dir_all(dst.join("f")).unwrap();
        fs::write(src.join("f/x.txt"), "new").unwrap();
        fs::write(dst.join("f/x.txt"), "old").unwrap();
        fs::write(src.join("f/only-new"), "n").unwrap();
        let ctx = answer(a);
        transfer(&ctx, &[src.join("f")], &dst, Mode::Copy, &opts()).unwrap();
        // Folders merge; the conflicting file follows the answer.
        assert!(dst.join("f/only-new").exists());
        let content = fs::read_to_string(dst.join("f/x.txt")).unwrap();
        assert_eq!(content == "old", expect_old, "{a:?}");
        assert_eq!(dst.join("f/x (2).txt").exists(), expect_extra, "{a:?}");
    }
}

#[test]
fn cancel_on_conflict_stops_and_leaves_no_partial_files() {
    let dir = tempfile::tempdir().unwrap();
    let (src, dst) = (dir.path().join("s"), dir.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("x"), "new").unwrap();
    fs::write(dst.join("x"), "old").unwrap();
    let ctx = answer(Answer::Cancel);
    assert!(transfer(&ctx, &[src.join("x")], &dst, Mode::Copy, &opts()).is_err());
    assert_eq!(fs::read_to_string(dst.join("x")).unwrap(), "old");
}

#[test]
fn move_within_filesystem_renames_and_merges() {
    let dir = tempfile::tempdir().unwrap();
    let (src, dst) = (dir.path().join("s"), dir.path().join("d"));
    make_tree(&src.join("t"));
    fs::create_dir_all(dst.join("t/sub")).unwrap();
    fs::write(dst.join("t/sub/existing"), "e").unwrap();
    let ino = fs::metadata(src.join("t/sub/b.bin")).unwrap().ino();
    let ctx = JobCtx::new();
    transfer(&ctx, &[src.join("t")], &dst, Mode::Move, &opts()).unwrap();
    assert!(!src.join("t").exists());
    assert!(dst.join("t/sub/existing").exists());
    // Same inode: the file was renamed, not copied.
    assert_eq!(fs::metadata(dst.join("t/sub/b.bin")).unwrap().ino(), ino);
}

#[test]
fn move_across_filesystems_copies_then_deletes() {
    let shm = Path::new("/dev/shm");
    if !shm.is_dir() {
        return;
    }
    let from = tempfile::tempdir_in(shm).unwrap();
    let to = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    if fs::metadata(from.path()).unwrap().dev() == fs::metadata(to.path()).unwrap().dev() {
        return;
    }
    make_tree(&from.path().join("t"));
    let ctx = JobCtx::new();
    transfer(&ctx, &[from.path().join("t")], to.path(), Mode::Move, &opts()).unwrap();
    assert!(!from.path().join("t").exists());
    assert_eq!(fs::read_to_string(to.path().join("t/a.txt")).unwrap(), "alpha");
    assert!(fs::symlink_metadata(to.path().join("t/sub/link")).unwrap().file_type().is_symlink());
}

#[test]
fn many_small_files_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("many");
    for d in 0..20 {
        fs::create_dir_all(src.join(format!("d{d}"))).unwrap();
        for f in 0..50 {
            fs::write(src.join(format!("d{d}/f{f}")), format!("{d}-{f}")).unwrap();
        }
    }
    let dest = dir.path().join("out");
    fs::create_dir(&dest).unwrap();
    let ctx = JobCtx::new();
    transfer(&ctx, &[src], &dest, Mode::Copy, &Options { smooth_writes: false, workers: Some(8), verify: false }).unwrap();
    assert_eq!(ctx.snapshot().files_done, 1000);
    assert_eq!(fs::read_to_string(dest.join("many/d7/f42")).unwrap(), "7-42");
}

#[test]
fn unreadable_file_can_be_skipped() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("s");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("secret"), "x").unwrap();
    fs::write(src.join("fine"), "y").unwrap();
    fs::set_permissions(src.join("secret"), fs::Permissions::from_mode(0o000)).unwrap();
    let dest = dir.path().join("d");
    fs::create_dir(&dest).unwrap();
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let a2 = asked.clone();
    let ctx = JobCtx::with_answers(move |q| {
        a2.lock().unwrap().push(q.clone());
        Reply { answer: Answer::Skip, for_all: false }
    });
    transfer(&ctx, &[src], &dest, Mode::Copy, &opts()).unwrap();
    assert!(dest.join("s/fine").exists());
    assert!(!dest.join("s/secret").exists());
    assert!(matches!(asked.lock().unwrap()[0], Question::Error { .. }));
}

#[test]
fn replace_keeps_old_file_when_new_copy_fails() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (src, dst) = (dir.path().join("s"), dir.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("x"), "new").unwrap();
    fs::write(dst.join("x"), "old").unwrap();
    fs::set_permissions(src.join("x"), fs::Permissions::from_mode(0o000)).unwrap();
    let ctx = JobCtx::with_answers(|q| match q {
        Question::Conflict { .. } => Reply { answer: Answer::Replace, for_all: true },
        _ => Reply { answer: Answer::Skip, for_all: true },
    });
    transfer(&ctx, &[src.join("x")], &dst, Mode::Copy, &opts()).unwrap();
    assert_eq!(fs::read_to_string(dst.join("x")).unwrap(), "old");
    let leftovers: Vec<_> = fs::read_dir(&dst).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
}

#[test]
fn replace_swaps_in_new_content() {
    let dir = tempfile::tempdir().unwrap();
    let (src, dst) = (dir.path().join("s"), dir.path().join("d"));
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("x"), "new").unwrap();
    fs::write(dst.join("x"), "old").unwrap();
    transfer(&answer(Answer::Replace), &[src.join("x")], &dst, Mode::Copy, &opts()).unwrap();
    assert_eq!(fs::read_to_string(dst.join("x")).unwrap(), "new");
    assert_eq!(fs::read_dir(&dst).unwrap().count(), 1);
}

#[test]
fn verified_copy_counts_both_passes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    make_tree(&src);
    let dest = dir.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let ctx = JobCtx::new();
    transfer(&ctx, &[src], &dest, Mode::Copy, &Options { smooth_writes: false, workers: None, verify: true }).unwrap();
    let s = ctx.snapshot();
    assert_eq!(s.bytes_done, s.bytes_total);
    assert_eq!(s.bytes_total, 2 * ((3 << 20) + 5));
    assert!(ctx.errors().is_empty());
}

#[test]
fn verification_finds_differences() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b, c) = (dir.path().join("a"), dir.path().join("b"), dir.path().join("c"));
    fs::write(&a, vec![1u8; 2 << 20]).unwrap();
    fs::write(&b, vec![1u8; 2 << 20]).unwrap();
    let mut bad = vec![1u8; 2 << 20];
    bad[1_500_000] = 2;
    fs::write(&c, bad).unwrap();
    let ctx = JobCtx::new();
    let mut n = 0;
    failbrauwser::ops::copy::verify_copy(&ctx, &a, &b, &mut n).unwrap();
    assert!(failbrauwser::ops::copy::verify_copy(&ctx, &a, &c, &mut n).is_err());
    fs::write(&c, vec![1u8; 10]).unwrap();
    assert!(failbrauwser::ops::copy::verify_copy(&ctx, &a, &c, &mut n).is_err());
}

#[test]
fn paused_copy_waits_and_then_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big");
    fs::write(&src, vec![5u8; 64 << 20]).unwrap();
    let dest = dir.path().join("d");
    fs::create_dir(&dest).unwrap();
    let ctx = JobCtx::new();
    ctx.pause();
    let (c2, s2, d2) = (ctx.clone(), src.clone(), dest.clone());
    let t = std::thread::spawn(move || transfer(&c2, &[s2], &d2, Mode::Copy, &opts()));
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(ctx.snapshot().bytes_done < 64 << 20, "a paused copy must not finish");
    ctx.resume();
    t.join().unwrap().unwrap();
    assert_eq!(fs::metadata(dest.join("big")).unwrap().len(), 64 << 20);
}

/// The GVfs FUSE folder for a local folder, through GVfs's `localtest` backend, when the
/// session has GVfs with its FUSE daemon.
fn through_gvfs(dir: &Path) -> Option<std::path::PathBuf> {
    let fuse = gtk::glib::user_runtime_dir().join("gvfs");
    let ok = std::process::Command::new("gio").args(["mount", "localtest:///"]).stdin(std::process::Stdio::null()).output().is_ok();
    let p = fuse.join(format!("localtest:{}", dir.display()));
    (ok && p.is_dir()).then_some(p)
}

#[test]
fn copies_onto_gvfs_shares_through_gio() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    make_tree(&src);
    for i in 0..40 {
        fs::write(src.join(format!("sub/many{i}")), vec![i as u8; 70_000]).unwrap();
    }
    fs::create_dir(dir.path().join("dest")).unwrap();
    let Some(dest) = through_gvfs(&dir.path().join("dest")) else {
        eprintln!("GVfs FUSE with the localtest backend not available, skipping");
        return;
    };
    assert!(failbrauwser::ops::copy::is_gvfs_path(&dest));
    let ctx = JobCtx::new();
    transfer(&ctx, &[src.clone()], &dest, Mode::Copy, &opts()).unwrap();
    let out = dir.path().join("dest/src");
    assert_eq!(fs::read_to_string(out.join("a.txt")).unwrap(), "alpha");
    assert_eq!(fs::read(out.join("sub/b.bin")).unwrap(), vec![3u8; 3 << 20]);
    assert_eq!(fs::read(out.join("sub/many39")).unwrap(), vec![39u8; 70_000]);
    assert_eq!(fs::metadata(out.join("a.txt")).unwrap().mtime(), 1_600_000_000);
    assert_eq!(fs::read_link(out.join("sub/link")).unwrap(), Path::new("../a.txt"));
    let s = ctx.snapshot();
    assert_eq!(s.bytes_done, s.bytes_total);
    assert!(ctx.errors().is_empty(), "{:?}", ctx.errors());
}

fn open_fds() -> usize {
    fs::read_dir("/proc/self/fd").unwrap().count()
}

#[test]
fn copying_many_files_onto_gvfs_shares_keeps_no_descriptors_open() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    for i in 0..300 {
        fs::write(src.join(format!("f{i}")), "x").unwrap();
    }
    fs::create_dir(dir.path().join("dest")).unwrap();
    let Some(dest) = through_gvfs(&dir.path().join("dest")) else {
        eprintln!("GVfs FUSE with the localtest backend not available, skipping");
        return;
    };
    // Warms up GIO's own connections, which stay for the life of the process.
    transfer(&JobCtx::new(), &[src.join("f0")], &dest, Mode::Copy, &opts()).unwrap();
    let before = open_fds();
    let ctx = JobCtx::new();
    transfer(&ctx, &[src], &dest, Mode::Copy, &opts()).unwrap();
    assert!(ctx.errors().is_empty(), "{:?}", ctx.errors());
    assert_eq!(fs::read_dir(dir.path().join("dest/src")).unwrap().count(), 300);
    let after = open_fds();
    assert!(after < before + 20, "{before} descriptors open before the copy, {after} after");
}
