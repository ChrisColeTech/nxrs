//! Screenshots: the game's presented picture read back from the GPU at its own resolution (the window's crop applied,
//! NVN's bottom-up rows turned over), written as PNG. Triggered by present number, by a control file or by a request
//! (the front end's F3, the control pipe):
//!
//! - `NXGFX_SHOT=<n>[,<n>...]`: those presents (counted from 1);
//! - `NXGFX_SHOT_EVERY=<n>`: every n-th present;
//! - `NXGFX_SHOT_FILE=<path>`: when the file exists, the next present; the file is deleted (a path written in it is
//!   where the PNG goes);
//! - `NXGFX_SHOT_DIR=<dir>`: where they go (default `captures/` in the working directory);
//! - `request(path)`: the next present to `path` (empty: the default name).

use super::Gpu;
use crate::nvn::obj::tex_t;
use crate::nvn::types::NVNcopyRegion;
use ash::vk;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

struct Cfg {
    at: Vec<u64>,
    every: u64,
    file: Option<PathBuf>,
    dir: PathBuf,
}

fn cfg() -> &'static Cfg {
    static C: OnceLock<Cfg> = OnceLock::new();
    C.get_or_init(|| {
        let at = std::env::var("NXGFX_SHOT").ok().map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect()).unwrap_or_default();
        let every = std::env::var("NXGFX_SHOT_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        let file = std::env::var("NXGFX_SHOT_FILE").ok().map(PathBuf::from);
        let dir = std::env::var("NXGFX_SHOT_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("captures"));
        let c = Cfg { at, every, file, dir };
        if !c.at.is_empty() || c.every > 0 || c.file.is_some() {
            crate::vklog!("screenshots: {} presents named, every {}, control file {}, to {}", c.at.len(), c.every, c.file.as_ref().map(|p| p.display().to_string()).unwrap_or("-".into()), c.dir.display());
        }
        c
    })
}

static REQ: Mutex<Vec<String>> = Mutex::new(Vec::new());
static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The next present to `path` (empty: the default name); any thread.
pub fn request(path: &str) {
    let mut r = REQ.lock().unwrap();
    if r.len() < 8 {
        r.push(path.to_string());
        crate::vklog!("screenshot of the next present requested: {}", if path.is_empty() { "default name" } else { path });
    }
}

