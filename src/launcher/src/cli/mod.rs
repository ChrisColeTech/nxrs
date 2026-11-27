//! The command line: the Qt launcher's flags with the same names, values and exit codes. A verb that
//! needs no window (`--build`, `--smoke-build`, the DLC and update options,
//! `--nxrs-doctor`, `--check-image`) runs headless; everything else opens the window.
mod flags;
mod headless;
pub mod options;
mod serve;
mod startup;
mod window;

pub use options::{Options, parse_from};

use crate::flows::App;
use crate::services::{data_dir, legacy};

/// Runs the launcher as the options say; its exit code.
pub fn run(o: Options) -> i32 {
    // serving a folder reads no library
    if let Some(folder) = &o.serve_web {
        return serve::serve_web(folder);
    }
    // a library of its own (--library: tests, a portable library) leaves the data folder alone
    let note = crate::services::startup_log::note;
    if o.library.is_none() {
        note(&format!("data folder {}", data_dir::data_dir()));
        legacy::migrate(&data_dir::data_dir());
        note("migrated");
    }
    if let Some(image) = &o.check_image {
        return headless::check_image(image);
    }
    let path = o.library.clone().unwrap_or_else(data_dir::library_path);
    if o.smoke_build.is_some() && (o.build_output.is_none() || o.library.is_none()) {
        return 2;
    }
    if o.headless() {
        let mut app = App::new(&path, o.demo, crate::services::jobs::noop());
        app.interactive = false;
        app.open();
        return headless::run(&mut app, &o);
    }
    window::run(&path, &o)
}
