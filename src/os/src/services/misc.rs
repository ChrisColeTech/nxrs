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
                    next += period;
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
pub fn format(fmt: &str, ints: &mut dyn Iterator<Item = u64>, floats: &mut dyn Iterator<Item = f64>) -> String {
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
        let width: usize = spec.trim_start_matches(['-', '+', ' ', '#', '0']).split('.').next().unwrap_or("").parse().unwrap_or(0);
        let long = spec.contains('l') || spec.contains('z') || spec.contains('j') || spec.contains('t') || spec.contains('q');
        let s = match conv {
            b'd' | b'i' => {
                let v = ints.next().unwrap_or(0);
                if long { (v as i64).to_string() } else { (v as i32).to_string() }
            }
            b'u' => {
                let v = ints.next().unwrap_or(0);
                if long { v.to_string() } else { (v as u32).to_string() }
            }
            b'x' | b'X' => {
                let v = ints.next().unwrap_or(0);
                let v = if long { v } else { v as u32 as u64 };
                if conv == b'x' { format!("{v:x}") } else { format!("{v:X}") }
            }
            b'p' => format!("{:#x}", ints.next().unwrap_or(0)),
            b'c' => ((ints.next().unwrap_or(0) as u8) as char).to_string(),
            b's' => {
                let p = ints.next().unwrap_or(0);
                if p == 0 { "(null)".into() } else { cstr(p, 1024) }
            }
            b'f' | b'F' | b'e' | b'g' | b'G' => {
                let prec: usize = spec.split('.').nth(1).and_then(|p| p.parse().ok()).unwrap_or(6);
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
            ints.push(if nxrt::mem::readable(sp, 8) { r64(sp) } else { 0 });
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
    let (cond, func, file, line) = (cstr(cpu.x[0], 256), cstr(cpu.x[1], 256), cstr(cpu.x[2], 256), cpu.x[3] as i32);
    let msg = if name.contains("10VAbortImpl") {
        let (i, f) = va_streams(cpu.arg_x(7));
        format(&cstr(cpu.x[6], 1024), &mut i.into_iter(), &mut f.into_iter())
    } else if name.contains("PKNS_6ResultES3_z") {
        let fmt = cstr(cpu.x[5], 1024);
        let mut ints = (6..32).map(|i| cpu.arg_x(i));
        format(&fmt, &mut ints, &mut (0..8).map(|i| f64::from_bits(cpu.v[i] as u64)))
    } else if name.contains("iS3_z") {
        let fmt = cstr(cpu.x[4], 1024);
        let mut ints = (5..32).map(|i| cpu.arg_x(i));
        format(&fmt, &mut ints, &mut (0..8).map(|i| f64::from_bits(cpu.v[i] as u64)))
    } else {
        String::new()
    };
    let res = if name.contains("PKNS_6ResultE") && cpu.x[4] != 0 { r32(cpu.x[4]) } else { 0 };
    crate::log!("[RED] the game aborted (nn::diag): {cond} in {func} ({file}:{line}) Result {res:#x}: {msg}");
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

mfn!(aoc_count, |a| { crate::services::fs::aoc_indices().len() as u64 });
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
    crate::log_once!("aoc list", "aoc: {} add-on content installed: {:?}", v.len(), v);
    w
});
unsafe fn aoc_event(c: *mut Cpu) {
    let ev = (*c).x[0];
    crate::services::os::bind_system_event(c, ev, false);
}

// ---- shared fonts

struct Font {
    addr: u64,
    size: u64,
}

fn font() -> Option<&'static Font> {
    static F: OnceLock<Option<Font>> = OnceLock::new();
    F.get_or_init(|| {
        let dir = std::env::var("NXOS_FONT").map(std::path::PathBuf::from).unwrap_or_else(|_| {
            let w = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
            std::path::Path::new(&w).join("Fonts").join("arial.ttf")
        });
        let ttf = std::fs::read(&dir).ok()?;
        let n = ttf.len() as u64;
        let a = nxrt::kernel::get().space.kernel_alloc(n + 8, 0x1000);
        // the BFTTF header: a "decrypted" marker and the length, XOR-keyed and big-endian
        w32(a, 0x1802_9a7f);
        w32(a + 4, (n as u32 ^ 0x4962_1806).swap_bytes());
        crate::guest::write_bytes(a + 8, &ttf);
        crate::log!("pl: the shared fonts are the stand-in {} ({n} bytes): the console's are firmware content", dir.display());
        Some(Font { addr: a, size: n + 8 })
    })
    .as_ref()
}

