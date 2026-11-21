//! Runs the scripted UI tests in tests/ui/*.fbt inside a headless sway session
//! (Linux) or on the win32 GDK desktop (Windows). Skipped (with a notice) when the
//! platform's tools are missing; scenarios without a twin on one platform skip there
//! with the reason, listed in docs/WINDOWS.md.

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

/// The plain-Python twin's name differs by platform.
fn python() -> &'static str {
    if cfg!(windows) { "python" } else { "python3" }
}

/// A path inside a Python string literal: doubled backslashes, which a Unix path never has.
fn pysafe(p: &Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}

/// The archive helper a scenario reads inside an archive through: what the machine names, or this
/// checkout's own built helper. cargo never builds the C# side, so a built one can be absent.
fn archive_helper() -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) { "fb-archive.exe" } else { "fb-archive" };
    if let Some(p) = std::env::var_os("FB_ARCHIVE_HELPER") && Path::new(&p).is_file() {
        return Some(std::path::PathBuf::from(p));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    [format!("helper/FbArchive/bin/Release/net10.0/{name}"), format!("helper/FbArchive/bin/Debug/net10.0/{name}"), format!("target/helper/{name}")].iter().map(|p| root.join(p)).find(|p| p.is_file())
}

fn run(script: &str, setup: impl FnOnce(&Path)) {
    run_checked(script, &[], setup, |_| {});
}

/// Like `run`, with extra environment and a check of the folder afterwards.
fn run_checked(script: &str, env: &[(&str, &str)], setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    drive(script, dir.path(), env, setup, check);
}

/// The same, with the fixture outside this checkout. A Repository scenario needs that: a marker a
/// tool leaves reaches every folder below it, and `CARGO_TARGET_TMPDIR` sits inside this very
/// repository's own Git working copy — so under it there is no folder that no tool owns, and the
/// commands such a folder offers could never be seen.
fn run_outside_the_working_copy(script: &str, env: &[(&str, &str)], setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    let dir = tempfile::tempdir().unwrap();
    drive(script, dir.path(), env, setup, check);
}

fn drive(script: &str, dir: &Path, env: &[(&str, &str)], setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    #[cfg(unix)]
    if !(have("sway") || have("tilewin")) || !["grim", "dbus-run-session"].iter().all(|t| have(t)) {
        eprintln!("headless UI tools missing, skipping {script}");
        return;
    }
    setup(dir);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // The win32 twin of that harness: same arguments, same contract, same "SELFTEST OK".
    #[cfg(unix)]
    let mut run = Command::new(root.join("tests/ui/harness.sh"));
    #[cfg(windows)]
    let mut run = {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(root.join("tests/ui/harness.ps1"));
        c
    };
    // A step that reads inside an archive asks the C# helper, and `archive::locate_helper` answers
    // the bare name `fb-archive` when it finds no file: the step then finds nothing of its own,
    // quietly. `search.fbt` read 0 rows for its archive hit here three times on 2026-11-17 until the
    // built helper was named, which is why the scenario names it rather than trusting the machine.
    let helper = archive_helper();
    let mut env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    match &helper {
        Some(p) => env.push(("FB_ARCHIVE_HELPER".into(), p.display().to_string())),
        None => eprintln!("{script}: no built archive helper — a step reading inside an archive finds nothing (dotnet build helper/FbArchive -c Release)"),
    }
    // The harness answers into a file rather than a pipe, so that a tool still running when the run
    // ends cannot leave this waiting for an end of file that never comes (see tests\ui_git.rs).
    let log = dir.join("harness.log");
    let file = std::fs::File::create(&log).unwrap();
    let status = run
        .envs(env)
        .arg(env!("CARGO_BIN_EXE_failbrauwser"))
        .arg(root.join("tests/ui").join(script))
        .arg(dir)
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .status()
        .unwrap();
    // The log is read as the bytes it is: a Windows tool answers its errors in the machine's own
    // codepage, and `custom_actions.fbt` reached this with such a message in it (measured
    // 2026-11-17, where a strict UTF-8 read said only `stream did not contain valid UTF-8`).
    let log = std::fs::read(&log).unwrap_or_else(|e| panic!("{script}: no harness log: {e}"));
    let log = String::from_utf8_lossy(&log).into_owned();
    assert!(status.success() && log.contains("SELFTEST OK"), "{script} failed:\n{log}");
    check(dir);
}

#[test]
fn file_operations() {
    run("file_ops.fbt", |t| {
        std::fs::create_dir_all(t.join("files/sub")).unwrap();
        std::fs::create_dir_all(t.join("dest")).unwrap();
        std::fs::write(t.join("files/a.txt"), "a").unwrap();
        std::fs::write(t.join("files/b.txt"), "b").unwrap();
    });
}

#[test]
fn rename_onto_a_taken_name() {
    run_checked(
        "rename_conflict.fbt",
        &[],
        |t| {
            std::fs::create_dir_all(t.join("files/one")).unwrap();
            std::fs::create_dir_all(t.join("files/two")).unwrap();
            std::fs::write(t.join("files/a.txt"), "from a").unwrap();
            std::fs::write(t.join("files/b.txt"), "from b").unwrap();
            std::fs::write(t.join("files/one/from-one.txt"), "1").unwrap();
            std::fs::write(t.join("files/two/from-two.txt"), "2").unwrap();
            std::fs::write(t.join("files/x.txt"), "from x").unwrap();
            std::fs::write(t.join("files/y.txt"), "from y").unwrap();
        },
        |t| {
            // b.txt holds what was a.txt and y.txt what was x.txt; nothing else is left of them.
            assert_eq!(std::fs::read_to_string(t.join("files/b.txt")).unwrap(), "from a");
            assert_eq!(std::fs::read_to_string(t.join("files/y.txt")).unwrap(), "from x");
            let mut names: Vec<_> = std::fs::read_dir(t.join("files")).unwrap().map(|e| e.unwrap().file_name()).collect();
            names.sort();
            assert_eq!(names, ["b.txt", "two", "y.txt"]);
        },
    );
}

#[test]
fn progress_window_with_speed_chart() {
    run_checked("progress.fbt", &[], |_| {}, |t| {
        // The window was drawn, chart and all.
        let png = std::fs::read(t.join("progress.png")).unwrap();
        assert!(png.len() > 1000 && png.starts_with(b"\x89PNG"));
    });
}

#[test]
fn share_without_change_notices() {
    run("share_refresh.fbt", |t| {
        std::fs::create_dir_all(t.join("share")).unwrap();
        std::fs::create_dir_all(t.join("src")).unwrap();
        std::fs::write(t.join("share/existing.txt"), "x").unwrap();
        std::fs::write(t.join("src/a.txt"), "a").unwrap();
    });
}

#[test]
fn checksums_in_properties() {
    run("checksums.fbt", |t| std::fs::write(t.join("abc.txt"), "abc").unwrap());
}

#[test]
fn batch_rename_with_undo() {
    run("batch_rename.fbt", |t| {
        std::fs::create_dir_all(t.join("pics")).unwrap();
        for n in ["IMG_0001.JPG", "IMG_0002.JPG", "IMG_0003.JPG", "urlaub 000.jpg"] {
            std::fs::write(t.join("pics").join(n), n).unwrap();
        }
    });
}

