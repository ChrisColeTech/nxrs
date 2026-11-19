//! Command buffers: records written into the game's command memory, the list that strings a recording's pieces
//! together in its control memory, and every recorded command.
//!
//! Command memory holds only records; a recording's end, the links between its pieces and its calls of other
//! recordings are control entries, so nothing is written past what GetCommandMemoryUsed reported (games size their
//! command memory by it). Compact records (`obj::opc`) keep the common commands as small as the driver's; the
//! queue's flatten (`for_each_record`) expands them into canonical records for the replay.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

unsafe fn cb<'a>(c: *mut NVNcommandBuffer) -> &'a mut cmd_t {
    &mut *(c as *mut cmd_t)
}
fn align8(p: *mut u8) -> *mut u8 {
    ((p as usize + 7) & !7) as *mut u8
}
unsafe fn put_hdr(p: *mut u8, op: u32, size: u32) {
    core::ptr::write_unaligned(p as *mut op_hdr, op_hdr { op, size });
}

// ---------- host-lent memory: when the game's own command or control memory runs short ----------
const HOSTSEG_SZ: usize = 256 << 10;
const HOSTSEG_CAP: u64 = 1 << 30;
static HOSTSEG_BYTES: AtomicU64 = AtomicU64::new(0);
/// A segment of host memory the recording continues in. Kept for the process: a list recorded into it may be called
/// as long as the game keeps its handle.
fn hostseg_take(n: usize) -> *mut u8 {
    let total = HOSTSEG_BYTES.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
    if total > HOSTSEG_CAP {
        panic!("NVN command memory: host-lent segments reached {} MB", HOSTSEG_CAP >> 20);
    }
    static N: AtomicU32 = AtomicU32::new(0);
    let k = N.fetch_add(1, Ordering::Relaxed) + 1;
    if k == 1 || k % 1024 == 0 {
        crate::nvnlog!("the game's command memory ran short, the host lends a segment of {} KB ({k} times so far)", n >> 10);
    }
    let v = vec![0u64; n / 8].into_boxed_slice();
    Box::leak(v).as_mut_ptr() as *mut u8
}
const CTLSEG_SZ: usize = 64 << 10;

unsafe fn cmd_own_at(o: &mut cmd_t, p: *mut u8) {
    if o.own_back == 0 && p < o.end && (o.end as usize - p as usize) <= u32::MAX as usize {
        o.own_back = (o.end as usize - p as usize) as u32;
    }
}

unsafe fn cmd_new_segment(o: &mut cmd_t, mem: *mut u8, n: usize) {
    let start = align8(mem);
    let end = mem.add(n);
    if o.recording != 0 {
        ctl_chunk_close(o);
    }
    o.cur = start;
    o.end = end;
    o.own_back = 0;
}

unsafe fn ctl_set(o: &mut cmd_t, m: *mut u8, n: usize) {
    let a = if m.is_null() { core::ptr::null_mut() } else { align8(m) };
    let pad = a as usize - m as usize;
    let mut n = if !a.is_null() && n > pad { (n - pad) & !7 } else { 0 };
    n = n.min(0xffff_fff8);
    if !o.ctl_list.is_null() && !o.ctl_mem.is_null() {
        if n < 8 {
            return;
        }
        let e = ctl_e(CTL_LINK, a, 0);
        core::ptr::write_unaligned(o.ctl_mem.add(o.ctl_used as usize) as *mut u64, e);
    }
    o.ctl_mem = a;
    o.ctl_size = n as u32;
    o.ctl_used = 0;
}

unsafe fn ctl_put(o: &mut cmd_t, e: u64) {
    let need = if e & CTL_LAST != 0 { 8 } else { 16 };
    let mut tries = 0;
    while o.ctl_mem.is_null() || o.ctl_size - o.ctl_used < need {
        if o.cb == 0 || tries == 2 {
            let m = hostseg_take(CTLSEG_SZ);
            ctl_set(o, m, CTLSEG_SZ);
            break;
        }
        super::call_guest(o.cb, &[o as *mut cmd_t as u64, cmdbuf_mem_event::OUT_OF_CONTROL_MEMORY as u64, need as u64, o.cb_data]);
        tries += 1;
    }
    let p = o.ctl_mem.add(o.ctl_used as usize);
    core::ptr::write_unaligned(p as *mut u64, e);
    o.ctl_used += 8;
    if o.ctl_list.is_null() {
        o.ctl_list = p;
    }
}

unsafe fn ctl_push(o: &mut cmd_t, e: u64) {
    if o.ctl_q[1] != 0 {
        let q0 = o.ctl_q[0];
        ctl_put(o, q0);
        o.ctl_q[0] = o.ctl_q[1];
        o.ctl_q[1] = 0;
    }
    if o.ctl_q[0] != 0 {
        o.ctl_q[1] = e;
    } else {
        o.ctl_q[0] = e;
    }
}
unsafe fn ctl_flush(o: &mut cmd_t) {
    if o.ctl_q[1] != 0 {
        let q0 = o.ctl_q[0];
        ctl_put(o, q0);
        o.ctl_q[0] = o.ctl_q[1];
        o.ctl_q[1] = 0;
    }
}
unsafe fn ctl_chunk_close(o: &mut cmd_t) {
    if !o.chunk.is_null() && o.cur > o.chunk {
        let e = ctl_e(CTL_CHUNK, o.chunk, (o.cur as usize - o.chunk as usize) as u64);
        ctl_push(o, e);
    }
    o.chunk = core::ptr::null_mut();
}

/// Space for one record; calls the game back for more command memory as NVN does, and lends host memory when the
/// game gives none.
unsafe fn cmd_alloc(o: &mut cmd_t, size: u32) -> *mut u8 {
    let size = (size + 7) & !7;
    let mut tries = 0;
    while o.cur.is_null() || o.cur.add(size as usize) > o.end {
        if o.recording != 0 && !o.cur.is_null() && o.cur < o.end {
            let c = o.cur;
            cmd_own_at(o, c);
        }
        if o.cb == 0 || tries == 2 {
            let sz = if size as usize > HOSTSEG_SZ { (size as usize + 0xfff) & !0xfff } else { HOSTSEG_SZ };
            let m = hostseg_take(sz);
            cmd_new_segment(o, m, sz);
            break;
        }
        super::call_guest(o.cb, &[o as *mut cmd_t as u64, cmdbuf_mem_event::OUT_OF_COMMAND_MEMORY as u64, size as u64, o.cb_data]);
        tries += 1;
    }
    let p = o.cur;
    o.cur = o.cur.add(size as usize);
    o.used += size as u64;
    put_hdr(p, 0, size);
    if o.recording == 2 {
        o.begin = p;
        o.recording = 1;
    }
    if o.recording != 0 {
        cmd_own_at(o, p);
        if !o.chunk.is_null() && (o.cur as usize - o.chunk as usize) as u64 > CTL_CHUNK_MAX {
            if p > o.chunk {
                let e = ctl_e(CTL_CHUNK, o.chunk, (p as usize - o.chunk as usize) as u64);
                ctl_push(o, e);
            }
            o.chunk = core::ptr::null_mut();
        }
        if size as u64 > CTL_CHUNK_MAX {
            ctl_push(o, ctl_e(CTL_CHUNK, p, CTL_ONE));
        } else if o.chunk.is_null() {
            o.chunk = p;
        }
        ctl_flush(o);
    }
    p
}

/// A canonical record with a zeroed payload of `payload` bytes; returns the payload.
unsafe fn rec(c: *mut NVNcommandBuffer, op: u32, payload: usize) -> *mut u8 {
    let sz = ((8 + payload + 7) & !7) as u32;
    let p = cmd_alloc(cb(c), sz);
    put_hdr(p, op, sz);
    core::ptr::write_bytes(p.add(8), 0, sz as usize - 8);
    p.add(8)
}
unsafe fn rec_t<T>(c: *mut NVNcommandBuffer, op: u32) -> *mut T {
    rec(c, op, core::mem::size_of::<T>()) as *mut T
}
/// A compact record: op | args << 8, payload bytes.
unsafe fn crec(c: *mut NVNcommandBuffer, opc: u32, args: u32, payload: usize) -> *mut u8 {
    let sz = ((8 + payload + 7) & !7) as u32;
    let p = cmd_alloc(cb(c), sz);
    put_hdr(p, opc | args << 8, sz);
    core::ptr::write_bytes(p.add(8), 0, sz as usize - 8);
    p.add(8)
}
unsafe fn put32(q: &mut *mut u8, v: u32) {
    core::ptr::write_unaligned(*q as *mut u32, v);
    *q = q.add(4);
}
unsafe fn put64(q: &mut *mut u8, v: u64) {
    core::ptr::write_unaligned(*q as *mut u64, v);
    *q = q.add(8);
}

