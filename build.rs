fn main() {
    // Embed the application manifest (longPathAware, asInvoker, Win10 OS GUID) into
    // every Windows binary the package links — engines and integration tests alike.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo")).join("winpack/fb.manifest");
    println!("cargo:rerun-if-changed=winpack/fb.manifest");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    for flag in manifest_link_args(&manifest) {
        println!("cargo:rustc-link-arg={flag}");
        // rustc-link-arg reaches the tests already; MSVC takes the input twice without
        // complaint, MinGW's ld would see two manifest resources
        if msvc {
            println!("cargo:rustc-link-arg-tests={flag}");
        }
    }
}

/// Linker arguments that embed `manifest`: MSVC's linker takes the file itself, the MinGW
/// one (cross builds from Linux, winpack/cross.sh) links it as a resource object made by windres.
fn manifest_link_args(manifest: &std::path::Path) -> Vec<String> {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        return vec!["/MANIFEST:EMBED".into(), format!("/MANIFESTINPUT:{}", manifest.display())];
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    let rc = out.join("manifest.rc");
    let obj = out.join("manifest.o");
    // 1 = CREATEPROCESS_MANIFEST_RESOURCE_ID, 24 = RT_MANIFEST
    std::fs::write(&rc, format!("1 24 \"{}\"\n", manifest.display().to_string().replace('\\', "/"))).expect("write manifest.rc");
    println!("cargo:rerun-if-env-changed=WINDRES");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into());
        let cross = format!("{arch}-w64-mingw32-windres");
        if cfg!(windows) { "windres".into() } else { cross }
    });
    let status = std::process::Command::new(&windres).arg(&rc).arg("-O").arg("coff").arg("-o").arg(&obj).status();
    match status {
        Ok(s) if s.success() => vec![obj.display().to_string()],
        _ => panic!("{windres} could not compile the application manifest (set WINDRES to the MinGW resource compiler)"),
    }
}
