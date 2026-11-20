//! Present to the host window and the pacing log.
//!
//! The front end gives the window (`set_window`); every NVN window presents into it. A present is a blit of the
//! game's texture into the acquired swap chain image, letterboxed to its aspect and flipped for NVN's lower-left
//! origin, in the queue's order after the work submitted before it. With no window, presents are counted only.
//! Pacing is the acquire's (`nvn::window`): the swap chain runs MAILBOX when offered so the display does not add a
//! second clock.
//!
//! The pacing line is the engine's log contract (the launcher and the harness parse it):
//! `[  t.tt] [vk] pacing: F FPS | p95 a p99 b max c ms | >16.67 n >33/50/100 x/y/z (last N intervals)`.

use super::queue::Sub;
use super::Gpu;
use crate::nvn::obj::tex_t;
use ash::vk;
use std::sync::Mutex;
use std::time::Instant;

pub struct PendingPresent {
    pub acquired: vk::Semaphore,
    pub rendered: vk::Semaphore,
    pub index: u32,
    pub swapchain: vk::SwapchainKHR,
}

#[derive(Default)]
pub struct Presenter {
    hwnd: isize,
    hinstance: isize,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    extent: vk::Extent2D,
    format: vk::Format,
    srgb: bool,
    acquire_sems: Vec<(vk::Semaphore, u64)>,
    rendered_sems: Vec<vk::Semaphore>,
    next_sem: usize,
    stale: bool,
    pub presented: u64,
    /// the swap chain waits for the display's refresh (forced_vsync)
    vsync: bool,
    /// made with room for frame generation's extra images
    framegen: bool,
}

/// The host window every NVN window presents into (Win32 HWND and HINSTANCE). 0 = headless.
pub fn set_window(hwnd: isize, hinstance: isize) {
    let Ok(g) = Gpu::get() else { return };
    let mut p = g.present.lock().unwrap();
    p.hwnd = hwnd;
    p.hinstance = hinstance;
    p.stale = true;
}

/// The options the display follows (nxconfig, applied live): `fps_limit` (0 unlimited, 30, 60..240; F4 switches
/// 60 / unlimited) and `forced_vsync`.
fn fps_limit() -> u32 {
    nxconfig::fps_limit_hz()
}

/// The game's frame pacing for a present interval: one display slot (1/60 s) per interval unit; with `fps_limit` 0
/// no wait at all, and with a limit above 60 Hz the slot is that short. At `fps_limit=30` the game keeps its 60 Hz
/// steps and every other image is not shown (`show`).
pub fn pace_period(interval: i32) -> u64 {
    if interval <= 0 {
        return 0;
    }
    let slot = match fps_limit() {
        0 => return 0,
        hz if hz > 60 => 1_000_000_000 / hz as u64,
        _ => crate::nvn::window::VSYNC_NS,
    };
    slot * interval as u64
}

/// Whether this present is shown (fps_limit=30: at most 30 images a second reach the window).
fn show(now: Instant) -> bool {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let skip = nxconfig::frame_skip();
    if skip > 0 {
        // fps_target 30 / 20: the frames whose draws were skipped are not shown
        return frames() % (skip as u64 + 1) == 1 % (skip as u64 + 1);
    }
    let hz = fps_limit();
    if hz != 30 {
        return true;
    }
    let mut l = LAST.lock().unwrap();
    match *l {
        Some(t) if now.duration_since(t).as_secs_f64() < 1.0 / 30.0 - 0.002 => false,
        _ => {
            *l = Some(now);
            true
        }
    }
}

/// Whether the front end gave a window already.
pub fn has_window() -> bool {
    Gpu::try_get().is_some_and(|g| g.present.lock().unwrap().hwnd != 0)
}

/// The window's size changed: the next present rebuilds the swap chain.
pub fn window_resized() {
    if let Some(g) = Gpu::try_get() {
        g.present.lock().unwrap().stale = true;
    }
}

