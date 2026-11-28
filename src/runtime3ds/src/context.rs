//! the contract between the generated C and this host, as Rust: recomp.h v8 verbatim in shape and field
//! order (the donor's `abi/src/lib.rs` is the reference twin), the code includes the C header
//! (`translator3ds::abi::HEADER`) and this mirror has to describe the same memory. A test compiles the
//! header and holds the two to one layout, the way the donor's twin does.

/// recomp.h, the generated code includes it
pub const HEADER: &str = translator3ds::abi::HEADER;

/// interface version, code has to match it to run (recomp.h's RECOMP_ABI)
pub const ABI: u32 = translator3ds::abi::ABI;

/// oldest version code can be made for and still run, v4 left every svc to the host's loop
pub const OLDEST_ABI: u32 = 4;

/// codegen version, bumped when its code runs way better or does more
pub const GENERATION: u32 = translator3ds::abi::GENERATION;

/// why the code handed control back
pub const EXIT_NONE: u32 = 0;
pub const EXIT_SVC: u32 = 1;
pub const EXIT_BUDGET: u32 = 2;
pub const EXIT_UNWIND: u32 = 3;

/// what code returns when it stopped for the host, an addr no code returns to
pub const STOPPED: u32 = u32::MAX;

/// how many self answered svc addrs the code keeps (recomp.h's RECOMP_TRAIL)
pub const TRAIL: usize = 64;

/// a tick_at where no code answers GetSystemTick
pub const NO_TICKS: u32 = 0;

/// a held_floor that makes the code answer no wait or release itself
pub const NOTHING_HELD: i32 = i32::MAX;

/// how much C stack the code gets below where the host enters it, thousands of guest calls deep
pub const STACK: usize = 1 << 20;

/// an entry's owner when handwritten code runs it
pub const NO_ORIGIN: u32 = translator3ds::abi::NO_ORIGIN;

/// an addr no code starts at, the start a stale func gets
pub const NO_START: u32 = u32::MAX;

/// runs the guest from r15, gives back where it went on return (the value written to pc, Thumb in bit 0)
/// or STOPPED when the host takes over; hosts go by exit and the context, never by the return value
pub type Code = unsafe extern "C" fn(*mut Context) -> u32;

#[repr(C)]
pub struct Host {
    pub read8: unsafe extern "C" fn(*mut Context, u32) -> u8,
    pub read16: unsafe extern "C" fn(*mut Context, u32) -> u16,
    pub read32: unsafe extern "C" fn(*mut Context, u32) -> u32,
    pub write8: unsafe extern "C" fn(*mut Context, u32, u8),
    pub write16: unsafe extern "C" fn(*mut Context, u32, u16),
    pub write32: unsafe extern "C" fn(*mut Context, u32, u32),
    /// runs one instruction the recompiler punted to the interpreter
    pub interpret: unsafe extern "C" fn(*mut Context, u32, u32),
    /// code for an addr (bit 0 means Thumb) or null
    pub lookup: Option<unsafe extern "C" fn(*mut Context, u32) -> Option<Code>>,
    /// runs an svc w/ r15 after it
    pub svc: unsafe extern "C" fn(*mut Context, u32),
}

#[repr(C)]
pub struct Context {
    pub r: [u32; 16],
    pub n: u8,
    pub z: u8,
    pub c: u8,
    pub v: u8,
    pub q: u8,
    pub thumb: u8,
    pub ge: u8,
    /// did ldrex mark exclusive_address
    pub exclusive: u8,
    pub budget: i32,
    pub exit: u32,
    pub svc: u32,
    /// what pre v6 code counted calls in, 0 before each call
    pub depth: u32,
    pub exclusive_address: u32,
    /// the readonly thread id reg, the thread's TLS addr
    pub tls: u32,
    pub read_pages: *const *mut u8,
    pub write_pages: *const *mut u8,
    /// VFP regs s0 to s31 (dN lives in s2N and s2N+1)
    pub vfp: *mut u32,
    pub fpscr: *mut u32,
    pub host: *const Host,
    /// belongs to the host, the code never touches it
    pub user: *mut core::ffi::c_void,
    /// lowest addr the C stack may reach, funcs check it on the way in
    pub stack_limit: usize,
    /// from v7 the code answers GetSystemTick itself, see docs/interface.md §Ticks
    pub tick_end: u64,
    pub tick_last: u64,
    pub tick_floor: i32,
    pub tick_cost: i32,
    pub tick_at: u32,
    pub ticks: u32,
    /// from v8 the code takes and drops the one mutex the host lends it, see §A mutex lent
    pub wait_at: u32,
    pub release_at: u32,
    pub held: u32,
    pub holding: u32,
    pub held_floor: i32,
    pub done: u32,
    pub trail: [u32; TRAIL],
}

