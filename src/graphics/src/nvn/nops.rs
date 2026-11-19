//! Entry points with nothing to do on this host: debug labels and groups, ZCull, tiled-cache hints, fast-clear
//! registration. Each takes the arguments of its prototype and ignores them.

use super::entry::nvn_fns;
use super::types::*;

nvn_fns! {
    fn nvnDeviceSetDebugLabel(d: *mut NVNdevice, s: *const u8) { let _ = (d, s); }
    fn nvnDeviceSetIntermediateShaderCache(d: *mut NVNdevice, n: i32) { let _ = (d, n); }
    fn nvnDeviceApplyDeferredFinalizes(d: *mut NVNdevice, age: i32) { let _ = (d, age); }
    fn nvnDeviceRegisterFastClearColor(d: *mut NVNdevice, c: *const f32, f: i32) { let _ = (d, c, f); }
    fn nvnDeviceRegisterFastClearColori(d: *mut NVNdevice, c: *const i32, f: i32) { let _ = (d, c, f); }
    fn nvnDeviceRegisterFastClearColorui(d: *mut NVNdevice, c: *const u32, f: i32) { let _ = (d, c, f); }
    fn nvnDeviceRegisterFastClearDepth(d: *mut NVNdevice, v: f32) { let _ = (d, v); }
    fn nvnMemoryPoolSetDebugLabel(p: *mut NVNmemoryPool, s: *const u8) { let _ = (p, s); }
    fn nvnCommandBufferPushDebugGroup(c: *mut NVNcommandBuffer, s: *const u8) { let _ = (c, s); }
    fn nvnCommandBufferPushDebugGroupStatic(c: *mut NVNcommandBuffer, id: u32, s: *const u8) { let _ = (c, id, s); }
    fn nvnCommandBufferPushDebugGroupDynamic(c: *mut NVNcommandBuffer, id: u32, s: *const u8) { let _ = (c, id, s); }
    fn nvnCommandBufferPopDebugGroup(c: *mut NVNcommandBuffer) { let _ = c; }
    fn nvnCommandBufferPopDebugGroupId(c: *mut NVNcommandBuffer, id: u32) { let _ = (c, id); }
    fn nvnCommandBufferInsertDebugMarker(c: *mut NVNcommandBuffer, s: *const u8) { let _ = (c, s); }
    fn nvnCommandBufferInsertDebugMarkerStatic(c: *mut NVNcommandBuffer, id: u32, s: *const u8) { let _ = (c, id, s); }
    fn nvnCommandBufferInsertDebugMarkerDynamic(c: *mut NVNcommandBuffer, id: u32, s: *const u8) { let _ = (c, id, s); }
    fn nvnCommandBufferSaveZCullData(c: *mut NVNcommandBuffer, a: u64, n: usize) { let _ = (c, a, n); }
    fn nvnCommandBufferRestoreZCullData(c: *mut NVNcommandBuffer, a: u64, n: usize) { let _ = (c, a, n); }
    fn nvnCommandBufferSetStencilCullCriteria(c: *mut NVNcommandBuffer, f: i32, r: i32) { let _ = (c, f, r); }
    fn nvnCommandBufferSetTiledCacheAction(c: *mut NVNcommandBuffer, a: i32) { let _ = (c, a); }
    fn nvnCommandBufferSetTiledCacheTileSize(c: *mut NVNcommandBuffer, w: i32, h: i32) { let _ = (c, w, h); }
    fn nvnCommandBufferDiscardColor(c: *mut NVNcommandBuffer, i: i32) { let _ = (c, i); }
    fn nvnCommandBufferDiscardDepthStencil(c: *mut NVNcommandBuffer) { let _ = c; }
    fn nvnCommandBufferInvalidateTextures(c: *mut NVNcommandBuffer, st: i32) { let _ = (c, st); }
}
