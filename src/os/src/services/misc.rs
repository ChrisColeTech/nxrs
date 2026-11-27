//! The rest of the SDK calls a boot reaches that have more than a constant answer: the display's vsync event (a
//! host thread signals it at 60 Hz), nn::diag (the game's log lines and aborts into the host log), add-on content
//! (the titles installed in the data folder), the shared fonts (a stand-in TTF: the system's own fonts are
//! firmware content), amiibo (one NFC device that never finds a tag), the profanity filter (no word list: nothing
//! is profanity; objects get a host vtable), and the error and shop applets (logged, nothing shown).

use crate::guest::{cstr, r32, r64, w32, w64, Args};
use crate::hle::{Registry, RES_OK};
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

macro_rules! mfn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

// ---- vsync

static VSYNC: Mutex<Vec<u64>> = Mutex::new(Vec::new());

unsafe fn vsync_event(c: *mut Cpu) {
    let ev = (*c).x[0];
    crate::services::os::bind_system_event(c, ev, true);
    let mut g = VSYNC.lock().unwrap();
    if !g.contains(&ev) {
        g.push(ev);
        std::thread::Builder::new()
            .name("vsync".into())
            .spawn(move || {
                let period = std::time::Duration::from_nanos(16_666_667);
                let mut next = std::time::Instant::now() + period;
                loop {
                    let now = std::time::Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    }
                    // the guest clock's rate (crate::clock): a slowed clock slows the display too
                    next += period.div_f64(crate::clock::rate());
                    if !crate::services::os::signal_bound(ev) {
                        break;
                    }
                }
            })
            .ok();
        crate::log!("vi::GetDisplayVsyncEvent: {ev:#x} signalled by the host at 60 Hz");
    }
    (*c).x[0] = RES_OK;
}

// ---- diag

/// printf for the guest: integer and pointer arguments from `ints`, doubles from `floats`, strings read from
/// guest memory.
pub fn format(
    fmt: &str,
    ints: &mut dyn Iterator<Item = u64>,
    floats: &mut dyn Iterator<Item = f64>,
) -> String {
    let mut out = String::new();
    let b = fmt.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'%' {
            out.push(b[i] as char);
            i += 1;
            continue;
        }
        i += 1;
        if i < b.len() && b[i] == b'%' {
            out.push('%');
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b"-+ #0123456789.*hlzjtLq".contains(&b[i]) {
            i += 1;
        }
        let spec = &fmt[start..i];
        let Some(&conv) = b.get(i) else { break };
        i += 1;
        let width: usize = spec
            .trim_start_matches(['-', '+', ' ', '#', '0'])
            .split('.')
            .next()
            .unwrap_or("")
            .parse()
            .unwrap_or(0);
        let long = spec.contains('l')
            || spec.contains('z')
            || spec.contains('j')
            || spec.contains('t')
            || spec.contains('q');
        let s = match conv {
            b'd' | b'i' => {
                let v = ints.next().unwrap_or(0);
                if long {
                    (v as i64).to_string()
                } else {
                    (v as i32).to_string()
                }
            }
            b'u' => {
                let v = ints.next().unwrap_or(0);
                if long {
                    v.to_string()
                } else {
                    (v as u32).to_string()
                }
            }
            b'x' | b'X' => {
                let v = ints.next().unwrap_or(0);
                let v = if long { v } else { v as u32 as u64 };
                if conv == b'x' {
                    format!("{v:x}")
                } else {
                    format!("{v:X}")
                }
            }
            b'p' => format!("{:#x}", ints.next().unwrap_or(0)),
            b'c' => ((ints.next().unwrap_or(0) as u8) as char).to_string(),
            b's' => {
                let p = ints.next().unwrap_or(0);
                if p == 0 {
                    "(null)".into()
                } else {
                    cstr(p, 1024)
                }
            }
            b'f' | b'F' | b'e' | b'g' | b'G' => {
                let prec: usize = spec
                    .split('.')
                    .nth(1)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(6);
                format!("{:.*}", prec, floats.next().unwrap_or(0.0))
            }
            _ => format!("%{spec}{}", conv as char),
        };
        if s.len() < width {
            out.push_str(&" ".repeat(width - s.len()));
        }
        out.push_str(&s);
    }
    out
}

/// AArch64 va_list {stack, gr_top, vr_top, i32 gr_offs, i32 vr_offs}: the integer and FP argument streams.
fn va_streams(va: u64) -> (Vec<u64>, Vec<f64>) {
    let (stack, gr_top, vr_top) = (r64(va), r64(va + 8), r64(va + 16));
    let (mut gr, mut vr) = (r32(va + 24) as i32 as i64, r32(va + 28) as i32 as i64);
    let mut ints = vec![];
    let mut sp = stack;
    for _ in 0..24 {
        if gr < 0 {
            ints.push(r64((gr_top as i64 + gr) as u64));
            gr += 8;
        } else {
            ints.push(if nxrt::mem::readable(sp, 8) {
                r64(sp)
            } else {
                0
            });
            sp += 8;
        }
    }
    let mut floats = vec![];
    for _ in 0..8 {
        if vr < 0 {
            floats.push(f64::from_bits(r64((vr_top as i64 + vr) as u64)));
            vr += 16;
        } else {
            floats.push(0.0);
        }
    }
    (ints, floats)
}

