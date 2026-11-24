//! Mario Kart World 1.8.0's zstd decoder on the host's libzstd (mkw-recomp `runtime/sdk/ov/ov_zstd.c`, MIT; the
//! profile's `zstd` set, `NXRS_PATCH_MKW_ZSTD`, on by default). Most RomFS files are zstd frames, many with a
//! dictionary; the decoder is BSD zstd linked into main. Lifted, it ran its literal and match copies as chains of
//! small functions (mkw-recomp: 18.5 % of the CPU); here they decode in native code.
//!
//! - ZSTD_decompressMultiFrame (main+0xe2280) is called by main+0xe1c60 and main+0xf0a10: both `bl`s go to
//!   patch page + 0xff0. Every result that is an error goes to the guest decoder again (it writes its own error code);
//!   so does an input inside its own output (in place: the result depends on how the decoder's writes overtake its
//!   reads).
//! - The loads' stream main+0xdeca0 (stream, src, size, ddict) -> int feeds a frame in chunks as they are read; its
//!   only call (main+0xdec5c) goes to patch page + 0xfe0, which mirrors it on a host ZSTD_DCtx per stream object.
//!   Stream object: u8 state (1 new frame, 2 decoding, 3 failed, 4 done), +0x08 dst, +0x10 i32 dst room, +0x14 i32
//!   content bytes left (the result), +0x18 guest DCtx memory, +0x20 staging, +0x28 i32 staging size, +0x2c i32 staged
//!   bytes, +0x30 guest DCtx (only its next input size, +0x7068, is kept up to date: the one field the game reads).
//! - A guest ZSTD_DDict (dictBuffer, dictContent, dictSize first) becomes a host one made from a copy of its content,
//!   cached by guest address, content address, size and a fingerprint of the content.
//!
//! `NXRS_MKW_ZSTD_CHECK=1` decodes each one-shot call both ways, keeps the guest's result and logs every difference.

use super::mkw::{main_addr, now_ns};
use crate::guest;
use nxrt::cpu::Cpu;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};
use zstd_sys as z;

const HOST_MULTIFRAME: u64 = 0xff0;
const HOST_STREAM: u64 = 0xfe0;
const GUEST_MULTIFRAME: u64 = 0xe2280;
const GUEST_STREAM: u64 = 0xdeca0;
const MAX_DICT: u64 = 64 << 20;
const NDICT: usize = 32;
const NSTREAM: usize = 64;
const DCTX_EXPECTED: u64 = 0x7068;
const ADDRESS_LIMIT: u64 = 1 << 40;

pub(super) fn install(page: u64) {
    nxrt::hostfn::alias(
        page + HOST_MULTIFRAME,
        "gamefix:mkw_zstd_multiframe",
        host_multiframe,
    );
    nxrt::hostfn::alias(page + HOST_STREAM, "gamefix:mkw_zstd_stream", host_stream);
}

struct Dict {
    guest: u64,
    content: u64,
    size: u64,
    print: u64,
    dd: usize, // *mut ZSTD_DDict, never freed (another thread may be decoding with it)
}

static DICTS: RwLock<Vec<Dict>> = RwLock::new(Vec::new());

#[derive(Default)]
struct Stats {
    calls: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    ns: AtomicU64,
    guest_ns: AtomicU64,
    fallbacks: AtomicU64,
    inplace: AtomicU64,
}
static S: Stats = Stats {
    calls: AtomicU64::new(0),
    bytes_in: AtomicU64::new(0),
    bytes_out: AtomicU64::new(0),
    ns: AtomicU64::new(0),
    guest_ns: AtomicU64::new(0),
    fallbacks: AtomicU64::new(0),
    inplace: AtomicU64::new(0),
};

fn check_mode() -> bool {
    static V: AtomicUsize = AtomicUsize::new(2);
    let v = V.load(Ordering::Relaxed);
    if v != 2 {
        return v == 1;
    }
    let on = super::set_on("NXRS_MKW_ZSTD_CHECK", false);
    V.store(on as usize, Ordering::Relaxed);
    on
}

fn is_error(r: usize) -> bool {
    unsafe { z::ZSTD_isError(r) != 0 }
}

fn error_name(r: usize) -> String {
    unsafe {
        std::ffi::CStr::from_ptr(z::ZSTD_getErrorName(r))
            .to_string_lossy()
            .into_owned()
    }
}