#[test]
fn undo_operations() {
    run_checked(
        "undo.fbt",
        &[],
        |t| {
            std::fs::create_dir_all(t.join("src")).unwrap();
            std::fs::create_dir_all(t.join("dest")).unwrap();
            for n in ["a.txt", "b.txt", "c.txt", "d.txt"] {
                std::fs::write(t.join("src").join(n), n).unwrap();
            }
        },
        |t| {
            // Everything as before, and the copy that was taken back is in the trash.
            for n in ["a.txt", "b.txt", "c.txt", "d.txt"] {
                assert_eq!(std::fs::read_to_string(t.join("src").join(n)).unwrap(), n);
            }
            assert_eq!(std::fs::read_dir(t.join("dest")).unwrap().count(), 0);
            // The copy that was taken back went to the trash — the XDG one, or the shell's.
            #[cfg(unix)]
            assert!(t.join("data/Trash/files/a.txt").exists());
            #[cfg(windows)]
            assert!(failbrauwser::win::recyclebin::list().unwrap().iter().any(|b| b.original == t.join("dest").join("a.txt")), "the taken-back copy is not in the Recycle Bin");
        },
    );
}

#[test]
fn disk_image_as_drive() {
    if cfg!(windows) {
        eprintln!("image loop-mounting belongs to udisks on Linux; the Windows twin would be a virtual-CD story of its own, skipping");
        return;
    }
    run("mount_image.fbt", |t| {
        std::fs::create_dir_all(t.join("inside")).unwrap();
        std::fs::write(t.join("inside/report.txt"), "r").unwrap();
        std::fs::write(t.join("backup.ntfs"), "not really").unwrap();
        std::fs::write(t.join("photos.zip"), "PK").unwrap();
    });
}

#[test]
fn compare_folders() {
    run_checked(
        "compare.fbt",
        &[],
        |t| {
            let put = |rel: &str, text: &str, age: u64| {
                let p = t.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, text).unwrap();
                let f = std::fs::File::options().write(true).open(&p).unwrap();
                f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(age)).unwrap();
            };
            put("left/same.txt", "same", 5000);
            put("right/same.txt", "same", 5000);
            put("left/only-left.txt", "l", 50);
            put("right/only-right.txt", "r", 50);
            put("left/sub/report.txt", "new report", 50);
            put("right/sub/report.txt", "old", 5000);
            put("left/notes.txt", "old notes", 5000);
            put("right/notes.txt", "new notes", 50);
            put("left/photos/beach.jpg", "jpg", 50);
        },
        |t| {
            // Copied: missing and newer from the left; the right's newer notes untouched.
            assert_eq!(std::fs::read_to_string(t.join("right/sub/report.txt")).unwrap(), "new report");
            assert_eq!(std::fs::read_to_string(t.join("right/only-left.txt")).unwrap(), "l");
            assert_eq!(std::fs::read_to_string(t.join("right/notes.txt")).unwrap(), "new notes");
            assert!(t.join("right/only-right.txt").exists());
        },
    );
}

#[test]
fn servers_on_the_network_page() {
    run_checked("servers.fbt", &[], |_| {}, |t| {
        let saved = std::fs::read_to_string(t.join("config/failbrauwser/servers")).unwrap();
        assert_eq!(saved, "Pi\tsftp://me@pi.local:2222/srv/My%20Files\n");
    });
}

#[test]
fn location_bar_completes_paths() {
    run_checked(
        "location_complete.fbt",
        &[],
        |t| {
            for d in ["w/Projects", "w/projects-old", "w/Pictures/2024", "w/.hidden"] {
                std::fs::create_dir_all(t.join(d)).unwrap();
            }
            std::fs::write(t.join("w/notes.txt"), "").unwrap();
            std::fs::write(t.join("w/music.zip"), "").unwrap();
        },
        |_| {},
    );
}

/// A Host of the SSH configuration, picked in Add Server and opened, against an SSH server of the
/// test's own: `sshd` as this user on a free port of 127.0.0.1, with keys made for the test. GVfs
/// runs `ssh` from the PATH, so an `ssh` in front of the real one carries the port, the key and
/// a known-hosts file of the test's own; the user's `~/.ssh` is never read or written.
#[test]
#[cfg(unix)]
fn ssh_host_from_the_configuration_mounts_like_a_share() {
    let sshd = Path::new("/usr/bin/sshd");
    if !sshd.exists() || !have("ssh-keygen") || !Path::new("/usr/lib/gvfsd-sftp").exists() && !Path::new("/usr/libexec/gvfsd-sftp").exists() {
        eprintln!("no sshd, ssh-keygen or GVfs SFTP backend, skipping");
        return;
    }
    let ssh = tempfile::tempdir().unwrap();
    let d = ssh.path();
    std::fs::create_dir_all(d.join("remote")).unwrap();
    std::fs::create_dir_all(d.join("bin")).unwrap();
    std::fs::write(d.join("remote/hello-from-ssh.txt"), "hi\n").unwrap();
    for key in ["host", "client"] {
        let ok = Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "", "-f"]).arg(d.join(key)).status().unwrap().success();
        assert!(ok, "ssh-keygen {key}");
    }
    std::fs::copy(d.join("client.pub"), d.join("authorized_keys")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let p = |name: &str| d.join(name).display().to_string();
    std::fs::write(
        d.join("sshd_config"),
        format!(
            "ListenAddress 127.0.0.1\nPort {port}\nHostKey {}\nAuthorizedKeysFile {}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\nPidFile none\nSubsystem sftp internal-sftp -d {}\n",
            p("host"),
            p("authorized_keys"),
            p("remote")
        ),
    )
    .unwrap();
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _server = Kill(Command::new(sshd).arg("-D").arg("-f").arg(d.join("sshd_config")).stderr(std::process::Stdio::null()).spawn().unwrap());
    std::fs::write(d.join("config"), "Host *.example\n  User nobody\n\nHost testbox\n  HostName 127.0.0.1\n").unwrap();
    let wrapper = d.join("bin/ssh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexec /usr/bin/ssh -F {} -p {port} -i {} -o IdentitiesOnly=yes -o UserKnownHostsFile={} -o StrictHostKeyChecking=accept-new \"$@\"\n",
            p("config"),
            p("client"),
            p("known_hosts")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    // The server is up once it takes connections.
    let up = (0..50).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    });
    assert!(up, "sshd did not start on port {port}");
    let path = format!("{}:{}", p("bin"), std::env::var("PATH").unwrap_or_default());
    let config = p("config");
    run_checked("ssh_host.fbt", &[("PATH", &path), ("FAILBRAUWSER_SSH_CONFIG", &config)], |_| {}, |t| {
        let saved = std::fs::read_to_string(t.join("config/failbrauwser/servers")).unwrap();
        assert_eq!(saved, "testbox\tsftp://testbox/\n");
    });
}

/// A picture of soft colour shapes; `variant` moves them.
fn picture(w: i32, h: i32, variant: i32) -> gtk::gdk_pixbuf::Pixbuf {
    let pb = gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, w, h).unwrap();
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f64 / w as f64, y as f64 / h as f64);
            let r = ((fx * 6.0 + variant as f64).sin() * 0.5 + 0.5) * 255.0;
            let g = ((fy * 5.0 - variant as f64 * 1.7).cos() * 0.5 + 0.5) * 255.0;
            let b = (((fx + fy) * 4.0 + variant as f64 * 2.3).sin() * 0.5 + 0.5) * 255.0;
            pb.put_pixel(x as u32, y as u32, r as u8, g as u8, b as u8, 255);
        }
    }
    pb
}

