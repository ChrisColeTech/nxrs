//! The web build's host and its description (docs/35 W5), written into the output folder by `--target web` after
//! the module and the data description (webpack.rs), so the folder is complete for any static server that sends
//! COOP/COEP (the launcher's, src/web/dev_server.py, another):
//!
//! - the host files (`HOST_FILES`: the page and its workers), embedded here from `src/web`, so a release needs no
//!   source tree; written only when they changed;
//! - `build.json`: `{"format", "kind", "title", "titleId", "engine": {"name", "contract", "version"}, "entry",
//!   "data", "files": [{"path", "size", "role"}, ...]}`. `data` names the build's `<name>_data` folder beside it
//!   (never copied: servers map `data/<path>` to it). Roles: `host` (the page and its scripts), `loader`
//!   (game.js), `module` (game.wasm), `manifest` (data.json), `bootset`. Readers ignore roles they do not know:
//!   W4 adds its GPU worker to `HOST_FILES` and its shader packs as `shaders`, W6 its cold module as another
//!   `module`.

use anyhow::{Context, Result};
use std::path::Path;

pub const BUILD_JSON: &str = "build.json";
/// `build.json`'s layout version; a reader refuses a newer one.
pub const FORMAT: u64 = 1;
/// The page a server opens.
pub const ENTRY: &str = "index.html";

/// The host files of every web build, as they are in `src/web` (nxrs-lib.js and nxgfx.lib.js are linked into
/// game.js instead). Later waves extend this list, one line each.
pub const HOST_FILES: &[(&str, &str)] = &[
    ("index.html", include_str!("../../web/index.html")),
    ("boot.js", include_str!("../../web/boot.js")),
    ("input.js", include_str!("../../web/input.js")),
    ("saves.js", include_str!("../../web/saves.js")),
    ("io-worker.js", include_str!("../../web/io-worker.js")),
    (
        "audio-worklet.js",
        include_str!("../../web/audio-worklet.js"),
    ),
    ("gpu.js", include_str!("../../web/gpu.js")),
    ("gpu-worker.js", include_str!("../../web/gpu-worker.js")),
];

/// The engine's version for `build.json`: a release's `engine.json` beside nxrs-build (its commit), else
/// `dev-<crate version>` (a development build has no recorded commit).
fn engine_version() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|e| std::fs::read_to_string(e.with_file_name("engine.json")).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["version"].as_str().map(str::to_string))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| format!("dev-{}", env!("CARGO_PKG_VERSION")))
}

/// The files `build.json` lists, with their roles, in this order; those the folder lacks are left out.
fn listed() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<(&str, &str)> = HOST_FILES.iter().map(|(p, _)| (*p, "host")).collect();
    v.extend([
        ("game.js", "loader"),
        ("game.wasm", "module"),
        (crate::webpack::MANIFEST, "manifest"),
        (crate::webpack::BOOTSET, "bootset"),
    ]);
    v
}

/// The `build.json` of the folder `outdir` (its files' sizes read from it).
pub fn describe(
    outdir: &Path,
    data: &Path,
    title: &str,
    title_id: Option<u64>,
    version: &str,
) -> serde_json::Value {
    let files: Vec<serde_json::Value> = listed()
        .into_iter()
        .filter_map(|(p, role)| {
            let size = std::fs::metadata(outdir.join(p)).ok()?.len();
            Some(serde_json::json!({"path": p, "size": size, "role": role}))
        })
        .collect();
    serde_json::json!({
        "format": FORMAT,
        "kind": "nxrs-web",
        "title": title,
        "titleId": title_id.map(|t| format!("{t:016x}")),
        "engine": {"name": "nxrs", "contract": 2, "version": version},
        "entry": ENTRY,
        "data": data.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "files": files,
    })
}

