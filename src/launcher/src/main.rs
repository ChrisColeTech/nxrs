// The launcher is a GUI program on Windows: no console window of its own.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! `nxrs`: parses the command line and runs the launcher; its exit code is the outcome.

fn main() {
    // run from a terminal, the command-line verbs print there
    nxrs_launcher::services::platform::attach_console();
    nxrs_launcher::services::platform::report_panics();
    let code = match nxrs_launcher::cli::parse_from(std::env::args_os()) {
        Ok(options) => nxrs_launcher::cli::run(options),
        Err(e) => {
            let _ = e.print();
            if e.use_stderr() { 2 } else { 0 }
        }
    };
    std::process::exit(code);
}
