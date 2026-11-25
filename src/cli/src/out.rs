//! Console output as the launcher reads it: one line at a time, flushed, CRLF on Windows (text mode; the
//! launcher trims either). With `--machine` (contract v2) the progress and the result are
//! also given as tab-separated `@progress` / `@result` lines.

use std::io::Write;
use std::sync::Mutex;

#[derive(Default)]
pub struct Out {
    /// `-q`: the build's messages are left out (UPDATE lines and DLC listings are not: they are the answer)
    pub quiet: bool,
    pub machine: bool,
    lock: Mutex<()>,
}

pub fn line(m: &str) {
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    let mut o = std::io::stdout().lock();
    let _ = write!(o, "{m}{nl}");
    let _ = o.flush();
}

impl Out {
    pub fn new(quiet: bool, machine: bool) -> Out {
        Out {
            quiet,
            machine,
            lock: Mutex::new(()),
        }
    }

    pub fn say(&self, m: &str) {
        if !self.quiet {
            let _g = self.lock.lock().unwrap();
            line(m);
        }
    }

    /// Printed even with `-q`.
    pub fn print(&self, m: &str) {
        let _g = self.lock.lock().unwrap();
        line(m);
    }

    pub fn progress(&self, stage: &str, done: usize, total: usize) {
        if self.machine {
            let _g = self.lock.lock().unwrap();
            line(&format!("@progress\t{stage}\t{done}\t{total}"));
        }
    }

    pub fn result(&self, ok: bool, detail: &str) {
        if self.machine {
            let _g = self.lock.lock().unwrap();
            line(&format!(
                "@result\t{}\t{detail}",
                if ok { "ok" } else { "error" }
            ));
        }
    }
}