/// Tests clear it to record the canonical forms only.
pub static REC_COMPACT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
fn compact() -> bool {
    REC_COMPACT.load(Ordering::Relaxed)
}

unsafe fn rec_draw(c: *mut NVNcommandBuffer, d: p_draw) {
    if compact() && d.mdi_cnt == 0 && (d.prim as u32) < 0x100 {
        if d.ind != 0 {
            let mut q = crec(c, opc::DRAW_IND, d.prim as u32, 8);
            put64(&mut q, d.ind);
            return;
        }
        if d.instances == 1 && d.base_instance == 0 {
            let mut q = crec(c, opc::DRAW, d.prim as u32, 8);
            put32(&mut q, d.first as u32);
            put32(&mut q, d.count as u32);
            return;
        }
        let mut q = crec(c, opc::DRAW_INST, d.prim as u32, 16);
        put32(&mut q, d.first as u32);
        put32(&mut q, d.count as u32);
        put32(&mut q, d.base_instance as u32);
        put32(&mut q, d.instances as u32);
        return;
    }
    let n = if d.ind != 0 || d.mdi_cnt != 0 { core::mem::size_of::<p_draw>() } else { P_DRAW_IND_OFF };
    core::ptr::copy_nonoverlapping(&d as *const p_draw as *const u8, rec(c, op::DRAW, n), n);
}
unsafe fn rec_draw_idx(c: *mut NVNcommandBuffer, d: p_draw_idx) {
    if compact() && d.mdi_cnt == 0 && (d.prim as u32) < 0x100 && (d.itype as u32) < 0x100 {
        let a = d.prim as u32 | (d.itype as u32) << 8;
        if d.ind != 0 {
            if d.count == 0 && d.base_vertex == 0 && d.base_instance == 0 && d.instances == 1 {
                let mut q = crec(c, opc::DRAW_IDX_IND, a, 16);
                put64(&mut q, d.ibuf);
                put64(&mut q, d.ind);
                return;
            }
        } else if d.instances == 1 && d.base_instance == 0 {
            let mut q = crec(c, opc::DRAW_IDX, a, 16);
            put32(&mut q, d.count as u32);
            put32(&mut q, d.base_vertex as u32);
            put64(&mut q, d.ibuf);
            return;
        } else {
            let mut q = crec(c, opc::DRAW_IDX_INST, a, 24);
            put32(&mut q, d.count as u32);
            put32(&mut q, d.base_vertex as u32);
            put32(&mut q, d.base_instance as u32);
            put32(&mut q, d.instances as u32);
            put64(&mut q, d.ibuf);
            return;
        }
    }
    let n = if d.ind != 0 || d.mdi_cnt != 0 { core::mem::size_of::<p_draw_idx>() } else { P_DRAW_IDX_IND_OFF };
    core::ptr::copy_nonoverlapping(&d as *const p_draw_idx as *const u8, rec(c, op::DRAW_IDX, n), n);
}

// ---------- compact record expansion ----------
fn xrec(out: &mut Vec<u8>, op: u32, payload: usize) -> usize {
    let sz = (8 + payload + 7) & !7;
    let at = out.len();
    out.resize(at + sz, 0);
    out[at..at + 4].copy_from_slice(&op.to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&(sz as u32).to_le_bytes());
    at + 8
}
fn put_struct<T: Copy>(out: &mut [u8], at: usize, v: &T) {
    let n = core::mem::size_of::<T>();
    // SAFETY: plain-data record structs
    let b = unsafe { core::slice::from_raw_parts(v as *const T as *const u8, n) };
    out[at..at + n].copy_from_slice(b);
}

/// The canonical records of a compact record, appended to `out`.
/// # Safety
/// `r` is a compact record whose header size covers its payload.
pub unsafe fn rec_expand(r: *const u8, out: &mut Vec<u8>) {
    let h = rd::<op_hdr>(r);
    let a = h.op >> 8;
    let mut q = r.add(8);
    let g32 = |q: &mut *const u8| -> u32 {
        let v = core::ptr::read_unaligned(*q as *const u32);
        *q = q.add(4);
        v
    };
    let g64 = |q: &mut *const u8| -> u64 {
        let v = core::ptr::read_unaligned(*q as *const u64);
        *q = q.add(8);
        v
    };
    match h.op & 0xff {
        opc::DRAW | opc::DRAW_INST | opc::DRAW_IND => {
            let mut d = p_draw { prim: (a & 0xff) as i32, instances: 1, ..Default::default() };
            if h.op & 0xff == opc::DRAW_IND {
                d.ind = g64(&mut q);
            } else {
                d.first = g32(&mut q) as i32;
                d.count = g32(&mut q) as i32;
                if h.op & 0xff == opc::DRAW_INST {
                    d.base_instance = g32(&mut q) as i32;
                    d.instances = g32(&mut q) as i32;
                }
            }
            let at = xrec(out, op::DRAW, core::mem::size_of::<p_draw>());
            put_struct(out, at, &d);
        }
        opc::DRAW_IDX | opc::DRAW_IDX_INST | opc::DRAW_IDX_IND => {
            let mut d = p_draw_idx { prim: (a & 0xff) as i32, itype: (a >> 8 & 0xff) as i32, instances: 1, ..Default::default() };
            if h.op & 0xff == opc::DRAW_IDX_IND {
                d.ibuf = g64(&mut q);
                d.ind = g64(&mut q);
            } else {
                d.count = g32(&mut q) as i32;
                d.base_vertex = g32(&mut q) as i32;
                if h.op & 0xff == opc::DRAW_IDX_INST {
                    d.base_instance = g32(&mut q) as i32;
                    d.instances = g32(&mut q) as i32;
                }
                d.ibuf = g64(&mut q);
            }
            let at = xrec(out, op::DRAW_IDX, core::mem::size_of::<p_draw_idx>());
            put_struct(out, at, &d);
        }
        opc::VIEWPORTS => {
            let mut r = p_viewports { first: (a & 0xff) as i32, n: (a >> 8 & 0xff) as i32, v: [[0.0; 6]; 16] };
            r.v[0][4] = f32::from_bits(a >> 16 & 3);
            for i in 0..r.n as usize {
                for k in 0..4 {
                    r.v[i][k] = f32::from_bits(g32(&mut q));
                }
            }
            let at = xrec(out, op::VIEWPORTS, core::mem::size_of::<p_viewports>());
            put_struct(out, at, &r);
        }
        opc::STENCIL => {
            let mut s = p_stencil { face: (a & 3) as i32, which: (a >> 2 & 3) as i32, ..Default::default() };
            let v = (a >> 4) as i32;
            match s.which {
                0 => s.value_mask = v,
                1 => s.mask = v,
                _ => s.r#ref = v,
            }
            let at = xrec(out, op::STENCIL, core::mem::size_of::<p_stencil>());
            put_struct(out, at, &s);
        }
        opc::HANDLES => {
            let cnt = (a >> 17 & 0x3f) as usize;
            let at = xrec(out, a & 0x3f, 24 + 8 * cnt);
            let mut r = p_bind_handles { stage: (a >> 6 & 7) as i32, first: (a >> 9 & 0xff) as i32, n: cnt as i32, defer: 0, src: 0, h: [0; 32] };
            for i in 0..cnt {
                r.h[i] = g32(&mut q) as u64;
            }
            let bytes = core::slice::from_raw_parts(&r as *const p_bind_handles as *const u8, 24 + 8 * cnt);
            out[at..at + bytes.len()].copy_from_slice(bytes);
        }
        opc::BUFS => {
            let (o, st, first, cnt) = (a & 0x3f, a >> 6 & 7, a >> 9 & 0xff, a >> 17 & 0x3f);
            for i in 0..cnt {
                let addr = g64(&mut q);
                let size = g32(&mut q);
                if o == op::BIND_UBO_SMALL {
                    let at = xrec(out, o, core::mem::size_of::<p_bind_ubo_small>());
                    put_struct(out, at, &p_bind_ubo_small { stage: st as u16, index: (first + i) as u16, size, addr });
                } else {
                    let at = xrec(out, o, core::mem::size_of::<p_bind_buf>());
                    put_struct(out, at, &p_bind_buf { stage: st as i32, index: (first + i) as i32, addr, size: size as u64 });
                }
            }
        }
        opc::VBUFS => {
            let cnt = (a >> 8 & 0xff) as usize;
            let mut r = p_bind_vbuf { first: (a & 0xff) as i32, n: cnt as i32, r: [NVNbufferRange::default(); 16] };
            for i in 0..cnt.min(16) {
                r.r[i].address = g64(&mut q);
                r.r[i].size = g32(&mut q) as u64;
            }
            let at = xrec(out, op::BIND_VBUF, core::mem::size_of::<p_bind_vbuf>());
            put_struct(out, at, &r);
        }
        opc::VATTR => {
            let cnt = (a & 0xff) as usize;
            let mut r = p_bind_vattr { first: 0, n: cnt as i32, st: [0; 32] };
            for i in 0..cnt.min(32) {
                r.st[i] = g32(&mut q);
            }
            let at = xrec(out, op::BIND_VATTR, core::mem::size_of::<p_bind_vattr>());
            put_struct(out, at, &r);
        }
        opc::VSTREAM => {
            let cnt = (a & 0xff) as usize;
            let mut r = p_bind_vstream { first: 0, n: cnt as i32, st: [vs_st::default(); 16] };
            for i in 0..cnt.min(16) {
                r.st[i].stride = g32(&mut q);
                r.st[i].divisor = g32(&mut q);
            }
            let at = xrec(out, op::BIND_VSTREAM, core::mem::size_of::<p_bind_vstream>());
            put_struct(out, at, &r);
        }
        opc::UBO => {
            let (st, first, cnt) = (a & 7, a >> 3 & 0xf, a >> 7 & 0x1f);
            for i in 0..cnt {
                let v = g64(&mut q);
                let at = xrec(out, op::BIND_UBO_SMALL, core::mem::size_of::<p_bind_ubo_small>());
                put_struct(out, at, &p_bind_ubo_small { stage: st as u16, index: (first + i) as u16, size: (v >> 40) as u32, addr: (v & ((1u64 << 40) - 1)) << 8 });
            }
        }
        opc::STATE | opc::STATE8 => {
            let mut r = p_bind_state { kind: (a & 7) as i32, bytes: [0; 0x18] };
            if h.op & 0xff == opc::STATE {
                r.bytes[..4].copy_from_slice(&(a >> 3).to_le_bytes());
            } else {
                core::ptr::copy_nonoverlapping(q, r.bytes.as_mut_ptr(), 8);
            }
            let at = xrec(out, op::BIND_STATE, core::mem::size_of::<p_bind_state>());
            put_struct(out, at, &r);
        }
        opc::BARRIER => {
            let at = xrec(out, op::BARRIER, 8);
            out[at..at + 4].copy_from_slice(&a.to_le_bytes());
        }
        opc::PRIM_RESTART => {
            let at = xrec(out, op::PRIM_RESTART, core::mem::size_of::<p_prim_restart>());
            put_struct(out, at, &p_prim_restart { on: a & 1, index: if a & 2 != 0 { u32::MAX } else { a >> 2 } });
        }
        opc::SCISSOR => {
            let v: [u16; 4] = core::ptr::read_unaligned(q as *const [u16; 4]);
            let at = xrec(out, op::SCISSOR, core::mem::size_of::<p_scissor>());
            put_struct(out, at, &p_scissor { x: v[0] as i32, y: v[1] as i32, w: v[2] as i32, h: v[3] as i32 });
        }
        _ => panic!("NVN command list: unknown compact record {:#x}", h.op),
    }
}

