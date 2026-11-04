//! Windows twins of the copy/move scenarios in `copy_engine.rs`: the same engine driven
//! through real NTFS folders, with the semantics a Windows filesystem actually has —
//! attributes instead of mode bits, case-insensitive names, sharing locks from other
//! processes, and paths beyond 260 characters served by the embedded longPathAware
//! manifest. Symlinks are absent here on purpose: unprivileged link creation is a
//! Developer-Mode feature, and the scenario for links runs on Unix.

use failbrauwser::ops::copy::{Mode, Options, rename_replacing, transfer};
use failbrauwser::ops::job::{Answer, JobCtx, Reply};
use failbrauwser::osprim;
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

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
    osprim::set_attrs(&root.join("a.txt"), 0x3); // READONLY | HIDDEN
    osprim::set_times(&root.join("a.txt"), (1_600_000_000, 0), (1_600_000_000, 123_000_000), true);
}

#[test]
fn copies_tree_with_attributes_and_times() {
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
    assert_eq!(osprim::attrs_of(&out.join("a.txt")), 0x3, "read-only and hidden travel to the copy");
    let secs = fs::metadata(out.join("a.txt")).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap();
    assert_eq!(secs, Duration::new(1_600_000_000, 123_000_000), "the modification time comes back unchanged, sub-second included");
    let snap = ctx.snapshot();
    assert_eq!(snap.bytes_done, snap.bytes_total);
    assert_eq!(snap.files_done, snap.files_total);
    assert!(src.join("a.txt").exists(), "copy keeps the source");
    assert_eq!(osprim::attrs_of(&src.join("a.txt")), 0x3, "and leaves its attributes alone");
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
fn case_only_rename_keeps_one_file() {
    // The NTFS spelling of the "same file, different case" case: the engine must not
    // refuse, duplicate, or lose the content.
    let dir = tempfile::tempdir().unwrap();
    let lower = dir.path().join("a.txt");
    fs::write(&lower, "alpha").unwrap();
    rename_replacing(&lower, &dir.path().join("A.txt")).unwrap();
    let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(entries, vec!["A.txt".to_string()]);
    assert_eq!(fs::read_to_string(dir.path().join("A.txt")).unwrap(), "alpha");
}

#[test]
fn sharing_violation_retries_then_succeeds() {
    // An exclusive handle on the source (an AV scan or preview handler standing in the
    // way): the engine must wait it out on its own and never bother the user.
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("locked.txt");
    fs::write(&src, "payload").unwrap();
    let dest = dir.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let blocker = OpenOptions::new().read(true).share_mode(0).open(&src).unwrap(); // share mode 0 = nobody else may open it
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(blocker);
    });
    let ctx = JobCtx::new();
    transfer(&ctx, &[src.clone()], &dest, Mode::Copy, &opts()).unwrap();
    assert_eq!(fs::read_to_string(dest.join("locked.txt")).unwrap(), "payload");
    assert_eq!(ctx.errors().len(), 0, "the waiting happens quietly: nobody is asked");
}

#[test]
fn long_paths_copy_and_read_back() {
    // Beyond 260 characters: the longPathAware manifest makes real paths work without
    // the engines ever seeing a \\?\ prefix.
    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().join("src");
    for i in 0..6 {
        deep = deep.join(format!("level{i}-{}", "x".repeat(40)));
    }
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("file.txt"), "deep content").unwrap();
    let deep_len = deep.join("file.txt").to_string_lossy().len();
    assert!(deep_len > 260, "the fixture must actually pass the old MAX_PATH: {deep_len}");
    let dest = dir.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let ctx = JobCtx::new();
    transfer(&ctx, &[dir.path().join("src")], &dest, Mode::Copy, &opts()).unwrap();
    let mut mirror = dest.join("src");
    for i in 0..6 {
        mirror = mirror.join(format!("level{i}-{}", "x".repeat(40)));
    }
    assert_eq!(fs::read_to_string(mirror.join("file.txt")).unwrap(), "deep content");
}