fn game_line(s: &str) {
    let s = s.trim_end_matches(['\n', '\0']);
    if !s.is_empty() {
        crate::log!("[game] {s}");
    }
}

mfn!(diag_put, |a| {
    let (s, n) = (a.x(1), a.x(2).min(4096));
    game_line(&String::from_utf8_lossy(crate::guest::bytes(s, n)));
    0
});
unsafe fn diag_log(c: *mut Cpu) {
    let cpu = &mut *c;
    let fmt = cstr(cpu.x[1], 1024);
    let mut ints = (2..32).map(|i| cpu.arg_x(i));
    let mut floats = (0..8).map(|i| f64::from_bits(cpu.v[i] as u64));
    game_line(&format(&fmt, &mut ints, &mut floats));
}
unsafe fn diag_vlog(c: *mut Cpu) {
    let cpu = &mut *c;
    let fmt = cstr(cpu.x[1], 1024);
    let (i, f) = va_streams(cpu.x[2]);
    game_line(&format(&fmt, &mut i.into_iter(), &mut f.into_iter()));
}
unsafe fn diag_abort(c: *mut Cpu) {
    let cpu = &mut *c;
    let name = nxrt::hostfn::name_of(cpu.pc).unwrap_or_default();
    // AbortImpl(const char *condition, const char *function, const char *file, int line, ...)
    let (cond, func, file, line) = (
        cstr(cpu.x[0], 256),
        cstr(cpu.x[1], 256),
        cstr(cpu.x[2], 256),
        cpu.x[3] as i32,
    );
    let msg = if name.contains("10VAbortImpl") {
        let (i, f) = va_streams(cpu.arg_x(7));
        format(
            &cstr(cpu.x[6], 1024),
            &mut i.into_iter(),
            &mut f.into_iter(),
        )
    } else if name.contains("PKNS_6ResultES3_z") {
        let fmt = cstr(cpu.x[5], 1024);
        let mut ints = (6..32).map(|i| cpu.arg_x(i));
        format(
            &fmt,
            &mut ints,
            &mut (0..8).map(|i| f64::from_bits(cpu.v[i] as u64)),
        )
    } else if name.contains("iS3_z") {
        let fmt = cstr(cpu.x[4], 1024);
        let mut ints = (5..32).map(|i| cpu.arg_x(i));
        format(
            &fmt,
            &mut ints,
            &mut (0..8).map(|i| f64::from_bits(cpu.v[i] as u64)),
        )
    } else {
        String::new()
    };
    let res = if name.contains("PKNS_6ResultE") && cpu.x[4] != 0 {
        r32(cpu.x[4])
    } else {
        0
    };
    crate::log!("[RED] the game aborted (nn::diag): {cond} in {func} ({file}:{line}) Result {res:#x}: {msg}");
    crate::log::report_gaps("guest abort");
    crate::nvn::report();
    nxrt::rt::guest_fatal(c, "nn::diag abort");
}
/// AbortImpl() and AbortImpl(const Result *): no message, the Result when there is one, and the call site.
unsafe fn diag_abort_short(c: *mut Cpu) {
    let cpu = &mut *c;
    let name = nxrt::hostfn::name_of(cpu.pc).unwrap_or_default();
    let res = if name.ends_with("PKNS_6ResultE") && cpu.x[0] != 0 {
        r32(cpu.x[0])
    } else {
        0
    };
    crate::log!(
        "[RED] the game aborted (nn::diag) Result {res:#x}, called from {}",
        nxrt::dispatch::guest_name(cpu.x[30])
    );
    crate::log::report_gaps("guest abort");
    crate::nvn::report();
    nxrt::rt::guest_fatal(c, "nn::diag abort");
}
mfn!(diag_sym, |a| {
    if a.x(0) != 0 && a.x(1) != 0 {
        crate::guest::w8(a.x(0), 0);
    }
    0
});
mfn!(zero, |a| { 0 });

// ---- add-on content

mfn!(aoc_count, |a| {
    crate::services::fs::aoc_indices().len() as u64
});
mfn!(aoc_list, |a| {
    // ListAddOnContent(int *out, int offset, int count)
    let (out, off, n) = (a.x(0), a.i(1).max(0) as usize, a.i(2));
    let v = crate::services::fs::aoc_indices();
    let mut w = 0u64;
    for idx in v.iter().skip(off) {
        if w as i32 >= n {
            break;
        }
        w32(out + 4 * w, *idx);
        w += 1;
    }
    crate::log_once!(
        "aoc list",
        "aoc: {} add-on content installed: {:?}",
        v.len(),
        v
    );
    w
});
unsafe fn aoc_event(c: *mut Cpu) {
    let ev = (*c).x[0];
    crate::services::os::bind_system_event(c, ev, false);
}

// ---- shared fonts

