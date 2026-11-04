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
            assert!(t.join("data/Trash/files/a.txt").exists());
        },
    );
}

#[test]
fn disk_image_as_drive() {
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

#[test]
fn update_from_release_or_source() {
    run_checked(
        "update.fbt",
        &[],
        |t| {
            let fake = |dir: &Path, record: &str| {
                std::fs::create_dir_all(dir).unwrap();
                let script = format!("#!/bin/sh\necho \"$@\" > {}/{record}\necho installed\n", t.display());
                std::fs::write(dir.join("install.sh"), script).unwrap();
                std::process::Command::new("chmod").arg("+x").arg(dir.join("install.sh")).status().unwrap();
            };
            let pkg = format!("failbrauwser-99.0-{}-linux.tar.gz", std::env::consts::ARCH);
            fake(&t.join("pkgroot/failbrauwser-99.0"), "installed-args");
            assert!(std::process::Command::new("tar").args(["czf"]).arg(t.join(&pkg)).arg("-C").arg(t.join("pkgroot")).arg("failbrauwser-99.0").status().unwrap().success());
            fake(&t.join("srcroot/failBrauwser-main"), "built-args");
            assert!(std::process::Command::new("tar").args(["czf"]).arg(t.join("source.tar.gz")).arg("-C").arg(t.join("srcroot")).arg("failBrauwser-main").status().unwrap().success());
            std::fs::write(
                t.join("release.json"),
                format!(r#"{{"tag_name": "v99.0", "name": "failBrauwser 99.0", "published_at": "2026-10-30T19:00:00Z", "assets": [{{"name": "{pkg}", "browser_download_url": "file://{}/{pkg}"}}]}}"#, t.display()),
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
            let projects = format!("file://{}", t.join("projects").display());
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
        // (Unix link fixtures; the Windows fixtures of these scenarios come with the harness milestone.)
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.join("big"), root.join("with-link/big-link")).unwrap();
        // A link back up the tree: following it would go round in circles.
        #[cfg(unix)]
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
fn unmounted_drives_in_the_side_panel() {
    run("unmounted_drives.fbt", |t| {
        std::fs::create_dir_all(t.join("photos")).unwrap();
        std::fs::write(t.join("photos/holiday.jpg"), "jpg").unwrap();
        std::fs::create_dir_all(t.join("vault")).unwrap();
        std::fs::write(t.join("vault/secret.txt"), "s").unwrap();
    });
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
            for line in ["Type=Link", "Name=Music", &format!("URL=file://{}/files/Music", t.display())] {
                assert!(music.lines().any(|l| l == line), "{line} missing in {music}");
            }
            assert!(read("notes.txt.desktop").contains("\nName=notes.txt\n"));
            assert!(read("Projects.desktop").contains(&format!("\nURL=file://{}/projects\n", t.display())));
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
