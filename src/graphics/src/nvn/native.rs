//! Native images (NVN2): a decoded video picture as a texture (nn::gs::NativeImageHandle → nvnNativeImage →
//! nvnTextureInitializeFromNativeImage). These are not in the proc table: the game imports them by name from its sdk
//! module, so the OS layer binds `takeovers()` by mangled name. The handle names a picture of a video decoder's
//! image pool, which keeps it as RGBA in memory (`set_source`, the OS layer's decoders); the texture is a linear
//! texture over a memory pool of that memory, so each new picture is read with no copy of the host's.

use super::entry::HostFn;
use super::types::*;
use std::sync::OnceLock;

/// A native image handle → (memory, size, width, height, row stride) of its picture.
pub type Source = fn(handle: u32) -> Option<(u64, u64, u32, u32, u32)>;
static SOURCE: OnceLock<Source> = OnceLock::new();
pub fn set_source(f: Source) {
    let _ = SOURCE.set(f);
}

#[repr(C)]
struct Builder {
    dev: *mut NVNdevice,
    handle: u32,
    format: i32,
}
#[repr(C)]
struct Image {
    dev: *mut NVNdevice,
    handle: u32,
    format: i32,
    has_pool: u32,
    pad: u32,
    pool: [u8; 0x100],
}
const _: () = assert!(core::mem::size_of::<Image>() <= 0x200);

super::entry::nvn_fns! {
    fn nvnNativeImageBuilderSetDefaults(b: *mut u8) { let o = &mut *(b as *mut Builder); let d = o.dev; *o = Builder { dev: d, handle: 0, format: 0 }; }
    fn nvnNativeImageBuilderSetDevice(b: *mut u8, d: *mut NVNdevice) { (*(b as *mut Builder)).dev = d; }
    fn nvnNativeImageBuilderSetNativeImageHandle(b: *mut u8, h: u32) { (*(b as *mut Builder)).handle = h; }
    fn nvnNativeImageBuilderSetMemoryPermission(b: *mut u8, p: i32) { let _ = (b, p); }
    fn nvnNativeImageBuilderSetFormat(b: *mut u8, f: i32) { (*(b as *mut Builder)).format = f; }
    fn nvnNativeImageInitialize(i: *mut u8, b: *const u8) -> NVNboolean {
        let (o, s) = (&mut *(i as *mut Image), &*(b as *const Builder));
        o.dev = s.dev;
        o.handle = s.handle;
        o.format = s.format;
        o.has_pool = 0;
        1
    }
    fn nvnNativeImageFinalize(i: *mut u8) {
        let o = &mut *(i as *mut Image);
        if o.has_pool != 0 {
            super::pool::nvnMemoryPoolFinalize(o.pool.as_mut_ptr() as *mut NVNmemoryPool);
        }
        o.has_pool = 0;
    }
    fn nvnTextureInitializeFromNativeImage(t: *mut NVNtexture, i: *mut u8) -> NVNboolean {
        let o = &mut *(i as *mut Image);
        let Some((mem, size, w, h, stride)) = SOURCE.get().and_then(|f| f(o.handle)) else {
            crate::log::red(&format!("nvnTextureInitializeFromNativeImage: handle {:#x} names no picture", o.handle));
            return 0;
        };
        let pool = o.pool.as_mut_ptr() as *mut NVNmemoryPool;
        if o.has_pool == 0 {
            let mut pb = NVNmemoryPoolBuilder::boxed();
            let pbp = pb.0.as_mut_ptr() as *mut NVNmemoryPoolBuilder;
            super::pool::nvnMemoryPoolBuilderSetDefaults(pbp);
            super::pool::nvnMemoryPoolBuilderSetDevice(pbp, o.dev);
            super::pool::nvnMemoryPoolBuilderSetStorage(pbp, mem as *mut u8, size as usize);
            super::pool::nvnMemoryPoolBuilderSetFlags(pbp, (mem_flags::CPU_UNCACHED | mem_flags::GPU_CACHED) as i32);
            if super::pool::nvnMemoryPoolInitialize(pool, pbp) == 0 {
                return 0;
            }
            o.has_pool = 1;
        }
        let mut tbx = NVNtextureBuilder::boxed();
        let tbp = tbx.0.as_mut_ptr() as *mut NVNtextureBuilder;
        super::tex::nvnTextureBuilderSetDefaults(tbp);
        super::tex::nvnTextureBuilderSetDevice(tbp, o.dev);
        super::tex::nvnTextureBuilderSetTarget(tbp, target::T2D);
        super::tex::nvnTextureBuilderSetFormat(tbp, o.format);
        super::tex::nvnTextureBuilderSetSize2D(tbp, w as i32, h as i32);
        super::tex::nvnTextureBuilderSetLevels(tbp, 1);
        super::tex::nvnTextureBuilderSetFlags(tbp, tex_flags::LINEAR as i32);
        super::tex::nvnTextureBuilderSetStride(tbp, stride as isize);
        super::tex::nvnTextureBuilderSetStorage(tbp, pool, 0);
        super::tex::nvnTextureInitialize(t, tbp)
    }
}

/// The sdk symbols to take over, by mangled name.
pub fn takeovers() -> Vec<(&'static str, HostFn)> {
    let mut v = Vec::new();
    entries(&mut v);
    let mangled = |n: &str| -> &'static str {
        match n {
            "nvnNativeImageBuilderSetDefaults" => "_Z32nvnNativeImageBuilderSetDefaultsP21NVNnativeImageBuilder",
            "nvnNativeImageBuilderSetDevice" => "_Z30nvnNativeImageBuilderSetDeviceP21NVNnativeImageBuilderP9NVNdevice",
            "nvnNativeImageBuilderSetNativeImageHandle" => "_Z41nvnNativeImageBuilderSetNativeImageHandleP21NVNnativeImageBuilderN2nn2gs17NativeImageHandleE",
            "nvnNativeImageBuilderSetMemoryPermission" => "_Z40nvnNativeImageBuilderSetMemoryPermissionP21NVNnativeImageBuilder30NVNnativeImageMemoryPermission",
            "nvnNativeImageBuilderSetFormat" => "_Z30nvnNativeImageBuilderSetFormatP21NVNnativeImageBuilder9NVNformat",
            "nvnNativeImageInitialize" => "_Z24nvnNativeImageInitializeP14NVNnativeImagePK21NVNnativeImageBuilder",
            "nvnNativeImageFinalize" => "_Z22nvnNativeImageFinalizeP14NVNnativeImage",
            _ => "_Z35nvnTextureInitializeFromNativeImageP10NVNtextureP14NVNnativeImage",
        }
    };
    v.into_iter().map(|e| (mangled(e.name), e.f)).collect()
}
