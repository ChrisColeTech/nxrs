//! Windows: processes by full image path (Toolhelp + QueryFullProcessImageNameW), WM_CLOSE to a
//! game's windows, a detached start.

use crate::model::game::Game;
use crate::model::path_text as paths;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
};

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

pub fn process_path(pid: u32) -> String {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return String::new();
        }
        let mut buf = vec![0u16; 32768];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(h);
        if ok == 0 {
            return String::new();
        }
        paths::clean(&String::from_utf16_lossy(&buf[..size as usize]))
    }
}

pub fn find_running_games(games: &[Game]) -> BTreeMap<String, u32> {
    let mut result = BTreeMap::new();
    let names: BTreeSet<String> = games
        .iter()
        .map(|g| paths::file_name(&g.executable()).to_lowercase())
        .collect();
    if names.is_empty() {
        return result;
    }
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return result;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            let name = wide_to_string(&entry.szExeFile).to_lowercase();
            if names.contains(&name) {
                let path = process_path(entry.th32ProcessID);
                if !path.is_empty() {
                    for g in games {
                        if path.to_lowercase() == paths::absolute(&g.executable()).to_lowercase()
                            && !result.contains_key(&g.id)
                        {
                            result.insert(g.id.clone(), entry.th32ProcessID);
                        }
                    }
                }
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    result
}

pub fn request_close(pid: u32, executable: &str) -> bool {
    if process_path(pid).to_lowercase() != paths::absolute(executable).to_lowercase() {
        return false;
    }
    struct Request {
        pid: u32,
        sent: bool,
    }
    // SAFETY: `data` is the `Request` passed to EnumWindows below, alive for the whole call.
    unsafe extern "system" fn each(window: HWND, data: LPARAM) -> i32 {
        unsafe {
            let request = &mut *(data as *mut Request);
            let mut id = 0u32;
            GetWindowThreadProcessId(window, &mut id);
            if id == request.pid && IsWindowVisible(window) != 0 {
                PostMessageW(window, WM_CLOSE, 0, 0);
                request.sent = true;
            }
        }
        1
    }
    let mut request = Request { pid, sent: false };
    unsafe {
        EnumWindows(Some(each), &mut request as *mut Request as LPARAM);
    }
    request.sent
}

pub fn detach(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    // as QProcess::startDetached from a GUI program: its own process and process group, no console
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}
