//! install.sh on a Debian 13 system, played by stand-ins: `apt-get` and `dpkg-query` that
//! record what is installed, the system's Rust 1.85 (older than the bindings need), and
//! `curl` handing out a stand-in for rustup's installer. Nothing is downloaded or installed
//! for real.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Debian {
    dir: tempfile::TempDir,
}

fn write_exe(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

impl Debian {
    /// A source folder (install.sh and Cargo.toml), a home and the stand-ins, with the system's
    /// Rust at `rust` and the packages `installed`.
    fn new(rust: &str, installed: &[&str]) -> Debian {
        let dir = tempfile::tempdir().unwrap();
        let d = Debian { dir };
        let src = d.path("src");
        std::fs::create_dir_all(&src).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(root.join("install.sh"), src.join("install.sh")).unwrap();
        std::fs::copy(root.join("Cargo.toml"), src.join("Cargo.toml")).unwrap();
        std::fs::create_dir_all(d.path("home")).unwrap();
        std::fs::write(d.path("installed"), installed.iter().map(|p| format!("{p}\n")).collect::<String>()).unwrap();
        let t = d.dir.path().display().to_string();
        let bin = d.path("bin");
        write_exe(&bin.join("sudo"), &format!("#!/bin/sh\necho \"$*\" >> {t}/sudo.log\nexec \"$@\"\n"));
        write_exe(&bin.join("apt-get"), &format!("#!/bin/sh\necho \"$*\" >> {t}/apt.log\nshift 2\nfor p; do echo \"$p\" >> {t}/installed; done\n"));
        write_exe(
            &bin.join("dpkg-query"),
            &format!("#!/bin/sh\nfor p; do :; done\ngrep -qx \"$p\" {t}/installed && printf 'install ok installed' && exit 0\nexit 1\n"),
        );
        write_exe(&bin.join("pkg-config"), "#!/bin/sh\nexit 0\n");
        // curl -fsSL <url> -o <file>: rustup's installer, which puts Rust 1.92 into $CARGO_HOME.
        let rustup = format!(
            "#!/bin/sh\necho \"$*\" > {t}/rustup.args\necho \"$CARGO_HOME $RUSTUP_HOME\" > {t}/rustup.homes\nmkdir -p \"$CARGO_HOME/bin\"\n\
             printf '#!/bin/sh\\necho \"rustc 1.92.0 (ded5c06cf 2025-12-08)\"\\n' > \"$CARGO_HOME/bin/rustc\"\n\
             printf '#!/bin/sh\\necho \"cargo 1.92.0 (344c4567c 2025-10-21)\"\\n' > \"$CARGO_HOME/bin/cargo\"\n\
             printf '#!/bin/sh\\necho \"$*\" >> {t}/rustup-update.log\\n' > \"$CARGO_HOME/bin/rustup\"\n\
             chmod +x \"$CARGO_HOME/bin/\"*\n"
        );
        std::fs::write(d.path("rustup-init.sh"), rustup).unwrap();
        write_exe(
            &bin.join("curl"),
            &format!("#!/bin/sh\nfor a; do case \"$prev\" in -o) out=$a;; esac; prev=$a; done\nfor u; do case \"$u\" in https://*) echo \"$u\" >> {t}/downloads;; esac; done\ncp {t}/rustup-init.sh \"$out\"\n"),
        );
        d.system_rust(rust);
        d
    }

    fn system_rust(&self, version: &str) {
        let bin = self.path("bin");
        write_exe(&bin.join("rustc"), &format!("#!/bin/sh\necho \"rustc {version} (4d91de4e4 2025-02-17) (built from a source tarball)\"\n"));
        write_exe(&bin.join("cargo"), &format!("#!/bin/sh\necho \"cargo {version} (65f24aea6 2025-02-17)\"\n"));
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    fn install(&self, args: &[&str]) -> Output {
        Command::new("bash")
            .arg(self.path("src/install.sh"))
            .args(args)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.path("bin").display()))
            .output()
            .unwrap()
    }
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn debian_13_gets_build_packages_and_a_newer_rust() {
    let deb = Debian::new("1.85.0", &["pkg-config"]);
    let o = deb.install(&["--deps-only", "--no-archives"]);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    // Only the missing packages, through sudo.
    assert_eq!(deb.read("apt.log").trim(), "install -y build-essential libgtk-3-dev");
    assert!(deb.read("sudo.log").starts_with("apt-get install -y build-essential libgtk-3-dev"));
    // The system's Rust stays; a newer one is set up beside it for building.
    let rust_home = deb.path("home/.local/share/failbrauwser/rust");
    assert_eq!(deb.read("downloads").trim(), "https://sh.rustup.rs");
    assert_eq!(deb.read("rustup.args").trim(), "-y --no-modify-path --profile minimal --default-toolchain stable");
    assert_eq!(deb.read("rustup.homes").trim(), format!("{} {}", rust_home.join("cargo").display(), rust_home.join("rustup").display()));
    assert!(out.contains("the system has 1.85.0"), "{out}");
    assert!(out.contains("Ready to build: cargo 1.92.0"), "{out}");

    // Again: everything is there, nothing is installed or downloaded.
    std::fs::remove_file(deb.path("apt.log")).unwrap();
    std::fs::remove_file(deb.path("downloads")).unwrap();
    let o = deb.install(&["--deps-only", "--no-archives"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(deb.read("apt.log").is_empty() && deb.read("downloads").is_empty() && deb.read("rustup-update.log").is_empty());
    assert!(text(&o).contains("Ready to build: cargo 1.92.0"));
}

#[test]
fn a_new_enough_rust_and_the_archive_packages() {
    let deb = Debian::new("1.92.0", &["build-essential", "pkg-config", "libgtk-3-dev"]);
    // Without --no-archives the helper's zlib is wanted too (its source is not here, so no .NET).
    let o = deb.install(&["--deps-only"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(deb.read("apt.log").trim(), "install -y zlib1g-dev");
    assert!(deb.read("downloads").is_empty(), "the system's Rust is new enough");
    assert!(text(&o).contains("Ready to build: cargo 1.92.0"));
    assert!(!deb.path("home/.local/share/failbrauwser/rust").exists());
}

#[test]
fn an_outdated_private_rust_is_updated() {
    let deb = Debian::new("1.85.0", &["build-essential", "pkg-config", "libgtk-3-dev"]);
    let cargo_bin = deb.path("home/.local/share/failbrauwser/rust/cargo/bin");
    write_exe(&cargo_bin.join("rustc"), "#!/bin/sh\necho 'rustc 1.90.0 (1159e78c4 2025-09-14)'\n");
    write_exe(&cargo_bin.join("cargo"), "#!/bin/sh\necho cargo 1.90.0\n");
    let t = deb.dir.path().display().to_string();
    write_exe(
        &cargo_bin.join("rustup"),
        &format!("#!/bin/sh\necho \"$*\" >> {t}/rustup-update.log\nprintf '#!/bin/sh\\necho rustc 1.93.1\\n' > \"$CARGO_HOME/bin/rustc\"\nprintf '#!/bin/sh\\necho cargo 1.93.1\\n' > \"$CARGO_HOME/bin/cargo\"\n"),
    );
    let o = deb.install(&["--deps-only", "--no-archives"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(deb.read("rustup-update.log").trim(), "update stable --no-self-update");
    assert!(deb.read("downloads").is_empty());
    assert!(text(&o).contains("Ready to build: cargo 1.93.1"));
}

#[test]
fn without_any_rust_one_is_set_up() {
    let deb = Debian::new("1.85.0", &["build-essential", "pkg-config", "libgtk-3-dev"]);
    std::fs::remove_file(deb.path("bin/rustc")).unwrap();
    std::fs::remove_file(deb.path("bin/cargo")).unwrap();
    // Arch and others have their Rust in /usr/bin; this Debian has none at all.
    let o = Command::new("bash")
        .arg(deb.path("src/install.sh"))
        .args(["--deps-only", "--no-archives"])
        .env_clear()
        .env("HOME", deb.path("home"))
        .env("PATH", format!("{}:{}", deb.path("bin").display(), tool_dir(&deb).display()))
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(deb.read("downloads").trim(), "https://sh.rustup.rs");
    assert!(text(&o).contains("Ready to build: cargo 1.92.0"));
}

/// The few programs install.sh runs besides the stand-ins, without the real Rust next to them.
fn tool_dir(deb: &Debian) -> PathBuf {
    let dir = deb.path("tools");
    std::fs::create_dir_all(&dir).unwrap();
    for t in ["sh", "bash", "sed", "grep", "awk", "cut", "sort", "head", "mktemp", "rm", "cp", "mkdir", "chmod", "printf", "cat", "dirname", "ls"] {
        let found = ["/usr/bin", "/bin"].iter().map(|d| Path::new(d).join(t)).find(|p| p.exists());
        if let Some(p) = found {
            let _ = std::os::unix::fs::symlink(p, dir.join(t));
        }
    }
    dir
}

#[test]
fn a_package_built_for_a_newer_system_is_refused() {
    let deb = Debian::new("1.85.0", &[]);
    let pkg = deb.path("src");
    write_exe(&pkg.join("bin/failbrauwser"), "#!/bin/sh\necho \"failbrauwser: /lib/x86_64-linux-gnu/libc.so.6: version \\`GLIBC_2.42' not found\" >&2\nexit 1\n");
    let prefix = deb.path("prefix");
    let o = deb.install(&["--prebuilt", "--prefix", &prefix.to_string_lossy(), "--no-autostart", "--no-file-chooser", "--no-network"]);
    let out = text(&o);
    assert!(!o.status.success(), "{out}");
    assert!(out.contains("does not run on this system") && out.contains("GLIBC_2.42"), "{out}");
    assert!(out.contains("build it from the source"), "{out}");
    assert!(!prefix.exists(), "nothing was installed");
    assert!(deb.read("apt.log").is_empty() && deb.read("downloads").is_empty(), "a release package builds nothing");
}

/// A release package beside install.sh (bin/, lib/ with the GTK 3 module, data/), and a
/// session whose GTK3_MODULES the stand-in `systemctl` keeps in `session-env`.
fn release_package(deb: &Debian) {
    let src = deb.path("src");
    write_exe(&src.join("bin/failbrauwser"), "#!/bin/sh\n[ \"$1\" = --version ] && echo failbrauwser 0.7.0\nexit 0\n");
    std::fs::create_dir_all(src.join("lib")).unwrap();
    std::fs::write(src.join("lib/libfailbrauwser-chooser.so"), "module").unwrap();
    let copy = Command::new("cp").arg("-r").arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("data")).arg(&src).status().unwrap();
    assert!(copy.success());
    let t = deb.dir.path().display().to_string();
    std::fs::write(deb.path("session-env"), "GTK3_MODULES=/usr/lib/gtk-3.0/modules/libcanberra-gtk-module.so\n").unwrap();
    write_exe(
        &deb.path("bin/systemctl"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> {t}/systemctl.log\ncase \"$2\" in\n  show-environment) cat {t}/session-env ;;\n  set-environment) echo \"$3\" > {t}/session-env ;;\n  unset-environment) : > {t}/session-env ;;\nesac\nexit 0\n"
        ),
    );
    write_exe(&deb.path("bin/dbus-update-activation-environment"), &format!("#!/bin/sh\necho \"$*\" >> {t}/dbus-env.log\n"));
}

const QUIET: [&str; 3] = ["--no-autostart", "--no-network", "--no-archives"];

#[test]
fn gtk3_applications_get_the_module_through_the_session_environment() {
    let deb = Debian::new("1.92.0", &[]);
    release_package(&deb);
    let prefix = deb.path("prefix");
    let prefix_arg = prefix.to_string_lossy().to_string();
    let o = deb.install(&[&["--prebuilt", "--prefix", &prefix_arg][..], &QUIET].concat());
    assert!(o.status.success(), "{}", text(&o));
    let module = prefix.join("lib/failbrauwser/libfailbrauwser-chooser.so");
    assert!(module.exists());
    // For every session that reads environment.d, before the modules the user has.
    let env = std::fs::read_to_string(deb.path("home/.config/environment.d/60-failbrauwser.conf")).unwrap();
    assert!(env.contains(&format!("\nGTK3_MODULES={}${{GTK3_MODULES:+:$GTK3_MODULES}}\n", module.display())), "{env}");
    // The running session too, keeping its own modules.
    let both = format!("GTK3_MODULES={}:/usr/lib/gtk-3.0/modules/libcanberra-gtk-module.so", module.display());
    assert_eq!(deb.read("session-env").trim(), both);
    assert_eq!(deb.read("dbus-env.log").trim(), both);

    // Installing again changes nothing in the session.
    std::fs::remove_file(deb.path("dbus-env.log")).unwrap();
    let o = deb.install(&[&["--prebuilt", "--prefix", &prefix_arg][..], &QUIET].concat());
    assert!(o.status.success(), "{}", text(&o));
    assert!(deb.read("dbus-env.log").is_empty());
    assert_eq!(deb.read("session-env").trim(), both);

    // Uninstalled: the module, the file and the session's entry go; the user's module stays.
    let o = deb.install(&[&["--uninstall", "--prefix", &prefix_arg][..], &QUIET].concat());
    assert!(o.status.success(), "{}", text(&o));
    assert!(!module.exists() && !deb.path("home/.config/environment.d/60-failbrauwser.conf").exists());
    assert_eq!(deb.read("session-env").trim(), "GTK3_MODULES=/usr/lib/gtk-3.0/modules/libcanberra-gtk-module.so");
}

#[test]
fn without_the_file_chooser_gtk3_applications_keep_their_dialogs() {
    let deb = Debian::new("1.92.0", &[]);
    release_package(&deb);
    let prefix = deb.path("prefix");
    let o = deb.install(&[&["--prebuilt", "--prefix", &prefix.to_string_lossy(), "--no-file-chooser"][..], &QUIET].concat());
    assert!(o.status.success(), "{}", text(&o));
    assert!(!prefix.join("lib/failbrauwser/libfailbrauwser-chooser.so").exists());
    assert!(!deb.path("home/.config/environment.d/60-failbrauwser.conf").exists());
    assert_eq!(deb.read("session-env").trim(), "GTK3_MODULES=/usr/lib/gtk-3.0/modules/libcanberra-gtk-module.so");
}

#[test]
fn systemd_reads_the_environment_file_as_meant() {
    let generator = Path::new("/usr/lib/systemd/user-environment-generators/30-systemd-environment-d-generator");
    if !generator.exists() {
        eprintln!("no systemd environment generator, skipping");
        return;
    }
    let deb = Debian::new("1.92.0", &[]);
    release_package(&deb);
    let prefix = deb.path("prefix");
    let o = deb.install(&[&["--prebuilt", "--prefix", &prefix.to_string_lossy()][..], &QUIET].concat());
    assert!(o.status.success(), "{}", text(&o));
    let module = prefix.join("lib/failbrauwser/libfailbrauwser-chooser.so");
    let read = |modules: Option<&str>| {
        let mut c = Command::new(generator);
        c.env_clear().env("HOME", deb.path("home")).env("XDG_CONFIG_HOME", deb.path("home/.config"));
        if let Some(m) = modules {
            c.env("GTK3_MODULES", m);
        }
        let out = String::from_utf8_lossy(&c.output().unwrap().stdout).into_owned();
        out.lines().find_map(|l| l.strip_prefix("GTK3_MODULES=").map(str::to_string)).unwrap_or_default()
    };
    assert_eq!(read(None), module.display().to_string());
    assert_eq!(read(Some("/x/other.so")), format!("{}:/x/other.so", module.display()));
}

/// A source folder as a download has it (no .git), with stand-ins that build: `cargo` writes
/// the program into the build folder it is given and logs which, `make helper` writes the archive
/// helper and logs that it ran, `dotnet` is an SDK 10.
fn buildable_source(deb: &Debian) {
    let src = deb.path("src");
    let t = deb.dir.path().display().to_string();
    let copy = Command::new("cp").arg("-r").arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("data")).arg(&src).status().unwrap();
    assert!(copy.success());
    std::fs::write(src.join("Makefile"), "helper:\n").unwrap();
    std::fs::create_dir_all(src.join("helper/FbArchive")).unwrap();
    std::fs::write(src.join("helper/FbArchive/Program.cs"), "// one\n").unwrap();
    std::fs::create_dir_all(src.join("vendor/compressionworkbench/Compression.Lib")).unwrap();
    std::fs::write(src.join("vendor/compressionworkbench/Compression.Lib/Compression.Lib.csproj"), "<Project/>\n").unwrap();
    write_exe(
        &deb.path("bin/cargo"),
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'cargo 1.92.0' ;;\n  tree) echo 'gtk v0.19'; echo 'glib v0.19' ;;\n  build) echo \"build ${{CARGO_TARGET_DIR:-target}}\" >> {t}/cargo.log\n    out=\"${{CARGO_TARGET_DIR:-target}}/release\"; mkdir -p \"$out\"\n    printf '#!/bin/sh\\nexit 0\\n' > \"$out/failbrauwser\"; chmod +x \"$out/failbrauwser\" ;;\nesac\n"
        ),
    );
    write_exe(
        &deb.path("bin/make"),
        &format!("#!/bin/sh\necho \"$1\" >> {t}/make.log\n[ \"$1\" = helper ] || exit 0\nmkdir -p target/helper\nprintf '#!/bin/sh\\necho helper\\n' > target/helper/fb-archive\nchmod +x target/helper/fb-archive\n"),
    );
    write_exe(&deb.path("bin/dotnet"), "#!/bin/sh\n[ \"$1\" = --list-sdks ] && echo '10.0.100 [/sdk]'\nexit 0\n");
}

#[test]
fn a_downloaded_source_builds_again_only_what_changed() {
    let deb = Debian::new("1.92.0", &["build-essential", "pkg-config", "libgtk-3-dev", "zlib1g-dev"]);
    buildable_source(&deb);
    let prefix = deb.path("prefix");
    let args = ["--prefix", &prefix.to_string_lossy(), "--no-autostart", "--no-network", "--no-file-chooser"];
    let build_dir = deb.path("home/.cache/failbrauwser/build");

    // The first build: every crate counted, the helper built and kept.
    let o = deb.install(&args);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("Building failBrauwser: 2 crates"), "{out}");
    assert_eq!(deb.read("cargo.log"), format!("build {}\n", build_dir.display()), "a download builds into the kept folder");
    assert_eq!(deb.read("make.log"), "helper\n");
    assert!(prefix.join("lib/failbrauwser/fb-archive").exists() && prefix.join("bin/failbrauwser").exists());

    // Again from the same sources: the helper is taken as it is, the build folder goes on.
    let o = deb.install(&args);
    let out = text(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.contains("archive helper is unchanged"), "{out}");
    assert!(out.contains(" Building failBrauwser\n"), "a build that goes on is not counted: {out}");
    assert_eq!(deb.read("make.log"), "helper\n", "the helper was not built again");
    assert!(prefix.join("lib/failbrauwser/fb-archive").exists());

    // A changed helper source builds it again, and only the newest one is kept.
    std::fs::write(deb.path("src/helper/FbArchive/Program.cs"), "// two\n").unwrap();
    let o = deb.install(&args);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(deb.read("make.log"), "helper\nhelper\n");
    assert_eq!(std::fs::read_dir(deb.path("home/.cache/failbrauwser/helper")).unwrap().count(), 1);

    // A git checkout keeps its own target folder.
    std::fs::create_dir(deb.path("src/.git")).unwrap();
    let o = deb.install(&args);
    assert!(o.status.success(), "{}", text(&o));
    assert!(deb.read("cargo.log").ends_with("build target\n"), "{}", deb.read("cargo.log"));
}
