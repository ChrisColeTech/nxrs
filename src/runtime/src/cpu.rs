//! The guest CPU state the generated C works on, mirrored from the `Cpu` struct of `cpu.h` (vendored by the
//! translator as `src/translator/runtime/cpu.h`). The layout is the ABI: the offsets are checked by the unit tests below and the C
//! side checks its own with `_Static_assert`.
//!
//! Also the AAPCS64 argument helpers host functions use.

use core::ffi::c_void;

/// `typedef struct __attribute__((aligned(16))) Cpu { ... } Cpu;`
#[repr(C, align(16))]
pub struct Cpu {
    /// x0-x30; x[31] unused (sp is separate). Offset 0.
    pub x: [u64; 32],
    /// 256
    pub sp: u64,
    /// 264: the target of an indirect branch, read by `rt_dispatch`.
    pub pc: u64,
    /// 272: NZCV in bits 31..28 (only outside function-local flags).
    pub nzcv: u64,
    /// 280: TPIDR_EL0, guest-writable thread pointer.
    pub tpidr: u64,
    /// 288: TPIDRRO_EL0, the thread's 0x200-byte TLS region (set by the kernel).
    pub tpidrro: u64,
    /// 296: exclusive monitor address (~0 = none).
    pub excl_addr: u64,
    /// 304
    pub excl_val: u64,
    /// 312: the runtime's thread object (`kernel::Thread`).
    pub thread: *mut c_void,
    /// 320
    pub fpcr: u64,
    /// 328
    pub fpsr: u64,
    /// 336: upper half of a 128-bit exclusive pair.
    pub excl_val_hi: u64,
    /// 344: the newest live setjmp owner of this guest thread (`sjlj.rs`).
    pub sj: *mut c_void,
    /// 352: the exclusive monitor bucket's sequence at the last LDXR (`rt_excl_mon`).
    pub excl_seq: u32,
    pub _pad: [u8; 512 - 356],
    /// 512: v0-v31.
    pub v: [u128; 32],
}

/// The global exclusive monitor (cpu.h `rt_excl_mon`): one sequence number per 16-byte address bucket, 2^16
/// buckets. NULL: STXR compares values only (Switch 2 programs; see `set_exclusive_monitor`).
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static mut rt_excl_mon: *mut u32 = core::ptr::null_mut();

/// Turns the global exclusive monitor on or off; call before any guest code runs.
pub fn set_exclusive_monitor(on: bool) {
    static TABLE: [core::sync::atomic::AtomicU32; 1 << 16] = [const { core::sync::atomic::AtomicU32::new(0) }; 1 << 16];
    let p = if on { TABLE.as_ptr() as *mut u32 } else { core::ptr::null_mut() };
    // SAFETY: start-up, before guest threads run
    unsafe { core::ptr::addr_of_mut!(rt_excl_mon).write_volatile(p) }
}

/// Whether the global exclusive monitor is on.
pub fn exclusive_monitor() -> bool {
    // SAFETY: a pointer read
    unsafe { !core::ptr::addr_of!(rt_excl_mon).read_volatile().is_null() }
}

/// `typedef void (*GuestFn)(Cpu *);`
pub type GuestFn = unsafe extern "C" fn(*mut Cpu);

pub const OFF_SP: usize = 256;
pub const OFF_PC: usize = 264;
pub const OFF_SJ: usize = 344;

impl Cpu {
    /// A zeroed state with the exclusive monitor clear, boxed (the struct is 1 KiB and 16-aligned).
    pub fn new_boxed() -> Box<Cpu> {
        // SAFETY: every field is an integer, a raw pointer or a byte array: all-zero is valid.
        let mut c: Box<Cpu> = unsafe { Box::new(core::mem::zeroed()) };
        c.excl_addr = !0;
        c
    }