#[test]
fn find_duplicates() {
    run("duplicates.fbt", |t| {
        std::fs::create_dir_all(t.join("photos/small")).unwrap();
        std::fs::create_dir_all(t.join("docs")).unwrap();
        picture(1600, 1200, 0).savev(t.join("photos/beach.png"), "png", &[]).unwrap();
        picture(1600, 1200, 0).scale_simple(400, 300, gtk::gdk_pixbuf::InterpType::Hyper).unwrap().savev(t.join("photos/small/beach-small.jpg"), "jpeg", &[("quality", "75")]).unwrap();
        picture(1600, 1200, 3).savev(t.join("photos/other.png"), "png", &[]).unwrap();
        std::fs::write(t.join("docs/a.txt"), "the same text").unwrap();
        std::fs::write(t.join("docs/copy of a.txt"), "the same text").unwrap();
        std::fs::write(t.join("docs/b.txt"), "something else").unwrap();
        // The original is the older one.
        let f = std::fs::File::options().write(true).open(t.join("docs/a.txt")).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
    });
}

#[test]
fn file_chooser_for_other_applications() {
    if cfg!(windows) {
        eprintln!("the XDG portal service has no Windows twin — serving other apps is a Linux feature, skipping");
        return;
    }
    run("portal.fbt", |t| {
        std::fs::create_dir_all(t.join("pics/sub")).unwrap();
        std::fs::create_dir_all(t.join("docs")).unwrap();
        for n in ["pics/a.png", "pics/b.png", "pics/notes.txt", "docs/old.txt"] {
            std::fs::write(t.join(n), "x").unwrap();
        }
    });
}

#[test]
fn file_chooser_through_xdg_desktop_portal() {
    if !Path::new("/usr/lib/xdg-desktop-portal").exists() && !Path::new("/usr/libexec/xdg-desktop-portal").exists() {
        eprintln!("xdg-desktop-portal not installed, skipping");
        return;
    }
    run("portal_frontend.fbt", |t| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // As install.sh registers it, below the test's own config and data folders.
        std::fs::create_dir_all(t.join("data/xdg-desktop-portal/portals")).unwrap();
        std::fs::copy(root.join("data/failbrauwser.portal"), t.join("data/xdg-desktop-portal/portals/failbrauwser.portal")).unwrap();
        std::fs::create_dir_all(t.join("config/xdg-desktop-portal")).unwrap();
        std::fs::write(t.join("config/xdg-desktop-portal/portals.conf"), "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=failbrauwser\n").unwrap();
        std::fs::create_dir_all(t.join("home")).unwrap();
        std::fs::write(t.join("home/pick.txt"), "x").unwrap();
    });
}

/// Other applications' "show in folder" (Firefox's downloads, Chromium, Thunderbird), asked
/// over D-Bus as they ask it.
#[test]
#[cfg(unix)]
fn other_applications_show_files_in_their_folder() {
    if !have("gdbus") {
        eprintln!("gdbus missing, skipping");
        return;
    }
    run("show_in_folder.fbt", |t| {
        std::fs::create_dir_all(t.join("dl")).unwrap();
        for f in ["a.pdf", "b.zip", "c.txt"] {
            std::fs::write(t.join("dl").join(f), "x").unwrap();
        }
    });
}

#[test]
fn gtk3_applications_get_failbrauwsers_dialogs() {
    if cfg!(windows) {
        eprintln!("the module is built with cc and pkg-config and served through xdg-desktop-portal — no Windows twin, skipping");
        return;
    }
    if !Path::new("/usr/lib/xdg-desktop-portal").exists() && !Path::new("/usr/libexec/xdg-desktop-portal").exists() {
        eprintln!("xdg-desktop-portal not installed, skipping");
        return;
    }
    let gtk3_python = Command::new("python3").args(["-c", "import gi; gi.require_version('Gtk', '3.0')"]).status().is_ok_and(|s| s.success());
    if !gtk3_python || !have("cc") || !have("pkg-config") {
        eprintln!("python3 with GTK 3, cc or pkg-config missing, skipping");
        return;
    }
    run("gtk3_dialogs.fbt", |t| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Built as install.sh builds it.
        let flags = |what: &str| -> Vec<String> {
            let out = Command::new("pkg-config").args([what, "gtk+-3.0"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::to_string).collect()
        };
        let built = Command::new("cc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(t.join("module.so"))
            .args(flags("--cflags"))
            .arg(root.join("gtkmodule/failbrauwser-chooser.c"))
            .args(flags("--libs"))
            .status()
            .unwrap();
        assert!(built.success());
        std::fs::create_dir_all(t.join("data/xdg-desktop-portal/portals")).unwrap();
        std::fs::copy(root.join("data/failbrauwser.portal"), t.join("data/xdg-desktop-portal/portals/failbrauwser.portal")).unwrap();
        std::fs::create_dir_all(t.join("config/xdg-desktop-portal")).unwrap();
        std::fs::write(t.join("config/xdg-desktop-portal/portals.conf"), "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=failbrauwser\n").unwrap();
        std::fs::create_dir_all(t.join("home/sub")).unwrap();
        for n in ["a.txt", "b.txt"] {
            std::fs::write(t.join("home").join(n), n).unwrap();
        }
    });
}

#[test]
fn update_from_release_or_source() {
    // Outside the checkout: the file:// addresses are made from the path, which must hold no
    // space ("Working Copies") for curl to take them.
    run_outside_the_working_copy(
        "update.fbt",
        &[],
        |t| {
            // The stand-in package is named by the very formula the updater matches on,
            // and laid out like package.ps1: a folder holding the payload to swap in.
            let pkg = failbrauwser::update::package_name("v99.0");
            let root = t.join("pkgroot/failbrauwser-99.0");
            // The base keeps its trailing slash: the asset URL is this base plus the
            // package name, and a join without one names the sibling of the folder.
            let url = |p: &str| if cfg!(windows) { format!("file:///{}/", p.replace('\\', "/")) } else { format!("file://{p}/") };
            if cfg!(windows) {
                std::fs::create_dir_all(root.join("bin")).unwrap();
                std::fs::create_dir_all(root.join("share")).unwrap();
                std::fs::write(root.join("bin/failbrauwser.exe"), "NEW-EXE").unwrap();
                std::fs::write(root.join("share/payload.txt"), "NEW-DATA").unwrap();
                // Packed like package.ps1 does: the folder itself, so the zip holds one
                // failbrauwser-* root with bin/ and share/ inside.
                let pack = format!("Compress-Archive -Path '{}' -DestinationPath '{}'", root.display(), t.join(&pkg).display());
                let zip = std::process::Command::new("powershell").args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &pack]).output().unwrap();
                assert!(zip.status.success(), "{}{}", String::from_utf8_lossy(&zip.stdout), String::from_utf8_lossy(&zip.stderr));
            } else {
                let fake = |dir: &Path, record: &str| {
                    std::fs::create_dir_all(dir).unwrap();
                    let script = format!("#!/bin/sh\necho \"$@\" > {}/{record}\necho installed\n", t.display());
                    std::fs::write(dir.join("install.sh"), script).unwrap();
                    std::process::Command::new("chmod").arg("+x").arg(dir.join("install.sh")).status().unwrap();
                };
                fake(&root, "installed-args");
                assert!(std::process::Command::new("tar").args(["czf"]).arg(t.join(&pkg)).arg("-C").arg(t.join("pkgroot")).arg("failbrauwser-99.0").status().unwrap().success());
                fake(&t.join("srcroot/failBrauwser-main"), "built-args");
                // The source's install.sh builds four crates, as install.sh says it, and waits
                // at the third until the scenario has seen the progress bar.
                let build = format!(
                    "#!/bin/sh\necho \"$@\" > {t}/built-args\nprintf '\\033[1m==>\\033[0m %s\\n' 'Building failBrauwser: 4 crates'\nfor c in alpha beta gamma; do echo \"   Compiling $c v1.0.0\"; done\nwhile [ ! -f {t}/go ]; do sleep 0.1; done\necho '   Compiling failbrauwser v99.0.0'\necho installed\n",
                    t = t.display()
                );
                std::fs::write(t.join("srcroot/failBrauwser-main/install.sh"), build).unwrap();
                assert!(std::process::Command::new("tar").args(["czf"]).arg(t.join("source.tar.gz")).arg("-C").arg(t.join("srcroot")).arg("failBrauwser-main").status().unwrap().success());
            }
            std::fs::write(
                t.join("release.json"),
                format!(r#"{{"tag_name": "v99.0", "name": "failBrauwser 99.0", "published_at": "2026-10-30T19:00:00Z", "assets": [{{"name": "{pkg}", "browser_download_url": "{}{pkg}"}}]}}"#, url(&t.display().to_string())),
            )
            .unwrap();
            std::fs::create_dir_all(t.join("prefix/bin")).unwrap();
            std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
            std::fs::write(t.join("config/failbrauwser/install-options"), "--default\n").unwrap();
        },
        |t| {
            // The downloads are gone.
            let left: Vec<_> = std::fs::read_dir(t.join("cache")).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("failbrauwser-update.")).collect();
            assert!(left.is_empty(), "{left:?}");
        },
    );
}

