//! nxgfx-replay: a capture (`NXGFX_CAPTURE`, vk/capture.rs) replayed on this machine's GPU without the game.
//!
//! `nxgfx-replay <capture> --shaders <game data>/shaders [--out <dir>]`: each present is written to
//! `<dir>/frame-<n>.png` (default: the capture's folder). The game's translated shaders are read from `--shaders`.
use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut cap = None;
    let (mut shaders, mut out) = (None, None);
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--shaders" if i + 1 < a.len() => {
                shaders = Some(PathBuf::from(&a[i + 1]));
                i += 1;
            }
            "--out" if i + 1 < a.len() => {
                out = Some(PathBuf::from(&a[i + 1]));
                i += 1;
            }
            s => cap = Some(PathBuf::from(s)),
        }
        i += 1;
    }
    let Some(cap) = cap else {
        eprintln!("usage: nxgfx-replay <capture> --shaders <dir> [--out <dir>]");
        std::process::exit(2);
    };
    nxgfx::log::set_sink(|l| eprintln!("{l}"));
    let out = out.unwrap_or_else(|| cap.parent().map(|p| p.to_path_buf()).unwrap_or_default());
    match nxgfx::vk::capture::replay(&cap, &out, shaders) {
        Ok(n) => println!("{n} frames written to {}", out.display()),
        Err(e) => {
            eprintln!("replay: {e}");
            std::process::exit(1);
        }
    }
}
