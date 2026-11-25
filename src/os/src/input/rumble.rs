//! HD rumble for the Switch Pro Controller, Joy-Con and Nintendo's Online pads, through RenHDrum's core
//! (`third_party/renhdrum`, docs/waves/RENHDRUM.md). The core turns the game's timestamped values into 0x10 packets
//! (the 5 ms AM/FM envelope over USB, Compatible pacing over Bluetooth, the amplitude table with motor protection, the
//! Switch 2 amplitude laws); it opens no device. This module is the host: one `HdPad` per claimed device, with its own
//! output handle on the device's path; a `rumble` thread that ticks every claimed pad every 1 ms and writes what is
//! due; and the host rules of RenHDrum's recomp bridge, reimplemented: a 250 ms silence watchdog per side, a restate of
//! a running packet (40 ms USB, 150 ms Bluetooth), mute on focus loss / host prompt / PermitVibration(false), and the
//! neutral packet to every pad at exit.
//!
//! A pad is claimed while `hd_rumble` is on (ini, live) and its handle is writable; the standard path (`Dev::rumble`)
//! keeps every other pad, and a player whose pads are not all claimed still gets the standard path for the rest.
//! Locks: the registry, then a pad's state, then the core's own; none is held across a HID write or a join.

use super::{hidio, ilog, Vib, PLAYERS};
use renhdrum::{
    renhdrum_engine_apply_settings, renhdrum_engine_create, renhdrum_engine_destroy,
    renhdrum_engine_push_half_sample, renhdrum_engine_tick, renhdrum_set_nx2_mode,
    renhdrum_settings_resolve_pure, renhdrum_store_create, renhdrum_store_set,
    renhdrum_store_shape_amplitude, renhdrum_store_to_config, RenHdcConfig, RenHdcEngine,
    RenHdcSettings, RenHdcStore, RENHDRUM_OK,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Rumble off on both sides: report 0x10, counter 0, the neutral pattern twice.
pub const NEUTRAL_PACKET: [u8; 10] = [0x10, 0x00, 0x00, 0x01, 0x40, 0x40, 0x00, 0x01, 0x40, 0x40];
const NEUTRAL_SIDE: [u8; 4] = [0x00, 0x01, 0x40, 0x40];
/// A side the game stopped feeding is silenced after this long (the bridge's RH_STALE_US).
pub const STALE_US: i64 = 250_000;
/// A running packet is restated after this long without one (the core leaves it to the transport).
pub const RESEND_USB_US: i64 = 40_000;
pub const RESEND_BT_US: i64 = 150_000;
const PERIOD: Duration = Duration::from_millis(1);

/// Microseconds since the first call (one clock for the game's pushes and the rumble thread's ticks).
pub fn now_us() -> i64 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_micros() as i64
}

/// The Switch 2 amplitude laws (RenHDrum "Mode 2") for a Switch 2 program (title ids 04...), else the Switch 1 ones.
/// Process-wide in the core; set once at boot.
pub fn set_program(program_id: u64) -> i32 {
    let mode = if (nxrt::process::Program { program_id }).switch2() {
        2
    } else {
        0
    };
    renhdrum_set_nx2_mode(mode);
    mode
}

// ---------------------------------------------------------------- settings

/// The ini's rumble keys as HD rumble uses them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HdConfig {
    /// `hd_rumble`: claim Nintendo pads.
    pub hd: bool,
    /// `rumble`: the master switch.
    pub rumble: bool,
    /// `rumble_gain` x100 (0 = silent).
    pub gain_pct: i32,
}

impl HdConfig {
    pub fn from_ini() -> HdConfig {
        let gain = nxconfig::string("rumble_gain")
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .unwrap_or(1.0)
            .clamp(0.0, 3.0);
        HdConfig {
            hd: nxconfig::string("hd_rumble").trim() != "0",
            rumble: nxconfig::string("rumble").trim() != "0",
            gain_pct: (gain * 100.0).round() as i32,
        }
    }

    /// The core's Intensity (`gain`, 25..1000 %).
    pub fn store_gain(&self) -> i32 {
        self.gain_pct.clamp(25, 1000)
    }

