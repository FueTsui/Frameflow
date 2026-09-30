fn main() {
    println!("cargo:rerun-if-changed=assets/Frameflow.ico");
    println!("cargo:rerun-if-changed=scripts/windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = std::env::var("CARGO_PKG_VERSION").expect("package version");
        let target = std::env::var("TARGET").expect("build target");
        let profile = std::env::var("PROFILE").expect("build profile");
        let rustc = std::env::var("RUSTC").expect("Rust compiler");
        let compiler = std::process::Command::new(rustc)
            .arg("--version")
            .output()
            .expect("read Rust compiler version");
        assert!(
            compiler.status.success(),
            "cannot read Rust compiler version"
        );
        let compiler = String::from_utf8_lossy(&compiler.stdout);
        let provenance = format!("{}; target={target}; profile={profile}", compiler.trim());
        winresource::WindowsResource::new()
            .set_icon("assets/Frameflow.ico")
            .set_manifest_file("scripts/windows.manifest")
            .set("ProductName", "Frameflow")
            .set("FileDescription", "帧流 Frameflow")
            .set("FileVersion", &version)
            .set("ProductVersion", &version)
            .set("OriginalFilename", "frameflow.exe")
            .set("InternalName", "frameflow")
            .set("Comments", &provenance)
            .compile()
            .expect("failed to embed Frameflow icon");
    }
}
