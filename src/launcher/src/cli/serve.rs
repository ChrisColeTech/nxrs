//! `--serve-web <folder>`: the launcher's web server for one web build, without a window, until the
//! process is closed (development and verification; Play in browser uses the same server).

use crate::constants::words_web as w;
use crate::model::fill::fill;
use crate::model::path_text::{absolute, file_name, native};
use crate::services::web_serve::{self, WebServer};

pub fn serve_web(folder: &str) -> i32 {
    let dir = absolute(folder);
    let (build, mount) = match web_serve::open_build(&dir) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}{e}", w::SERVE_FAILED);
            return 1;
        }
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
    loop {
        std::thread::park();
    }
}
