//! Mario Kart World 1.8.0 on an output that is not 16:9 (mkw-recomp `runtime/core/boot/patches.c`, MIT: the
//! `aspect`, `vertplus` and `display` sets, off by default). `NXRS_MKW_ASPECT=21:9` (or `32:9`, `48:9`, any `w:h`)
//! with `NXRS_PATCH_MKW_ASPECT=1` widens each camera that is 16:9 (Hor+) and fits the 2D layouts in the middle 16:9
//! without stretching; a taller shape (with `NXRS_PATCH_MKW_VERTPLUS=1`) keeps the 16:9 horizontal field of view and
//! shows more vertically. `NXRS_PATCH_MKW_DISPLAY=1` gives the display size (what the picture is cropped to) the
//! output's shape; the output itself is the `output` set's (`NXRS_PATCH_MKW_OUTPUT=1`, `NXRS_MKW_RESOLUTION`).
//!
//! | Patch page | Site | Host function |
//! |---|---|---|
//! | 0xf80 | fdiv s0, s0, s1 at main+0x8a6d4 / 0x8a784 (the camera's w / h) | `aspect_host` |
//! | 0xf70 | bl DrawInfo's projection getter at main+0x17f3c20 / 0x1805f1c | `ui_proj` |
//! | 0xf40 | ldp q0, q4, [x21, #0x10] at main+0x17f5084 | `ui_rows2` |
//! | 0xf30 | fmul s8, s0, s14 at main+0x894a4 (race FOV) | `camera_fov` |
//! | 0xf20 | bl memcpy of nn::vfx2's view at main+0x139b60 | `fx_view` |
//! | 0xf10 | str x12, [x9, #0x10] at main+0x3723b4 (display size) | `display_size` |
//! | 0xf00 | bl main+0xde6060 at main+0x372840 / 0xde5b58 | `display_set` |
//! | 0xdf0 | bl main+0x17f89f0 (pane global matrix) at 3 sites | `pane_matrix` |

use super::mkw::{main_addr, output_wh, ticks60};
use super::{env_str, set_on};
use crate::guest;
use nxrt::cpu::Cpu;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

const ASPECT_HOST: u64 = 0xf80;
const UI_PROJ_HOST: u64 = 0xf70;
const UI_ROWS2_HOST: u64 = 0xf40;
const CAMERA_FOV_HOST: u64 = 0xf30;
const FX_VIEW_HOST: u64 = 0xf20;
const DISPLAY_SIZE_HOST: u64 = 0xf10;
const DISPLAY_SET_HOST: u64 = 0xf00;
const PANE_MATRIX_HOST: u64 = 0xdf0;

// version.c 1.8.0
const UI_TEXT_LR: u64 = 0x1805f20;
const FX_GROUP_LR: u64 = 0x137f4c;
const FX_GROUP_FRAME: u32 = 1;
const FX_GROUP_WORD: u64 = 6;
const FX_SCREEN_GROUP: u64 = 0x80;
const ASPECT_FIRST_LR: u64 = 0x8a6d8;
const FOV_ENTRY_X: usize = 20;
const FOV_HALF_V: usize = 14;
const DISPLAY_SET_FN: u64 = 0xde6060;
const DISPLAY_VALUE_REG: usize = 12;
const DISPLAY_BASE_REG: usize = 9;
const PANE_MATRIX_FN: u64 = 0x17f89f0;

const R169: f32 = 16.0 / 9.0;

/// The output's width / height (0: 16:9, the sets off) and that over 16:9.
static ASPECT_OUT: AtomicU32 = AtomicU32::new(0);
static ASPECT_SCALE: AtomicU32 = AtomicU32::new(0);
/// 1 + the 60 Hz tick of the latest view of a split screen
static SPLIT_AT: AtomicU64 = AtomicU64::new(0);

