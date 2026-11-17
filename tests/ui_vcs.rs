//! The Repository menu and TortoiseGit's windows on working copies of the other tools, made by
//! their own clients: overlays, a commit of what is ticked (an unversioned file added on the way)
//! and the log afterwards. Inside a headless compositor on Linux, on the win32 GDK surface on
//! Windows. A tool whose client does not answer on PATH is skipped with the reason printed: on a
//! normal development machine the client is installed, so the scenario runs there.

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

/// The clients' settings for a test: no user configuration, a fixed identity.
fn env(t: &Path) -> Vec<(String, String)> {
    vec![
        ("HGRCPATH".into(), t.join("hgrc").display().to_string()),
        ("HGUSER".into(), "Ann Tester <ann@example.org>".into()),
        // git's configuration is this test's own file, empty: git refuses the null device on Windows
        // (`unable to access 'NUL': Invalid argument`, measured 2026-11-17).
        ("GIT_CONFIG_GLOBAL".into(), t.join("gitconfig").display().to_string()),
        ("LC_ALL".into(), "C".into()),
    ]
}

/// One client command, by name and arguments: the tool is looked up on PATH, which is where its
/// installer puts it. Never a shell string, so the scenario means the same thing on either platform.
fn client(t: &Path, dir: &Path, program: &str, args: &[&str]) {
    let out = Command::new(program).current_dir(dir).args(args).envs(env(t)).output().unwrap();
    assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A working copy at `$T/wc`: same.txt and one.txt committed ("first"), one.txt changed and
/// committed again ("second"), then one.txt edited and new.txt left unversioned.
fn working_copy(t: &Path, tool: &str) {
    std::fs::write(t.join("hgrc"), "[ui]\nusername = Ann Tester <ann@example.org>\n").unwrap();
    let wc = t.join("wc");
    match tool {
        "hg" => client(t, t, "hg", &["init", "wc"]),
        "svn" => {
            client(t, t, "svnadmin", &["create", "repo"]);
            // Forward slashes either way; a Windows path is a drive letter, so it also needs the
            // third slash, which `file://C:/…` would otherwise spend on a host name.
            let path = t.display().to_string().replace('\\', "/");
            let url = if cfg!(windows) { format!("file:///{path}/repo") } else { format!("file://{path}/repo") };
            client(t, t, "svn", &["mkdir", "-q", "-m", "layout", &format!("{url}/trunk")]);
            client(t, t, "svn", &["checkout", "-q", &format!("{url}/trunk"), "wc"]);
        }
        "cvs" => {
            let root = t.join("cvsroot");
            client(t, t, "cvs", &["-d", root.to_str().unwrap(), "init"]);
            std::fs::create_dir_all(t.join("import")).unwrap();
            client(t, &t.join("import"), "cvs", &["-d", root.to_str().unwrap(), "import", "-m", "start", "mod", "vendor", "start"]);
            client(t, t, "cvs", &["-d", root.to_str().unwrap(), "checkout", "-d", "wc", "mod"]);
        }
        _ => unreachable!(),
    }
    std::fs::write(wc.join("same.txt"), "same\n").unwrap();
    std::fs::write(wc.join("one.txt"), "alpha\nbeta\n").unwrap();
    client(t, &wc, tool, &["add", "same.txt", "one.txt"]);
    client(t, &wc, tool, &["commit", "-m", "first"]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(wc.join("one.txt"), "alpha\nbeta changed\n").unwrap();
    client(t, &wc, tool, &["commit", "-m", "second"]);
    if tool == "svn" {
        client(t, &wc, "svn", &["update", "-q"]);
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(wc.join("one.txt"), "alpha\nbeta changed\ngamma\n").unwrap();
    std::fs::write(wc.join("new.txt"), "fresh\n").unwrap();
}

fn run(script: &str, tool: &str, needs: &[&str]) {
    #[cfg(unix)]
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
    std::fs::write(t.join("gitconfig"), "").unwrap();
    working_copy(t, tool);
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
    // The harness answers into a file rather than a pipe, so that a tool still running when the run
    // ends cannot leave this waiting for an end of file that never comes (see tests\ui_git.rs).
    let log = t.join("harness.log");
    let file = std::fs::File::create(&log).unwrap();
    let status = run.envs(env(t)).arg(bin).arg(&script_path).arg(t).stdout(file.try_clone().unwrap()).stderr(file).status().unwrap();
    // Read as bytes and decoded leniently: a client answers in the machine's own codepage, and a
    // strict UTF-8 read would lose the very message the scenario is there to show.
    let log = std::fs::read(&log).unwrap_or_else(|e| panic!("{script}: no harness log: {e}"));
    let log = String::from_utf8_lossy(&log).into_owned();
    assert!(status.success() && log.contains("SELFTEST OK"), "{script} failed:\n{log}");
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
