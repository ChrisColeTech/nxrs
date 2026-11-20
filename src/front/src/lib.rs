//! nxfront: the front end and the game's entry. It owns the one game window (graphics presents into it), the settings
//! file `<name>.ini`, the hotkeys, the keyboard and mouse as player 1's controller, screenshots, video recording, frame
//! generation (LSFG through the player's own Lossless.dll), the control pipe, the metrics files and the log file.
//! See README.md.
//!
//! The game executable's code DLL calls `nxfront_main(argc, argv, mods, count)` (the runtime's `nxrt_main` contract)
//! instead of `nxos_main`: it opens the ini, applies `--key=value` arguments for this run, opens the window, starts
//! the controllers and the 60 Hz front-end loop, then boots the game through the os layer.

#![allow(clippy::missing_safety_doc)]

pub mod capture;
pub mod ctl;
pub mod help;
pub mod keys;
pub mod lsfg;
pub mod metrics;
pub mod rec;
#[cfg(windows)]
pub mod window;
#[cfg(target_os = "linux")]
#[path = "window_linux.rs"]
pub mod window;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;

pub struct Game {
    pub exe_dir: PathBuf,
    pub name: String,
    pub title: String,
    pub data: PathBuf,
}

static GAME: OnceLock<Game> = OnceLock::new();
pub fn game() -> &'static Game {
    GAME.get().expect("front: not started")
}

static QUIT: AtomicBool = AtomicBool::new(false);
pub static FRAME_SKIP: AtomicU32 = AtomicU32::new(0);

/// `[front t] ...` on stderr (the run's log).
pub fn line(msg: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr().lock(), "[front {:8.3}] {msg}", nxos::log::secs());
}
#[macro_export]
macro_rules! flog {
    ($($a:tt)*) => { $crate::line(&format!($($a)*)) };
}

/// The game's display title: control.nacp's first non-empty name, else the executable's name.
fn title_of(data: &Path, name: &str) -> String {
    if let Ok(n) = std::fs::read(data.join("control.nacp")) {
        for i in 0..16 {
            let o = i * 0x300;
            if let Some(b) = n.get(o..o + 0x200) {
                let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
                if end > 0 {
                    return String::from_utf8_lossy(&b[..end]).into_owned();
                }
            }
        }
    }
    name.to_string()
}

/// A GUI-subsystem process started without redirected output has no stdout / stderr: the log goes to
/// `<name>.log` beside the executable (Turbo's place), so a player's run always leaves one.
#[cfg(target_os = "linux")]
fn log_to_file_when_detached(dir: &Path, name: &str) {
    // Linux: started from a desktop entry or a file manager, stderr is /dev/null (or closed): the log goes to the file
    // then. A terminal, a pipe or a file keeps it.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        let open = libc::fstat(2, &mut st) == 0;
        let devnull = open && (st.st_mode & libc::S_IFMT) == libc::S_IFCHR && libc::isatty(2) == 0;
        if open && !devnull {
            return;
        }
        let p = std::ffi::CString::new(dir.join(format!("{name}.log")).as_os_str().as_encoded_bytes()).unwrap_or_default();
        let fd = libc::open(p.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC, 0o644);
        if fd >= 0 {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
            libc::close(fd);
        }
    }
}

