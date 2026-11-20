//! Raw HID devices. Linux (below, `linux`): hidraw, the same surface. Windows: the interfaces SetupAPI lists, opened read+write with full sharing (as SDL and hidapi
//! do: denying FILE_SHARE_WRITE fails with error 32 whenever another handle on the pad can write, which on the owner's
//! PC is always), overlapped reads drained without blocking, and bounded writes.

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
#[cfg(windows)]
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
#[cfg(windows)]
use windows_sys::Win32::Foundation::*;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::*;
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
#[cfg(windows)]
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

#[derive(Clone, Debug, Default)]
pub struct HidInfo {
    pub path: Vec<u16>,
    pub vid: u16,
    pub pid: u16,
    pub version: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    pub product: String,
    /// The interface path names the Bluetooth stack (`{00001124-…}` or `bth`), not USB.
    pub bluetooth: bool,
}

impl HidInfo {
    pub fn path_string(&self) -> String {
        String::from_utf16_lossy(&self.path[..self.path.len().saturating_sub(1)])
    }
    pub fn is_gamepad(&self) -> bool {
        self.usage_page == 1 && (self.usage == 4 || self.usage == 5 || self.usage == 8)
    }
}

#[cfg(windows)]
fn open_raw(path: &[u16], access: u32, overlapped: bool) -> HANDLE {
    unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            if overlapped { FILE_FLAG_OVERLAPPED } else { 0 },
            std::ptr::null_mut(),
        )
    }
}

#[cfg(windows)]
fn describe(path: Vec<u16>) -> Option<HidInfo> {
    let h = open_raw(&path, 0, false);
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut info = HidInfo { path, ..Default::default() };
    unsafe {
        let mut a: HIDD_ATTRIBUTES = std::mem::zeroed();
        a.Size = std::mem::size_of::<HIDD_ATTRIBUTES>() as u32;
        if HidD_GetAttributes(h, &mut a) {
            info.vid = a.VendorID;
            info.pid = a.ProductID;
            info.version = a.VersionNumber;
        }
        let mut pp: PHIDP_PREPARSED_DATA = 0;
        if HidD_GetPreparsedData(h, &mut pp) {
            let mut caps: HIDP_CAPS = std::mem::zeroed();
            if HidP_GetCaps(pp, &mut caps) == HIDP_STATUS_SUCCESS {
                info.usage_page = caps.UsagePage;
                info.usage = caps.Usage;
                info.input_len = caps.InputReportByteLength;
                info.output_len = caps.OutputReportByteLength;
                info.feature_len = caps.FeatureReportByteLength;
            }
            HidD_FreePreparsedData(pp);
        }
        let mut name = [0u16; 128];
        if HidD_GetProductString(h, name.as_mut_ptr() as *mut c_void, (name.len() * 2) as u32) {
            let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            info.product = String::from_utf16_lossy(&name[..n]);
        }
        CloseHandle(h);
    }
    let p = info.path_string().to_ascii_lowercase();
    info.bluetooth = p.contains("{00001124-0000-1000-8000-00805f9b34fb}") || p.contains("bthenum") || p.contains("&col") && p.contains("bth");
    Some(info)
}

