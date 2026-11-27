//! CPU-only submission ownership and finite completion policy. No driver dependencies.

use std::time::Duration;

pub const COMPLETION_BUDGET: Duration = Duration::from_secs(5);
const WAIT_SLICE: Duration = Duration::from_millis(250);

/// Stable harness protocol: GPU completion/resource safety failure (not guest timeout).
pub const RESOURCE_SAFETY_EXIT: i32 = 72;

/// No logging, formatting, locks, driver queries, unwinding, or runtime exit handlers.
/// The external owner records the reason from status 72, independently of log delivery.
pub fn resource_safety_fatal() -> ! {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn TerminateProcess(process: *mut std::ffi::c_void, code: u32) -> i32;
        }
        // SAFETY: the pseudo-handle names only this process; no owned external PID.
        // Successful self-termination does not return. Only failure reaches abort.
        unsafe { TerminateProcess(GetCurrentProcess(), RESOURCE_SAFETY_EXIT as u32) };
        std::process::abort()
    }
    // the web build: `_exit` on a pthread only ends that thread (the runtime is kept alive under PROXY_TO_PTHREAD), so
    // the program went on without it and waited for ever (WEB-W4c). abort() traps and reaches the page's onAbort.
    #[cfg(target_os = "emscripten")]
    {
        std::process::abort()
    }
    #[cfg(all(unix, not(target_os = "emscripten")))]
    {
        extern "C" {
            fn _exit(code: i32) -> !;
        }
        // SAFETY: POSIX immediate process exit, bypassing atexit and stdio flushing.
        unsafe { _exit(RESOURCE_SAFETY_EXIT) }
    }
    #[cfg(not(any(windows, unix)))]
    {
        // No native immediate-exit binding for this unsupported target.
        std::process::abort()
    }
}

pub fn resource_safety_result<T, E>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(_error) => resource_safety_fatal(),
    }
}

/// VkResult values, kept dependency-free for tests of the exact production classifier.
/// All other errors, including host/device OOM and device loss, are terminal.
pub fn descriptor_capacity_error(result: i32) -> bool {
    matches!(result, -12 | -1_000_069_000) // FRAGMENTED_POOL / OUT_OF_POOL_MEMORY
}

#[derive(Debug, PartialEq, Eq)]
pub enum DescriptorAllocationError<E> {
    InvalidCursor,
    Create(E),
    Allocate(E),
}

/// Visit each remaining existing pool at most once, then create at most one pool.
/// Register a newly created pool before set allocation. Failure leaves every pool
/// owned: earlier commands may reference them, and failure is not GPU completion.
pub fn allocate_descriptor_set<P, S, E>(
    pools: &mut Vec<P>,
    cursor: &mut usize,
    create: impl FnOnce() -> Result<P, E>,
    mut allocate: impl FnMut(&P) -> Result<S, E>,
    is_capacity: impl Fn(&E) -> bool,
) -> Result<S, DescriptorAllocationError<E>> {
    let existing = pools.len();
    if *cursor > existing {
        return Err(DescriptorAllocationError::InvalidCursor);
    }
    while *cursor < existing {
        match allocate(&pools[*cursor]) {
            Ok(set) => return Ok(set),
            Err(error) if is_capacity(&error) => *cursor += 1,
            Err(error) => return Err(DescriptorAllocationError::Allocate(error)),
        }
    }
    let pool = create().map_err(DescriptorAllocationError::Create)?;
    pools.push(pool);
    // No retry on any fresh-pool failure, including exhaustion/fragmentation.
    allocate(&pools[*cursor]).map_err(DescriptorAllocationError::Allocate)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompletionError<E> {
    Deadline,
    Driver(E),
}

/// Only a successful wait within budget constructs this receipt. Not a CPU fence signal.
#[derive(Debug)]
pub struct Completion {
    fence: u64,
}

/// `elapsed` is monotonic time since this submission's wait began. `wait` returns
/// true only for driver success, false for TIMEOUT. Charge each timeout its requested
/// slice too, so even an erroneously early TIMEOUT cannot create an unbounded retry.
pub fn wait_for_completion<E>(
    fence: u64,
    mut elapsed: impl FnMut() -> Duration,
    mut wait: impl FnMut(Duration) -> Result<bool, E>,
) -> Result<Completion, CompletionError<E>> {
    let mut charged = Duration::ZERO;
    loop {
        let spent = elapsed().max(charged);
        if spent >= COMPLETION_BUDGET {
            return Err(CompletionError::Deadline);
        }
        let slice = WAIT_SLICE.min(COMPLETION_BUDGET - spent);
        let result = wait(slice);
        // Even a late success/error must not trigger writebacks or driver diagnostics.
        if elapsed() >= COMPLETION_BUDGET {
            return Err(CompletionError::Deadline);
        }
        match result {
            Ok(true) => return Ok(Completion { fence }),
            Ok(false) => charged = spent + slice,
            Err(e) => return Err(CompletionError::Driver(e)),
        }
    }
}

/// Guest pools are reused; native one-use pools must never enter that retained list.
pub struct DescriptorPools<P> {
    pub reusable: Vec<P>,
    transient: Vec<P>,
}

impl<P> Default for DescriptorPools<P> {
    fn default() -> Self {
        Self { reusable: Vec::new(), transient: Vec::new() }
    }
}

impl<P> DescriptorPools<P> {
    /// Register before any command can reference this pool's sets. Allocation failure
    /// before registration remains the allocator's responsibility.
    pub fn retain_transient(&mut self, pool: P) {
        self.transient.push(pool);
    }

    /// Called for this exact submission, never merely an earlier completed fence.
    /// On failure nothing is reset, released or removed from ownership.
    pub fn recycle(
        &mut self,
        submitted: u64,
        completed: &Completion,
        mut reset: impl FnMut(&P),
        mut destroy: impl FnMut(P),
    ) -> Result<(), &'static str> {
        if submitted != completed.fence {
            return Err("descriptor pool completion belongs to another submission");
        }
        for pool in &self.reusable {
            reset(pool);
        }
        for pool in self.transient.drain(..) {
            destroy(pool);
        }
        Ok(())
    }
}