/// a stack_limit giving the code STACK bytes of C stack below the caller's frame, set it before each call
#[inline(always)]
pub fn stack_limit() -> usize {
    let here = 0u8;
    (core::ptr::addr_of!(here) as usize).saturating_sub(STACK)
}

#[repr(C)]
pub struct Entry {
    pub address: u32,
    pub code: Code,
}

/// a module's code, entries are offsets from its load addr
#[repr(C)]
pub struct Module {
    pub(crate) name: *const core::ffi::c_char,
    /// where the host loaded it, the code reads this
    pub(crate) base: *mut u32,
    pub size: u32,
    pub(crate) count: u32,
    pub(crate) entries: *const Entry,
}

/// a chunk of guest code (addrs or module offsets) plus its bytes' hash
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Piece {
    pub start: u32,
    pub end: u32,
    pub hash: u64,
}

/// a run of a unit's pieces, as indices
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Run {
    pub first: u32,
    pub end: u32,
}

/// a func's code, count runs from the unit's runs[first], plus where it starts
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Origin {
    pub first: u32,
    pub count: u32,
    pub start: u32,
}

/// code of the exe or a module, its funcs, the func each entry runs, and a stale flag per func
#[repr(C)]
pub struct Origins {
    pub(crate) count: u32,
    pub(crate) piece_count: u32,
    pub(crate) pieces: *const Piece,
    pub(crate) functions: *const Origin,
    pub(crate) runs: *const Run,
    pub(crate) owners: *const u32,
    /// the host sets a func's flag when the code in memory changed
    pub stale: *mut u8,
    /// the start each func compares r15 with on the way in
    pub starts: *mut u32,
}

pub type CodeHash = translator3ds::abi::CodeHash;

impl Module {
    /// # Safety
    ///
    /// the generated table only holds string literals and counts, alive as long as the code
    pub unsafe fn name(&self) -> &'static str {
        let bytes = unsafe { core::ffi::CStr::from_ptr(self.name) }.to_bytes();
        core::str::from_utf8(bytes).unwrap_or_default()
    }

    /// # Safety
    ///
    /// the table lives as long as the code
    pub unsafe fn entries(&self) -> &[Entry] {
        unsafe { core::slice::from_raw_parts(self.entries, self.count as usize) }
    }

    /// tells the module's code where it got loaded, 0 when it goes away
    pub fn place(&self, base: u32) {
        // safe bc base points at the module's var next to the code, read once per entry
        unsafe { *self.base = base };
    }
}

