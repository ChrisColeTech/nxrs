fn main() {
    println!("cargo:rerun-if-changed=VERSION");
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/nxrs.manifest");
    println!("cargo:rerun-if-env-changed=NXRS_VERSION");
    let version = std::env::var("NXRS_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::fs::read_to_string("VERSION").ok())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "0.0.0".into());
    println!("cargo:rustc-env=NXRS_VERSION={version}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // only the nxrs binary carries the icon and manifest (the test helper does not need them)
        embed_resource::compile_for("assets/app.rc", ["nxrs"], embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
