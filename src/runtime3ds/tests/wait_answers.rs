//! the WaitSynchronization answers (kernel ABI, 3dbrew `SVC` / Citra `WaitSynchronizationN`):
//! an immediately satisfied wait takes what it waited for (a mutex is owned, a semaphore
//! decremented, a one-shot event reset); a timeout answers r0 = 0x09401BFE, r1 = -1; a parked
//! wait woken by a signal answers r0 = 0, r1 = the index of the handle that fired. The guest code
//! is Rust `Code` functions, so this default test needs no compiler.

use runtime3ds::context::{self, Context, Entry, EXIT_NONE};
use runtime3ds::dispatch::Library;
use runtime3ds::mem::{Kind, Memory, Perm, State};
use runtime3ds::svc::{R_OK, R_TIMEOUT};
use runtime3ds::{BootOptions, Process3ds};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const MAIN_ENTRY: u32 = 0x2000_0000;
const CHILD_ENTRY: u32 = 0x2000_0100;
const CHILD_STACK: u32 = 0x0FF0_1000;
/// the handle arrays the N form reads
const HANDLES: u32 = 0x0FF8_0000;

/// (case, r0, r1) answers the main entry recorded
static ANSWERS: Mutex<Vec<(&'static str, u32, u32)>> = Mutex::new(Vec::new());
static SIGNALED: AtomicU32 = AtomicU32::new(0);
static MEMORY: OnceLock<Arc<Memory>> = OnceLock::new();

fn svc(ctx: *mut Context, number: u32) {
    // safe: ctx is the live context the run loop handed the entry
    unsafe {
        ((*(*ctx).host).svc)(ctx, number);
        // the entry keeps running after a parked wait; the unwind request is the run loop's
        (*ctx).exit = EXIT_NONE;
    }
}

fn record(case: &'static str, ctx: &Context) {
    ANSWERS.lock().unwrap().push((case, ctx.r[0], ctx.r[1]));
}

/// WaitSynchronizationN(handles, waitAll = 0, ns)
fn wait_n(ctx: *mut Context, handles: &[u32], ns: u64) {
    let context = unsafe { &mut *ctx };
    for (i, handle) in handles.iter().enumerate() {
        let at = HANDLES + 4 * i as u32;
        let bytes = handle.to_le_bytes();
        MEMORY.get().unwrap().write_bytes(at, &bytes);
    }
    context.r[1] = HANDLES;
    context.r[2] = handles.len() as u32;
    context.r[3] = 0;
    context.r[4] = ns as u32;
    context.r[5] = (ns >> 32) as u32;
    svc(ctx, 0x25);
}

fn create(ctx: *mut Context, number: u32, r1: u32, r2: u32) -> u32 {
    let context = unsafe { &mut *ctx };
    context.r[1] = r1;
    context.r[2] = r2;
    svc(ctx, number);
    context.r[1]
}

unsafe extern "C" fn main_entry(ctx: *mut Context) -> u32 {
    let context = unsafe { &mut *ctx };
    // a free mutex: the wait takes it, so this thread's ReleaseMutex succeeds
    let mutex = create(ctx, 0x13, 0, 0);
    wait_n(ctx, &[mutex], 0);
    record("mutex wait", context);
    context.r[0] = mutex;
    svc(ctx, 0x14);
    record("mutex release", context);
    // a semaphore with one count: the first wait takes it, the second poll times out
    let semaphore = create(ctx, 0x15, 1, 4);
    wait_n(ctx, &[semaphore], 0);
    record("semaphore first", context);
    wait_n(ctx, &[semaphore], 0);
    record("semaphore second", context);
    // a signaled one-shot event: the first wait resets it, the second poll times out
    let event = create(ctx, 0x17, 0, 0);
    context.r[0] = event;
    svc(ctx, 0x18);
    wait_n(ctx, &[event], 0);
    record("oneshot first", context);
    wait_n(ctx, &[event], 0);
    record("oneshot second", context);
    // a parked wait that times out: r1 = -1, not index 0
    let quiet = create(ctx, 0x17, 0, 0);
    wait_n(ctx, &[quiet, event], 1_000_000);
    record("timeout", context);
    // a parked wait woken by the child's signal of the second handle
    let woken = create(ctx, 0x17, 0, 0);
    context.r[0] = 0x18; // priority
    context.r[1] = CHILD_ENTRY;
    context.r[2] = woken;
    context.r[3] = CHILD_STACK;
    context.r[4] = 0xFFFF_FFFE;
    svc(ctx, 0x08);
    wait_n(ctx, &[quiet, woken], u64::MAX);
    record("woken", context);
    svc(ctx, 0x09); // ExitThread
    0
}

unsafe extern "C" fn child_entry(ctx: *mut Context) -> u32 {
    let context = unsafe { &mut *ctx };
    let event = context.r[0];
    // sleep so the main thread is parked when the signal lands (5 s guest = 50 ms host)
    context.r[0] = 5_000_000_000u64 as u32;
    context.r[1] = (5_000_000_000u64 >> 32) as u32;
    svc(ctx, 0x0A);
    context.r[0] = event;
    svc(ctx, 0x18);
    SIGNALED.fetch_add(1, Ordering::Relaxed);
    svc(ctx, 0x09);
    0
}

#[test]
fn wait_answers_follow_the_kernel_abi() {
    let memory = Arc::new(Memory::new().unwrap());
    memory.map(0x0FF0_0000, 0x0010_0000, Perm::Rw, State::Private, Kind::Stack).unwrap();
    MEMORY.set(memory.clone()).ok();
    let entries = [
        Entry { address: MAIN_ENTRY, code: main_entry },
        Entry { address: CHILD_ENTRY, code: child_entry },
    ];
    // safe: the two Rust entries above stand in for generated code
    let library = unsafe {
        Library::linked(context::ABI, entries.as_ptr(), entries.len(), std::ptr::null(), 0, std::ptr::null(), 0)
    };
    let process = Process3ds::new(library, memory, BootOptions::default());
    runtime3ds::start(&process, 0, MAIN_ENTRY, 0x1000_0000, 0x4000).unwrap();
    let answers = ANSWERS.lock().unwrap().clone();
    let answer = |case: &str| {
        answers.iter().find(|(c, _, _)| *c == case).map(|&(_, r0, r1)| (r0, r1)).unwrap_or_else(|| panic!("{case} not reached: {answers:?}"))
    };
    assert_eq!(answer("mutex wait"), (R_OK, 0));
    assert_eq!(answer("mutex release").0, R_OK, "the wait took the mutex, so the release is the owner's");
    assert_eq!(answer("semaphore first"), (R_OK, 0));
    assert_eq!(answer("semaphore second"), (R_TIMEOUT, u32::MAX), "the first wait decremented it");
    assert_eq!(answer("oneshot first"), (R_OK, 0));
    assert_eq!(answer("oneshot second"), (R_TIMEOUT, u32::MAX), "the first wait reset it");
    assert_eq!(answer("timeout"), (R_TIMEOUT, u32::MAX));
    assert_eq!(SIGNALED.load(Ordering::Relaxed), 1);
    assert_eq!(answer("woken"), (R_OK, 1), "the signaled handle's index");
}
