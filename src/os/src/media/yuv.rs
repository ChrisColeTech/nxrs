//! Decoded pictures (8-bit 4:2:0 planes owned by the host) and the layouts games read them in: NV12 with the
//! luma stride equal to the width (the movie decoders' CPU route), and RGBA (their texture route), converted by
//! the colour matrix and range the stream declares (undeclared: BT.709 from 720 lines up, else BT.601).

#[derive(Clone, Default)]
pub struct Picture {
    pub w: usize,
    pub h: usize,
    /// Luma, w x h.
    pub y: Vec<u8>,
    /// Interleaved chroma (NV12's second plane), (h + 1) / 2 rows of ((w + 1) / 2) * 2 bytes.
    pub uv: Vec<u8>,
    /// Some(true) BT.709, Some(false) BT.601, None undeclared.
    pub bt709: Option<bool>,
    pub full_range: bool,
    /// The time the packet carried, in the caller's unit.
    pub pts: i64,
}

impl Picture {
    pub fn cw(&self) -> usize {
        self.w.div_ceil(2)
    }
    pub fn ch(&self) -> usize {
        self.h.div_ceil(2)
    }
    /// From planar 4:2:0 rows (each plane with its own stride).
    #[allow(clippy::too_many_arguments)]
    pub fn from_i420(
        w: usize,
        h: usize,
        y: &[u8],
        ys: usize,
        u: &[u8],
        us: usize,
        v: &[u8],
        vs: usize,
    ) -> Picture {
        let mut p = Picture {
            w,
            h,
            y: vec![0; w * h],
            uv: vec![0; w.div_ceil(2) * 2 * h.div_ceil(2)],
            ..Default::default()
        };
        for r in 0..h {
            p.y[r * w..r * w + w].copy_from_slice(&y[r * ys..r * ys + w]);
        }
        let cw = p.cw();
        for r in 0..p.ch() {
            let o = &mut p.uv[r * cw * 2..(r + 1) * cw * 2];
            for x in 0..cw {
                o[2 * x] = u[r * us + x];
                o[2 * x + 1] = v[r * vs + x];
            }
        }
        p
    }
    /// From NV12 rows (luma and chroma with one stride).
    pub fn from_nv12(w: usize, h: usize, y: &[u8], uv: &[u8], stride: usize) -> Picture {
        let mut p = Picture {
            w,
            h,
            y: vec![0; w * h],
            uv: vec![0; w.div_ceil(2) * 2 * h.div_ceil(2)],
            ..Default::default()
        };
        for r in 0..h {
            p.y[r * w..r * w + w].copy_from_slice(&y[r * stride..r * stride + w]);
        }
        let n = p.cw() * 2;
        for r in 0..p.ch() {
            p.uv[r * n..r * n + n].copy_from_slice(&uv[r * stride..r * stride + n]);
        }
        p
    }

    fn bt709_for(&self, h: usize) -> bool {
        self.bt709.unwrap_or(h >= 720)
    }

    /// NV12 of `w` x `h` with stride `w` (a smaller picture padded with black, a larger one cropped) into `dst`
    /// (w * h * 3 / 2 bytes).
    pub fn nv12_into(&self, dst: &mut [u8], w: usize, h: usize) {
        let (cw, ch) = (self.w.min(w), self.h.min(h));
        if cw < w || ch < h {
            dst[..w * h].fill(16);
            dst[w * h..w * h + w * (h / 2)].fill(128);
        }
        for r in 0..ch {
            dst[r * w..r * w + cw].copy_from_slice(&self.y[r * self.w..r * self.w + cw]);
        }
        let n = (cw.div_ceil(2) * 2).min(w);
        let sn = self.cw() * 2;
        for r in 0..ch.div_ceil(2).min(h / 2) {
            let o = w * h + r * w;
            dst[o..o + n].copy_from_slice(&self.uv[r * sn..r * sn + n]);
        }
    }

    /// RGBA rows of `w` pixels (stride w * 4) for `w` x `h`; padding opaque black.
    pub fn rgba_into(&self, dst: &mut [u8], w: usize, h: usize) {
        let full = self.full_range;
        let b709 = self.bt709_for(h);
        let (ky, kc) = if full {
            (1.0, 1.0)
        } else {
            (255.0 / 219.0, 255.0 / 224.0)
        };
        let (rv, gu, gv, bu) = if b709 {
            (1.5748, 0.1873, 0.4681, 1.8556)
        } else {
            (1.402, 0.3441, 0.7141, 1.772)
        };
        let f = |x: f64| (x * 65536.0) as i32;
        let (yk, rvk, guk, gvk, buk) = (f(ky), f(rv * kc), f(gu * kc), f(gv * kc), f(bu * kc));
        let y0 = if full { 0 } else { 16 };
        let (cw, ch) = (self.w.min(w), self.h.min(h));
        if cw < w || ch < h {
            for px in dst[..w * h * 4].chunks_exact_mut(4) {
                px.copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        let sn = self.cw() * 2;
        for r in 0..ch {
            let py = &self.y[r * self.w..];
            let puv = &self.uv[(r / 2) * sn..];
            let o = &mut dst[r * w * 4..];
            for x in 0..cw {
                let l = (py[x] as i32 - y0) * yk;
                let u = puv[(x / 2) * 2] as i32 - 128;
                let v = puv[(x / 2) * 2 + 1] as i32 - 128;
                let cl = |v: i32| v.clamp(0, 255) as u8;
                o[4 * x] = cl((l + rvk * v + 32768) >> 16);
                o[4 * x + 1] = cl((l - guk * u - gvk * v + 32768) >> 16);
                o[4 * x + 2] = cl((l + buk * u + 32768) >> 16);
                o[4 * x + 3] = 255;
            }
        }
    }

    /// The NV12 colour-space number nn::movie reports (1 BT.601 limited, 2 BT.601 full, 3 BT.709 limited, 4 BT.709 full).
    pub fn colorspace(&self) -> i32 {
        (if self.bt709_for(self.h) { 3 } else { 1 }) + self.full_range as i32
    }

    /// True when any luma sample is not black: a decoder that hands out black pictures shows here.
    pub fn lit(&self) -> bool {
        self.y.iter().step_by(97).any(|&v| v > 24)
    }
}

/// An RGBA picture as a PNG file (stored deflate blocks; a debugging aid: `NXOS_MOVIE_DUMP`).
pub fn write_png(path: &std::path::Path, rgba: &[u8], w: usize, h: usize) -> std::io::Result<()> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut t = ty.to_vec();
        t.extend_from_slice(data);
        out.extend_from_slice(&t);
        out.extend_from_slice(&crc(&t).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for r in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[r * w * 4..(r + 1) * w * 4]);
    }
    let mut z = vec![0x78, 0x01];
    for (i, blk) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(blk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(blk.len() as u16)).to_le_bytes());
        z.extend_from_slice(blk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = vec![];
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        let (y, u, v) = (vec![235u8; 4 * 2], vec![128u8; 2], vec![128u8; 2]);
        let p = Picture::from_i420(4, 2, &y, 4, &u, 2, &v, 2);
        let mut nv = vec![0u8; 8 * 4 * 3 / 2];
        p.nv12_into(&mut nv, 8, 4);
        assert_eq!(&nv[0..4], &[235; 4]);
        assert_eq!(nv[4], 16, "padded with black");
        let mut rgba = vec![0u8; 4 * 2 * 4];
        p.rgba_into(&mut rgba, 4, 2);
        assert_eq!(&rgba[0..4], &[255, 255, 255, 255], "limited-range white");
        assert!(p.lit());
    }
}
