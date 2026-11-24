//! Guest memory and call helpers for host functions. Guest addresses are host addresses (the runtime's identity
//! map), so these are plain unaligned loads and stores; the heap's pages are committed on first touch
//! (`mem::prepare` commits a range up front before an OS call writes into it).

use nxrt::boundary::{call_guest, GuestCall};
use nxrt::cpu::Cpu;

pub fn rd<T: Copy>(a: u64) -> T {
    unsafe { core::ptr::read_unaligned(a as *const T) }
}
pub fn wr<T: Copy>(a: u64, v: T) {
    unsafe { core::ptr::write_unaligned(a as *mut T, v) }
}
pub fn r8(a: u64) -> u8 {
    rd(a)
}
pub fn r16(a: u64) -> u16 {
    rd(a)
}
pub fn r32(a: u64) -> u32 {
    rd(a)
}
pub fn r64(a: u64) -> u64 {
    rd(a)
}
pub fn w8(a: u64, v: u8) {
    wr(a, v)
}
pub fn w16(a: u64, v: u16) {
    wr(a, v)
}
pub fn w32(a: u64, v: u32) {
    wr(a, v)
}
pub fn w64(a: u64, v: u64) {
    wr(a, v)
}

/// Writes `v` through a guest out-pointer, unless it is null.
pub fn out<T: Copy>(a: u64, v: T) {
    if a != 0 {
        wr(a, v)
    }
}

pub fn zero(a: u64, n: u64) {
    if a != 0 && n != 0 {
        crate::mem::prepare(a, n);
        unsafe { core::ptr::write_bytes(a as *mut u8, 0, n as usize) }
    }
}

pub fn bytes<'a>(a: u64, n: u64) -> &'a [u8] {
    if a == 0 || n == 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) }
}

pub fn bytes_mut<'a>(a: u64, n: u64) -> &'a mut [u8] {
    if a == 0 || n == 0 {
        return &mut [];
    }
    crate::mem::prepare(a, n);
    unsafe { core::slice::from_raw_parts_mut(a as *mut u8, n as usize) }
}

pub fn write_bytes(a: u64, b: &[u8]) {
    bytes_mut(a, b.len() as u64).copy_from_slice(b);
}

/// A NUL-terminated guest string (at most `max` bytes), lossily as UTF-8.
pub fn cstr(a: u64, max: usize) -> String {
    if a == 0 {
        return String::new();
    }
    let mut v = vec![];
    for i in 0..max as u64 {
        let b = r8(a + i);
        if b == 0 {
            break;
        }
        v.push(b);
    }
    String::from_utf8_lossy(&v).into_owned()
}

/// Writes `s` NUL-terminated into a guest buffer of `cap` bytes (truncated to fit).
pub fn put_cstr(a: u64, cap: u64, s: &str) {
    if a == 0 || cap == 0 {
        return;
    }
    let n = s.len().min(cap as usize - 1);
    write_bytes(a, &s.as_bytes()[..n]);
    w8(a + n as u64, 0);
}

/// The arguments of a host function, AAPCS64: x0-x7 then the guest stack; narrow integers are truncated here.
pub struct Args<'a>(pub &'a mut Cpu);

impl<'a> Args<'a> {
    pub fn x(&self, i: usize) -> u64 {
        self.0.arg_x(i)
    }
    pub fn w(&self, i: usize) -> u32 {
        self.0.arg_x(i) as u32
    }
    pub fn i(&self, i: usize) -> i32 {
        self.0.arg_x(i) as i32
    }
    pub fn b(&self, i: usize) -> bool {
        self.0.arg_x(i) as u8 != 0
    }
    pub fn f(&self, i: usize) -> f32 {
        self.0.arg_s(i)
    }
    pub fn d(&self, i: usize) -> f64 {
        self.0.arg_d(i)
    }
    pub fn ret(&mut self, v: u64) {
        self.0.x[0] = v
    }
    pub fn ret2(&mut self, a: u64, b: u64) {
        self.0.x[0] = a;
        self.0.x[1] = b;
    }
    pub fn ret_f(&mut self, v: f32) {
        self.0.ret_s(v)
    }
    pub fn lr(&self) -> u64 {
        self.0.x[30]
    }
}

