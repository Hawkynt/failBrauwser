//! The git module against real repositories made with the git command.

use failbrauwser::vcs::git::{self, RefLabel, Status};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

fn sh(dir: &Path, cmd: &str) {
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Ann")
        .env("GIT_AUTHOR_EMAIL", "ann@example.org")
        .env("GIT_COMMITTER_NAME", "Ann")
        .env("GIT_COMMITTER_EMAIL", "ann@example.org")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success(), "{cmd}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A repository with two commits, then changes of every kind in the working tree.
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    sh(
        d.path(),
        "git init -q -b main . && mkdir -p src/ui docs && printf 'a\\nb\\n' > src/ui/x.rs && echo m > src/main.rs && echo r > README.md \
         && echo old > old.txt && echo '*.log' > .gitignore && git add -A && git commit -qm first \
         && echo c >> src/main.rs && git commit -qam 'second change' && git tag v1",
    );
    sh(d.path(), "printf 'a\\nB\\nc\\n' > src/ui/x.rs && echo n > new.txt && git add new.txt && git mv old.txt moved.txt && rm README.md && echo u > docs/notes.md && echo l > build.log");
    d
}

#[test]
fn discovers_the_working_tree_from_inside() {
    let d = repo();
    let r = git::discover(&d.path().join("src/ui")).unwrap();
    assert_eq!(r.root, d.path());
    assert_eq!(r.relative(&d.path().join("src/ui/x.rs")).unwrap(), "src/ui/x.rs");
    assert_eq!(r.relative(d.path()).unwrap(), "");
    assert_eq!(r.branch().as_deref(), Some("main"));
    assert!(git::discover(&d.path().join(".git/refs")).is_none());
    let outside = tempfile::tempdir().unwrap();
    assert!(git::discover(outside.path()).is_none());
}

#[test]
fn overlay_status_of_a_folder() {
    let d = repo();
    let r = git::discover(d.path()).unwrap();
    let top = r.folder_status(d.path()).unwrap();
    let s = |n: &str| top.get(&OsString::from(n)).copied();
    assert_eq!(s("src"), Some(Status::Modified));
    assert_eq!(s("new.txt"), Some(Status::Added));
    assert_eq!(s("moved.txt"), Some(Status::Renamed));
    assert_eq!(s("README.md"), Some(Status::Deleted));
    assert_eq!(s("docs"), Some(Status::Unversioned));
    assert_eq!(s("build.log"), Some(Status::Ignored));
    assert_eq!(s(".gitignore"), Some(Status::Normal));
    let ui = r.folder_status(&d.path().join("src")).unwrap();
    assert_eq!(ui.get(&OsString::from("ui")), Some(&Status::Modified));
    assert_eq!(ui.get(&OsString::from("main.rs")), Some(&Status::Normal));
}

#[test]
fn changes_to_commit_with_line_counts() {
    let d = repo();
    let r = git::discover(d.path()).unwrap();
    let all = r.changes(&[], true, false).unwrap();
    let got: Vec<(&str, Status, Option<u64>, Option<u64>)> = all.iter().map(|c| (c.path.as_str(), c.status, c.added, c.removed)).collect();
    assert_eq!(
        got,
        [
            ("README.md", Status::Deleted, Some(0), Some(1)),
            ("docs/notes.md", Status::Unversioned, Some(1), Some(0)),
            ("moved.txt", Status::Renamed, Some(0), Some(0)),
            ("new.txt", Status::Added, Some(1), Some(0)),
            ("src/ui/x.rs", Status::Modified, Some(2), Some(1)),
        ]
    );
    // Only under a path, and without unversioned files.
    let src = r.changes(&["src".into()], false, false).unwrap();
    assert_eq!(src.len(), 1);
    assert!(r.changes(&[], false, false).unwrap().iter().all(|c| c.status != Status::Unversioned));
    assert!(r.changes(&[], true, true).unwrap().iter().any(|c| c.path == "build.log" && c.status == Status::Ignored));
}

