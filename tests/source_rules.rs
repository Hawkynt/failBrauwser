//! Rules for the source code that tests of the running program cannot catch.

use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// GTK's `iter_next(&iter)` / `iter_previous(&iter)` change an iterator Rust sees as borrowed
/// unchanged: an optimised build may keep reading the first row and loop forever (0.8.1 froze
/// entering folders so). Rows are stepped through with `util::child_iters` instead.
#[test]
fn no_tree_iterators_changed_in_place() {
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let mut found = Vec::new();
    for f in files {
        for (n, line) in std::fs::read_to_string(&f).unwrap().lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains(".iter_next(") || code.contains(".iter_previous(") {
                found.push(format!("{}:{}", f.display(), n + 1));
            }
        }
    }
    assert!(found.is_empty(), "use util::child_iters instead of iter_next/iter_previous: {found:?}");
}

/// This program is linked windowed, so a console tool it starts is given a console of its own unless
/// the start says otherwise — that is the black window a user sees for every overlay refresh, log
/// read, blame run and commit. `win::console::no_window` says otherwise, and `progress::prepare`
/// applies it for the windows that build their commands through it. A VCS file that starts a client
/// either way is the rule break this catches, because the code still works: only the desktop shows it.
#[test]
fn every_vcs_client_start_hides_its_console_window() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    for sub in ["vcs", "ui/vcs"] {
        rust_files(&root.join(sub), &mut files);
    }
    assert!(!files.is_empty(), "the version-control trees are expected to exist");
    let mut found = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            if !line.split("//").next().unwrap_or("").contains("Command::new(") {
                continue;
            }
            let near = lines[n..(n + 12).min(lines.len())].iter().any(|l| l.contains("no_window(") || l.contains("prepare("));
            if !near {
                found.push(format!("{}:{}", f.display(), n + 1));
            }
        }
    }
    assert!(found.is_empty(), "start each console client with win::console::no_window: {found:?}");
}