/// Every present HID interface.
#[cfg(windows)]
pub fn enumerate() -> Vec<HidInfo> {
    let mut out = vec![];
    unsafe {
        let guid = GUID_DEVINTERFACE_HID;
        let list = SetupDiGetClassDevsW(&guid, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if list == INVALID_HANDLE_VALUE as HDEVINFO {
            return out;
        }
        for i in 0.. {
            let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(list, std::ptr::null(), &guid, i, &mut iface) == 0 {
                break;
            }
            let mut needed = 0u32;
            SetupDiGetDeviceInterfaceDetailW(list, &iface, std::ptr::null_mut(), 0, &mut needed, std::ptr::null_mut());
            if needed < 8 {
                continue;
            }
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(list, &iface, detail, needed, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                continue;
            }
            let p = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let mut path: Vec<u16> = std::slice::from_raw_parts(p, n).to_vec();
            path.push(0);
            if let Some(d) = describe(path) {
                out.push(d);
            }
        }
        SetupDiDestroyDeviceInfoList(list);
    }
    out
}

/// An open HID interface: overlapped input, bounded output.
#[cfg(windows)]
pub struct HidDev {
    h: HANDLE,
    ov: Box<OVERLAPPED>,
    buf: Vec<u8>,
    pending: bool,
    pub writable: bool,
    pub info: HidInfo,
    pub open_error: u32,
}

#[cfg(windows)]
unsafe impl Send for HidDev {}

pub enum Read {
    /// A whole report.
    Report(Vec<u8>),
    /// Nothing queued now.
    Empty,
    /// The device is gone or failed (the Win32 error).
    Gone(u32),
}

#[cfg(windows)]
impl HidDev {
    /// Opens read+write, or read-only when a write handle is refused (`writable` false, `open_error` the reason).
    pub fn open(info: &HidInfo, write: bool) -> Option<HidDev> {
        let mut writable = write;
        let mut err = 0;
        let mut h = open_raw(&info.path, GENERIC_READ | if write { GENERIC_WRITE } else { 0 }, true);
        if h == INVALID_HANDLE_VALUE && write {
            err = unsafe { GetLastError() };
            h = open_raw(&info.path, GENERIC_READ, true);
            writable = false;
        }
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut ov: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
        ov.hEvent = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if ov.hEvent.is_null() {
            unsafe { CloseHandle(h) };
            return None;
        }
        let len = (info.input_len as usize).max(64);
        Some(HidDev { h, ov, buf: vec![0; len], pending: false, writable, info: info.clone(), open_error: err })
    }

    /// The next queued input report, without blocking.
    pub fn read(&mut self) -> Read {
        let mut n = 0u32;
        unsafe {
            if self.pending {
                if GetOverlappedResult(self.h, &*self.ov, &mut n, 0) == 0 {
                    let e = GetLastError();
                    if e == ERROR_IO_INCOMPLETE {
                        return Read::Empty;
                    }
                    self.pending = false;
                    return Read::Gone(e);
                }
                self.pending = false;
                return Read::Report(self.buf[..n as usize].to_vec());
            }
            ResetEvent(self.ov.hEvent);
            let want = self.info.input_len.max(1) as u32;
            if ReadFile(self.h, self.buf.as_mut_ptr(), want.min(self.buf.len() as u32), &mut n, &mut *self.ov) == 0 {
                let e = GetLastError();
                if e == ERROR_IO_PENDING {
                    self.pending = true;
                    // completed already?
                    if GetOverlappedResult(self.h, &*self.ov, &mut n, 0) != 0 {
                        self.pending = false;
                        return Read::Report(self.buf[..n as usize].to_vec());
                    }
                    return Read::Empty;
                }
                return Read::Gone(e);
            }
            Read::Report(self.buf[..n as usize].to_vec())
        }
    }

    /// Writes one output report (padded to the interface's output length), waiting at most 30 ms. Returns the error,
    /// 0 when written whole.
    pub fn write(&mut self, data: &[u8]) -> u32 {
        if !self.writable {
            return ERROR_ACCESS_DENIED;
        }
        let mut pkt = data.to_vec();
        let ol = self.info.output_len as usize;
        if ol > pkt.len() {
            pkt.resize(ol, 0);
        }
        unsafe {
            let mut ov: OVERLAPPED = std::mem::zeroed();
            ov.hEvent = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if ov.hEvent.is_null() {
                return GetLastError();
            }
            let mut done = 0u32;
            let mut err = 0;
            let mut ok = WriteFile(self.h, pkt.as_ptr(), pkt.len() as u32, &mut done, &mut ov) != 0;
            if !ok {
                err = GetLastError();
                if err == ERROR_IO_PENDING {
                    if WaitForSingleObject(ov.hEvent, 30) != WAIT_OBJECT_0 {
                        CancelIoEx(self.h, &ov);
                        GetOverlappedResult(self.h, &ov, &mut done, 1);
                        err = ERROR_TIMEOUT;
                    } else {
                        ok = GetOverlappedResult(self.h, &ov, &mut done, 0) != 0;
                        err = if ok { 0 } else { GetLastError() };
                    }
                }
            }
            CloseHandle(ov.hEvent);
            if ok && done as usize == pkt.len() {
                0
            } else if err != 0 {
                err
            } else {
                ERROR_WRITE_FAULT
            }
        }
    }

    /// Reads feature report `id` (`len` bytes including the id).
    pub fn feature(&self, id: u8, len: usize) -> Option<Vec<u8>> {
        let mut b = vec![0u8; len.max(2)];
        b[0] = id;
        unsafe { HidD_GetFeature(self.h, b.as_mut_ptr() as *mut c_void, b.len() as u32) }.then_some(b)
    }
}

#[cfg(windows)]
impl Drop for HidDev {
    fn drop(&mut self) {
        unsafe {
            if self.pending {
                let mut n = 0;
                CancelIoEx(self.h, &*self.ov);
                GetOverlappedResult(self.h, &*self.ov, &mut n, 1);
            }
            CloseHandle(self.ov.hEvent);
            CloseHandle(self.h);
        }
    }
}

// ---------------------------------------------------------------- Linux: hidraw

/// What a report descriptor says of its device, as HidP_GetCaps reports it: the top-level collection's usage page and
/// usage, and each report kind's longest report in bytes including the report-id byte (also counted when the
/// device numbers no reports: Windows then prefixes a zero id).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescCaps {
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
}

