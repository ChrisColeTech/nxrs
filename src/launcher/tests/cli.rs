//! Every command-line flag with its exit code, against a temporary library and data folder and the fake
//! engine; the two cases of `tests/test_launcher_workflow.py` ported (import twice, a smoke build).

mod common;

use common::*;
use serde_json::json;

#[test]
fn help_version_and_bad_flags() {
    let (_t, dir) = tmp();
    let o = nxrs(&dir, &["--version"]);
    assert!(
        o.status.success()
            && text(&o).contains(env!("CARGO_PKG_VERSION").split('.').next().unwrap())
    );
    assert!(nxrs(&dir, &["--help"]).status.success());
    assert_eq!(nxrs(&dir, &["--no-such-flag"]).status.code(), Some(2));
    // --smoke-build needs --build-output and --library
    assert_eq!(
        nxrs(&dir, &["--smoke-build", "x.nup"]).status.code(),
        Some(2)
    );
}

#[test]
fn smoke_build_saves_a_ready_entry() {
    let (_t, dir) = tmp();
    let source = format!("{dir}/Test game [v0] & input");
    write(&format!("{source}/exefs/main"), b"NSO0");
    let (lib, output) = (
        format!("{dir}/library.json"),
        format!("{dir}/Build output [test]"),
    );
    write(
        &lib,
        json!({"schema": 1, "games": [], "nxrsBuild": FAKE})
            .to_string()
            .as_bytes(),
    );
    let o = nxrs(
        &dir,
        &[
            "--library",
            &lib,
            "--smoke-build",
            &source,
            "--build-output",
            &output,
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let saved = library(&lib);
    let game = &saved["games"][0];
    assert_eq!(game["buildIncomplete"], false);
    assert_eq!(game["builtSource"], game["source"]);
    let base = format!("{output}/{}", game["outputName"].as_str().unwrap());
    assert!(std::path::Path::new(&format!("{base}{}", std::env::consts::EXE_SUFFIX)).is_file());
    assert!(read(&format!("{base}.ini")).contains("renderer=vulkan"));
    // a source that is not one
    let bad = format!("{dir}/notes.txt");
    write(&bad, b"hello");
    assert_eq!(
        nxrs(
            &dir,
            &[
                "--library",
                &lib,
                "--smoke-build",
                &bad,
                "--build-output",
                &output
            ]
        )
        .status
        .code(),
        Some(2)
    );
}

/// Needs a display (CI: xvfb on Linux): the window opens, imports, takes its picture and exits.
#[test]
fn import_existing_build_without_compiling_and_screenshot() {
    let (_t, dir) = tmp();
    let exe = format!("{dir}/Mario Kart World 180{}", std::env::consts::EXE_SUFFIX);
    write(&exe, b"fixture, never execute");
    let module = if cfg!(windows) {
        "_code.dll"
    } else {
        "_code.so"
    };
    write(&format!("{dir}/Mario Kart World 180{module}"), b"fixture");
    let source = format!("{dir}/Source game 1.8.0.nsp");
    write(&source, b"PFS0");
    write(
        &format!("{dir}/Mario Kart World 180_data/romfs.nsp"),
        format!("SWRNSP1\n{source}\n100\n200\n").as_bytes(),
    );
    write(
        &format!("{dir}/Mario Kart World 180.ini"),
        b"[Graphics]\nrenderer=vulkan\n",
    );
    let lib = format!("{dir}/library.json");
    let original = json!({"schema": 1, "jobs": 3, "colorMode": "light", "games": [{"id": "existing", "title": "Keep my game", "source": format!("{dir}/Other.nup"), "cover": "custom.png"}]});
    write(&lib, original.to_string().as_bytes());
    for _ in 0..2 {
        let o = nxrs(
            &dir,
            &[
                "--library",
                &lib,
                "--import-build",
                &exe,
                "--screenshot",
                &format!("{dir}/preview.png"),
                "--screenshot-delay",
                "300",
            ],
        );
        assert_eq!(o.status.code(), Some(0), "{}", text(&o));
        let saved = library(&lib);
        assert_eq!(saved["games"].as_array().unwrap().len(), 2);
        assert_eq!(
            (saved["jobs"].clone(), saved["colorMode"].clone()),
            (json!(3), json!("light"))
        );
        for (k, v) in original["games"][0].as_object().unwrap() {
            assert_eq!(&saved["games"][0][k], v);
        }
        let imported = &saved["games"][1];
        assert_eq!(imported["title"], "Mario Kart World 180");
        assert_eq!(
            (imported["renderer"].clone(), imported["source"].clone()),
            (json!("vulkan"), json!(source))
        );
        assert_eq!(imported["buildIncomplete"], false);
        assert_eq!(read(&exe), "fixture, never execute");
    }
    let shot = image::open(format!("{dir}/preview.png")).unwrap();
    assert!(shot.width() <= 1600 && shot.width() > 600);
}
