//! NVN2's device command buffers (storage for commands the GPU generates) and device message buffers (a storage
//! range shaders write printf records into, read back on the CPU).
//!
//! Message buffer memory (pool CPU address + offset): a header {u32 write offset, u32 capacity, u32 wrap end}, then
//! records from +0xc: u32 record size, u16 source stage code, i16 at +6, four u32 at +8..+0x18, a NUL-terminated
//! printf format at +0x18, then the arguments, 4-byte aligned, to the record's end.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;

const DMB_HEADER: u32 = 0xc;

unsafe fn dcb<'a>(d: *const NVNdeviceCommandBuffer) -> &'a mut dcb_t {
    &mut *(d as *mut dcb_t)
}
unsafe fn dmb<'a>(m: *const NVNdeviceMessageBuffer) -> &'a mut dmb_t {
    &mut *(m as *mut dmb_t)
}
/// The CPU address of a message buffer's storage (null without storage or CPU access).
unsafe fn dmb_mem(o: &dmb_t) -> *mut u8 {
    if o.pool.is_null() {
        return core::ptr::null_mut();
    }
    let base = super::pool::pool_cpu_map(&*o.pool);
    if base.is_null() { base } else { base.add(o.off as usize) }
}

/// The NVN shader stage of a record's source stage code (-1 = unknown).
pub fn msg_stage(code: u16) -> i32 {
    match code {
        0 => 0,
        4 => 1,
        3 => 2,
        1 => 3,
        2 => 4,
        5 => 5,
        0x1493 => 6,
        0x1494 => 7,
        0x14c1..=0x14c6 => 8,
        _ => -1,
    }
}

/// Formats a shader printf record's format string with its argument words: %d %i %u %x %X %o %c %f %F %e %E %g %G %a
/// %A, l / ll / h length modifiers (ll takes two words), flags, width and precision, %% and the vector forms %v2f ..
/// %v4f (and %v2d ..), whose components are joined by ", ".
pub fn format_message(fmt: &[u8], args: &[u32]) -> String {
    let mut out = String::new();
    let mut ai = 0usize;
    let mut next = |n: usize| -> u64 {
        let mut v = 0u64;
        for k in 0..n {
            v |= (args.get(ai).copied().unwrap_or(0) as u64) << (32 * k);
            ai += 1;
        }
        v
    };
    let mut i = 0;
    while i < fmt.len() {
        let ch = fmt[i];
        if ch != b'%' {
            out.push(ch as char);
            i += 1;
            continue;
        }
        i += 1;
        if i < fmt.len() && fmt[i] == b'%' {
            out.push('%');
            i += 1;
            continue;
        }
        let mut left = false;
        let mut zero = false;
        let mut plus = false;
        while i < fmt.len() && matches!(fmt[i], b'-' | b'0' | b'+' | b' ' | b'#') {
            match fmt[i] {
                b'-' => left = true,
                b'0' => zero = true,
                b'+' => plus = true,
                _ => {}
            }
            i += 1;
        }
        let mut width = 0usize;
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            width = width * 10 + (fmt[i] - b'0') as usize;
            i += 1;
        }
        let mut prec: Option<usize> = None;
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            let mut p = 0;
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                p = p * 10 + (fmt[i] - b'0') as usize;
                i += 1;
            }
            prec = Some(p);
        }
        let mut vec = 1usize;
        if i + 1 < fmt.len() && fmt[i] == b'v' && (b'2'..=b'4').contains(&fmt[i + 1]) {
            vec = (fmt[i + 1] - b'0') as usize;
            i += 2;
        }
        let mut words = 1usize;
        while i < fmt.len() && matches!(fmt[i], b'l' | b'h') {
            if fmt[i] == b'l' && i + 1 < fmt.len() && fmt[i + 1] == b'l' {
                words = 2;
                i += 1;
            }
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        let mut parts = Vec::new();
        for _ in 0..vec {
            let s = match conv {
                b'd' | b'i' => {
                    let v = if words == 2 { next(2) as i64 } else { next(1) as u32 as i32 as i64 };
                    if plus && v >= 0 { format!("+{v}") } else { v.to_string() }
                }
                b'u' => next(words).to_string(),
                b'x' => format!("{:x}", next(words)),
                b'X' => format!("{:X}", next(words)),
                b'o' => format!("{:o}", next(words)),
                b'c' => ((next(1) as u8) as char).to_string(),
                b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                    let v = f32::from_bits(next(1) as u32) as f64;
                    let p = prec.unwrap_or(6);
                    let s = match conv {
                        b'e' => format!("{v:.p$e}"),
                        b'E' => format!("{v:.p$E}"),
                        b'g' | b'G' | b'a' | b'A' => format!("{v}"),
                        _ => format!("{v:.p$}"),
                    };
                    if plus && v >= 0.0 { format!("+{s}") } else { s }
                }
                _ => {
                    // an unknown conversion is copied as written
                    format!("%{}", conv as char)
                }
            };
            parts.push(s);
        }
        let s = parts.join(", ");
        let pad = width.saturating_sub(s.chars().count());
        if left {
            out.push_str(&s);
            out.extend(std::iter::repeat_n(' ', pad));
        } else if zero && vec == 1 && !matches!(conv, b'c') {
            let (sign, digits) = if s.starts_with(['-', '+']) { s.split_at(1) } else { ("", s.as_str()) };
            out.push_str(sign);
            out.extend(std::iter::repeat_n('0', pad));
            out.push_str(digits);
        } else {
            out.extend(std::iter::repeat_n(' ', pad));
            out.push_str(&s);
        }
    }
    out
}

