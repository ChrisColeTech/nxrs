//! The verbs that run without a window: they print their outcome (`DLC: …`, `Updates: …`, `Saves: …`)
//! and exit 0 or 1 (2 for refused arguments).

use super::{Options, startup};
use crate::constants::timing;
use crate::flows::{App, Outcome};
use std::time::Duration;

fn lines(prefix: &str, ok: bool, detail: &str) {
    for line in detail.split('\n').filter(|l| !l.is_empty()) {
        if ok {
            println!("{prefix}: {line}");
        } else {
            eprintln!("{prefix} error: {line}");
        }
    }
}

/// Waits for the outcome the verb produces; its exit code.
fn outcome(app: &mut App) -> i32 {
    let (limit, start) = (
        Duration::from_secs(timing::HEADLESS_LIMIT_S),
        std::time::Instant::now(),
    );
    loop {
        // a build's outcome comes after its DLC sync, when it has one
        if let Some(o) = app.outcomes.first().cloned() {
            let ok = match &o {
                Outcome::Smoke(ok) => *ok,
                Outcome::Dlc(ok, _) | Outcome::Update(ok, _) | Outcome::SaveCopy(ok, _) => *ok,
            };
            let prefix = match &o {
                Outcome::Dlc(..) => "DLC",
                Outcome::Update(..) => "Updates",
                Outcome::SaveCopy(..) => "Saves",
                Outcome::Smoke(_) => "",
            };
            if let Outcome::Dlc(_, d) | Outcome::Update(_, d) | Outcome::SaveCopy(_, d) = &o {
                lines(prefix, ok, d);
            }
            return i32::from(!ok);
        }
        if !app.working() || start.elapsed() > limit {
            return 1;
        }
        app.wait_idle(Duration::from_millis(50));
    }
}

pub fn run(app: &mut App, o: &Options) -> i32 {
    app.refresh_tools_now();
    if let Err(code) = startup::apply(app, o) {
        return code;
    }
    if o.nxrs_doctor {
        let d = crate::services::run::doctor(&app.tools);
        println!("{}", d.text());
        return i32::from(!d.ok());
    }
    if let Some(input) = &o.smoke_build {
        let output = o.build_output.clone().unwrap_or_default();
        if !app.import_source(input, false, &output) {
            return 2;
        }
        app.start_build();
    } else if !o.remove_dlc.is_empty() {
        app.remove_dlc(&o.remove_dlc);
    } else if !o.install_dlc.is_empty() {
        app.install_dlc_packages(&o.install_dlc, false);
    } else if !o.remove_update.is_empty() {
        app.remove_updates(&o.remove_update);
    } else if let Some(choice) = &o.update_choice {
        app.choose_update(choice);
    } else if !o.install_update.is_empty() {
        app.install_update_packages(&o.install_update, false);
    } else if o.copy_saves {
        app.copy_saves(false, false);
    } else if o.build {
        app.start_build();
    }
    outcome(app)
}

/// `--check-image`: the decoder's answer; 0 when it decodes.
pub fn check_image(path: &str) -> i32 {
    match crate::flows::decode_artwork(path) {
        Ok(img) => {
            println!("Artwork decoded: {} x {}", img.width(), img.height());
            0
        }
        Err(e) => {
            eprintln!("Artwork decode failed: {e}");
            1
        }
    }
}