fn f(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

/// (aspect_out, aspect_scale): the output's shape when the aspect set widens it, else (0, 0).
pub(super) fn aspect() -> (f32, f32) {
    (f(&ASPECT_OUT), f(&ASPECT_SCALE))
}

fn aspect_of() -> f32 {
    let a = env_str("NXRS_MKW_ASPECT");
    if a.is_empty() || a == "16:9" {
        return 0.0;
    }
    if a == "window" {
        crate::log!("[RED] gamefix: NXRS_MKW_ASPECT=window is not supported here (give the shape, e.g. 21:9); keeping 16:9");
        return 0.0;
    }
    let mut it = a.split(':').map(|v| v.trim().parse::<i32>().unwrap_or(0));
    let (w, h) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    let r = if w > 0 && h > 0 { w as f32 / h as f32 } else { 0.0 };
    if r > 0.0 && r < R169 {
        return r; // taller: the vertplus set (1.8.0 has its FOV site)
    }
    if r > R169 + 0.01 { r } else { 0.0 }
}

pub(super) fn install(page: u64) {
    let al = |off: u64, name: &str, f: nxrt::hostfn::HostFn| nxrt::hostfn::alias(page + off, name, f);
    al(ASPECT_HOST, "gamefix:mkw_aspect", aspect_host);
    al(UI_PROJ_HOST, "gamefix:mkw_ui_proj", ui_proj);
    al(UI_ROWS2_HOST, "gamefix:mkw_ui_rows2", ui_rows2);
    al(CAMERA_FOV_HOST, "gamefix:mkw_camera_fov", camera_fov);
    al(FX_VIEW_HOST, "gamefix:mkw_fx_view", fx_view);
    al(DISPLAY_SIZE_HOST, "gamefix:mkw_display_size", display_size);
    al(DISPLAY_SET_HOST, "gamefix:mkw_display_set", display_set);
    al(PANE_MATRIX_HOST, "gamefix:mkw_pane_matrix", pane_matrix);
    let on = set_on("NXRS_PATCH_MKW_ASPECT", false);
    let mut out = aspect_of();
    if out > 0.0 && out < R169 && !set_on("NXRS_PATCH_MKW_VERTPLUS", false) {
        crate::log!("[RED] gamefix: a taller NXRS_MKW_ASPECT needs the vertplus set (NXRS_PATCH_MKW_VERTPLUS=1); keeping 16:9");
        out = 0.0;
    }
    if out > 0.0 && !on {
        crate::log!("[RED] gamefix: NXRS_MKW_ASPECT={} needs the aspect set (NXRS_PATCH_MKW_ASPECT=1, with NXRS_PATCH_MKW_DISPLAY=1 and the output set); keeping 16:9", env_str("NXRS_MKW_ASPECT"));
        out = 0.0;
    }
    if on && out == 0.0 {
        crate::log!("[RED] gamefix: the aspect set is on with a 16:9 NXRS_MKW_ASPECT: its sites keep the game's values");
    }
    ASPECT_OUT.store(out.to_bits(), Ordering::Relaxed);
    ASPECT_SCALE.store((out / R169).to_bits(), Ordering::Relaxed);
    if out > 0.0 {
        crate::log!("gamefix: camera aspect {out:.3}, x{:.3} over 16:9 (NXRS_MKW_ASPECT {})", out / R169, env_str("NXRS_MKW_ASPECT"));
    }
}

fn s_of(v: u128) -> f32 {
    f32::from_bits(v as u32)
}

/// A 16:9 view, or an 8:9 one doubled (half of a two-player split), within 0.008, is widened; other views keep
/// their aspect. A 16:9 view narrower than the full one (1280x720) is a quarter of a split screen.
fn aspect_widen(w: f32, h: f32) -> f32 {
    let a = w / h;
    let r = if a < 1.2 { 2.0 * a } else { a };
    let d = r - R169;
    let on = d < 0.008 && d > -0.008;
    if on && (a < 1.2 || w < 1000.0) {
        SPLIT_AT.store(ticks60() + 1, Ordering::Relaxed);
    }
    static SEEN: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.len() < 16 && !seen.contains(&(w.to_bits(), h.to_bits())) {
        seen.push((w.to_bits(), h.to_bits()));
        crate::log!("gamefix: camera view {w:.0}x{h:.0} ({a:.3}): {}", if on { "fitted" } else { "kept" });
    }
    if on { f(&ASPECT_SCALE) } else { 1.0 }
}

/// In place of fdiv s0, s0, s1: the camera's aspect, fitted. The first projection's (return main+0x8a6d8) keeps its
/// 16:9 calibration on a taller output (the race camera's FOV is corrected at main+0x894a4).
unsafe fn aspect_host(c: *mut Cpu) {
    let (w, h) = (s_of((*c).v[0]), s_of((*c).v[1]));
    let mut scale = if f(&ASPECT_OUT) > 0.0 { aspect_widen(w, h) } else { 1.0 };
    if scale < 1.0 && (*c).x[30] == main_addr(ASPECT_FIRST_LR) {
        scale = 1.0;
    }
    (*c).v[0] = (w / h * scale).to_bits() as u128;
}

fn aspect_fov_angle(angle: f32, scale: f32) -> f32 {
    2.0 * ((angle * 0.5).tan() / scale).atan()
}

/// In place of fmul s8, s0, s14 at main+0x894a4: a taller output keeps the 16:9 horizontal FOV (the requested angle,
/// before the game writes it and its caches), then the replaced multiply (upper lanes cleared).
unsafe fn camera_fov(c: *mut Cpu) {
    let mut angle = s_of((*c).v[0]);
    let half = s_of((*c).v[FOV_HALF_V]);
    let scale = f(&ASPECT_SCALE);
    let a = guest::rd::<f32>((*c).x[FOV_ENTRY_X] + 0xb0);
    if scale > 0.0 && scale < 1.0 && (a - R169 * scale).abs() < 0.008 {
        angle = aspect_fov_angle(angle, scale);
        (*c).v[0] = angle.to_bits() as u128;
    }
    (*c).v[8] = (half * angle).to_bits() as u128;
}

/// A 16:9 display size fitted to the output's aspect: on a taller output the same width and the rows of its aspect,
/// on a wider one the same rows and the columns of its aspect (both even), shrunk to fit the output's textures.
fn display_fit(wh: u64) -> u64 {
    let (w, h) = (wh as u32, (wh >> 32) as u32);
    let (out, scale) = aspect();
    if !(scale > 0.0) || scale == 1.0 || w == 0 || h == 0 || (w as f32 / h as f32 - R169).abs() > 0.01 {
        return wh;
    }
    let (mut fw, mut fh) = (w as f32, h as f32);
    if scale > 1.0 {
        fw = fh * out;
    } else {
        fh = fw / out;
    }
    let (ow, oh) = output_wh();
    let fit = if ow != 0 && oh != 0 { 1.0f32.min((ow as f32 / fw).min(oh as f32 / fh)) } else { 1.0 };
    let nw = ((fw * fit + 0.5) as u32 + 1) & !1;
    let nh = ((fh * fit + 0.5) as u32 + 1) & !1;
    nw as u64 | (nh as u64) << 32
}

fn display_note(wh: u64, fitted: u64) {
    static LOGGED: AtomicI32 = AtomicI32::new(0);
    if fitted != wh && LOGGED.fetch_add(1, Ordering::Relaxed) == 0 {
        let (ow, oh) = output_wh();
        crate::log!("gamefix: display {}x{} made {}x{} (NXRS_MKW_ASPECT, output {ow}x{oh})", wh as u32, wh >> 32, fitted as u32, fitted >> 32);
    }
}

/// In place of the store of the display's requested size in its per-mode update: the same store, of the fitted size.
unsafe fn display_size(c: *mut Cpu) {
    let wh = (*c).x[DISPLAY_VALUE_REG];
    let fitted = display_fit(wh);
    guest::w64((*c).x[DISPLAY_BASE_REG] + 0x10, fitted);
    display_note(wh, fitted);
}

/// In place of the calls to the display size setter (display, const size *, slot): the setter, then its slot
/// rewritten with the fitted size.
unsafe fn display_set(c: *mut Cpu) {
    let (display, size, slot) = ((*c).x[0], (*c).x[1], (*c).x[2] as i32 as i64 as u64);
    guest::call(c, main_addr(DISPLAY_SET_FN), &[display, size, slot]);
    let at = display + 0x10 + 8 * slot;
    let wh = guest::r64(at);
    let fitted = display_fit(wh);
    guest::w64(at, fitted);
    display_note(wh, fitted);
}

fn mat(a: u64) -> [f32; 16] {
    let mut m = [0f32; 16];
    for (i, v) in m.iter_mut().enumerate() {
        *v = guest::rd(a + 4 * i as u64);
    }
    m
}

fn put_mat(a: u64, m: &[f32; 16]) {
    for (i, v) in m.iter().enumerate() {
        guest::wr(a + 4 * i as u64, *v);
    }
}

/// x and y unskewed and an aspect m[5] / m[0] of 16:9 (row-major, x row first)
fn is_16x9(m: &[f32; 16]) -> bool {
    if m[1] != 0.0 || m[2] != 0.0 || m[4] != 0.0 || m[6] != 0.0 || m[8] != 0.0 || m[9] != 0.0 || m[12] != 0.0 || m[13] != 0.0 {
        return false;
    }
    let a = if m[0] != 0.0 { m[5] / m[0] } else { 0.0 };
    let d = a.abs() - R169;
    d < 0.008 && d > -0.008
}

/// A complete 16:9 layout fitted without losing either axis: a narrow output spends the extra height.
fn aspect_ui_fit(m: &mut [f32; 16], scale: f32) {
    if scale < 1.0 {
        m[5] *= scale;
        m[7] *= scale;
    } else {
        m[0] /= scale;
        m[3] /= scale;
    }
}

/// Whether the pane whose material is set up covers the canvas (1920x1080 layout units); tx: its x. -1: x22 / x23
/// are not such a pane. The text setup (returning to main+0x1805f20) holds the pane in x21.
fn ui_full_screen(c: &Cpu, tx: &mut f32) -> i32 {
    let text = c.x[30] == main_addr(UI_TEXT_LR);
    let mtx = if text { c.x[21] + 0x70 } else { c.x[23] };
    let size = if text { c.x[21] + 0x50 } else { c.x[22] };
    *tx = 0.0;
    if size != mtx.wrapping_sub(0x20) || mtx & 3 != 0 || mtx < 0x70 || !guest::readable(mtx - 0x70, 0xa0) {
        return -1;
    }
    let g: [f32; 4] = [guest::rd(mtx), 0.0, 0.0, guest::rd(mtx + 12)];
    let g5: f32 = guest::rd(mtx + 20);
    let (sz0, sz1): (f32, f32) = (guest::rd(size), guest::rd(size + 4));
    let (w, h) = ((g[0] * sz0).abs(), (g5 * sz1).abs());
    *tx = g[3];
    (w > 1920.0 * 0.98 && h > 1080.0 * 0.98) as i32
}

fn name_at(a: u64, n: usize) -> Vec<u8> {
    guest::bytes(a, n as u64).to_vec()
}

fn name_is(a: u64, s: &str, n: usize) -> bool {
    let b = name_at(a, n);
    let mut want = s.as_bytes().to_vec();
    want.resize(n, 0);
    // strncmp(name, s, n): equal up to the first NUL of either
    for i in 0..n {
        if b[i] != want[i] {
            return false;
        }
        if b[i] == 0 {
            return true;
        }
    }
    true
}

/// A taller output: the character and kart lists' 100-unit capture masks and the cup select's 1080-square backdrop
/// cover the added height (drawn wide again).
fn ui_tall_backdrop(c: &Cpu) -> bool {
    let scale = f(&ASPECT_SCALE);
    if !(scale > 0.0 && scale < 1.0) {
        return false;
    }
    let mtx = c.x[23];
    if mtx < 0x70 || mtx & 3 != 0 || c.x[22] != mtx - 0x20 || !guest::readable(mtx - 0x70, 0xe0) {
        return false;
    }
    let (s0, s1): (f32, f32) = (guest::rd(c.x[22]), guest::rd(c.x[22] + 4));
    let name = mtx + 0x40;
    if (s0 - 1080.0).abs() < 0.01 && (s1 - 1080.0).abs() < 0.01 && name_at(name, 8) == b"P_BG_00\0" {
        return true;
    }
    (s0 - 100.0).abs() < 0.01 && (900.0..=1080.0).contains(&s1) && (name_at(name, 9) == b"P_Cap_00\0" || name_at(name, 9) == b"P_Cap_01\0")
}

const UI_MASK_REST: f32 = 900.0;

fn pane_name(pane: u64) -> u64 {
    pane + 0xb0
}

fn pane_parent(pane: u64) -> u64 {
    guest::r64(pane + 0x18)
}

/// -1: the outer list mask, 1: the inner one, 0: another pane (P_Cap_0x < N_Root_02 < the RootPane)
fn ui_list_mask(pane: u64) -> i32 {
    let n = name_at(pane_name(pane), 9);
    if &n[..7] != b"P_Cap_0" || (n[7] != b'0' && n[7] != b'1') || n[8] != 0 {
        return 0;
    }
    let parent = pane_parent(pane);
    if parent == 0 || !name_is(pane_name(parent), "N_Root_02", 24) || pane_parent(parent) == 0 || pane_parent(pane_parent(parent)) != 0 {
        return 0;
    }
    if n[7] == b'0' { -1 } else { 1 }
}

/// The masks placed by pane_matrix (add) and whether a drawn pane is one (query), by address and name.
fn ui_mask_mark(pane: u64, add: bool) -> bool {
    static SET: Mutex<(Vec<(u64, Vec<u8>)>, usize)> = Mutex::new((Vec::new(), 0));
    let name = name_at(pane_name(pane), 24);
    let mut g = SET.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if g.0.iter().any(|(p, n)| *p == pane && *n == name) {
        return true;
    }
    if !add {
        return false;
    }
    let at = g.1 % 8;
    g.1 += 1;
    if g.0.len() < 8 {
        g.0.push((pane, name));
    } else {
        g.0[at] = (pane, name);
    }
    true
}

fn ui_mask_place(side: i32, g: &mut [f32; 12], w: f32, scale: f32) {
    let hw = g[0] * w / 2.0;
    let (mut l, r) = (g[3] - hw, g[3] + hw);
    if side < 0 && l <= -UI_MASK_REST {
        l = -960.0 * scale;
    }
    if w > 0.0 {
        g[0] = (r - l) / w;
    }
    g[3] = (l + r) / 2.0;
    g[0] /= scale;
    g[1] /= scale;
    g[2] /= scale;
    g[3] /= scale;
}

/// The race and pause prompts (KeyAssign_00): L_Key_02 under N_LeftKey_00 (bottom left), A_Align_00 under N_Key_00
/// (bottom right): kept at the display's corners with their 16:9 shape.
fn ui_prompt(pane: u64) -> Option<i32> {
    let parent = pane_parent(pane);
    if parent == 0 {
        return None;
    }
    let (n, up) = (pane_name(pane), pane_name(parent));
    if name_is(n, "L_Key_02", 24) && name_is(up, "N_LeftKey_00", 24) {
        return Some(-1);
    }
    if name_is(n, "A_Align_00", 24) && name_is(up, "N_Key_00", 24) {
        return Some(1);
    }
    None
}

fn ui_prompt_place(g: &mut [f32; 12], ax: i32, scale: f32) {
    if scale > 1.0 {
        g[0] /= scale;
        g[1] /= scale;
        g[2] /= scale;
        g[3] = (g[3] + ax as f32 * 960.0 * (scale - 1.0)) / scale;
    } else {
        g[4] *= scale;
        g[5] *= scale;
        g[6] *= scale;
        g[7] = (g[7] - 540.0 * (1.0 / scale - 1.0)) * scale;
    }
}

/// In place of the calls of nn::ui2d's pane global-matrix builder (pane, draw info): the builder, then a list mask or
/// a prompt placed for an output that is not 16:9 (a split screen keeps its place).
unsafe fn pane_matrix(c: *mut Cpu) {
    let (pane, info) = ((*c).x[0], (*c).x[1]);
    guest::call(c, main_addr(PANE_MATRIX_FN), &[pane, info]);
    let scale = f(&ASPECT_SCALE);
    if !(scale > 0.0) || scale == 1.0 {
        return;
    }
    let at = SPLIT_AT.load(Ordering::Relaxed);
    if at != 0 && ticks60() + 1 - at < 30 {
        return;
    }
    let ga = pane + 0x70;
    let mut g = [0f32; 12];
    for (i, v) in g.iter_mut().enumerate() {
        *v = guest::rd(ga + 4 * i as u64);
    }
    let side = if scale > 1.0 { ui_list_mask(pane) } else { 0 };
    if side != 0 {
        ui_mask_place(side, &mut g, guest::rd::<f32>(pane + 0x50), scale);
        ui_mask_mark(pane, true);
    } else if let Some(ax) = ui_prompt(pane) {
        ui_prompt_place(&mut g, ax, scale);
    } else {
        return;
    }
    for (i, v) in g.iter().enumerate() {
        guest::wr(ga + 4 * i as u64, *v);
    }
}

/// In place of the bl to DrawInfo's projection getter (the 4x4 at [x0+0x10] to [x1]) from the two draw
/// constant-buffer setups of the 2D layouts: the same copy, a 16:9 projection narrowed to the middle 16:9 (also in
/// DrawInfo, for its other readers); a pane over the whole canvas (fades, backdrops) drawn wide again, except a
/// movie's; a split screen's HUD moved to the middle of its half.
unsafe fn ui_proj(c: *mut Cpu) {
    let s = (*c).x[0] + 0x10;
    let dst = (*c).x[1];
    let mut m = mat(s);
    let scale = f(&ASPECT_SCALE);
    if is_16x9(&m) && scale > 0.0 {
        aspect_ui_fit(&mut m, scale);
        put_mat(s, &m);
    }
    let mut tx = 0.0;
    let full = if f(&ASPECT_OUT) > 0.0 { ui_full_screen(&*c, &mut tx) } else { -1 };
    if (full == 1 || ui_tall_backdrop(&*c)) && !crate::services::movie::playing() {
        if scale < 1.0 {
            m[5] /= scale;
            m[7] /= scale;
        } else {
            m[0] *= scale;
            m[3] *= scale;
        }
    } else if full == 0
        && scale > 1.0
        && (*c).x[22] == (*c).x[23].wrapping_sub(0x20)
        && guest::readable((*c).x[23].wrapping_sub(0x70), 0xe0)
        && ui_mask_mark((*c).x[23] - 0x70, false)
    {
        m[0] *= scale;
        m[3] *= scale;
    }
    let at = SPLIT_AT.load(Ordering::Relaxed);
    let moved = scale > 1.0 && full == 0 && at != 0 && ticks60() + 1 - at < 30 && (tx > 1.0 || tx < -1.0);
    if moved {
        let d = (if tx > 0.0 { 0.5 } else { -0.5 }) * (1.0 - 1.0 / scale);
        for i in 0..4 {
            m[i] += d * m[12 + i];
        }
    }
    put_mat(dst, &m);
}

/// In place of ldp q0, q4, [x21, #0x10] at main+0x17f5084: a pane drawn through a view-projection product made on
/// the CPU reads DrawInfo's projection itself; the same two rows, the x row narrowed.
unsafe fn ui_rows2(c: *mut Cpu) {
    let mut m = mat((*c).x[21] + 0x10);
    let scale = f(&ASPECT_SCALE);
    if is_16x9(&m) && scale > 0.0 {
        aspect_ui_fit(&mut m, scale);
    }
    let row = |k: usize| -> u128 {
        let mut b = [0u8; 16];
        for j in 0..4 {
            b[4 * j..4 * j + 4].copy_from_slice(&m[4 * k + j].to_le_bytes());
        }
        u128::from_le_bytes(b)
    };
    (*c).v[0] = row(0);
    (*c).v[4] = row(1);
}

/// In place of nn::vfx2's view copy to the GPU (bl memcpy at main+0x139b60): the copy, and for the screen effects
/// (draw group 7, as the mask 0x80 the group draw keeps) the GPU copy gets 16:9's x scale: the projection's and the
/// view-projection's x rows times the widening, the x columns of their inverses divided by it.
unsafe fn fx_view(c: *mut Cpu) {
    let (dst, src, n) = ((*c).x[0], (*c).x[1], (*c).x[2]);
    guest::write_bytes(dst, &guest::bytes(src, n).to_vec());
    let scale = f(&ASPECT_SCALE);
    if scale <= 1.0 {
        return;
    }
    let all = env_str("NXRS_MKW_FX_WIDE_LR") == "all";
    let f1 = (*c).x[29];
    let f2 = if f1 != 0 { guest::r64(f1) } else { 0 };
    let fg = if FX_GROUP_FRAME == 1 { f1 } else { f2 };
    let lr2 = if f2 != 0 { guest::r64(f2 + 8) } else { 0 };
    let group = if fg != 0 { guest::r64(fg + 8 * FX_GROUP_WORD) } else { 0 };
    let wide = all || (lr2 == main_addr(FX_GROUP_LR) && group == FX_SCREEN_GROUP);
    if !wide {
        return;
    }
    let at = |k: usize| dst + 4 * k as u64;
    for i in 0..4 {
        for k in [16 + i, 32 + i] {
            guest::wr(at(k), guest::rd::<f32>(at(k)) * scale);
        }
        for k in [64 + 4 * i, 80 + 4 * i] {
            guest::wr(at(k), guest::rd::<f32>(at(k)) / scale);
        }
    }
}
