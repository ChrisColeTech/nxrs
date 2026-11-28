//! Wait accounting (`NXRS_WAIT_ACCT=1`): what each guest thread blocks on, for how long, and who ends the wait.
//!
//! Every blocking wait of a guest thread is added to one table by (thread, kind, object): the kernel's waits (a lock
//! word, a condition key, an address, the handle that ended a WaitSynchronization, a sleep), a blocking host function
//! (fence, present and queue waits, services that block: the os layer's `without_core`, keyed by the host function
//! the thread is in) and the wait for a virtual core. Each entry keeps its count, total and longest time, and the
//! time by who ended it: the guest thread that unlocked, signalled or woke it, a host thread (by name), or nobody (a
//! timeout). With `NXRS_WAIT_ACCT=2`, beside the waits, the wall time inside each host function (`call`) and each SVC
//! that is not a kernel wait (`svc`, IPC by session handle) is kept the same way, outside the waits' sum (it includes
//! their own blocking waits).
//! A `waits:` block goes to the log every 10 s: per thread its CPU share (from the OS), its entry function,
//! the time it was neither running nor in a counted wait, and its top waits in ms a second with their wakers. That is
//! the chain: what a thread waits on, who ends that wait, and what that thread waits on in turn.
//!
//! Light by design (OOT-FPS2's every-thread stack sampling ran out of memory): one map update under a lock per wait
//! (only when on), no sampling, no stacks. Costs one relaxed load per wait when off.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Wait kinds.
pub const K_MUTEX: u8 = 0;
pub const K_CONDVAR: u8 = 1;
pub const K_ADDRESS: u8 = 2;
pub const K_SYNC: u8 = 3;
pub const K_SLEEP: u8 = 4;
/// A host function that blocks (graphics fences, presents, queue room; services).
pub const K_HOST: u8 = 5;
/// The wait for a virtual core (the os scheduler).
pub const K_CORE: u8 = 6;
/// Time inside a host function (wall time, its own blocking waits included: not a wait, not in the waits' sum).
pub const K_CALL: u8 = 7;
/// Time inside an SVC that is not a kernel wait (IPC to services and the rest; not in the waits' sum).
pub const K_SVC: u8 = 8;
const KIND_NAMES: [&str; 9] = ["mutex", "condvar", "address", "sync", "sleep", "host", "core", "call", "svc"];

/// A waker that is a host thread: this bit, and the index of its name.
pub const BY_HOST: u32 = 0x8000_0000;
/// No waker: the wait timed out (or the kind has none: sleeps, host waits, cores).
pub const BY_NONE: u32 = 0;

/// The report period.
const PERIOD: Duration = Duration::from_secs(10);

/// Whether the accounting runs (`NXRS_WAIT_ACCT`, any value but empty or 0; read once).
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NXRS_WAIT_ACCT").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Whether host-function and SVC time are kept too (`NXRS_WAIT_ACCT=2`): one table update per host call, tens of
/// thousands a second across the job threads (it slowed Ocarina of Time's frame, so it is not part of `1`).
pub fn calls_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| on() && std::env::var("NXRS_WAIT_ACCT").is_ok_and(|v| v.trim() == "2"))
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Acc {
    pub n: u64,
    pub ns: u64,
    pub max: u64,
    /// time by waker
    pub by: HashMap<u32, u64>,
}

#[derive(Default)]
pub struct Table {
    /// (thread, kind, object) -> totals
    pub m: HashMap<(u32, u8, u64), Acc>,
}

impl Table {
    pub fn add(&mut self, thread: u32, kind: u8, key: u64, ns: u64, by: u32) {
        let a = self.m.entry((thread, kind, key)).or_default();
        a.n += 1;
        a.ns += ns;
        a.max = a.max.max(ns);
        *a.by.entry(by).or_default() += ns;
    }
}

struct Global {
    tab: Table,
    /// host waker names (index = the waker's low bits)
    names: Vec<String>,
    /// guest thread -> its entry function
    entries: HashMap<u32, u64>,
}

fn global() -> &'static Mutex<Global> {
    static G: OnceLock<Mutex<Global>> = OnceLock::new();
    G.get_or_init(|| {
        Mutex::new(Global {
            tab: Table::default(),
            names: vec![],
            entries: HashMap::new(),
        })
    })
}

thread_local! {
    /// The guest thread this host thread runs (0: none).
    static ME: Cell<u32> = const { Cell::new(0) };
    /// This host thread's waker id when it is not a guest thread (0: not looked up yet).
    static HOST_ID: Cell<u32> = const { Cell::new(0) };
}

/// The calling host thread runs guest thread `handle` (entry `entry`).
pub fn thread_start(handle: u32, entry: u64) {
    ME.set(handle);
    if on() {
        let mut g = global().lock().unwrap_or_else(|p| p.into_inner());
        g.entries.insert(handle, entry);
    }
}

/// The guest thread the calling host thread runs (0: none).
pub fn current() -> u32 {
    ME.get()
}

