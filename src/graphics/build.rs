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

fn compile_dir(src: &Path, out: &Path, tool: &Path) {
    println!("cargo:rerun-if-changed={}", src.display());
    let Ok(rd) = std::fs::read_dir(src) else { return };
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
        let dst = out.join(format!("{name}.spv"));
        let st = Command::new(tool)
            .args(["-V", "--target-env", "vulkan1.3", "-o"])
            .arg(&dst)
            .arg(&p)
            .output()
            .unwrap_or_else(|e| panic!("cannot run {} ({e}): install the Vulkan SDK or set NXGFX_GLSLANG", tool.display()));
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

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let tool = glslang();
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    println!("cargo:rerun-if-env-changed=NXGFX_GLSLANG");
    compile_dir(Path::new("shaders"), &out.join("spv"), &tool);
    compile_dir(Path::new("tests/shaders"), &out.join("test_spv"), &tool);
}
