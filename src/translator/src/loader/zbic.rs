//! ZBIC: the zstd variant Switch 2 uses for NSO segments (segment data starting with "ZBIC"). It is zstd with two
//! framing changes: the frame magic is "ZBIC", and every FSE table description (the sequences' literal length,
//! offset and match length tables, and the Huffman weight tables) is a binary interpolative coded header. Those
//! headers are rewritten into standard ones and the result goes to the stock zstd decoder.

use std::io::Read;

type R<T> = Result<T, String>;

pub const MAGIC: [u8; 4] = *b"ZBIC";
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
/// NSO header flag: the loader's UseZbicCompression (Ocarina of Time's `main` has flags 0xff).
pub const NSO_FLAG_ZBIC: u32 = 1 << 7;

/// Whether `data` starts a ZBIC frame.
pub fn is_zbic(data: &[u8]) -> bool {
    data.starts_with(&MAGIC)
}
const BLOCK_MAX: usize = 128 << 10;

/// (middle, first, last) in the interpolative coder's order over [0, 256]: midpoint, then the upper half, then the
/// lower half; a table of 2^n symbols is the tail of this one, which ends with (0, 0, 1).
fn bic_table() -> Vec<(usize, usize, usize)> {
    fn visit(lo: usize, hi: usize, out: &mut Vec<(usize, usize, usize)>) {
        if hi - lo < 2 {
            return;
        }
        let mid = (lo + hi) / 2;
        out.push((mid, lo, hi));
        visit(mid, hi, out);
        visit(lo, mid, out);
    }
    let mut out = vec![];
    visit(0, 256, &mut out);
    out.push((0, 0, 1));
    out
}

fn bad(why: &str) -> String {
    format!("ZBIC: {why}")
}

/// A BIC table header at `buf[pos..end]` → (normalized counts, table log, bytes used).
fn read_bic_ncount(buf: &[u8], pos: usize, end: usize, max_symbol: usize) -> R<(Vec<i64>, u32, usize)> {
    if end <= pos || end > buf.len() {
        return Err(bad("empty table header"));
    }
    let size = end - pos;
    let raw = (buf[pos] & 0x7f) as usize;
    let low = (buf[pos] >> 7) as i64;
    if raw + 1 >= size {
        return Err(bad("table header is truncated"));
    }
    // a mixed-radix number read from the header's last byte backwards
    let mut left = raw;
    let refill = |mut v: u64, left: &mut usize| -> u64 {
        while *left > 0 && v < (1u64 << 40) {
            v = buf[pos + *left] as u64 | (v << 8);
            *left -= 1;
        }
        v
    };
    let mut v = refill(0, &mut left);
    let nsym = (v % 0x34 + 1) as usize;
    v = refill(v / 0x34, &mut left);
    if nsym > max_symbol {
        return Err(bad("table header has too many symbols"));
    }
    let log = (v & 7) as u32 + 5;
    v = refill(v >> 3, &mut left);
    let mut total = 1i64 << log;
    let last = (v % total as u64) as i64 + 1 + if low != 0 { nsym as i64 + 1 } else { 0 };
    v = refill(v / total as u64, &mut left);
    let mut span = 1usize;
    while span <= nsym {
        span <<= 1;
    }
    if span > 0xff {
        return Err(bad("table header has too many symbols"));
    }
    let mut cum = [0i64; 257];
    cum[span] = last;
    let bic = bic_table();
    for &(mid, lo, hi) in &bic[256 - span..] {
        let (a, b) = (cum[lo], cum[hi]);
        if a == b {
            for c in cum.iter_mut().take(hi).skip(lo + 1) {
                *c = a;
            }
        } else {
            if b < a {
                return Err(bad("table header is damaged"));
            }
            let r = (b - a + 1) as u64;
            let entry = v % r;
            v = refill(v / r, &mut left);
            cum[mid] = a + entry as i64;
        }
    }
    let mut counts = vec![];
    let mut acc = 0i64;
    for c in cum.iter().take(nsym + 2).skip(1) {
        let count = c - acc - low;
        acc = *c;
        if count < -1 {
            return Err(bad("table header is damaged"));
        }
        counts.push(count);
        total -= count.abs();
    }
    if total != 0 || left != 0 {
        return Err(bad("table header is damaged"));
    }
    Ok((counts, log, raw + 1))
}