/// The calling thread as a waker: its guest handle, else its host thread's name (interned).
pub fn waker() -> u32 {
    let me = ME.get();
    if me != 0 {
        return me;
    }
    let id = HOST_ID.get();
    if id != 0 {
        return id;
    }
    let name = std::thread::current().name().unwrap_or("unnamed").to_string();
    let mut g = global().lock().unwrap_or_else(|p| p.into_inner());
    let i = match g.names.iter().position(|n| *n == name) {
        Some(i) => i,
        None => {
            g.names.push(name);
            g.names.len() - 1
        }
    };
    let id = BY_HOST | i as u32;
    HOST_ID.set(id);
    id
}

/// A wait of the calling guest thread ended: `kind` on `key`, `ns` long, ended by `by`.
pub fn add(kind: u8, key: u64, ns: u64, by: u32) {
    let me = ME.get();
    if me == 0 || !on() {
        return;
    }
    global()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .tab
        .add(me, kind, key, ns, by);
}

/// Starts the reporter when on (once).
pub fn start() {
    static ONCE: OnceLock<()> = OnceLock::new();
    if !on() || ONCE.set(()).is_err() {
        return;
    }
    crate::rt::log("wait accounting: on (NXRS_WAIT_ACCT): a `waits:` block every 10 s");
    let _ = std::thread::Builder::new()
        .name("wait acct".into())
        .spawn(|| {
            let t0 = Instant::now();
            let mut last = Instant::now();
            let mut cpu_was: HashMap<u32, f64> = crate::profile::thread_cpu().into_iter().collect();
            loop {
                std::thread::sleep(PERIOD);
                let secs = last.elapsed().as_secs_f64();
                last = Instant::now();
                let cpu_now: HashMap<u32, f64> = crate::profile::thread_cpu().into_iter().collect();
                let (tab, names, entries) = {
                    let mut g = global().lock().unwrap_or_else(|p| p.into_inner());
                    (std::mem::take(&mut g.tab), g.names.clone(), g.entries.clone())
                };
                let cpu: HashMap<u32, f64> = cpu_now
                    .iter()
                    .map(|(h, b)| (*h, (b - cpu_was.get(h).copied().unwrap_or(*b)).max(0.0) / secs))
                    .collect();
                let name = |w: u32| waker_name(w, &names);
                let key = |k: u8, v: u64| key_name(k, v);
                let entry = |h: u32| entries.get(&h).map_or("?".into(), |&e| crate::dispatch::guest_name(e));
                for line in report(&tab, &cpu, secs, t0.elapsed().as_secs_f64(), &name, &key, &entry) {
                    crate::rt::log(&line);
                }
                cpu_was = cpu_now;
            }
        });
}

fn waker_name(w: u32, names: &[String]) -> String {
    if w == BY_NONE {
        "timeout".into()
    } else if w & BY_HOST != 0 {
        format!("host:{}", names.get((w & !BY_HOST) as usize).map_or("?", |s| s.as_str()))
    } else {
        format!("{w:#x}")
    }
}

fn key_name(kind: u8, key: u64) -> String {
    match kind {
        K_HOST | K_CALL => crate::hostfn::name_of(key).unwrap_or_else(|| format!("{key:#x}")),
        K_SVC if key & 0xffff_ffff != 0 => format!("{:#x} h{:#x}", key >> 32, key & 0xffff_ffff),
        K_SVC => format!("{:#x}", key >> 32),
        K_SYNC => format!("h{key:#x}"),
        K_SLEEP if key > 0 => format!("{:.0} us asked", key as f64 / 1e3),
        K_SLEEP | K_CORE => String::new(),
        _ => format!("{key:#x}"),
    }
}

/// The most waits listed per thread, and the most threads listed.
const TOP_WAITS: usize = 10;
const TOP_THREADS: usize = 16;