/// Size, both ends and a sample of the middle: what tells a new dictionary at a reused address from the old one.
fn fingerprint(p: &[u8]) -> u64 {
    let n = p.len() as u64;
    let mut h = n.wrapping_mul(0x9E3779B97F4A7C15);
    for at in [0, n / 2, if n > 64 { n - 64 } else { 0 }] {
        let mut i = at;
        while i < at + 64 && i < n {
            let mut v = [0u8; 8];
            let k = ((n - i) as usize).min(8);
            v[..k].copy_from_slice(&p[i as usize..i as usize + k]);
            h = (h ^ u64::from_le_bytes(v)).wrapping_mul(0xC2B2AE3D27D4EB4F);
            h ^= h >> 29;
            i += 8;
        }
    }
    h
}

/// The host dictionary for a guest ZSTD_DDict or raw dictionary bytes (guest == 0); null when its fields look wrong.
fn dict_for(guest_dd: u64, content: u64, size: u64) -> *mut z::ZSTD_DDict {
    if content == 0
        || size == 0
        || size > MAX_DICT
        || content >= ADDRESS_LIMIT
        || content + size > ADDRESS_LIMIT
        || !guest::readable(content, size)
    {
        return core::ptr::null_mut();
    }
    let bytes = guest::bytes(content, size);
    let print = fingerprint(bytes);
    let find = |v: &Vec<Dict>| {
        v.iter()
            .find(|d| {
                d.guest == guest_dd && d.content == content && d.size == size && d.print == print
            })
            .map(|d| d.dd)
    };
    if let Some(dd) = find(
        &DICTS
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    ) {
        return dd as *mut z::ZSTD_DDict;
    }
    // a copy: the guest may free or reuse its buffer
    let made = unsafe { z::ZSTD_createDDict(bytes.as_ptr() as *const c_void, size as usize) };
    if made.is_null() {
        return made;
    }
    let mut v = DICTS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(dd) = find(&v) {
        unsafe { z::ZSTD_freeDDict(made) };
        return dd as *mut z::ZSTD_DDict;
    }
    if v.len() >= NDICT {
        unsafe { z::ZSTD_freeDDict(made) };
        crate::log!(
            "gamefix: zstd: more than {NDICT} dictionaries: further ones decode in the guest"
        );
        return core::ptr::null_mut();
    }
    v.push(Dict {
        guest: guest_dd,
        content,
        size,
        print,
        dd: made as usize,
    });
    crate::log!(
        "gamefix: zstd: host dictionary {} for {} {:#x}: {size} bytes at {content:#x}, id {}",
        v.len() - 1,
        if guest_dd != 0 { "DDict" } else { "raw dict" },
        if guest_dd != 0 { guest_dd } else { content },
        unsafe { z::ZSTD_getDictID_fromDDict(made) }
    );
    made
}

/// A guest ZSTD_DDict's host twin; null when it cannot be taken.
fn ddict_of(ddict: u64) -> *mut z::ZSTD_DDict {
    if ddict >= ADDRESS_LIMIT - 24 || !guest::readable(ddict, 24) {
        return core::ptr::null_mut();
    }
    let dd = dict_for(ddict, guest::r64(ddict + 8), guest::r64(ddict + 16));
    if dd.is_null() {
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 4 {
            let w: Vec<String> = (0..8)
                .map(|i| format!("{:x}", guest::r64(ddict + 8 * i)))
                .collect();
            crate::log!("gamefix: zstd: DDict {ddict:#x} not taken: {}", w.join(" "));
        }
    }
    dd
}