pub fn live_ndve_permission() -> Result<(), &'static str> {
    Err("live NDVE quarantined under owner hold: bounded resources and certification unproven")
}

/// No environment value can bypass the hold; off retains ordinary guest dispatches.
pub fn ndve_mode_permission(value: Option<&str>) -> Result<(), &'static str> {
    match value {
        None | Some("0" | "off") => Ok(()),
        Some("1" | "on" | "check") => live_ndve_permission(),
        _ => Err("expected off; live NDVE on/1/check are quarantined"),
    }
}

/// Own partial buffer construction until binding succeeds. Callbacks must release
/// allocator guards before returning; cleanup runs only after those calls return.
/// Destroy the buffer before freeing its backing allocation. On success the caller
/// receives both owners, with no cleanup or host-memory accounting performed here.
pub fn create_bound_buffer<B, A, E>(
    create: impl FnOnce() -> Result<B, E>,
    allocate: impl FnOnce(&B) -> Result<A, E>,
    bind: impl FnOnce(&B, &A) -> Result<(), E>,
    destroy: impl FnOnce(B),
    free: impl FnOnce(A),
) -> Result<(B, A), E> {
    let buffer = create()?;
    let allocation = match allocate(&buffer) {
        Ok(allocation) => allocation,
        Err(error) => {
            destroy(buffer);
            return Err(error);
        }
    };
    if let Err(error) = bind(&buffer, &allocation) {
        destroy(buffer);
        free(allocation);
        return Err(error);
    }
    Ok((buffer, allocation))
}

/// Mapping failure still owns the successfully bound pair locally. A failed map
/// has no mapping to unmap; success transfers all three values to the caller.
pub fn map_bound_buffer<B, A, P, E>(
    pair: (B, A),
    map: impl FnOnce(&A) -> Result<P, E>,
    destroy: impl FnOnce(B),
    free: impl FnOnce(A),
) -> Result<(B, A, P), E> {
    let (buffer, allocation) = pair;
    match map(&allocation) {
        Ok(pointer) => Ok((buffer, allocation, pointer)),
        Err(error) => { destroy(buffer); free(allocation); Err(error) }
    }
}

