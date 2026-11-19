//! NVN2 programs: the program builder and nvnProgramInitializeFromBuilder, which builds a program exactly as
//! nvnProgramSetShaders does (`prog`); SetShadersExt; sample shading and subroutine linkage; the per-program queries.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// What NVN2 reads from a program's control sections at SetShaders and answers later (the game may free the control
/// sections once SetShaders returns): the device-side compute memory of a compute stage, by program serial.
static DEVICE_COMPUTE: Mutex<Option<HashMap<u64, i32>>> = Mutex::new(None);

/// The memory a device-side compute launch of this stage needs: an NVN2 compute stage whose control +0xa8 is 1 needs
/// (control +0xba + 0x22f) & 0x1ff00 bytes; any other stage 0.
/// # Safety
/// `ctl` is null or a readable control section.
pub unsafe fn device_compute_size(ctl: *const u8) -> i32 {
    if ctl.is_null() {
        return 0;
    }
    let rd32 = |o: usize| core::ptr::read_unaligned(ctl.add(o) as *const u32);
    if rd32(0) != 0x9876_1234 || rd32(4) != 0xc9 {
        return 0;
    }
    if *ctl.add(0x56) as u32 != stage::COMPUTE || *ctl.add(0xa8) != 1 {
        return 0;
    }
    let m = core::ptr::read_unaligned(ctl.add(0xba) as *const u16) as i32;
    (m + 0x22f) & 0x1ff00
}

/// Called by nvnProgramSetShaders with the program's new serial and stages.
/// # Safety
/// The stages' control sections are readable.
pub unsafe fn note_program(serial: u64, sd: &[NVNshaderData]) {
    let n = sd.iter().map(|s| device_compute_size(s.control)).max().unwrap_or(0);
    if n != 0 {
        DEVICE_COMPUTE.lock().unwrap().get_or_insert_with(HashMap::new).insert(serial, n);
    }
}

unsafe fn pb<'a>(b: *const NVNprogramBuilder) -> &'a mut prog_builder_t {
    &mut *(b as *mut prog_builder_t)
}

/// NVN2's extended shader record: {code, control, debug data hash} (NVN 55: an 8-byte extension field).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ShaderDataExt {
    pub data: u64,
    pub control: *const u8,
    pub ext: u64,
}

