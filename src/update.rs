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
/// below `cache` that is removed at the end, whatever happened.
#[cfg(not(windows))]
pub fn script(package: Option<&str>, prefix: &Path, options: &[String], cache: &Path) -> String {
    let opts: String = options.iter().map(|o| format!(" {}", quote(o))).collect();
    let fetch = |url: &str, into: &str| format!("curl -fL --retry 2 -o {into} {} || wget -O {into} {}", quote(url), quote(url));
    let (get, run) = match package {
        Some(url) => (
            format!("echo \"Downloading {url}\"\n{}\ntar xzf package.tar.gz\ncd failbrauwser-*/", fetch(url, "package.tar.gz")),
            format!("./install.sh --prebuilt --prefix {}{opts} --reopen", quote(&prefix.display().to_string())),
        ),
        None => (
            format!("echo \"Downloading the source\"\n{}\nmkdir source\ntar xzf source.tar.gz -C source --strip-components=1\ncd source", fetch(&source_url(), "source.tar.gz")),
            format!("./install.sh --prefix {}{opts} --reopen", quote(&prefix.display().to_string())),
        ),
    };
    format!(
        "#!/bin/sh\n# failBrauwser update (Help > Check for Updates)\nset -e\nmkdir -p {cache}\nwork=$(mktemp -d {cache}/failbrauwser-update.XXXXXX)\ntrap 'cd /; rm -rf \"$work\"; echo \"Removed the downloaded files.\"' EXIT\ncd \"$work\"\n{get}\necho \"Installing\"\n{run}\necho \"UPDATE FINISHED\"\n",
        cache = quote(&cache.display().to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(s.contains("tar xzf source.tar.gz -C source --strip-components=1") && s.contains("./install.sh --prefix '/opt/fb' --reopen"));
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