/// Parses a HID report descriptor (short items; long items are skipped).
pub fn parse_descriptor(d: &[u8]) -> DescCaps {
    use std::collections::HashMap;
    #[derive(Clone, Copy, Default)]
    struct Glob {
        page: u32,
        size: u32,
        count: u32,
        id: u32,
    }
    let mut caps = DescCaps::default();
    let mut g = Glob::default();
    let mut stack: Vec<Glob> = vec![];
    let mut usages: Vec<u32> = vec![];
    let mut depth = 0u32;
    let mut top_found = false;
    // bits per (kind 0 input / 1 output / 2 feature, report id)
    let mut bits: HashMap<(u8, u32), u32> = HashMap::new();
    let mut i = 0usize;
    while i < d.len() {
        let b = d[i];
        if b == 0xFE {
            // long item: size, tag, data
            let n = d.get(i + 1).copied().unwrap_or(0) as usize;
            i += 3 + n;
            continue;
        }
        let size = match b & 3 {
            3 => 4,
            n => n as usize,
        };
        if i + 1 + size > d.len() {
            break;
        }
        let mut v = 0u32;
        for k in 0..size {
            v |= (d[i + 1 + k] as u32) << (8 * k);
        }
        let (kind, tag) = ((b >> 2) & 3, b >> 4);
        match kind {
            0 => {
                match tag {
                    8 | 9 | 11 => {
                        let k = match tag {
                            8 => 0,
                            9 => 1,
                            _ => 2,
                        };
                        *bits.entry((k, g.id)).or_insert(0) += g.size * g.count;
                    }
                    10 => {
                        if depth == 0 && !top_found {
                            let u = usages.first().copied().unwrap_or(0);
                            caps.usage_page = if u > 0xffff { (u >> 16) as u16 } else { g.page as u16 };
                            caps.usage = u as u16;
                            top_found = true;
                        }
                        depth += 1;
                    }
                    12 => depth = depth.saturating_sub(1),
                    _ => {}
                }
                usages.clear();
            }
            1 => match tag {
                0 => g.page = v,
                7 => g.size = v,
                8 => g.id = v,
                9 => g.count = v,
                10 => stack.push(g),
                11 => {
                    if let Some(p) = stack.pop() {
                        g = p;
                    }
                }
                _ => {}
            },
            2 => {
                if tag == 0 {
                    usages.push(if size == 4 { v } else { v & 0xffff });
                }
            }
            _ => {}
        }
        i += 1 + size;
    }
    let len = |k: u8| -> u16 {
        let m = bits.iter().filter(|((kk, _), _)| *kk == k).map(|(_, &b)| b.div_ceil(8)).max();
        m.map_or(0, |m| (m + 1).min(u16::MAX as u32) as u16)
    };
    caps.input_len = len(0);
    caps.output_len = len(1);
    caps.feature_len = len(2);
    caps
}

