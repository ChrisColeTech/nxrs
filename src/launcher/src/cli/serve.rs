//! `--serve-web <folder>`: the launcher's web server for one web build, without a window, until the
//! process is closed (development and verification; Play in browser uses the same server).

use crate::constants::words_web as w;
use crate::model::fill::fill;
use crate::model::path_text::{absolute, file_name, native};
use crate::services::toolchain::Toolchain;
use crate::services::web_serve::{self, WebServer};

pub fn serve_web(folder: &str) -> i32 {
    let dir = absolute(folder);
    let (build, mut mount) = match web_serve::open_build(&dir) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}{e}", w::SERVE_FAILED);
            return 1;
        }
    };
    // the page's run-time shaders: the nxrs-build found as Play would find it, without Settings
    // (NXRS_BUILD, the release's nxrs-engine\, beside the launcher)
    mount.translator = Toolchain::find("", "").nxrs_build;
    let translator = if mount.translator.is_empty() {
        w::NO_TRANSLATOR.to_string()
    } else {
        fill(w::XLATE_WITH, &[&native(&mount.translator)])
    };
    let server = match WebServer::start(web_serve::wanted_port()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let name = if build.title.is_empty() {
        file_name(&dir)
    } else {
        build.title.clone()
    };
    match server.mount(&name, mount) {
        Ok(url) => println!("{}", fill(w::SERVING, &[&native(&dir), &url])),
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    }
    if let Some(note) = server.moved_note() {
        println!("{note}");
    }
    println!("{translator}");
    loop {
        std::thread::park();
    }
}
