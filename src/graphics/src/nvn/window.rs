//! Windows: the swap chain textures the game renders into, acquire with present pacing, and present.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use crate::vk;

/// Present pacer: present slot = previous slot + period. Early: wait until the slot. Late by less than a period:
/// present now and keep the phase. Behind by a period or more: restart the cadence from now (no burst to catch up).
/// Period 0 = no wait.
#[derive(Default, Clone, Copy, Debug)]
pub struct Pacer {
    pub target: u64,
}
impl Pacer {
    /// The ns to wait at `now` for a frame of `period` ns.
    pub fn pace(&mut self, now: u64, period: u64) -> u64 {
        if period == 0 || self.target == 0 {
            self.target = now;
            return 0;
        }
        let next = self.target + period;
        if now <= next {
            self.target = next;
            return next - now;
        }
        self.target = if now - next < period { next } else { now };
        0
    }
}

/// The display period of one present interval (the Switch's 60 Hz).
pub const VSYNC_NS: u64 = 16_666_667;

fn window_active(o: &win_t) -> i32 {
    if o.nactive > 0 && o.nactive < o.ntex { o.nactive } else { o.ntex }
}

/// Sleeps until `deadline` (ns on `device::now_ns`'s clock): a high-resolution waitable timer, then a short spin.
pub fn sleep_until(deadline: u64) {
    loop {
        let now = super::device::now_ns();
        if now >= deadline {
            return;
        }
        let left = deadline - now;
        if left > 1_500_000 {
            crate::vk::present::precise_sleep(left - 1_000_000);
        } else {
            std::hint::spin_loop();
            std::thread::yield_now();
        }
    }
}

/// nvnQueuePresentTexture.
pub fn present(q: &queue_t, w: &mut win_t, idx: i32) {
    if idx < 0 || idx >= w.ntex || w.tex[idx as usize].is_null() {
        crate::log::red(&format!("QueuePresentTexture: texture index {idx} of {}", w.ntex));
        return;
    }
    // SAFETY: the window's textures are the game's live texture objects.
    let t = unsafe { *(w.tex[idx as usize] as *const tex_t) };
    let crop = if w.crop[2] > 0 && w.crop[3] > 0 { Some(w.crop) } else { None };
    w.presents += 1;
    if vk::queue::aq_on() {
        super::blocking(|| vk::queue::aq_wait_room(true));
    }
    w.ticket[idx as usize] = vk::queue::present(q.serial, q.origin_ll != 0, &t, crop);
}