nvn_fns! {
    fn nvnProgramBuilderSetDefaults(b: *mut NVNprogramBuilder) {
        let dev = pb(b).dev;
        core::ptr::write_bytes(b as *mut u8, 0, core::mem::size_of::<prog_builder_t>());
        let o = pb(b);
        o.dev = dev;
        o.max_trace = 2;
        o.max_callable = 2;
    }
    fn nvnProgramBuilderSetDevice(b: *mut NVNprogramBuilder, d: *mut NVNdevice) { pb(b).dev = d; }
    fn nvnProgramBuilderGetDevice(b: *const NVNprogramBuilder) -> *mut NVNdevice { pb(b).dev }
    fn nvnProgramBuilderSetSampleShading(b: *mut NVNprogramBuilder, m: i32) { pb(b).sample_shading = m; }
    fn nvnProgramBuilderGetSampleShading(b: *const NVNprogramBuilder) -> i32 { pb(b).sample_shading }
    fn nvnProgramBuilderSetDebugDataHashes(b: *mut NVNprogramBuilder, n: i32, h: *const u8) { pb(b).nhash = n; pb(b).hash = h; }
    fn nvnProgramBuilderGetDebugDataHashes(b: *const NVNprogramBuilder) -> *const u8 { pb(b).hash }
    /// The records are not copied (nor by NVN): they must live until InitializeFromBuilder.
    fn nvnProgramBuilderSetShaders(b: *mut NVNprogramBuilder, n: i32, sd: *const NVNshaderData) { pb(b).nshader = n; pb(b).shader = sd; }
    fn nvnProgramBuilderGetShaders(b: *const NVNprogramBuilder) -> *const NVNshaderData { pb(b).shader }
    fn nvnProgramBuilderGetShaderCount(b: *const NVNprogramBuilder) -> i32 { pb(b).nshader }
    fn nvnProgramBuilderSetRayTracingModules(b: *mut NVNprogramBuilder, n: i32, sd: *const NVNshaderData) { pb(b).nrt = n; pb(b).rt = sd; }
    fn nvnProgramBuilderGetRayTracingModuleCount(b: *const NVNprogramBuilder) -> i32 { pb(b).nrt }
    fn nvnProgramBuilderGetRayTracingModuleData(b: *const NVNprogramBuilder) -> *const NVNshaderData { pb(b).rt }
    fn nvnProgramBuilderSetSubroutineLinkage(b: *mut NVNprogramBuilder, n: i32, maps: *const u64) { pb(b).nlink = n; pb(b).link = maps; }
    fn nvnProgramBuilderGetSubroutineLinkage(b: *const NVNprogramBuilder) -> *const u64 { pb(b).link }
    fn nvnProgramBuilderGetSubroutineLinkageCount(b: *const NVNprogramBuilder) -> i32 { pb(b).nlink }
    fn nvnProgramBuilderSetMaxTraceRecursionDepth(b: *mut NVNprogramBuilder, v: i32) { pb(b).max_trace = v; }
    fn nvnProgramBuilderGetMaxTraceRecursionDepth(b: *const NVNprogramBuilder) -> i32 { pb(b).max_trace }
    fn nvnProgramBuilderSetMaxCallableRecursionDepth(b: *mut NVNprogramBuilder, v: i32) { pb(b).max_callable = v; }
    fn nvnProgramBuilderGetMaxCallableRecursionDepth(b: *const NVNprogramBuilder) -> i32 { pb(b).max_callable }
    fn nvnProgramBuilderSetProgramRayTracingFlags(b: *mut NVNprogramBuilder, f: u32) { pb(b).rt_flags = f; }
    fn nvnProgramBuilderGetProgramRayTracingFlags(b: *const NVNprogramBuilder) -> u32 { pb(b).rt_flags }

    /// Initialize, then the builder's stages through SetShaders (the same keys, live translation and replay as a
    /// program set up with nvnProgramSetShaders), then its subroutine linkage. Ray-tracing modules need ray-tracing
    /// pipelines, which the back end does not have: such a program is refused (false), as NVN refuses one it cannot link.
    fn nvnProgramInitializeFromBuilder(p: *mut NVNprogram, b: *const NVNprogramBuilder) -> NVNboolean {
        let o = &*(b as *const prog_builder_t);
        if !super::b(super::prog::nvnProgramInitialize(p, o.dev) as u32) { return 0; }
        if o.nshader < 0 || o.nrt < 0 || (o.nshader > 0 && o.shader.is_null()) || (o.nrt > 0 && o.rt.is_null()) || (o.nshader > 0 && o.nrt > 0) {
            crate::log::red(&format!("ProgramInitializeFromBuilder: invalid or mixed builder ({} shaders, {} ray-tracing modules)", o.nshader, o.nrt));
            return 0;
        }
        if o.nrt > 0 {
            crate::log::red_once(&format!("ProgramInitializeFromBuilder: {} ray-tracing modules: the back end has no ray-tracing pipelines; the program is refused", o.nrt));
            return 0;
        }
        if o.sample_shading != 0 { nvnProgramSetSampleShading(p, o.sample_shading); }
        if o.nshader > 0 && !super::b(super::prog::nvnProgramSetShaders(p, o.nshader, o.shader) as u32) { return 0; }
        if o.nlink > 0 && !super::b(nvnProgramSetSubroutineLinkage(p, o.nlink, o.link) as u32) { return 0; }
        1
    }
    /// {code, control, extension} records: the extension (NVN2's debug data hash) does not take part in a stage's key.
    fn nvnProgramSetShadersExt(p: *mut NVNprogram, n: i32, sd: *const ShaderDataExt) -> NVNboolean {
        if !(0..=9).contains(&n) || (n > 0 && sd.is_null()) {
            crate::log::red(&format!("ProgramSetShadersExt stage count {n}"));
            return 0;
        }
        if n > 6 {
            crate::log::red(&format!("ProgramSetShadersExt: {n} stages; a program holds at most 6"));
            return 0;
        }
        let mut plain = [NVNshaderData::default(); 6];
        for i in 0..n as usize {
            let e = *sd.add(i);
            plain[i] = NVNshaderData { data: e.data, control: e.control };
        }
        super::prog::nvnProgramSetShaders(p, n, plain.as_ptr())
    }
    /// Per-sample fragment shading. Vulkan shades per sample where the translated shader reads the sample index or
    /// interpolates at a sample, as NVN does in its default mode; an explicit mode is not applied.
    fn nvnProgramSetSampleShading(p: *mut NVNprogram, m: i32) -> NVNboolean {
        let _ = p;
        if m != 0 { crate::log::red_once(&format!("ProgramSetSampleShading mode {m}: not applied; shading per sample follows the shader")); }
        1
    }
    /// Subroutine linkage maps (one per stage that uses subroutines; the stage is the map's int at +4): checked as NVN
    /// does. The translated shaders take no subroutine tables.
    fn nvnProgramSetSubroutineLinkage(p: *mut NVNprogram, n: i32, maps: *const u64) -> NVNboolean {
        let _ = p;
        if n < 0 || (n > 0 && maps.is_null()) { return 0; }
        for i in 0..n as usize {
            let m = *maps.add(i) as *const u8;
            if m.is_null() || core::ptr::read_unaligned(m.add(4) as *const i32) as u32 > stage::COMPUTE { return 0; }
        }
        if n > 0 { crate::log::red_once(&format!("ProgramSetSubroutineLinkage: {n} linkage maps: shader subroutines are not translated")); }
        1
    }
    /// The program's byte +0, which every SetShaders clears and nothing sets.
    fn nvnProgramGetSubtileSize(p: *const NVNprogram) -> u8 { let _ = p; 0 }
    fn nvnProgramGetDeviceComputeMemorySize(p: *const NVNprogram) -> i32 {
        let s = (*(p as *const prog_t)).serial;
        DEVICE_COMPUTE.lock().unwrap().as_ref().and_then(|m| m.get(&s).copied()).unwrap_or(0)
    }
    /// Ray-tracing scratch per warp: 0 for a program with no ray-tracing pipeline (every program here).
    fn nvnProgramGetScratchBytesPerWarp(p: *const NVNprogram) -> i32 { let _ = p; 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults_and_device_compute_size() {
        let mut b = NVNprogramBuilder::boxed();
        let p = &mut *b as *mut NVNprogramBuilder;
        unsafe {
            nvnProgramBuilderSetDevice(p, 0x1000 as *mut NVNdevice);
            nvnProgramBuilderSetMaxTraceRecursionDepth(p, 7);
            nvnProgramBuilderSetDefaults(p);
            assert_eq!(nvnProgramBuilderGetDevice(p) as usize, 0x1000);
            assert_eq!((nvnProgramBuilderGetMaxTraceRecursionDepth(p), nvnProgramBuilderGetMaxCallableRecursionDepth(p)), (2, 2));
            assert_eq!((nvnProgramBuilderGetShaderCount(p), nvnProgramBuilderGetRayTracingModuleCount(p)), (0, 0));
        }
        let mut ctl = vec![0u8; 0xd8];
        ctl[0..4].copy_from_slice(&0x9876_1234u32.to_le_bytes());
        ctl[4..8].copy_from_slice(&0xc9u32.to_le_bytes());
        ctl[0x56] = stage::COMPUTE as u8;
        ctl[0xa8] = 1;
        ctl[0xba..0xbc].copy_from_slice(&0x40u16.to_le_bytes());
        assert_eq!(unsafe { device_compute_size(ctl.as_ptr()) }, (0x40 + 0x22f) & 0x1ff00);
        ctl[0xa8] = 0;
        assert_eq!(unsafe { device_compute_size(ctl.as_ptr()) }, 0);
    }

    #[test]
    fn ray_tracing_builder_is_refused() {
        let mut b = NVNprogramBuilder::boxed();
        let mut prog = NVNprogram::boxed();
        let sd = NVNshaderData::default();
        unsafe {
            nvnProgramBuilderSetDefaults(&mut *b);
            nvnProgramBuilderSetRayTracingModules(&mut *b, 1, &sd);
            assert_eq!(nvnProgramInitializeFromBuilder(&mut *prog, &*b), 0);
            // an empty builder makes an empty program
            nvnProgramBuilderSetRayTracingModules(&mut *b, 0, core::ptr::null());
            assert_eq!(nvnProgramInitializeFromBuilder(&mut *prog, &*b), 1);
            assert_eq!((*(prog.0.as_ptr() as *const prog_t)).magic, MAGIC_PROG);
        }
    }
}