#[cfg(target_os = "linux")]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Describes /sys/class/hidraw/<node> (the `uevent` of its HID device, its report descriptor).
#[cfg(target_os = "linux")]
fn describe_node(node: &str) -> Option<HidInfo> {
    let sys = std::path::Path::new("/sys/class/hidraw").join(node).join("device");
    let uevent = std::fs::read_to_string(sys.join("uevent")).ok()?;
    let mut info = HidInfo { path: wide(&format!("/dev/{node}")), ..Default::default() };
    let mut bus = 0u32;
    for l in uevent.lines() {
        if let Some(id) = l.strip_prefix("HID_ID=") {
            let p: Vec<&str> = id.split(':').collect();
            if p.len() == 3 {
                bus = u32::from_str_radix(p[0], 16).unwrap_or(0);
                info.vid = u32::from_str_radix(p[1], 16).unwrap_or(0) as u16;
                info.pid = u32::from_str_radix(p[2], 16).unwrap_or(0) as u16;
            }
        } else if let Some(n) = l.strip_prefix("HID_NAME=") {
            info.product = n.to_string();
        }
    }
    info.bluetooth = bus == 0x0005;
    // bcdDevice of the USB device (the parent of the HID device's interface); Bluetooth reports none here
    if let Ok(real) = std::fs::canonicalize(&sys) {
        for up in real.ancestors().skip(1).take(3) {
            if let Ok(v) = std::fs::read_to_string(up.join("bcdDevice")) {
                info.version = u16::from_str_radix(v.trim(), 16).unwrap_or(0);
                break;
            }
        }
    }
    if let Ok(d) = std::fs::read(sys.join("report_descriptor")) {
        let c = parse_descriptor(&d);
        info.usage_page = c.usage_page;
        info.usage = c.usage;
        info.input_len = c.input_len;
        info.output_len = c.output_len;
        info.feature_len = c.feature_len;
    }
    Some(info)
}

/// Every present HID interface (the hidraw nodes), in node order.
#[cfg(target_os = "linux")]
pub fn enumerate() -> Vec<HidInfo> {
    let Ok(rd) = std::fs::read_dir("/sys/class/hidraw") else { return vec![] };
    let mut nodes: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("hidraw")).collect();
    nodes.sort_by_key(|n| n[6..].parse::<u32>().unwrap_or(u32::MAX));
    nodes.iter().filter_map(|n| describe_node(n)).collect()
}

/// An open hidraw node: non-blocking reads, writes bounded to 30 ms.
#[cfg(target_os = "linux")]
pub struct HidDev {
    fd: i32,
    buf: Vec<u8>,
    pub writable: bool,
    pub info: HidInfo,
    /// errno of a refused read+write open
    pub open_error: u32,
}

#[cfg(target_os = "linux")]
fn errno() -> u32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
}

#[cfg(target_os = "linux")]
impl HidDev {
    /// Opens read+write, or read-only when a write open is refused (`writable` false, `open_error` the errno).
    pub fn open(info: &HidInfo, write: bool) -> Option<HidDev> {
        let path = std::ffi::CString::new(info.path_string()).ok()?;
        let mut writable = write;
        let mut err = 0;
        let flags = libc::O_NONBLOCK | libc::O_CLOEXEC;
        // SAFETY: open(2) on a path of ours
        let mut fd = unsafe { libc::open(path.as_ptr(), flags | if write { libc::O_RDWR } else { libc::O_RDONLY }) };
        if fd < 0 && write {
            err = errno();
            // SAFETY: as above
            fd = unsafe { libc::open(path.as_ptr(), flags | libc::O_RDONLY) };
            writable = false;
        }
        if fd < 0 {
            return None;
        }
        let len = (info.input_len as usize).max(64);
        Some(HidDev { fd, buf: vec![0; len], writable, info: info.clone(), open_error: err })
    }