nvn_fns! {
    fn nvnWindowBuilderSetDefaults(b: *mut NVNwindowBuilder) {
        let o = b as *mut win_builder_t;
        let dev = (*o).dev;
        core::ptr::write_bytes(o as *mut u8, 0, core::mem::size_of::<win_builder_t>());
        (*o).dev = dev;
        (*o).interval = 1;
        // NVN2 builders are 0x80 bytes: their colour space and HDR metadata lie past NVN1's 0x40
        if super::device::SWITCH2.load(std::sync::atomic::Ordering::Relaxed) {
            *((b as *mut u8).add(WIN_BUILDER2_OFF) as *mut win_builder2_t) = win_builder2_t { color_space: 0, hdr_disp: HDR_DISPLAY_DEFAULT, hdr_content: HDR_CONTENT_DEFAULT };
        }
    }
    fn nvnWindowBuilderSetDevice(b: *mut NVNwindowBuilder, d: *mut NVNdevice) { (*(b as *mut win_builder_t)).dev = d; }
    fn nvnWindowBuilderSetNativeWindow(b: *mut NVNwindowBuilder, w: u64) { (*(b as *mut win_builder_t)).native = w; }
    fn nvnWindowBuilderSetTextures(b: *mut NVNwindowBuilder, n: i32, t: *const *mut NVNtexture) {
        let o = &mut *(b as *mut win_builder_t);
        let n = if !(0..=4).contains(&n) { crate::log::red(&format!("WindowBuilderSetTextures count {n}")); n.clamp(0, 4) } else { n };
        o.ntex = n;
        for i in 0..n as usize { o.tex[i] = *t.add(i); }
    }
    fn nvnWindowBuilderSetPresentInterval(b: *mut NVNwindowBuilder, v: i32) { (*(b as *mut win_builder_t)).interval = v; }
    fn nvnWindowBuilderGetPresentInterval(b: *const NVNwindowBuilder) -> i32 { (*(b as *const win_builder_t)).interval }
    fn nvnWindowBuilderSetNumActiveTextures(b: *mut NVNwindowBuilder, n: i32) { (*(b as *mut win_builder_t)).nactive = n; }
    fn nvnWindowBuilderGetNativeWindow(b: *const NVNwindowBuilder) -> u64 { (*(b as *const win_builder_t)).native }
    fn nvnWindowInitialize(w: *mut NVNwindow, b: *const NVNwindowBuilder) -> NVNboolean {
        let s = &*(b as *const win_builder_t);
        let o = w as *mut win_t;
        core::ptr::write_bytes(o as *mut u8, 0, core::mem::size_of::<win_t>());
        let o = &mut *o;
        o.magic = MAGIC_WIN;
        o.native = s.native;
        o.ntex = s.ntex;
        o.interval = s.interval;
        o.nactive = s.nactive;
        o.tex = s.tex;
        if super::device::SWITCH2.load(std::sync::atomic::Ordering::Relaxed) {
            let x = &*((b as *const u8).add(WIN_BUILDER2_OFF) as *const win_builder2_t);
            o.hdr_disp = x.hdr_disp;
            o.hdr_content = x.hdr_content;
        } else {
            o.hdr_disp = HDR_DISPLAY_DEFAULT;
            o.hdr_content = HDR_CONTENT_DEFAULT;
        }
        let (tw, th, tf) = if o.ntex > 0 && !o.tex[0].is_null() { let t = &*(o.tex[0] as *const tex_t); (t.d.width, t.d.height, t.d.format) } else { (0, 0, 0) };
        crate::nvnlog!("WindowInitialize swap chain {} textures ({} active), interval {}, first {}x{} format {:#x}", o.ntex, window_active(o), o.interval, tw, th, tf);
        1
    }
    fn nvnWindowFinalize(w: *mut NVNwindow) {
        let o = &mut *(w as *mut win_t);
        let ts = o.ticket;
        super::blocking(|| for t in ts { if t != 0 { vk::queue::present_wait(t); } });
        o.ticket = [0; 4];
        o.magic = 0;
    }
    fn nvnWindowSetDebugLabel(w: *mut NVNwindow, s: *const u8) { let _ = (w, s); }
    fn nvnWindowGetNativeWindow(w: *const NVNwindow) -> u64 { (*(w as *const win_t)).native }
    fn nvnWindowGetPresentInterval(w: *const NVNwindow) -> i32 { (*(w as *const win_t)).interval }
    fn nvnWindowSetPresentInterval(w: *mut NVNwindow, v: i32) {
        let o = &mut *(w as *mut win_t);
        if o.interval != v { crate::nvnlog!("the game sets present interval {} -> {}", o.interval, v); }
        o.interval = v;
    }
    fn nvnWindowSetCrop(w: *mut NVNwindow, x: i32, y: i32, ww: i32, hh: i32) { (*(w as *mut win_t)).crop = [x, y, ww, hh]; }
    fn nvnWindowGetCrop(w: *const NVNwindow, r: *mut NVNrectangle) {
        let o = &*(w as *const win_t);
        if o.crop[2] > 0 && o.crop[3] > 0 {
            *r = NVNrectangle { x: o.crop[0], y: o.crop[1], width: o.crop[2], height: o.crop[3] };
        } else {
            let (tw, th) = if o.ntex > 0 && !o.tex[0].is_null() { let t = &*(o.tex[0] as *const tex_t); (t.d.width as i32, t.d.height as i32) } else { (0, 0) };
            *r = NVNrectangle { x: 0, y: 0, width: tw, height: th };
        }
    }
    fn nvnWindowGetNumTextures(w: *const NVNwindow) -> i32 { (*(w as *const win_t)).ntex }
    fn nvnWindowGetNumActiveTextures(w: *const NVNwindow) -> i32 { window_active(&*(w as *const win_t)) }
    fn nvnWindowSetNumActiveTextures(w: *mut NVNwindow, n: i32) { (*(w as *mut win_t)).nactive = n; }
    /// Vsync blocks here, not in present (a game often presents while holding a lock): the texture's previous present
    /// must be done, and the pacer holds the frame to the present interval.
    fn nvnWindowAcquireTexture(w: *mut NVNwindow, s: *mut NVNsync, idx: *mut i32) -> u32 {
        let o = &mut *(w as *mut win_t);
        let n = window_active(o);
        let i = if n > 0 { let v = o.next % n; o.next = (o.next + 1) % n; v } else { 0 };
        *idx = i;
        let period = crate::vk::present::pace_period(forced_interval().unwrap_or(o.interval));
        let mut pacer = Pacer { target: o.pace };
        let wait = pacer.pace(super::device::now_ns(), period);
        o.pace = pacer.target;
        let tk = o.ticket[i as usize & 3];
        let until = o.pace;
        super::blocking(|| {
            if wait > 0 { sleep_until(until); }
            // the GPU side only: the vsync pacing above is not the GPU's
            let t0 = std::time::Instant::now();
            if tk != 0 { vk::queue::present_wait(tk); }
            vk::perf::add(vk::perf::C::WaitAcquireNs, t0.elapsed().as_nanos() as u64);
        });
        o.ticket[i as usize & 3] = 0;
        if !s.is_null() {
            let so = &mut *(s as *mut sync_t);
            // the texture is the game's once its last present is done (present_wait above): the sync is satisfied at
            // once, as mkw-recomp's (fenced, fence 0). A fence over every queue's work so far made Mario Kart World's
            // frame update (main+0x69d034, an infinite nvnSyncWait) wait for a queue parked on another queue's GPU
            // event the next frame would signal: one frame every ~13 s after the course map (lane K)
            so.fenced = 2;
            so.fence = 0;
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::Pacer;
    #[test]
    fn pacer_holds_phase_and_restarts_when_behind() {
        let mut p = Pacer::default();
        assert_eq!(p.pace(1000, 100), 0);
        assert_eq!(p.pace(1050, 100), 50); // early: wait to 1100
        assert_eq!(p.pace(1230, 100), 0); // late by 30 (< period): keep phase at 1200
        assert_eq!(p.target, 1200);
        assert_eq!(p.pace(1450, 100), 0); // behind by >= a period: restart at now
        assert_eq!(p.target, 1450);
        assert_eq!(p.pace(1460, 0), 0); // period 0: no wait
    }
}

/// A forced present interval (5A.6): the game presents every `n` vsyncs whatever it asks the window for.
/// The ini option `present_interval` (0 = the game's own, 1 / 2 / 3 forced) wins over the per-game profile's value
/// (`set_profile_interval`, from the profile's `present_interval` key). Off by default.
static PROFILE_INTERVAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// The game's profile forces this interval (0 = not forced); os calls it at start-up.
pub fn set_profile_interval(n: i32) {
    let n = n.clamp(0, 4);
    if PROFILE_INTERVAL.swap(n, std::sync::atomic::Ordering::Relaxed) != n && n > 0 {
        crate::nvnlog!("present interval forced to {n} by the game's profile");
    }
}

pub fn forced_interval() -> Option<i32> {
    let ini = nxconfig::int("present_interval");
    let n = if ini > 0 { ini as i32 } else { PROFILE_INTERVAL.load(std::sync::atomic::Ordering::Relaxed) };
    (n > 0).then_some(n.min(4))
}