#[test]
fn sidebar_tree() {
    run("sidebar.fbt", |t| {
        std::fs::create_dir_all(t.join("a/b/c/d")).unwrap();
        std::fs::create_dir_all(t.join("a/other/inside")).unwrap();
    });
}

#[test]
fn side_panel_entries() {
    run_checked(
        "shortcuts.fbt",
        &[],
        |t| {
            for d in ["work", "projects", "music"] {
                std::fs::create_dir_all(t.join(d)).unwrap();
            }
            // A known start instead of the GTK bookmarks the harness copies from the desktop.
            // A Windows path needs the extra slash: `file://A:/…` would read “A:” as a host.
            let projects = if cfg!(windows) {
                format!("file:///{}", t.join("projects").display().to_string().replace('\\', "/"))
            } else {
                format!("file://{}", t.join("projects").display())
            };
            std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
            std::fs::write(
                t.join("config/failbrauwser/shortcuts"),
                format!("0\tdrives\t\n0\thome\t\n0\tdesktop\t\n0\t{projects}\tProjects\n0\ttrash\t\n1\tmounts\t\n1\tnetwork\t\n"),
            )
            .unwrap();
        },
        |t| {
            // Kept for the next start, in the order arranged.
            let saved = std::fs::read_to_string(t.join("config/failbrauwser/shortcuts")).unwrap();
            let keys: Vec<&str> = saved.lines().map(|l| l.split('\t').nth(1).unwrap_or("")).collect();
            assert_eq!(keys[..3], ["drives", "home", "desktop"]);
            assert!(keys[3].ends_with("/music"), "{saved}");
            assert!(saved.contains("\tCode\n"), "{saved}");
        },
    );
}

/// An unprivileged directory link: NTFS junctions report as symlinks and need no
/// elevation, unlike real symlinks on machines without Developer Mode (error 1314).
#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    // cmd treats a stray `/` as a switch start: hand mklink backslash paths only.
    let plain = |p: &Path| p.display().to_string().replace('/', "\\");
    let out = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(plain(link)).arg(plain(target)).output().unwrap();
    assert!(out.status.success(), "mklink /J {}: {}", link.display(), String::from_utf8_lossy(&out.stderr));
}

#[test]
fn size_column_adds_up_folders() {
    run("dirsize.fbt", |t| {
        let root = t.join("root");
        std::fs::create_dir_all(root.join("photos/2024/summer")).unwrap();
        std::fs::write(root.join("photos/a.jpg"), vec![0u8; 1500]).unwrap();
        std::fs::write(root.join("photos/2024/summer/b.jpg"), vec![0u8; 2000]).unwrap();
        std::fs::create_dir_all(root.join("with-link")).unwrap();
        std::fs::write(root.join("with-link/small"), vec![0u8; 10]).unwrap();
        std::fs::write(t.join("big"), vec![0u8; 100_000]).unwrap();
        // A link pointing at something big outside: its target must not be counted.
        // Unix follows a file symlink; Windows points a junction at a big outside folder.
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.join("big"), root.join("with-link/big-link")).unwrap();
        #[cfg(windows)]
        {
            std::fs::create_dir_all(t.join("outside")).unwrap();
            std::fs::write(t.join("outside/big"), vec![0u8; 100_000]).unwrap();
            junction(&root.join("with-link/big-link"), &t.join("outside"));
        }
        // A link back up the tree: following it would go round in circles.
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("with-link/loop")).unwrap();
        #[cfg(windows)]
        junction(&root.join("with-link/loop"), &root);
        std::fs::write(root.join("note.txt"), "hello").unwrap();
    });
}

#[test]
fn default_application_from_properties() {
    // The desktop's own applications; any two that open text will do.
    let apps = ["org.xfce.mousepad.desktop", "vim.desktop"];
    if !apps.iter().all(|a| Path::new("/usr/share/applications").join(a).exists()) {
        eprintln!("mousepad and vim not installed, skipping");
        return;
    }
    run_checked(
        "default_app.fbt",
        &[],
        |t| {
            std::fs::create_dir_all(t.join("d")).unwrap();
            std::fs::write(t.join("d/a.txt"), "hi").unwrap();
        },
        |t| {
            let list = std::fs::read_to_string(t.join("config/mimeapps.list")).unwrap();
            assert!(list.contains("text/plain=vim.desktop"), "{list}");
        },
    );
}

#[test]
fn drives_page() {
    run("drives.fbt", |t| {
        std::fs::create_dir(t.join("d")).unwrap();
        std::fs::write(t.join("d/file"), "x").unwrap();
    });
}

