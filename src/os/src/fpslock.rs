//! The engine frame lock (nxnative `hle_fpslock.c`, reimplemented): some engines cap their own frame rate with a
//! value in game memory the host can raise. The target is the profile's `fps_target` (`NXOS_FPS_TARGET` overrides;
//! 0 = leave the game's own limit). Each present the values are written whenever they differ (the game sets them
//! after boot and may set them again). Kinds of limiter:
//!
//! * rows of fixed fields per main build (FPSLocker's evaluate-write entries, nxnative's table): a frame rate as a
//!   float, double or int, vsyncs per frame, the frame time in milliseconds, ticks, seconds, or an engine's 60 fps
//!   mode switch;
//! * NieR:Automata 1.0.3's display object (its present interval and interval mode), and its resolution floor;
//! * Frostbite's `SimulationTimeSettings` (ForceSimRate / MaxSimFps), found by name in any build (its reflected class
//!   record, the type info its registration stores and the settings lookups' instance caches).
//!
//! nxnative's rewriter-found places (the target frame rate a game steps by, Unreal's console variables and fixed
//! frame rate, Unity's targetFrameRate / vSyncCount, Monolith's interval) come from build-time scans that are not
//! ported yet; they are listed in PORT_PLAN.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    FpsF32,
    VsyncsI32,
    MsF32,
    FpsI32,
    TicksU32,
    SecsF32,
    ModeU32,
    FpsF64,
}
use Kind::*;

/// At 60 a frame time gets 0.5 ms less than the vsync (a limiter counting from the frame's start is late by the
/// work after it); at 30 the game's own 33.333 ms.
pub fn ms_want(fps: u32) -> f32 {
    1000.0 / fps as f32 - if fps == 60 { 0.5 } else { 0.0 }
}

#[derive(Clone, Copy, Debug)]
pub struct Lock {
    pub build: &'static str,
    pub what: &'static str,
    /// A global in main holding the object's address (0: the field is at main + field).
    pub ptr_off: u64,
    pub field: u32,
    pub kind: Kind,
    pub copies: u32,
    pub scale: u32,
}

const fn row(build: &'static str, what: &'static str, ptr_off: u64, field: u32, kind: Kind, copies: u32, scale: u32) -> Lock {
    Lock { build, what, ptr_off, field, kind, copies, scale }
}

/// nxnative's table (each from FPSLocker-Warehouse configs or the engines' own code, see hle_fpslock.c's notes).
pub const LOCKS: &[Lock] = &[
    row("c9a3dd7702075ecd90101f4555389582", "RE Engine frame limit", 0x1295b3f0, 0x434, FpsF32, 1, 0),
    row("79e5950ffa85acf63e28c9aec051eac2", "NLG frame limiter target", 0, 0x13c94dc, MsF32, 1, 0),
    row("799d1061182c1302e08e706b84ce76e5", "Criterion 60 fps mode", 0, 0xf0043c, ModeU32, 1, 0),
    row("799d1061182c1302e08e706b84ce76e5", "Criterion frame ticks", 0xed96a8, 0x30, TicksU32, 1, 19_200_000),
    row("799d1061182c1302e08e706b84ce76e5", "Criterion frame ticks / 10", 0xed96a8, 0x28, TicksU32, 1, 1_920_000),
    row("799d1061182c1302e08e706b84ce76e5", "Criterion 2D game step", 0xeffc60, 0x30, SecsF32, 1, 0),
    row("799d1061182c1302e08e706b84ce76e5", "Criterion 3D game step", 0xeffc60, 0x50, SecsF32, 1, 0),
    row("a48c0de3bc353efa0e4d359bde05ef24", "FIFA frame regulator", 0xda20e38, 0x8, FpsF64, 2, 0),
];

/// NieR:Automata 1.0.3 (US / EU and the Asian release): the display object (its first word the NVN device, whose
/// +0x9c160 is the present interval; +0x504 the interval mode, 0 = 60 fps, 1 = 30) and the dynamic-resolution floor.
pub struct Nier {
    pub build: &'static str,
    pub clamp: u64,
    pub publish: u64,
    pub floor0: u64,
    pub floor1: u64,
    pub minimum: u64,
    pub display: u64,
}
pub const NIER: &[Nier] = &[
    Nier { build: "3c10ec35c5ef4c3686b4053afb0e58e7", clamp: 0x9bf568, publish: 0x9bf578, floor0: 0x9bf528, floor1: 0x9bf534, minimum: 0x128e068, display: 0x192be00 },
    Nier { build: "98397a10419894652f372d4dc1b6fc2c", clamp: 0x9c0928, publish: 0x9c0938, floor0: 0x9c08e8, floor1: 0x9c08f4, minimum: 0x128f130, display: 0x192d600 },
];