/// The `waits:` block of one period: threads by CPU + wait time (the main thread 0x1001 always), each with its CPU
/// share, entry, time in no counted wait, and its top waits (ms a second, count a second, longest, wakers' shares).
pub fn report(
    tab: &Table,
    cpu: &HashMap<u32, f64>,
    secs: f64,
    at: f64,
    waker: &dyn Fn(u32) -> String,
    key: &dyn Fn(u8, u64) -> String,
    entry: &dyn Fn(u32) -> String,
) -> Vec<String> {
    let secs = secs.max(1e-9);
    let mut per: HashMap<u32, Vec<(&(u32, u8, u64), &Acc)>> = HashMap::new();
    for (k, a) in &tab.m {
        per.entry(k.0).or_default().push((k, a));
    }
    for h in cpu.keys() {
        per.entry(*h).or_default();
    }
    let waited = |v: &Vec<(&(u32, u8, u64), &Acc)>| {
        v.iter().filter(|(k, _)| k.1 < K_CALL).map(|(_, a)| a.ns).sum::<u64>() as f64 / 1e9 / secs
    };
    let mut ths: Vec<(u32, f64, f64)> = per
        .iter()
        .map(|(h, v)| (*h, cpu.get(h).copied().unwrap_or(0.0), waited(v)))
        .collect();
    // the main thread first, then by CPU, then by time in counted waits
    ths.sort_by(|a, b| {
        (b.0 == 0x1001)
            .cmp(&(a.0 == 0x1001))
            .then(b.1.total_cmp(&a.1))
            .then(b.2.total_cmp(&a.2))
            .then(a.0.cmp(&b.0))
    });
    let mut out = vec![format!(
        "waits: at {at:.0} s, over {secs:.1} s: {} guest threads; per thread ms a second (count a second, longest ms; by: who ended the wait)",
        ths.len()
    )];
    for (h, c, w) in ths.into_iter().take(TOP_THREADS) {
        let mut v = per.remove(&h).unwrap_or_default();
        if c < 0.005 && v.is_empty() {
            continue;
        }
        v.sort_by(|a, b| b.1.ns.cmp(&a.1.ns).then(a.0.cmp(b.0)));
        let other = (1.0 - c - w).max(0.0);
        let mut s = format!(
            "waits: {h:#x} {} cpu {:.0}%, in waits {:.0}%, other {:.0}%:",
            entry(h),
            100.0 * c,
            100.0 * w,
            100.0 * other
        );
        for ((_, k, obj), a) in v.into_iter().take(TOP_WAITS) {
            let ms = a.ns as f64 / 1e6 / secs;
            let mut by: Vec<(&u32, &u64)> = a.by.iter().collect();
            by.sort_by(|x, y| y.1.cmp(x.1).then(x.0.cmp(y.0)));
            let by: Vec<String> = by
                .into_iter()
                .take(3)
                .map(|(w, ns)| format!("{} {:.0}%", waker(*w), 100.0 * *ns as f64 / a.ns.max(1) as f64))
                .collect();
            let kn = key(*k, *obj);
            s.push_str(&format!(
                " {}{}{} {ms:.1} ({:.0}/s, {:.1}; by {}) |",
                KIND_NAMES[*k as usize],
                if kn.is_empty() { "" } else { " " },
                kn,
                a.n as f64 / secs,
                a.max as f64 / 1e6,
                by.join(", ")
            ));
        }
        out.push(s.trim_end_matches(" |").trim_end_matches(':').to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table adds each wait to its (thread, kind, object), with its waker's time; the report lists the main
    /// thread first, then by CPU, each thread's waits by time with ms a second, count a second, the longest and the
    /// wakers' shares; time in no counted wait is what is left of the period.
    #[test]
    fn waits_are_added_by_object_and_reported_with_their_wakers() {
        let mut t = Table::default();
        t.add(0x1001, K_CONDVAR, 0x5000, 6_000_000, 0x1004);
        t.add(0x1001, K_CONDVAR, 0x5000, 2_000_000, BY_HOST);
        t.add(0x1001, K_HOST, 0x77, 1_000_000, BY_NONE);
        t.add(0x1004, K_SYNC, 0x10a2, 500_000, BY_NONE);
        // a host function's wall time is listed but not counted as a wait
        t.add(0x1004, K_CALL, 0x99, 200_000, BY_NONE);
        let a = &t.m[&(0x1001, K_CONDVAR, 0x5000)];
        assert_eq!((a.n, a.ns, a.max), (2, 8_000_000, 6_000_000));
        assert_eq!(a.by[&0x1004], 6_000_000);
        let cpu = HashMap::from([(0x1001u32, 0.1f64), (0x1004, 0.5), (0x1009, 0.0)]);
        let r = report(
            &t,
            &cpu,
            0.1,
            850.0,
            &|w| waker_name(w, &["vk replay".to_string()]),
            &|k, v| if k == K_HOST { "nvnSyncWait".into() } else { key_name(k, v) },
            &|h| format!("e{h:x}"),
        );
        assert!(r[0].starts_with("waits: at 850 s, over 0.1 s: 3 guest threads"), "{}", r[0]);
        assert_eq!(r.len(), 3, "the idle thread with no waits is left out: {r:?}");
        assert_eq!(
            r[1],
            "waits: 0x1001 e1001 cpu 10%, in waits 9%, other 81%: condvar 0x5000 80.0 (20/s, 6.0; by 0x1004 75%, host:vk replay 25%) | host nvnSyncWait 10.0 (10/s, 1.0; by timeout 100%)"
        );
        assert!(r[2].starts_with("waits: 0x1004 e1004 cpu 50%, in waits 5%") && r[2].contains("sync h0x10a2 5.0"), "{}", r[2]);
        assert!(r[2].contains("| call 0x99 2.0 (10/s"), "{}", r[2]);
        assert_eq!(key_name(K_SVC, 0x21 << 32 | 0x10f0), "0x21 h0x10f0");
    }

    #[test]
    fn wakers_are_named() {
        let names = ["audio".to_string()];
        assert_eq!(waker_name(BY_NONE, &names), "timeout");
        assert_eq!(waker_name(BY_HOST, &names), "host:audio");
        assert_eq!(waker_name(BY_HOST | 3, &names), "host:?");
        assert_eq!(waker_name(0x1003, &names), "0x1003");
    }
}