#[cfg(windows)]
fn log_to_file_when_detached(dir: &Path, name: &str) {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::Console::*;
    unsafe {
        let e = GetStdHandle(STD_ERROR_HANDLE);
        if !e.is_null() && e != INVALID_HANDLE_VALUE && GetFileType(e) != FILE_TYPE_UNKNOWN {
            return;
        }
        let p: Vec<u16> = dir.join(format!("{name}.log")).as_os_str().encode_wide_z();
        let h = CreateFileW(p.as_ptr(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut());
        if h != INVALID_HANDLE_VALUE {
            SetStdHandle(STD_OUTPUT_HANDLE, h);
            SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
}

pub trait WideZ {
    fn encode_wide_z(&self) -> Vec<u16>;
}
impl WideZ for std::ffi::OsStr {
    #[cfg(windows)]
    fn encode_wide_z(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
    #[cfg(not(windows))]
    fn encode_wide_z(&self) -> Vec<u16> {
        self.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect()
    }
}
impl WideZ for str {
    fn encode_wide_z(&self) -> Vec<u16> {
        self.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

/// Ends the run: controllers quiet, settings and caches written, recording finished, then the process ends without
/// joining the guest threads (Turbo's TerminateProcess).
pub fn quit(why: &str) -> ! {
    if !QUIT.swap(true, Ordering::AcqRel) {
        flog!("exit: {why}");
        nxos::input::shutdown();
        rec::stop();
        nxconfig::flush();
        nxgfx::save_caches();
        nxos::log::report_gaps("exit");
        ctl::cleanup();
        flog!("Exit.");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    }
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Threading::TerminateProcess(windows_sys::Win32::System::Threading::GetCurrentProcess(), 0);
    }
    // Linux: _exit ends the process at once, without joining the guest threads or running exit handlers (the drivers'
    // atexit work under a live device can hang)
    #[cfg(target_os = "linux")]
    unsafe {
        libc::_exit(0);
    }
    #[allow(unreachable_code)]
    std::process::exit(0)
}

pub fn quitting() -> bool {
    QUIT.load(Ordering::Acquire)
}

/// The 60 Hz front-end loop: input focus, the mouse stick, the ini (once a second), recordings, metrics, timed exits.
fn front_loop() {
    let period = std::time::Duration::from_nanos(16_666_667);
    let mut next = std::time::Instant::now();
    let mut tick: u64 = 0;
    let seconds = std::env::var("NXRS_SECONDS").ok().and_then(|v| v.parse::<f64>().ok()).or_else(|| nxconfig::string("seconds").parse().ok()).filter(|s| *s > 0.0);
    let t0 = std::time::Instant::now();
    loop {
        window::poll_input();
        rec::tick();
        if tick % 60 == 0 {
            if nxconfig::poll() {
                flog!("settings: {} reloaded", nxconfig::path().map(|p| p.display().to_string()).unwrap_or_default());
            }
            metrics::tick();
        }
        // diag=1: the diagnostic dump (F5) by itself at 20 s and 40 s (Turbo's automatic dumps)
        if (tick == 60 * 20 || tick == 60 * 40) && nxconfig::flag("diag") {
            window::diag_dump();
        }
        if let Some(s) = seconds {
            if t0.elapsed().as_secs_f64() >= s {
                let shot = nxconfig::string("shot");
                if !shot.is_empty() {
                    capture::screenshot_to(Path::new(&shot));
                }
                flog!("frames={} after {s} s (seconds=)", nxos::nvn::presents());
                quit("the timed exit (seconds=)");
            }
        }
        tick += 1;
        next += period;
        let now = std::time::Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > std::time::Duration::from_millis(100) {
            next = now;
        }
    }
}

fn settings_log() {
    let (on, mult, target) = nxconfig::lsfg();
    flog!(
        "settings: docked={} language={} fullscreen={} resolution={} dynamic_resolution={} (min {}) game_dynamic_resolution={} shadows={} fps_target={} (frame skip {}) fps_limit={} ({} Hz) forced_vsync={} memory_gb={} driver_exclusive={} shader_thread={} low_priority_shader_compiler={} lsfg={} x{} target {} button_layout={} mouse_right_stick={} log={} diag={} profile={} metrics={} null_trap={}",
        nxconfig::int("docked"),
        nxconfig::int("language"),
        nxconfig::int("fullscreen"),
        nxconfig::f32("resolution"),
        nxconfig::int("dynamic_resolution"),
        nxconfig::f32("dynamic_resolution_min"),
        nxconfig::string("game_dynamic_resolution"),
        nxconfig::int("shadows"),
        nxconfig::int("fps_target"),
        nxconfig::frame_skip(),
        nxconfig::string("fps_limit"),
        nxconfig::fps_limit_hz(),
        nxconfig::int("forced_vsync"),
        nxconfig::int("memory_gb"),
        nxconfig::string("driver_exclusive"),
        nxconfig::int("shader_thread"),
        nxconfig::int("low_priority_shader_compiler"),
        on as u8,
        mult,
        target,
        nxconfig::string("button_layout"),
        nxconfig::int("mouse_right_stick"),
        nxconfig::int("log"),
        nxconfig::int("diag"),
        nxconfig::int("profile"),
        nxconfig::int("metrics"),
        nxconfig::int("null_trap"),
    );
}

/// Sets up the front end for `exe` (its folder holds `<name>.ini`, `<name>_data`). Returns the arguments left for
/// the os layer (the `--key=value` ones applied and removed). Usable without booting a game (tests, nxfront-dev).
pub fn setup(args: &[String]) -> Vec<String> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(args.first().cloned().unwrap_or_default()));
    let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = std::env::var("NXRS_NAME").unwrap_or_else(|_| exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "game".into()));
    log_to_file_when_detached(&exe_dir, &name);
    let mut rest = vec![args.first().cloned().unwrap_or_default()];
    let mut data: Option<PathBuf> = None;
    for a in args.iter().skip(1) {
        if a.starts_with("--") && a.contains('=') {
            continue; // applied after the ini is read
        }
        if data.is_none() && !a.starts_with('-') {
            data = Some(PathBuf::from(a));
        }
        rest.push(a.clone());
    }
    let data = data
        .or_else(|| std::env::var("NXOS_DATA").ok().map(PathBuf::from))
        .or_else(|| std::env::var("NXRS_DATA").ok().map(PathBuf::from))
        .unwrap_or_else(|| exe_dir.join(format!("{name}_data")));
    if rest.len() == 1 {
        rest.push(data.display().to_string());
    }
    let title = title_of(&data, &name);
    let ini = std::env::var("NXRS_INI").map(PathBuf::from).unwrap_or_else(|_| exe_dir.join(format!("{name}.ini")));
    if let Err(e) = nxconfig::open(&ini, &title) {
        flog!("[RED] settings: cannot write {}: {e}", ini.display());
    }
    for a in args.iter().skip(1) {
        if a.starts_with("--") && a.contains('=') && nxconfig::apply_arg(a) {
            flog!("settings: {a} for this run");
        }
    }
    let _ = GAME.set(Game { exe_dir, name, title, data });
    flog!("{} - nxrs front end; settings {}", game().title, ini.display());
    settings_log();
    rest
}

/// Opens the window and starts the controllers, the loop, the control pipe and the metrics; returns once the window
/// is up.
pub fn start() {
    nxos::nvn::FRONT_WINDOW.store(true, Ordering::Release);
    window::open(&game().title);
    nxos::input::start();
    ctl::start();
    metrics::start();
    lsfg::start();
    nxgfx::vk::framegen::set_lsfg_state(lsfg::state);
    std::thread::Builder::new().name("front".into()).spawn(front_loop).expect("front thread");
}

/// `int nxfront_main(int argc, char **argv, const RtModule *mods, int count)`: the game's entry.
#[no_mangle]
pub unsafe extern "C" fn nxfront_main(argc: i32, argv: *const *const core::ffi::c_char, mods: *const nxrt::dispatch::RtModule, count: i32) -> i32 {
    // the guest map's fixed regions first, before the window and the GPU driver take address space
    nxrt::process::claim_fixed_regions(mods, count);
    let args: Vec<String> = (0..argc as usize).map(|i| std::ffi::CStr::from_ptr(*argv.add(i)).to_string_lossy().into_owned()).collect();
    let rest = setup(&args);
    start();
    let cs: Vec<std::ffi::CString> = rest.iter().map(|s| std::ffi::CString::new(s.as_str()).unwrap_or_default()).collect();
    let ptrs: Vec<*const core::ffi::c_char> = cs.iter().map(|c| c.as_ptr()).collect();
    nxos::boot::nxos_main_with(ptrs.len() as i32, ptrs.as_ptr(), mods, count, None)
}