/// main as loaded: base, size, the end of its text, the start of its read-only data, its build id (hex).
pub struct Main {
    pub base: u64,
    pub size: u64,
    pub text_end: u64,
    pub ro_off: u64,
    pub build: String,
}

static MAIN: OnceLock<Main> = OnceLock::new();

/// Boot: main's facts (from the loader).
pub fn set_main(m: Main) {
    let _ = MAIN.set(m);
}

/// main's facts once booted.
pub fn main() -> Option<&'static Main> {
    MAIN.get()
}

fn rd32(a: u64) -> Option<u32> {
    nxrt::mem::readable(a, 4).then(|| unsafe { (*(a as *const AtomicU32)).load(Ordering::Relaxed) })
}
fn rd64(a: u64) -> Option<u64> {
    nxrt::mem::readable(a, 8).then(|| unsafe { (*(a as *const AtomicU64)).load(Ordering::Relaxed) })
}
fn wr32(a: u64, v: u32) {
    crate::mem::prepare(a, 4);
    unsafe { (*(a as *const AtomicU32)).store(v, Ordering::Relaxed) }
}
fn wr64(a: u64, v: u64) {
    crate::mem::prepare(a, 8);
    unsafe { (*(a as *const AtomicU64)).store(v, Ordering::Relaxed) }
}

/// The value a field of `kind` should hold for `target` fps, given what it holds (`cur`, its raw bits): None to leave
/// it (already right, or not set by the game yet).
pub fn want(kind: Kind, scale: u32, cur: u32, target: u32) -> Option<u32> {
    let f = f32::from_bits(cur);
    let w = match kind {
        FpsF32 => {
            if !(10.0..=120.0).contains(&f) {
                return None;
            }
            (target as f32).to_bits()
        }
        MsF32 => {
            if !(8.0..=100.0).contains(&f) {
                return None;
            }
            ms_want(target).to_bits()
        }
        SecsF32 => {
            if !(1.0 / 120.0..=0.1).contains(&f) {
                return None;
            }
            (1.0f32 / target as f32).to_bits()
        }
        TicksU32 => {
            if cur < scale / 120 || cur > scale / 10 {
                return None;
            }
            scale / target
        }
        ModeU32 => {
            if cur != 0 {
                return None;
            }
            1
        }
        FpsI32 => {
            if !(10..=120).contains(&cur) {
                return None;
            }
            target
        }
        VsyncsI32 => {
            if !(1..=4).contains(&cur) {
                return None;
            }
            if target > 60 { 1 } else { 60 / target }
        }
        FpsF64 => return None,
    };
    (w != cur).then_some(w)
}

/// Frostbite's SimulationTimeSettings, found by name.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct Frostbite {
    pub ti: u64,
    pub cache: Vec<u64>,
    pub off_rate: u32,
    pub off_max: u32,
}