mfn!(font_state, |a| { font().is_some() as u64 });
mfn!(font_addr, |a| { font().map_or(0, |f| f.addr + 8) });
mfn!(font_size, |a| { font().map_or(0, |f| f.size - 8) });
mfn!(shared_font, |a| {
    // GetSharedFont(int *count, SharedFontData *out, int max, LanguageCode): {u32 type, pad, address, size} per type
    let (count, out, max) = (a.x(0), a.x(1), a.i(2));
    let Some(f) = font() else {
        w32(count, 0);
        return 0;
    };
    let mut n = 0;
    for t in 0..6u64 {
        if (n as i32) < max {
            let e = out + 0x18 * n;
            w32(e, t as u32);
            w32(e + 4, 0);
            w64(e + 8, f.addr + 8);
            w64(e + 16, f.size - 8);
        }
        n += 1;
    }
    w32(count, n as u32);
    1
});

// ---- amiibo

static NFP_LIB: AtomicI32 = AtomicI32::new(0);
static NFP_STATE: AtomicI32 = AtomicI32::new(0);
const NFP_NO_TAG: u64 = 115 | 97 << 9;
mfn!(nfp_init, |a| {
    NFP_LIB.store(1, Ordering::Relaxed);
    crate::log_once!("nfp", "nfp::Initialize: one NFC device that never finds an amiibo");
    0
});
mfn!(nfp_fini, |a| {
    NFP_LIB.store(0, Ordering::Relaxed);
    0
});
mfn!(nfp_lib_state, |a| { NFP_LIB.load(Ordering::Relaxed) as u64 });
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
mfn!(nfp_dev_state, |a| { NFP_STATE.load(Ordering::Relaxed) as u64 });
mfn!(nfp_no_tag, |a| { NFP_NO_TAG });
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
            ("_ZN2nn3ngc15ProfanityFilter19CheckProfanityWordsEPjjPPKcm", check),
            ("_ZN2nn3ngc15ProfanityFilter19CheckProfanityWordsEPjjPPKtm", check),
            ("_ZN2nn3ngc15ProfanityFilter24MaskProfanityWordsInTextEPiPcj", mask),
            ("_ZN2nn3ngc15ProfanityFilter24MaskProfanityWordsInTextEPiPtj", mask),
        ];
        if sdk != 0 {
            w64(table, r64(sdk));
            w64(table + 8, r64(sdk + 8));
        }
        for k in 0..40u64 {
            let slot = if sdk != 0 { r64(sdk + 16 + 8 * k) } else { 0 };
            let f = named.iter().find(|(n, _)| crate::hle::sym(n) == Some(slot)).map_or(none, |(_, f)| *f);
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
    crate::log!("[RED] err::ShowError Result {r:#x} (module {}, description {}): the host shows no error applet", r & 0x1ff, (r >> 9) & 0x1fff);
    0
});
mfn!(shop, |a| {
    crate::log!("ec: a shop page was asked for: the host has no shop");
    0
});

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
    ] {
        r.force(n, w, f);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn printf() {
        let s = b"str\0";
        let mut ints = [42u64, (-7i64) as u64, 0xbeef, s.as_ptr() as u64].into_iter();
        let mut fl = [1.5f64].into_iter();
        assert_eq!(super::format("a %d b %ld c %x %s %.2f %%", &mut ints, &mut fl), "a 42 b -7 c beef str 1.50 %");
        let mut ints = [5u64].into_iter();
        assert_eq!(super::format("[%4d]", &mut ints, &mut [].into_iter()), "[   5]");
    }
}
