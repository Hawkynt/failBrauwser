// Shared by the two Windows build scripts (`build.rs` and `winpack\onefile\build.rs`), which
// are separate crates: compile the application icon resource once per crate and hand the
// result to the linker, so every program file shows the same icon in Explorer, the Start Menu
// and the titlebar. See docs\WINDOWS.md.

use std::path::{Component, Path, PathBuf};

#[allow(dead_code)] // Windows-only wiring: a Linux build links no icon resource.
fn manifest_dir() -> PathBuf { PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo")) }

/// Drops `.` and `..` segments: rc.exe takes a folder as given, and a path still spelled
/// `onefile\..\data\icons` is one it refuses to open.
#[allow(dead_code)]
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Compiles the icon resource at `source` (relative to this crate) with the Windows SDK's
/// `rc.exe`. A machine without that compiler links the same programs without the icon, and
/// says so at build time rather than failing.
#[allow(dead_code)]
fn link_icon(source: &str) {
    println!("cargo:rerun-if-changed={source}");
    println!("cargo:rerun-if-changed={}", source.replace(".rc", ".ico"));
    let source = normalized(&manifest_dir().join(source));
    let (Some(rc), Some(out)) = (rc_compiler(), std::env::var_os("OUT_DIR").map(PathBuf::from)) else {
        println!("cargo:warning=no rc.exe from the Windows SDK: these programs link without the application icon");
        return;
    };
    let resource = out.join("failbrauwser.res");
    // rc.exe reads the .ico next to the .rc, and refuses a path that still spells out `..`, so
    // both file names go over fully resolved; `/fo<path>` keeps a folder with a space in one word.
    let built = std::process::Command::new(&rc).args(["/nologo", &format!("/fo{}", resource.display()), &source.display().to_string()]).status();
    if !matches!(built, Ok(status) if status.success()) {
        println!("cargo:warning={} could not compile {}: these programs link without the application icon", rc.display(), source.display());
        return;
    }
    println!("cargo:rustc-link-arg={}", resource.display());
}

/// The `rc.exe` this machine offers: an explicit `FB_RC` first, then PATH (a shell that ran
/// vcvars already has one), then the newest Windows Kit under Program Files.
#[allow(dead_code)]
fn rc_compiler() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("FB_RC").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(explicit);
    }
    if let Some(on_path) = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|d| d.join("rc.exe")).find(|p| p.is_file()) {
        return Some(on_path);
    }
    let program_files = ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"].into_iter().filter_map(std::env::var_os).map(PathBuf::from);
    let mut newest: Option<(String, PathBuf)> = None;
    for root in program_files {
        let Ok(versions) = std::fs::read_dir(root.join("Windows Kits\\10\\bin")) else { continue };
        for version in versions.flatten() {
            let candidate = version.path().join("x64\\rc.exe");
            if !candidate.is_file() {
                continue;
            }
            let name = version.file_name().to_string_lossy().into_owned();
            if newest.as_ref().is_none_or(|(v, _)| &name > v) {
                newest = Some((name, candidate));
            }
        }
    }
    newest.map(|(_, path)| path)
}