/// Bits written from the low end, as zstd's table descriptions are.
#[derive(Default)]
struct Bits {
    b: Vec<u8>,
    n: usize,
}
impl Bits {
    /// OR `v` in at the current position, then advance by `adv` bits.
    fn put(&mut self, v: u64, adv: usize) {
        let need = (self.n + 64 + 7) / 8 + 1;
        if self.b.len() < need {
            self.b.resize(need, 0);
        }
        for i in 0..64 {
            if v >> i & 1 != 0 {
                let p = self.n + i;
                self.b[p / 8] |= 1 << (p % 8);
            }
        }
        self.n += adv;
    }
    fn bytes(mut self) -> Vec<u8> {
        self.b.truncate((self.n + 7) / 8);
        self.b
    }
}

/// A standard zstd FSE table description (RFC 8878 4.1.1).
fn write_ncount(counts: &[i64], log: u32) -> R<Vec<u8>> {
    let mut bits = Bits::default();
    bits.put((log - 5) as u64, 4);
    let mut remaining = (1i64 << log) + 1;
    let mut threshold = 1i64 << log;
    let mut width = log as usize + 1;
    let mut symbol = 0usize;
    let mut previous0 = false;
    while symbol < counts.len() && remaining > 1 {
        if previous0 {
            let mut start = symbol;
            while symbol < counts.len() && counts[symbol] == 0 {
                symbol += 1;
            }
            if symbol == counts.len() {
                return Err(bad("table has no remaining probability"));
            }
            while symbol >= start + 3 {
                start += 3;
                bits.put(3, 2);
            }
            bits.put((symbol - start) as u64, 2);
        }
        let mut count = counts[symbol];
        symbol += 1;
        let top = 2 * threshold - 1 - remaining;
        remaining -= count.abs();
        count += 1;
        if count >= threshold {
            count += top;
        }
        bits.put(count as u64, width - (count < top) as usize);
        previous0 = count == 1;
        if remaining < 1 {
            return Err(bad("table probabilities overflow"));
        }
        while remaining < threshold {
            width -= 1;
            threshold >>= 1;
        }
    }
    if remaining != 1 {
        return Err(bad("table probabilities do not add up"));
    }
    Ok(bits.bytes())
}

fn table(buf: &[u8], pos: usize, end: usize, max_symbol: usize) -> R<(Vec<u8>, usize)> {
    let (counts, log, used) = read_bic_ncount(buf, pos, end, max_symbol)?;
    Ok((write_ncount(&counts, log)?, used))
}

/// (header bytes, size field width) of a compressed literals section by its size format.
const LITERAL_SIZES: [(usize, u32); 4] = [(3, 10), (3, 10), (4, 14), (5, 18)];