#[cfg(test)]
mod mapped_construction_tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn every_descriptor_construction_stage_has_exact_once_ordered_cleanup() {
        // Memory-type selection is part of allocate, just as at the production call site.
        for fail in 0..=5 {
            let events = RefCell::new(Vec::new());
            let stage = |n| { events.borrow_mut().push(n); if n == fail { Err(n) } else { Ok(n) } };
            let pair = create_bound_buffer(|| stage(0), |_| { stage(1)?; stage(2) },
                |_, _| stage(3).map(|_| ()), |_| events.borrow_mut().push(10),
                |_| events.borrow_mut().push(11));
            let result = pair.and_then(|pair| map_bound_buffer(pair, |_| stage(4),
                |_| events.borrow_mut().push(10), |_| events.borrow_mut().push(11)));
            let expected = match fail {
                0 => vec![0], 1 => vec![0, 1, 10], 2 => vec![0, 1, 2, 10],
                3 => vec![0, 1, 2, 3, 10, 11], 4 => vec![0, 1, 2, 3, 4, 10, 11],
                _ => vec![0, 1, 2, 3, 4],
            };
            assert_eq!(*events.borrow(), expected);
            assert_eq!(result.is_ok(), fail == 5);
        }
    }
}

#[cfg(test)]
mod descriptor_allocation_tests {
    use super::*;
    use std::cell::Cell;

    const EXHAUSTED: i32 = -1_000_069_000;
    const FRAGMENTED: i32 = -12;

    fn capacity(error: &i32) -> bool {
        descriptor_capacity_error(*error)
    }

    #[test]
    fn exhausted_old_pools_then_one_good_new_pool() {
        let mut pools = vec![10, 20, 30];
        let mut cursor = 0;
        let creates = Cell::new(0);
        let mut attempts = Vec::new();
        let set = allocate_descriptor_set(&mut pools, &mut cursor, || {
            creates.set(creates.get() + 1);
            Ok(40)
        }, |pool| {
            attempts.push(*pool);
            match pool {
                10 | 30 => Err(EXHAUSTED),
                20 => Err(FRAGMENTED),
                _ => Ok(99),
            }
        }, capacity).unwrap();
        assert_eq!(set, 99);
        assert_eq!(attempts, [10, 20, 30, 40]);
        assert_eq!(creates.get(), 1);
        assert_eq!(pools, [10, 20, 30, 40]);
        assert_eq!(cursor, 3);
    }

    #[test]
    fn existing_success_does_not_create_or_revisit_prior_pools() {
        let mut pools = vec![10, 20, 30];
        let mut cursor = 1;
        let mut attempts = Vec::new();
        let set = allocate_descriptor_set(&mut pools, &mut cursor,
            || panic!("unexpected fresh pool"), |pool| {
                attempts.push(*pool);
                if *pool == 20 { Err(EXHAUSTED) } else { Ok(7) }
            }, capacity).unwrap();
        assert_eq!(set, 7);
        assert_eq!(attempts, [20, 30]);
        assert_eq!(cursor, 2);
    }

    #[test]
    fn persistent_fresh_pool_errors_are_attempted_once_not_retried() {
        for error in [EXHAUSTED, FRAGMENTED, -1, -2, -4, -13] {
            for old_count in [0, 3] {
                let mut pools: Vec<_> = (0..old_count).collect();
                let mut cursor = 0;
                let mut creates = 0;
                let mut attempts = 0;
                let result = allocate_descriptor_set(&mut pools, &mut cursor, || {
                    creates += 1;
                    Ok(old_count)
                }, |pool| {
                    attempts += 1;
                    Err::<(), _>(if *pool < old_count { EXHAUSTED } else { error })
                }, capacity);
                assert_eq!(result, Err(DescriptorAllocationError::Allocate(error)));
                assert_eq!(creates, 1);
                assert_eq!(attempts, old_count + 1);
                assert_eq!(pools.len(), old_count + 1, "no early resource removal");
            }
        }
    }

    #[test]
    fn terminal_old_pool_errors_never_allocate_another_pool() {
        // HOST_MEMORY, DEVICE_MEMORY, DEVICE_LOST, UNKNOWN, and unrelated positive codes.
        for error in [-1, -2, -4, -13, 1, 2, 5, i32::MIN, i32::MAX] {
            let mut pools = vec![10, 20];
            let mut cursor = 0;
            let mut attempts = 0;
            let result = allocate_descriptor_set(&mut pools, &mut cursor,
                || panic!("terminal error created another pool"), |_| {
                    attempts += 1;
                    Err::<(), _>(error)
                }, capacity);
            assert_eq!(result, Err(DescriptorAllocationError::Allocate(error)));
            assert_eq!(attempts, 1);
            assert_eq!(cursor, 0);
            assert_eq!(pools, [10, 20]);
        }
    }