/// Writes the host files and `build.json` into `outdir`; a line for the build log.
pub fn write(outdir: &Path, data: &Path, title: &str, title_id: Option<u64>) -> Result<String> {
    let mut changed = 0;
    for (name, text) in HOST_FILES {
        let p = outdir.join(name);
        if crate::write_if_changed(&p, text.as_bytes())
            .with_context(|| format!("writing {}", p.display()))?
        {
            changed += 1;
        }
    }
    let d = describe(outdir, data, title, title_id, &engine_version());
    let text = serde_json::to_string_pretty(&d)?;
    std::fs::write(outdir.join(BUILD_JSON), text)
        .with_context(|| format!("writing {BUILD_JSON}"))?;
    Ok(format!(
        "Web host: {} files ({changed} updated), {BUILD_JSON} (data folder {}); serve {} with COOP/COEP and open {ENTRY}",
        HOST_FILES.len(),
        d["data"].as_str().unwrap_or(""),
        outdir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_files_are_the_pages_own() {
        let names: Vec<&str> = HOST_FILES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "index.html",
                "boot.js",
                "input.js",
                "saves.js",
                "io-worker.js",
                "audio-worklet.js",
                "gpu.js",
                "gpu-worker.js"
            ]
        );
        // what index.html loads and what boot.js and gpu.js start are all here
        let file = |n: &str| HOST_FILES.iter().find(|(f, _)| *f == n).unwrap().1;
        for n in ["input.js", "saves.js", "gpu.js", "boot.js"] {
            assert!(file("index.html").contains(&format!("src=\"{n}\"")), "{n}");
        }
        for n in ["io-worker.js", "audio-worklet.js"] {
            assert!(file("boot.js").contains(&format!("'{n}'")), "{n}");
        }
        assert!(file("gpu.js").contains("'gpu-worker.js'"));
        assert!(HOST_FILES.iter().all(|(_, t)| !t.is_empty()));
    }

    #[test]
    fn build_json_describes_the_folder() {
        let d = std::env::temp_dir().join(format!("nxrs-webhost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let data = d.join("X Y_data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(d.join("game.js"), b"js").unwrap();
        std::fs::write(d.join("game.wasm"), b"wasm!").unwrap();
        std::fs::write(d.join("data.json"), b"{}").unwrap();
        let line = write(&d, &data, "X Y", Some(0x0100_4060_145c_0000)).unwrap();
        assert!(line.contains("8 files (8 updated)"), "{line}");
        for (n, t) in HOST_FILES {
            assert_eq!(std::fs::read_to_string(d.join(n)).unwrap(), *t);
        }
        let j: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.join(BUILD_JSON)).unwrap()).unwrap();
        assert_eq!(j["format"], 1);
        assert_eq!(j["kind"], "nxrs-web");
        assert_eq!(j["title"], "X Y");
        assert_eq!(j["titleId"], "01004060145c0000");
        assert_eq!(j["entry"], "index.html");
        assert_eq!(j["data"], "X Y_data");
        assert_eq!(j["engine"]["name"], "nxrs");
        assert_eq!(j["engine"]["contract"], 2);
        assert!(j["engine"]["version"]
            .as_str()
            .is_some_and(|v| !v.is_empty()));
        let files = j["files"].as_array().unwrap();
        // eight host files, the loader, the module, the manifest; no bootset.json here, so none listed
        assert_eq!(files.len(), 11);
        assert_eq!(files[7]["path"], "gpu-worker.js");
        assert_eq!(
            files[8],
            serde_json::json!({"path": "game.js", "size": 2, "role": "loader"})
        );
        assert_eq!(
            files[9],
            serde_json::json!({"path": "game.wasm", "size": 5, "role": "module"})
        );
        assert_eq!(files[10]["role"], "manifest");
        let html = &files[0];
        assert_eq!(html["role"], "host");
        assert_eq!(html["size"], HOST_FILES[0].1.len() as u64);
        // a second write changes no host file
        assert!(write(&d, &data, "X Y", None)
            .unwrap()
            .contains("(0 updated)"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