    /// RenHDrum's settings store for this configuration: the recommended preset, Bluetooth Compatible (phase 1: no
    /// write-after-input transport yet), Intensity from `rumble_gain`.
    fn store(&self) -> Store {
        let s = Store(renhdrum_store_create());
        let set = |k: &std::ffi::CStr, v: i32| unsafe { renhdrum_store_set(s.0, k.as_ptr(), v) };
        set(c"enable_hd_rumble", 1);
        set(c"switch_bt_mode", 1); // Compatible
        set(c"bt_after_input", 0);
        set(c"gain", self.store_gain());
        s
    }

    /// The core's controller configuration (`RumbleConfigController` through the store).
    pub fn controller_config(&self) -> RenHdcConfig {
        let s = self.store();
        let mut c: RenHdcConfig = unsafe { std::mem::zeroed() };
        unsafe { renhdrum_store_to_config(s.0, &mut c) };
        c
    }

    /// The resolved engine settings (pure: no flag files, no shared-memory app).
    pub fn settings(&self) -> RenHdcSettings {
        let c = self.controller_config();
        let mut out: RenHdcSettings = unsafe { std::mem::zeroed() };
        unsafe { renhdrum_settings_resolve_pure(&c, &mut out) };
        out
    }

    /// One amplitude as the pad gets it: the core's Intensity curve (gain, no low boost).
    pub fn shape(&self, a: f32) -> f32 {
        if self.gain_pct <= 0 {
            return 0.0;
        }
        shaper(self.store_gain(), a)
    }
}

struct Store(*mut RenHdcStore);
// SAFETY: the store is a plain value table; every access here is under SHAPER's lock or owned by one caller
unsafe impl Send for Store {}
impl Drop for Store {
    fn drop(&mut self) {
        unsafe { renhdrum::renhdrum_store_destroy(self.0) };
    }
}

fn shaper(gain: i32, a: f32) -> f32 {
    static S: Mutex<Option<(i32, Store)>> = Mutex::new(None);
    let mut g = S.lock().unwrap();
    if g.as_ref().is_none_or(|(k, _)| *k != gain) {
        let s = Store(renhdrum_store_create());
        unsafe { renhdrum_store_set(s.0, c"gain".as_ptr(), gain) };
        *g = Some((gain, s));
    }
    let s = &g.as_ref().unwrap().1;
    unsafe { renhdrum_store_shape_amplitude(s.0, a) }
}

/// The ini's configuration, re-read when nxconfig reloads.
pub fn config() -> HdConfig {
    static C: Mutex<Option<(u64, HdConfig)>> = Mutex::new(None);
    let gen = nxconfig::generation();
    let mut g = C.lock().unwrap();
    match *g {
        Some((k, c)) if k == gen => c,
        _ => {
            let c = HdConfig::from_ini();
            *g = Some((gen, c));
            c
        }
    }
}

// ---------------------------------------------------------------- one pad

struct Engine(*mut RenHdcEngine);
// SAFETY: RenHdcEngine keeps its state behind its own mutexes (ffi.rs); the pointer is freed once, in Drop
unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}
impl Drop for Engine {
    fn drop(&mut self) {
        unsafe { renhdrum_engine_destroy(self.0) };
    }
}

struct PadSt {
    /// The last packet written (or due), and whether it rumbles.
    last: [u8; 10],
    live: bool,
    last_us: i64,
    /// Per side: the last push, and whether the watchdog has silenced it since.
    at: [i64; 2],
    stale: [bool; 2],
}

/// A claimed Nintendo pad: its engine, its output handle, the packet counter it shares with the input thread's
/// subcommands.
pub struct HdPad {
    engine: Engine,
    pub name: String,
    pub bt: bool,
    /// Bit 0 left, bit 1 right: the sides it rumbles (a Joy-Con its own).
    pub sides: u8,
    counter: AtomicU8,
    ready: AtomicBool,
    out: Mutex<Option<hidio::HidDev>>,
    st: Mutex<PadSt>,
    writes: AtomicU32,
    errors: AtomicU32,
}