    /// The next queued input report, without blocking.
    pub fn read(&mut self) -> Read {
        // SAFETY: reading into our own buffer
        let n = unsafe { libc::read(self.fd, self.buf.as_mut_ptr() as *mut _, self.buf.len()) };
        if n < 0 {
            let e = errno();
            if e == libc::EAGAIN as u32 || e == libc::EINTR as u32 {
                return Read::Empty;
            }
            return Read::Gone(e);
        }
        if n == 0 {
            return Read::Gone(libc::ENODEV as u32);
        }
        Read::Report(self.buf[..n as usize].to_vec())
    }

    /// Writes one output report (padded to the interface's output length), waiting at most 30 ms. Returns the errno,
    /// 0 when written whole.
    pub fn write(&mut self, data: &[u8]) -> u32 {
        if !self.writable {
            return libc::EACCES as u32;
        }
        let mut pkt = data.to_vec();
        let ol = self.info.output_len as usize;
        if ol > pkt.len() {
            pkt.resize(ol, 0);
        }
        let end = std::time::Instant::now() + std::time::Duration::from_millis(30);
        loop {
            // SAFETY: writing our own buffer
            let n = unsafe { libc::write(self.fd, pkt.as_ptr() as *const _, pkt.len()) };
            if n >= 0 {
                return if n as usize == pkt.len() { 0 } else { libc::EIO as u32 };
            }
            let e = errno();
            if e != libc::EAGAIN as u32 && e != libc::EINTR as u32 {
                return e;
            }
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return libc::ETIMEDOUT as u32;
            }
            let mut p = libc::pollfd { fd: self.fd, events: libc::POLLOUT, revents: 0 };
            // SAFETY: one pollfd of ours
            unsafe { libc::poll(&mut p, 1, left.as_millis().max(1) as i32) };
        }
    }

    /// Reads feature report `id` (`len` bytes including the id): HIDIOCGFEATURE.
    pub fn feature(&self, id: u8, len: usize) -> Option<Vec<u8>> {
        let mut b = vec![0u8; len.max(2)];
        b[0] = id;
        // _IOC(_IOC_READ | _IOC_WRITE, 'H', 0x07, len)
        let req = (3u64 << 30) | ((b.len() as u64 & 0x3fff) << 16) | (0x48 << 8) | 0x07;
        // SAFETY: the kernel writes at most b.len() bytes
        let n = unsafe { libc::ioctl(self.fd, req as _, b.as_mut_ptr()) };
        (n >= 0).then_some(b)
    }
}

