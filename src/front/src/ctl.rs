//! The control pipe: `\\.\pipe\nxrs_ctl_<process id>` (and Turbo's name `\\.\pipe\swr_ctl_<process id>`), one
//! command line per connection, the reply text back (Turbo's vk_ctl.c protocol; scripts/play.py is the
//! client). Commands come from the engine-wide registry (`nxconfig::ctl`): the input layer's `press`, `P2`..`P8` and
//! `pads`, graphics' own, and the front end's below.

//!
//! Linux: a Unix domain socket `$XDG_RUNTIME_DIR/nxrs_ctl_<process id>` (and `swr_ctl_<process id>`; /tmp without
//! XDG_RUNTIME_DIR), the same protocol: one command line per connection, the reply, then the socket closes. The
//! socket files go when the run ends (and a stale one of a dead process is replaced).

#[cfg(windows)]
use crate::WideZ;
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
#[cfg(windows)]
use windows_sys::Win32::Foundation::*;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::*;
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::*;

/// The socket path of a control channel name on Linux.
#[cfg(target_os = "linux")]
pub fn socket_path(name: &str) -> std::path::PathBuf {
    // XDG_RUNTIME_DIR when it exists (a session without systemd's user manager may name one that does not)
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| "/tmp".into());
    dir.join(name)
}

/// One command per connection: the line read, the reply written, the connection closed.
fn handle<P: Read + Write>(mut p: P) -> P {
    let mut line = Vec::new();
    let mut b = [0u8; 4096];
    while !line.contains(&b'\n') && line.len() < 65536 {
        match p.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(n) => line.extend_from_slice(&b[..n]),
        }
    }
    let text = String::from_utf8_lossy(&line);
    let cmd = text.lines().next().unwrap_or("").trim().to_string();
    crate::flog!("ctl: {cmd}");
    let reply = nxconfig::ctl::run(&cmd);
    let _ = p.write_all(reply.as_bytes());
    let _ = p.flush();
    p
}