impl HdPad {
    /// A pad's engine (`out` None: no device, for tests). None when the core refuses.
    pub fn new(
        name: &str,
        bt: bool,
        pro: bool,
        sides: u8,
        out: Option<hidio::HidDev>,
        cfg: &HdConfig,
    ) -> Option<Arc<HdPad>> {
        let e = renhdrum_engine_create(bt, pro);
        if e.is_null() {
            return None;
        }
        let engine = Engine(e);
        let s = cfg.settings();
        if unsafe { renhdrum_engine_apply_settings(engine.0, &s) } != RENHDRUM_OK {
            return None;
        }
        Some(Arc::new(HdPad {
            engine,
            name: name.to_string(),
            bt,
            sides,
            counter: AtomicU8::new(0),
            ready: AtomicBool::new(false),
            out: Mutex::new(out),
            st: Mutex::new(PadSt {
                last: NEUTRAL_PACKET,
                live: false,
                last_us: 0,
                at: [0; 2],
                stale: [false; 2],
            }),
            writes: AtomicU32::new(0),
            errors: AtomicU32::new(0),
        }))
    }

    /// Claims a device: a second handle on its path for the rumble writes.
    pub fn open(info: &hidio::HidInfo, name: &str, kind: u8) -> Option<Arc<HdPad>> {
        let out = hidio::HidDev::open(info, true).filter(|h| h.writable)?;
        let sides = match kind {
            2 => 1,
            3 => 2,
            _ => 3,
        };
        let pad = HdPad::new(name, info.bluetooth, kind == 1, sides, Some(out), &config())?;
        ilog!(
            "{name}: HD rumble (RenHDrum 0.5.0, {})",
            if info.bluetooth {
                "Bluetooth Compatible"
            } else {
                "USB 5 ms envelope"
            }
        );
        ensure_thread();
        Some(pad)
    }

    /// The next 4-bit packet counter (the input thread's subcommands take theirs from here too).
    pub fn next_counter(&self) -> u8 {
        self.counter.fetch_add(1, Ordering::AcqRel) & 0x0f
    }

    /// The handshake has enabled vibration: packets may go out.
    pub fn set_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    /// The rumble bytes of the last packet (for a subcommand that must not cut a running rumble).
    pub fn last_rumble(&self) -> [u8; 8] {
        let st = self.st.lock().unwrap();
        let mut r = [0u8; 8];
        r.copy_from_slice(&st.last[2..10]);
        r
    }

    /// One of the game's values for a side (amplitudes already shaped), at `now` µs.
    pub fn push_half(&self, now: i64, side: usize, v: Vib) {
        if side > 1 || self.sides >> side & 1 == 0 {
            return;
        }
        let mut st = self.st.lock().unwrap();
        st.at[side] = now.max(1);
        st.stale[side] = false;
        unsafe {
            renhdrum_engine_push_half_sample(
                self.engine.0,
                now,
                side == 1,
                v.amp_low,
                v.freq_low,
                v.amp_high,
                v.freq_high,
            )
        };
    }

    fn push_neutral(&self, now: i64, side: usize) {
        unsafe {
            renhdrum_engine_push_half_sample(self.engine.0, now, side == 1, 0.0, 160.0, 0.0, 320.0)
        };
    }

    /// Both sides silenced now (focus loss, a prompt, the game's PermitVibration(false), rumble turned off).
    pub fn silence(&self, now: i64) {
        let mut st = self.st.lock().unwrap();
        for s in 0..2 {
            if self.sides >> s & 1 != 0 {
                self.push_neutral(now, s);
                st.stale[s] = true;
            }
        }
    }

    /// One tick: the watchdog, the core's tick, the restate. The packet to write now, its counter set.
    pub fn service(&self, now: i64) -> Option<[u8; 10]> {
        let mut st = self.st.lock().unwrap();
        for s in 0..2 {
            if st.at[s] != 0 && !st.stale[s] && now - st.at[s] > STALE_US {
                self.push_neutral(now, s);
                st.stale[s] = true;
            }
        }
        let mut pkt = [0u8; 10];
        let r = unsafe { renhdrum_engine_tick(self.engine.0, now, pkt.as_mut_ptr()) };
        let resend = if self.bt { RESEND_BT_US } else { RESEND_USB_US };
        if r == 1 {
            st.last = pkt;
            st.live = !is_neutral(&pkt);
        } else if st.live && now - st.last_us >= resend {
            pkt = st.last;
        } else {
            return None;
        }
        st.last_us = now;
        pkt[1] = self.next_counter();
        st.last[1] = pkt[1];
        Some(pkt)
    }

