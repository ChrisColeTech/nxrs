//! The operating system's own calls: which games are running, asking one to close, starting one
//! detached, stopping a build with everything it started, and (Windows) the title bar's hit testing.
//! The only `unsafe` code of the launcher is here: Win32 and libc calls that have no safe wrapper.

use crate::model::game::Game;
use std::collections::BTreeMap;
use std::process::Command;

#[cfg(windows)]
pub mod windows_caption;
#[cfg(windows)]
mod windows_process;
#[cfg(windows)]
mod windows_registry;
#[cfg(windows)]
use windows_process as sys;
#[cfg(windows)]
mod windows_tree;
#[cfg(windows)]
use windows_tree as tree;
#[cfg(unix)]
mod linux_process;
#[cfg(unix)]
use linux_process as sys;
#[cfg(unix)]
use linux_process as tree;

/// A GUI program started from a terminal writes to that terminal (when its output is not already
/// redirected): the command-line verbs' lines.
pub fn attach_console() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE,
        };
        // SAFETY: reads this process's own standard handle and attaches to its parent's console.
        unsafe {
            if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }
}

/// Says why the launcher stopped: on stderr, or in a message box when nothing reads stderr (a
/// double-clicked GUI program).
pub fn fatal(text: &str) {
    super::startup_log::note(&format!("fatal: {text}"));
    eprintln!("nxrs: {text}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        // SAFETY: reads this process's own standard handle. Someone reads stderr (a terminal, a test,
        // CI): no box to block them.
        if !unsafe { GetStdHandle(STD_ERROR_HANDLE) }.is_null() {
            return;
        }
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (body, title) = (wide(text), wide("nxrs"));
        // SAFETY: two NUL-terminated UTF-16 buffers that outlive the call; no owner window.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

/// Every panic says where and why before the launcher ends, instead of the window just vanishing.
pub fn report_panics() {
    std::panic::set_hook(Box::new(|info| {
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let at = info
            .location()
            .map(|l| format!(" ({}:{})", l.file(), l.line()))
            .unwrap_or_default();
        fatal(&format!("The launcher stopped: {what}{at}"));
    }));
}

/// No console window for a child of this GUI program (Windows).
pub fn no_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// game id -> pid of every game whose executable runs (by full path: games started elsewhere count).
pub fn find_running_games(games: &[Game]) -> BTreeMap<String, u32> {
    sys::find_running_games(games)
}

/// Re-checks that `pid` runs `executable`, then asks it to close (WM_CLOSE to its visible windows;
/// SIGTERM on Linux). Never forces. False when nothing was asked.
pub fn request_close(pid: u32, executable: &str) -> bool {
    sys::request_close(pid, executable)
}

/// Starts a game so it outlives the launcher, in its build folder. Its pid.
pub fn launch_detached(
    executable: &str,
    cwd: &str,
    env: &[(String, String)],
) -> Result<u32, String> {
    let mut cmd = Command::new(executable);
    cmd.current_dir(cwd).envs(env.iter().map(|(k, v)| (k, v)));
    sys::detach(&mut cmd);
    cmd.spawn().map(|c| c.id()).map_err(|e| e.to_string())
}

/// A registry text value, HKLM (`machine`) or HKCU (Windows); None elsewhere or when missing.
pub fn registry_text(machine: bool, key: &str, value: &str) -> Option<String> {
    #[cfg(windows)]
    return windows_registry::text(machine, key, value);
    #[cfg(not(windows))]
    {
        let _ = (machine, key, value);
        None
    }
}

/// Before spawn: no console, its own process group (Linux).
pub fn prepare_tree(cmd: &mut Command) {
    tree::prepare_tree(cmd);
}

/// A started run that can be stopped with everything it started: a job object (Windows), a process
/// group (Linux).
pub struct ProcessTree(tree::Tree);
impl ProcessTree {
    pub fn adopt(child: &std::process::Child) -> ProcessTree {
        ProcessTree(tree::Tree::adopt(child))
    }
    pub fn terminate(&self, grace: std::time::Duration) {
        self.0.terminate(grace)
    }
}