thread_local! {
    static DCTX: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Decodes on this thread's host context into host memory `dst` (a guest address or a host buffer). Returns libzstd's
/// result, or None when the inputs cannot be taken.
fn host_decode(
    dst: *mut u8,
    cap: u64,
    src: u64,
    size: u64,
    dict: u64,
    dict_size: u64,
    ddict: u64,
) -> Option<usize> {
    if (cap != 0 && dst.is_null())
        || src == 0
        || src + size > ADDRESS_LIMIT
        || !guest::readable(src, size.max(1))
    {
        return None;
    }
    let dd = if ddict != 0 {
        let dd = ddict_of(ddict);
        if dd.is_null() {
            return None;
        }
        dd
    } else if dict != 0 && dict_size != 0 {
        let dd = dict_for(0, dict, dict_size);
        if dd.is_null() {
            return None;
        }
        dd
    } else {
        core::ptr::null_mut()
    };
    let ctx = DCTX.with(|c| {
        if c.get() == 0 {
            c.set(unsafe { z::ZSTD_createDCtx() } as usize);
        }
        c.get()
    }) as *mut z::ZSTD_DCtx;
    if ctx.is_null() {
        return None;
    }
    let s = src as *const c_void;
    Some(unsafe {
        if dd.is_null() {
            z::ZSTD_decompressDCtx(ctx, dst as *mut c_void, cap as usize, s, size as usize)
        } else {
            z::ZSTD_decompress_usingDDict(
                ctx,
                dst as *mut c_void,
                cap as usize,
                s,
                size as usize,
                dd,
            )
        }
    })
}

fn count(bytes_in: u64, bytes_out: u64, ns: u64) {
    let n = S.calls.fetch_add(1, Ordering::Relaxed) + 1;
    S.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
    S.bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
    S.ns.fetch_add(ns, Ordering::Relaxed);
    if n == 1 || n & 0x3fff == 0 {
        crate::log!(
            "gamefix: zstd: {n} calls on the host, {:.1} MiB in, {:.1} MiB out, {:.0} ms (check mode: the guest decoder {:.0} ms); {} to the guest decoder, {} in place in the guest",
            S.bytes_in.load(Ordering::Relaxed) as f64 / 1048576.0,
            S.bytes_out.load(Ordering::Relaxed) as f64 / 1048576.0,
            S.ns.load(Ordering::Relaxed) as f64 / 1e6,
            S.guest_ns.load(Ordering::Relaxed) as f64 / 1e6,
            S.fallbacks.load(Ordering::Relaxed),
            S.inplace.load(Ordering::Relaxed)
        );
    }
}

/// x0 dctx, x1 dst, x2 capacity, x3 src, x4 size, x5 dict, x6 dict size, x7 ddict (ZSTD_decompressMultiFrame).
unsafe fn host_multiframe(c: *mut Cpu) {
    let x = (*c).x;
    let a: [u64; 8] = x[..8].try_into().unwrap();
    let (dst, cap, src, size, dict, dict_size, ddict) = (a[1], a[2], a[3], a[4], a[5], a[6], a[7]);
    let guest_fn = main_addr(GUEST_MULTIFRAME);
    let run_guest = |c: *mut Cpu| {
        if let Some(r) = guest::call(c, guest_fn, &a) {
            (*c).x[0] = r;
        }
    };
    if cap != 0 && src < dst + cap && dst < src + size {
        S.inplace.fetch_add(1, Ordering::Relaxed);
        return run_guest(c);
    }
    if check_mode() {
        let mut mine = vec![0u8; cap as usize];
        let t0 = now_ns();
        let r = host_decode(mine.as_mut_ptr(), cap, src, size, dict, dict_size, ddict);
        let t1 = now_ns();
        let g = guest::call(c, guest_fn, &a).unwrap_or(0);
        (*c).x[0] = g;
        S.guest_ns.fetch_add(now_ns() - t1, Ordering::Relaxed);
        S.ns.fetch_add(t1 - t0, Ordering::Relaxed);
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        match r {
            Some(r) if !is_error(r) => {
                if g != r as u64 || (r != 0 && mine[..r] != *guest::bytes(dst, r as u64)) {
                    if LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
                        crate::log!("[RED] gamefix: zstd check: MISMATCH {size} bytes from {src:#x} (cap {cap}, dict {dict:#x}/{dict_size}, ddict {ddict:#x}, lr {:#x}): the host decoded {r} bytes, the guest {g:#x}", (*c).x[30]);
                    }
                }
            }
            other => {
                if LOGGED.fetch_add(1, Ordering::Relaxed) < 6 {
                    crate::log!("gamefix: zstd check: host {}, guest {g:#x}; dst {dst:#x} cap {cap} src {src:#x} size {size} dict {dict:#x}/{dict_size} ddict {ddict:#x}", other.map_or("inputs not taken".into(), error_name));
                }
            }
        }
        count(size, r.filter(|r| !is_error(*r)).unwrap_or(0) as u64, 0);
        return;
    }
    let t0 = now_ns();
    let r = if dst + cap > ADDRESS_LIMIT {
        None
    } else {
        // the heap's pages are committed on first touch: up front, as before an OS call writes into them
        crate::mem::prepare(dst, cap);
        host_decode(dst as *mut u8, cap, src, size, dict, dict_size, ddict)
    };
    match r {
        Some(r) if !is_error(r) => {
            (*c).x[0] = r as u64;
            count(size, r as u64, now_ns() - t0);
        }
        other => {
            // the guest decoder decides (and writes its own error code)
            S.fallbacks.fetch_add(1, Ordering::Relaxed);
            static LOGGED: AtomicUsize = AtomicUsize::new(0);
            if LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
                crate::log!(
                    "gamefix: zstd: {size} bytes at {src:#x} go to the guest decoder: {}",
                    other.map_or("inputs not taken".into(), error_name)
                );
            }
            run_guest(c);
        }
    }
}