#[test]
fn phones_with_the_drives() {
    run("phones.fbt", |t| {
        for d in ["phone/Internal shared storage/DCIM", "phone/SD card/Music", "locked"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
    });
}

#[test]
fn unmounted_drives_in_the_side_panel() {
    run("unmounted_drives.fbt", |t| {
        std::fs::create_dir_all(t.join("photos")).unwrap();
        std::fs::write(t.join("photos/holiday.jpg"), "jpg").unwrap();
        std::fs::create_dir_all(t.join("vault")).unwrap();
        std::fs::write(t.join("vault/secret.txt"), "s").unwrap();
    });
}

#[test]
fn renaming_and_formatting_a_volume() {
    // The stand-in volume whose format and name are driven here is never mounted, so its folder is never read.
    // The mounted image the two device-icon verbs write into is a folder of this run's own, which is why the
    // folder and the image file both exist before the scenario starts.
    run("drive_format.fbt", |t| {
        std::fs::create_dir_all(t.join("inside")).unwrap();
        std::fs::write(t.join("backup.ntfs"), "fake image").unwrap();
    });
}

#[test]
fn a_device_shows_what_its_own_files_name() {
    // The four files of one device icon are staged by this program's own writer, into the folder the stand-in
    // stick mounts on, before the scenario starts: what the listing reads is the bytes a create actually
    // leaves, pinned against a file system whose own label holds eleven characters. A second stick holds the
    // same pair with nonsense written over its two picture files, which is what the scenario shows the listing
    // refuse to draw.
    run_checked(
        "device_shown.fbt",
        &[],
        |t| {
            let stick = t.join("stick");
            std::fs::create_dir_all(&stick).unwrap();
            std::fs::create_dir_all(t.join("plain")).unwrap();
            let plan = failbrauwser::fsformat::name_plan(failbrauwser::fsformat::FileSystem::ExFat, "Samsung 64 GB Family Pictures").expect("a name exFAT is given");
            failbrauwser::fsformat::icon::create(&stick, failbrauwser::fsformat::Shape::UsbStick, "Samsung Flash Drive", 64_160_400_896, &plan).expect("the four files of one device icon");
            // A second stand-in stick, its pair of ours standing and pinning a name, with nonsense written over
            // the two picture files: the listing refuses the picture alone.
            let broken = t.join("broken");
            std::fs::create_dir_all(&broken).unwrap();
            let rotten = failbrauwser::fsformat::name_plan(failbrauwser::fsformat::FileSystem::ExFat, "Broken Stick With A Rotten Picture").expect("a name exFAT is given");
            failbrauwser::fsformat::icon::create(&broken, failbrauwser::fsformat::Shape::UsbStick, "Samsung Flash Drive", 64_160_400_896, &rotten).expect("the four files of the second device icon");
            for name in ["folder.ico", "DevIcon.fil"] {
                std::fs::write(broken.join(name), "no icon: what a half-written act or an archive leaves behind").expect("nonsense where a picture of ours stood");
            }
        },
        |t| {
            // The scenario's own removal took both sticks away again: no file of the four, no staged name of ours,
            // and the mark of ours off the folders themselves (README: *Device icons*) — the folder whose pictures
            // were nonsense included, since removal obeys the names a marked file carries and not their bytes.
            for folder in ["stick", "broken"] {
                let root = t.join(folder);
                for name in ["autorun.inf", "desktop.ini", "folder.ico", "DevIcon.fil", "autorun.inf.fbtmp", "desktop.ini.fbtmp", "folder.ico.fbtmp", "DevIcon.fil.fbtmp"] {
                    assert!(!root.join(name).exists(), "{name} is left in the folder whose device icon was taken away");
                }
                #[cfg(windows)]
                assert_eq!(failbrauwser::osprim::attrs_of(&root) & failbrauwser::osprim::SYSTEM, 0, "the mark of ours stands on a folder its icon was taken off");
            }
        },
    );
}

#[test]
fn light_and_dark_follow_the_desktop() {
    run("appearance.fbt", |_| {});
}

#[test]
fn show_only_and_subfolders() {
    run_checked(
        "show_only.fbt",
        &[],
        |t| {
            let w = t.join("w");
            for d in ["docs/deep", "pics"] {
                std::fs::create_dir_all(w.join(d)).unwrap();
            }
            for f in ["a.txt", "b.png", "docs/notes.txt", "docs/deep/plan.txt", "pics/photo.jpg"] {
                std::fs::write(w.join(f), "x").unwrap();
            }
        },
        |t| {
            // Files only is kept for the next start; the subfolder view is not.
            let ini = std::fs::read_to_string(t.join("config/failbrauwser/settings.ini")).unwrap_or_default();
            assert!(ini.contains("show_only=files"), "{ini}");
        },
    );
}

#[test]
fn big_listings_do_not_freeze_the_window() {
    run("big_listings.fbt", |t| {
        std::fs::create_dir_all(t.join("flat")).unwrap();
        for i in 0..15000 {
            std::fs::write(t.join(format!("flat/f{i:05}.txt")), "").unwrap();
        }
        for d in 0..150 {
            let dir = t.join(format!("tree/d{d:03}"));
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..100 {
                std::fs::write(dir.join(format!("f{i:03}.txt")), "").unwrap();
            }
        }
    });
}

#[test]
fn network_page() {
    run_checked(
        "network.fbt",
        &[],
        |t| {
            for (d, f) in [("music", "song.mp3"), ("photos", "beach.jpg"), ("secret", "diary.txt")] {
                std::fs::create_dir_all(t.join(d)).unwrap();
                std::fs::write(t.join(d).join(f), "x").unwrap();
            }
            std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
            std::fs::write(t.join("config/failbrauwser/shortcuts"), "0\tdrives\t\n0\thome\t\n0\tdesktop\t\n0\ttrash\t\n1\tmounts\t\n").unwrap();
        },
        |t| {
            // Pinned places are kept by their address.
            let saved = std::fs::read_to_string(t.join("config/failbrauwser/shortcuts")).unwrap();
            assert!(saved.contains("\tsmb://nas/Photos\t") && saved.contains("\tnetwork:///nas\t"), "{saved}");
        },
    );
}

#[test]
fn drives_in_the_side_panel() {
    run("side_drives.fbt", |t| {
        for d in ["phone/Internal shared storage/DCIM", "phone/SD card/Music", "locked", "elsewhere"] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
    });
}

#[test]
fn links_on_the_desktop() {
    if cfg!(windows) {
        eprintln!("win32 GLib resolves the desktop through the roaming profile: test links would land on the user's real desktop. .lnk creation itself is covered by the shelllink tests, skipping the scenario");
        return;
    }
    run_checked(
        "desktop_links.fbt",
        &[],
        |t| {
            for d in ["files/Music", "projects", "phone/SD card", "phone/Internal shared storage"] {
                std::fs::create_dir_all(t.join(d)).unwrap();
            }
            std::fs::write(t.join("files/notes.txt"), "n").unwrap();
            // The desktop of the test, not the user's.
            std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
            std::fs::write(t.join("config/user-dirs.dirs"), format!("XDG_DESKTOP_DIR=\"{}/Desktop\"\n", t.display())).unwrap();
            let projects = format!("file://{}", t.join("projects").display());
            std::fs::write(t.join("config/failbrauwser/shortcuts"), format!("0\tdrives\t\n0\thome\t\n0\t{projects}\tProjects\n0\ttrash\t\n")).unwrap();
        },
        |t| {
            let read = |name: &str| std::fs::read_to_string(t.join("Desktop").join(name)).unwrap();
            let music = read("Music.desktop");
            assert!(music.starts_with("[Desktop Entry]\n"), "{music}");
            // A URL escapes what a path may hold (a space in the checkout's path, say), as the program's own writer does.
            let music_url = format!("URL={}", failbrauwser::desktoplink::file_url(&t.join("files/Music")).expect("a path is a file URL"));
            for line in ["Type=Link", "Name=Music", music_url.as_str()] {
                assert!(music.lines().any(|l| l == line), "{line} missing in {music}");
            }
            assert!(read("notes.txt.desktop").contains("\nName=notes.txt\n"));
            assert!(read("Projects.desktop").contains(&format!("\nURL={}\n", failbrauwser::desktoplink::file_url(&t.join("projects")).expect("a path is a file URL"))));
            assert!(read("Pixel 7 – SD card.desktop").contains("/phone/SD%20card\n"));
            let drives = read("Drives.desktop");
            assert!(drives.contains("\nType=Application\n") && drives.lines().any(|l| l.starts_with("Exec=") && l.ends_with(" drives:///")), "{drives}");
            assert!(read("Pixel 7 – Internal shared storage.desktop").contains("/phone/Internal%20shared%20storage\n"));
        },
    );
}

#[test]
fn archives_as_folders() {
    if !failbrauwser::archive::client::locate_helper().is_file() {
        eprintln!("fb-archive helper not built, skipping");
        return;
    }
    run("archives.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(w.join("out")).unwrap();
        std::fs::write(w.join("new.txt"), "new file").unwrap();
        let script = format!(
            r#"
import io, tarfile, zipfile
buf = io.BytesIO()
with tarfile.open(fileobj=buf, mode="w:gz") as t:
    data = b"deep content"
    info = tarfile.TarInfo("deep/file.txt"); info.size = len(data)
    t.addfile(info, io.BytesIO(data))
with zipfile.ZipFile("{w}/test.zip", "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("a.txt", "alpha")
    z.writestr("sub/b.txt", "beta")
    z.writestr("inner.tar.gz", buf.getvalue())
"#,
            w = pysafe(&w)
        );
        let ok = std::process::Command::new(python()).arg("-c").arg(script).status().unwrap();
        assert!(ok.success());
    });
}

/// Open With on files inside an archive: a real application is started with real files, and its
/// edits are saved back.
#[test]
#[cfg(unix)]
fn open_with_inside_an_archive() {
    if !failbrauwser::archive::client::locate_helper().is_file() {
        eprintln!("fb-archive helper not built, skipping");
        return;
    }
    run("archive_open_with.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(&w).unwrap();
        let script = format!(
            r#"
import zipfile
with zipfile.ZipFile("{w}/test.zip", "w") as z:
    z.writestr("a.txt", "alpha")
    z.writestr("b.txt", "beta")
    z.writestr("12:30.txt", "noon")
    z.writestr("sub/c.txt", "gamma")
"#,
            w = pysafe(&w)
        );
        assert!(Command::new(python()).arg("-c").arg(script).status().unwrap().success());
        // The application: writes what it was given, whether that is an extracted copy, then
        // edits it as an editor would.
        let recorder = t.join("recorder.sh");
        std::fs::write(
            &recorder,
            format!(
                "#!/bin/sh
for f; do
  case \"$f\" in */failbrauwser/open/fb-*/*) where=staged ;; *) where=elsewhere ;; esac
  [ -f \"$f\" ] || where=missing
  printf '%s:%s:%s\\n' \"$(basename \"$f\")\" \"$(cat \"$f\")\" \"$where\" >> '{got}'
  printf ' edited' >> \"$f\"
done
",
                got = t.join("got").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&recorder, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(t.join("data/applications")).unwrap();
        std::fs::write(
            t.join("data/applications/fbtest-recorder.desktop"),
            // Quoted as the Desktop Entry specification asks of an argument holding a space, which a checkout
            // path may: unquoted, GIO looks for the program at the first word and leaves the application out.
            format!("[Desktop Entry]\nType=Application\nName=Recorder\nExec=\"{}\" %F\nMimeType=text/plain;\n", recorder.display()),
        )
        .unwrap();
        // GIO looks a folder's types up in its index (what update-desktop-database writes).
        std::fs::write(t.join("data/applications/mimeinfo.cache"), "[MIME Cache]\ntext/plain=fbtest-recorder.desktop;\n").unwrap();
    });
}

#[test]
fn iso_images() {
    let have_xorriso = Command::new("sh").arg("-c").arg("command -v xorriso").output().is_ok_and(|o| o.status.success());
    if !have_xorriso || !failbrauwser::archive::client::locate_helper().is_file() {
        eprintln!("xorriso or archive helper missing, skipping");
        return;
    }
    run("iso.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(w.join("src/docs")).unwrap();
        std::fs::create_dir_all(w.join("out")).unwrap();
        std::fs::write(w.join("src/readme.txt"), "hello").unwrap();
        std::fs::write(w.join("src/docs/a.txt"), "inside the disc").unwrap();
        std::fs::write(w.join("new.txt"), "added").unwrap();
        let ok = Command::new("xorriso")
            .args(["-as", "mkisofs", "-R", "-J", "-V", "DISC", "-o"])
            .arg(w.join("disc.iso"))
            .arg(w.join("src"))
            .output()
            .unwrap();
        assert!(ok.status.success());
        let hy = Command::new("xorriso")
            .args(["-as", "mkisofs", "-R", "-J", "--protective-msdos-label", "-o"])
            .arg(w.join("hybrid.iso"))
            .arg(w.join("src"))
            .output()
            .unwrap();
        assert!(hy.status.success());
        std::fs::remove_dir_all(w.join("src")).unwrap();
    });
}

#[test]
fn path_bar() {
    run("pathbar.fbt", |t| {
        std::fs::create_dir_all(t.join("a/b/c/a-rather-long-folder-name/another-long-folder-name/and-the-last-one")).unwrap();
        std::fs::create_dir_all(t.join("x")).unwrap();
    });
}

/// The mouse's back and forward keys. Their Win32 half — the messages never reach GDK, so the
/// program reads them from its own window procedure — is what the scenario drives; the same keys
/// reach a pane through GDK as buttons 8 and 9 on X11, where there is no message to post.
#[test]
fn mouse_back_and_forward_keys() {
    run("mouse_keys.fbt", |t| {
        std::fs::create_dir_all(t.join("a/b/c")).unwrap();
    });
}

/// The Repository menu of every tool (README, Version control). The markers are the plain ones
/// each tool leaves — `.git` with its HEAD, `.hg`, `.svn`, a `CVS/Repository` file, a
/// `vssver2.scc` file — because the menu reads nothing else; what the clients answer is measured
/// in `tests/vcs_backends.rs`.
#[test]
fn repository_menu_per_working_copy() {
    run_outside_the_working_copy(
        "repo_menu.fbt",
        &[],
        |t| {
            for git in ["gitcopy", "wrapper/inner"] {
                std::fs::create_dir_all(t.join(git).join(".git/refs/heads")).unwrap();
                std::fs::create_dir_all(t.join(git).join(".git/objects")).unwrap();
                std::fs::write(t.join(git).join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            }
            std::fs::create_dir_all(t.join("gitcopy/deep")).unwrap();
            std::fs::create_dir_all(t.join("plain/sub")).unwrap();
            std::fs::create_dir_all(t.join("cvs/CVS")).unwrap();
            std::fs::write(t.join("cvs/CVS/Repository"), "module\n").unwrap();
            std::fs::write(t.join("cvs/CVS/Entries"), "D\n").unwrap();
            std::fs::create_dir_all(t.join("hgcopy/.hg")).unwrap();
            std::fs::create_dir_all(t.join("svncopy/.svn")).unwrap();
            std::fs::create_dir_all(t.join("vsscopy")).unwrap();
            std::fs::write(t.join("vsscopy/vssver2.scc"), "").unwrap();
            std::fs::write(t.join("vsscopy/a.txt"), "a\n").unwrap();
        },
        |_| {},
    );
}


/// The shell's Recycle Bin belongs to the whole machine, so an older run's `a.txt`
/// would be picked by name instead of this run's, and our leftovers would pile up.
/// Anything whose original place is under the test tmp root is ours: clear it out
/// first (also recovering from a run that failed halfway) and again afterwards.
#[cfg(windows)]
fn bin_sweep() {
    for item in failbrauwser::win::recyclebin::list_unfiltered().unwrap_or_default() {
        if item.original.starts_with(env!("CARGO_TARGET_TMPDIR")) {
            let _ = failbrauwser::win::recyclebin::delete(&item.path);
        }
    }
}

#[test]
fn encrypted_drive_with_lvm_inside() {
    run("lvm_drive.fbt", |t| {
        std::fs::create_dir_all(t.join("linux")).unwrap();
        std::fs::write(t.join("linux/notes.txt"), "x").unwrap();
    });
}

#[test]
fn windows_share_names_in_the_location_bar() {
    run("unc_address.fbt", |_| {});
}

#[test]
fn trash() {
    #[cfg(windows)]
    bin_sweep();
    run("trash.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(&w).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(w.join(n), n).unwrap();
        }
    });
    #[cfg(windows)]
    bin_sweep();
}

#[test]
fn zoom() {
    run("zoom.fbt", |t| {
        std::fs::create_dir_all(t.join("d/sub")).unwrap();
        std::fs::write(t.join("d/file.txt"), "x").unwrap();
    });
}

#[cfg(unix)]
fn make_pictures(t: &Path) {
    std::fs::create_dir_all(t.join("pics")).unwrap();
    let script = format!(
        "from PIL import Image\nfor i in range(12):\n    Image.new('RGB', (640, 480), (i * 20, 80, 160)).save('{}/pics/p%02d.png' % i)\nopen('{}/pics/notes.txt', 'w').write('x')\n",
        t.display(),
        t.display()
    );
    assert!(Command::new("python3").arg("-c").arg(script).status().unwrap().success());
}

/// PIL is not a given on Windows boxes; the same twelve soft-color pictures come from GDK-Pixbuf.
#[cfg(windows)]
fn make_pictures(t: &Path) {
    std::fs::create_dir_all(t.join("pics")).unwrap();
    for i in 0..12u32 {
        picture(640, 480, i as i32).savev(t.join(format!("pics/p{i:02}.png")), "png", &[]).unwrap();
    }
    std::fs::write(t.join("pics/notes.txt"), "x").unwrap();
}

/// How many of the 12 pictures have a thumbnail in the cache (by the freedesktop.org name: the
/// MD5 of the file's URI). Other files may get one too where a thumbnailer for them is installed.
fn cached_thumbnails(t: &Path) -> usize {
    use md5::Digest;
    (0..12)
        .filter(|i| {
            let uri = gtk::gio::prelude::FileExt::uri(&gtk::gio::File::for_path(t.join(format!("pics/p{i:02}.png"))));
            let name: String = md5::Md5::digest(uri.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            t.join("cache/thumbnails/normal").join(format!("{name}.png")).is_file()
        })
        .count()
}

#[test]
fn thumbnails_through_the_thumbnailer_service() {
    let have_tumbler = Path::new("/usr/share/dbus-1/services/org.xfce.Tumbler.Thumbnailer1.service").exists();
    if !have_tumbler {
        eprintln!("tumbler not installed, skipping");
        return;
    }
    run_checked("thumbnails.fbt", &[], make_pictures, |t| assert_eq!(cached_thumbnails(t), 12));
}

#[test]
fn thumbnails_without_a_service() {
    run_checked("thumbnails.fbt", &[("FB_NO_TUMBLER", "1")], make_pictures, |t| assert_eq!(cached_thumbnails(t), 12));
}

#[test]
fn status_bar_summary() {
    if cfg!(windows) {
        eprintln!("the summary fixture needs a file symlink, which this machine only allows elevated (Developer Mode off) — junction links are exercised by dirsize instead, skipping");
        return;
    }
    run("summary.fbt", |t| {
        let s = t.join("s");
        std::fs::create_dir_all(s.join("sub1")).unwrap();
        std::fs::create_dir_all(s.join("sub2/deep")).unwrap();
        std::fs::write(s.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(s.join("b.bin"), vec![0u8; 500]).unwrap();
        std::fs::write(s.join("sub2/deep/c.bin"), vec![0u8; 2000]).unwrap();
        // A symlink to something big must not count. (Unix link fixture; see above.)
        std::fs::write(t.join("huge"), vec![0u8; 50_000]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.join("huge"), s.join("sub1/link")).unwrap();
    });
}

#[test]
fn search() {
    run("search.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(w.join("a/b")).unwrap();
        std::fs::write(w.join("report-2024.txt"), "r").unwrap();
        std::fs::write(w.join("a/b/REPORT old.txt"), "r").unwrap();
        std::fs::write(w.join("a/report.md"), "r").unwrap();
        std::fs::write(w.join("notes.txt"), "n").unwrap();
        let ok = Command::new(python())
            .arg("-c")
            .arg(format!("import zipfile\nz=zipfile.ZipFile('{}/stuff.zip','w')\nz.writestr('docs/inside-report.txt','x')\nz.close()", pysafe(&w)))
            .status()
            .unwrap();
        assert!(ok.success());
    });
}

#[test]
fn click_into_empty_space() {
    run("click_empty.fbt", |t| {
        std::fs::create_dir_all(t.join("d")).unwrap();
        std::fs::write(t.join("d/a.txt"), "a").unwrap();
        std::fs::write(t.join("d/b.txt"), "b").unwrap();
    });
}

#[test]
fn several_selected_items_stay_selected_for_a_drag() {
    run("drag_selection.fbt", |t| {
        std::fs::create_dir_all(t.join("d")).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(t.join("d").join(n), n).unwrap();
        }
    });
}

#[test]
fn right_click_menus_have_icons() {
    run("menu_icons.fbt", |t| {
        std::fs::create_dir_all(t.join("d/sub")).unwrap();
        std::fs::write(t.join("d/a.txt"), "a").unwrap();
    });
}

#[test]
#[cfg(unix)] // the fixture makes Unix symbolic links
fn links_their_overlays_and_the_right_drag_menu() {
    run("links.fbt", |t| {
        let d = t.join("d");
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::create_dir_all(t.join("dest/sub2")).unwrap();
        std::fs::create_dir_all(t.join("dest2")).unwrap();
        std::fs::write(d.join("plain.txt"), "plain").unwrap();
        std::fs::write(d.join("hard1.txt"), "h").unwrap();
        std::fs::hard_link(d.join("hard1.txt"), d.join("hard2.txt")).unwrap();
        std::os::unix::fs::symlink("plain.txt", d.join("sym")).unwrap();
        std::os::unix::fs::symlink("nowhere", d.join("dead")).unwrap();
        std::fs::write(d.join("sub/inner.txt"), "i").unwrap();
        std::os::unix::fs::symlink("inner.txt", d.join("sub/to-inner")).unwrap();
        std::os::unix::fs::symlink("/", d.join("drive")).unwrap();
        std::fs::create_dir_all(d.join("vault")).unwrap();
        std::fs::create_dir_all(t.join("dest3")).unwrap();
        std::fs::write(d.join("vault/sealed.txt"), "only root reads this").unwrap();
        std::fs::set_permissions(d.join("vault/sealed.txt"), std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    });
}

#[test]
fn dragging_from_a_name_and_several_items() {
    run("drag_start.fbt", |t| {
        std::fs::create_dir_all(t.join("d")).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(t.join("d").join(n), n).unwrap();
        }
    });
}

#[test]
fn folder_menu_from_an_open_menu() {
    run("folder_menu.fbt", |t| {
        std::fs::create_dir_all(t.join("d")).unwrap();
        for n in ["a.txt", "b.txt"] {
            std::fs::write(t.join("d").join(n), n).unwrap();
        }
    });
}

#[test]
fn custom_actions() {
    // Any existing program makes the built-in "Open Terminal Here" appear, on any machine;
    // on Windows the built-in console always exists, so no TERMINAL hint is needed.
    #[cfg(windows)]
    let env: &[(&str, &str)] = &[];
    #[cfg(unix)]
    let env: &[(&str, &str)] = &[("TERMINAL", "sh")];
    run_checked(
        "custom_actions.fbt",
        env,
        |t| {
            let w = t.join("w");
            std::fs::create_dir_all(w.join("sub")).unwrap();
            std::fs::write(w.join("pic.png"), "png").unwrap();
            std::fs::create_dir_all(t.join("config/Thunar")).unwrap();
            std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
            // The actions drop marker files: touch on Unix, cmd redirection on Windows.
            #[cfg(unix)]
            let (folder_cmd, image_cmd) = ("touch %f/marker", "touch %f.done");
            #[cfg(windows)]
            let (folder_cmd, image_cmd) = ("echo m > %f/marker", "echo done > %f.done");
            std::fs::write(
                t.join("config/Thunar/uca.xml"),
                format!("<actions><action><name>Thunar Folder Action</name><command>{folder_cmd}</command><patterns>*</patterns><directories/></action></actions>"),
            )
            .unwrap();
            std::fs::write(
                t.join("config/failbrauwser/actions.xml"),
                format!("<actions><action><name>Image Action</name><command>{image_cmd}</command><patterns>*.png</patterns><image-files/></action></actions>"),
            )
            .unwrap();
        },
        |_| {},
    );
}

#[test]
fn queued_paused_and_verified_copies() {
    run_checked("queue.fbt", &[], |t| {
        let s = t.join("src");
        std::fs::create_dir_all(s.join("out")).unwrap();
        for (n, size) in [("a.bin", 8 << 20), ("b.bin", 3 << 20), ("c.bin", 5 << 20)] {
            std::fs::write(s.join(n), vec![7u8; size]).unwrap();
        }
    }, |t| {
        for n in ["a.bin", "b.bin", "c.bin"] {
            assert_eq!(std::fs::read(t.join("src/out").join(n)).unwrap(), std::fs::read(t.join("src").join(n)).unwrap());
        }
    });
}

#[test]
fn encrypted_archives() {
    if !failbrauwser::archive::client::locate_helper().is_file() {
        return;
    }
    run_checked("passwords.fbt", &[], |t| {
        std::fs::create_dir_all(t.join("w")).unwrap();
        std::fs::write(t.join("w/doc.txt"), "top secret").unwrap();
        std::fs::write(t.join("w/plan.txt"), "the plan").unwrap();
    }, |t| {
        assert_eq!(std::fs::read_to_string(t.join("w/locked/plan.txt")).unwrap(), "the plan");
    });
}

/// M14 *Open as* in the menu and the view it produces: the four container kinds are offered for a
/// file whose name cannot say what it is, a chosen kind lists what its own reader found, and the
/// *Archive* entry reaches a program's resources instead of running it. What each choice reads out
/// byte for byte is pinned in `archive_helper.rs` and `archive_vfs.rs`, where reading does not
/// depend on a clipboard job this host answers late.
#[test]
fn open_as_chosen_kind() {
    let helper = failbrauwser::archive::client::locate_helper();
    if !helper.is_file() {
        eprintln!("fb-archive helper not built, skipping");
        return;
    }
    let volume = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/veracrypt/test.sha256.hc");
    run("open_as.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(&w).unwrap();
        let h = failbrauwser::archive::Helper::new(helper.clone());
        // A dm-crypt (plain) container: written by the chosen kind, carrying no marker to detect.
        let payload = w.join("payload");
        std::fs::write(&payload, vec![0x5Au8; 4096]).unwrap();
        h.create(&w.join("plain.img"), "DmCryptPlain", &[(payload, "volume.img".into())], Some("s3cret")).unwrap();
        // A real VeraCrypt container under a name that says nothing about it.
        std::fs::copy(&volume, w.join("disk.bin")).unwrap();
        // The same container under a second name: this session has no passphrase for it yet, which
        // is what the cancelled-prompt step needs.
        std::fs::copy(&volume, w.join("locked.bin")).unwrap();
        // A program: a name whose default action runs it, whose insides the Archive entry shows.
        std::fs::copy(&helper, w.join("setup.exe")).unwrap();
    });
}