#[test]
fn history_refs_and_blame() {
    let d = repo();
    let r = git::discover(d.path()).unwrap();
    let log = r.log(false, &[], 100).unwrap();
    assert_eq!(log.iter().map(|c| c.subject()).collect::<Vec<_>>(), ["second change", "first"]);
    assert_eq!(log[0].author, "Ann");
    let only_main = r.log(false, &["src/main.rs".into()], 100).unwrap();
    assert_eq!(only_main.len(), 2);
    let only_x = r.log(false, &["src/ui/x.rs".into()], 100).unwrap();
    assert_eq!(only_x.len(), 1);
    let changed = r.commit_changes(&log[0].hash).unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!((changed[0].path.as_str(), changed[0].status, changed[0].added), ("src/main.rs", Status::Modified, Some(1)));
    let first = r.commit_changes(&log[1].hash).unwrap();
    assert!(first.iter().all(|c| c.status == Status::Added) && first.len() == 5);
    let refs = r.refs();
    assert_eq!(refs.branches, ["main"]);
    assert_eq!(refs.tags, ["v1"]);
    let labels = &refs.by_commit[&log[0].hash];
    assert!(labels.contains(&RefLabel::Branch("main".into())) && labels.contains(&RefLabel::Tag("v1".into())));
    let blame = r.blame("src/main.rs", None).unwrap();
    assert_eq!(blame.len(), 2);
    assert_eq!(blame[1].summary, "second change");
    assert_eq!(r.last_two_versions("src/main.rs").unwrap().0, log[0].hash);
    assert_eq!(String::from_utf8(r.show_file("HEAD", "src/main.rs")).unwrap(), "m\nc\n");
    assert!(r.show_file("HEAD", "nothing").is_empty());
    assert!(r.reflog().unwrap().iter().any(|e| e.action == "commit" && e.message == "second change"));
}

#[test]
fn summary_of_a_nested_working_tree() {
    let d = repo();
    assert_eq!(git::summary(d.path()).unwrap(), Status::Modified);
    sh(d.path(), "git add -A && git commit -qm all");
    assert_eq!(git::summary(d.path()).unwrap(), Status::Normal);
    // Untracked files leave it clean.
    sh(d.path(), "echo x > untracked.txt");
    assert_eq!(git::summary(d.path()).unwrap(), Status::Normal);
    assert!(!git::discover(d.path()).unwrap().has_stash());
    sh(d.path(), "echo y >> src/main.rs && git stash -q");
    let r = git::discover(d.path()).unwrap();
    assert!(r.has_stash());
    assert_eq!(r.stashes().len(), 1);
}

#[test]
fn a_repository_without_commits() {
    let d = tempfile::tempdir().unwrap();
    sh(d.path(), "git init -q -b main . && printf 'one\\ntwo\\n' > a.txt && git add a.txt");
    let r = git::discover(d.path()).unwrap();
    assert!(!r.has_commits());
    assert!(r.log(false, &[], 10).unwrap().is_empty());
    let c = r.changes(&[], true, false).unwrap();
    assert_eq!((c[0].status, c[0].added), (Status::Added, Some(2)));
    assert_eq!(r.folder_status(d.path()).unwrap()[&OsString::from("a.txt")], Status::Added);
}

#[test]
fn conflicts_are_found() {
    let d = tempfile::tempdir().unwrap();
    sh(
        d.path(),
        "git init -q -b main . && echo base > f && git add f && git commit -qm base && git checkout -qb other && echo theirs > f && git commit -qam theirs \
         && git checkout -q main && echo ours > f && git commit -qam ours && (git merge -q other || true)",
    );
    let r = git::discover(d.path()).unwrap();
    assert!(r.has_conflicts());
    assert_eq!(r.folder_status(d.path()).unwrap()[&OsString::from("f")], Status::Conflicted);
    assert_eq!(git::summary(d.path()).unwrap(), Status::Conflicted);
}