#[cfg(target_os = "linux")]
impl Drop for HidDev {
    fn drop(&mut self) {
        // SAFETY: our descriptor
        unsafe { libc::close(self.fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::super::sony;
    use super::*;

    // Descriptors with the report layouts of the real devices (their report ids and sizes), checked against the
    // lengths Windows' HidP_GetCaps gives for them (the numbers sony::transport keys on).
    fn item(out: &mut Vec<u8>, b: u8, v: &[u8]) {
        out.push(b | if v.len() == 4 { 3 } else { v.len() as u8 });
        out.extend_from_slice(v);
    }
    fn report(out: &mut Vec<u8>, main: u8, id: u8, bytes: u8) {
        item(out, 0x84, &[id]); // Report ID
        item(out, 0x74, &[8]); // Report Size 8
        item(out, 0x94, &[bytes]); // Report Count
        item(out, 0x08, &[0x01]); // Usage
        item(out, main, &[0x02]);
    }
    fn gamepad(reports: &[(u8, u8, u8)]) -> Vec<u8> {
        let mut d = vec![];
        item(&mut d, 0x04, &[0x01]); // Usage Page (Generic Desktop)
        item(&mut d, 0x08, &[0x05]); // Usage (Game Pad)
        item(&mut d, 0xA0, &[0x01]); // Collection (Application)
        for &(main, id, n) in reports {
            report(&mut d, main, id, n);
        }
        d.push(0xC0);
        d
    }
    const IN: u8 = 0x80;
    const OUT: u8 = 0x90;
    const FEAT: u8 = 0xB0;

    #[test]
    fn dualsense_usb_and_bluetooth_lengths() {
        // USB: input 0x01 (63 bytes), output 0x02 (47), features 0x05 (40), 0x20 (63)
        let usb = parse_descriptor(&gamepad(&[(IN, 1, 63), (OUT, 2, 47), (FEAT, 5, 40), (FEAT, 0x20, 63)]));
        assert_eq!(usb, DescCaps { usage_page: 1, usage: 5, input_len: 64, output_len: 48, feature_len: 64 });
        assert_eq!(sony::transport(sony::DS5, usb.input_len, usb.output_len), sony::USB);
        // Bluetooth: input 0x01 (9) and 0x31 (77), output 0x31 (77)
        let bt = parse_descriptor(&gamepad(&[(IN, 1, 9), (IN, 0x31, 77), (OUT, 0x31, 77), (FEAT, 5, 40)]));
        assert_eq!((bt.input_len, bt.output_len), (78, 78));
        assert_eq!(sony::transport(sony::DS5, bt.input_len, bt.output_len), sony::BT);
    }

    #[test]
    fn switch_pro_lengths_and_bit_fields() {
        // input 0x21 / 0x81 and output 0x01 / 0x10 / 0x80 (63 bytes each): 64, as Windows reports them
        let mut d = gamepad(&[(IN, 0x21, 63), (IN, 0x81, 63), (OUT, 0x01, 63), (OUT, 0x10, 63), (OUT, 0x80, 63)]);
        // report 0x30 from bit fields: 10 one-bit buttons, 6 padding bits, then 61 bytes
        d.pop();
        item(&mut d, 0x84, &[0x30]);
        item(&mut d, 0x74, &[1]);
        item(&mut d, 0x94, &[10]);
        item(&mut d, 0x80, &[0x02]);
        item(&mut d, 0x74, &[6]);
        item(&mut d, 0x94, &[1]);
        item(&mut d, 0x80, &[0x03]);
        item(&mut d, 0x74, &[8]);
        item(&mut d, 0x94, &[61]);
        item(&mut d, 0x80, &[0x02]);
        d.push(0xC0);
        let c = parse_descriptor(&d);
        assert_eq!(c, DescCaps { usage_page: 1, usage: 5, input_len: 64, output_len: 64, feature_len: 0 });
    }

    #[test]
    fn unnumbered_reports_still_count_the_id_byte_and_push_pop() {
        let mut d = vec![];
        item(&mut d, 0x04, &[0x01]);
        item(&mut d, 0x08, &[0x04]); // Joystick
        item(&mut d, 0xA0, &[0x01]);
        item(&mut d, 0xA4, &[]); // Push
        item(&mut d, 0x74, &[16]);
        item(&mut d, 0x94, &[4]);
        item(&mut d, 0x80, &[0x02]);
        item(&mut d, 0xB4, &[]); // Pop: size and count back to 0
        item(&mut d, 0x80, &[0x02]);
        d.push(0xC0);
        let c = parse_descriptor(&d);
        assert_eq!((c.usage_page, c.usage, c.input_len, c.output_len), (1, 4, 9, 0));
    }

    /// The hidraw nodes this machine has describe without error (none in a plain WSL).
    #[cfg(target_os = "linux")]
    #[test]
    fn enumerate_runs() {
        for i in enumerate() {
            assert!(i.path_string().starts_with("/dev/hidraw"));
        }
    }
}