fn rdu32(b: &[u8], o: u64) -> Option<u32> {
    b.get(o as usize..o as usize + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn rdu64(b: &[u8], o: u64) -> Option<u64> {
    b.get(o as usize..o as usize + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

/// The target of `adrp reg; add reg, reg, #lo` at words i, i + 1 (module offsets), or None.
fn adrp_add(t: &[u32], i: usize, reg: u32) -> Option<u64> {
    let (w, x) = (*t.get(i)?, *t.get(i + 1)?);
    if w & 0x9F00_0000 != 0x9000_0000 || w & 31 != reg || x & 0xFFC0_0000 != 0x9100_0000 || x & 31 != reg || (x >> 5) & 31 != reg {
        return None;
    }
    let mut imm = (((w >> 29) & 3) | ((w >> 5) & 0x7FFFF) << 2) as i64;
    if imm & (1 << 20) != 0 {
        imm -= 1 << 21;
    }
    let v = ((i as i64 * 4) & !0xfff) + imm * 4096 + ((x >> 10) & 0xfff) as i64;
    (v > 0).then_some(v as u64)
}

/// Finds SimulationTimeSettings in a main image whose pointers are relocated for `base` (as loaded).
pub fn frostbite_find(img: &[u8], base: u64, text_end: u64, ro_off: u64) -> Result<Option<Frostbite>, String> {
    let needle = b"\0SimulationTimeSettings\0";
    let size = img.len() as u64;
    if ro_off >= size {
        return Ok(None);
    }
    let Some(pos) = img[ro_off as usize..].windows(needle.len()).position(|w| w == needle) else { return Ok(None) };
    let name = base + ro_off + pos as u64 + 1;
    let name_at = |p: u64, want: &str| -> bool {
        p >= base && p - base + want.len() as u64 + 1 <= size && &img[(p - base) as usize..(p - base) as usize + want.len()] == want.as_bytes() && img[(p - base) as usize + want.len()] == 0
    };
    let mut fb = Frostbite::default();
    let mut rec = 0u64;
    let mut o = (ro_off + 7) & !7;
    while o + 0x48 <= size && rec == 0 {
        if rdu64(img, o) == Some(name) {
            let fl = rdu64(img, o + 0x40).unwrap_or(0);
            if fl >= base && fl - base + 0x20 <= size {
                let mut f = fl - base;
                let (mut rate, mut max) = (0, 0);
                while f + 0x20 <= size {
                    let fname = rdu64(img, f).unwrap_or(0);
                    if fname < base || fname - base >= size {
                        break;
                    }
                    let off = rdu32(img, f + 0x10).unwrap_or(0);
                    if name_at(fname, "ForceSimRate") {
                        rate = off;
                    } else if name_at(fname, "MaxSimFps") {
                        max = off;
                    }
                    f += 0x20;
                }
                if rate != 0 && max != 0 {
                    fb.off_rate = rate;
                    fb.off_max = max;
                    rec = o;
                }
            }
        }
        o += 8;
    }
    if rec == 0 {
        return Err("SimulationTimeSettings found, but not its ForceSimRate and MaxSimFps fields".into());
    }
    let t: Vec<u32> = (0..(text_end.min(size) / 4) as usize).map(|i| rdu32(img, 4 * i as u64).unwrap()).collect();
    // the registration: adrp / add xR = the record, str xR, [xT] with xT the type info from an adrp / add just before
    for i in 0..t.len().saturating_sub(3) {
        if fb.ti != 0 {
            break;
        }
        let r = t[i] & 31;
        if adrp_add(&t, i, r) != Some(rec) {
            continue;
        }
        let st = t[i + 2];
        if st & 0xFFFF_FC00 != 0xF900_0000 || st & 31 != r {
            continue;
        }
        let xt = (st >> 5) & 31;
        for k in i.saturating_sub(6)..i {
            if let Some(ti) = adrp_add(&t, k, xt) {
                fb.ti = ti;
            }
        }
    }
    if fb.ti == 0 {
        return Err("SimulationTimeSettings: its type info was not found".into());
    }
    // the readers: adrp / add x1 = the type info, adrp / add x2 = a cache word, near each other
    for i in 8..t.len().saturating_sub(12) {
        if fb.cache.len() >= 8 {
            break;
        }
        if adrp_add(&t, i, 1) != Some(fb.ti) {
            continue;
        }
        for k in i - 8..i + 10 {
            if let Some(c) = adrp_add(&t, k, 2).filter(|&c| c + 8 <= size) {
                if !fb.cache.contains(&c) {
                    fb.cache.push(c);
                }
                break;
            }
        }
    }
    if fb.cache.is_empty() {
        return Err("SimulationTimeSettings: no settings lookup found".into());
    }
    Ok(Some(fb))
}

struct State {
    rows: Vec<Lock>,
    nier: Option<&'static Nier>,
    fb: Option<Frostbite>,
    target: u32,
    logs: Vec<u32>,
}

static STATE: Mutex<Option<Option<State>>> = Mutex::new(None);

/// The target frame rate: `NXOS_FPS_TARGET`, else the profile's `fps_target` (0: leave the game's limit).
pub fn target() -> u32 {
    let t = std::env::var("NXOS_FPS_TARGET").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or_else(|| {
        profiles::by_title(crate::profile::get().program_id).map_or(0, |p| p.settings.fps_target)
    });
    match t {
        0 | 30 | 60 => t,
        120 => 60,
        _ => {
            crate::log_once!("fps target bad", "[RED] fps lock: frame rate target {t}: must be 0, 30 or 60; the game's own limit stays");
            0
        }
    }
}

fn find() -> Option<State> {
    let m = MAIN.get()?;
    let rows: Vec<Lock> = LOCKS.iter().filter(|l| m.build.starts_with(l.build)).copied().collect();
    let nier = NIER.iter().find(|n| m.build.starts_with(n.build));
    // main as it is in memory now (relocated): the scan reads its pointers as the game does
    let image = crate::guest::bytes(m.base, m.size).to_vec();
    let fb = match frostbite_find(&image, m.base, m.text_end, m.ro_off) {
        Ok(f) => f,
        Err(e) => {
            crate::log!("[RED] fps lock: Frostbite's {e}; leaving the simulation rate");
            None
        }
    };
    if let Some(f) = &fb {
        crate::log!("fps lock: Frostbite SimulationTimeSettings (ForceSimRate +{:#x}, MaxSimFps +{:#x}), type info main+{:#x}, {} instance caches", f.off_rate, f.off_max, f.ti, f.cache.len());
    }
    if rows.is_empty() && nier.is_none() && fb.is_none() {
        return None;
    }
    let target = target();
    if target == 0 {
        for r in &rows {
            crate::log!("fps lock: {} left as the game sets it (no fps_target)", r.what);
        }
        return None;
    }
    let n = rows.len();
    Some(State { rows, nier, fb, target, logs: vec![0; n] })
}

/// Each present: the engine limiters of this main set to the target.
pub fn tick() {
    let mut g = STATE.lock().unwrap();
    let st = g.get_or_insert_with(find);
    let Some(st) = st.as_mut() else { return };
    let Some(m) = MAIN.get() else { return };
    let base = m.base;
    let target = st.target;
    if let Some(n) = st.nier {
        let want = if target > 60 { 1 } else { 60 / target };
        if let Some(mode) = rd32(base + n.display + 0x504) {
            if mode != want - 1 && mode <= 1 {
                wr32(base + n.display + 0x504, want - 1);
            }
        }
        if let Some(dev) = rd64(base + n.display).filter(|&p| p != 0) {
            if let Some(cur) = rd32(dev + 0x9c160) {
                if cur != want && (cur == 1 || cur == 2) {
                    wr32(dev + 0x9c160, want);
                    crate::log_once!("nier interval", "fps lock: NieR frame interval {cur} -> {want} ({target} fps)");
                }
            }
        }
        // full resolution: the preset minimum scale 1.0
        if let Some(cur) = rd32(base + n.minimum) {
            if cur != 0x3f80_0000 {
                wr32(base + n.minimum, 0x3f80_0000);
            }
        }
    }
    if let Some(fb) = &st.fb {
        for &c in &fb.cache {
            let Some(inst) = rd64(base + c).filter(|&i| i != 0) else { continue };
            if rd64(inst + 8) != Some(base + fb.ti) {
                continue;
            }
            let (Some(rate), Some(max)) = (rd32(inst + fb.off_rate as u64), rd32(inst + fb.off_max as u64)) else { continue };
            if rate != target {
                wr32(inst + fb.off_rate as u64, target);
            }
            if f32::from_bits(max) != target as f32 {
                wr32(inst + fb.off_max as u64, (target as f32).to_bits());
            }
            if rate != target || f32::from_bits(max) != target as f32 {
                crate::log_once!("frostbite rate", "fps lock: Frostbite simulation rate ForceSimRate {rate}, MaxSimFps {} -> {target}", f32::from_bits(max));
            }
            break;
        }
    }
    for (r, l) in st.rows.iter().enumerate() {
        let obj = if l.ptr_off != 0 {
            match rd64(base + l.ptr_off) {
                Some(o) if o != 0 => o,
                _ => continue,
            }
        } else {
            base
        };
        for c in 0..l.copies as u64 {
            if l.kind == FpsF64 {
                let a = obj + l.field as u64 + 8 * c;
                let Some(cur) = rd64(a).map(f64::from_bits) else { break };
                if cur != target as f64 && (10.0..=120.0).contains(&cur) {
                    wr64(a, (target as f64).to_bits());
                    if st.logs[r] < 8 {
                        st.logs[r] += 1;
                        crate::log!("fps lock: {} {cur} -> {target} fps", l.what);
                    }
                }
                continue;
            }
            let a = obj + l.field as u64 + 4 * c;
            let Some(cur) = rd32(a) else { break };
            if let Some(w) = want(l.kind, l.scale, cur, target) {
                wr32(a, w);
                if st.logs[r] < 8 {
                    st.logs[r] += 1;
                    crate::log!("fps lock: {} {cur:#x} -> {w:#x} ({target} fps)", l.what);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// nxnative's per-kind rules: what each field becomes at 60 and 30, and what is left alone.
    #[test]
    fn field_values() {
        assert_eq!(want(FpsF32, 0, 30f32.to_bits(), 60), Some(60f32.to_bits()));
        assert_eq!(want(FpsF32, 0, 6000f32.to_bits(), 60), None, "RE Engine's boot value: not set yet");
        assert_eq!(want(MsF32, 0, 33.333f32.to_bits(), 60), Some(ms_want(60).to_bits()));
        assert!((ms_want(60) - 16.1667).abs() < 0.001);
        assert_eq!(want(TicksU32, 19_200_000, 640_000, 60), Some(320_000));
        assert_eq!(want(ModeU32, 0, 0, 60), Some(1));
        assert_eq!(want(ModeU32, 0, 1, 60), None);
        assert_eq!(want(SecsF32, 0, (1.0f32 / 30.0).to_bits(), 60), Some((1.0f32 / 60.0).to_bits()));
        assert_eq!(want(VsyncsI32, 0, 2, 60), Some(1));
        assert_eq!(want(VsyncsI32, 0, 2, 30), None, "already 2 at 30");
        assert_eq!(want(FpsI32, 0, 30, 60), Some(60));
        assert_eq!(LOCKS.len(), 8);
    }

    /// Frostbite's settings found by name in a synthetic image laid out as the engine's: the class name, its record
    /// (name pointer, field list at +0x40), two field records, the registration and a lookup in code.
    #[test]
    fn frostbite_by_name() {
        let base = 0x7000_0000_00u64;
        let mut img = vec![0u8; 0x8000];
        let put32 = |img: &mut Vec<u8>, o: usize, v: u32| img[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let put64 = |img: &mut Vec<u8>, o: usize, v: u64| img[o..o + 8].copy_from_slice(&v.to_le_bytes());
        let ro = 0x4000usize;
        img[ro + 0x10..ro + 0x10 + 24].copy_from_slice(b"\0SimulationTimeSettings\0");
        img[ro + 0x30..ro + 0x30 + 13].copy_from_slice(b"ForceSimRate\0");
        img[ro + 0x40..ro + 0x40 + 10].copy_from_slice(b"MaxSimFps\0");
        let rec = ro + 0x100;
        put64(&mut img, rec, base + ro as u64 + 0x11);
        put64(&mut img, rec + 0x40, base + 0x5000);
        put64(&mut img, 0x5000, base + ro as u64 + 0x30);
        put32(&mut img, 0x5010, 0x44);
        put64(&mut img, 0x5020, base + ro as u64 + 0x40);
        put32(&mut img, 0x5030, 0x48);
        let enc_adrp = |rd: u32, pc: u64, target: u64| -> u32 {
            let imm = ((target & !0xfff) as i64 - (pc & !0xfff) as i64) >> 12;
            let imm = imm as u32 & 0x1F_FFFF;
            0x9000_0000 | (imm & 3) << 29 | (imm >> 2) << 5 | rd
        };
        let enc_add = |rd: u32, lo: u64| 0x9100_0000 | ((lo & 0xfff) as u32) << 10 | rd << 5 | rd;
        let ti = 0x6000u64;
        let cache = 0x6100u64;
        // type info: x9 = ti (words 0x100, 0x104), record: x8 = rec (0x108, 0x10c), str x8, [x9] (0x110)
        let mut code = vec![(0x100u64, enc_adrp(9, 0x100, ti)), (0x104, enc_add(9, ti)), (0x108, enc_adrp(8, 0x108, rec as u64)), (0x10c, enc_add(8, rec as u64)), (0x110, 0xF900_0128)];
        // a lookup: x1 = ti, x2 = cache
        code.extend([(0x200u64, enc_adrp(1, 0x200, ti)), (0x204, enc_add(1, ti)), (0x208, enc_adrp(2, 0x208, cache)), (0x20c, enc_add(2, cache))]);
        for (o, w) in code {
            put32(&mut img, o as usize, w);
        }
        let fb = frostbite_find(&img, base, 0x1000, ro as u64).unwrap().unwrap();
        assert_eq!((fb.off_rate, fb.off_max, fb.ti, fb.cache.clone()), (0x44, 0x48, ti, vec![cache]));
        assert_eq!(frostbite_find(&vec![0u8; 0x2000], base, 0x1000, 0x1000).unwrap(), None, "not Frostbite");
    }
}