#[test]
fn compress_settings() {
    let helper = failbrauwser::archive::client::locate_helper();
    if !helper.is_file() {
        eprintln!("fb-archive helper not built, skipping");
        return;
    }
    run_checked(
        "compress_options.fbt",
        &[],
        |t| {
            std::fs::create_dir_all(t.join("w/pack")).unwrap();
            std::fs::write(t.join("w/pack/a.txt"), "alpha ".repeat(2000)).unwrap();
            // Random bytes: stored as they are, so the size of the parts is known.
            let mut x = 0x2545F491u32;
            let big: Vec<u8> = (0..10_000).map(|_| { x ^= x << 13; x ^= x >> 17; x ^= x << 5; x as u8 }).collect();
            std::fs::write(t.join("w/big.bin"), big).unwrap();
        },
        |t| {
            let h = failbrauwser::archive::Helper::new(helper);
            let w = t.join("w");
            assert_eq!(&std::fs::read(w.join("old.rar")).unwrap()[..7], b"Rar!\x1a\x07\x00");
            let zip = std::fs::read(w.join("packed.zip")).unwrap();
            assert!(zip.windows(4).enumerate().any(|(i, x)| x == b"PK\x03\x04" && zip[i + 8] == 14), "no LZMA entry");
            let parts: Vec<u64> = (1..=3).map(|n| std::fs::metadata(w.join(format!("big.7z.{n:03}"))).unwrap().len()).collect();
            assert_eq!(parts[..2], [4096, 4096]);
            let names: Vec<String> = h.list(&w.join("big.7z.001"), None).unwrap().entries.into_iter().map(|e| e.name).collect();
            assert_eq!(names, ["big.bin"]);
            let fat = h.list(&w.join("floppy.img"), None).unwrap();
            assert!(fat.entries.iter().any(|e| e.name.ends_with("a.txt")), "{fat:?}");
            let img = std::fs::read(w.join("floppy.img")).unwrap();
            assert!(img.windows(7).any(|x| x == b"HOLIDAY"), "volume label missing");
        },
    );
}

