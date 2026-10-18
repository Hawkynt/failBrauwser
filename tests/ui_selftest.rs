//! Runs the scripted UI tests in tests/ui/*.fbt inside a headless sway session.
//! Skipped (with a notice) when sway, grim or dbus-run-session are not installed.

use std::path::Path;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

fn run(script: &str, setup: impl FnOnce(&Path)) {
    run_checked(script, &[], setup, |_| {});
}

/// Like `run`, with extra environment and a check of the folder afterwards.
fn run_checked(script: &str, env: &[(&str, &str)], setup: impl FnOnce(&Path), check: impl FnOnce(&Path)) {
    if !["sway", "grim", "dbus-run-session"].iter().all(|t| have(t)) {
        eprintln!("headless UI tools missing, skipping {script}");
        return;
    }
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    setup(dir.path());
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new(root.join("tests/ui/harness.sh"))
        .envs(env.iter().copied())
        .arg(env!("CARGO_BIN_EXE_failbrauwser"))
        .arg(root.join("tests/ui").join(script))
        .arg(dir.path())
        .output()
        .unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("SELFTEST OK"), "{script} failed:\n{log}");
    check(dir.path());
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
            let projects = format!("file://{}", t.join("projects").display());
            std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
            std::fs::write(
                t.join("config/failbrauwser/shortcuts"),
                format!("0\tdrives\t\n0\thome\t\n0\tdesktop\t\n0\t{projects}\tProjects\n0\ttrash\t\n1\tmounts\t\n"),
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
        // A link pointing at a big file outside: its target must not be counted.
        std::os::unix::fs::symlink(t.join("big"), root.join("with-link/big-link")).unwrap();
        // A link back up the tree: following it would go round in circles.
        std::os::unix::fs::symlink(&root, root.join("with-link/loop")).unwrap();
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
            w = w.display()
        );
        let ok = std::process::Command::new("python3").arg("-c").arg(script).status().unwrap();
        assert!(ok.success());
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

#[test]
fn trash() {
    run("trash.fbt", |t| {
        let w = t.join("w");
        std::fs::create_dir_all(&w).unwrap();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(w.join(n), n).unwrap();
        }
    });
}

#[test]
fn zoom() {
    run("zoom.fbt", |t| {
        std::fs::create_dir_all(t.join("d/sub")).unwrap();
        std::fs::write(t.join("d/file.txt"), "x").unwrap();
    });
}

fn make_pictures(t: &Path) {
    std::fs::create_dir_all(t.join("pics")).unwrap();
    let script = format!(
        "from PIL import Image\nfor i in range(12):\n    Image.new('RGB', (640, 480), (i * 20, 80, 160)).save('{}/pics/p%02d.png' % i)\nopen('{}/pics/notes.txt', 'w').write('x')\n",
        t.display(),
        t.display()
    );
    assert!(Command::new("python3").arg("-c").arg(script).status().unwrap().success());
}

fn cached_thumbnails(t: &Path) -> usize {
    std::fs::read_dir(t.join("cache/thumbnails/normal")).map(|d| d.count()).unwrap_or(0)
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
    run("summary.fbt", |t| {
        let s = t.join("s");
        std::fs::create_dir_all(s.join("sub1")).unwrap();
        std::fs::create_dir_all(s.join("sub2/deep")).unwrap();
        std::fs::write(s.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(s.join("b.bin"), vec![0u8; 500]).unwrap();
        std::fs::write(s.join("sub2/deep/c.bin"), vec![0u8; 2000]).unwrap();
        // A symlink to something big must not count.
        std::fs::write(t.join("huge"), vec![0u8; 50_000]).unwrap();
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
        let ok = Command::new("python3")
            .arg("-c")
            .arg(format!("import zipfile\nz=zipfile.ZipFile('{}/stuff.zip','w')\nz.writestr('docs/inside-report.txt','x')\nz.close()", w.display()))
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
fn custom_actions() {
    // Any existing program makes the built-in "Open Terminal Here" appear, on any machine.
    run_checked("custom_actions.fbt", &[("TERMINAL", "sh")], |t| {
        let w = t.join("w");
        std::fs::create_dir_all(w.join("sub")).unwrap();
        std::fs::write(w.join("pic.png"), "png").unwrap();
        std::fs::create_dir_all(t.join("config/Thunar")).unwrap();
        std::fs::create_dir_all(t.join("config/failbrauwser")).unwrap();
        std::fs::write(
            t.join("config/Thunar/uca.xml"),
            "<actions><action><name>Thunar Folder Action</name><command>touch %f/marker</command><patterns>*</patterns><directories/></action></actions>",
        )
        .unwrap();
        std::fs::write(
            t.join("config/failbrauwser/actions.xml"),
            "<actions><action><name>Image Action</name><command>touch %f.done</command><patterns>*.png</patterns><image-files/></action></actions>",
        )
        .unwrap();
    }, |_| {});
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
