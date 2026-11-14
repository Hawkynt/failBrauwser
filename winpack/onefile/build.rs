include!("../iconresource.rs"); // the icon wiring the program crate shares (winpack\iconresource.rs)

fn main() {
    // The payload and its version come from winpack\package.ps1: the staging zip is
    // copied where src/portable.rs can include_bytes! it, and the version becomes a
    // compile-time env so the stamp comparison sees the real release number.
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo"));
    let payload = std::env::var_os("FB_PAYLOAD").map(std::path::PathBuf::from).filter(|p| p.is_file()).unwrap_or_else(|| panic!("FB_PAYLOAD must name the staging zip - winpack\\package.ps1 builds winpack\\out\\failbrauwser-<version>-x86_64-windows.zip"));
    std::fs::copy(&payload, out.join("payload.zip")).expect("could not stage the payload zip into OUT_DIR");
    println!("cargo:rerun-if-changed={}", payload.display());
    println!("cargo:rerun-if-env-changed=FB_PAYLOAD");
    if let Some(v) = std::env::var_os("FB_VERSION") {
        println!("cargo:rustc-env=FB_VERSION={}", v.to_string_lossy());
        println!("cargo:rerun-if-env-changed=FB_VERSION");
    }
    // The payload's build id (package.ps1's packaging time, the same string the tree carries as
    // payload-id.txt): a rebuild of the same version has to be a rebuild of this crate too, so
    // the comparison rule reruns on it whether or not it is set.
    println!("cargo:rerun-if-env-changed=FB_BUILD_ID");
    if let Some(id) = std::env::var_os("FB_BUILD_ID") {
        println!("cargo:rustc-env=FB_BUILD_ID={}", id.to_string_lossy());
    }

    // The same manifest as the program itself - longPathAware matters while unpacking
    // the deep icon and locale trees below the install prefix.
    // rustc's working directory is not this crate's folder, so the manifest has to be
    // named absolutely.
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo")).join("../fb.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    for flag in ["/MANIFEST:EMBED", &format!("/MANIFESTINPUT:{}", manifest.display())] {
        println!("cargo:rustc-link-arg={flag}");
    }

    // The launcher is the file Explorer shows and the file the user starts from, so it carries
    // the same icon as the program inside it.
    link_icon("../../data/icons/failbrauwser.rc");
}
