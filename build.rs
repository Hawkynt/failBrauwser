fn main() {
    // Embed the application manifest (longPathAware, asInvoker, Win10 OS GUID) into
    // every Windows binary the package links — engines and integration tests alike.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo")).join("winpack/fb.manifest");
    println!("cargo:rerun-if-changed=winpack/fb.manifest");
    for flag in ["/MANIFEST:EMBED", &format!("/MANIFESTINPUT:{}", manifest.display())] {
        println!("cargo:rustc-link-arg={flag}");
        println!("cargo:rustc-link-arg-tests={flag}");
    }
}