fn literals(buf: &[u8], p: usize, end: usize, out: &mut Vec<u8>) -> R<usize> {
    let b0 = *buf.get(p).ok_or_else(|| bad("literals are truncated"))?;
    let (kind, mut fmt) = (b0 & 3, ((b0 >> 2) & 3) as usize);
    let at = |i: usize| -> R<usize> { buf.get(i).map(|&x| x as usize).ok_or_else(|| bad("literals are truncated")) };
    if kind < 2 {
        let (head, size) = match fmt {
            0 | 2 => (1, (b0 >> 3) as usize),
            1 => (2, (b0 as usize >> 4) | at(p + 1)? << 4),
            _ => (3, (b0 as usize >> 4) | at(p + 1)? << 4 | at(p + 2)? << 12),
        };
        let n = head + if kind == 0 { size } else { 1 };
        if p + n > end {
            return Err(bad("literals are truncated"));
        }
        out.extend_from_slice(&buf[p..p + n]);
        return Ok(p + n);
    }
    let (head, width) = LITERAL_SIZES[fmt];
    if p + head > end {
        return Err(bad("literals are truncated"));
    }
    let mut v = 0u64;
    for i in 0..head {
        v |= (buf[p + i] as u64) << (8 * i);
    }
    let mask = (1u64 << width) - 1;
    let (regen, comp) = ((v >> 4) & mask, (v >> (4 + width)) & mask);
    let q = p + head;
    let end_lit = q + comp as usize;
    if end_lit > end {
        return Err(bad("literals are truncated"));
    }
    let mut body = buf[q..end_lit].to_vec();
    if kind == 2 && !body.is_empty() && body[0] < 128 {
        // Huffman weights compressed with FSE
        let h = body[0] as usize;
        if 1 + h > body.len() {
            return Err(bad("Huffman table is truncated"));
        }
        let (t, used) = table(&body, 1, 1 + h, 255)?;
        let mut weights = t;
        weights.extend_from_slice(&body[1 + used..1 + h]);
        if weights.len() >= 128 {
            return Err(bad("Huffman table does not fit a standard header"));
        }
        let mut nb = vec![weights.len() as u8];
        nb.extend_from_slice(&weights);
        nb.extend_from_slice(&body[1 + h..]);
        body = nb;
    }
    let comp = body.len() as u64;
    if fmt != 0 {
        while fmt < 3 && regen.max(comp) > (1u64 << LITERAL_SIZES[fmt].1) - 1 {
            fmt += 1;
        }
    }
    let (head, width) = LITERAL_SIZES[fmt];
    if regen.max(comp) >> width != 0 {
        return Err(bad("literals do not fit a standard header"));
    }
    let hv = kind as u64 | (fmt as u64) << 2 | regen << 4 | comp << (4 + width);
    out.extend_from_slice(&hv.to_le_bytes()[..head]);
    out.extend_from_slice(&body);
    Ok(end_lit)
}

fn block(buf: &[u8], mut p: usize, end: usize) -> R<Vec<u8>> {
    let mut out = vec![];
    p = literals(buf, p, end, &mut out)?;
    if p >= end {
        return Err(bad("sequences are truncated"));
    }
    let b = buf[p];
    let n = if b < 128 { 1 } else if b < 255 { 2 } else { 3 };
    if p + n > end {
        return Err(bad("sequences are truncated"));
    }
    out.extend_from_slice(&buf[p..p + n]);
    p += n;
    if b != 0 {
        if p >= end {
            return Err(bad("sequences are truncated"));
        }
        let modes = buf[p];
        out.push(modes);
        p += 1;
        for (shift, max_symbol) in [(6, 35), (4, 31), (2, 52)] {
            match (modes >> shift) & 3 {
                1 => {
                    out.push(*buf.get(p).ok_or_else(|| bad("sequences are truncated"))?);
                    p += 1;
                }
                2 => {
                    let (t, used) = table(buf, p, end, max_symbol)?;
                    out.extend_from_slice(&t);
                    p += used;
                }
                _ => {}
            }
        }
    }
    if p > end {
        return Err(bad("sequences are truncated"));
    }
    out.extend_from_slice(&buf[p..end]);
    Ok(out)
}