    /// Released (hd_rumble turned off, or the pad leaves): the neutral packet now, from the caller's thread.
    pub fn release(&self) {
        self.silence(now_us());
        let mut pkt = NEUTRAL_PACKET;
        pkt[1] = self.next_counter();
        self.write(&pkt);
        let mut st = self.st.lock().unwrap();
        st.last = pkt;
        st.live = false;
    }

    /// Writes a packet (never queued: the write is synchronous on the rumble thread, so the next tick carries the
    /// newest state). Nothing goes out before the handshake has enabled vibration.
    fn write(&self, pkt: &[u8; 10]) {
        if !self.ready.load(Ordering::Acquire) {
            return;
        }
        let mut g = self.out.lock().unwrap();
        let Some(h) = g.as_mut() else { return };
        let e = h.write(pkt);
        if e != 0 {
            if self.errors.fetch_add(1, Ordering::Relaxed) < 8 {
                ilog!("{}: HD rumble write FAILED error={e}", self.name);
            }
        } else if self.writes.fetch_add(1, Ordering::Relaxed) < 4 || super::trace() {
            ilog!("{}: HD rumble {:02x?}", self.name, pkt);
        }
    }
}

/// Whether both sides of a 0x10 packet are the neutral pattern.
pub fn is_neutral(p: &[u8; 10]) -> bool {
    p[2..6] == NEUTRAL_SIDE && p[6..10] == NEUTRAL_SIDE
}

/// The standard path's encoder for one side: RenHDrum's table encoder (with motor-overdrive protection).
pub fn encode_side(v: &Vib) -> [u8; 4] {
    let mut s = [0u8; 4];
    renhdrum::hd_encode_side(&mut s, v.amp_low, v.freq_low, v.amp_high, v.freq_high);
    s
}

// ---------------------------------------------------------------- the registry

struct Reg {
    pads: [Vec<Arc<HdPad>>; PLAYERS],
    /// Rumble-capable devices of the player that are not claimed (they keep the standard path).
    others: [u32; PLAYERS],
}

static REG: Mutex<Reg> = Mutex::new(Reg {
    pads: [const { Vec::new() }; PLAYERS],
    others: [0; PLAYERS],
});
/// Muted: no focus, a prompt, or the game withheld permission. Pushes are dropped meanwhile.
static MUTED: AtomicBool = AtomicBool::new(false);
static PERMITTED: AtomicBool = AtomicBool::new(true);
static SHUT: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

/// The input thread: the claimed pads by player, and how many unclaimed rumble pads each player has.
pub fn sync(claimed: &[(usize, Arc<HdPad>)], others: [u32; PLAYERS]) {
    let mut pads: [Vec<Arc<HdPad>>; PLAYERS] = [const { Vec::new() }; PLAYERS];
    for (p, h) in claimed {
        if *p < PLAYERS {
            pads[*p].push(h.clone());
        }
    }
    let mut r = REG.lock().unwrap();
    let same = r.others == others
        && r.pads
            .iter()
            .zip(pads.iter())
            .all(|(a, b)| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(x, y)));
    if !same {
        r.pads = pads;
        r.others = others;
    }
}

fn all_pads() -> Vec<Arc<HdPad>> {
    REG.lock().unwrap().pads.iter().flatten().cloned().collect()
}

/// The game's value for player p's side, raw (cleaned, before the standard path's settings). Returns whether every
/// rumble pad of that player is claimed (then the standard path has nothing to do for it).
pub fn push(p: usize, side: usize, v: Vib, now: i64) -> bool {
    push_with(p, side, v, now, &config())
}

pub fn push_with(p: usize, side: usize, v: Vib, now: i64, cfg: &HdConfig) -> bool {
    if p >= PLAYERS {
        return false;
    }
    let (pads, all) = {
        let r = REG.lock().unwrap();
        if r.pads[p].is_empty() {
            return false;
        }
        (r.pads[p].clone(), r.others[p] == 0)
    };
    if cfg.rumble && !MUTED.load(Ordering::Acquire) {
        let v = Vib {
            amp_low: cfg.shape(v.amp_low),
            amp_high: cfg.shape(v.amp_high),
            ..v
        };
        for h in &pads {
            h.push_half(now, side, v);
        }
    }
    all
}

