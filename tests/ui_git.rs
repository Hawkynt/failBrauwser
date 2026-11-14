//! The Git integration in the real window (TortoiseGit's overlays, menus and dialogs), run
//! like the other UI tests inside a headless sway session. Git is configured for each test
//! alone (GIT_CONFIG_GLOBAL), never from the user's own configuration.

use std::path::Path;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

/// Runs a shell command in `dir` with the test's git configuration.
fn sh(t: &Path, dir: &Path, cmd: &str) -> String {
    let out = Command::new("sh").arg("-c").arg(cmd).current_dir(dir).envs(git_env(t)).output().unwrap();
    assert!(out.status.success(), "{cmd}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git_env(t: &Path) -> Vec<(String, String)> {
    vec![("GIT_CONFIG_GLOBAL".into(), t.join("gitconfig").display().to_string()), ("GIT_CONFIG_NOSYSTEM".into(), "1".into())]
}

fn run(script: &str, setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    run_with(script, true, setup, check);
}

/// `enabled`: the Git integration is switched on (Edit → Version Control Integration) from the start.
fn run_with(script: &str, enabled: bool, setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    if !(have("sway") || have("tilewin")) || !["grim", "dbus-run-session", "git"].iter().all(|t| have(t)) {
        eprintln!("headless UI tools or git missing, skipping {script}");
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
    let out = Command::new(root.join("tests/ui/harness.sh"))
        .envs(git_env(t))
        .arg(env!("CARGO_BIN_EXE_failbrauwser"))
        .arg(root.join("tests/ui").join(script))
        .arg(t)
        .output()
        .unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("SELFTEST OK"), "{script} failed:\n{log}");
    check(t);
}

/// A working tree with two commits and changes of each kind.
fn repo(t: &Path) {
    let r = t.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    sh(
        t,
        &r,
        "git init -q && mkdir -p src docs && echo a > clean.txt && printf 'one\\ntwo\\nthree\\n' > changed.txt && echo s > src/lib.rs && echo d > docs/readme.md \
         && echo '*.log' > .gitignore && git add -A && git commit -qm 'First commit' && echo four >> changed.txt && git commit -qam 'Second commit' && git tag v1",
    );
    sh(t, &r, "echo 'one\nTWO\nthree\nfour' > changed.txt && echo n > added.txt && git add added.txt && echo u > new.txt && echo l > build.log && echo x >> src/lib.rs");
}

#[test]
fn overlays_and_menus() {
    run_with("vcs_overlays.fbt", false, |t| {
        repo(t);
        std::fs::create_dir_all(t.join("plain")).unwrap();
    }, keep_snapshots);
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
        assert_eq!(sh(t, &r, "git log -1 --format=%s").trim(), "Change two files");
        assert_eq!(sh(t, &r, "git show --name-only --format= HEAD").trim(), "changed.txt\nnew.txt");
        assert_eq!(sh(t, &r, "git status --porcelain").trim(), "M src/lib.rs\n?? added.txt");
        keep_snapshots(t);
    });
}

#[test]
fn log_diff_and_blame() {
    run("git_log.fbt", repo, |t| {
        let r = t.join("repo");
        assert_eq!(sh(t, &r, "git rev-parse v0").trim(), sh(t, &r, "git rev-parse HEAD~1").trim());
        keep_snapshots(t);
    });
}

/// A bare remote with two commits; `repo` cloned from it with a commit of its own, and a
/// commit pushed to the remote from elsewhere meanwhile.
fn remote(t: &Path) {
    sh(
        t,
        t,
        "git init -q --bare remote.git && git clone -q remote.git repo 2>/dev/null && cd repo && echo a > clean.txt && git add -A && git commit -qm 'First commit' \
         && git push -q -u origin main && cd .. && git clone -q remote.git other && cd other && echo o > other.txt && git add other.txt \
         && git commit -qm 'From elsewhere' && git push -q && cd ../repo && echo l > local.txt && git add local.txt && git commit -qm 'Local commit'",
    );
}

#[test]
fn sync_pull_push_fetch_and_clone() {
    run("git_remote.fbt", remote, |t| {
        let r = t.join("repo");
        assert_eq!(sh(t, &r, "git rev-parse HEAD").trim(), sh(t, &t.join("remote.git"), "git rev-parse main").trim());
        assert_eq!(sh(t, &r, "git log -1 --format=%p HEAD").split_whitespace().count(), 2, "the pull merged");
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
        let staged = sh(t, &r, "git diff --cached --name-status -M HEAD");
        assert!(staged.contains("R100\tclean.txt\ttidy.txt") && staged.contains("A\tnew.txt") && staged.contains("D\tdocs/readme.md"), "{staged}");
        assert!(r.join("docs/readme.md").is_file(), "kept on disk");
        assert!(!r.join("build.log").exists() && !r.join("scratch.tmp").exists());
        assert_eq!(sh(t, &r, "git stash list").trim(), "");
        assert!(t.join("export.zip").metadata().unwrap().len() > 100);
        assert_eq!(std::fs::read_dir(t.join("patches")).unwrap().count(), 1);
        assert_eq!(sh(t, &r, "git config --local user.name").trim(), "Local Name");
        assert!(t.join("plain/.git/HEAD").is_file());
        keep_snapshots(t);
    });
}

/// `repo` with everything committed.
fn committed(t: &Path) {
    repo(t);
    sh(t, &t.join("repo"), "git add -A && git commit -qm 'Third commit' && git reset -q --hard HEAD~1 && git clean -qfdx");
}

#[test]
fn branches_merge_and_reset() {
    run("git_branches.fbt", committed, |t| {
        let r = t.join("repo");
        assert_eq!(sh(t, &r, "git log -1 --format=%s").trim(), "On main");
        assert_eq!(sh(t, &r, "git cat-file -t v2").trim(), "tag");
        assert_eq!(sh(t, &r, "git log -1 --format=%s v2").trim(), "Merge feature");
        assert_eq!(sh(t, &r, "git log -1 --format=%s feature").trim(), "On feature");
        keep_snapshots(t);
    });
}