    #[test]
    fn pool_create_failure_is_terminal_without_set_attempt_or_owner_removal() {
        let mut pools = vec![10];
        let mut cursor = 1;
        let result = allocate_descriptor_set::<_, (), _>(&mut pools, &mut cursor,
            || Err(-2), |_| panic!("set allocation after pool creation failed"), capacity);
        assert_eq!(result, Err(DescriptorAllocationError::Create(-2)));
        assert_eq!(pools, [10]);
        assert_eq!(cursor, 1);
    }

    #[test]
    fn invalid_cursor_fails_without_driver_work() {
        let result = allocate_descriptor_set::<u32, (), i32>(&mut vec![10], &mut 2,
            || panic!("create with invalid cursor"), |_| panic!("allocate with invalid cursor"),
            capacity);
        assert_eq!(result, Err(DescriptorAllocationError::InvalidCursor));
    }
}

#[cfg(test)]
mod buffer_creation_tests {
    use super::create_bound_buffer;
    use std::cell::RefCell;
    use std::sync::Mutex;

    // Deliberately not Copy/Clone: the production allocation is a moved owner too.
    #[derive(Debug, PartialEq, Eq)]
    struct Owner(u32);

    #[test]
    fn create_failure_destroys_nothing_and_never_allocates_or_binds() {
        let result = create_bound_buffer::<Owner, Owner, _>(
            || Err("create failed"),
            |_| panic!("allocation after create failure"),
            |_, _| panic!("bind after create failure"),
            |_| panic!("destroy without a buffer"),
            |_| panic!("free without an allocation"),
        );
        assert_eq!(result, Err("create failed"));
    }

    #[test]
    fn allocation_failure_destroys_buffer_once_after_unlock() {
        let allocator = Mutex::new(());
        let destroyed = RefCell::new(Vec::new());
        let result = create_bound_buffer::<_, Owner, _>(
            || Ok(Owner(17)),
            |buffer| {
                assert_eq!(buffer.0, 17);
                let result = {
                    let _guard = allocator.lock().unwrap();
                    Err("allocation failed")
                };
                result
            },
            |_, _| panic!("bind after allocation failure"),
            |buffer| {
                let _guard = allocator.try_lock().expect("allocator held during destroy");
                destroyed.borrow_mut().push(buffer.0);
            },
            |_| panic!("free without an allocation"),
        );
        assert_eq!(result, Err("allocation failed"));
        assert_eq!(*destroyed.borrow(), [17]);
    }

    #[test]
    fn bind_failure_destroys_then_frees_once_without_recursive_allocator_lock() {
        let allocator = Mutex::new(());
        let events = RefCell::new(Vec::new());
        let result = create_bound_buffer(
            || Ok(Owner(23)),
            |_| {
                let _guard = allocator.lock().unwrap();
                Ok(Owner(41))
            },
            |buffer, allocation| {
                let _guard = allocator.try_lock().expect("allocator held during bind");
                assert_eq!((buffer.0, allocation.0), (23, 41));
                Err("bind failed")
            },
            |buffer| {
                let _guard = allocator.try_lock().expect("allocator held during destroy");
                events.borrow_mut().push(("destroy", buffer.0));
            },
            |allocation| {
                let _guard = allocator.try_lock().expect("recursive allocator lock on free");
                events.borrow_mut().push(("free", allocation.0));
            },
        );
        assert_eq!(result, Err("bind failed"));
        assert_eq!(*events.borrow(), [("destroy", 23), ("free", 41)]);
        assert!(allocator.try_lock().is_ok());
    }