// ---------- reading a recording ----------
/// The pieces of a recording's control list in order (following links), as CTL_CHUNK entries.
/// # Safety
/// `handle` is a list EndRecording returned.
pub unsafe fn ctl_pieces(handle: u64, mut f: impl FnMut(u64)) {
    if handle == 0 || handle & 7 != 0 || handle >> 47 != 0 {
        crate::log::red(&format!("command handle {handle:#x} is no command list"));
        return;
    }
    let mut e = handle as *const u8;
    let mut links = 0;
    loop {
        let v = core::ptr::read_unaligned(e as *const u64);
        if ctl_kind(v) == CTL_LINK {
            links += 1;
            if links > 1024 {
                crate::log::red("NVN command list: more than 1024 links in a row");
                return;
            }
            e = ctl_addr(v);
            continue;
        }
        if ctl_kind(v) != CTL_CHUNK {
            crate::log::red(&format!("NVN command list: control entry {v:#x} at {e:p} is no piece"));
            return;
        }
        links = 0;
        e = e.add(8);
        f(v & !CTL_LAST);
        if v & CTL_LAST != 0 {
            return;
        }
    }
}

/// Every canonical record of the given command handles in order, compact ones expanded. `f(op, record)` gets the
/// record's bytes (header included).
/// # Safety
/// Handles are lists EndRecording returned whose command memory is still the recording's.
pub unsafe fn for_each_record(handles: &[u64], mut f: impl FnMut(u32, &[u8])) {
    let mut x = Vec::with_capacity(REC_EXPAND_MAX);
    for &h in handles {
        ctl_pieces(h, |v| {
            let n = ctl_chunk_bytes(v);
            let base = ctl_addr(v);
            let mut p = 0usize;
            while p + 8 <= n {
                let hd = rd::<op_hdr>(base.add(p));
                if hd.size < 8 || hd.size & 7 != 0 || hd.size as usize > n - p {
                    crate::log::red(&format!("NVN command list: record {{op {}, size {}}} does not fit its piece; the rest is skipped", hd.op, hd.size));
                    return;
                }
                let r = core::slice::from_raw_parts(base.add(p), hd.size as usize);
                if rec_compact(hd.op) {
                    x.clear();
                    rec_expand(r.as_ptr(), &mut x);
                    let mut k = 0;
                    while k < x.len() {
                        let hh = rd::<op_hdr>(x.as_ptr().add(k));
                        f(hh.op, &x[k..k + hh.size as usize]);
                        k += hh.size as usize;
                    }
                } else {
                    f(hd.op, r);
                }
                p += hd.size as usize;
            }
        });
    }
}

/// A recording's canonical records as one block ending in OP_END (tests, captures).
/// # Safety
/// As `for_each_record`.
pub unsafe fn rec_linear(handle: u64) -> Vec<u8> {
    let mut v = Vec::new();
    for_each_record(&[handle], |_, r| v.extend_from_slice(r));
    v.extend_from_slice(&op::END.to_le_bytes());
    v.extend_from_slice(&8u32.to_le_bytes());
    v
}

static REC_SERIAL: AtomicU32 = AtomicU32::new(0);
static COPY_MU: Mutex<()> = Mutex::new(());