/// ZBIC frames → the same frames as standard zstd.
pub fn to_zstd(data: &[u8]) -> R<Vec<u8>> {
    let mut out = vec![];
    let mut p = 0;
    while p < data.len() {
        let magic = data.get(p..p + 4).ok_or_else(|| bad("frame header is truncated"))?;
        let m = u32::from_le_bytes(magic.try_into().unwrap());
        if m & 0xFFFF_FFF0 == 0x184D_2A50 {
            let n = 8 + u32::from_le_bytes(data.get(p + 4..p + 8).ok_or_else(|| bad("skippable frame is truncated"))?.try_into().unwrap()) as usize;
            out.extend_from_slice(data.get(p..p + n).ok_or_else(|| bad("skippable frame is truncated"))?);
            p += n;
            continue;
        }
        if magic != MAGIC {
            return Err(bad(&format!("no ZBIC frame at byte {p}")));
        }
        out.extend_from_slice(&ZSTD_MAGIC);
        p += 4;
        let fhd = *data.get(p).ok_or_else(|| bad("frame header is truncated"))?;
        let single = (fhd >> 5) & 1;
        let head = 1 + if single != 0 { 0 } else { 1 } + [0usize, 1, 2, 4][(fhd & 3) as usize] + [single as usize, 2, 4, 8][(fhd >> 6) as usize];
        out.extend_from_slice(data.get(p..p + head).ok_or_else(|| bad("frame header is truncated"))?);
        p += head;
        loop {
            if p + 3 > data.len() {
                return Err(bad("frame is truncated"));
            }
            let bh = data[p] as usize | (data[p + 1] as usize) << 8 | (data[p + 2] as usize) << 16;
            let (last, kind, size) = (bh & 1, (bh >> 1) & 3, bh >> 3);
            p += 3;
            let n = if kind == 1 { 1 } else { size };
            if kind == 3 || p + n > data.len() {
                return Err(bad("frame is damaged or truncated"));
            }
            if kind == 2 {
                let b = block(data, p, p + size)?;
                if b.len() > BLOCK_MAX {
                    return Err(bad("block does not fit a standard zstd block"));
                }
                let h = (b.len() << 3) | (2 << 1) | last;
                out.extend_from_slice(&(h as u32).to_le_bytes()[..3]);
                out.extend_from_slice(&b);
            } else {
                out.extend_from_slice(&data[p - 3..p + n]);
            }
            p += n;
            if last != 0 {
                break;
            }
        }
        if fhd & 4 != 0 {
            out.extend_from_slice(data.get(p..p + 4).ok_or_else(|| bad("checksum is truncated"))?);
            p += 4;
        }
    }
    Ok(out)
}

/// ZBIC data → exactly `size` bytes.
pub fn decompress(data: &[u8], size: usize) -> R<Vec<u8>> {
    let frames = to_zstd(data)?;
    let mut dec = zstd::stream::read::Decoder::with_buffer(&frames[..]).map_err(|e| bad(&e.to_string()))?;
    dec.window_log_max(31).map_err(|e| bad(&e.to_string()))?;
    let mut out = vec![];
    dec.take(size as u64 + 1).read_to_end(&mut out).map_err(|e| bad(&e.to_string()))?;
    if out.len() != size {
        return Err(bad(&format!("{} bytes decoded, {size} expected", out.len())));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text the sample was compressed from (generated, no game data).
    fn sample(n: usize) -> Vec<u8> {
        let words: [&[u8]; 20] = [b"link", b"zelda", b"ocarina", b"temple", b"rupee", b"sword", b"shield", b"deku", b"hyrule", b"navi", b"ganon", b"epona", b"castle", b"forest", b"water", b"fire", b"shadow", b"spirit", b"time", b"song"];
        let mut x: u64 = 12345;
        let mut out = vec![];
        while out.len() < n {
            x = (x * 1103515245 + 12345) & 0x7FFF_FFFF;
            let w = words[if x & 3 != 0 { (x >> 8) as usize % words.len() } else { (x >> 8) as usize % 4 }];
            out.extend_from_slice(w);
            if x & 16 != 0 {
                out.push(b' ');
            } else {
                out.extend_from_slice(&[48 + ((x >> 20) % 10) as u8, 10]);
            }
        }
        out.truncate(n);
        out.extend(vec![0u8; 4096]);
        out
    }

    #[test]
    fn reference_sample() {
        // compressed at level 19 by the reference ZBIC encoder: Huffman weight tables, all three sequence tables in
        // normal and low-probability form, several blocks
        let data = include_bytes!("../../tests/data/zbic_sample.zbic");
        let want = sample(70000);
        assert_eq!(decompress(data, want.len()).unwrap(), want);
        assert!(decompress(&data[..data.len() - 9], want.len()).is_err());
    }

    #[test]
    fn interpolative_table() {
        let t = bic_table();
        assert_eq!(t.len(), 256);
        assert_eq!(t[0], (128, 0, 256));
        assert_eq!(*t.last().unwrap(), (0, 0, 1));
    }
}
