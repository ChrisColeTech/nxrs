//! `nxrs-build doctor`'s answer: one `ok   <check>` / `FAIL <check>` line per check, the rest notes.

use super::fill::fill;
use crate::constants::words_build as w;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DoctorCheck {
    pub ok: bool,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NxrsDoctor {
    pub checks: Vec<DoctorCheck>,
    pub notes: Vec<String>,
    pub exit_code: i32,
    /// nxrs-build did not run, or did not finish
    pub error: String,
}

impl NxrsDoctor {
    pub fn failed_to_run(error: &str) -> NxrsDoctor {
        NxrsDoctor {
            error: error.into(),
            exit_code: -1,
            ..NxrsDoctor::default()
        }
    }
    pub fn ok(&self) -> bool {
        self.error.is_empty() && self.exit_code == 0 && !self.checks.is_empty()
    }
    pub fn failed(&self) -> usize {
        self.checks.iter().filter(|c| !c.ok).count()
    }
    /// One line per check, "✓" or "✗" first, notes as they came.
    pub fn text(&self) -> String {
        let mut lines = Vec::new();
        if !self.error.is_empty() {
            lines.push(self.error.clone());
        }
        for c in &self.checks {
            lines.push(format!(
                "{} {}",
                if c.ok { "\u{2713}" } else { "\u{2717}" },
                c.text
            ));
        }
        lines.extend(self.notes.iter().map(|n| format!("   {n}")));
        lines.join("\n")
    }
    /// Settings > Engine: "All checks passed." or "n check(s) failed.", then the lines.
    pub fn summary(&self) -> String {
        let head = if self.ok() {
            w::DOCTOR_PASSED.to_string()
        } else {
            fill(w::DOCTOR_FAILED, &[&self.failed().max(1).to_string()])
        };
        format!("{head}\n{}", self.text())
    }
}

pub fn parse(output: &str, exit_code: i32) -> NxrsDoctor {
    let mut d = NxrsDoctor {
        exit_code,
        ..NxrsDoctor::default()
    };
    for line in output.split('\n') {
        let line = line.replace('\r', "");
        if line.trim().is_empty() || line.starts_with('@') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("ok ") {
            d.checks.push(DoctorCheck {
                ok: true,
                text: rest.trim().into(),
            });
        } else if let Some(rest) = line.strip_prefix("FAIL ") {
            d.checks.push(DoctorCheck {
                ok: false,
                text: rest.trim().into(),
            });
        } else {
            d.notes.push(line.trim().into());
        }
    }
    if d.checks.is_empty() {
        d.error = w::DOCTOR_NONE.into();
    }
    d
}