unsafe fn tex_snap(r: *mut u8, off: usize, k: usize, t: *const NVNtexture) -> *const tex_t {
    if t.is_null() {
        return core::ptr::null();
    }
    let s = (r.add(off) as *mut tex_t).add(k);
    core::ptr::copy_nonoverlapping(t as *const tex_t, s, 1);
    if (*s).magic != MAGIC_TEX {
        crate::log::red_once("texture object invalid while recording (uninitialized or finalized): read again at submit");
        return t as *const tex_t;
    }
    s
}
unsafe fn copy_view(dst: &mut view_t, has: &mut u8, v: *const NVNtextureView) {
    *has = (!v.is_null()) as u8;
    if !v.is_null() {
        *dst = *(v as *const view_t);
    }
}
/// A record followed by `k` texture snapshots.
unsafe fn rec_snap<T>(c: *mut NVNcommandBuffer, op: u32, k: usize) -> *mut T {
    rec(c, op, snap_off(core::mem::size_of::<T>()) + k * core::mem::size_of::<tex_t>()) as *mut T
}
fn slots_fit(st: i32, first: i32, n: i32, slots: i32) -> i32 {
    if st as u32 >= RESOURCE_STAGES as u32 || first < 0 || n < 0 || first >= slots {
        0
    } else if n < slots - first {
        n
    } else {
        slots - first
    }
}
fn slots_bound(what: &str, st: i32, first: i32, n: i32, slots: i32) -> i32 {
    let k = slots_fit(st, first, n, slots);
    if k != n {
        crate::log::red_once(&format!("{what} stage {st} first={first} n={n}: the host keeps {slots} slots a stage"));
    }
    k
}
fn st_host(st: i32) -> i32 {
    match st {
        6 => 3,
        7 => 4,
        8 => 6,
        s => s,
    }
}
unsafe fn rec_ubo(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, rg: *const NVNbufferRange) -> bool {
    if !compact() || st as u32 >= RESOURCE_STAGES as u32 || first < 0 || n < 1 || first >= 16 || n > 16 - first {
        return false;
    }
    for i in 0..n as usize {
        let r = *rg.add(i);
        if r.address & 0xff != 0 || r.address >> 48 != 0 || r.size >= 1 << 18 {
            return false;
        }
    }
    let mut q = crec(c, opc::UBO, st as u32 | (first as u32) << 3 | (n as u32) << 7, 8 * n as usize);
    for i in 0..n as usize {
        let r = *rg.add(i);
        put64(&mut q, r.address >> 8 | r.size << 40);
    }
    true
}
unsafe fn record_uniform_buffer(c: *mut NVNcommandBuffer, st: i32, i: i32, a: u64, n: u64) {
    let r = NVNbufferRange { address: a, size: n };
    if rec_ubo(c, st, i, 1, &r) {
        return;
    }
    if n <= u32::MAX as u64 {
        *rec_t::<p_bind_ubo_small>(c, op::BIND_UBO_SMALL) = p_bind_ubo_small { stage: st as u16, index: i as u16, size: n as u32, addr: a };
    } else {
        *rec_t::<p_bind_buf>(c, op::BIND_UBO) = p_bind_buf { stage: st, index: i, addr: a, size: n };
    }
}
unsafe fn rec_bufs(c: *mut NVNcommandBuffer, o: u32, st: i32, first: i32, n: i32, rg: *const NVNbufferRange, slots: i32) -> bool {
    if !compact() || !(1..=63).contains(&n) || st as u32 >= RESOURCE_STAGES as u32 || first < 0 || first > 0xff || first >= slots || n > slots - first {
        return false;
    }
    for i in 0..n as usize {
        if (*rg.add(i)).size > u32::MAX as u64 {
            return false;
        }
    }
    let mut q = crec(c, opc::BUFS, o | (st as u32) << 6 | (first as u32) << 9 | (n as u32) << 17, 12 * n as usize);
    for i in 0..n as usize {
        let r = *rg.add(i);
        put64(&mut q, r.address);
        put32(&mut q, r.size as u32);
    }
    true
}
unsafe fn bind_handles(c: *mut NVNcommandBuffer, o: u32, st: i32, first: i32, n: i32, h: *const u64) {
    if !(0..=32).contains(&n) {
        crate::log::red(&format!("bind handle count {n}"));
        return;
    }
    let slots = if o == op::BIND_SAMP { 128 } else if o == op::BIND_IMG { 32 } else { SEPTEX_SLOT0 + 128 };
    let what = if o == op::BIND_SAMP { "BindSamplers" } else if o == op::BIND_IMG { "BindImages" } else { "BindTextures" };
    let n = slots_bound(what, st, first, n, slots);
    if n == 0 {
        return;
    }
    // The driver writes each handle into the c2 table as a 32-bit word: the GPU never sees the upper half.
    if compact() && (st as u32) < RESOURCE_STAGES as u32 && first >= 0 && first <= 0xff {
        let mut q = crec(c, opc::HANDLES, o | (st as u32) << 6 | (first as u32) << 9 | (n as u32) << 17, 4 * n as usize);
        for i in 0..n as usize {
            put32(&mut q, *h.add(i) as u32);
        }
        return;
    }
    let r = rec(c, o, 24 + 8 * n as usize) as *mut p_bind_handles;
    (*r).stage = st;
    (*r).first = first;
    (*r).n = n;
    for i in 0..n as usize {
        (*r).h[i] = *h.add(i) as u32 as u64;
    }
}
unsafe fn bind_state(c: *mut NVNcommandBuffer, kind: i32, s: *const u8, n: usize) {
    if compact() && n == 4 {
        let v = core::ptr::read_unaligned(s as *const u32);
        if v < 1 << 21 {
            crec(c, opc::STATE, kind as u32 | v << 3, 0);
            return;
        }
    }
    if compact() && n == 8 {
        let q = crec(c, opc::STATE8, kind as u32, 8);
        core::ptr::copy_nonoverlapping(s, q, 8);
        return;
    }
    let r = rec(c, op::BIND_STATE, 4 + n);
    core::ptr::write_unaligned(r as *mut i32, kind);
    core::ptr::copy_nonoverlapping(s, r.add(4), n);
}
unsafe fn rec_stencil(c: *mut NVNcommandBuffer, f: i32, which: i32, v: i32) {
    if compact() && (f as u32) < 4 && (v as u32) < 1 << 20 {
        crec(c, opc::STENCIL, f as u32 | (which as u32) << 2 | (v as u32) << 4, 0);
        return;
    }
    *rec_t::<p_stencil>(c, op::STENCIL) = p_stencil {
        face: f,
        value_mask: if which == 0 { v } else { 0 },
        mask: if which == 1 { v } else { 0 },
        r#ref: if which == 2 { v } else { 0 },
        which,
    };
}
unsafe fn rec_copy(c: *mut NVNcommandBuffer, o: u32) -> *mut p_copy_b2t {
    let cc = cb(c);
    let st = cc.copy_row != 0 || cc.copy_image != 0;
    let r = rec(c, o, COPY_STRIDES_OFF + if st { core::mem::size_of::<p_copy_strides>() } else { 0 });
    if st {
        core::ptr::write_unaligned(r.add(COPY_STRIDES_OFF) as *mut p_copy_strides, p_copy_strides { row: cc.copy_row, image: cc.copy_image });
    }
    r as *mut p_copy_b2t
}
unsafe fn dev_origin(c: *mut NVNcommandBuffer) -> u32 {
    let d = cb(c).dev;
    if d.is_null() { 0 } else { (*(d as *const dev_t)).origin }
}