/// The six shared font slots (pl::SharedFontType): the console's are firmware content, so each slot is the player's
/// own `<data>\fonts\<Name>.bfttf` (decrypted) or `.ttf` when there is one, else one system
/// TTF for every slot (Arial, Segoe UI, Tahoma; `NXOS_FONT` names another).
const FONT_NAMES: [&str; 6] = [
    "FontStandard",
    "FontChineseSimplified",
    "FontExtendedChineseSimplified",
    "FontChineseTraditional",
    "FontKorean",
    "FontNintendoExtended",
];

#[derive(Clone, Copy)]
struct Font {
    addr: u64,
    size: u64,
}

/// A BFTTF file's TTF (the console's XOR obfuscation: key from its first word, big-endian words).
pub fn bfttf_decrypt(d: &[u8]) -> Option<Vec<u8>> {
    const RESULT: u32 = 0x7f9a_0218;
    if d.len() < 8 {
        return None;
    }
    let w = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let k2 = w(0) ^ RESULT;
    let tsz = ((w(4) ^ k2).swap_bytes() as usize).min(d.len() - 8);
    let mut out = Vec::with_capacity(d.len() - 8);
    let mut k = 0;
    while k + 8 < d.len() {
        let x = if k + 12 <= d.len() { w(8 + k) } else { 0 };
        out.extend_from_slice(&(x ^ k2).swap_bytes().to_le_bytes());
        k += 4;
    }
    out.truncate(tsz);
    Some(out)
}

/// The font file for a slot: the player's file in the data folder's `fonts\`, else None.
fn slot_file(dir: &std::path::Path, name: &str) -> Option<(Vec<u8>, std::path::PathBuf)> {
    let b = dir.join(format!("{name}.bfttf"));
    if let Ok(d) = std::fs::read(&b) {
        return bfttf_decrypt(&d).map(|t| (t, b));
    }
    let t = dir.join(format!("{name}.ttf"));
    std::fs::read(&t).ok().map(|d| (d, t))
}

fn fallback_ttf() -> Option<(Vec<u8>, std::path::PathBuf)> {
    if let Ok(p) = std::env::var("NXOS_FONT") {
        return std::fs::read(&p).ok().map(|d| (d, p.into()));
    }
    // the web: a page sees no system fonts (without one the game waited for its shared font for ever at Anime Girls'
    // first level load), and a system font may not be redistributed in a web build: the repository's own OFL font
    // (Hind, LICENSES/hind-ofl.txt, which the web build carries beside the page)
    #[cfg(target_os = "emscripten")]
    {
        static HIND: &[u8] = include_bytes!("../../third_party/fonts/Hind-Regular.ttf");
        return Some((HIND.to_vec(), "Hind-Regular.ttf (built in)".into()));
    }
    #[allow(unreachable_code)]
    let w = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    ["arial.ttf", "segoeui.ttf", "tahoma.ttf"]
        .iter()
        .map(|n| std::path::Path::new(&w).join("Fonts").join(n))
        .find_map(|p| std::fs::read(&p).ok().map(|d| (d, p)))
}

/// A TTF placed in guest memory behind its BFTTF header (a "decrypted" marker and the length, XOR-keyed).
fn place(ttf: &[u8]) -> Font {
    let n = ttf.len() as u64;
    let a = nxrt::kernel::get().space.kernel_alloc(n + 8, 0x1000);
    w32(a, 0x1802_9a7f);
    w32(a + 4, (n as u32 ^ 0x4962_1806).swap_bytes());
    crate::guest::write_bytes(a + 8, ttf);
    Font {
        addr: a,
        size: n + 8,
    }
}

fn fonts() -> &'static [Option<Font>; 6] {
    static F: OnceLock<[Option<Font>; 6]> = OnceLock::new();
    F.get_or_init(|| {
        let dir = crate::boot::try_process().map(|p| p.data_dir.join("fonts")).unwrap_or_default();
        let mut fb: Option<Font> = None;
        let mut out = [None; 6];
        for (i, name) in FONT_NAMES.iter().enumerate() {
            if let Some((ttf, path)) = slot_file(&dir, name) {
                crate::log!("pl: {name} <- {} ({} bytes)", path.display(), ttf.len());
                out[i] = Some(place(&ttf));
                continue;
            }
            if fb.is_none() {
                if let Some((ttf, path)) = fallback_ttf() {
                    crate::log!("pl: the shared fonts without a file in {} are the system's {} ({} bytes): the console's are firmware content", dir.display(), path.display(), ttf.len());
                    fb = Some(place(&ttf));
                }
            }
            out[i] = fb;
        }
        if out.iter().all(|f| f.is_none()) {
            crate::log!("[RED] pl: no font found: the game's system text will not show");
        }
        out
    })
}

fn font_of(ty: u32) -> Option<Font> {
    fonts().get(ty as usize).copied().flatten().or(fonts()[0])
}