/// nn::hid::system::PermitVibration: false silences every pad and mutes until it is permitted again.
pub fn permit(on: bool) {
    if PERMITTED.swap(on, Ordering::AcqRel) != on {
        ilog!(
            "vibration {}",
            if on {
                "permitted"
            } else {
                "withheld by the game"
            }
        );
    }
}
pub fn permitted() -> bool {
    PERMITTED.load(Ordering::Acquire)
}

fn ensure_thread() {
    if SHUT.load(Ordering::Acquire) {
        return;
    }
    let mut t = THREAD.lock().unwrap();
    if t.as_ref().is_some_and(|h| !h.is_finished()) {
        return;
    }
    *t = std::thread::Builder::new()
        .name("rumble".into())
        .spawn(run)
        .ok();
}

/// The rumble thread: every 1 ms, each claimed pad's watchdog, tick and write. Ends after a second without pads.
fn run() {
    let mut idle = 0u32;
    while !SHUT.load(Ordering::Acquire) {
        let now = now_us();
        let pads = all_pads();
        let mute = !super::active() || !permitted() || !config().rumble;
        if MUTED.swap(mute, Ordering::AcqRel) != mute && mute {
            for h in &pads {
                h.silence(now);
            }
        }
        for h in &pads {
            if let Some(pkt) = h.service(now) {
                h.write(&pkt);
            }
        }
        if pads.is_empty() {
            idle += 1;
            if idle > 1000 {
                break;
            }
        } else {
            idle = 0;
        }
        std::thread::sleep(PERIOD);
    }
}

