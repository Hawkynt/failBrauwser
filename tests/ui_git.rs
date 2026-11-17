//! The Git integration in the real window (TortoiseGit's overlays, menus and dialogs), run inside a
//! headless sway session (Linux) or on the win32 GDK surface (Windows). Git is configured for each
//! test alone (GIT_CONFIG_GLOBAL), never from the user's own configuration, and the fixture is built
//! by git's own commands: a run needs git on PATH and nothing else beyond the platform's UI tools.

use std::path::Path;
use std::process::Command;

#[cfg(unix)]
fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

#[cfg(windows)]
fn have(tool: &str) -> bool {
    Command::new("where").arg(tool).output().is_ok_and(|o| o.status.success())
}

fn git_env(t: &Path) -> Vec<(String, String)> {
    vec![("GIT_CONFIG_GLOBAL".into(), t.join("gitconfig").display().to_string()), ("GIT_CONFIG_NOSYSTEM".into(), "1".into())]
}

/// One client command, given as arguments rather than a shell string: the fixture has to mean the
/// same thing wherever it runs, and a shell string means whatever that shell's `echo` and `printf`
/// happen to do with a backslash (dash and bash disagree — `echo 'a\nb'` is one line or two).
fn cli(t: &Path, dir: &Path, program: &str, args: &[&str]) -> String {
    let out = Command::new(program).current_dir(dir).args(args).envs(git_env(t)).output().unwrap();
    assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// git with the test's own configuration, in `dir`.
fn git(t: &Path, dir: &Path, args: &[&str]) -> String {
    cli(t, dir, "git", args)
}

/// Appends to a file the way `echo x >> file` does.
fn append(path: &Path, text: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn run(script: &str, setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    run_with(script, true, setup, check);
}

/// `enabled`: the Git integration is switched on (Edit → Version Control Integration) from the start.
fn run_with(script: &str, enabled: bool, setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    #[cfg(unix)]
    if !(have("sway") || have("tilewin")) || !["grim", "dbus-run-session"].iter().all(|tool| have(tool)) {
        eprintln!("headless UI tools missing, skipping {script}");
        return;
    }
    if !have("git") {
        eprintln!("git is not on PATH, skipping {script}");
        return;
    }
    // Outside of this project's own working tree.
    let dir = tempfile::Builder::new().prefix("fb-git-ui").tempdir().unwrap();
    let t = dir.path();
    std::fs::write(t.join("gitconfig"), "[user]\n\tname = Ann Tester\n\temail = ann@example.org\n[init]\n\tdefaultBranch = main\n").unwrap();
    if enabled {
        std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
        std::fs::write(t.join("config/failbrauwser/settings.ini"), "[General]\nvcs_integration=true\n").unwrap();
    }
    setup(t);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let script_path = root.join("tests/ui").join(script);
    let bin = env!("CARGO_BIN_EXE_failbrauwser");
    // The win32 twin of that harness: same arguments, same contract, same "SELFTEST OK".
    #[cfg(unix)]
    let mut run = Command::new(root.join("tests/ui/harness.sh"));
    #[cfg(windows)]
    let mut run = {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(root.join("tests/ui/harness.ps1"));
        c
    };
    // What the harness prints goes into a file, not a pipe: a pipe stays open while any tool the run
    // started is alive, whoever reads it then waits for an end that never comes, and the test says
    // nothing at all (measured here on 2026-11-17 — the scenario had answered `SELFTEST OK`, the
    // check afterwards never ran, and the run sat for five minutes before it was ended by hand).
    let log = t.join("harness.log");
    let file = std::fs::File::create(&log).unwrap();
    let status = run.envs(git_env(t)).arg(bin).arg(&script_path).arg(t).stdout(file.try_clone().unwrap()).stderr(file).status().unwrap();
    // Read as bytes and decoded leniently: a client answers in the machine's own codepage, and a
    // strict UTF-8 read would lose the very message the scenario is there to show.
    let log = std::fs::read(&log).unwrap_or_else(|e| panic!("{script}: no harness log: {e}"));
    let log = String::from_utf8_lossy(&log).into_owned();
    assert!(status.success() && log.contains("SELFTEST OK"), "{script} failed:\n{log}");
    check(t);
}

/// A working tree with two commits and changes of each kind.
fn repo(t: &Path) {
    let r = t.join("repo");
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::create_dir_all(r.join("docs")).unwrap();
    std::fs::write(r.join("clean.txt"), "a\n").unwrap();
    std::fs::write(r.join("changed.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(r.join("src/lib.rs"), "s\n").unwrap();
    std::fs::write(r.join("docs/readme.md"), "d\n").unwrap();
    std::fs::write(r.join(".gitignore"), "*.log\n").unwrap();
    git(t, &r, &["init", "-q"]);
    git(t, &r, &["add", "-A"]);
    git(t, &r, &["commit", "-qm", "First commit"]);
    append(&r.join("changed.txt"), "four\n");
    git(t, &r, &["commit", "-qam", "Second commit"]);
    git(t, &r, &["tag", "v1"]);
    // One of every state the overlays and the Commit window have to tell apart: changed, staged,
    // unversioned, ignored, and a change inside a folder.
    std::fs::write(r.join("changed.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::write(r.join("added.txt"), "n\n").unwrap();
    git(t, &r, &["add", "added.txt"]);
    std::fs::write(r.join("new.txt"), "u\n").unwrap();
    std::fs::write(r.join("build.log"), "l\n").unwrap();
    append(&r.join("src/lib.rs"), "x\n");
}

/// Keeps the windows a test drew, for looking at them (target/git-ui/).
fn keep_snapshots(t: &Path) {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("git-ui");
    std::fs::create_dir_all(&out).unwrap();
    for e in std::fs::read_dir(t).unwrap().flatten() {
        if e.path().extension().is_some_and(|x| x == "png") {
            let png = std::fs::read(e.path()).unwrap();
            assert!(png.len() > 1000 && png.starts_with(b"\x89PNG"), "{:?} is no picture", e.path());
            std::fs::copy(e.path(), out.join(e.file_name())).unwrap();
        }
    }
}

#[test]
fn commit_what_is_ticked() {
    run("git_commit.fbt", repo, |t| {
        let r = t.join("repo");
        assert_eq!(git(t, &r, &["log", "-1", "--format=%s"]).trim(), "Change two files");
        assert_eq!(git(t, &r, &["show", "--name-only", "--format=", "HEAD"]).trim(), "changed.txt\nnew.txt");
        assert_eq!(git(t, &r, &["status", "--porcelain"]).trim(), "M src/lib.rs\n?? added.txt");
        keep_snapshots(t);
    });
}

#[test]
fn log_diff_and_blame() {
    run("git_log.fbt", repo, |t| {
        let r = t.join("repo");
        assert_eq!(git(t, &r, &["rev-parse", "v0"]).trim(), git(t, &r, &["rev-parse", "HEAD~1"]).trim());
        keep_snapshots(t);
    });
}

/// A bare remote with two commits; `repo` cloned from it with a commit of its own, and a
/// commit pushed to the remote from elsewhere meanwhile.
fn remote(t: &Path) {
    git(t, t, &["init", "-q", "--bare", "remote.git"]);
    // Cloning a bare repository with nothing in it warns on stderr: the clone still answers zero.
    git(t, t, &["clone", "-q", "remote.git", "repo"]);
    let r = t.join("repo");
    std::fs::write(r.join("clean.txt"), "a\n").unwrap();
    git(t, &r, &["add", "-A"]);
    git(t, &r, &["commit", "-qm", "First commit"]);
    git(t, &r, &["push", "-q", "-u", "origin", "main"]);
    git(t, t, &["clone", "-q", "remote.git", "other"]);
    let o = t.join("other");
    std::fs::write(o.join("other.txt"), "o\n").unwrap();
    git(t, &o, &["add", "other.txt"]);
    git(t, &o, &["commit", "-qm", "From elsewhere"]);
    git(t, &o, &["push", "-q"]);
    std::fs::write(r.join("local.txt"), "l\n").unwrap();
    git(t, &r, &["add", "local.txt"]);
    git(t, &r, &["commit", "-qm", "Local commit"]);
}

#[test]
fn sync_pull_push_fetch_and_clone() {
    run("git_remote.fbt", remote, |t| {
        let r = t.join("repo");
        assert_eq!(git(t, &r, &["rev-parse", "HEAD"]).trim(), git(t, &t.join("remote.git"), &["rev-parse", "main"]).trim());
        assert_eq!(git(t, &r, &["log", "-1", "--format=%p", "HEAD"]).split_whitespace().count(), 2, "the pull merged");
        assert!(t.join("cloned/other.txt").is_file() && t.join("cloned/local.txt").is_file());
        keep_snapshots(t);
    });
}

#[test]
fn tools_for_files() {
    run("git_tools.fbt", |t| {
        repo(t);
        std::fs::create_dir_all(t.join("plain")).unwrap();
    }, |t| {
        let r = t.join("repo");
        assert!(std::fs::read_to_string(r.join(".gitignore")).unwrap().lines().any(|l| l == "*.tmp"));
        let staged = git(t, &r, &["diff", "--cached", "--name-status", "-M", "HEAD"]);
        assert!(staged.contains("R100\tclean.txt\ttidy.txt") && staged.contains("A\tnew.txt") && staged.contains("D\tdocs/readme.md"), "{staged}");
        assert!(r.join("docs/readme.md").is_file(), "kept on disk");
        assert!(!r.join("build.log").exists() && !r.join("scratch.tmp").exists());
        assert_eq!(git(t, &r, &["stash", "list"]).trim(), "");
        assert!(t.join("export.zip").metadata().unwrap().len() > 100);
        assert_eq!(std::fs::read_dir(t.join("patches")).unwrap().count(), 1);
        assert_eq!(git(t, &r, &["config", "--local", "user.name"]).trim(), "Local Name");
        assert!(t.join("plain/.git/HEAD").is_file());
        keep_snapshots(t);
    });
}

/// `repo` with everything committed.
fn committed(t: &Path) {
    repo(t);
    let r = t.join("repo");
    git(t, &r, &["add", "-A"]);
    git(t, &r, &["commit", "-qm", "Third commit"]);
    git(t, &r, &["reset", "-q", "--hard", "HEAD~1"]);
    git(t, &r, &["clean", "-qfdx"]);
}

#[test]
fn branches_merge_and_reset() {
    run("git_branches.fbt", committed, |t| {
        let r = t.join("repo");
        assert_eq!(git(t, &r, &["log", "-1", "--format=%s"]).trim(), "On main");
        assert_eq!(git(t, &r, &["cat-file", "-t", "v2"]).trim(), "tag");
        assert_eq!(git(t, &r, &["log", "-1", "--format=%s", "v2"]).trim(), "Merge feature");
        assert_eq!(git(t, &r, &["log", "-1", "--format=%s", "feature"]).trim(), "On feature");
        keep_snapshots(t);
    });
}

#[test]
fn overlays_and_menus() {
    run_with("git_overlays.fbt", false, |t| {
        repo(t);
        std::fs::create_dir_all(t.join("plain")).unwrap();
    }, keep_snapshots);
}