mfn!(font_state, |a| { font_of(a.w(0)).is_some() as u64 });
mfn!(font_addr, |a| { font_of(a.w(0)).map_or(0, |f| f.addr + 8) });
mfn!(font_size, |a| { font_of(a.w(0)).map_or(0, |f| f.size - 8) });
mfn!(shared_font, |a| {
    // GetSharedFont(int *count, SharedFontData *out, int max, LanguageCode): {u32 type, pad, address, size} per type
    let (count, out, max) = (a.x(0), a.x(1), a.i(2));
    let mut n = 0u64;
    for t in 0..6u32 {
        let Some(f) = font_of(t) else { continue };
        if (n as i32) < max {
            let e = out + 0x18 * n;
            w32(e, t);
            w32(e + 4, 0);
            w64(e + 8, f.addr + 8);
            w64(e + 16, f.size - 8);
        }
        n += 1;
    }
    w32(count, n as u32);
    (n > 0) as u64
});

/// One built-in font (SDK 21's pl): {u8 type, u8 flag, pad, address, u64 size}: the slot of that number.
fn builtin_font(out: u64, ty: u32) -> bool {
    let Some(f) = font_of(ty) else { return false };
    crate::guest::zero(out, 0x18);
    crate::guest::w8(out, ty as u8);
    w64(out + 8, f.addr + 8);
    w64(out + 16, f.size - 8);
    true
}
mfn!(builtin_font_fn, |a| { builtin_font(a.x(0), a.w(1)) as u64 });
// GetBuiltInFontDataSet(int *count, BuiltInFontData *out, const u8 *types, int n, LanguageCode, bool): the types asked
// for, in order, at most 6 (Switch 2 types) or 7 (Switch 1 types); the Switch 2 form writes the count even when
// nothing is loaded.
mfn!(builtin_font_set, |a| {
    let name = nxrt::hostfn::name_of(a.0.pc).unwrap_or_default();
    let ounce = name.contains("Ounce");
    let (count, out, types, n) = (a.x(0), a.x(1), a.x(2), a.i(3));
    let max = if ounce { 6 } else { 7 };
    let mut k = 0u64;
    for i in 0..n.clamp(0, max) as u64 {
        if builtin_font(out + 0x18 * k, crate::guest::r8(types + i) as u32) {
            k += 1;
        }
    }
    if k > 0 || ounce {
        w32(count, k as u32);
    }
    (k > 0) as u64
});
/// ScalableFontEngine::LoadFont(self, name, data, index, capacity): a font handed over at its BFTTF header goes to
/// the SDK as the TTF behind it; parsing and errors stay the SDK's.
unsafe fn font_load(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (this, name, mut data, index, cap) = (a.x(0), a.x(1), a.x(2), a.x(3), a.x(4));
    if let Some(f) = fonts().iter().flatten().find(|f| f.addr == data) {
        data = f.addr + 8;
    }
    let Some(orig) = crate::hle::sym("_ZN2nn6fontll18ScalableFontEngine8LoadFontEPcPKvjj") else {
        nxrt::rt::guest_fatal(c, "ScalableFontEngine::LoadFont is not in the SDK");
    };
    if let Some(r) = crate::guest::call(c, orig, &[this, name, data, index, cap]) {
        (*c).x[0] = r;
    }
}

// ---- amiibo

static NFP_LIB: AtomicI32 = AtomicI32::new(0);
static NFP_STATE: AtomicI32 = AtomicI32::new(0);
const NFP_NO_TAG: u64 = 115 | 97 << 9;
mfn!(nfp_init, |a| {
    NFP_LIB.store(1, Ordering::Relaxed);
    crate::log_once!(
        "nfp",
        "nfp::Initialize: one NFC device that never finds an amiibo"
    );
    0
});
mfn!(nfp_fini, |a| {
    NFP_LIB.store(0, Ordering::Relaxed);
    0
});
mfn!(nfp_lib_state, |a| {
    NFP_LIB.load(Ordering::Relaxed) as u64
});
mfn!(nfp_list, |a| {
    let (out, n, max) = (a.x(0), a.x(1), a.i(2));
    w32(n, 0);
    if max >= 1 {
        crate::guest::zero(out, 8);
        w32(out, 0x4e46_5030);
        w32(n, 1);
    }
    RES_OK
});
mfn!(nfp_npad, |a| {
    w32(a.x(0), crate::services::hid::connected_id());
    RES_OK
});
mfn!(nfp_start, |a| {
    NFP_STATE.store(1, Ordering::Relaxed);
    RES_OK
});
mfn!(nfp_stop, |a| {
    NFP_STATE.store(0, Ordering::Relaxed);
    RES_OK
});
mfn!(nfp_dev_state, |a| {
    NFP_STATE.load(Ordering::Relaxed) as u64
});
mfn!(nfp_no_tag, |a| { NFP_NO_TAG });
// Reading an application area: no tag, and nothing read.
mfn!(nfp_get_area, |a| {
    crate::guest::out(a.x(1), 0u64);
    NFP_NO_TAG
});
mfn!(nfp_init_ext, |a| {
    NFP_LIB.store(1, Ordering::Relaxed);
    crate::log_once!(
        "nfp",
        "nfp::InitializeWithExtendedApplicationArea: one NFC device that never finds an amiibo"
    );
    0
});

// ---- nfc: the library beside nfp, on a console whose NFC is switched off (the game shows its own message)

