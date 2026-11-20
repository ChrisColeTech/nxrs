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

/// The graceful exit's time limits (nxnative hle_lifecycle.c): graphics' drain is waited for at most `DRAIN`, and the
/// whole exit at most `DEADLINE`; past it the process ends all the same.
pub const EXIT_DRAIN: std::time::Duration = std::time::Duration::from_millis(1500);
pub const EXIT_DEADLINE: std::time::Duration = std::time::Duration::from_millis(4000);

/// The graceful exit's steps, in nxnative's order (hle_lifecycle.c): the writers of state stopped (controllers quiet,
/// recording finished), the sound paused, graphics drained beside the rest (waited for at most `drain_for`: a draw
/// compiling its pipeline can take seconds), then what the run keeps written (the pipeline cache, the settings).
/// Returns whether the drain finished in time.
pub fn graceful_exit(stop_writers: &dyn Fn(), pause_audio: &dyn Fn(), drain: Box<dyn FnOnce() + Send>, keep: &dyn Fn(), drain_for: std::time::Duration) -> bool {
    stop_writers();
    pause_audio();
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new().name("exit drain".into()).spawn(move || {
        drain();
        let _ = tx.send(());
    });
    let drained = rx.recv_timeout(drain_for).is_ok();
    keep();
    drained
}

/// Ends the run: controllers quiet, recording finished, the sound paused, graphics drained, settings and caches
/// written, then the process ends without joining the guest threads (Turbo's TerminateProcess); a watchdog ends it
/// all the same if the exit is stuck.
pub fn quit(why: &str) -> ! {
    if !QUIT.swap(true, Ordering::AcqRel) {
        let t0 = std::time::Instant::now();
        flog!("exit: {why}");
        let _ = std::thread::Builder::new().name("exit deadline".into()).spawn(|| {
            std::thread::sleep(EXIT_DEADLINE);
            flog!("[RED] graceful exit not done in {:.1} s, exiting at once", EXIT_DEADLINE.as_secs_f64());
            end_process();
        });
        let drained = graceful_exit(
            &|| {
                nxos::input::shutdown();
                rec::stop();
            },
            &nxos::media::sink::pause_for_exit,
            Box::new(nxgfx::vk::queue::finish),
            &|| {
                nxgfx::save_caches();
                nxconfig::flush();
                nxos::log::report_gaps("exit");
                ctl::cleanup();
            },
            EXIT_DRAIN,
        );
        if !drained {
            flog!("[RED] graceful exit: graphics not drained in {:.1} s (a draw under way); the pipeline cache is written all the same", EXIT_DRAIN.as_secs_f64());
        }
        flog!("Exit. ({:.3} s)", t0.elapsed().as_secs_f64());
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        end_process()
    }
    // another path is ending the process (the first quit, or its deadline): wait for it
    loop {
        std::thread::park();
    }
}

/// The console's Ctrl+C, Ctrl+Break and close (Windows), SIGINT and SIGTERM (Linux): the graceful exit, as the
/// window's close does (nxnative hle_lifecycle.c's SIGTERM; Turbo has no handler and the process just dies).
pub fn install_exit_handlers() {
    #[cfg(windows)]
    unsafe {
        unsafe extern "system" fn on_ctrl(kind: u32) -> windows_sys::core::BOOL {
            let why = match kind {
                0 => "Ctrl+C in the console",
                1 => "Ctrl+Break in the console",
                2 => "the console closed",
                5 => "the user logged off",
                6 => "the system shut down",
                _ => return 0,
            };
            quit(why)
        }
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), 1);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        static SIG: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
        extern "C" fn on_sig(s: libc::c_int) {
            SIG.store(s, Ordering::Release);
        }
        libc::signal(libc::SIGTERM, on_sig as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_sig as *const () as libc::sighandler_t);
        // the exit runs on an ordinary thread, not in the handler
        let _ = std::thread::Builder::new().name("exit signals".into()).spawn(|| loop {
            match SIG.load(Ordering::Acquire) {
                0 => std::thread::sleep(std::time::Duration::from_millis(20)),
                s => quit(if s == libc::SIGTERM { "SIGTERM" } else { "SIGINT" }),
            }
        });
    }
}

