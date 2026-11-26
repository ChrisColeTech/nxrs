//! Links the codec libraries into nxos.
//!
//! Windows: third_party/ (libopus, libvpx's VP8 and VP9 decoders, dav1d; static x86_64 archives built from source with
//! MinGW-w64; provenance and licenses in the README's "Credits and licenses"). rustc bundles them into the staticlib;
//! the game's link adds legacy_stdio_definitions.
//!
//! Linux: those archives are PE objects, so the system's libopus, libvpx and libdav1d are linked instead (the same
//! upstream libraries; pkg-config), and src/media/ffdec.c, the H.264 / AAC decoder over FFmpeg's libavcodec (what Media
//! Foundation does on Windows), is compiled against the system headers. All of them are linked dynamically: the game
//! link adds `-lopus -lvpx -ldav1d -lavcodec -lavutil`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        linux();
        return;
    }
    // the web (docs/35): the archives here are x86-64 objects; wasm builds of libopus, libvpx and dav1d come with the
    // web build's audio and video (docs/35 W3). Until then their functions are imports of the module, which stop the
    // game with the function's name if a codec is opened.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("emscripten") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let here =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("third_party");
    for (dir, lib, name) in [
        ("opus", "libopus.a", "nxopus"),
        ("vpx", "libvpx.a", "nxvpx"),
        ("dav1d", "libdav1d.a", "nxdav1d"),
    ] {
        let src = here.join(dir).join(lib);
        println!("cargo:rerun-if-changed={}", src.display());
        // MSVC looks for {name}.lib, the gnullvm toolchain for lib{name}.a
        for dst in [
            out.join(format!("{name}.lib")),
            out.join(format!("lib{name}.a")),
        ] {
            std::fs::copy(&src, &dst).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
        }
        println!("cargo:rustc-link-lib=static={name}");
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    // fprintf / fscanf / vsnprintf as functions (the MinGW objects call them; the UCRT has them inline)
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-lib=dylib=legacy_stdio_definitions");
    }
}

fn linux() {
    let need = |name: &str, what: &str| {
        pkg_config::Config::new()
            .cargo_metadata(true)
            .probe(name)
            .unwrap_or_else(|e| panic!("{what} ({name}) not found: install its development package (Debian / Ubuntu: lib{name}-dev; FFmpeg: libavcodec-dev libavutil-dev): {e}"))
    };
    need("opus", "libopus");
    need("vpx", "libvpx");
    need("dav1d", "dav1d");
    let avcodec = need("libavcodec", "FFmpeg's libavcodec");
    let avutil = need("libavutil", "FFmpeg's libavutil");
    let src = "src/media/ffdec.c";
    println!("cargo:rerun-if-changed={src}");
    let mut b = cc::Build::new();
    b.file(src).opt_level(2).warnings(true).pic(true);
    for p in avcodec
        .include_paths
        .iter()
        .chain(avutil.include_paths.iter())
    {
        b.include(p);
    }
    b.compile("nxffdec");
}
