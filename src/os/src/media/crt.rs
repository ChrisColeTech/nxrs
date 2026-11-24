//! What the MinGW-built codec archives (third_party/) expect from their C runtime and that the MSVC one does not
//! have under the same name: GCC's stack probe `___chkstk_ms`, MinGW's `setjmp` / `longjmp` entries (libvpx's
//! error recovery: GCC's `__intrinsic_setjmpex`, clang's `_setjmp` import slot; a plain register save and restore,
//! no unwinding: the frames between are C) and `__mingw_vfprintf` (dav1d's log).

use std::ffi::{c_char, c_int, c_void};

std::arch::global_asm!(
    // GCC's ___chkstk_ms: probes each page of a frame of rax bytes below the caller's rsp; keeps every register.
    ".globl ___chkstk_ms",
    "___chkstk_ms:",
    "push rcx",
    "push rax",
    "cmp rax, 0x1000",
    "lea rcx, [rsp + 24]",
    "jb 2f",
    "1:",
    "sub rcx, 0x1000",
    "or qword ptr [rcx], 0",
    "sub rax, 0x1000",
    "cmp rax, 0x1000",
    "ja 1b",
    "2:",
    "sub rcx, rax",
    "or qword ptr [rcx], 0",
    "pop rax",
    "pop rcx",
    "ret",
    // setjmp(rcx = jmp_buf, rdx = frame, unused): the callee-saved registers, rsp after the return, the return
    // address, MXCSR and the x87 control word, xmm6-xmm15 (the 256-byte MinGW jmp_buf)
    ".globl __intrinsic_setjmpex",
    "__intrinsic_setjmpex:",
    "mov rdx, [rsp]",
    "lea r8, [rsp + 8]",
    "mov [rcx + 0x08], rbx",
    "mov [rcx + 0x10], r8",
    "mov [rcx + 0x18], rbp",
    "mov [rcx + 0x20], rsi",
    "mov [rcx + 0x28], rdi",
    "mov [rcx + 0x30], r12",
    "mov [rcx + 0x38], r13",
    "mov [rcx + 0x40], r14",
    "mov [rcx + 0x48], r15",
    "mov [rcx + 0x50], rdx",
    "stmxcsr [rcx + 0x58]",
    "fnstcw [rcx + 0x5c]",
    "movdqu [rcx + 0x60], xmm6",
    "movdqu [rcx + 0x70], xmm7",
    "movdqu [rcx + 0x80], xmm8",
    "movdqu [rcx + 0x90], xmm9",
    "movdqu [rcx + 0xa0], xmm10",
    "movdqu [rcx + 0xb0], xmm11",
    "movdqu [rcx + 0xc0], xmm12",
    "movdqu [rcx + 0xd0], xmm13",
    "movdqu [rcx + 0xe0], xmm14",
    "movdqu [rcx + 0xf0], xmm15",
    "xor eax, eax",
    "ret",
    // longjmp(rcx = jmp_buf, edx = value)
    ".globl nxos_mingw_longjmp",
    "nxos_mingw_longjmp:",
    "mov eax, edx",
    "test eax, eax",
    "jnz 3f",
    "mov eax, 1",
    "3:",
    "mov rbx, [rcx + 0x08]",
    "mov rbp, [rcx + 0x18]",
    "mov rsi, [rcx + 0x20]",
    "mov rdi, [rcx + 0x28]",
    "mov r12, [rcx + 0x30]",
    "mov r13, [rcx + 0x38]",
    "mov r14, [rcx + 0x40]",
    "mov r15, [rcx + 0x48]",
    "ldmxcsr [rcx + 0x58]",
    "fldcw [rcx + 0x5c]",
    "movdqu xmm6, [rcx + 0x60]",
    "movdqu xmm7, [rcx + 0x70]",
    "movdqu xmm8, [rcx + 0x80]",
    "movdqu xmm9, [rcx + 0x90]",
    "movdqu xmm10, [rcx + 0xa0]",
    "movdqu xmm11, [rcx + 0xb0]",
    "movdqu xmm12, [rcx + 0xc0]",
    "movdqu xmm13, [rcx + 0xd0]",
    "movdqu xmm14, [rcx + 0xe0]",
    "movdqu xmm15, [rcx + 0xf0]",
    "mov rsp, [rcx + 0x10]",
    "jmp qword ptr [rcx + 0x50]",
);

extern "C" {
    fn __intrinsic_setjmpex(buf: *mut c_void, frame: *mut c_void) -> c_int;
    fn nxos_mingw_longjmp(buf: *mut c_void, v: c_int) -> !;
    fn vfprintf(f: *mut c_void, fmt: *const c_char, ap: *mut c_void) -> c_int;
}

/// MinGW's import slot for `_setjmp(buf, frame)` (clang's setjmp): the same register save as `__intrinsic_setjmpex`.
#[no_mangle]
#[used]
pub static __imp__setjmp: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int =
    __intrinsic_setjmpex;

/// MinGW's import slot for longjmp.
#[no_mangle]
#[used]
pub static __imp_longjmp: unsafe extern "C" fn(*mut c_void, c_int) -> ! = nxos_mingw_longjmp;

#[no_mangle]
pub unsafe extern "C" fn __mingw_vfprintf(
    f: *mut c_void,
    fmt: *const c_char,
    ap: *mut c_void,
) -> c_int {
    vfprintf(f, fmt, ap)
}

/// Keeps this module's symbols in a link that otherwise reaches them only from the archives.
pub fn keep() -> usize {
    __imp_longjmp as *const () as usize
        ^ __imp__setjmp as *const () as usize
        ^ __mingw_vfprintf as *const () as usize
}

#[cfg(test)]
mod tests {

    #[inline(never)]
    unsafe fn jump(buf: *mut u8) -> ! {
        (super::__imp_longjmp)(buf as _, 7)
    }

    #[test]
    fn setjmp_longjmp() {
        let mut buf = [0u8; 256];
        let mut hits = 0;
        let hits_p = std::hint::black_box(&mut hits as *mut i32);
        unsafe {
            let r = (super::__imp__setjmp)(buf.as_mut_ptr() as _, std::ptr::null_mut());
            *hits_p += 1;
            if r == 0 {
                jump(buf.as_mut_ptr());
            }
            assert_eq!(r, 7);
        }
        assert_eq!(hits, 2);
    }
}
