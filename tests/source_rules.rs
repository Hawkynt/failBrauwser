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
/// read, blame run and commit. `win::console::no_window` says otherwise, `progress::prepare` applies
/// it for the windows that build their commands through it, and a file built by the launcher crate
/// repeats the flag itself (portable.rs). Any start in `src` without one of those nearby is the rule
/// break this catches, because the code still works: only the desktop shows it.
///
/// A start the compiler only builds for Unix is exempt — the window cannot appear there; the Linux
/// answer to the same complaint is recorded in docs\TOOL-STARTS.md.
#[test]
fn every_start_of_another_program_hides_its_console_window() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(!files.is_empty(), "the source tree is expected to exist");
    let mut found = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            if !line.split("//").next().unwrap_or("").contains("Command::new(") {
                continue;
            }
            let unix_only = lines[n.saturating_sub(8)..n].iter().any(|l| l.contains("#[cfg(unix)]") || l.contains("#[cfg(not(windows))]"));
            let guarded = lines[n..(n + 14).min(lines.len())].iter().any(|l| l.contains("no_window(") || l.contains("prepare(") || l.contains("creation_flags("));
            if !(unix_only || guarded) {
                found.push(format!("{}:{}", f.display(), n + 1));
            }
        }
    }
    assert!(found.is_empty(), "start each outside program with win::console::no_window: {found:?}");
}

/// The client of a read ends with the instance that started it (docs\TOOL-STARTS.md). One way an
/// instance ends is `main` returning; the other is the process ending itself, which is what a
/// self-test run does — and an end of its own skips `main`'s tail, where the clients are ended.
/// Measured 2026-11-17 with only the return path covered: a failing Git scenario left
/// `git ls-files -z -- .` running with its own process gone, still there a quarter of an hour later.
/// Everything below `src\ui` therefore ends the instance through `vcs::run::end_and_exit`, which ends
/// the clients first — the code still works without it, and only a process list shows the difference.
#[test]
fn no_end_of_the_process_in_the_ui_skips_the_reading_clients() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("ui");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(!files.is_empty(), "the ui tree is expected to exist");
    let mut found = Vec::new();
    for f in &files {
        for (n, line) in std::fs::read_to_string(f).unwrap().lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("process::exit(") || code.contains("process::abort(") {
                found.push(format!("{}:{}", f.display(), n + 1));
            }
        }
    }
    assert!(found.is_empty(), "end the instance through vcs::run::end_and_exit, which ends the clients of reads still in flight: {found:?}");
}
