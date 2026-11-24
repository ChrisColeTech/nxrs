//! nxgfx-replay: a capture (`NXGFX_CAPTURE`, vk/capture.rs) replayed on this machine's GPU without the game.
//!
//! `nxgfx-replay <capture> --shaders <game data>/shaders [--out <dir>]`: each present is written to
//! `<dir>/frame-<n>.png` (default: the capture's folder). The game's translated shaders are read from `--shaders`.
//! `nxgfx-replay <capture> --summary`: per present, its submissions and NVN record counts by op.
//! `nxgfx-replay <capture> --dump-shader <key> --out <dir>`: extract captured SPIR-V and reflection without replay.
use std::path::PathBuf;

fn main() {
    nxrt::fault::install();
    let a: Vec<String> = std::env::args().collect();
    let mut cap = None;
    let mut summary = false;
    let mut dump_shader = None;
    let (mut shaders, mut out) = (None, None);
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "--shaders" if i + 1 < a.len() => {
                shaders = Some(PathBuf::from(&a[i + 1]));
                i += 1;
            }
            "--summary" => summary = true,
            "--dump-shader" if i + 1 < a.len() => {
                dump_shader = Some(a[i + 1].clone());
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
        eprintln!("usage: nxgfx-replay <capture> [--shaders <dir>] [--out <dir>] [--summary | --dump-shader <16-hex-key>]");
        std::process::exit(2);
    };
    nxgfx::log::set_sink(|l| eprintln!("{l}"));
    if summary {
        match nxgfx::vk::capture::summary(&cap) {
            Ok(v) => v.iter().for_each(|l| println!("{l}")),
            Err(e) => {
                eprintln!("summary: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let out = out.unwrap_or_else(|| cap.parent().map(|p| p.to_path_buf()).unwrap_or_default());
    if let Some(key) = dump_shader {
        match nxgfx::vk::capture::dump_shader(&cap, &key, &out) {
            Ok(files) => files.iter().for_each(|f| println!("{}", f.display())),
            Err(e) => {
                eprintln!("dump shader: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    match nxgfx::vk::capture::replay(&cap, &out, shaders) {
        Ok(n) => println!("{n} frames written to {}", out.display()),
        Err(e) => {
            eprintln!("replay: {e}");
            std::process::exit(1);
        }
    }
}