/// Calls the guest function at `addr` with integer arguments, on the calling guest thread, keeping the caller's
/// callee-saved registers. Returns x0, or None when a guest jump left the call (the host function must then return
/// at once).
///
/// # Safety
/// `c` is the calling thread's own guest state; the guest stack below sp is free.
pub unsafe fn call(c: *mut Cpu, addr: u64, args: &[u64]) -> Option<u64> {
    let saved: [u64; 32] = (*c).x;
    let sp = (*c).sp;
    for (i, &a) in args.iter().take(8).enumerate() {
        (*c).x[i] = a;
    }
    let r = call_guest(c, addr);
    if r != GuestCall::Returned {
        return None;
    }
    let ret = (*c).x[0];
    let ret1 = (*c).x[1];
    (*c).x = saved;
    (*c).sp = sp;
    (*c).x[0] = ret;
    (*c).x[1] = ret1;
    Some(ret)
}

/// Calls a guest function on the calling guest thread (its own guest state), for callers that hold no `Cpu`
/// (the graphics layer's command-memory callbacks: `nxgfx::nvn::set_guest_caller`). Returns x0, or 0 when the
/// thread runs no guest code or a guest jump left the call.
pub fn call_current(addr: u64, args: &[u64]) -> u64 {
    match nxrt::kernel::current_cpu() {
        Some(c) => unsafe { call(c, addr, args) }.unwrap_or(0),
        None => {
            crate::log_once!(
                "call_current",
                "[RED] a guest call from a host thread that runs no guest code ({addr:#x})"
            );
            0
        }
    }
}

/// A guest thread made with the SDK's own nn::os::CreateThread (malloc'd ThreadType and stack), running the host
/// function `entry` with `arg` in x0, started at once. For host work that must call guest code (signalling an
/// nn::os event, the renderer's frames). Returns the ThreadType.
///
/// # Safety
/// `c` is the calling guest thread's state.
pub unsafe fn spawn(
    c: *mut Cpu,
    name: &str,
    entry: nxrt::hostfn::HostFn,
    arg: u64,
    prio: u64,
    stack_size: u64,
) -> Option<u64> {
    let sym = |n: &str| crate::hle::sym(n);
    let (malloc, create, start) = (
        sym("malloc")?,
        sym("_ZN2nn2os12CreateThreadEPNS0_10ThreadTypeEPFvPvES3_S3_mi")?,
        sym("_ZN2nn2os11StartThreadEPNS0_10ThreadTypeE")?,
    );
    let tt = call(c, malloc, &[0x200])?;
    zero(tt, 0x200);
    let sa = call(c, malloc, &[stack_size + 0x1000])?;
    let stack = (sa + 0xfff) & !0xfff;
    let e = crate::hle::addr(name, entry);
    let rc = call(c, create, &[tt, e, arg, stack, stack_size, prio])?;
    if rc as u32 != 0 {
        crate::log!("[RED] {name}: CreateThread failed {rc:#x}");
        return None;
    }
    call(c, start, &[tt])?;
    Some(tt)
}

/// True when `a` is committed, readable memory (a pointer the guest handed over that may be stale).
#[cfg(target_os = "linux")]
pub fn readable(a: u64, n: u64) -> bool {
    a >= 0x1000 && nxrt::mem::readable(a, n)
}

/// True when `a` is committed, readable memory (a pointer the guest handed over that may be stale).
#[cfg(windows)]
pub fn readable(a: u64, n: u64) -> bool {
    use windows_sys::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS,
    };
    if a < 0x1000 {
        return false;
    }
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    let got = unsafe {
        VirtualQuery(
            a as *const _,
            &mut m,
            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    got != 0
        && m.State == MEM_COMMIT
        && m.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0
        && a + n <= m.BaseAddress as u64 + m.RegionSize as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_and_values() {
        let mut buf = [0u8; 32];
        let a = buf.as_mut_ptr() as u64;
        put_cstr(a, 8, "abcdefghij");
        assert_eq!(cstr(a, 32), "abcdefg");
        w64(a + 9, 0x1122334455667788);
        assert_eq!(r64(a + 9), 0x1122334455667788);
        assert_eq!(r8(a + 9), 0x88);
        out::<u32>(0, 5); // null out-pointer is skipped
    }
}
