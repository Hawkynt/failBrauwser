//! The Repository menu and TortoiseGit's windows on working copies of the other tools, made by
//! their own clients: overlays, a commit of what is ticked (an unversioned file added on the way)
//! and the log afterwards. Run like the other UI tests inside a headless compositor; a tool whose
//! client is not installed is skipped with the reason printed.

use std::path::Path;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

/// The clients' settings for a test: no user configuration, a fixed identity.
fn env(t: &Path) -> Vec<(String, String)> {
    vec![
        ("HGRCPATH".into(), t.join("hgrc").display().to_string()),
        ("HGUSER".into(), "Ann Tester <ann@example.org>".into()),
        ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ("LC_ALL".into(), "C".into()),
    ]
}

fn sh(t: &Path, dir: &Path, program: &str, args: &[&str]) {
    let out = Command::new(program).current_dir(dir).args(args).envs(env(t)).output().unwrap();
    assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A working copy at `$T/wc`: same.txt and one.txt committed ("first"), one.txt changed and
/// committed again ("second"), then one.txt edited and new.txt left unversioned.
fn working_copy(t: &Path, tool: &str) {
    std::fs::write(t.join("hgrc"), "[ui]\nusername = Ann Tester <ann@example.org>\n").unwrap();
    let wc = t.join("wc");
    match tool {
        "hg" => sh(t, t, "hg", &["init", "wc"]),
        "svn" => {
            sh(t, t, "svnadmin", &["create", "repo"]);
            let url = format!("file://{}/repo", t.display());
            sh(t, t, "svn", &["mkdir", "-q", "-m", "layout", &format!("{url}/trunk")]);
            sh(t, t, "svn", &["checkout", "-q", &format!("{url}/trunk"), "wc"]);
        }
        "cvs" => {
            let root = t.join("cvsroot");
            sh(t, t, "cvs", &["-d", root.to_str().unwrap(), "init"]);
            std::fs::create_dir_all(t.join("import")).unwrap();
            sh(t, &t.join("import"), "cvs", &["-d", root.to_str().unwrap(), "import", "-m", "start", "mod", "vendor", "start"]);
            sh(t, t, "cvs", &["-d", root.to_str().unwrap(), "checkout", "-d", "wc", "mod"]);
        }
        _ => unreachable!(),
    }
    std::fs::write(wc.join("same.txt"), "same\n").unwrap();
    std::fs::write(wc.join("one.txt"), "alpha\nbeta\n").unwrap();
    sh(t, &wc, tool, &["add", "same.txt", "one.txt"]);
    sh(t, &wc, tool, &["commit", "-m", "first"]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(wc.join("one.txt"), "alpha\nbeta changed\n").unwrap();
    sh(t, &wc, tool, &["commit", "-m", "second"]);
    if tool == "svn" {
        sh(t, &wc, "svn", &["update", "-q"]);
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(wc.join("one.txt"), "alpha\nbeta changed\ngamma\n").unwrap();
    std::fs::write(wc.join("new.txt"), "fresh\n").unwrap();
}

fn run(script: &str, tool: &str, needs: &[&str]) {
    if !(have("sway") || have("tilewin")) || !["grim", "dbus-run-session"].iter().all(|t| have(t)) {
        eprintln!("headless UI tools missing, skipping {script}");
        return;
    }
    if let Some(missing) = needs.iter().find(|p| !have(p)) {
        eprintln!("{missing} is not installed, skipping {script}");
        return;
    }
    let dir = tempfile::Builder::new().prefix("fb-vcs-ui").tempdir().unwrap();
    let t = dir.path();
    std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
    std::fs::write(t.join("config/failbrauwser/settings.ini"), "[General]\nvcs_integration=true\n").unwrap();
    working_copy(t, tool);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(root.join("tests/ui/harness.sh")).envs(env(t)).arg(env!("CARGO_BIN_EXE_failbrauwser")).arg(root.join("tests/ui").join(script)).arg(t).output().unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("SELFTEST OK"), "{script} failed:\n{log}");
}

#[test]
fn a_mercurial_working_copy() {
    run("vcs_hg.fbt", "hg", &["hg"]);
}

#[test]
fn a_subversion_working_copy() {
    run("vcs_svn.fbt", "svn", &["svn", "svnadmin"]);
}

#[test]
fn a_cvs_working_copy() {
    run("vcs_cvs.fbt", "cvs", &["cvs"]);
}
