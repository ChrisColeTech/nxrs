//! The command-line verbs with their exit codes: build, DLC, updates, saves, the engine, the doctor,
//! images.

mod common;

use common::*;

#[test]
fn build_dlc_updates_and_saves_verbs() {
    let (_t, dir) = tmp();
    let lib = fixture(&dir);
    let data = format!("{dir}/data");
    let sel = ["--library", lib.as_str(), "--select-title", "kart"];
    let dlc = format!("{dir}/Booster.nsp");
    write(&dlc, b"PFS0....");
    let o = nxrs(&data, &[&sel[..], &["--install-dlc", &dlc]].concat());
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("DLC: Installed: 0100152000023001 v65536"));
    assert_eq!(
        library(&lib)["games"][0]["dlc"].as_array().unwrap().len(),
        1
    );
    let o = nxrs(&data, &[&sel[..], &["--remove-dlc", &dlc]].concat());
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let o = nxrs(
        &data,
        &[&sel[..], &["--install-dlc", &format!("{dir}/missing.nsp")]].concat(),
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).contains("DLC error: The file does not exist"));
    let (update, other) = (format!("{dir}/Update.nsp"), format!("{dir}/other game.nsp"));
    write(&update, b"PFS0....");
    write(&other, b"PFS0....");
    let o = nxrs(
        &data,
        &[
            &sel[..],
            &["--install-update", &update, "--install-update", &other],
        ]
        .concat(),
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("Updates error: other game.nsp: this update is for another game"));
    let o = nxrs(&data, &[&sel[..], &["--update-choice", "none"]].concat());
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(library(&lib)["games"][0]["updateChoice"], "none");
    let o = nxrs(
        &data,
        &[&sel[..], &["--update-choice", "D:/not/listed.nsp"]].concat(),
    );
    assert_eq!(o.status.code(), Some(1));
    let o = nxrs(&data, &[&sel[..], &["--remove-update", &update]].concat());
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let o = nxrs(&data, &[&sel[..], &["--copy-saves"]].concat());
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).contains("Saves error: The Turbo build has no saves to copy"));
    let o = nxrs(&data, &[&sel[..], &["--build"]].concat());
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        library(&lib)["games"][0]["lastBuilt"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
    );
}

#[test]
fn source_can_be_changed_and_built_through_the_product_cli() {
    let (_t, dir) = tmp();
    let lib = fixture(&dir);
    let data = format!("{dir}/data");
    let source = format!("{dir}/replacement.nsp");
    write(&source, b"PFS0....");
    let o = nxrs(
        &data,
        &[
            "--library",
            &lib,
            "--select-title",
            "Kart",
            "--source",
            &source,
            "--build",
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("Source:"));
    let game = &library(&lib)["games"][0];
    let normalized =
        |value: &serde_json::Value| value.as_str().unwrap().replace('\\', "/").to_lowercase();
    assert_eq!(
        normalized(&game["source"]),
        source.replace('\\', "/").to_lowercase()
    );
    assert_eq!(
        normalized(&game["builtSource"]),
        source.replace('\\', "/").to_lowercase()
    );
}

#[test]
fn engine_doctor_and_images() {
    let (_t, dir) = tmp();
    let lib = fixture(&dir);
    let data = format!("{dir}/data");
    let o = nxrs(
        &data,
        &[
            "--library",
            &lib,
            "--select-title",
            "Kart",
            "--engine",
            "turbo",
            "--build",
        ],
    );
    assert_eq!(
        o.status.code(),
        Some(2),
        "Turbo is no longer an engine choice"
    );
    let o = nxrs(&data, &["--library", &lib, "--nxrs-doctor"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).contains("\u{2717} Vulkan device: Old GPU"));
    let o = nxrs(
        &data,
        &["--library", &lib, "--nxrs-build", "", "--nxrs-doctor"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(library(&lib)["nxrsBuild"], "");
    let png = format!("{dir}/one.png");
    let pixel = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
    pixel.save(&png).unwrap();
    assert_eq!(nxrs(&data, &["--check-image", &png]).status.code(), Some(0));
    write(&format!("{dir}/bad.png"), b"not an image");
    assert_eq!(
        nxrs(&data, &["--check-image", &format!("{dir}/bad.png")])
            .status
            .code(),
        Some(1)
    );
}
