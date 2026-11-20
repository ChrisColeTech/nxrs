//! Linux: processes by `/proc/<pid>/exe`, SIGTERM through a pidfd, process groups for builds.

use crate::model::game::Game;
use crate::model::path_text as paths;
use std::collections::BTreeMap;
use std::process::Command;

pub fn process_path(pid: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn find_running_games(games: &[Game]) -> BTreeMap<String, u32> {
    let mut result = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return result;
    };
    for e in entries.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let path = process_path(pid);
        if path.is_empty() {
            continue;
        }
        for g in games {
            if paths::same_path(&path, &g.executable()) && !result.contains_key(&g.id) {
                result.insert(g.id.clone(), pid);
            }
        }
    }
    result
}

pub fn request_close(pid: u32, executable: &str) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let fd = libc::syscall(libc::SYS_pidfd_open, pid as libc::c_long, 0) as i32;
        if fd < 0 {
            return false;
        }
        let path = process_path(pid);
        let sent = !path.is_empty()
            && paths::same_path(&path, executable)
            && libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd,
                libc::SIGTERM,
                std::ptr::null::<u8>(),
                0,
            ) == 0;
        libc::close(fd);
        sent
    }
}

pub fn detach(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

pub fn prepare_tree(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

pub struct Tree {
    pgid: i32,
}
impl Tree {
    pub fn adopt(child: &std::process::Child) -> Tree {
        Tree {
            pgid: child.id() as i32,
        }
    }
    pub fn terminate(&self, grace: std::time::Duration) {
        let pgid = self.pgid;
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        std::thread::spawn(move || {
            std::thread::sleep(grace);
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        });
    }
}
