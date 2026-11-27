//! The launcher owns its artwork (LAUNCH-ART-WEBIMPORT): images set on an entry are copied into
//! `<data folder>\artwork`, replaced with the art and removed with the entry; the start's step copies
//! a library's outside artwork in once, backed up first, leaving missing images as they are.
mod common;

use common::*;
use nxrs_launcher::constants::engine as e;
use nxrs_launcher::flows::surface::Surface;

fn png(path: &str, shade: u8) {
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(2, 2, image::Rgb([shade, 2, 3]))
        .save(path)
        .unwrap();
}

#[test]
fn the_start_copies_outside_artwork_in_once_and_leaves_missing_images() {
    let (_t, dir) = tmp();
    let mut g = built_game(&dir);
    let picked = format!("{dir}/Downloads/kart cover.png");
    png(&picked, 9);
    g.cover = picked.clone();
    g.background = format!("{dir}/Downloads/gone.jpg");
    let mut app = app_with(&dir, vec![g]);
    app.save();
    let lines = app.own_all_artwork();
    let own = format!("{dir}/artwork/g-cover.png");
    let entry = app.lib.find("g").unwrap().clone();
    assert_eq!(entry.cover, own);
    assert_eq!(
        std::fs::read(&own).unwrap(),
        std::fs::read(&picked).unwrap()
    );
    // a missing image is never blanked, and is said
    assert_eq!(entry.background, format!("{dir}/Downloads/gone.jpg"));
    assert!(lines.iter().any(|l| l.contains("gone.jpg")), "{lines:?}");
    // the file was backed up before the repoint, and the repoint was saved
    let backup = library(&format!("{dir}/{}", e::PRE_ARTWORK_BACKUP));
    assert_eq!(backup["games"][0]["cover"], picked.as_str());
    assert_eq!(
        library(&format!("{dir}/library.json"))["games"][0]["cover"],
        own.as_str()
    );
    // the original may go: the entry's art is the launcher's own copy
    std::fs::remove_file(&picked).unwrap();
    assert!(std::fs::metadata(&own).is_ok());
    // idempotent: nothing more is copied, the backup is not rewritten
    let again = app.own_all_artwork();
    assert!(
        again.iter().all(|l| !l.starts_with("Artwork:")),
        "{again:?}"
    );
    assert_eq!(app.lib.find("g").unwrap().cover, own);
}

#[test]
fn properties_apply_copies_and_replaces_and_remove_deletes_the_copies() {
    let (_t, dir) = tmp();
    let first = format!("{dir}/pics/a.png");
    let second = format!("{dir}/pics/b.jpg");
    png(&first, 1);
    png(&second, 2);
    let mut app = app_with(&dir, vec![built_game(&dir)]);
    let apply = |app: &mut nxrs_launcher::flows::App, cover: &str| {
        app.show_properties(0);
        let Some(Surface::Properties(mut d)) = app.surfaces.pop() else {
            panic!("no Properties window")
        };
        d.cover = cover.into();
        d.background = cover.into();
        assert!(app.apply_properties(&d));
    };
    apply(&mut app, &first);
    let g = app.lib.find("g").unwrap().clone();
    assert_eq!(g.cover, format!("{dir}/artwork/g-cover.png"));
    assert_eq!(g.background, format!("{dir}/artwork/g-background.png"));
    // replaced by an image of another kind: the old copies go, the new ones are the images
    apply(&mut app, &second);
    let g = app.lib.find("g").unwrap().clone();
    assert_eq!(g.cover, format!("{dir}/artwork/g-cover.jpg"));
    assert!(std::fs::metadata(format!("{dir}/artwork/g-cover.png")).is_err());
    assert_eq!(
        std::fs::read(&g.background).unwrap(),
        std::fs::read(&second).unwrap()
    );
    // the entry's own copy chosen again is left as it is
    apply(&mut app, &g.cover.clone());
    assert_eq!(app.lib.find("g").unwrap().cover, g.cover);
    app.confirm_remove("g", false);
    assert!(app.lib.find("g").is_none());
    assert!(
        std::fs::read_dir(format!("{dir}/artwork"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn an_imported_build_gets_its_cover_copied_in() {
    let (_t, dir) = tmp();
    let g = built_game(&dir);
    png(&format!("{}/icon.png", g.data_dir()), 5);
    let mut app = app_with(&dir, vec![]);
    assert!(app.import_build(&g.executable()));
    let entry = app.lib.games[0].clone();
    assert_eq!(entry.cover, format!("{dir}/artwork/{}-cover.png", entry.id));
}
