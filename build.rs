include!("winpack/iconresource.rs");

fn main() {
    // What this script reads, said on every system: without a rerun-if-changed line cargo runs
    // it again, and builds the program again, whenever any file of the folder changes (the
    // archive helper's build output, a log). The Windows lines below add theirs.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=winpack/iconresource.rs");
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
