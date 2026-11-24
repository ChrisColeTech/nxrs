//! What is left of the GPU driver's own interfaces once the graphics layer takes over NVN: nvdrv (the NVIDIA
//! driver service, which only the SDK movie library's hardware decoder opens directly; opening a device fails, so
//! the decoder takes its own failure path: its ioctls, mappings and event queries answer NvError 3 from
//! `simple_table.rs`) and the game's own run-time GLSL compiler, whose outputs `NXOS_GLSLC_CAPTURE=<dir>` saves
//! for offline translation (each successful compile's GLSLC output as `<dir>/<FNV-1a of it>.glslc`; the compile
//! itself is the game's).

use crate::guest::{r32, r64, r8};
use crate::hle::Registry;
use nxrt::cpu::Cpu;
use std::path::PathBuf;
use std::sync::OnceLock;

unsafe fn nvos_open(c: *mut Cpu) {
    let p = (*c).x[0];
    let path = if p != 0 {
        crate::guest::cstr(p, 256)
    } else {
        "(null)".into()
    };
    crate::log_once!("nvdrv open", "[RED] nvdrv: cannot open {path} (the host has no nvdrv; only hardware movie decoding uses it)");
    (*c).x[0] = (-1i64) as u64;
}

fn capture_dir() -> Option<&'static PathBuf> {
    static D: OnceLock<Option<PathBuf>> = OnceLock::new();
    D.get_or_init(|| {
        let d = PathBuf::from(std::env::var("NXOS_GLSLC_CAPTURE").ok()?);
        let _ = std::fs::create_dir_all(&d);
        crate::log!(
            "glslc: run-time compile outputs are saved to {}",
            d.display()
        );
        Some(d)
    })
    .as_ref()
}

/// FNV-1a 64 of a GLSLC output: its file name.
fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
        (h ^ x as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// The compile object's first word points at its last results {GLSLCcompilationStatus *, GLSLCoutput *}; the
/// status has "success" at +0xc and "allocation error" at +0xd. The output: magic 0x19866891, its length at +0x44.
fn capture(obj: u64, dir: &std::path::Path) {
    let res = r64(obj);
    if res == 0 {
        return;
    }
    let (status, out) = (r64(res), r64(res + 8));
    if status == 0 || out == 0 || r8(status + 0xc) == 0 || r8(status + 0xd) != 0 {
        return;
    }
    let n = r32(out + 0x44);
    if r32(out) != 0x1986_6891 || !(0x48..=64 << 20).contains(&n) {
        crate::log!(
            "[RED] glslc: unknown output header (magic {:#x} length {n:#x}), not saved",
            r32(out)
        );
        return;
    }
    let data = crate::guest::bytes(out, n as u64);
    let path = dir.join(format!("{:016x}.glslc", fnv1a(data)));
    if path.exists() {
        return;
    }
    match std::fs::write(&path, data) {
        Ok(()) => crate::log!(
            "glslc: saved a run-time compile output {} ({n} bytes)",
            path.display()
        ),
        Err(e) => crate::log!("[RED] glslc: cannot write {}: {e}", path.display()),
    }
}

/// glslcCompile(GLSLCcompileObject *): the game's own compiler, its output captured.
unsafe fn glslc_compile(c: *mut Cpu) {
    let obj = (*c).x[0];
    let Some(orig) = crate::hle::sym("glslcCompile") else {
        nxrt::rt::guest_fatal(c, "glslcCompile is not in the game");
    };
    let Some(ok) = crate::guest::call(c, orig, &[obj]) else {
        return;
    };
    if ok as u8 != 0 {
        if let Some(d) = capture_dir() {
            capture(obj, d);
        }
    }
    (*c).x[0] = ok;
}

pub fn register(r: &mut Registry) {
    r.force(
        "NvOsDrvOpen",
        "nvdrv: the host has none, the open fails",
        nvos_open,
    );
    // Only with a capture folder: otherwise the game's compiler runs untouched.
    if std::env::var_os("NXOS_GLSLC_CAPTURE").is_some() {
        r.force(
            "glslcCompile",
            "the game's run-time GLSL compile, its outputs saved (NXOS_GLSLC_CAPTURE)",
            glslc_compile,
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fnv() {
        assert_eq!(super::fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
