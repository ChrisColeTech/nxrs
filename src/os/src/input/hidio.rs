//! Raw HID devices on Windows: the interfaces SetupAPI lists, opened read+write with full sharing (as SDL and hidapi
//! do: denying FILE_SHARE_WRITE fails with error 32 whenever another handle on the pad can write, which on the owner's
//! PC is always), overlapped reads drained without blocking, and bounded writes.

use std::ffi::c_void;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};
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
pub struct HidDev {
    h: HANDLE,
    ov: Box<OVERLAPPED>,
    buf: Vec<u8>,
    pending: bool,
    pub writable: bool,
    pub info: HidInfo,
    pub open_error: u32,
}

unsafe impl Send for HidDev {}

pub enum Read {
    /// A whole report.
    Report(Vec<u8>),
    /// Nothing queued now.
    Empty,
    /// The device is gone or failed (the Win32 error).
    Gone(u32),
}

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