#[test]
fn more_formats_open_as_folders() {
    let helper = failbrauwser::archive::client::locate_helper();
    if !helper.is_file() {
        eprintln!("fb-archive helper not built, skipping");
        return;
    }
    run("formats.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(t.join("a.txt"), "alpha").unwrap();
        let h = failbrauwser::archive::Helper::new(helper);
        for name in ["old.lzh", "disk.img", "floppy.adf", "root.sqfs"] {
            h.add(&w.join(name), &[(t.join("a.txt"), "a.txt".into())], None).unwrap();
        }
        std::fs::write(t.join("notes.txt"), "notes").unwrap();
        // A ZIP under a generic name, and an LHA archive without any suffix.
        h.create(&t.join("backup.zip"), "Zip", &[(t.join("a.txt"), "a.txt".into())], None).unwrap();
        std::fs::rename(t.join("backup.zip"), w.join("backup.bin")).unwrap();
        h.create(&w.join("mystery"), "Lzh", &[(t.join("a.txt"), "a.txt".into())], None).unwrap();
        std::fs::write(w.join("junk.bin"), (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect::<Vec<u8>>()).unwrap();
        std::fs::create_dir_all(w.join("pack/sub")).unwrap();
        std::fs::write(w.join("pack/sub/p.txt"), "packed").unwrap();
        h.add(&w.join("notes.txt.br"), &[(t.join("notes.txt"), "notes.txt".into())], None).unwrap();
        h.shutdown();
    });
}