    /// Argument `i` of an AAPCS64 call: x0-x7, then the stack.
    pub fn arg_x(&self, i: usize) -> u64 {
        if i < 8 {
            self.x[i]
        } else {
            // SAFETY: guest addresses are host addresses; the caller's stack is mapped.
            unsafe { core::ptr::read_unaligned((self.sp + 8 * (i as u64 - 8)) as *const u64) }
        }
    }
    pub fn arg_w(&self, i: usize) -> u32 {
        self.arg_x(i) as u32
    }
    pub fn arg_s(&self, i: usize) -> f32 {
        f32::from_bits(self.v[i] as u32)
    }
    pub fn arg_d(&self, i: usize) -> f64 {
        f64::from_bits(self.v[i] as u64)
    }
    pub fn ret_x(&mut self, v: u64) {
        self.x[0] = v;
    }
    pub fn ret_d(&mut self, v: f64) {
        self.v[0] = v.to_bits() as u128;
    }
    pub fn ret_s(&mut self, v: f32) {
        self.v[0] = v.to_bits() as u128;
    }

    /// MXCSR of a host thread running guest code mirrors FPCR: FTZ|DAZ = FPCR.FZ, RC = nearest (cpu.h
    /// `rt_fpcr_sync`, x86 branch). Set wherever the host FP context changes under the guest (thread entry, a
    /// longjmp back to a setjmp owner, a fiber switch).
    pub fn sync_mxcsr(&self) {
        #[cfg(target_arch = "x86_64")]
        #[allow(deprecated)]
        unsafe {
            use core::arch::x86_64::{_mm_getcsr, _mm_setcsr};
            let cur = _mm_getcsr();
            let want = (cur & !0xE040) | if self.fpcr & (1 << 24) != 0 { 0x8040 } else { 0 };
            if cur != want {
                _mm_setcsr(want);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn layout_matches_cpu_h() {
        assert_eq!(offset_of!(Cpu, x), 0);
        assert_eq!(offset_of!(Cpu, sp), 256);
        assert_eq!(offset_of!(Cpu, pc), 264);
        assert_eq!(offset_of!(Cpu, nzcv), 272);
        assert_eq!(offset_of!(Cpu, tpidr), 280);
        assert_eq!(offset_of!(Cpu, tpidrro), 288);
        assert_eq!(offset_of!(Cpu, excl_addr), 296);
        assert_eq!(offset_of!(Cpu, excl_val), 304);
        assert_eq!(offset_of!(Cpu, thread), 312);
        assert_eq!(offset_of!(Cpu, fpcr), 320);
        assert_eq!(offset_of!(Cpu, fpsr), 328);
        assert_eq!(offset_of!(Cpu, excl_val_hi), 336);
        assert_eq!(offset_of!(Cpu, sj), 344);
        assert_eq!(offset_of!(Cpu, v), 512);
        assert_eq!(size_of::<Cpu>(), 1024);
        assert_eq!(align_of::<Cpu>(), 16);
        assert_eq!(OFF_SP, offset_of!(Cpu, sp));
        assert_eq!(OFF_PC, offset_of!(Cpu, pc));
        assert_eq!(OFF_SJ, offset_of!(Cpu, sj));
    }

    /// The vendored cpu.h states the same offsets: a changed header fails here before it fails at run time.
    #[test]
    fn header_offsets_agree() {
        let h = include_str!("../../translator/runtime/cpu.h");
        for (field, off) in [("sp", 256), ("pc", 264), ("nzcv", 272), ("fpcr", 320), ("fpsr", 328), ("v", 512)] {
            let want = format!("_Static_assert(offsetof(Cpu, {field}) == {off}");
            assert!(h.contains(&want), "cpu.h no longer asserts {field} at {off}");
        }
        assert!(h.contains("void *sj;                // 344"));
    }

    #[test]
    fn args() {
        let mut c = Cpu::new_boxed();
        let stack = [0u64, 11, 12];
        c.sp = stack.as_ptr() as u64 + 8;
        c.x[3] = 0xFFFF_FFFF_0000_0005;
        assert_eq!(c.arg_w(3), 5);
        assert_eq!(c.arg_x(8), 11);
        assert_eq!(c.arg_x(9), 12);
        c.v[1] = 2.5f64.to_bits() as u128;
        assert_eq!(c.arg_d(1), 2.5);
        c.ret_s(1.5);
        assert_eq!(c.arg_s(0), 1.5);
        assert_eq!(c.excl_addr, !0);
    }
}