// ---- the loads' stream

struct Stream {
    obj: u64,
    d: usize, // *mut ZSTD_DCtx
}
static STREAMS: Mutex<Vec<Stream>> = Mutex::new(Vec::new());
static SS_FRAMES: AtomicU64 = AtomicU64::new(0);
static SS_CHUNKS: AtomicU64 = AtomicU64::new(0);
static SS_IN: AtomicU64 = AtomicU64::new(0);
static SS_OUT: AtomicU64 = AtomicU64::new(0);
static SS_NS: AtomicU64 = AtomicU64::new(0);
static SS_GUEST: AtomicU64 = AtomicU64::new(0);

/// The host context of a stream; create: a new one for a new frame. Null: not a host stream.
fn stream_ctx(obj: u64, create: bool) -> *mut z::ZSTD_DCtx {
    let mut v = STREAMS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(s) = v.iter().find(|s| s.obj == obj) {
        return s.d as *mut z::ZSTD_DCtx;
    }
    if !create || v.len() >= NSTREAM {
        return core::ptr::null_mut();
    }
    let d = unsafe { z::ZSTD_createDCtx() };
    if !d.is_null() {
        v.push(Stream { obj, d: d as usize });
    }
    d
}

fn stream_drop(obj: u64) {
    let mut v = STREAMS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    v.retain(|s| {
        if s.obj == obj {
            unsafe { z::ZSTD_freeDCtx(s.d as *mut z::ZSTD_DCtx) };
            false
        } else {
            true
        }
    });
}

fn stream_fail(obj: u64, r: i32) -> i32 {
    guest::w8(obj, 3);
    stream_drop(obj);
    r
}

/// One block as the guest's ZSTD_decompressContinue call would decode it; advances dst, room and content left.
fn stream_block(obj: u64, d: *mut z::ZSTD_DCtx, src: *const u8, n: usize) -> Result<(), i32> {
    let dst = guest::r64(obj + 8);
    let room = guest::r32(obj + 0x10) as i32 as i64 as usize;
    crate::mem::prepare(dst, room as u64);
    let r =
        unsafe { z::ZSTD_decompressContinue(d, dst as *mut c_void, room, src as *const c_void, n) };
    if is_error(r) {
        static LOGGED: AtomicUsize = AtomicUsize::new(0);
        if LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
            crate::log!("gamefix: zstd stream {obj:#x}: {}", error_name(r));
        }
        // the guest: -2 when the output is too small, else -1
        let small =
            unsafe { z::ZSTD_getErrorCode(r) } == z::ZSTD_ErrorCode::ZSTD_error_dstSize_tooSmall;
        return Err(if small { -2 } else { -1 });
    }
    guest::w64(obj + 8, dst.wrapping_add(r as u64));
    guest::w32(
        obj + 0x10,
        (guest::r32(obj + 0x10) as i32 - r as i32) as u32,
    );
    guest::w32(
        obj + 0x14,
        (guest::r32(obj + 0x14) as i32 - r as i32) as u32,
    );
    SS_OUT.fetch_add(r as u64, Ordering::Relaxed);
    Ok(())
}