static NFC_STATE: AtomicI32 = AtomicI32::new(0);
mfn!(nfc_init, |a| {
    NFC_STATE.store(1, Ordering::Relaxed);
    crate::log_once!(
        "nfc",
        "nfc::Initialize: NFC is disabled (the PC has no NFC reader)"
    );
    0
});
mfn!(nfc_fini, |a| {
    NFC_STATE.store(0, Ordering::Relaxed);
    0
});
mfn!(nfc_state, |a| { NFC_STATE.load(Ordering::Relaxed) as u64 });
unsafe fn nfp_attach(c: *mut Cpu) {
    let ev = (*c).x[0];
    crate::services::os::bind_system_event(c, ev, false);
}

// ---- profanity filter

mfn!(ngc_check, |a| {
    let (masks, n) = (a.x(1), a.x(4));
    for i in 0..n.min(4096) {
        if masks != 0 {
            w32(masks + 4 * i, 0);
        }
    }
    RES_OK
});
mfn!(ngc_mask, |a| {
    crate::guest::out(a.x(1), 0u32);
    RES_OK
});
static NGC_VT: AtomicU64 = AtomicU64::new(0);
unsafe fn ngc_ctor(c: *mut Cpu) {
    let this = (*c).x[0];
    let mut vt = NGC_VT.load(Ordering::Relaxed);
    if vt == 0 {
        // a host vtable: the class's slots answer as the host's methods (nothing is profanity), any other slot 0
        let sdk = crate::hle::sym("_ZTVN2nn3ngc15ProfanityFilterE").unwrap_or(0);
        let table = nxrt::kernel::get().space.kernel_alloc(8 * 42, 16);
        let check = crate::hle::addr("ngc vtable check", ngc_check);
        let mask = crate::hle::addr("ngc vtable mask", ngc_mask);
        let none = crate::hle::addr("ngc vtable zero", zero);
        let named = [
            (
                "_ZN2nn3ngc15ProfanityFilter19CheckProfanityWordsEPjjPPKcm",
                check,
            ),
            (
                "_ZN2nn3ngc15ProfanityFilter19CheckProfanityWordsEPjjPPKtm",
                check,
            ),
            (
                "_ZN2nn3ngc15ProfanityFilter24MaskProfanityWordsInTextEPiPcj",
                mask,
            ),
            (
                "_ZN2nn3ngc15ProfanityFilter24MaskProfanityWordsInTextEPiPtj",
                mask,
            ),
        ];
        if sdk != 0 {
            w64(table, r64(sdk));
            w64(table + 8, r64(sdk + 8));
        }
        for k in 0..40u64 {
            let slot = if sdk != 0 { r64(sdk + 16 + 8 * k) } else { 0 };
            let f = named
                .iter()
                .find(|(n, _)| crate::hle::sym(n) == Some(slot))
                .map_or(none, |(_, f)| *f);
            w64(table + 16 + 8 * k, f);
        }
        vt = table + 16;
        NGC_VT.store(vt, Ordering::Relaxed);
    }
    if this != 0 {
        w64(this, vt);
    }
}

// ---- applets that show something: logged

mfn!(err_show, |a| {
    let r = a.w(0);
    crate::services::notice::error(crate::services::notice::Shown::Result(r));
    0
});
unsafe fn err_log(c: *mut Cpu) {
    let name = nxrt::hostfn::name_of((*c).pc).unwrap_or_default();
    use crate::services::notice::{error, Shown};
    if name.contains("9ShowErrorENS0_9ErrorCodeE") {
        let code = (*c).x[0];
        error(Shown::Code(code as u32, (code >> 32) as u32));
        (*c).x[0] = 0;
        return;
    }
    if name.contains("ApplicationError") {
        // nn::err::ApplicationErrorArg: its message text after the header, error number and language (+0x14)
        let arg = (*c).x[0];
        let msg = (arg != 0)
            .then(|| crate::guest::cstr(arg + 0x14, 0x800))
            .filter(|m| m.len() >= 2 && !m.chars().any(|ch| ch.is_control() && ch != '\n'));
        error(Shown::Application(msg.as_deref()));
        (*c).x[0] = 0;
        return;
    }
    let what = if false {
        String::new()
    } else if name.contains("ForUpgradeVersion") {
        "its upgrade add-on content is older than this program needs (install the add-on of the game's version)".into()
    } else if name.contains("AddOnContentVersion") {
        "add-on content version mismatch".into()
    } else if name.contains("ApplicationVersion") {
        "game version mismatch".into()
    } else if name.contains("AddOnContentLost") {
        format!("add-on content lost ({} entries)", (*c).x[1] as i32)
    } else {
        "an error (ErrorResultVariant)".into()
    };
    crate::services::notice::error(crate::services::notice::Shown::Other(&what));
    (*c).x[0] = 0;
}
mfn!(shop, |a| {
    crate::log!("ec: a shop page was asked for: the host has no shop");
    0
});

