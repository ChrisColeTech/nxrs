//! the thread proof: svcCreateThread (0x08) spawns a guest thread, the created thread runs its
//! entry on its own host thread with the argument the kernel passes in r0, and both threads end
//! cleanly. The code entries are Rust `Code` functions so this default test needs no compiler.

use runtime3ds::context::{self, Context, Entry};
use runtime3ds::dispatch::Library;
use runtime3ds::mem::Memory;
use runtime3ds::{BootOptions, Process3ds, Stop};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

static MAIN_RAN: AtomicU32 = AtomicU32::new(0);
static CHILD_RAN: AtomicU32 = AtomicU32::new(0);
static CHILD_HANDLE: AtomicU32 = AtomicU32::new(0);
static CHILD_ARG: AtomicU32 = AtomicU32::new(0);
static CHILD_SP: AtomicU32 = AtomicU32::new(0);

const MAIN_ENTRY: u32 = 0x2000_0000;
const CHILD_ENTRY: u32 = 0x2000_0100;
const CHILD_STACK: u32 = 0x0FF0_1000;
const CHILD_ARGUMENT: u32 = 0x1234_5678;

unsafe extern "C" fn main_entry(ctx: *mut Context) -> u32 {
    MAIN_RAN.fetch_add(1, Ordering::Relaxed);
    let context = unsafe { &mut *ctx };
    context.r[0] = 0x18; // priority
    context.r[1] = CHILD_ENTRY;
    context.r[2] = CHILD_ARGUMENT;
    context.r[3] = CHILD_STACK;
    context.r[4] = 0xFFFF_FFFE; // processor: the process default
    unsafe { ((*context.host).svc)(ctx, 0x08) };
    CHILD_HANDLE.store(context.r[1], Ordering::Relaxed);
    assert_eq!(context.r[0], 0, "CreateThread succeeded");
    assert_ne!(context.r[1], 0, "CreateThread answered a handle in r1");
    unsafe { ((*context.host).svc)(ctx, 0x09) }; // ExitThread
    0
}

unsafe extern "C" fn child_entry(ctx: *mut Context) -> u32 {
    CHILD_RAN.fetch_add(1, Ordering::Relaxed);
    let context = unsafe { &*ctx };
    CHILD_ARG.store(context.r[0], Ordering::Relaxed);
    CHILD_SP.store(context.r[13], Ordering::Relaxed);
    unsafe { ((*context.host).svc)(ctx, 0x09) }; // ExitThread
    0
}

#[test]
fn create_thread_runs_the_created_thread() {
    let memory = Arc::new(Memory::new().unwrap());
    memory
        .map(0x0FF0_0000, 0x0010_0000, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Private, runtime3ds::mem::Kind::Stack)
        .unwrap();
    let entries = [
        Entry { address: MAIN_ENTRY, code: main_entry },
        Entry { address: CHILD_ENTRY, code: child_entry },
    ];
    let library = unsafe {
        Library::linked(context::ABI, entries.as_ptr(), entries.len(), std::ptr::null(), 0, std::ptr::null(), 0)
    };
    let process = Process3ds::new(library, memory, BootOptions::default());
    let report = runtime3ds::start(&process, 0, MAIN_ENTRY, 0x1000_0000, 0x4000).unwrap();

    assert_eq!(MAIN_RAN.load(Ordering::Relaxed), 1, "the main entry ran once");
    assert_eq!(CHILD_RAN.load(Ordering::Relaxed), 1, "the created thread ran its entry");
    assert_eq!(CHILD_ARG.load(Ordering::Relaxed), CHILD_ARGUMENT, "r0 is the CreateThread arg");
    assert_eq!(CHILD_SP.load(Ordering::Relaxed), CHILD_STACK, "the stack top it was given");
    assert_ne!(CHILD_HANDLE.load(Ordering::Relaxed), 0);
    assert!(report.threads_ran >= 2, "main and child both ran, {}", report.threads_ran);
    assert!(matches!(report.stop, Stop::Idle), "clean stop after every thread ended: {:?}", report.stop);
}