fn end_process() -> ! {
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
        // the stuck-mutex healer, each frame (Turbo main.c `kernel_heal_mutexes`)
        if let Some(k) = nxrt::kernel::try_get() {
            k.heal_mutexes();
        }
        // every 15 s, `threads.txt` beside the game: every guest thread's state (Turbo main.c), for a hang
        if tick % 900 == 0 && tick > 0 {
            if let Some(_k) = nxrt::kernel::try_get() {
                let s = format!("second {}, frames {}\n{}", tick / 60, nxos::nvn::presents(), nxrt::kernel::dump_threads());
                let _ = std::fs::write(game().exe_dir.join("threads.txt"), s);
            }
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
        // -v (Turbo main.c: g_verbose=2): everything logged for this run, as log=2
        if a == "-v" && nxconfig::apply_arg("log=2") {
            flog!("settings: -v: log=2 for this run");
        }
    }
    hidden_keys_log();
    let _ = GAME.set(Game { exe_dir, name, title, data });
    flog!("{} - nxrs front end; settings {}", game().title, ini.display());
    settings_log();
    rest
}

/// The buttons the ini's hidden `script` holds now (Turbo's test input; `keys::script_buttons`), from the front end's
/// start.
pub fn script_now() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t0 = *T0.get_or_init(std::time::Instant::now);
    let s = nxconfig::string("script");
    if s.is_empty() {
        return 0;
    }
    keys::script_buttons(&s, t0.elapsed().as_millis() as u64)
}

/// Turbo's hidden ini keys (never written, read when present): `script` (test input), `heap_max_mb` (os), and the
/// three of Turbo's OpenGL renderer, which this Vulkan engine has no use for: said once when set.
fn hidden_keys_log() {
    let _ = script_now();
    if nxconfig::is_set("script") {
        flog!("settings: script={} (test input: buttons held 300 ms at those times)", nxconfig::string("script"));
    }
    for (k, what) in [
        ("loop_limit", "the loop guard of Turbo's OpenGL shader translator"),
        ("show_game_rt", "Turbo's OpenGL render-target view"),
        ("shader_process", "Turbo's OpenGL helper process for driver crashes; pipelines are built on threads here (shader_thread)"),
    ] {
        if nxconfig::is_set(k) {
            flog!("settings: {k}={} has no effect: {what}, and this engine renders with Vulkan only", nxconfig::string(k));
        }
    }
}

/// Opens the window and starts the controllers, the loop, the control pipe and the metrics; returns once the window
/// is up.
pub fn start() {
    nxos::nvn::FRONT_WINDOW.store(true, Ordering::Release);
    window::open(&game().title);
    nxos::input::start();
    install_exit_handlers();
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

#[cfg(test)]
mod exit_tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// nxnative hle_lifecycle.c's order: writers stopped, sound paused, graphics drained, then what the run keeps
    /// written; a drain that hangs is waited for only its time limit and the rest still runs.
    #[test]
    fn graceful_exit_order_and_drain_limit() {
        let log = Arc::new(Mutex::new(Vec::<&str>::new()));
        let (l1, l2, l3, l4) = (log.clone(), log.clone(), log.clone(), log.clone());
        let ok = super::graceful_exit(
            &move || l1.lock().unwrap().push("writers"),
            &move || l2.lock().unwrap().push("audio"),
            Box::new(move || l3.lock().unwrap().push("drain")),
            &move || l4.lock().unwrap().push("keep"),
            Duration::from_secs(5),
        );
        assert!(ok);
        assert_eq!(*log.lock().unwrap(), ["writers", "audio", "drain", "keep"]);
        let t = Instant::now();
        let kept = Arc::new(Mutex::new(false));
        let k = kept.clone();
        let ok = super::graceful_exit(&|| {}, &|| {}, Box::new(|| std::thread::sleep(Duration::from_secs(3))), &move || *k.lock().unwrap() = true, Duration::from_millis(100));
        assert!(!ok && *kept.lock().unwrap(), "a stuck drain: the cache is written all the same");
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}