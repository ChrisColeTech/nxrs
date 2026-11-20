//! Windows: an engine run in a job object, so Cancel (and the launcher ending) ends everything it
//! started. The run starts suspended and joins the job before it runs a single instruction: nothing it
//! starts can escape the job (a child started before the join would).

use std::process::Command;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

const CREATE_SUSPENDED: u32 = 0x0000_0004;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// No console, and suspended until it has joined its job.
pub fn prepare_tree(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
}

/// SAFETY: a thread snapshot of the system, and the threads of our own child resumed.
fn resume(pid: u32) {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return;
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snapshot, &mut entry);
        while more != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    ResumeThread(thread);
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
}

pub struct Tree {
    job: HANDLE,
    pid: u32,
}
// SAFETY: a job handle may be used from any thread.
unsafe impl Send for Tree {}
unsafe impl Sync for Tree {}

impl Tree {
    /// The suspended child joins a new job (killed when the job's last handle closes), then runs.
    pub fn adopt(child: &std::process::Child) -> Tree {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: a new job object and our own child's handle.
        let job = unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size,
                );
                AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
            }
            job
        };
        resume(child.id());
        Tree {
            job,
            pid: child.id(),
        }
    }

    /// Ends every process of the job; without a job, the tree by its pid (never by image name).
    pub fn terminate(&self, _grace: std::time::Duration) {
        if !self.job.is_null() {
            // SAFETY: our own job handle.
            unsafe {
                TerminateJobObject(self.job, 1);
            }
            return;
        }
        let mut cmd = Command::new("taskkill.exe");
        cmd.args(["/PID", &self.pid.to_string(), "/T", "/F"]);
        super::no_console(&mut cmd);
        let _ = cmd.status();
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        if !self.job.is_null() {
            // SAFETY: the handle is ours and closed once.
            unsafe {
                CloseHandle(self.job);
            }
        }
    }
}
