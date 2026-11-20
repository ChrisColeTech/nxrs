//! Each frame's work: the tools check, the doctor, the engine runs' events, the running-games watch.

use super::App;
use crate::constants::timing;
use crate::services::platform;
use std::time::{Duration, Instant};

impl App {
    pub fn tick(&mut self) {
        if let Some(t) = self.tools_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.tools, self.tools_checked, self.tools_rx) = (t, true, None);
            self.refresh_facts();
            // Settings opened before nxrs-build was found: its checks run now
            let settings = self
                .surfaces
                .iter()
                .any(|s| matches!(s, super::surface::Surface::Settings(_)));
            if settings && !self.doctor_running {
                self.run_doctor();
            }
        }
        if let Some(d) = self.doctor_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.doctor_text, self.doctor_running, self.doctor_rx) = (d.summary(), false, None);
        }
        self.drain_build();
        self.drain_probe();
        self.drain_dlc();
        if !self.demo && self.last_poll.elapsed() >= Duration::from_millis(timing::POLL_MS) {
            self.last_poll = Instant::now();
            let detected = platform::find_running_games(&self.lib.games);
            if detected != self.running {
                self.running = detected;
                self.fix_selection();
            }
            self.refresh_facts();
        }
    }

    /// An engine run is going (the UI keeps repainting for its events).
    pub fn working(&self) -> bool {
        self.build.is_some()
            || self.probe.is_some()
            || self.dlc_run.is_some()
            || self.tools_rx.is_some()
            || self.doctor_rx.is_some()
    }

    /// Waits, ticking, until no engine run is left (command-line verbs, tests).
    pub fn wait_idle(&mut self, limit: Duration) -> bool {
        let start = Instant::now();
        while self.working() {
            if start.elapsed() > limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
            self.tick();
        }
        true
    }
}