#[cfg(target_os = "linux")]
fn serve(name: String) {
    use std::os::unix::net::UnixListener;
    let path = socket_path(&name);
    let _ = std::fs::remove_file(&path);
    let l = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            crate::flog!("[RED] control socket {}: {e}", path.display());
            return;
        }
    };
    SOCKETS.lock().unwrap().push(path);
    for s in l.incoming() {
        match s {
            Ok(s) => {
                std::thread::spawn(move || {
                    let s = handle(s);
                    let _ = s.shutdown(std::net::Shutdown::Both);
                });
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

/// The socket files this run made (removed at exit).
#[cfg(target_os = "linux")]
static SOCKETS: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Removes the control sockets (the exit path).
pub fn cleanup() {
    #[cfg(target_os = "linux")]
    for p in SOCKETS.lock().unwrap().drain(..) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(windows)]
struct Pipe(HANDLE);
#[cfg(windows)]
unsafe impl Send for Pipe {}
#[cfg(windows)]
impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        if unsafe {
            ReadFile(
                self.0,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut n,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(n as usize)
    }
}
#[cfg(windows)]
impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut n = 0u32;
        if unsafe {
            WriteFile(
                self.0,
                buf.as_ptr(),
                buf.len() as u32,
                &mut n,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(n as usize)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        unsafe { FlushFileBuffers(self.0) };
        Ok(())
    }
}

#[cfg(windows)]
fn serve(name: String) {
    let wname = name.as_str().encode_wide_z();
    loop {
        let h = unsafe {
            CreateNamedPipeW(
                wname.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                0,
                std::ptr::null(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            crate::flog!(
                "[RED] control pipe {name}: CreateNamedPipe failed ({})",
                unsafe { GetLastError() }
            );
            return;
        }
        let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if !ok {
            unsafe { CloseHandle(h) };
            continue;
        }
        let p = Pipe(h);
        std::thread::spawn(move || {
            let p = handle(p);
            unsafe {
                DisconnectNamedPipe(p.0);
                CloseHandle(p.0);
            }
        });
    }
}

fn register() {
    use nxconfig::ctl::register;
    register(
        "status",
        ": frames presented, seconds, FPS limit, recording, window",
        |_| {
            format!(
            "frame {}\nseconds {:.1}\nfps_limit {}\nrecording {}\nfullscreen {}\nfocused {}\nlsfg {}\n",
            nxos::nvn::presents(),
            nxos::log::secs(),
            nxconfig::fps_limit_hz(),
            crate::rec::recording() as u8,
            crate::window::fullscreen() as u8,
            crate::window::focused() as u8,
            crate::lsfg::state()
        )
        },
    );
    register("shot", "[path]: the window as shown, now (PNG)", |a| {
        let p = match a.first() {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                crate::capture::captures_dir().join(format!("ctl_{}.png", nxos::nvn::presents()))
            }
        };
        if crate::capture::screenshot_to(&p) {
            format!("{}", p.display())
        } else {
            "failed".into()
        }
    });
    register(
        "fullres",
        "[path]: the game's picture at its own resolution at the next present (graphics)",
        |a| {
            nxgfx::vk::shot::request(a.first().copied().unwrap_or(""));
            "the next present".into()
        },
    );
    register("get", "<key>: an ini value", |a| match a.first() {
        Some(k) => format!("{k} = {}", nxconfig::string(k)),
        None => "get <key>".into(),
    });
    register(
        "set",
        "<key> <value>: an ini value, kept in the file",
        |a| match (a.first(), a.get(1)) {
            (Some(k), Some(v)) => {
                nxconfig::set(k, v);
                format!("{k} = {}", nxconfig::string(k))
            }
            _ => "set <key> <value>".into(),
        },
    );
    register(
        "run",
        "<key> <value>: an ini value for this run only",
        |a| match (a.first(), a.get(1)) {
            (Some(k), Some(v)) => {
                nxconfig::apply_arg(&format!("{k}={v}"));
                format!("{k} = {}", nxconfig::string(k))
            }
            _ => "run <key> <value>".into(),
        },
    );
    register("record", "start | stop: F7", |a| {
        let want = a.first().copied().unwrap_or("");
        if (want == "start") != crate::rec::recording() {
            crate::rec::toggle();
        }
        format!("recording {}", (want == "start") as u8)
    });
    register("fullscreen", "0 | 1", |a| {
        let on = a.first().is_some_and(|v| *v == "1");
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                crate::window::hwnd(),
                crate::window::WM_APP_FULLSCREEN,
                on as usize,
                0,
            )
        };
        #[cfg(target_os = "linux")]
        crate::window::set_fullscreen(on);
        format!("fullscreen {}", on as u8)
    });
    register(
        "touch",
        "<x> <y> [seconds]: a press on the touch screen (1280x720) held that long (0.1)",
        |a| {
            let (Some(x), Some(y)) = (
                a.first().and_then(|v| v.parse::<u32>().ok()),
                a.get(1).and_then(|v| v.parse::<u32>().ok()),
            ) else {
                return "usage: touch <x> <y> [seconds]\n".into();
            };
            let secs = a
                .get(2)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.1)
                .clamp(0.0, 30.0);
            nxos::input::set_touch(true, x, y);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs_f64(secs));
                nxos::input::set_touch(false, x, y);
            });
            format!("touched {x} {y} for {secs} s\n")
        },
    );
    register("counters", ": every engine counter", |_| {
        nxconfig::metrics::snapshot()
            .iter()
            .map(|(k, v)| format!("{k} {v}\n"))
            .collect()
    });
    register("quit", ": ends the game", |_| {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            crate::quit("the control pipe's quit");
        });
        "quitting".into()
    });
}

pub fn start() {
    register();
    let pid = std::process::id();
    #[cfg(windows)]
    {
        for n in [
            format!(r"\\.\pipe\nxrs_ctl_{pid}"),
            format!(r"\\.\pipe\swr_ctl_{pid}"),
        ] {
            let name = n.clone();
            std::thread::Builder::new()
                .name("control pipe".into())
                .spawn(move || serve(name))
                .ok();
        }
        crate::flog!("control pipe: \\\\.\\pipe\\nxrs_ctl_{pid} (and swr_ctl_{pid})");
    }
    #[cfg(target_os = "linux")]
    {
        for n in [format!("nxrs_ctl_{pid}"), format!("swr_ctl_{pid}")] {
            std::thread::Builder::new()
                .name("control pipe".into())
                .spawn(move || serve(n))
                .ok();
        }
        crate::flog!(
            "control socket: {} (and swr_ctl_{pid})",
            socket_path(&format!("nxrs_ctl_{pid}")).display()
        );
    }
    let _ = Ordering::Relaxed;
}