/// At exit: the thread stopped (waiting at most 200 ms), then the neutral packet to every claimed pad.
pub fn shutdown() {
    SHUT.store(true, Ordering::Release);
    let h = THREAD.lock().unwrap().take();
    if let Some(h) = h {
        for _ in 0..200 {
            if h.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if h.is_finished() {
            let _ = h.join();
        }
    }
    for h in all_pads() {
        let mut pkt = NEUTRAL_PACKET;
        pkt[1] = h.next_counter();
        h.write(&pkt);
        if super::trace() {
            ilog!("{}: HD rumble neutral at exit", h.name);
        }
    }
    sync(&[], [0; PLAYERS]);
}

/// Claimed pads now, and how many have finished their handshake (for `nxinput rumble --hd`).
pub fn claimed() -> (usize, usize) {
    let p = all_pads();
    let ready = p.iter().filter(|h| h.ready.load(Ordering::Acquire)).count();
    (p.len(), ready)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry and the core's nx2 mode are process-wide: one test at a time touches them.
    static LOCK: Mutex<()> = Mutex::new(());

    fn cfg() -> HdConfig {
        HdConfig {
            hd: true,
            rumble: true,
            gain_pct: 100,
        }
    }

    fn note(amp: f32, freq: f32) -> Vib {
        Vib {
            amp_low: amp * 0.5,
            freq_low: freq / 2.0,
            amp_high: amp,
            freq_high: freq,
        }
    }

    /// Runs the pad's ticks every 1 ms from `from` to `to` µs, collecting what would be written.
    fn ticks(h: &HdPad, from: i64, to: i64) -> Vec<(i64, [u8; 10])> {
        (from..to)
            .step_by(1000)
            .filter_map(|t| h.service(t).map(|p| (t, p)))
            .collect()
    }

    /// Plan test 1: a note sequence on both sides of a USB Pro pad gives rumbling 0x10 packets whose counter is the shared one;
    /// 300 ms of silence after it gives the neutral packet (the watchdog).
    #[test]
    fn engine_path_packets_counter_watchdog() {
        let h = HdPad::new("test pro", false, true, 3, None, &cfg()).unwrap();
        let mut t = 1_000_000;
        let mut out = vec![];
        for (k, f) in [160.0f32, 320.0, 640.0, 1000.0].iter().enumerate() {
            for _ in 0..20 {
                let v = note(0.4 + 0.1 * k as f32, *f);
                h.push_half(t, 0, v);
                h.push_half(t, 1, v);
                out.extend(ticks(&h, t, t + 5000));
                t += 5000;
            }
        }
        assert!(out.len() > 10, "packets: {}", out.len());
        assert!(out.iter().all(|(_, p)| p[0] == 0x10));
        assert!(out.iter().any(|(_, p)| !is_neutral(p)), "rumbling packets");
        // counters run 0, 1, 2 ... mod 16 in write order: the shared counter, and a subcommand takes the next one
        for (i, (_, p)) in out.iter().enumerate() {
            assert_eq!(p[1], (i as u8) & 0x0f);
        }
        let sub = h.next_counter();
        let after = ticks(&h, t, t + 300_000);
        assert!(!after.is_empty());
        assert_eq!(after[0].1[1], (sub + 1) & 0x0f, "the counter is shared");
        let last = after.last().unwrap().1;
        assert!(
            is_neutral(&last),
            "silence: neutral at the end, got {last:02x?}"
        );
        let stop = after.iter().find(|(_, p)| is_neutral(p)).unwrap().0;
        // the watchdog's neutral goes through the core's envelope window (up to ~30 ms)
        assert!(
            stop - t <= STALE_US + 40_000,
            "stopped {} µs after the last push",
            stop - t
        );
        assert!(stop - t >= STALE_US - 10_000, "not before the watchdog");
    }

    /// Plan test 2: a held value is restated after 40 ms on USB and 150 ms on Bluetooth; a neutral one is not.
    #[test]
    fn resend_usb_bt_not_neutral() {
        for (bt, gap) in [(false, RESEND_USB_US), (true, RESEND_BT_US)] {
            let h = HdPad::new("test", bt, true, 3, None, &cfg()).unwrap();
            let v = note(0.6, 320.0);
            let t0 = 2_000_000;
            // the game holds the value (pushes it every 16 ms) for 200 ms: after the first packet the core may stay
            // quiet; the host restates it
            let mut seen = vec![];
            let mut t = t0;
            while t < t0 + 200_000 {
                if (t - t0) % 16_000 == 0 {
                    h.push_half(t, 0, v);
                    h.push_half(t, 1, v);
                }
                if let Some(p) = h.service(t) {
                    seen.push((t, p));
                }
                t += 1000;
            }
            assert!(seen.iter().all(|(_, p)| !is_neutral(p)), "bt={bt}");
            let max_gap = seen
                .windows(2)
                .map(|w| w[1].0 - w[0].0)
                .max()
                .unwrap_or(i64::MAX);
            assert!(
                max_gap <= gap + 1000,
                "bt={bt}: a {max_gap} µs gap between packets"
            );
            // neutral: after the stop goes out, nothing is restated
            h.silence(t);
            let after = ticks(&h, t, t + 200_000);
            let first_neutral = after.iter().position(|(_, p)| is_neutral(p)).unwrap();
            assert!(
                after[first_neutral..].iter().all(|(_, p)| is_neutral(p)),
                "bt={bt}"
            );
            assert!(
                after.len() - first_neutral <= 1,
                "bt={bt}: neutral restated"
            );
        }
    }

    /// Plan test 3, claims: one claimed and one unclaimed pad keep the standard path; all claimed leave it alone; `rumble = 0`
    /// pushes nothing.
    #[test]
    fn claims_and_master_switch() {
        let _g = LOCK.lock().unwrap();
        let p = PLAYERS - 2;
        let h = HdPad::new("test", false, true, 3, None, &cfg()).unwrap();
        let mut others = [0u32; PLAYERS];
        others[p] = 1;
        sync(&[(p, h.clone())], others);
        let v = note(0.8, 320.0);
        assert!(
            !push_with(p, 0, v, 5_000_000, &cfg()),
            "an unclaimed pad remains"
        );
        // through input::vibrate_with: the standard store updates
        super::super::RUMBLE.lock().unwrap().dirty[p] = false;
        super::super::vibrate(p, 0, v);
        assert!(super::super::RUMBLE.lock().unwrap().dirty[p]);
        sync(&[(p, h.clone())], [0; PLAYERS]);
        assert!(push_with(p, 0, v, 5_001_000, &cfg()), "all claimed");
        super::super::RUMBLE.lock().unwrap().dirty[p] = false;
        super::super::vibrate(p, 1, note(0.3, 200.0));
        assert!(
            !super::super::RUMBLE.lock().unwrap().dirty[p],
            "all claimed: the standard store is untouched"
        );
        // rumble = 0: nothing reaches a fresh pad's engine
        let q = HdPad::new("test2", false, true, 3, None, &cfg()).unwrap();
        sync(&[(p, q.clone())], [0; PLAYERS]);
        let off = HdConfig {
            rumble: false,
            ..cfg()
        };
        for k in 0..20 {
            assert!(push_with(p, 0, v, 6_000_000 + k * 5000, &off));
        }
        assert!(
            ticks(&q, 6_000_000, 6_200_000).is_empty(),
            "rumble=0 pushed something"
        );
        sync(&[], [0; PLAYERS]);
    }

    /// Plan test 4, settings: the ini keys through RenHDrum's store, `RumbleConfigController` and `resolve_pure`.
    #[test]
    fn settings_from_ini_keys() {
        let c = HdConfig {
            gain_pct: 200,
            ..cfg()
        };
        assert_eq!(c.store_gain(), 200, "rumble_gain 2.0 is a gain of 200");
        assert!((c.shape(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(c.shape(0.8), 1.0);
        assert_eq!(
            HdConfig { gain_pct: 0, ..c }.shape(0.8),
            0.0,
            "rumble_gain 0 is silent"
        );
        assert_eq!(
            HdConfig {
                gain_pct: 5000,
                ..c
            }
            .store_gain(),
            1000
        );
        let rc = c.controller_config();
        assert!(rc.enable_hd_rumble);
        assert!(!rc.hd_rumble_experimental_bt_fast, "Bluetooth Compatible");
        assert!(!rc.hd_rumble_experimental_bt_after_input);
        assert_eq!(rc.hd_rumble_bluetooth_level, 4);
        let s = c.settings();
        assert!(s.enabled && s.envelope_5ms && s.usb_no_queue && !s.bt_fast);
        assert_eq!(s.bluetooth_level, 4);
        // hd_rumble = 0: the input thread claims nothing (`Inputs::hd_update`), and the standard path keeps the pad
        let _g = LOCK.lock().unwrap();
        let p = PLAYERS - 3;
        sync(&[], [0; PLAYERS]);
        assert!(!push_with(
            p,
            0,
            note(0.5, 320.0),
            1,
            &HdConfig { hd: false, ..c }
        ));
    }

    /// Plan test 5: nx2 mode from the program id (process-wide in the core).
    #[test]
    fn nx2_mode_from_program() {
        let _g = LOCK.lock().unwrap();
        assert_eq!(set_program(0x0400_74A0_1BF1_2000), 2);
        assert_eq!(renhdrum::nx2::get_nx2_mode(), 2);
        assert_eq!(set_program(0x0100_9650_1733_8800), 0);
        assert_eq!(renhdrum::nx2::get_nx2_mode(), 0);
    }

    /// Plan test 6: the standard path's encoder: RenHDrum's table (replaces nintendo.rs's `rumble_encoding`).
    #[test]
    fn legacy_encoder_table() {
        assert_eq!(
            encode_side(&Vib::default()),
            NEUTRAL_SIDE,
            "silence is neutral"
        );
        let hi = encode_side(&Vib {
            amp_high: 1.0,
            ..Default::default()
        });
        // 320 Hz: round(log2(32) * 32) = 160, (160 - 0x60) * 4 = 0x100; amplitude index 100 → 200, plus the 9th bit
        assert_eq!(hi, [0x00, 201, 0x40, 0x40]);
        let lo = encode_side(&Vib {
            amp_low: 1.0,
            ..Default::default()
        });
        // 160 Hz: round(log2(16) * 32) - 0x40 = 0x40; index 100 (even) → 0x40 + 50 = 114
        assert_eq!(lo, [0x00, 0x01, 0x40, 114]);
        // both at full scale: motor protection halves each band
        let both = encode_side(&Vib {
            amp_low: 1.0,
            amp_high: 1.0,
            ..Default::default()
        });
        assert!(both[1] < 201 && both[3] < 114, "{both:02x?}");
    }
}
