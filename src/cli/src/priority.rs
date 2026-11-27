//! nxrs-build runs below normal priority (WEB-WFREEZE): a build is batch work beside the desktop and the games the
//! player runs, so the process lowers itself before it does anything, and every compiler, Python, node and wasm tool
//! it starts inherits that (a Windows child takes its parent's below-normal class; a Linux child its nice value).
//! The work is the same and as parallel on an idle machine; under load the interactive programs go first.

/// Windows `BELOW_NORMAL_PRIORITY_CLASS`; Linux nice 5 (as the shader translator's children, src/shaders xlate.rs).
pub fn lower() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
        };
        // SAFETY: the pseudo-handle of this process; failure leaves the priority as it was
        unsafe {
            SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
        }
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: plain system call on this process (called before any thread is started, so all inherit it)
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 5);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
    };

    /// The build lowers itself, and a child it starts (as emcc's Python, clang, node) runs below normal too.
    #[test]
    fn the_build_and_its_children_run_below_normal() {
        use std::os::windows::io::AsRawHandle;
        super::lower();
        // SAFETY: this process's pseudo-handle
        assert_eq!(
            unsafe { GetPriorityClass(GetCurrentProcess()) },
            BELOW_NORMAL_PRIORITY_CLASS
        );
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "ping", "-n", "2", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        // SAFETY: our own child's process handle, still open
        let class = unsafe { GetPriorityClass(child.as_raw_handle() as _) };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(class, BELOW_NORMAL_PRIORITY_CLASS);
    }
}