/// The chunk's result; Err(()) when the guest decoder takes the stream.
fn stream_chunk(obj: u64, src0: u64, size0: i32, ddict: u64) -> Result<i32, ()> {
    let state = guest::r8(obj);
    let d;
    if state == 1 {
        let mut dd = core::ptr::null_mut();
        if ddict != 0 {
            dd = ddict_of(ddict);
            if dd.is_null() {
                stream_drop(obj);
                return Err(());
            }
        }
        if check_mode() {
            stream_drop(obj);
            return Err(());
        }
        stream_drop(obj); // a new frame: a fresh context
        d = stream_ctx(obj, true);
        if d.is_null() {
            return Err(());
        }
        let mut h: z::ZSTD_FrameHeader = unsafe { core::mem::zeroed() };
        let r =
            unsafe { z::ZSTD_getFrameHeader(&mut h, src0 as *const c_void, size0.max(0) as usize) };
        if r != 0
            || h.frameType == z::ZSTD_FrameType_e::ZSTD_skippableFrame
            || h.frameContentSize >= (z::ZSTD_CONTENTSIZE_ERROR as i64 as u64)
        {
            return Ok(stream_fail(obj, -1));
        }
        guest::w32(obj + 0x14, h.frameContentSize as u32);
        let b = unsafe {
            if dd.is_null() {
                z::ZSTD_decompressBegin(d)
            } else {
                z::ZSTD_decompressBegin_usingDDict(d, dd)
            }
        };
        if is_error(b) {
            return Ok(stream_fail(obj, -1));
        }
        let mem = guest::r64(obj + 0x18);
        guest::w64(obj + 0x30, if mem & 7 != 0 { 0 } else { mem });
        guest::w8(obj, 2);
        SS_FRAMES.fetch_add(1, Ordering::Relaxed);
    } else if state == 3 {
        return Ok(-1);
    } else if state == 4 {
        return Ok(-3);
    } else {
        d = stream_ctx(obj, false);
        if d.is_null() {
            return Err(()); // a stream the guest decoder started
        }
    }
    let gdctx = guest::r64(obj + 0x30);
    let stage = guest::r64(obj + 0x20);
    SS_CHUNKS.fetch_add(1, Ordering::Relaxed);
    SS_IN.fetch_add(size0.max(0) as u64, Ordering::Relaxed);
    let mut src = src0;
    let mut size = size0;
    while size > 0 {
        let want = unsafe { z::ZSTD_nextSrcSizeToDecompress(d) };
        if want == 0 {
            return Ok(stream_fail(obj, -3)); // input past the end of the frame
        }
        let exp = want as i32;
        let mut staged = guest::r32(obj + 0x2c) as i32;
        if staged < 1 {
            if size < exp {
                // a block split across chunks: wait for the rest in the staging buffer
                if (guest::r32(obj + 0x28) as i32) < exp {
                    return Ok(stream_fail(obj, -1));
                }
                guest::write_bytes(stage, guest::bytes(src, size as u64));
                guest::w32(obj + 0x2c, size as u32);
                break;
            }
            if let Err(e) = stream_block(obj, d, src as *const u8, exp as usize) {
                return Ok(stream_fail(obj, e));
            }
            src += exp as u64;
            size -= exp;
            continue;
        }
        let n = (exp - staged).min(size);
        if n > 0 {
            guest::write_bytes(stage + staged as u64, guest::bytes(src, n as u64));
            src += n as u64;
            size -= n;
            staged += n;
            guest::w32(obj + 0x2c, staged as u32);
        }
        if staged < exp {
            continue;
        }
        if let Err(e) = stream_block(obj, d, stage as *const u8, exp as usize) {
            return Ok(stream_fail(obj, e));
        }
        if staged != exp {
            let rest = guest::bytes(stage + exp as u64, (staged - exp) as u64).to_vec();
            guest::write_bytes(stage, &rest);
        }
        guest::w32(obj + 0x2c, (staged - exp) as u32);
    }
    let left = unsafe { z::ZSTD_nextSrcSizeToDecompress(d) };
    if gdctx != 0 {
        guest::w64(gdctx + DCTX_EXPECTED, left as u64);
    }
    if left == 0 {
        guest::w8(obj, 4);
        stream_drop(obj);
    }
    Ok(guest::r32(obj + 0x14) as i32)
}

unsafe fn host_stream(c: *mut Cpu) {
    let x = (*c).x;
    let a: [u64; 4] = x[..4].try_into().unwrap();
    let was_done = guest::r8(a[0]) == 4;
    let t0 = now_ns();
    match stream_chunk(a[0], a[1], a[2] as i32, a[3]) {
        Err(()) => {
            SS_GUEST.fetch_add(1, Ordering::Relaxed);
            if let Some(r) = guest::call(c, main_addr(GUEST_STREAM), &a) {
                (*c).x[0] = r;
            }
        }
        Ok(r) => {
            (*c).x[0] = r as u32 as u64;
            SS_NS.fetch_add(now_ns() - t0, Ordering::Relaxed);
            let f = SS_FRAMES.load(Ordering::Relaxed);
            if !was_done && guest::r8(a[0]) == 4 && (f == 1 || f & 0x1ff == 0) {
                crate::log!(
                    "gamefix: zstd stream: {f} frames, {} chunks on the host, {:.1} MiB in, {:.1} MiB out, {:.0} ms; {} chunks in the guest decoder",
                    SS_CHUNKS.load(Ordering::Relaxed),
                    SS_IN.load(Ordering::Relaxed) as f64 / 1048576.0,
                    SS_OUT.load(Ordering::Relaxed) as f64 / 1048576.0,
                    SS_NS.load(Ordering::Relaxed) as f64 / 1e6,
                    SS_GUEST.load(Ordering::Relaxed)
                );
            }
        }
    }
}