impl Origins {
    /// # Safety
    ///
    /// the generated table has piece_count pieces, alive as long as the code
    pub unsafe fn pieces(&self) -> &[Piece] {
        if self.pieces.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.pieces, self.piece_count as usize) }
    }

    /// # Safety
    ///
    /// the generated table has count funcs, alive as long as the code
    pub unsafe fn functions(&self) -> &[Origin] {
        if self.functions.is_null() {
            return &[];
        }
        unsafe { core::slice::from_raw_parts(self.functions, self.count as usize) }
    }

    /// # Safety
    ///
    /// the funcs' runs are all in the table, alive as long as the code
    pub unsafe fn runs(&self) -> &[Run] {
        if self.runs.is_null() {
            return &[];
        }
        let count = unsafe { self.functions() }
            .iter()
            .map(|f| f.first + f.count)
            .max()
            .unwrap_or(0) as usize;
        unsafe { core::slice::from_raw_parts(self.runs, count) }
    }

    /// is the func running entry i stale
    pub fn owner_is_stale(&self, entry: usize) -> bool {
        if self.owners.is_null() {
            return false;
        }
        // safe bc every entry of the unit has an owner and every func a flag
        unsafe {
            let owner = *self.owners.add(entry);
            owner != NO_ORIGIN && *self.stale.add(owner as usize) != 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// prints the header's layout the way the lines below expect (the donor twin's probe)
    const LAYOUT: &str = r#"
#include <stddef.h>
#include <stdio.h>
#include "recomp.h"

#define SIZE(type) printf(#type " %zu\n", sizeof(type))
#define AT(type, field) printf(#type "." #field " %zu\n", offsetof(type, field))

int main(void) {
    SIZE(Context);
    AT(Context, r); AT(Context, n); AT(Context, z); AT(Context, c); AT(Context, v);
    AT(Context, q); AT(Context, thumb); AT(Context, ge); AT(Context, exclusive);
    AT(Context, budget); AT(Context, exit); AT(Context, svc); AT(Context, depth);
    AT(Context, exclusive_address); AT(Context, tls); AT(Context, read_pages);
    AT(Context, write_pages); AT(Context, vfp); AT(Context, fpscr); AT(Context, host);
    AT(Context, user); AT(Context, stack_limit); AT(Context, tick_end); AT(Context, tick_last);
    AT(Context, tick_floor); AT(Context, tick_cost); AT(Context, tick_at); AT(Context, ticks);
    AT(Context, wait_at); AT(Context, release_at); AT(Context, held); AT(Context, holding);
    AT(Context, held_floor); AT(Context, done); AT(Context, trail);
    SIZE(Host);
    AT(Host, read8); AT(Host, read16); AT(Host, read32); AT(Host, write8);
    AT(Host, write16); AT(Host, write32); AT(Host, interpret); AT(Host, lookup); AT(Host, svc);
    SIZE(Entry);
    AT(Entry, address); AT(Entry, code);
    SIZE(Module);
    AT(Module, name); AT(Module, base); AT(Module, size); AT(Module, count); AT(Module, entries);
    SIZE(Piece);
    AT(Piece, start); AT(Piece, end); AT(Piece, hash);
    SIZE(Run);
    AT(Run, first); AT(Run, end);
    SIZE(Origin);
    AT(Origin, first); AT(Origin, count); AT(Origin, start);
    SIZE(Origins);
    AT(Origins, count); AT(Origins, piece_count); AT(Origins, pieces); AT(Origins, functions);
    AT(Origins, runs); AT(Origins, owners); AT(Origins, stale); AT(Origins, starts);
    printf("NO_ORIGIN %u\nNO_START %u\n", NO_ORIGIN, NO_START);
    printf("RECOMP_ABI %d\nRECOMP_STOPPED %u\nRECOMP_TRAIL %d\n", RECOMP_ABI, RECOMP_STOPPED, RECOMP_TRAIL);
    printf("EXIT_NONE %d\nEXIT_SVC %d\nEXIT_BUDGET %d\nEXIT_UNWIND %d\n", EXIT_NONE, EXIT_SVC, EXIT_BUDGET, EXIT_UNWIND);
    return 0;
}
"#;

    /// recomp.h and this mirror describe the same memory, a C program built from the real header reports it
    #[test]
    fn the_header_has_the_same_layout() {
        let Some(compiler) = translator3ds::compile::Compiler::find().filter(|c| c.runs()) else {
            eprintln!("skipping: there is no C compiler, the header's layout went unchecked");
            return;
        };
        let dir = std::env::temp_dir().join(format!("runtime3ds-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("recomp.h"), HEADER).unwrap();
        std::fs::write(dir.join("layout.c"), LAYOUT).unwrap();
        let exe = dir.join("layout.exe");
        let mut compile = std::process::Command::new(&compiler.program);
        compile.args(&compiler.leading);
        let built = compile
            .args(["-w", "-o"])
            .arg(&exe)
            .arg(dir.join("layout.c"))
            // the header's VFP helpers call sqrt: libm is its own library on unix
            .args(if cfg!(unix) { &["-lm"][..] } else { &[] })
            .status()
            .is_ok_and(|s| s.success());
        assert!(built, "the layout probe did not compile");
        let output = std::process::Command::new(&exe).output().unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let header: Vec<String> =
            String::from_utf8(output.stdout).unwrap().lines().map(str::to_owned).collect();

        let mut lines = Vec::new();
        let mut at = |t: &str, fields: &[(&str, usize)]| {
            lines.push(format!("{} {}", t, fields.iter().find(|(f, _)| f.is_empty()).map_or(0, |(_, s)| *s)));
            for (field, offset) in fields.iter().filter(|(f, _)| !f.is_empty()) {
                lines.push(format!("{}.{} {}", t, field, offset));
            }
        };
        at(
            "Context",
            &[
                ("", size_of::<Context>()),
                ("r", offset_of!(Context, r)),
                ("n", offset_of!(Context, n)),
                ("z", offset_of!(Context, z)),
                ("c", offset_of!(Context, c)),
                ("v", offset_of!(Context, v)),
                ("q", offset_of!(Context, q)),
                ("thumb", offset_of!(Context, thumb)),
                ("ge", offset_of!(Context, ge)),
                ("exclusive", offset_of!(Context, exclusive)),
                ("budget", offset_of!(Context, budget)),
                ("exit", offset_of!(Context, exit)),
                ("svc", offset_of!(Context, svc)),
                ("depth", offset_of!(Context, depth)),
                ("exclusive_address", offset_of!(Context, exclusive_address)),
                ("tls", offset_of!(Context, tls)),
                ("read_pages", offset_of!(Context, read_pages)),
                ("write_pages", offset_of!(Context, write_pages)),
                ("vfp", offset_of!(Context, vfp)),
                ("fpscr", offset_of!(Context, fpscr)),
                ("host", offset_of!(Context, host)),
                ("user", offset_of!(Context, user)),
                ("stack_limit", offset_of!(Context, stack_limit)),
                ("tick_end", offset_of!(Context, tick_end)),
                ("tick_last", offset_of!(Context, tick_last)),
                ("tick_floor", offset_of!(Context, tick_floor)),
                ("tick_cost", offset_of!(Context, tick_cost)),
                ("tick_at", offset_of!(Context, tick_at)),
                ("ticks", offset_of!(Context, ticks)),
                ("wait_at", offset_of!(Context, wait_at)),
                ("release_at", offset_of!(Context, release_at)),
                ("held", offset_of!(Context, held)),
                ("holding", offset_of!(Context, holding)),
                ("held_floor", offset_of!(Context, held_floor)),
                ("done", offset_of!(Context, done)),
                ("trail", offset_of!(Context, trail)),
            ],
        );
        at(
            "Host",
            &[
                ("", size_of::<Host>()),
                ("read8", offset_of!(Host, read8)),
                ("read16", offset_of!(Host, read16)),
                ("read32", offset_of!(Host, read32)),
                ("write8", offset_of!(Host, write8)),
                ("write16", offset_of!(Host, write16)),
                ("write32", offset_of!(Host, write32)),
                ("interpret", offset_of!(Host, interpret)),
                ("lookup", offset_of!(Host, lookup)),
                ("svc", offset_of!(Host, svc)),
            ],
        );
        at(
            "Entry",
            &[("", size_of::<Entry>()), ("address", offset_of!(Entry, address)), ("code", offset_of!(Entry, code))],
        );
        at(
            "Module",
            &[
                ("", size_of::<Module>()),
                ("name", offset_of!(Module, name)),
                ("base", offset_of!(Module, base)),
                ("size", offset_of!(Module, size)),
                ("count", offset_of!(Module, count)),
                ("entries", offset_of!(Module, entries)),
            ],
        );
        at(
            "Piece",
            &[
                ("", size_of::<Piece>()),
                ("start", offset_of!(Piece, start)),
                ("end", offset_of!(Piece, end)),
                ("hash", offset_of!(Piece, hash)),
            ],
        );
        at("Run", &[("", size_of::<Run>()), ("first", offset_of!(Run, first)), ("end", offset_of!(Run, end))]);
        at(
            "Origin",
            &[
                ("", size_of::<Origin>()),
                ("first", offset_of!(Origin, first)),
                ("count", offset_of!(Origin, count)),
                ("start", offset_of!(Origin, start)),
            ],
        );
        at(
            "Origins",
            &[
                ("", size_of::<Origins>()),
                ("count", offset_of!(Origins, count)),
                ("piece_count", offset_of!(Origins, piece_count)),
                ("pieces", offset_of!(Origins, pieces)),
                ("functions", offset_of!(Origins, functions)),
                ("runs", offset_of!(Origins, runs)),
                ("owners", offset_of!(Origins, owners)),
                ("stale", offset_of!(Origins, stale)),
                ("starts", offset_of!(Origins, starts)),
            ],
        );
        lines.push(format!("NO_ORIGIN {NO_ORIGIN}"));
        lines.push(format!("NO_START {NO_START}"));
        lines.push(format!("RECOMP_ABI {ABI}"));
        lines.push(format!("RECOMP_STOPPED {STOPPED}"));
        lines.push(format!("RECOMP_TRAIL {TRAIL}"));
        for (name, value) in
            [("EXIT_NONE", EXIT_NONE), ("EXIT_SVC", EXIT_SVC), ("EXIT_BUDGET", EXIT_BUDGET), ("EXIT_UNWIND", EXIT_UNWIND)]
        {
            lines.push(format!("{name} {value}"));
        }
        assert_eq!(header, lines, "this mirror and recomp.h disagree");
    }

    /// the stack limit leaves STACK bytes below the caller's frame
    #[test]
    fn the_stack_limit_stays_below_here() {
        // a local, on this frame (`&0u8` is a promoted static: read-only data, above the stack on Linux)
        let local = 0u8;
        let here = core::ptr::addr_of!(local) as usize;
        let limit = stack_limit();
        assert!(limit < here);
        // STACK bytes below this frame, give or take the frame itself
        assert!(here.abs_diff(limit + STACK) < 4096, "{here:#x} {limit:#x}");
    }
}
