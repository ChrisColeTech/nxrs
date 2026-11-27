//! The web build's host and its description (docs/35 W5), written into the output folder by `--target web` after
//! the module and the data description (webpack.rs), so the folder is complete for any static server that sends
//! COOP/COEP (the launcher's, scripts/web/dev_server.py, another):
//!
//! - the host files (`HOST_FILES`: the page and its workers), embedded here from `src/front/web/page`, so a release needs no
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

/// The host files of every web build, as they are in `src/front/web/page` (`link/`'s nxrs-lib.js and nxgfx.lib.js are linked into
/// game.js instead). Later waves extend this list, one line each.
pub const HOST_FILES: &[(&str, &str)] = &[
    ("index.html", include_str!("../../front/web/page/index.html")),
    ("boot.js", include_str!("../../front/web/page/boot.js")),
    ("input.js", include_str!("../../front/web/page/input.js")),
    ("saves.js", include_str!("../../front/web/page/saves.js")),
    ("io-worker.js", include_str!("../../front/web/page/io-worker.js")),
    (
        "audio-worklet.js",
        include_str!("../../front/web/page/audio-worklet.js"),
    ),
    ("gpu.js", include_str!("../../front/web/page/gpu.js")),
    ("gpu-worker.js", include_str!("../../front/web/page/gpu-worker.js")),
    // WEB-W9: the in-game menu, the settings it keeps, the loading screen's style
    ("menu.js", include_str!("../../front/web/page/menu.js")),
    ("menu.css", include_str!("../../front/web/page/menu.css")),
    // WEB-RUMBLE-HID: the DualSense's motors through WebHID
    ("hid-rumble.js", include_str!("../../front/web/page/hid-rumble.js")),
    // WEB-PADBRIDGE: rumble and lights through the launcher's pad bridge
    ("pad-bridge.js", include_str!("../../front/web/page/pad-bridge.js")),
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
        // WEB-W6: the guest functions outside the recorded hot set (websplit.rs), loaded at the first cold call
        (crate::websplit::COLD, "module"),
        (crate::webpack::MANIFEST, "manifest"),
        (crate::webpack::BOOTSET, "bootset"),
        // the names of both modules' functions (they ship stripped), for reading a stack
        (crate::websplit::SYMBOLS, "symbols"),
        (HIND_LICENSE.0, "license"),
    ]);
    // precompressed copies a server sends with Content-Encoding: gzip (webzip.rs)
    v.extend([
        ("game.js.gz", "gzip"),
        ("game.wasm.gz", "gzip"),
        ("game.cold.wasm.gz", "gzip"),
        ("data.json.gz", "gzip"),
    ]);
    v
}

/// The built-in fallback font's license (the engine embeds Hind on the web: os services/misc.rs `fallback_ttf`),
/// written beside the page: the OFL asks that it travel with the font.
pub const HIND_LICENSE: (&str, &str) = (
    "LICENSES/hind-ofl.txt",
    include_str!("../../../LICENSES/hind-ofl.txt"),
);

/// The `build.json` of the folder `outdir` (its files' sizes read from it).
pub fn describe(
    outdir: &Path,
    data: &Path,
    title: &str,
    title_id: Option<u64>,
    version: &str,
) -> serde_json::Value {
    let mut files: Vec<serde_json::Value> = listed()
        .into_iter()
        .filter_map(|(p, role)| {
            let size = std::fs::metadata(outdir.join(p)).ok()?.len();
            Some(serde_json::json!({"path": p, "size": size, "role": role}))
        })
        .collect();
    // WEB-W6b: the shader packs and their index (webshaders.rs), with their gzip copies
    let dir = shaders::pack::DIR;
    let mut packs: Vec<(String, u64)> = std::fs::read_dir(outdir.join(dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?.len())))
        .collect();
    packs.sort();
    files.extend(packs.into_iter().map(|(n, size)| {
        let role = if n.ends_with(".gz") { "gzip" } else { "shaders" };
        serde_json::json!({"path": format!("{dir}/{n}"), "size": size, "role": role})
    }));
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
    let lic = outdir.join(HIND_LICENSE.0);
    if let Some(dir) = lic.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::write_if_changed(&lic, HIND_LICENSE.1.as_bytes())
        .with_context(|| format!("writing {}", lic.display()))?;
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
                "gpu-worker.js",
                "menu.js",
                "menu.css",
                "hid-rumble.js",
                "pad-bridge.js"
            ]
        );
        // what index.html loads and what boot.js and gpu.js start are all here
        let file = |n: &str| HOST_FILES.iter().find(|(f, _)| *f == n).unwrap().1;
        for n in [
            "hid-rumble.js",
            "pad-bridge.js",
            "input.js",
            "saves.js",
            "gpu.js",
            "menu.js",
            "boot.js",
        ] {
            assert!(file("index.html").contains(&format!("src=\"{n}\"")), "{n}");
        }
        assert!(file("index.html").contains("href=\"menu.css\""));
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
        assert!(line.contains("12 files (12 updated)"), "{line}");
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
        // twelve host files, the loader, the module, the manifest, the font's license; no bootset.json, cold module,
        // symbols or compressed copies here, so none listed
        assert_eq!(files.len(), 16);
        assert_eq!(files[15]["role"], "license");
        assert_eq!(
            std::fs::read_to_string(d.join(HIND_LICENSE.0)).unwrap(),
            HIND_LICENSE.1
        );
        assert!(HIND_LICENSE.1.contains("SIL OPEN FONT LICENSE"));
        assert_eq!(files[7]["path"], "gpu-worker.js");
        assert_eq!(files[9]["path"], "menu.css");
        assert_eq!(files[10]["path"], "hid-rumble.js");
        assert_eq!(files[11]["path"], "pad-bridge.js");
        assert_eq!(
            files[12],
            serde_json::json!({"path": "game.js", "size": 2, "role": "loader"})
        );
        assert_eq!(
            files[13],
            serde_json::json!({"path": "game.wasm", "size": 5, "role": "module"})
        );
        assert_eq!(files[14]["role"], "manifest");
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
