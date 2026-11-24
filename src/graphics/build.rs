//! Compiles the crate's GLSL (shaders/: the engine's own compute and blit shaders; tests/shaders/: the test programs) to
//! SPIR-V in OUT_DIR with glslangValidator from the Vulkan SDK (VULKAN_SDK, else PATH).

use std::path::{Path, PathBuf};
use std::process::Command;

fn glslang() -> PathBuf {
    if let Ok(p) = std::env::var("NXGFX_GLSLANG") {
        return PathBuf::from(p);
    }
    if let Ok(sdk) = std::env::var("VULKAN_SDK") {
        let p = Path::new(&sdk).join("Bin").join("glslangValidator.exe");
        if p.exists() {
            return p;
        }
        let p = Path::new(&sdk).join("bin").join("glslangValidator");
        if p.exists() {
            return p;
        }
    }
    let p = Path::new(r"C:\VulkanSDK\1.3.268.0\Bin\glslangValidator.exe");
    if p.exists() {
        return p.to_path_buf();
    }
    PathBuf::from("glslangValidator")
}

/// A shader that names variants in a comment line `// variants: NAME=DEFINE ...` is also compiled once per variant
/// with `-DDEFINE`, into `<file>_<NAME>.spv` (the BC7 encoder's mode-6-only build).
fn variants(p: &Path) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(p) else {
        return vec![];
    };
    text.lines()
        .take(40)
        .filter_map(|l| l.trim().strip_prefix("// variants:"))
        .flat_map(|v| {
            v.split_whitespace()
                .filter_map(|kv| kv.split_once('='))
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn compile_dir(src: &Path, out: &Path, tool: &Path) {
    println!("cargo:rerun-if-changed={}", src.display());
    let Ok(rd) = std::fs::read_dir(src) else {
        return;
    };
    std::fs::create_dir_all(out).unwrap();
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "comp" | "vert" | "frag" | "geom" | "tesc" | "tese") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", p.display());
        let name = p.file_name().unwrap().to_str().unwrap().replace('.', "_");
        let mut builds = vec![(format!("{name}.spv"), None)];
        for (v, def) in variants(&p) {
            builds.push((format!("{name}_{v}.spv"), Some(def)));
        }
        for (file, def) in builds {
            let mut cmd = Command::new(tool);
            cmd.args(["-V", "--target-env", "vulkan1.3"]);
            if let Some(d) = def {
                cmd.arg(format!("-D{d}"));
            }
            let st = cmd
                .arg("-o")
                .arg(out.join(file))
                .arg(&p)
                .output()
                .unwrap_or_else(|e| {
                    panic!(
                        "cannot run {} ({e}): install the Vulkan SDK or set NXGFX_GLSLANG",
                        tool.display()
                    )
                });
            if !st.status.success() {
                panic!(
                    "glslang failed on {}:\n{}{}",
                    p.display(),
                    String::from_utf8_lossy(&st.stdout),
                    String::from_utf8_lossy(&st.stderr)
                );
            }
        }
    }
}

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let tool = glslang();
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    println!("cargo:rerun-if-env-changed=NXGFX_GLSLANG");
    compile_dir(Path::new("shaders"), &out.join("spv"), &tool);
    compile_dir(Path::new("tests/shaders"), &out.join("test_spv"), &tool);
}