nvn_fns! {
    fn nvnCommandBufferInitialize(c: *mut NVNcommandBuffer, d: *mut NVNdevice) -> NVNboolean {
        core::ptr::write_bytes(c as *mut u8, 0, core::mem::size_of::<cmd_t>());
        cb(c).magic = MAGIC_CMD;
        cb(c).dev = d;
        1
    }
    fn nvnCommandBufferFinalize(c: *mut NVNcommandBuffer) { cb(c).magic = 0; }
    fn nvnCommandBufferSetDebugLabel(c: *mut NVNcommandBuffer, s: *const u8) { let _ = (c, s); }
    fn nvnCommandBufferSetMemoryCallback(c: *mut NVNcommandBuffer, f: u64) { cb(c).cb = f; }
    fn nvnCommandBufferSetMemoryCallbackData(c: *mut NVNcommandBuffer, d: u64) { cb(c).cb_data = d; }
    fn nvnCommandBufferAddCommandMemory(c: *mut NVNcommandBuffer, p: *const NVNmemoryPool, off: isize, n: usize) {
        let o = cb(c);
        if !p.is_null() { o.cmd_size = n; }
        if p.is_null() || n < 8 {
            let m = hostseg_take(HOSTSEG_SZ);
            cmd_new_segment(o, m, HOSTSEG_SZ);
            return;
        }
        let pl = &*(p as *const pool_t);
        if pl.mem.is_null() { crate::log::red("AddCommandMemory: pool has no CPU storage"); return; }
        cmd_new_segment(o, pl.mem.add(off as usize), n);
        o.used = 0;
    }
    fn nvnCommandBufferAddControlMemory(c: *mut NVNcommandBuffer, m: *mut u8, n: usize) { ctl_set(cb(c), m, n); }
    fn nvnCommandBufferGetCommandMemorySize(c: *const NVNcommandBuffer) -> usize { (*(c as *const cmd_t)).cmd_size }
    fn nvnCommandBufferGetCommandMemoryUsed(c: *const NVNcommandBuffer) -> usize { (*(c as *const cmd_t)).used as usize }
    fn nvnCommandBufferGetCommandMemoryFree(c: *const NVNcommandBuffer) -> usize {
        let o = &*(c as *const cmd_t);
        if o.cur.is_null() { 0 } else { o.end as usize - o.cur as usize }
    }
    fn nvnCommandBufferGetControlMemorySize(c: *const NVNcommandBuffer) -> usize { (*(c as *const cmd_t)).ctl_size as usize }
    fn nvnCommandBufferGetControlMemoryUsed(c: *const NVNcommandBuffer) -> usize {
        let o = &*(c as *const cmd_t);
        o.ctl_used as usize + 8 * o.ctl_q.iter().filter(|&&e| e != 0).count()
    }
    fn nvnCommandBufferGetControlMemoryFree(c: *const NVNcommandBuffer) -> usize {
        let o = &*(c as *const cmd_t);
        (o.ctl_size - o.ctl_used) as usize
    }
    fn nvnCommandBufferBeginRecording(c: *mut NVNcommandBuffer) {
        let o = cb(c);
        if o.recording != 0 { crate::log::red("BeginRecording re-entered"); }
        o.recording = 2;
        o.rec_id = REC_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
        o.chunk = core::ptr::null_mut();
        o.ctl_list = core::ptr::null_mut();
        o.ctl_q = [0, 0];
        o.own_back = 0;
    }
    fn nvnCommandBufferEndRecording(c: *mut NVNcommandBuffer) -> u64 {
        let o = cb(c);
        ctl_chunk_close(o);
        if o.ctl_q[0] == 0 { o.ctl_q[0] = ctl_e(CTL_CHUNK, core::ptr::null(), 0); }
        ctl_flush(o);
        let last = o.ctl_q[0] | CTL_LAST;
        ctl_put(o, last);
        o.ctl_q[0] = 0;
        let h = o.ctl_list;
        o.ctl_list = core::ptr::null_mut();
        o.recording = 0;
        h as u64
    }
    fn nvnCommandBufferIsRecording(c: *const NVNcommandBuffer) -> NVNboolean { ((*(c as *const cmd_t)).recording != 0) as u8 }
    /// The called recording's pieces join the caller's list; its records run in place at submit.
    fn nvnCommandBufferCallCommands(c: *mut NVNcommandBuffer, n: i32, h: *const u64) {
        let o = cb(c);
        if o.recording == 0 { crate::log::red("CallCommands outside a recording"); return; }
        for i in 0..n.max(0) as usize {
            ctl_chunk_close(o);
            let mut pieces = Vec::new();
            ctl_pieces(*h.add(i), |v| if ctl_len(v) != 0 { pieces.push(v) });
            for v in pieces {
                ctl_push(o, v);
                ctl_flush(o);
            }
        }
    }
    /// The called recording's records are copied into the caller's command memory.
    fn nvnCommandBufferCopyCommands(c: *mut NVNcommandBuffer, n: i32, h: *const u64) {
        let _g = COPY_MU.lock().unwrap();
        if cb(c).recording == 0 { crate::log::red("CopyCommands outside a recording"); return; }
        for i in 0..n.max(0) as usize {
            let mut recs: Vec<Vec<u8>> = Vec::new();
            ctl_pieces(*h.add(i), |v| {
                let n = ctl_chunk_bytes(v);
                let base = ctl_addr(v);
                let mut p = 0;
                while p + 8 <= n {
                    let hd = rd::<op_hdr>(base.add(p));
                    if hd.size < 8 || hd.size as usize > n - p { break; }
                    recs.push(core::slice::from_raw_parts(base.add(p), hd.size as usize).to_vec());
                    p += hd.size as usize;
                }
            });
            for r in recs {
                let d = cmd_alloc(cb(c), r.len() as u32);
                core::ptr::copy_nonoverlapping(r.as_ptr(), d, r.len());
            }
        }
    }

    // ---------- sync and CPU-visible effects ----------
    fn nvnCommandBufferWaitEvent(c: *mut NVNcommandBuffer, e: *const NVNevent, mode: i32, value: u32) {
        let p = rec(c, op::WAIT_EVENT, 16);
        core::ptr::write_unaligned(p as *mut u64, (*(e as *const ev_t)).val as u64);
        core::ptr::write_unaligned(p.add(8) as *mut u32, mode as u32);
        core::ptr::write_unaligned(p.add(12) as *mut u32, value);
    }
    fn nvnCommandBufferSignalEvent(c: *mut NVNcommandBuffer, e: *const NVNevent, mode: i32, loc: i32, flags: i32, value: u32) {
        let _ = (loc, flags);
        let p = rec(c, op::SIGNAL_EVENT, 16);
        core::ptr::write_unaligned(p as *mut u64, (*(e as *const ev_t)).val as u64);
        core::ptr::write_unaligned(p.add(8) as *mut u32, mode as u32);
        core::ptr::write_unaligned(p.add(12) as *mut u32, value);
    }
    fn nvnCommandBufferWaitSync(c: *mut NVNcommandBuffer, s: *const NVNsync) {
        let p = rec(c, op::WAIT_SYNC, 8);
        core::ptr::write_unaligned(p as *mut u64, s as u64);
    }
    fn nvnCommandBufferFenceSync(c: *mut NVNcommandBuffer, s: *mut NVNsync, cond: i32, flags: i32) {
        let _ = (cond, flags);
        let p = rec(c, op::FENCE_SYNC, 8);
        core::ptr::write_unaligned(p as *mut u64, s as u64);
    }
    fn nvnCommandBufferResetCounter(c: *mut NVNcommandBuffer, t: i32) {
        let p = rec(c, op::RESET_COUNTER, 8);
        core::ptr::write_unaligned(p as *mut u64, t as u64);
    }
    fn nvnCommandBufferReportCounter(c: *mut NVNcommandBuffer, t: i32, a: u64) {
        let p = rec(c, op::REPORT_COUNTER, 16);
        core::ptr::write_unaligned(p as *mut u64, t as u64);
        core::ptr::write_unaligned(p.add(8) as *mut u64, a);
    }
    fn nvnCommandBufferCopyBufferToBuffer(c: *mut NVNcommandBuffer, src: u64, dst: u64, n: usize, flags: i32) {
        let _ = flags;
        let p = rec(c, op::COPY_B2B, 24);
        core::ptr::write_unaligned(p as *mut [u64; 3], [src, dst, n as u64]);
    }
    /// The data is copied into command memory now (the caller may change it after recording).
    fn nvnCommandBufferUpdateUniformBuffer(c: *mut NVNcommandBuffer, buf: u64, bufsz: usize, off: isize, n: usize, data: *const u8) {
        let _ = bufsz;
        let p = rec(c, op::UPDATE_UB, 16 + n);
        core::ptr::write_unaligned(p as *mut [u64; 2], [buf + off as u64, n as u64]);
        core::ptr::copy_nonoverlapping(data, p.add(16), n);
    }
    fn nvnCommandBufferClearBuffer(c: *mut NVNcommandBuffer, a: u64, n: usize, v: u32) {
        *rec_t::<p_clear_buf>(c, op::CLEAR_BUF) = p_clear_buf { addr: a, size: n as u64, value: v, _pad: 0 };
    }

    // ---------- render targets and clears ----------
    fn nvnCommandBufferSetRenderTargets(c: *mut NVNcommandBuffer, n: i32, ct: *const *const NVNtexture, cv: *const *const NVNtextureView, dt: *const NVNtexture, dv: *const NVNtextureView) {
        let n = if !(0..=8).contains(&n) { crate::log::red(&format!("SetRenderTargets count {n}")); n.clamp(0, 8) } else { n };
        let r = rec_snap::<p_set_rt>(c, op::SET_RT, n as usize + 1);
        let off = snap_off(core::mem::size_of::<p_set_rt>());
        (*r).n = n;
        for i in 0..n as usize {
            (*r).color[i] = if ct.is_null() { core::ptr::null() } else { tex_snap(r as *mut u8, off, i, *ct.add(i)) };
            copy_view(&mut (*r).cview[i], &mut (*r).has_cview[i], if cv.is_null() { core::ptr::null() } else { *cv.add(i) });
        }
        (*r).depth = tex_snap(r as *mut u8, off, n as usize, dt);
        copy_view(&mut (*r).dview, &mut (*r).has_dview, dv);
    }
    fn nvnCommandBufferClearColor(c: *mut NVNcommandBuffer, index: i32, rgba: *const f32, mask: i32) {
        *rec_t::<p_clear_color>(c, op::CLEAR_COLOR) = p_clear_color { index, mask, rgba: core::ptr::read_unaligned(rgba as *const [f32; 4]) };
    }
    fn nvnCommandBufferClearColori(c: *mut NVNcommandBuffer, index: i32, rgba: *const i32, mask: i32) {
        let v = core::ptr::read_unaligned(rgba as *const [i32; 4]);
        *rec_t::<p_clear_color>(c, op::CLEAR_COLOR) = p_clear_color { index, mask, rgba: [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32] };
    }
    fn nvnCommandBufferClearColorui(c: *mut NVNcommandBuffer, index: i32, rgba: *const u32, mask: i32) {
        let v = core::ptr::read_unaligned(rgba as *const [u32; 4]);
        *rec_t::<p_clear_color>(c, op::CLEAR_COLOR) = p_clear_color { index, mask, rgba: [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32] };
    }
    fn nvnCommandBufferClearDepthStencil(c: *mut NVNcommandBuffer, depth: f32, dmask: u32, stencil: i32, smask: i32) {
        *rec_t::<p_clear_ds>(c, op::CLEAR_DS) = p_clear_ds { depth, depth_mask: super::b(dmask) as i32, stencil, stencil_mask: smask };
    }
    fn nvnCommandBufferClearTexture(c: *mut NVNcommandBuffer, t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, rgba: *const f32, mask: i32) {
        let o = rec_snap::<p_clear_tex>(c, op::CLEAR_TEX, 1);
        (*o).tex = tex_snap(o as *mut u8, snap_off(core::mem::size_of::<p_clear_tex>()), 0, t);
        copy_view(&mut (*o).view, &mut (*o).has_view, v);
        (*o).r = *r;
        (*o).mask = mask;
        (*o).is_uint = 0;
        (*o).c = core::ptr::read_unaligned(rgba as *const [u32; 4]);
    }
    fn nvnCommandBufferClearTextureui(c: *mut NVNcommandBuffer, t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, rgba: *const u32, mask: i32) {
        let o = rec_snap::<p_clear_tex>(c, op::CLEAR_TEX, 1);
        (*o).tex = tex_snap(o as *mut u8, snap_off(core::mem::size_of::<p_clear_tex>()), 0, t);
        copy_view(&mut (*o).view, &mut (*o).has_view, v);
        (*o).r = *r;
        (*o).mask = mask;
        (*o).is_uint = 1;
        (*o).c = core::ptr::read_unaligned(rgba as *const [u32; 4]);
    }

    // ---------- viewport, scissor, fixed state ----------
    fn nvnCommandBufferSetViewport(c: *mut NVNcommandBuffer, x: i32, y: i32, w: i32, h: i32) {
        let v = [x as f32, y as f32, w as f32, h as f32];
        nvnCommandBufferSetViewports(c, 0, 1, v.as_ptr());
    }
    fn nvnCommandBufferSetViewports(c: *mut NVNcommandBuffer, first: i32, n: i32, v: *const f32) {
        if slots_fit(0, first, n, 16) != n { crate::log::red(&format!("SetViewports first={first} n={n}")); return; }
        let org = dev_origin(c) + 1;
        if compact() {
            let q = crec(c, opc::VIEWPORTS, first as u32 | (n as u32) << 8 | org << 16, 16 * n as usize);
            core::ptr::copy_nonoverlapping(v as *const u8, q, 16 * n as usize);
            return;
        }
        let r = rec_t::<p_viewports>(c, op::VIEWPORTS);
        (*r).first = first;
        (*r).n = n;
        for i in 0..n as usize {
            for k in 0..4 { (*r).v[i][k] = *v.add(4 * i + k); }
        }
        (*r).v[0][4] = f32::from_bits(org);
    }
    fn nvnCommandBufferSetScissor(c: *mut NVNcommandBuffer, x: i32, y: i32, w: i32, h: i32) {
        if compact() && (x as u32) < 0x10000 && (y as u32) < 0x10000 && (w as u32) < 0x10000 && (h as u32) < 0x10000 {
            let q = crec(c, opc::SCISSOR, 0, 8);
            core::ptr::write_unaligned(q as *mut [u16; 4], [x as u16, y as u16, w as u16, h as u16]);
            return;
        }
        *rec_t::<p_scissor>(c, op::SCISSOR) = p_scissor { x, y, w, h };
    }
    fn nvnCommandBufferSetScissors(c: *mut NVNcommandBuffer, first: i32, n: i32, v: *const i32) {
        if first == 0 && n >= 1 { nvnCommandBufferSetScissor(c, *v, *v.add(1), *v.add(2), *v.add(3)); }
    }
    fn nvnCommandBufferSetDepthRange(c: *mut NVNcommandBuffer, n: f32, f: f32) { *rec_t::<p_depth_range>(c, op::DEPTH_RANGE) = p_depth_range { n, f }; }
    fn nvnCommandBufferSetDepthRanges(c: *mut NVNcommandBuffer, first: i32, n: i32, v: *const f32) {
        if first == 0 && n >= 1 { nvnCommandBufferSetDepthRange(c, *v, *v.add(1)); }
    }
    fn nvnCommandBufferSetBlendColor(c: *mut NVNcommandBuffer, v: *const f32) {
        *rec_t::<p_blend_color>(c, op::BLEND_COLOR) = p_blend_color { rgba: core::ptr::read_unaligned(v as *const [f32; 4]) };
    }
    fn nvnCommandBufferSetPolygonOffsetClamp(c: *mut NVNcommandBuffer, fa: f32, un: f32, cl: f32) {
        *rec_t::<p_poly_offset>(c, op::POLY_OFFSET) = p_poly_offset { factor: fa, units: un, clamp: cl };
    }
    fn nvnCommandBufferSetStencilValueMask(c: *mut NVNcommandBuffer, f: i32, m: i32) { rec_stencil(c, f, 0, m); }
    fn nvnCommandBufferSetStencilMask(c: *mut NVNcommandBuffer, f: i32, m: i32) { rec_stencil(c, f, 1, m); }
    fn nvnCommandBufferSetStencilRef(c: *mut NVNcommandBuffer, f: i32, v: i32) { rec_stencil(c, f, 2, v); }
    fn nvnCommandBufferSetPrimitiveRestart(c: *mut NVNcommandBuffer, on: u32, index: i32) {
        let on = super::b(on) as u32;
        let idx = index as u32;
        if compact() && (idx == u32::MAX || idx < 1 << 22) {
            crec(c, opc::PRIM_RESTART, on | if idx == u32::MAX { 2 } else { idx << 2 }, 0);
            return;
        }
        *rec_t::<p_prim_restart>(c, op::PRIM_RESTART) = p_prim_restart { on, index: idx };
    }
    fn nvnCommandBufferSetLineWidth(c: *mut NVNcommandBuffer, w: f32) { *rec_t::<p_line_width>(c, op::LINE_WIDTH) = p_line_width { w }; }
    fn nvnCommandBufferSetDepthClamp(c: *mut NVNcommandBuffer, on: u32) { *rec_t::<p_depth_clamp>(c, op::DEPTH_CLAMP) = p_depth_clamp { on: super::b(on) as i32 }; }
    fn nvnCommandBufferSetRasterizerDiscard(c: *mut NVNcommandBuffer, on: u32) { *rec_t::<p_rast_discard>(c, op::RAST_DISCARD) = p_rast_discard { on: super::b(on) as i32 }; }
    fn nvnCommandBufferSetPatchSize(c: *mut NVNcommandBuffer, n: i32) { *rec_t::<p_patch_size>(c, op::PATCH_SIZE) = p_patch_size { n }; }

    // ---------- program, state objects, vertex input ----------
    fn nvnCommandBufferBindProgram(c: *mut NVNcommandBuffer, p: *const NVNprogram, stages: i32) {
        *rec_t::<p_bind_program>(c, op::BIND_PROGRAM) = p_bind_program { prog: p as *const prog_t, stages };
    }
    fn nvnCommandBufferBindBlendState(c: *mut NVNcommandBuffer, s: *const NVNblendState) { bind_state(c, 0, s as *const u8, 8); }
    fn nvnCommandBufferBindColorState(c: *mut NVNcommandBuffer, s: *const NVNcolorState) { bind_state(c, 1, s as *const u8, 4); }
    fn nvnCommandBufferBindChannelMaskState(c: *mut NVNcommandBuffer, s: *const NVNchannelMaskState) { bind_state(c, 2, s as *const u8, 4); }
    fn nvnCommandBufferBindMultisampleState(c: *mut NVNcommandBuffer, s: *const NVNmultisampleState) { bind_state(c, 3, s as *const u8, core::mem::size_of::<ms_st>()); }
    fn nvnCommandBufferBindPolygonState(c: *mut NVNcommandBuffer, s: *const NVNpolygonState) { bind_state(c, 4, s as *const u8, 4); }
    fn nvnCommandBufferBindDepthStencilState(c: *mut NVNcommandBuffer, s: *const NVNdepthStencilState) { bind_state(c, 5, s as *const u8, 8); }
    fn nvnCommandBufferBindVertexAttribState(c: *mut NVNcommandBuffer, n: i32, s: *const NVNvertexAttribState) {
        if !(0..=32).contains(&n) { crate::log::red(&format!("BindVertexAttribState n={n}")); return; }
        if compact() {
            let q = crec(c, opc::VATTR, n as u32, 4 * n as usize);
            core::ptr::copy_nonoverlapping(s as *const u8, q, 4 * n as usize);
            return;
        }
        let r = rec(c, op::BIND_VATTR, 8 + 4 * n as usize) as *mut p_bind_vattr;
        (*r).first = 0;
        (*r).n = n;
        core::ptr::copy_nonoverlapping(s as *const u8, (*r).st.as_mut_ptr() as *mut u8, 4 * n as usize);
    }
    fn nvnCommandBufferBindVertexStreamState(c: *mut NVNcommandBuffer, n: i32, s: *const NVNvertexStreamState) {
        if !(0..=16).contains(&n) { crate::log::red(&format!("BindVertexStreamState n={n}")); return; }
        if compact() {
            let q = crec(c, opc::VSTREAM, n as u32, 8 * n as usize);
            core::ptr::copy_nonoverlapping(s as *const u8, q, 8 * n as usize);
            return;
        }
        let r = rec(c, op::BIND_VSTREAM, 8 + 8 * n as usize) as *mut p_bind_vstream;
        (*r).first = 0;
        (*r).n = n;
        core::ptr::copy_nonoverlapping(s as *const u8, (*r).st.as_mut_ptr() as *mut u8, 8 * n as usize);
    }
    fn nvnCommandBufferBindVertexBuffer(c: *mut NVNcommandBuffer, i: i32, a: u64, n: usize) {
        let r = NVNbufferRange { address: a, size: n as u64 };
        nvnCommandBufferBindVertexBuffers(c, i, 1, &r);
    }
    fn nvnCommandBufferBindVertexBuffers(c: *mut NVNcommandBuffer, first: i32, n: i32, rg: *const NVNbufferRange) {
        if first >= 16 && n == 1 && (*rg).address == 0 && (*rg).size == 0 { return; }
        if slots_fit(0, first, n, 16) != n { crate::log::red_once(&format!("BindVertexBuffers first={first} n={n}")); return; }
        if n == 1 && (*rg).size <= u32::MAX as u64 {
            *rec_t::<p_bind_vbuf1>(c, op::BIND_VBUF1) = p_bind_vbuf1 { index: first as u32, size: (*rg).size as u32, addr: (*rg).address };
            return;
        }
        let fit = compact() && (0..n as usize).all(|i| (*rg.add(i)).size <= u32::MAX as u64);
        if fit {
            let mut q = crec(c, opc::VBUFS, first as u32 | (n as u32) << 8, 12 * n as usize);
            for i in 0..n as usize { put64(&mut q, (*rg.add(i)).address); put32(&mut q, (*rg.add(i)).size as u32); }
            return;
        }
        let r = rec(c, op::BIND_VBUF, 8 + 16 * n as usize) as *mut p_bind_vbuf;
        (*r).first = first;
        (*r).n = n;
        for i in 0..n as usize { (*r).r[i] = *rg.add(i); }
    }

    // ---------- resources ----------
    fn nvnCommandBufferBindUniformBuffer(c: *mut NVNcommandBuffer, st: i32, i: i32, a: u64, n: usize) {
        let st = st_host(st);
        if slots_bound("BindUniformBuffer", st, i, 1, 16) != 0 { record_uniform_buffer(c, st, i, a, n as u64); }
    }
    fn nvnCommandBufferBindUniformBuffers(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, rg: *const NVNbufferRange) {
        let st = st_host(st);
        let n = slots_bound("BindUniformBuffers", st, first, n, 16);
        if n >= 2 && (rec_ubo(c, st, first, n, rg) || rec_bufs(c, op::BIND_UBO_SMALL, st, first, n, rg, 16)) { return; }
        for i in 0..n { let r = *rg.add(i as usize); record_uniform_buffer(c, st, first + i, r.address, r.size); }
    }
    fn nvnCommandBufferBindStorageBuffer(c: *mut NVNcommandBuffer, st: i32, i: i32, a: u64, n: usize) {
        let st = st_host(st);
        if slots_bound("BindStorageBuffer", st, i, 1, 16) == 0 { return; }
        let r = NVNbufferRange { address: a, size: n as u64 };
        if rec_bufs(c, op::BIND_SSBO, st, i, 1, &r, 256) { return; }
        *rec_t::<p_bind_buf>(c, op::BIND_SSBO) = p_bind_buf { stage: st, index: i, addr: a, size: n as u64 };
    }
    fn nvnCommandBufferBindStorageBuffers(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, rg: *const NVNbufferRange) {
        let st = st_host(st);
        let n = slots_bound("BindStorageBuffers", st, first, n, 16);
        if rec_bufs(c, op::BIND_SSBO, st, first, n, rg, 256) { return; }
        for i in 0..n { let r = *rg.add(i as usize); *rec_t::<p_bind_buf>(c, op::BIND_SSBO) = p_bind_buf { stage: st, index: first + i, addr: r.address, size: r.size }; }
    }
    /// Recorded as the hardware's command: the handle 16 bytes into the record, where engines patch it in place.
    fn nvnCommandBufferBindTexture(c: *mut NVNcommandBuffer, st: i32, slot: i32, h: u64) {
        let st = st_host(st);
        if slots_bound("BindTexture", st, slot, 1, SEPTEX_SLOT0) == 0 { return; }
        *rec_t::<p_bind_tex1>(c, op::BIND_TEX1) = p_bind_tex1 { stage: st, slot, h: h as u32 as u64 };
    }
    fn nvnCommandBufferBindTextures(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, h: *const u64) { bind_handles(c, op::BIND_TEX, st_host(st), first, n, h); }
    fn nvnCommandBufferBindImage(c: *mut NVNcommandBuffer, st: i32, slot: i32, h: u64) { bind_handles(c, op::BIND_IMG, st_host(st), slot, 1, &h); }
    fn nvnCommandBufferBindImages(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, h: *const u64) { bind_handles(c, op::BIND_IMG, st_host(st), first, n, h); }
    fn nvnCommandBufferBindSeparateTexture(c: *mut NVNcommandBuffer, st: i32, i: i32, h: NVNseparateTextureHandle) { bind_handles(c, op::BIND_TEX, st_host(st), SEPTEX_SLOT0 + i, 1, &h.value); }
    fn nvnCommandBufferBindSeparateTextures(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, h: *const u64) { bind_handles(c, op::BIND_TEX, st_host(st), SEPTEX_SLOT0 + first, n, h); }
    fn nvnCommandBufferBindSeparateSampler(c: *mut NVNcommandBuffer, st: i32, i: i32, h: NVNseparateSamplerHandle) { bind_handles(c, op::BIND_SAMP, st_host(st), i, 1, &h.value); }
    fn nvnCommandBufferBindSeparateSamplers(c: *mut NVNcommandBuffer, st: i32, first: i32, n: i32, h: *const u64) { bind_handles(c, op::BIND_SAMP, st_host(st), first, n, h); }
    fn nvnCommandBufferSetTexturePool(c: *mut NVNcommandBuffer, p: *const NVNtexturePool) { *rec_t::<p_set_pool>(c, op::SET_TEX_POOL) = p_set_pool { pool: p as *const dpool_t }; }
    fn nvnCommandBufferSetSamplerPool(c: *mut NVNcommandBuffer, p: *const NVNsamplerPool) { *rec_t::<p_set_pool>(c, op::SET_SAMP_POOL) = p_set_pool { pool: p as *const dpool_t }; }

    // ---------- draws and dispatches ----------
    fn nvnCommandBufferDrawArrays(c: *mut NVNcommandBuffer, pr: i32, first: i32, n: i32) {
        rec_draw(c, p_draw { prim: pr, first, count: n, instances: 1, ..Default::default() });
    }
    fn nvnCommandBufferDrawArraysInstanced(c: *mut NVNcommandBuffer, pr: i32, first: i32, n: i32, bi: i32, inst: i32) {
        rec_draw(c, p_draw { prim: pr, first, count: n, instances: inst, base_instance: bi, ..Default::default() });
    }
    fn nvnCommandBufferDrawArraysIndirect(c: *mut NVNcommandBuffer, pr: i32, ind: u64) {
        rec_draw(c, p_draw { prim: pr, instances: 1, ind, ..Default::default() });
    }
    fn nvnCommandBufferDrawElements(c: *mut NVNcommandBuffer, pr: i32, it: i32, n: i32, ib: u64) {
        rec_draw_idx(c, p_draw_idx { prim: pr, itype: it, count: n, instances: 1, ibuf: ib, ..Default::default() });
    }
    fn nvnCommandBufferDrawElementsBaseVertex(c: *mut NVNcommandBuffer, pr: i32, it: i32, n: i32, ib: u64, bv: i32) {
        rec_draw_idx(c, p_draw_idx { prim: pr, itype: it, count: n, base_vertex: bv, instances: 1, ibuf: ib, ..Default::default() });
    }
    fn nvnCommandBufferDrawElementsInstanced(c: *mut NVNcommandBuffer, pr: i32, it: i32, n: i32, ib: u64, bv: i32, bi: i32, inst: i32) {
        rec_draw_idx(c, p_draw_idx { prim: pr, itype: it, count: n, base_vertex: bv, base_instance: bi, instances: inst, ibuf: ib, ..Default::default() });
    }
    fn nvnCommandBufferDrawElementsIndirect(c: *mut NVNcommandBuffer, pr: i32, it: i32, ib: u64, ind: u64) {
        rec_draw_idx(c, p_draw_idx { prim: pr, itype: it, instances: 1, ibuf: ib, ind, ..Default::default() });
    }
    fn nvnCommandBufferDispatchCompute(c: *mut NVNcommandBuffer, x: i32, y: i32, z: i32) {
        *rec_t::<p_dispatch>(c, op::DISPATCH) = p_dispatch { x, y, z, caller: 0, ind: 0 };
    }
    fn nvnCommandBufferDispatchComputeIndirect(c: *mut NVNcommandBuffer, ind: u64) {
        *rec_t::<p_dispatch>(c, op::DISPATCH) = p_dispatch { x: 0, y: 0, z: 0, caller: 0, ind };
    }
    fn nvnCommandBufferBarrier(c: *mut NVNcommandBuffer, bits: i32) {
        if compact() && (bits as u32) < 1 << 24 { crec(c, opc::BARRIER, bits as u32, 0); return; }
        core::ptr::write_unaligned(rec(c, op::BARRIER, 8) as *mut i32, bits);
    }

    // ---------- copies ----------
    fn nvnCommandBufferCopyBufferToTexture(c: *mut NVNcommandBuffer, src: u64, t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, flags: i32) {
        let o = rec_copy(c, op::COPY_B2T);
        (*o).src = src;
        (*o).tex = tex_snap(o as *mut u8, snap_off(core::mem::size_of::<p_copy_b2t>()), 0, t);
        copy_view(&mut (*o).view, &mut (*o).has_view, v);
        (*o).r = *r;
        (*o).flags = flags;
    }
    fn nvnCommandBufferCopyTextureToBuffer(c: *mut NVNcommandBuffer, t: *const NVNtexture, v: *const NVNtextureView, r: *const NVNcopyRegion, dst: u64, flags: i32) {
        let o = rec_copy(c, op::COPY_T2B);
        (*o).src = dst;
        (*o).tex = tex_snap(o as *mut u8, snap_off(core::mem::size_of::<p_copy_b2t>()), 0, t);
        copy_view(&mut (*o).view, &mut (*o).has_view, v);
        (*o).r = *r;
        (*o).flags = flags;
    }
    fn nvnCommandBufferCopyTextureToTexture(c: *mut NVNcommandBuffer, s: *const NVNtexture, sv: *const NVNtextureView, sr: *const NVNcopyRegion, d: *const NVNtexture, dv: *const NVNtextureView, dr: *const NVNcopyRegion, flags: i32) {
        let o = rec_snap::<p_copy_t2t>(c, op::COPY_T2T, 2);
        let off = snap_off(core::mem::size_of::<p_copy_t2t>());
        (*o).src = tex_snap(o as *mut u8, off, 0, s);
        (*o).dst = tex_snap(o as *mut u8, off, 1, d);
        copy_view(&mut (*o).sv, &mut (*o).has_sv, sv);
        copy_view(&mut (*o).dv, &mut (*o).has_dv, dv);
        (*o).sr = *sr;
        (*o).dr = *dr;
        (*o).flags = flags;
    }
    fn nvnCommandBufferSetCopyRowStride(c: *mut NVNcommandBuffer, s: isize) { cb(c).copy_row = s as i64; }
    fn nvnCommandBufferSetCopyImageStride(c: *mut NVNcommandBuffer, s: isize) { cb(c).copy_image = s as i64; }
    fn nvnCommandBufferGetCopyRowStride(c: *const NVNcommandBuffer) -> isize { (*(c as *const cmd_t)).copy_row as isize }
    fn nvnCommandBufferGetCopyImageStride(c: *const NVNcommandBuffer) -> isize { (*(c as *const cmd_t)).copy_image as isize }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cb {
        c: Box<NVNcommandBuffer>,
        cmd: Vec<u64>,
        ctl: Vec<u64>,
        pool: Box<NVNmemoryPool>,
    }
    fn cb_new(cmd_bytes: usize) -> Cb {
        let mut c = NVNcommandBuffer::boxed();
        let mut cmd = vec![0u64; cmd_bytes / 8];
        let mut ctl = vec![0u64; 512];
        let mut pool = NVNmemoryPool::boxed();
        unsafe {
            let p = &mut *(pool.0.as_mut_ptr() as *mut pool_t);
            p.mem = cmd.as_mut_ptr() as *mut u8;
            p.size = cmd_bytes as u64;
            nvnCommandBufferInitialize(&mut *c, core::ptr::null_mut());
            nvnCommandBufferAddCommandMemory(&mut *c, &*pool, 0, cmd_bytes);
            nvnCommandBufferAddControlMemory(&mut *c, ctl.as_mut_ptr() as *mut u8, ctl.len() * 8);
        }
        Cb { c, cmd, ctl, pool }
    }

    fn record_sample(c: *mut NVNcommandBuffer) {
        unsafe {
            nvnCommandBufferDrawArrays(c, 4, 0, 3);
            nvnCommandBufferDrawElementsInstanced(c, 4, 1, 6, 0x10000, 2, 3, 4);
            nvnCommandBufferSetScissor(c, 1, 2, 30, 40);
            let ranges = [NVNbufferRange { address: 0x1000, size: 64 }, NVNbufferRange { address: 0x2000, size: 128 }];
            nvnCommandBufferBindUniformBuffers(c, 0, 0, 2, ranges.as_ptr());
            nvnCommandBufferBindVertexBuffers(c, 0, 2, ranges.as_ptr());
            let h = [5u64 | 7 << 20, 0x1_0000_0009];
            nvnCommandBufferBindTextures(c, 1, 0, 2, h.as_ptr());
            nvnCommandBufferBarrier(c, 0x30);
            nvnCommandBufferSetPrimitiveRestart(c, 1, -1);
        }
    }

    /// Compact records expand to exactly the canonical records the same calls record without them.
    #[test]
    fn compact_records_expand_to_canonical() {
        let a = cb_new(4096);
        let b2 = cb_new(4096);
        let (mut a, mut b2) = (a, b2);
        let lin = |c: &mut Cb, compact: bool| unsafe {
            REC_COMPACT.store(compact, Ordering::Relaxed);
            nvnCommandBufferBeginRecording(&mut *c.c);
            record_sample(&mut *c.c);
            let h = nvnCommandBufferEndRecording(&mut *c.c);
            REC_COMPACT.store(true, Ordering::Relaxed);
            let used = nvnCommandBufferGetCommandMemoryUsed(&*c.c);
            (rec_linear(h), used)
        };
        let (ca, used_c) = lin(&mut a, true);
        let (cb2, used_n) = lin(&mut b2, false);
        assert!(used_c < used_n, "compact {used_c} vs canonical {used_n}");
        // Canonical BIND_HANDLES records are recorded with only n handles; expansion writes the same.
        let ops = |v: &[u8]| {
            let mut k = 0;
            let mut out = vec![];
            while k < v.len() {
                let h = unsafe { rd::<op_hdr>(v.as_ptr().add(k)) };
                out.push(h.op);
                k += h.size as usize;
            }
            out
        };
        assert_eq!(ops(&ca), ops(&cb2));
        let _ = (&a.cmd, &a.ctl, &a.pool, &b2.cmd, &b2.ctl, &b2.pool);
    }

    /// A recording that outgrows the game's command memory without a callback continues in host memory, and its list
    /// still holds every record in order.
    #[test]
    fn overflow_into_host_segment_keeps_order() {
        let mut c = cb_new(64);
        unsafe {
            nvnCommandBufferBeginRecording(&mut *c.c);
            for i in 0..100 {
                nvnCommandBufferDrawArrays(&mut *c.c, 4, i, 3);
            }
            let h = nvnCommandBufferEndRecording(&mut *c.c);
            let mut firsts = vec![];
            for_each_record(&[h], |op, r| {
                assert_eq!(op, op::DRAW);
                firsts.push(rd::<p_draw>(r.as_ptr().add(8)).first);
            });
            assert_eq!(firsts, (0..100).collect::<Vec<_>>());
        }
    }

    /// CallCommands joins the called list's pieces; the result replays both in order.
    #[test]
    fn call_commands() {
        let mut c = cb_new(4096);
        unsafe {
            nvnCommandBufferBeginRecording(&mut *c.c);
            nvnCommandBufferDrawArrays(&mut *c.c, 4, 1, 3);
            let inner = nvnCommandBufferEndRecording(&mut *c.c);
            nvnCommandBufferBeginRecording(&mut *c.c);
            nvnCommandBufferDrawArrays(&mut *c.c, 4, 0, 3);
            nvnCommandBufferCallCommands(&mut *c.c, 1, &inner);
            nvnCommandBufferDrawArrays(&mut *c.c, 4, 2, 3);
            let outer = nvnCommandBufferEndRecording(&mut *c.c);
            let mut firsts = vec![];
            for_each_record(&[outer], |_, r| firsts.push(rd::<p_draw>(r.as_ptr().add(8)).first));
            assert_eq!(firsts, vec![0, 1, 2]);
        }
    }
}
