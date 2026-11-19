//! Links the codec libraries (third_party/: libopus, libvpx's VP8 and VP9 decoders, dav1d; static x86_64 archives
//! built from source with MinGW-w64; provenance and licenses in the README's "Credits and licenses") into nxos.
//! rustc bundles them into the staticlib; the game's link adds legacy_stdio_definitions.

fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let here = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("third_party");
    for (dir, lib, name) in [("opus", "libopus.a", "nxopus"), ("vpx", "libvpx.a", "nxvpx"), ("dav1d", "libdav1d.a", "nxdav1d")] {
        let src = here.join(dir).join(lib);
        println!("cargo:rerun-if-changed={}", src.display());
        // MSVC looks for {name}.lib, the gnullvm toolchain for lib{name}.a
        for dst in [out.join(format!("{name}.lib")), out.join(format!("lib{name}.a"))] {
            std::fs::copy(&src, &dst).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
        }
        println!("cargo:rustc-link-lib=static={name}");
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    // fprintf / fscanf / vsnprintf as functions (the MinGW objects call them; the UCRT has them inline)
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-lib=dylib=legacy_stdio_definitions");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
