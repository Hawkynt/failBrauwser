include!("winpack/iconresource.rs");

fn main() {
    // Embed the application manifest (longPathAware, asInvoker, Win10 OS GUID) into
    // every Windows binary the package links — engines and integration tests alike.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let manifest = manifest_dir().join("winpack/fb.manifest");
    println!("cargo:rerun-if-changed=winpack/fb.manifest");
    for flag in ["/MANIFEST:EMBED", &format!("/MANIFESTINPUT:{}", manifest.display())] {
        println!("cargo:rustc-link-arg={flag}");
        println!("cargo:rustc-link-arg-tests={flag}");
    }
    link_icon("data/icons/failbrauwser.rc");
}
