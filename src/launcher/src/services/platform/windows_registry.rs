//! Reading one text value from the registry (the default browser, Chrome's App Paths entry).

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The text of `value` ("" = the key's default value) under `key` of HKLM (`machine`) or HKCU.
pub fn text(machine: bool, key: &str, value: &str) -> Option<String> {
    let hive: HKEY = if machine {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };
    let (k, v) = (wide(key), wide(value));
    let name = if value.is_empty() {
        std::ptr::null()
    } else {
        v.as_ptr()
    };
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut bytes: u32 = 0;
    // SAFETY: NUL-terminated key and value names that outlive the call; a null buffer asks for the size.
    let status = unsafe {
        RegGetValueW(
            hive,
            k.as_ptr(),
            name,
            flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if status != 0 || bytes < 2 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    // SAFETY: `buf` holds `bytes` bytes, the size the first call returned, and outlives the call.
    let status = unsafe {
        RegGetValueW(
            hive,
            k.as_ptr(),
            name,
            flags,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}
