//! WEB-W8b: the web build's FP helpers under FPCR.FZ (fast paths, relaxed-madd FFMA) give the slow paths' and libm
//! fma's results bit for bit (tests/web_fpcheck.c), built with the web game code's flags and run under emsdk's Node.
//! Opt-in (an emcc build and a few seconds of Node): `cargo test --release -p nxrs-cli --test web_fp -- --ignored`;
//! the SDK is `NXRS_EMSDK`, else the tree's `build\tools\emsdk`.
use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore]
fn web_fz_fast_paths_and_relaxed_fma_match_the_slow_paths() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let emsdk = std::env::var_os("NXRS_EMSDK")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("build").join("tools").join("emsdk"));
    let py_dir = std::fs::read_dir(emsdk.join("python"))
        .expect("emsdk python")
        .flatten()
        .map(|e| e.path())
        .next()
        .expect("emsdk python folder");
    let node = std::fs::read_dir(emsdk.join("node"))
        .expect("emsdk node")
        .flatten()
        .map(|e| e.path().join("node.exe"))
        .next()
        .expect("emsdk node folder");
    let out = std::env::temp_dir().join(format!("nxrs-web-fp-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let js = out.join("fpcheck.js");
    let mut path = vec![py_dir.clone()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let st = Command::new(py_dir.join("python.exe"))
        .arg("-E")
        .arg(emsdk.join("upstream").join("emscripten").join("emcc.py"))
        .env("EM_CONFIG", emsdk.join(".emscripten"))
        // nxrs-build's own Emscripten cache (target\emcache, cc.rs em_cache)
        .env("EM_CACHE", repo.join("target").join("emcache"))
        .env("PATH", std::env::join_paths(path).unwrap())
        .args(["-O2", "-std=gnu11", "-fno-strict-aliasing", "-w"])
        .args(["-sMEMORY64=1", "-msimd128", "-mrelaxed-simd"])
        .arg(format!(
            "-I{}",
            repo.join("src")
                .join("translator")
                .join("runtime")
                .display()
        ))
        .arg(
            repo.join("src")
                .join("cli")
                .join("tests")
                .join("web_fpcheck.c"),
        )
        .arg("-o")
        .arg(&js)
        .status()
        .expect("emcc");
    assert!(st.success(), "emcc failed");
    let o = Command::new(node).arg(&js).output().expect("node");
    let text = String::from_utf8_lossy(&o.stdout).to_string();
    let _ = std::fs::remove_dir_all(&out);
    assert!(o.status.success(), "{text}");
    assert!(
        text.contains(", 0 differ; FFMA") && text.trim_end().ends_with(" 0 differ"),
        "{text}"
    );
}