/// Destroys the swap chain and surface (before the window goes).
pub fn close_window() {
    let Some(g) = Gpu::try_get() else { return };
    super::queue::finish();
    unsafe { g.dev().device_wait_idle() }.ok();
    let mut p = g.present.lock().unwrap();
    destroy_swapchain(g, &mut p);
    if p.surface != vk::SurfaceKHR::null() {
        unsafe { g.d.surface_i.destroy_surface(p.surface, None) };
        p.surface = vk::SurfaceKHR::null();
    }
    p.hwnd = 0;
}

fn destroy_swapchain(g: &Gpu, p: &mut Presenter) {
    if p.swapchain != vk::SwapchainKHR::null() {
        unsafe { g.d.swapchain.destroy_swapchain(p.swapchain, None) };
        p.swapchain = vk::SwapchainKHR::null();
    }
    for (s, _) in p.acquire_sems.drain(..) {
        unsafe { g.dev().destroy_semaphore(s, None) };
    }
    for s in p.rendered_sems.drain(..) {
        unsafe { g.dev().destroy_semaphore(s, None) };
    }
    p.images.clear();
}

fn create_swapchain(g: &Gpu, p: &mut Presenter, srgb: bool) -> Result<(), String> {
    if p.surface == vk::SurfaceKHR::null() {
        let ci = vk::Win32SurfaceCreateInfoKHR::default().hwnd(p.hwnd).hinstance(p.hinstance);
        p.surface = unsafe { g.d.win32_i.create_win32_surface(&ci, None) }.map_err(|e| format!("surface: {e}"))?;
        let ok = unsafe { g.d.surface_i.get_physical_device_surface_support(g.d.pdev, g.d.qfam, p.surface) }.unwrap_or(false);
        if !ok {
            return Err("the queue cannot present to this window".into());
        }
    }
    let caps = unsafe { g.d.surface_i.get_physical_device_surface_capabilities(g.d.pdev, p.surface) }.map_err(|e| e.to_string())?;
    if caps.current_extent.width == 0 || caps.current_extent.height == 0 {
        return Err("window minimized".into());
    }
    let formats = unsafe { g.d.surface_i.get_physical_device_surface_formats(g.d.pdev, p.surface) }.map_err(|e| e.to_string())?;
    let want = if srgb { vk::Format::B8G8R8A8_SRGB } else { vk::Format::B8G8R8A8_UNORM };
    let fmt = formats.iter().find(|f| f.format == want).or_else(|| formats.first()).ok_or("no surface format")?;
    let modes = unsafe { g.d.surface_i.get_physical_device_surface_present_modes(g.d.pdev, p.surface) }.map_err(|e| e.to_string())?;
    let vsync = nxconfig::flag("forced_vsync");
    p.vsync = vsync;
    let mode = match std::env::var("NXGFX_PRESENT").as_deref() {
        Ok("fifo") => vk::PresentModeKHR::FIFO,
        _ if vsync => vk::PresentModeKHR::FIFO,
        Ok("immediate") if modes.contains(&vk::PresentModeKHR::IMMEDIATE) => vk::PresentModeKHR::IMMEDIATE,
        _ if modes.contains(&vk::PresentModeKHR::MAILBOX) => vk::PresentModeKHR::MAILBOX,
        _ => vk::PresentModeKHR::FIFO,
    };
    // frame generation acquires up to four images for one game frame
    p.framegen = super::framegen::wanted();
    let n = (caps.min_image_count + if p.framegen { 4 } else { 1 }).min(if caps.max_image_count == 0 { u32::MAX } else { caps.max_image_count });
    if p.swapchain != vk::SwapchainKHR::null() {
        // the old chain's semaphores and images may still be in use: let everything submitted finish first
        super::queue::wait_fence(g.q.last_fence(), u64::MAX);
        destroy_swapchain(g, p);
    }
    let ci = vk::SwapchainCreateInfoKHR::default()
        .surface(p.surface)
        .min_image_count(n)
        .image_format(fmt.format)
        .image_color_space(fmt.color_space)
        .image_extent(caps.current_extent)
        .image_array_layers(1)
        .image_usage(vk::ImageUsageFlags::TRANSFER_DST)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(caps.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(mode)
        .clipped(true);
    let sc = unsafe { g.d.swapchain.create_swapchain(&ci, None) }.map_err(|e| format!("swapchain: {e}"))?;
    p.swapchain = sc;
    p.images = unsafe { g.d.swapchain.get_swapchain_images(sc) }.map_err(|e| e.to_string())?;
    p.extent = caps.current_extent;
    p.format = fmt.format;
    p.srgb = srgb;
    let sem = || unsafe { g.dev().create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }.unwrap();
    p.acquire_sems = (0..p.images.len() + 2).map(|_| (sem(), 0)).collect();
    p.rendered_sems = (0..p.images.len()).map(|_| sem()).collect();
    p.next_sem = 0;
    p.stale = false;
    crate::vklog!("swapchain {}x{}, {} images, {:?}, {:?}", p.extent.width, p.extent.height, p.images.len(), fmt.format, mode);
    Ok(())
}

/// Records a present of `t` into `sub` (acquire, blit, layout for present); the queue presents after submitting.
pub fn record_present(g: &Gpu, sub: &mut Sub, t: &tex_t, origin_ll: bool, crop: Option<[i32; 4]>) {
    if !t.valid() || g.present.lock().unwrap().hwnd == 0 || !show(Instant::now()) {
        return;
    }
    // the source first: once an image is acquired this submission must wait for it
    let Some(src) = ({
        let mut imgs = g.images.lock().unwrap();
        if g.ensure_image(&mut imgs, t) {
            g.prepare_image(&mut imgs, t.serial, sub);
            let im = &imgs.by_serial[&t.serial];
            {
                static SEEN: Mutex<Vec<u64>> = Mutex::new(Vec::new());
                let mut seen = SEEN.lock().unwrap();
                if seen.len() < 16 && !seen.contains(&t.serial) {
                    seen.push(t.serial);
                    crate::vklog!(
                        "present: texture ser{} {}x{} format {:#x} ({:?}) at {:#x}{}",
                        t.serial,
                        t.d.width,
                        t.d.height,
                        t.d.format,
                        im.format,
                        t.mem as u64,
                        im.alias_of.map(|o| format!(", the image of ser{o}")).unwrap_or_default()
                    );
                }
            }
            Some((im.image, im.extent, im.scale))
        } else {
            None
        }
    }) else {
        return;
    };
    let (src, sext, sscale) = (src.0, src.1, src.2);
    let srgb = crate::tex::format::tex_format(t.d.format).map(|f| f.srgb).unwrap_or(false);
    // source region (crop); NVN's lower-left origin: row 0 is the bottom of the picture
    let (cx, cy, cw, ch) = match crop {
        // the crop in the game's pixels, the image at its resolution scale
        Some([x, y, w, h]) if sscale != 1.0 => {
            let c = |v: i32| super::scale::coord(v, sscale);
            (c(x), c(y), c(x + w) - c(x), c(y + h) - c(y))
        }
        Some([x, y, w, h]) => (x, y, w, h),
        None => (0, 0, sext.width as i32, sext.height as i32),
    };
    let (sy0, sy1) = if origin_ll { (cy + ch, cy) } else { (cy, cy + ch) };
    // frame generation: the generated frames first, then the game's frame (from the history, upright)
    if let Some(out) = super::framegen::process(g, sub, src, cx, sy0, cx + cw, sy1) {
        for gi in out.gens {
            if !present_image(g, sub, gi, srgb, [0, 0, out.w as i32, out.h as i32]) {
                return;
            }
        }
        present_image(g, sub, out.real, srgb, [0, 0, out.w as i32, out.h as i32]);
        return;
    }
    present_image(g, sub, src, srgb, [cx, sy0, cx + cw, sy1]);
}

/// Acquires a swap chain image and records the blit of `src`'s region [x0, y0, x1, y1] (y0 > y1: turned over) into it,
/// letterboxed; the present goes to `sub.present`. False when no image could be acquired.
fn present_image(g: &Gpu, sub: &mut Sub, src: vk::Image, srgb: bool, r: [i32; 4]) -> bool {
    let mut p = g.present.lock().unwrap();
    let done = g.q.completed();
    let fg = super::framegen::wanted();
    if p.swapchain == vk::SwapchainKHR::null() || p.stale || p.srgb != srgb || p.vsync != nxconfig::flag("forced_vsync") || p.framegen != fg {
        if let Err(e) = create_swapchain(g, &mut p, srgb) {
            crate::log::red_once(&format!("present: {e}"));
            return false;
        }
    }
    // an acquire semaphore whose last waiting submission has completed
    let k = p.next_sem;
    p.next_sem = (k + 1) % p.acquire_sems.len();
    let (asem, f) = p.acquire_sems[k];
    if f > done {
        drop(p);
        super::queue::wait_fence(f, u64::MAX);
        p = g.present.lock().unwrap();
    }
    let idx = match unsafe { g.d.swapchain.acquire_next_image(p.swapchain, u64::MAX, asem, vk::Fence::null()) } {
        Ok((i, sub_opt)) => {
            if sub_opt {
                p.stale = true;
            }
            i
        }
        Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
            p.stale = true;
            return false;
        }
        Err(e) => {
            crate::log::red_once(&format!("vkAcquireNextImageKHR: {e}"));
            return false;
        }
    };
    p.acquire_sems[k].1 = g.q.last_fence() + 1;
    let dst = p.images[idx as usize];
    let ext = p.extent;
    let (sc, rsem) = (p.swapchain, p.rendered_sems[idx as usize]);
    drop(p);
    let dev = g.dev();
    let range = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
    let to_dst = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .image(dst)
        .subresource_range(range)];
    // the letterboxed destination
    let [x0, y0, x1, y1] = r;
    let (sw, sh) = (((x1 - x0).abs()).max(1) as f64, ((y1 - y0).abs()).max(1) as f64);
    let scale = (ext.width as f64 / sw).min(ext.height as f64 / sh);
    let (dw, dh) = ((sw * scale) as i32, (sh * scale) as i32);
    let (dx, dy) = ((ext.width as i32 - dw) / 2, (ext.height as i32 - dh) / 2);
    let blit = vk::ImageBlit {
        src_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
        src_offsets: [vk::Offset3D { x: x0, y: y0, z: 0 }, vk::Offset3D { x: x1, y: y1, z: 1 }],
        dst_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
        dst_offsets: [vk::Offset3D { x: dx, y: dy, z: 0 }, vk::Offset3D { x: dx + dw, y: dy + dh, z: 1 }],
    };
    let to_present = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
        .image(dst)
        .subresource_range(range)];
    unsafe {
        dev.cmd_pipeline_barrier2(sub.main, &vk::DependencyInfo::default().image_memory_barriers(&to_dst));
        dev.cmd_clear_color_image(sub.main, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] }, &[range]);
        let mb = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)];
        dev.cmd_pipeline_barrier2(sub.main, &vk::DependencyInfo::default().memory_barriers(&mb));
        dev.cmd_blit_image(sub.main, src, vk::ImageLayout::GENERAL, dst, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[blit], vk::Filter::LINEAR);
        dev.cmd_pipeline_barrier2(sub.main, &vk::DependencyInfo::default().image_memory_barriers(&to_present));
    }
    sub.present.push(PendingPresent { acquired: asem, rendered: rsem, index: idx, swapchain: sc });
    true
}