#[test]
fn verify_option_reads_every_copy_back() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    fs::write(&src, vec![7u8; 5 << 20]).unwrap();
    let dest = dir.path().join("dest");
    fs::create_dir(&dest).unwrap();
    let ctx = JobCtx::new();
    transfer(&ctx, &[src], &dest, Mode::Copy, &Options { verify: true, ..opts() }).unwrap();
    assert_eq!(fs::read(dest.join("src.bin")).unwrap().len(), 5 << 20);
    let snap = ctx.snapshot();
    assert_eq!(snap.bytes_done, snap.bytes_total, "verification counted its own read pass");
    assert_eq!(ctx.errors().len(), 0);
}

/// Measurement, not assertion: the engine against a raw `std::fs::copy` loop on the
/// same fixture. Run explicitly with `cargo test --release --test copy_engine_windows
/// -- --ignored`; numbers go into the journal, never into a pass/fail gate.
#[test]
#[ignore] // a timing measurement — deliberately outside the deterministic suite
fn perf_baseline_engine_vs_raw_copy() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("many")).unwrap();
    fs::write(src.join("big.bin"), vec![0xA5u8; 128 << 20]).unwrap();
    for i in 0..1_000 {
        fs::write(src.join(format!("many/f{i:04}.bin")), vec![i as u8; 64 << 10]).unwrap();
    }
    let run = |label: &str, engine: bool, subset: &Path| {
        let dest = dir.path().join(label.replace(' ', "-"));
        fs::create_dir_all(&dest).unwrap();
        let start = std::time::Instant::now();
        let mut errs = 0usize;
        if engine {
            let ctx = JobCtx::new();
            transfer(&ctx, &[subset.to_path_buf()], &dest, Mode::Copy, &opts()).unwrap();
            errs = ctx.errors().len();
        } else {
            // Same shape as the engine's plan: folders first, then every file.
            let base = if subset.is_dir() { subset.to_path_buf() } else { subset.parent().unwrap().to_path_buf() };
            let mut stack = if subset.is_dir() { vec![subset.to_path_buf()] } else { vec![] };
            let mut files = if subset.is_dir() { vec![] } else { vec![subset.to_path_buf()] };
            while let Some(d) = stack.pop() {
                for e in fs::read_dir(&d).unwrap() {
                    let e = e.unwrap();
                    if e.file_type().unwrap().is_dir() {
                        fs::create_dir_all(dest.join(e.path().strip_prefix(&base).unwrap())).unwrap();
                        stack.push(e.path());
                    } else {
                        files.push(e.path());
                    }
                }
            }
            for f in files {
                fs::copy(&f, dest.join(f.strip_prefix(&base).unwrap())).unwrap();
            }
        }
        let secs = start.elapsed().as_secs_f64();
        println!("{label}: {secs:.2} s (engine errors: {errs})");
    };
    run("engine one big file", true, &src.join("big.bin"));
    run("engine 1000 small files", true, &src.join("many"));
    run("engine full tree", true, &src);
    run("raw one big file", false, &src.join("big.bin"));
    run("raw 1000 small files", false, &src.join("many"));
    run("raw full tree", false, &src);
}

#[test]
fn move_within_filesystem_renames_and_merges() {
    let dir = tempfile::tempdir().unwrap();
    let (src, dst) = (dir.path().join("s"), dir.path().join("d"));
    make_tree(&src.join("t"));
    fs::create_dir_all(dst.join("t/sub")).unwrap();
    fs::write(dst.join("t/sub/existing"), "e").unwrap();
    let before = osprim::identity(&src.join("t/sub/b.bin"), &fs::metadata(src.join("t/sub/b.bin")).unwrap());
    let ctx = JobCtx::new();
    transfer(&ctx, &[src.join("t")], &dst, Mode::Move, &opts()).unwrap();
    assert!(!src.join("t").exists());
    assert!(dst.join("t/sub/existing").exists());
    // Same (volume, file id) pair: the big file was renamed, not copied — and it
    // still carries its attributes through the rename.
    let after = dst.join("t/sub/b.bin");
    assert_eq!(osprim::identity(&after, &fs::metadata(&after).unwrap()), before);
}