    #[test]
    fn success_transfers_both_owners_without_early_cleanup() {
        let events = RefCell::new(Vec::new());
        let result = create_bound_buffer(
            || {
                events.borrow_mut().push("create");
                Ok::<_, &str>(Owner(53))
            },
            |buffer| {
                assert_eq!(buffer.0, 53);
                events.borrow_mut().push("allocate");
                Ok(Owner(67))
            },
            |buffer, allocation| {
                assert_eq!((buffer.0, allocation.0), (53, 67));
                events.borrow_mut().push("bind");
                Ok(())
            },
            |_| panic!("success destroyed buffer before ownership transfer"),
            |_| panic!("success freed allocation before ownership transfer"),
        );
        let (buffer, allocation) = result.unwrap();
        assert_eq!((buffer, allocation), (Owner(53), Owner(67)));
        assert_eq!(*events.borrow(), ["create", "allocate", "bind"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn completed(fence: u64) -> Completion {
        wait_for_completion(fence, || Duration::ZERO, |_| Ok::<_, ()>(true)).unwrap()
    }

    #[test]
    fn many_cycles_retain_only_guest_pools_and_destroy_native_once_after_own_fence() {
        let mut pools = DescriptorPools::default();
        pools.reusable.extend([0usize, 1, 2]);
        let mut freed = [false; 18];
        let mut destroyed = 0;
        let mut resets = 0;
        for cycle in 1..=10_000u64 {
            freed.fill(false);
            for pool in 0..18 {
                pools.retain_transient(pool);
            }
            assert_eq!(pools.reusable.len() + pools.transient.len(), 21);
            assert!(pools.recycle(cycle, &completed(cycle - 1), |_| panic!("early reset"),
                |_| panic!("early destroy")).is_err());
            assert_eq!(pools.transient.len(), 18);
            pools.recycle(cycle, &completed(cycle), |p| {
                assert!(*p < 3);
                resets += 1;
            }, |p| {
                assert!(!freed[p], "double free in cycle {cycle}");
                freed[p] = true;
                destroyed += 1;
            }).unwrap();
            assert!(freed.iter().all(|&v| v));
            assert_eq!(pools.reusable, [0, 1, 2]);
            assert!(pools.transient.is_empty());
            // A duplicate cleanup cannot destroy native handles a second time.
            pools.recycle(cycle, &completed(cycle), |_| {}, |_| panic!("double free")).unwrap();
        }
        assert_eq!(destroyed, 180_000);
        assert_eq!(resets, 30_000);
        println!("10000 cycles: retained=3 guest+0 native; peak=21; native destroyed=180000 exactly once");
    }

    #[test]
    fn partial_recording_error_keeps_registered_pools_until_completion() {
        let mut pools = DescriptorPools::default();
        let record = |pools: &mut DescriptorPools<u32>| -> Result<(), ()> {
            pools.retain_transient(7);
            // A later dispatch/preflight fails after an earlier one was recorded.
            Err(())
        };
        assert!(record(&mut pools).is_err());
        assert_eq!(pools.transient, [7]);
        let mut freed = Vec::new();
        pools.recycle(9, &completed(9), |_| {}, |p| freed.push(p)).unwrap();
        assert_eq!(freed, [7]);
    }

    #[test]
    fn timeout_is_finite_even_if_driver_returns_early_and_never_releases_ownership() {
        let mut pools = DescriptorPools::default();
        pools.reusable.push(1);
        pools.retain_transient(2);
        let mut waits = 0;
        let result = wait_for_completion(3, || Duration::ZERO, |slice| {
            assert_eq!(slice, WAIT_SLICE);
            waits += 1;
            Ok::<_, ()>(false)
        });
        assert!(matches!(result, Err(CompletionError::Deadline)));
        assert_eq!(waits, 20);
        assert_eq!(pools.reusable, [1]);
        assert_eq!(pools.transient, [2]);
    }

    #[test]
    fn elapsed_deadline_refuses_late_success_error_and_any_further_wait() {
        for response in [Ok(true), Ok(false), Err("device lost")] {
            let now = Cell::new(Duration::ZERO);
            let mut calls = 0;
            let result = wait_for_completion(1, || now.get(), |_| {
                calls += 1;
                now.set(COMPLETION_BUDGET);
                response
            });
            assert!(matches!(result, Err(CompletionError::Deadline)));
            assert_eq!(calls, 1);
        }
        let result = wait_for_completion::<()>(1, || COMPLETION_BUDGET,
            |_| panic!("wait after deadline"));
        assert!(matches!(result, Err(CompletionError::Deadline)));
    }

    #[test]
    fn success_after_timeout_and_driver_error_are_not_fabricated_completions() {
        let mut calls = 0;
        let receipt = wait_for_completion(12, || Duration::ZERO, |_| {
            calls += 1;
            Ok::<_, ()>(calls == 2)
        }).unwrap();
        assert_eq!(receipt.fence, 12);
        assert_eq!(calls, 2);
        let error = wait_for_completion(13, || Duration::ZERO, |_| Err::<bool, _>("lost"));
        assert!(matches!(error, Err(CompletionError::Driver("lost"))));
    }

    #[test]
    fn live_ndve_is_quarantined_for_every_enabling_mode() {
        for value in [None, Some("off"), Some("0")] {
            assert_eq!(ndve_mode_permission(value), Ok(()));
        }
        for value in ["on", "1", "check", "ON", ""] {
            assert!(ndve_mode_permission(Some(value)).is_err());
        }
        assert!(live_ndve_permission().is_err());
    }
}
