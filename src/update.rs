//! Updating failBrauwser from Help > Check for Updates: the newest release on GitHub (a
//! ready-built package) or the current source, built here. The work is done by a shell
//! script started on its own, so it outlives the program it replaces; the source it builds
//! from is deleted when it is done.

use std::path::{Path, PathBuf};

pub const REPO: &str = "KsmBl/failBrauwser";

/// Where the newest release is asked for (`FB_UPDATE_API` replaces it in tests).
pub fn release_api() -> String {
    std::env::var("FB_UPDATE_API").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"))
}

/// Where the current source is fetched from (`FB_UPDATE_SOURCE` replaces it in tests).
pub fn source_url() -> String {
    std::env::var("FB_UPDATE_SOURCE").unwrap_or_else(|_| format!("https://codeload.github.com/{REPO}/tar.gz/refs/heads/main"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub name: String,
    /// When it was published (`2026-10-30T12:00:00Z`).
    pub published: String,
    /// The ready-built package for this machine, if the release has one.
    pub package: Option<String>,
    pub page: String,
}

/// The name of the ready-built package for a version and this machine. One formula:
/// `package.ps1` names the zip it produces exactly this way.
pub fn package_name(version: &str) -> String {
    let v = version.trim_start_matches('v');
    #[cfg(windows)]
    let name = format!("failbrauwser-{v}-{}-windows.zip", std::env::consts::ARCH);
    #[cfg(not(windows))]
    let name = format!("failbrauwser-{v}-{}-linux.tar.gz", std::env::consts::ARCH);
    name
}

/// Reads GitHub's answer for the latest release.
pub fn parse_release(json: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v["tag_name"].as_str()?.to_string();
    #[cfg(windows)]
    let suffix = format!("-{}-windows.zip", std::env::consts::ARCH);
    #[cfg(not(windows))]
    let suffix = format!("-{}-linux.tar.gz", std::env::consts::ARCH);
    let package = v["assets"].as_array().and_then(|a| {
        a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.ends_with(&suffix))).and_then(|x| x["browser_download_url"].as_str()).map(str::to_string)
    });
    Some(Release {
        name: v["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&tag).to_string(),
        published: v["published_at"].as_str().unwrap_or_default().to_string(),
        page: v["html_url"].as_str().unwrap_or_default().to_string(),
        tag,
        package,
    })
}

fn numbers(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches(['v', 'V']).split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

/// Whether version `tag` (`v0.6`) is newer than `current` (`0.5.0`).
pub fn is_newer(tag: &str, current: &str) -> bool {
    let (mut a, mut b) = (numbers(tag), numbers(current));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

/// The installation this program runs from: the folder whose `bin` holds it —
/// `~/.local` for `~/.local/bin/failbrauwser`, the install folder for its Windows twin
/// `<install>\bin\failbrauwser.exe` (where `bin`, `lib` and `share` sit side by side as
/// `install.sh` and `package.ps1` lay them out). A build still under `target` — folder
/// `debug` or `release`, never `bin` — is not an installation.
pub fn prefix_of(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    (bin.file_name()? == "bin").then(|| bin.parent().map(Path::to_path_buf)).flatten()
}

/// The options the last install was made with (kept by install.sh), without the ones that
/// only make sense once.
pub fn install_options(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .map(|s| s.split_whitespace().filter(|o| !matches!(*o, "--uninstall" | "--prebuilt" | "--reopen")).map(str::to_string).collect())
        .unwrap_or_default()
}

/// `'text'` for the shell.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `'text'` for PowerShell: single quotes need doubling, not a shell escape.
#[cfg(windows)]
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The PowerShell swap: download, unpack, move the running program's file aside (Windows
/// allows renaming it, only not overwriting), copy the fresh files in, drop the old file and
/// start the new program. Nothing needs to outlive this one, so no detached script.
/// The package unpacks to its `failbrauwser-*` folder holding `bin`, `lib` and `share`;
/// their contents are copied into the install folder, whose stale extra files stay (the
/// same-named ones — the program and its DLL closure — are all replaced, which is what
/// matters).
#[cfg(windows)]
pub fn script(package: Option<&str>, prefix: &Path, _options: &[String], cache: &Path) -> String {
    let (cache_q, prefix_q) = (ps_quote(&cache.display().to_string()), ps_quote(&prefix.display().to_string()));
    let get = match package {
        Some(url) => format!("curl.exe -fsSL --retry 2 -o (Join-Path $work 'package.zip') {}\nExpand-Archive (Join-Path $work 'package.zip') -DestinationPath (Join-Path $work 'files')", ps_quote(url)),
        None => "Write-Error 'On Windows updates come as the ready-built release; this one has no package.'; exit 1".to_string(),
    };
    format!(
        "$ErrorActionPreference = 'Stop'\n# failBrauwser update (Help > Check for Updates)\n$cache = {cache_q}\nNew-Item -ItemType Directory -Force -Path $cache | Out-Null\n$work = Join-Path $cache ('failbrauwser-update-' + [guid]::NewGuid().ToString('N'))\nNew-Item -ItemType Directory -Path $work | Out-Null\ntry {{\n{get}\n$target = {prefix_q}\n$bin = Join-Path $target 'bin'\nNew-Item -ItemType Directory -Force -Path $bin | Out-Null\n$exe = Join-Path $bin 'failbrauwser.exe'\n$old = \"$exe.old\"\nif (Test-Path $old) {{ Remove-Item $old -Force }}\nif (Test-Path $exe) {{ Move-Item $exe $old -Force }}\n$src = Join-Path $work 'files'\n$inner = Get-ChildItem $src -Directory -Filter 'failbrauwser-*' | Select-Object -First 1\nif ($inner) {{ $src = $inner.FullName }}\nCopy-Item (Join-Path $src '*') $target -Recurse -Force\nRemove-Item $old -Force -ErrorAction SilentlyContinue\nWrite-Output 'UPDATE FINISHED'\ntry {{ Start-Process $exe }} catch {{ }}\n}} finally {{\nRemove-Item $work -Recurse -Force -ErrorAction SilentlyContinue\nWrite-Output 'Removed the downloaded files.'\n}}\n"
    )
}

/// The shell script that installs a downloaded release package, or builds the source
/// (`package: None`), into `prefix` with `options`; the download goes into a folder of its own
/// below `cache`, the source into `cache/failbrauwser/source`, both removed at the end,
/// whatever happened.
#[cfg(not(windows))]
pub fn script(package: Option<&str>, prefix: &Path, options: &[String], cache: &Path) -> String {
    let opts: String = options.iter().map(|o| format!(" {}", quote(o))).collect();
    let fetch = |url: &str, into: &str| format!("curl -fL --retry 2 -o {into} {} || wget -O {into} {}", quote(url), quote(url));
    let (get, run) = match package {
        Some(url) => (
            format!("echo \"Downloading {url}\"\n{}\ntar xzf package.tar.gz\ncd failbrauwser-*/", fetch(url, "package.tar.gz")),
            format!("./install.sh --prebuilt --prefix {}{opts} --reopen", quote(&prefix.display().to_string())),
        ),
        // The source is unpacked at the same place every time: Rust names what it builds of a
        // folder after the folder, and install.sh keeps the build in the cache (README:
        // Updates), so the same place lets the next update reuse it instead of adding to it.
        None => (
            format!(
                "echo \"Downloading the source\"\n{}\nsrc={}\nrm -rf \"$src\"\nmkdir -p \"$src\"\ntar xzf source.tar.gz -C \"$src\" --strip-components=1\ncd \"$src\"",
                fetch(&source_url(), "source.tar.gz"),
                quote(&cache.join("failbrauwser").join("source").display().to_string())
            ),
            format!("./install.sh --prefix {}{opts} --reopen", quote(&prefix.display().to_string())),
        ),
    };
    format!(
        "#!/bin/sh\n# failBrauwser update (Help > Check for Updates)\nset -e\nmkdir -p {cache}\nsrc=\nwork=$(mktemp -d {cache}/failbrauwser-update.XXXXXX)\ntrap 'cd /; rm -rf \"$work\" ${{src:+\"$src\"}}; echo \"Removed the downloaded files.\"' EXIT\ncd \"$work\"\n{get}\necho \"Installing\"\n{run}\necho \"UPDATE FINISHED\"\n",
        cache = quote(&cache.display().to_string())
    )
}

/// How far an update is, read from what its script wrote so far (README, Updates).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// The step, in words: "Compiling 12 of 90 crates (glib)", "Downloading the source", …
    pub step: String,
    /// How much of the step is done, where that is known (compiling); otherwise the bar pulses.
    pub fraction: Option<f64>,
    /// The script said `UPDATE FINISHED`.
    pub done: bool,
}

/// The progress an update's log `log` shows: the last step the script or `install.sh` named
/// (`==> …` lines), and while the source compiles the crates done of the crates
/// `install.sh` announced (`Building failBrauwser: <n> crates`), one `Compiling` line each.
pub fn progress(log: &str) -> Progress {
    let mut p = Progress { step: "Starting".into(), fraction: None, done: false };
    let (mut total, mut compiled, mut fetched) = (None::<usize>, 0usize, 0usize);
    // curl's meter rewrites its line with carriage returns.
    for raw in log.split(['\n', '\r']) {
        let line = strip_ansi(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "UPDATE FINISHED" {
            p = Progress { step: "Done".into(), fraction: Some(1.0), done: true };
            total = None;
        } else if let Some(said) = line.strip_prefix("==> ") {
            total = said.strip_prefix("Building failBrauwser: ").and_then(|r| r.strip_suffix(" crates")).and_then(|n| n.parse().ok());
            compiled = 0;
            fetched = 0;
            p.step = said.to_string();
            p.fraction = total.map(|_| 0.0);
        } else if let Some(rest) = line.strip_prefix("Compiling ") {
            let name = rest.split_whitespace().next().unwrap_or_default();
            match total {
                Some(n) => {
                    compiled += 1;
                    // The last crate has not finished when it starts.
                    p.fraction = Some((compiled.saturating_sub(1) as f64 / n.max(1) as f64).min(0.99));
                    p.step = format!("Compiling {} of {n} crates ({name})", compiled.min(n));
                }
                None => p.step = format!("Compiling {name}"),
            }
        } else if line.starts_with("Downloaded ") && total.is_some() {
            fetched += 1;
            p.step = format!("Downloading crates ({fetched})");
        } else if line.starts_with("Downloading ") && !line.starts_with("Downloading crates") || line == "Installing" {
            p.step = line.trim_end_matches('…').to_string();
            p.fraction = None;
        }
    }
    p
}

/// `s` without terminal colour codes (`install.sh` prints its `==>` in bold).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_update_counts_its_crates() {
        let mut log = String::from("Downloading the source\n  % Total    % Received\r 40  1000\r100  2500\nInstalling\n\u{1b}[1m==>\u{1b}[0m Building failBrauwser: 4 crates\n");
        assert_eq!(progress(&log), Progress { step: "Building failBrauwser: 4 crates".into(), fraction: Some(0.0), done: false });
        log.push_str("    Updating crates.io index\n  Downloaded libc v0.2.1\n  Downloaded glib v0.19.0\n");
        assert_eq!(progress(&log).step, "Downloading crates (2)");
        assert_eq!(progress(&log).fraction, Some(0.0));
        log.push_str("   Compiling libc v0.2.1\n   Compiling glib v0.19.0\n   Compiling gtk v0.19.0\n");
        let p = progress(&log);
        assert_eq!(p.step, "Compiling 3 of 4 crates (gtk)");
        assert_eq!(p.fraction, Some(0.5), "two finished of four");
        log.push_str("   Compiling failbrauwser v0.9.0 (/tmp/x)\n   Compiling extra v1.0.0\n");
        assert_eq!(progress(&log).fraction, Some(0.99), "never full before the build ends");
        log.push_str("    Finished `release` profile\n\u{1b}[1m==>\u{1b}[0m Waiting for the archive helper\n");
        assert_eq!(progress(&log), Progress { step: "Waiting for the archive helper".into(), fraction: None, done: false });
        log.push_str("\u{1b}[1m==>\u{1b}[0m Installing into /home/me/.local\nUPDATE FINISHED\nRemoved the downloaded files.\n");
        assert!(progress(&log).done);
        assert_eq!(progress(&log).fraction, Some(1.0));
    }

    #[test]
    fn steps_without_a_known_length_pulse() {
        assert_eq!(progress(""), Progress { step: "Starting".into(), fraction: None, done: false });
        let p = progress("Downloading https://example.org/pkg.tar.gz\n");
        assert_eq!((p.step.as_str(), p.fraction), ("Downloading https://example.org/pkg.tar.gz", None));
        // An install.sh too old to count, or cargo unable to: compiling pulses, named.
        let p = progress("==> Building failBrauwser\n   Compiling glib v0.19.0\n");
        assert_eq!((p.step.as_str(), p.fraction), ("Compiling glib", None));
    }

    #[test]
    fn releases_and_versions() {
        let pkg = package_name("v0.6");
        #[cfg(windows)]
        assert_eq!(pkg, format!("failbrauwser-0.6-{}-windows.zip", std::env::consts::ARCH));
        #[cfg(not(windows))]
        assert_eq!(pkg, format!("failbrauwser-0.6-{}-linux.tar.gz", std::env::consts::ARCH));
        let json = format!(
            r#"{{"tag_name": "v0.6", "name": "failBrauwser 0.6", "published_at": "2026-10-30T19:00:00Z", "html_url": "https://github.com/{REPO}/releases/tag/v0.6",
               "assets": [{{"name": "notes.txt", "browser_download_url": "https://x/notes.txt"}}, {{"name": "{pkg}", "browser_download_url": "https://x/{pkg}"}}]}}"#
        );
        let r = parse_release(&json).unwrap();
        assert_eq!((r.tag.as_str(), r.name.as_str()), ("v0.6", "failBrauwser 0.6"));
        assert_eq!(r.package, Some(format!("https://x/{pkg}")));
        assert!(parse_release(r#"{"tag_name": "v0.2", "assets": []}"#).unwrap().package.is_none());
        assert!(parse_release("not json").is_none());
        assert!(is_newer("v0.6", "0.5.0") && is_newer("v0.10", "0.9.3") && is_newer("v1.0", "0.99"));
        assert!(!is_newer("v0.5", "0.5.0") && !is_newer("v0.4", "0.5.0"));
    }

    #[test]
    #[cfg(unix)]
    fn installs_and_scripts() {
        assert_eq!(prefix_of(Path::new("/home/me/.local/bin/failbrauwser")), Some(PathBuf::from("/home/me/.local")));
        assert_eq!(prefix_of(Path::new("/home/me/src/target/release/failbrauwser")), None);
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("install-options");
        std::fs::write(&f, "--default --no-network --reopen\n").unwrap();
        assert_eq!(install_options(&f), ["--default", "--no-network"]);
        assert!(install_options(&dir.path().join("none")).is_empty());
        assert_eq!(quote("it's"), r"'it'\''s'");
        let s = script(Some("https://x/p.tar.gz"), Path::new("/home/me/.local"), &["--default".into()], Path::new("/home/me/.cache"));
        assert!(s.contains("./install.sh --prebuilt --prefix '/home/me/.local' '--default' --reopen"), "{s}");
        assert!(s.contains("trap 'cd /; rm -rf \"$work\"") && s.contains("mktemp -d '/home/me/.cache'/failbrauwser-update.XXXXXX"));
        let s = script(None, Path::new("/opt/fb"), &[], Path::new("/c"));
        assert!(s.contains("src='/c/failbrauwser/source'\nrm -rf \"$src\"") && s.contains("tar xzf source.tar.gz -C \"$src\" --strip-components=1"), "{s}");
        assert!(s.contains("./install.sh --prefix '/opt/fb' --reopen") && s.contains("rm -rf \"$work\" ${src:+\"$src\"}"), "{s}");
    }

    #[test]
    #[cfg(windows)]
    fn the_install_folder_is_the_prefix_and_a_debug_build_is_not() {
        assert_eq!(prefix_of(Path::new(r"C:\Users\me\AppData\Local\Programs\failbrauwser\bin\failbrauwser.exe")), Some(PathBuf::from(r"C:\Users\me\AppData\Local\Programs\failbrauwser")));
        assert_eq!(prefix_of(Path::new(r"A:\src\target\debug\failbrauwser.exe")), None);
        assert_eq!(prefix_of(Path::new(r"A:\src\target\release\failbrauwser.exe")), None);
        // An exe loose in some folder is not an installation either: the swap would
        // scatter bin/lib/share into wherever it happens to run.
        assert_eq!(prefix_of(Path::new(r"A:\misc\failbrauwser.exe")), None);
    }

    /// Runs the made swap against a fake install laid out like `package.ps1`: the program
    /// file in `bin` is renamed aside, the packaged one takes its place, the payload under
    /// the package's `failbrauwser-*` folder arrives (the new DLL in `bin`, the data tree
    /// in `share`), and the download leaves nothing behind.
    #[test]
    #[cfg(windows)]
    fn the_swap_script_replaces_the_running_program() {
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("inst");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(target.join("bin")).unwrap();
        std::fs::write(target.join("bin\\failbrauwser.exe"), b"OLD-EXE").unwrap();
        let pack_root = d.path().join("failbrauwser-9.9-x86_64-windows");
        std::fs::create_dir_all(pack_root.join("bin")).unwrap();
        std::fs::create_dir_all(pack_root.join("share")).unwrap();
        std::fs::write(pack_root.join("bin\\failbrauwser.exe"), b"NEW-EXE").unwrap();
        std::fs::write(pack_root.join("bin\\new.dll"), b"NEW-DLL").unwrap();
        std::fs::write(pack_root.join("share\\thing.txt"), b"NEW-DATA").unwrap();
        // Same shapes the app itself uses: policy may stop PowerShell from loading script
        // files below the temp folder, so the text goes in as a command.
        let zip = d.path().join("package.zip");
        let pack = format!("Compress-Archive -Path '{}\\*' -DestinationPath '{}'", pack_root.display(), zip.display());
        let mk = std::process::Command::new("powershell").args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &pack]).output().unwrap();
        assert!(mk.status.success(), "{}{}", String::from_utf8_lossy(&mk.stdout), String::from_utf8_lossy(&mk.stderr));
        let url = format!("file:///{}", zip.display().to_string().replace('\\', "/"));
        let swap = script(Some(&url), &target, &[], &d.path().join("cache"));
        let out = std::process::Command::new("powershell").args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &swap]).output().unwrap();
        let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && log.contains("UPDATE FINISHED"), "{log}");
        assert_eq!(std::fs::read(target.join("bin\\failbrauwser.exe")).unwrap(), b"NEW-EXE");
        assert_eq!(std::fs::read(target.join("bin\\new.dll")).unwrap(), b"NEW-DLL");
        assert_eq!(std::fs::read(target.join("share\\thing.txt")).unwrap(), b"NEW-DATA");
        assert!(!target.join("bin\\failbrauwser.exe.old").exists(), "the renamed old program is dropped: {log}");
        assert_eq!(std::fs::read_dir(d.path().join("cache")).map(|r| r.count()).unwrap_or(0), 0, "the download folder is gone: {log}");
    }
}