nvn_fns! {
    // ---------- device command buffers ----------
    fn nvnDeviceCommandBufferSetDefaults(d: *mut NVNdeviceCommandBuffer) {
        let dev = dcb(d).dev;
        core::ptr::write_bytes(d as *mut u8, 0, core::mem::size_of::<dcb_t>());
        dcb(d).dev = dev;
    }
    fn nvnDeviceCommandBufferSetDevice(d: *mut NVNdeviceCommandBuffer, dev: *mut NVNdevice) { dcb(d).dev = dev; }
    fn nvnDeviceCommandBufferSetStorage(d: *mut NVNdeviceCommandBuffer, p: *mut NVNmemoryPool, off: i64, n: u64) {
        let o = dcb(d);
        o.pool = p as *const pool_t;
        o.off = off;
        o.size = n;
    }
    fn nvnDeviceCommandBufferSetCommandMemorySize(d: *mut NVNdeviceCommandBuffer, n: u64) { dcb(d).cmd_size = n; }
    fn nvnDeviceCommandBufferSetComputeMemorySize(d: *mut NVNdeviceCommandBuffer, n: u64) { dcb(d).comp_size = n; }
    /// The storage's GPU address (= its pool's address + offset).
    fn nvnDeviceCommandBufferGetHandle(d: *const NVNdeviceCommandBuffer) -> u64 {
        let o = dcb(d);
        if o.pool.is_null() { 0 } else { (*o.pool).mem as u64 + o.off as u64 }
    }

    // ---------- device message buffers ----------
    fn nvnDeviceMessageBufferSetDefaults(m: *mut NVNdeviceMessageBuffer) {
        let dev = dmb(m).dev;
        core::ptr::write_bytes(m as *mut u8, 0, core::mem::size_of::<dmb_t>());
        dmb(m).dev = dev;
    }
    fn nvnDeviceMessageBufferSetDevice(m: *mut NVNdeviceMessageBuffer, d: *mut NVNdevice) { dmb(m).dev = d; }
    fn nvnDeviceMessageBufferSetStorage(m: *mut NVNdeviceMessageBuffer, p: *mut NVNmemoryPool, off: i32, n: i32) {
        let o = dmb(m);
        o.pool = p as *const pool_t;
        o.off = off;
        o.size = n;
    }
    /// Empties the buffer: header {0xc, capacity, 0} written on the CPU.
    fn nvnDeviceMessageBufferReset(m: *mut NVNdeviceMessageBuffer) {
        let o = dmb(m);
        let mem = dmb_mem(o);
        if mem.is_null() || o.size < DMB_HEADER as i32 {
            crate::log::red_once("DeviceMessageBufferReset: no CPU-visible storage of at least 12 bytes");
            return;
        }
        core::ptr::write_unaligned(mem as *mut [u32; 3], [DMB_HEADER, o.size as u32, 0]);
        crate::vk::mem::cpu_flushed(mem as u64, DMB_HEADER as u64);
        o.cursor = 0;
    }
    /// Starts reading the records written so far.
    fn nvnDeviceMessageBufferBeginMessages(m: *mut NVNdeviceMessageBuffer) {
        let o = dmb(m);
        let mem = dmb_mem(o);
        if mem.is_null() { o.cursor = 0; o.end = 0; return; }
        crate::vk::mem::cpu_invalidate(mem as u64, o.size.max(0) as u64);
        let w = core::ptr::read_unaligned(mem as *const u32);
        o.end = w.min(o.size.max(0) as u32);
        o.cursor = DMB_HEADER;
    }
    /// The next record: info = {the record's i16, NVN stage, its four words}, its formatted text (NUL-terminated,
    /// truncated to `n` bytes); false when there is none.
    fn nvnDeviceMessageBufferGetNextMessage(m: *mut NVNdeviceMessageBuffer, info: *mut i32, text: *mut u8, n: usize) -> NVNboolean {
        let o = dmb(m);
        let mem = dmb_mem(o);
        if mem.is_null() || o.cursor < DMB_HEADER || o.cursor + 0x18 > o.end { return 0; }
        let r = mem.add(o.cursor as usize);
        let size = core::ptr::read_unaligned(r as *const u32);
        if size < 0x18 || o.cursor as u64 + size as u64 > o.end as u64 {
            crate::log::red_once(&format!("DeviceMessageBufferGetNextMessage: a malformed record (size {size:#x} at {:#x})", o.cursor));
            o.cursor = o.end;
            return 0;
        }
        let code = core::ptr::read_unaligned(r.add(4) as *const u16);
        let tag = core::ptr::read_unaligned(r.add(6) as *const i16);
        if !info.is_null() {
            *info = tag as i32;
            *info.add(1) = msg_stage(code);
            for k in 0..4 { *info.add(2 + k) = core::ptr::read_unaligned(r.add(8 + 4 * k) as *const i32); }
        }
        let body = core::slice::from_raw_parts(r.add(0x18), size as usize - 0x18);
        let flen = body.iter().position(|&c| c == 0).unwrap_or(body.len());
        let aoff = (flen + 1).next_multiple_of(4).min(body.len());
        let args: Vec<u32> = body[aoff..].chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        if !text.is_null() && n > 0 {
            let s = format_message(&body[..flen], &args);
            let k = s.len().min(n - 1);
            core::ptr::copy_nonoverlapping(s.as_ptr(), text, k);
            *text.add(k) = 0;
        }
        o.cursor += size;
        1
    }
    fn nvnDeviceMessageBufferEndMessages(m: *mut NVNdeviceMessageBuffer) { dmb(m).cursor = 0; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printf_records() {
        assert_eq!(format_message(b"x=%d u=%u h=%04x %%", &[(-5i32) as u32, 7, 0xab]), "x=-5 u=7 h=00ab %");
        assert_eq!(format_message(b"%.2f|%v3f|%lld", &[1.5f32.to_bits(), 1f32.to_bits(), 2f32.to_bits(), 3f32.to_bits(), 5, 1]), "1.50|1.000000, 2.000000, 3.000000|4294967301");
    }

    #[test]
    fn message_buffer_reads_records() {
        let mut mem = vec![0u8; 0x100];
        let mut pool = NVNmemoryPool::boxed();
        unsafe {
            let p = &mut *(pool.0.as_mut_ptr() as *mut pool_t);
            p.flags = mem_flags::CPU_CACHED | mem_flags::GPU_CACHED;
            p.mem = mem.as_mut_ptr();
            p.size = 0x100;
        }
        let mut m = NVNdeviceMessageBuffer::boxed();
        let mp = &mut *m as *mut NVNdeviceMessageBuffer;
        unsafe {
            nvnDeviceMessageBufferSetDefaults(mp);
            nvnDeviceMessageBufferSetStorage(mp, &mut *pool, 0, 0x100);
            nvnDeviceMessageBufferReset(mp);
        }
        assert_eq!(&mem[0..12], &[0xc, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        // one record from a fragment shader (code 4): "v %d\0" + one argument
        let mut rec = Vec::new();
        rec.extend_from_slice(&0x24u32.to_le_bytes());
        rec.extend_from_slice(&4u16.to_le_bytes());
        rec.extend_from_slice(&(-2i16).to_le_bytes());
        for k in 0..4u32 { rec.extend_from_slice(&(10 + k).to_le_bytes()); }
        rec.extend_from_slice(b"v %d\0\0\0\0");
        rec.extend_from_slice(&42u32.to_le_bytes());
        assert_eq!(rec.len(), 0x24);
        mem[0xc..0xc + 0x24].copy_from_slice(&rec);
        mem[0..4].copy_from_slice(&(0xc + 0x24u32).to_le_bytes());
        let mut info = [0i32; 6];
        let mut text = [0u8; 16];
        unsafe {
            nvnDeviceMessageBufferBeginMessages(mp);
            assert_eq!(nvnDeviceMessageBufferGetNextMessage(mp, info.as_mut_ptr(), text.as_mut_ptr(), text.len()), 1);
            assert_eq!(info, [-2, 1, 10, 11, 12, 13]);
            assert_eq!(&text[..5], b"v 42\0");
            assert_eq!(nvnDeviceMessageBufferGetNextMessage(mp, info.as_mut_ptr(), text.as_mut_ptr(), text.len()), 0);
            nvnDeviceMessageBufferEndMessages(mp);
        }
    }
}