/// After the present's submission: hands the image to the display (queue lock held by the caller).
pub fn queue_present(g: &Gpu, p: PendingPresent) {
    let sems = [p.rendered];
    let scs = [p.swapchain];
    let idx = [p.index];
    let pi = vk::PresentInfoKHR::default().wait_semaphores(&sems).swapchains(&scs).image_indices(&idx);
    match unsafe { g.d.swapchain.queue_present(g.d.queue, &pi) } {
        Ok(false) => {}
        Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => g.present.lock().unwrap().stale = true,
        Err(e) => crate::log::red_once(&format!("vkQueuePresentKHR: {e}")),
    }
    g.present.lock().unwrap().presented += 1;
}

// ---------- pacing log ----------
struct Pace {
    last: Option<Instant>,
    t0: Option<Instant>,
    iv: Vec<f64>,
    frames: u64,
}
static PACE: Mutex<Pace> = Mutex::new(Pace { last: None, t0: None, iv: Vec::new(), frames: 0 });

fn pace_window() -> usize {
    std::env::var("NXGFX_PACE_N").ok().and_then(|v| v.parse().ok()).unwrap_or(300)
}

static PRESENT_NO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Presents so far (the replay's frame number).
pub fn frames() -> u64 {
    PRESENT_NO.load(std::sync::atomic::Ordering::Relaxed)
}

/// Every present call: its interval joins the window; a full window (or 5 s of a slow game) prints the pacing line.
/// Returns the present's number (from 1).
pub fn note_present() -> u64 {
    let no = PRESENT_NO.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    pace_note();
    no
}

fn pace_note() {
    let now = Instant::now();
    let mut p = PACE.lock().unwrap();
    p.frames += 1;
    if let Some(l) = p.last {
        if p.iv.is_empty() {
            p.t0 = Some(l);
        }
        let ms = now.duration_since(l).as_secs_f64() * 1000.0;
        p.iv.push(ms);
    }
    p.last = Some(now);
    let n = p.iv.len();
    let secs = p.t0.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
    if n == 0 || (n < pace_window() && secs < 5.0) {
        return;
    }
    let mut v = std::mem::take(&mut p.iv);
    let frames = p.frames;
    p.frames = 0;
    drop(p);
    let line = pacing_line(&mut v, secs);
    crate::vklog!("{line}");
    if let Some(g) = Gpu::try_get() {
        crate::vklog!("{}", super::mem::summary(g, frames));
    }
}

/// The pacing line's text after `[vk] `, for a window of present intervals (ms) spanning `secs`.
pub fn pacing_line(v: &mut [f64], secs: f64) -> String {
    let n = v.len();
    let (mut miss, mut s33, mut s50, mut s100) = (0, 0, 0, 0);
    for &x in v.iter() {
        miss += (x > 1000.0 / 60.0) as u32;
        s33 += (x > 33.0) as u32;
        s50 += (x > 50.0) as u32;
        s100 += (x > 100.0) as u32;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    format!(
        "pacing: {:.2} FPS | p95 {:.2} p99 {:.2} max {:.2} ms | >16.67 {miss} >33/50/100 {s33}/{s50}/{s100} (last {n} intervals)",
        n as f64 / secs.max(1e-9),
        v[n * 95 / 100],
        v[n * 99 / 100],
        v[n - 1]
    )
}

/// Sleeps about `ns` on a high-resolution waitable timer.
pub fn precise_sleep(ns: u64) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, INFINITE};
        thread_local! {
            static TIMER: isize = unsafe { CreateWaitableTimerExW(core::ptr::null(), core::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, 0x1F0003) as isize };
        }
        let h = TIMER.with(|t| *t);
        if h != 0 {
            let due: i64 = -((ns / 100) as i64).max(1);
            if SetWaitableTimer(h as _, &due, 0, None, core::ptr::null(), 0) != 0 {
                WaitForSingleObject(h as _, INFINITE);
                return;
            }
        }
        let _ = CloseHandle;
    }
    std::thread::sleep(std::time::Duration::from_nanos(ns));
}

#[cfg(test)]
mod tests {
    #[test]
    fn pacing_line_matches_the_parser() {
        let mut v: Vec<f64> = (0..60).map(|i| if i == 10 { 40.0 } else { 16.6 }).collect();
        let l = super::pacing_line(&mut v, 1.0);
        assert_eq!(l, "pacing: 60.00 FPS | p95 16.60 p99 40.00 max 40.00 ms | >16.67 1 >33/50/100 1/0/0 (last 60 intervals)");
    }
}

/// Presents handed to the display so far.
pub fn presented() -> u64 {
    Gpu::try_get().map(|g| g.present.lock().unwrap().presented).unwrap_or(0)
}