/// Screenshots written so far.
pub fn count() -> u64 {
    COUNT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Called with every present before it is queued (the frame's work is already submitted).
pub fn on_present(t: &tex_t, frame: u64, crop: Option<[i32; 4]>, origin_ll: bool) {
    let c = cfg();
    let mut want = c.at.contains(&frame) || (c.every > 0 && frame.is_multiple_of(c.every));
    let mut path = String::new();
    if let Some(f) = &c.file {
        if f.is_file() {
            if let Ok(s) = std::fs::read_to_string(f) {
                path = s.lines().next().unwrap_or("").trim().to_string();
                want = true;
                let _ = std::fs::remove_file(f);
            }
        }
    }
    if !want {
        let mut r = REQ.lock().unwrap();
        if !r.is_empty() {
            path = r.remove(0);
            want = true;
        }
    }
    if !want {
        return;
    }
    let path = if path.is_empty() {
        let _ = std::fs::create_dir_all(&c.dir);
        c.dir.join(format!("shot-{frame:06}.png"))
    } else {
        PathBuf::from(path)
    };
    let t0 = std::time::Instant::now();
    match capture(t, crop, origin_ll) {
        Some((mut px, w, h)) => {
            // opaque, as the window shows it: games leave alpha 0 under parts of the picture
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            let ok = png_write(&path, &px, w, h).is_ok();
            COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::vklog!("screenshot: present {frame} ({w}x{h}) {} {} ({:.1} ms)", if ok { "written to" } else { "could not be written to" }, path.display(), t0.elapsed().as_secs_f64() * 1e3);
        }
        None => crate::vklog!("screenshot of present {frame}: nothing to read back"),
    }
}

/// The presented texture as RGBA8 rows, top row first.
pub fn capture(t: &tex_t, crop: Option<[i32; 4]>, flip: bool) -> Option<(Vec<u8>, u32, u32)> {
    let g = Gpu::try_get()?;
    let tf = crate::tex::format::tex_format(t.d.format)?;
    let (tw, th) = (t.d.width as i32, t.d.height as i32);
    let mut r = NVNcopyRegion { xoffset: 0, yoffset: 0, zoffset: 0, width: tw, height: th, depth: 1 };
    if let Some([x, y, w, h]) = crop {
        if w > 0 && h > 0 && x >= 0 && y >= 0 && x + w <= tw && y + h <= th {
            r = NVNcopyRegion { xoffset: x, yoffset: y, zoffset: 0, width: w, height: h, depth: 1 };
        }
    }
    let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
    if bw != 1 || bh != 1 || bpb == 0 {
        return None;
    }
    let n = (r.width * r.height) as usize;
    let mut raw = vec![0u8; n * bpb as usize];
    g.read_texels(t, None, &r, raw.as_mut_ptr(), r.width as usize * bpb as usize, raw.len());
    let mut out = vec![0u8; n * 4];
    if !to_rgba8(tf.vk, &raw, n, &mut out) {
        crate::log::red_once(&format!("screenshot: the presented image's format {:?} is not converted", tf.vk));
        return None;
    }
    let (w, h) = (r.width as usize, r.height as usize);
    if flip {
        for y in 0..h / 2 {
            let (a, b) = out.split_at_mut((h - 1 - y) * w * 4);
            a[y * w * 4..(y + 1) * w * 4].swap_with_slice(&mut b[..w * 4]);
        }
    }
    Some((out, w as u32, h as u32))
}

fn f2u8(v: f32) -> u8 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= 1.0 {
        255
    } else {
        (v * 255.0).round() as u8
    }
}

pub fn half(h: u16) -> f32 {
    let s = ((h >> 15) as u32) << 31;
    let e = ((h >> 10) & 31) as u32;
    let m = (h & 1023) as u32;
    let f = if e == 0 {
        if m == 0 {
            s
        } else {
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 1024 == 0 {
                m <<= 1;
                e -= 1;
            }
            s | (e << 23) | ((m & 1023) << 13)
        }
    } else if e == 31 {
        s | 0x7f80_0000 | (m << 13)
    } else {
        s | ((e + 127 - 15) << 23) | (m << 13)
    };
    f32::from_bits(f)
}

/// Texels of `f` as RGBA8 (stored values; sRGB bytes stay sRGB, as the window shows them). False for a format the
/// screenshots do not know.
pub fn to_rgba8(f: vk::Format, s: &[u8], n: usize, d: &mut [u8]) -> bool {
    let rd32 = |i: usize| u32::from_le_bytes(s[i..i + 4].try_into().unwrap());
    let rd16 = |i: usize| u16::from_le_bytes(s[i..i + 2].try_into().unwrap());
    match f {
        vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB => d[..n * 4].copy_from_slice(&s[..n * 4]),
        vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB => {
            for i in 0..n {
                d[4 * i] = s[4 * i + 2];
                d[4 * i + 1] = s[4 * i + 1];
                d[4 * i + 2] = s[4 * i];
                d[4 * i + 3] = s[4 * i + 3];
            }
        }
        vk::Format::A2B10G10R10_UNORM_PACK32 => {
            for i in 0..n {
                let v = rd32(4 * i);
                d[4 * i] = ((v & 1023) >> 2) as u8;
                d[4 * i + 1] = ((v >> 10 & 1023) >> 2) as u8;
                d[4 * i + 2] = ((v >> 20 & 1023) >> 2) as u8;
                d[4 * i + 3] = ((v >> 30) * 85) as u8;
            }
        }
        vk::Format::R16G16B16A16_SFLOAT => {
            for i in 0..n {
                for c in 0..4 {
                    d[4 * i + c] = f2u8(half(rd16(8 * i + 2 * c)));
                }
            }
        }
        vk::Format::R16G16B16A16_UNORM => {
            for i in 0..n {
                for c in 0..4 {
                    d[4 * i + c] = (rd16(8 * i + 2 * c) >> 8) as u8;
                }
            }
        }
        vk::Format::R32G32B32A32_SFLOAT => {
            for i in 0..n {
                for c in 0..4 {
                    d[4 * i + c] = f2u8(f32::from_bits(rd32(16 * i + 4 * c)));
                }
            }
        }
        vk::Format::B10G11R11_UFLOAT_PACK32 => {
            for i in 0..n {
                let v = rd32(4 * i);
                let c = [v & 0x7ff, v >> 11 & 0x7ff, v >> 22];
                let mb = [6u32, 6, 5];
                for k in 0..3 {
                    let e = c[k] >> mb[k];
                    let m = c[k] & ((1 << mb[k]) - 1);
                    let f = if e != 0 { (1.0 + m as f32 / (1u32 << mb[k]) as f32) * 2f32.powi(e as i32 - 15) } else { m as f32 * 2f32.powi(-14 - mb[k] as i32) };
                    d[4 * i + k] = f2u8(f);
                }
                d[4 * i + 3] = 255;
            }
        }
        vk::Format::R32_SFLOAT => {
            for i in 0..n {
                let v = f2u8(f32::from_bits(rd32(4 * i)));
                d[4 * i..4 * i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        vk::Format::R16_SFLOAT => {
            for i in 0..n {
                let v = f2u8(half(rd16(2 * i)));
                d[4 * i..4 * i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        vk::Format::R8_UNORM => {
            for i in 0..n {
                d[4 * i..4 * i + 4].copy_from_slice(&[s[i], s[i], s[i], 255]);
            }
        }
        vk::Format::R8G8_UNORM => {
            for i in 0..n {
                d[4 * i..4 * i + 4].copy_from_slice(&[s[2 * i], s[2 * i + 1], 0, 255]);
            }
        }
        vk::Format::R16G16_SFLOAT => {
            for i in 0..n {
                d[4 * i..4 * i + 4].copy_from_slice(&[f2u8(half(rd16(4 * i))), f2u8(half(rd16(4 * i + 2))), 0, 255]);
            }
        }
        vk::Format::R5G6B5_UNORM_PACK16 => {
            for i in 0..n {
                let v = rd16(2 * i) as u32;
                d[4 * i..4 * i + 4].copy_from_slice(&[((v >> 11) * 255 / 31) as u8, ((v >> 5 & 63) * 255 / 63) as u8, ((v & 31) * 255 / 31) as u8, 255]);
            }
        }
        _ => return false,
    }
    true
}

fn crc32(mut c: u32, p: &[u8]) -> u32 {
    static TAB: OnceLock<[u32; 256]> = OnceLock::new();
    let t = TAB.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut v = i as u32;
            for _ in 0..8 {
                v = if v & 1 != 0 { 0xEDB8_8320 ^ (v >> 1) } else { v >> 1 };
            }
            *e = v;
        }
        t
    });
    c = !c;
    for &b in p {
        c = t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// 8-bit RGBA as PNG (RFC 2083): IHDR, one IDAT of a zlib stream of stored deflate blocks, IEND.
pub fn png_write(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) -> std::io::Result<()> {
    std::fs::write(path, png_bytes(rgba, w, h))
}

pub fn png_bytes(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let row = w as usize * 4 + 1;
    let raw = row * h as usize;
    let mut z = Vec::with_capacity(raw + raw / 65535 * 5 + 16);
    z.extend_from_slice(&[0x78, 0x01]);
    let (mut a, mut b) = (1u32, 0u32);
    let mut filtered = Vec::with_capacity(raw);
    for y in 0..h as usize {
        filtered.push(0);
        filtered.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    for &v in &filtered {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    let mut chunks = filtered.chunks(65535).peekable();
    if chunks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(c) = chunks.next() {
        let n = c.len() as u16;
        z.push(chunks.peek().is_none() as u8);
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut chunk = |ty: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(ty);
        out.extend_from_slice(data);
        let c = crc32(crc32(0, ty), data);
        out.extend_from_slice(&c.to_be_bytes());
    };
    let mut ih = Vec::with_capacity(13);
    ih.extend_from_slice(&w.to_be_bytes());
    ih.extend_from_slice(&h.to_be_bytes());
    ih.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ih);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_is_well_formed_and_half_floats_convert() {
        let px = [255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 1, 2, 3, 4];
        let p = super::png_bytes(&px, 2, 2);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
        assert_eq!(&p[12..16], b"IHDR");
        assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
        assert_eq!(super::half(0x3c00), 1.0);
        assert_eq!(super::half(0xc000), -2.0);
    }
}

// ---------- frames for video recordings (the front end's F7) ----------
/// The presented picture as BGRA rows (top first), its width and height.
pub type FrameSink = fn(bgra: &[u8], w: u32, h: u32);
static SINK: Mutex<Option<(u32, FrameSink, Option<std::time::Instant>)>> = Mutex::new(None);

/// While set, the presented picture is read back asynchronously at most `fps` times a second and handed to `f` on
/// the completion thread (never blocking the game); `None` stops it.
pub fn set_frame_sink(fps: u32, f: Option<FrameSink>) {
    *SINK.lock().unwrap() = f.map(|f| (fps.max(1), f, None));
}

/// In a present's submission: a copy of the presented texture for the frame sink when one is due.
pub fn record_sink(g: &Gpu, sub: &mut super::queue::Sub, t: &tex_t, crop: Option<[i32; 4]>, flip: bool) {
    let f = {
        let mut s = SINK.lock().unwrap();
        let Some((fps, f, last)) = s.as_mut() else { return };
        let now = std::time::Instant::now();
        if last.is_some_and(|l| now.duration_since(l).as_secs_f64() < 1.0 / *fps as f64 - 0.001) {
            return;
        }
        *last = Some(now);
        *f
    };
    let Some(tf) = crate::tex::format::tex_format(t.d.format) else { return };
    let (bpb, bw, bh) = crate::tex::layout::fmt_bpb(t.d.format);
    if bw != 1 || bh != 1 || bpb == 0 {
        return;
    }
    let (tw, th) = (t.d.width as i32, t.d.height as i32);
    let [x, y, w, h] = match crop {
        Some([x, y, w, h]) if w > 0 && h > 0 && x >= 0 && y >= 0 && x + w <= tw && y + h <= th => [x, y, w, h],
        _ => [0, 0, tw, th],
    };
    let image = {
        let mut imgs = g.images.lock().unwrap();
        if !g.ensure_image(&mut imgs, t) {
            return;
        }
        g.prepare_image(&mut imgs, t.serial, sub);
        imgs.by_serial[&t.serial].image
    };
    let n = w as u64 * h as u64 * bpb as u64;
    let Ok(buf) = g.buffer(n, vk::BufferUsageFlags::TRANSFER_DST, gpu_allocator::MemoryLocation::GpuToCpu, "frame sink") else { return };
    let r = vk::BufferImageCopy {
        buffer_offset: 0,
        buffer_row_length: 0,
        buffer_image_height: 0,
        image_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
        image_offset: vk::Offset3D { x, y, z: 0 },
        image_extent: vk::Extent3D { width: w as u32, height: h as u32, depth: 1 },
    };
    unsafe {
        super::queue::full_barrier(g, sub.main);
        g.dev().cmd_copy_image_to_buffer(sub.main, image, vk::ImageLayout::GENERAL, buf.buf, &[r]);
    }
    let ptr = buf.ptr as usize;
    let fmt = tf.vk;
    sub.transient.push(buf);
    sub.post.push(super::queue::Post::Callback(Box::new(move |_g: &Gpu| {
        let (w, h) = (w as usize, h as usize);
        // SAFETY: the read-back buffer, freed after this callback
        let src = unsafe { core::slice::from_raw_parts(ptr as *const u8, n as usize) };
        let mut px = vec![0u8; w * h * 4];
        if !to_rgba8(fmt, src, w * h, &mut px) {
            return;
        }
        let mut out = vec![0u8; w * h * 4];
        for row in 0..h {
            let sr = if flip { h - 1 - row } else { row };
            for c in 0..w {
                let s = (sr * w + c) * 4;
                let d = (row * w + c) * 4;
                out[d..d + 4].copy_from_slice(&[px[s + 2], px[s + 1], px[s], 255]);
            }
        }
        f(&out, w as u32, h as u32);
    })));
}