/// nn::pl::IsRunningOnOunce: the profile's platform. Super Mario Bros. Wonder 1.2.1 counts add-on content and takes
/// other Switch 2 paths only when true; its profile is the Switch 1 build (NVN 55, 19.2 MHz), as nxnative answers it.
unsafe fn running_on_ounce(c: *mut Cpu) {
    (*c).x[0] = crate::profile::get().switch2 as u64;
}

pub fn register(r: &mut Registry) {
    for (n, w, f) in [
        ("_ZN2nn2vi20GetDisplayVsyncEventEPNS_2os15SystemEventTypeEPNS0_7DisplayE", "vsync: a host thread signals it at 60 Hz", vsync_event as unsafe fn(*mut Cpu)),
        ("_ZN2nn4diag6detail7PutImplERKNS0_11LogMetaDataEPKcm", "the game's log into the host log", diag_put),
        ("_ZN2nn4diag6detail7LogImplERKNS0_11LogMetaDataEPKcz", "the game's log into the host log", diag_log),
        ("_ZN2nn4diag6detail7LogImplERKNS0_11LogMetaInfoEPKcz", "the game's log into the host log", diag_log),
        ("_ZN2nn4diag6detail8VLogImplERKNS0_11LogMetaDataEPKcSt9__va_list", "the game's log into the host log", diag_vlog),
        ("_ZN2nn4diag6detail8VLogImplERKNS0_11LogMetaInfoEPKcSt9__va_list", "the game's log into the host log", diag_vlog),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_i", "an abort: reported with its call site", diag_abort),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_iS3_z", "an abort: reported with its message", diag_abort),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_iPKNS_6ResultES3_z", "an abort: reported with its Result and message", diag_abort),
        ("_ZN2nn4diag6detail9AbortImplEPKcS3_S3_iPKNS_6ResultEPKNS_2os17UserExceptionInfoES3_z", "an abort: reported with its Result", diag_abort),
        ("_ZN2nn4diag6detail10VAbortImplEPKcS3_S3_iPKNS_6ResultEPKNS_2os17UserExceptionInfoES3_St9__va_list", "an abort: reported with its message", diag_abort),
        ("_ZN2nn4diag13GetSymbolNameEPcmm", "no symbols", diag_sym),
        ("_ZN2nn4diag12GetBacktraceEPmi", "no backtrace", zero),
        ("_ZN2nn3aoc17CountAddOnContentEv", "the add-on content installed in the data folder", aoc_count),
        ("_ZN2nn3aoc16ListAddOnContentEPiii", "the add-on content installed in the data folder", aoc_list),
        ("_ZN2nn3aoc31GetAddOnContentListChangedEventEPNS_2os11SystemEventE", "the add-on list never changes during a run", aoc_event),
        ("_ZN2nn2pl22GetSharedFontLoadStateENS0_14SharedFontTypeE", "the stand-in shared font", font_state),
        ("_ZN2nn2pl20GetSharedFontAddressENS0_14SharedFontTypeE", "the stand-in shared font", font_addr),
        ("_ZN2nn2pl17GetSharedFontSizeENS0_14SharedFontTypeE", "the stand-in shared font", font_size),
        ("_ZN2nn2pl13GetSharedFontEPiPNS0_14SharedFontDataEiNS_8settings12LanguageCodeE", "the stand-in shared font", shared_font),
        ("_ZN2nn2pl21RequestSharedFontLoadENS0_14SharedFontTypeE", "the stand-in shared font is always loaded", zero),
        ("_ZN2nn2pl16IsRunningOnOunceEv", "a Switch 2: the profile's platform (a Switch 1 build runs as on a Switch 1)", running_on_ounce),
        ("_ZN2nn3nfp10InitializeEv", "one NFC device, no tag", nfp_init),
        ("_ZN2nn3nfp8FinalizeEv", "amiibo", nfp_fini),
        ("_ZN2nn3nfp8GetStateEv", "amiibo", nfp_lib_state),
        ("_ZN2nn3nfp11ListDevicesEPNS0_12DeviceHandleEPii", "one NFC device", nfp_list),
        ("_ZN2nn3nfp9GetNpadIdEPjRKNS0_12DeviceHandleE", "the device is the connected controller's", nfp_npad),
        ("_ZN2nn3nfp14StartDetectionERKNS0_12DeviceHandleE", "searching, never finding", nfp_start),
        ("_ZN2nn3nfp13StopDetectionERKNS0_12DeviceHandleE", "amiibo", nfp_stop),
        ("_ZN2nn3nfp14GetDeviceStateERKNS0_12DeviceHandleE", "amiibo", nfp_dev_state),
        ("_ZN2nn3nfp12GetModelInfoEPNS0_9ModelInfoERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp5MountERKNS0_12DeviceHandleENS0_9ModelTypeENS0_11MountTargetE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp19AttachActivateEventEPNS_2os15SystemEventTypeERKNS0_12DeviceHandleE", "bound, never fires", nfp_attach),
        ("_ZN2nn3nfp21AttachDeactivateEventEPNS_2os15SystemEventTypeERKNS0_12DeviceHandleE", "bound, never fires", nfp_attach),
        ("_ZN2nn3ngc15ProfanityFilterC1Ev", "no word list: a host vtable", ngc_ctor),
        ("_ZN2nn3ngc15ProfanityFilter19CheckProfanityWordsEPjjPPKcm", "never profanity", ngc_check),
        ("_ZN2nn3ngc15ProfanityFilter24MaskProfanityWordsInTextEPiPcj", "nothing masked", ngc_mask),
        ("_ZN2nn3ngc15ProfanityFilter24MaskProfanityWordsInTextEPiPtj", "nothing masked", ngc_mask),
        ("_ZN2nn3err9ShowErrorENS_6ResultE", "the error applet: logged", err_show),
        ("_ZN2nn2ec24ShowShopAddOnContentListENS_13ApplicationIdERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopAddOnContentListENS_13ApplicationIdE", "no shop", shop),
        ("_ZN2nn2ec19ShowShopProductListEPKNS0_5NsUidEiPKc", "no shop", shop),
        ("_ZN2nn2ec19ShowShopProductListEPKNS0_5NsUidEiPKcRKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec22ShowShopEnterCodeSceneERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec22ShowShopEnterCodeSceneEv", "no shop", shop),
        ("_ZN2nn2ec22ShowShopProductDetailsENS0_5NsUidE", "no shop", shop),
        ("_ZN2nn2ec22ShowShopProductDetailsENS0_5NsUidERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopAddOnContentListERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopAddOnContentListEv", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListENS_13ApplicationIdE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListENS_13ApplicationIdERKNS0_8CourseIdE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListENS_13ApplicationIdERKNS0_8CourseIdERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListENS_13ApplicationIdERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListERKNS0_8CourseIdE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListERKNS0_8CourseIdERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec24ShowShopSubscriptionListEv", "no shop", shop),
        ("_ZN2nn2ec26ShowShopConsumableItemListENS_13ApplicationIdE", "no shop", shop),
        ("_ZN2nn2ec26ShowShopConsumableItemListENS_13ApplicationIdERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec26ShowShopConsumableItemListERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec26ShowShopConsumableItemListEv", "no shop", shop),
        ("_ZN2nn2ec27ShowShopSubscriptionDetailsERKNS0_8CourseIdENS0_5NsUidERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec28ShowShopConsumableItemDetailENS_13ApplicationIdERKNS0_12ConsumableIdENS0_5NsUidE", "no shop", shop),
        ("_ZN2nn2ec28ShowShopConsumableItemDetailENS_13ApplicationIdERKNS0_12ConsumableIdENS0_5NsUidERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec28ShowShopConsumableItemDetailERKNS0_12ConsumableIdENS0_5NsUidE", "no shop", shop),
        ("_ZN2nn2ec28ShowShopConsumableItemDetailERKNS0_12ConsumableIdENS0_5NsUidERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec30ShowShopApplicationInformationENS_13ApplicationIdE", "no shop", shop),
        ("_ZN2nn2ec30ShowShopApplicationInformationENS_13ApplicationIdERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec30ShowShopApplicationInformationERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn2ec30ShowShopApplicationInformationEv", "no shop", shop),
        ("_ZN2nn2ec40ShowShopApplicationInformationForUpgradeENS0_14UpgradeEditionE", "no shop", shop),
        ("_ZN2nn2ec40ShowShopApplicationInformationForUpgradeENS0_14UpgradeEditionERKNS_7account10UserHandleE", "no shop", shop),
        ("_ZN2nn4diag6detail10VAbortImplEPKcS3_S3_iPKNS_6ResultEPKNS_2os17UserExceptionInfoES3_RSt9__va_list", "an abort: reported with its message", diag_abort),
        ("_ZN2nn4diag6detail9AbortImplEPKNS_6ResultE", "an abort: reported with its Result", diag_abort_short),
        ("_ZN2nn4diag6detail9AbortImplEv", "an abort: reported with its call site", diag_abort_short),
        ("_ZN2nn2pl18GetBuiltInFontDataEPNS0_17BuiltInFontDataNxENS0_17BuiltInFontTypeNxEb", "the stand-in shared font", builtin_font_fn),
        ("_ZN2nn2pl18GetBuiltInFontDataEPNS0_20BuiltInFontDataOunceENS0_20BuiltInFontTypeOunceEb", "the stand-in shared font", builtin_font_fn),
        ("_ZN2nn2pl21GetBuiltInFontDataSetEPiPNS0_17BuiltInFontDataNxEPKNS0_17BuiltInFontTypeNxEiNS_8settings12LanguageCodeEb", "the stand-in shared font", builtin_font_set),
        ("_ZN2nn2pl21GetBuiltInFontDataSetEPiPNS0_20BuiltInFontDataOunceEPKNS0_20BuiltInFontTypeOunceEiNS_8settings12LanguageCodeEb", "the stand-in shared font", builtin_font_set),
        ("_ZN2nn6fontll18ScalableFontEngine8LoadFontEPcPKvjj", "a stand-in font at its BFTTF header: the TTF behind it", font_load),
        ("_ZN2nn3nfc10InitializeEv", "NFC switched off (no reader)", nfc_init),
        ("_ZN2nn3nfc8FinalizeEv", "NFC switched off (no reader)", nfc_fini),
        ("_ZN2nn3nfc8GetStateEv", "NFC library state", nfc_state),
        ("_ZN2nn3nfp37InitializeWithExtendedApplicationAreaEv", "one NFC device, no tag", nfp_init_ext),
        ("_ZN2nn3nfp18GetApplicationAreaEPvPmRKNS0_12DeviceHandleEm", "no tag", nfp_get_area),
        ("_ZN2nn3ngc15ProfanityFilter19CheckProfanityWordsEPjjPPKDsm", "never profanity", ngc_check),
        ("_ZN2nn3ngc15ProfanityFilter24MaskProfanityWordsInTextEPiPDsj", "nothing masked", ngc_mask),
        ("_ZN2nn3err20ShowApplicationErrorERKNS0_19ApplicationErrorArgE", "the error applet: logged", err_log),
        ("_ZN2nn3err39ShowUnacceptableApplicationVersionErrorEv", "the error applet: logged", err_log),
        ("_ZN2nn3err40ShowUnacceptableAddOnContentVersionErrorEv", "the error applet: logged", err_log),
        ("_ZN2nn3err50ShowUnacceptableAddOnContentForUpgradeVersionErrorEv", "the error applet: logged", err_log),
        ("_ZN2nn3err9ShowErrorENS0_9ErrorCodeE", "the error applet: logged", err_log),
        ("_ZN2nn3err9ShowErrorERKNS0_18ErrorResultVariantE", "the error applet: logged", err_log),
        ("_ZN2nn3aoc25ShowAddOnContentLostErrorEPKii", "the error applet: logged", err_log),
        ("_ZN2nn3nfp10GetTagInfoEPNS0_7TagInfoERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp13GetCommonInfoEPNS0_10CommonInfoERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp13StartRestorerEPNS0_12DeviceHandleERKNS0_24AmiiboSettingsStartParamERKNS0_7TagInfoE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp15GetRegisterInfoEPNS0_12RegisterInfoERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp18SetApplicationAreaERKNS0_12DeviceHandleEPKvm", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp19OpenApplicationAreaERKNS0_12DeviceHandleEj", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp19StartGameDataEraserEPNS0_12DeviceHandleERKNS0_24AmiiboSettingsStartParamERKNS0_7TagInfoE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp21CreateApplicationAreaERKNS0_12DeviceHandleERKNS0_25ApplicationAreaCreateInfoE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp23RecreateApplicationAreaERKNS0_12DeviceHandleERKNS0_25ApplicationAreaCreateInfoE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp26GetExtendedApplicationAreaEPvPmRKNS0_12DeviceHandleEm", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp26SetExtendedApplicationAreaERKNS0_12DeviceHandleEPKvm", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp29StartNicknameAndOwnerSettingsEPNS0_12DeviceHandleEPbPNS0_12RegisterInfoERKNS0_24AmiiboSettingsStartParamERKNS0_7TagInfoE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp29StartNicknameAndOwnerSettingsEPNS0_12DeviceHandleEPbPNS0_12RegisterInfoERKNS0_24AmiiboSettingsStartParamERKNS0_7TagInfoERKS4_", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp30GetExtendedApplicationAreaInfoEPNS0_27ExtendedApplicationAreaInfoERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp5FlushERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp5MountERKNS0_12DeviceHandleENS0_9ModelTypeE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp7RestoreERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
        ("_ZN2nn3nfp7UnmountERKNS0_12DeviceHandleE", "no tag", nfp_no_tag),
    ] {
        r.force(n, w, f);
    }
}

#[cfg(test)]
mod tests {
    /// A BFTTF the player put in `fonts\` decrypts to its TTF (the key from the first word).
    #[test]
    fn bfttf_decrypts() {
        let ttf: Vec<u8> = (0..37u8).collect();
        let k2 = 0x1234_5678u32;
        let mut f = vec![];
        f.extend_from_slice(&(k2 ^ 0x7f9a_0218).to_le_bytes());
        f.extend_from_slice(&((ttf.len() as u32).swap_bytes() ^ k2).to_le_bytes());
        let mut padded = ttf.clone();
        padded.resize(40, 0);
        for c in padded.chunks(4) {
            let x = u32::from_le_bytes(c.try_into().unwrap());
            f.extend_from_slice(&(x.swap_bytes() ^ k2).to_le_bytes());
        }
        assert_eq!(super::bfttf_decrypt(&f).unwrap(), ttf);
        assert_eq!(super::FONT_NAMES.len(), 6);
    }

    #[test]
    fn printf() {
        let s = b"str\0";
        let mut ints = [42u64, (-7i64) as u64, 0xbeef, s.as_ptr() as u64].into_iter();
        let mut fl = [1.5f64].into_iter();
        assert_eq!(
            super::format("a %d b %ld c %x %s %.2f %%", &mut ints, &mut fl),
            "a 42 b -7 c beef str 1.50 %"
        );
        let mut ints = [5u64].into_iter();
        assert_eq!(
            super::format("[%4d]", &mut ints, &mut [].into_iter()),
            "[   5]"
        );
    }
}
