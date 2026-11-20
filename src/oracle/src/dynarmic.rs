//! dynarmic's C entry points (the crate's vendored `dynarmic.cpp`), declared here directly: the crate's safe wrapper
//! does not expose the vector registers, FPCR, FPSR or PSTATE, which its context save / restore carries.

use std::ffi::c_void;

/// `struct context64` of the vendored dynarmic.h.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Context64 {
    pub sp: u64,
    pub pc: u64,
    pub x: [u64; 31],
    pub v: [[u64; 2]; 32],
    pub fpcr: u32,
    pub fpsr: u32,
    pub pstate: u32,
    pub tpidr_el0: u64,
    pub tpidrro_el0: u64,
}

const _: () = assert!(core::mem::size_of::<Context64>() == 808);

extern "C" {
    fn dynarmic_init_memory() -> *mut c_void;
    fn dynarmic_init_monitor(processor_count: u32) -> *mut c_void;
    fn dynarmic_init_page_table() -> *mut *mut c_void;
    fn dynarmic_new(id: u32, memory: *mut c_void, monitor: *mut c_void, pt: *mut *mut c_void, jit_size: u64, unsafe_opt: bool) -> *mut c_void;
    fn dynarmic_mmap(d: *mut c_void, address: u64, size: u64, perms: i32) -> i32;
    fn dynarmic_mem_write(d: *mut c_void, address: u64, data: *const u8, size: usize) -> i32;
    fn dynarmic_mem_read(d: *mut c_void, address: u64, data: *mut u8, size: usize) -> i32;
    fn dynarmic_invalidate_cache_range(d: *mut c_void, address: u64, size: u64);
    fn dynarmic_emu_start_bounded(d: *mut c_void, pc: u64, ticks: u64) -> i32;
    fn dynarmic_context_restore(d: *mut c_void, ctx: *const Context64) -> i32;
    fn dynarmic_context_save(d: *mut c_void, ctx: *mut Context64) -> i32;
}

pub struct Jit(*mut c_void);

impl Jit {
    /// An A64 JIT with accurate settings (no unsafe optimizations: FMA stays fused, NaNs as the architecture says).
    pub fn new() -> Jit {
        use dynarmic_sys as _; // links the crate's static dynarmic libraries
        // SAFETY: the vendored constructors return owned objects that live for the process
        unsafe {
            let mem = dynarmic_init_memory();
            let mon = dynarmic_init_monitor(1);
            let pt = dynarmic_init_page_table();
            let d = dynarmic_new(0, mem, mon, pt, 64 << 20, false);
            assert!(!d.is_null(), "dynarmic_new");
            Jit(d)
        }
    }
    pub fn map(&self, address: u64, size: u64) {
        // SAFETY: a valid handle; dynarmic allocates the pages
        let r = unsafe { dynarmic_mmap(self.0, address, size, 7) };
        assert_eq!(r, 0, "dynarmic_mmap {address:#x}");
    }
    pub fn write(&self, address: u64, data: &[u8]) {
        // SAFETY: valid handle and slice
        let r = unsafe { dynarmic_mem_write(self.0, address, data.as_ptr(), data.len()) };
        assert_eq!(r, 0, "dynarmic_mem_write {address:#x}");
    }
    pub fn read(&self, address: u64, out: &mut [u8]) {
        // SAFETY: valid handle and slice
        let r = unsafe { dynarmic_mem_read(self.0, address, out.as_mut_ptr(), out.len()) };
        assert_eq!(r, 0, "dynarmic_mem_read {address:#x}");
    }
    pub fn invalidate(&self, address: u64, size: u64) {
        // SAFETY: valid handle
        unsafe { dynarmic_invalidate_cache_range(self.0, address, size) }
    }
    /// Runs from ctx.pc for at most one block of `ticks` instructions and returns the state after.
    pub fn run(&self, ctx: &Context64, ticks: u64) -> Context64 {
        let mut out = *ctx;
        // SAFETY: valid handle and contexts
        unsafe {
            dynarmic_context_restore(self.0, ctx);
            dynarmic_emu_start_bounded(self.0, ctx.pc, ticks);
            dynarmic_context_save(self.0, &mut out);
        }
        out
    }
}
