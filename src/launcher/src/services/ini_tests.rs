//! A game's INI (`rendererPreservesSettings`, `screenModeIni`, `pauseOnFocusLossIni`, `askUser…`,
//! `screenModePerGameOverDefault`, `buttonLayoutIni`).

use super::ini_file::*;
use super::library_file::{Store, load, save};
use super::services_tests::{contains, read, tmp, write};
use crate::model::game::Game;
use crate::model::game_json;
use crate::model::library::Library;
use crate::model::play::PlaySettings;

#[test]
fn renderer_preserves_settings() {
    let (_t, dir) = tmp();
    let path = format!("{dir}/Game.ini");
    write(&path, b"; keep this comment\r\n[Graphics]\r\nrenderer=opengl\r\nresolution=0.75\r\n[Audio]\r\naudio_slowdown=0\r\n");
    write_renderer(&path, "vulkan").unwrap();
    let text = read(&path);
    for part in [
        "renderer=vulkan\r\n",
        "resolution=0.75",
        "; keep this comment",
        "audio_slowdown=0",
    ] {
        assert!(contains(&text, part), "{part}");
    }
}

#[test]
fn screen_mode_ini() {
    let (_t, dir) = tmp();
    let path = format!("{dir}/Game.ini");
    assert!(read_docked(&path));
    write(
        &path,
        b"[Display]\r\ndocked=1\r\nfullscreen=1\r\n[Graphics]\r\nrenderer=vulkan\r\n",
    );
    write_docked(&path, false).unwrap();
    assert!(!read_docked(&path));
    let text = read(&path);
    for part in [
        "docked=0\r\n",
        "fullscreen=1",
        "renderer=vulkan",
        "settings_version=2",
    ] {
        assert!(contains(&text, part), "{part}");
    }
    write_docked(&path, true).unwrap();
    assert!(read_docked(&path));
}

#[test]
fn pause_on_focus_loss_ini() {
    let (_t, dir) = tmp();
    let ini = format!("{dir}/Game.ini");
    write(&ini, b"; keep\r\n[Display]\r\ndocked=1\r\n[Controls]\r\nbutton_layout=switch\r\npause_on_focus_loss=0\r\n");
    assert!(!read_pause(&ini));
    write_pause(&ini, true).unwrap();
    assert_eq!(read(&ini), b"; keep\r\n[Display]\r\ndocked=1\r\n[Controls]\r\nbutton_layout=switch\r\npause_on_focus_loss=1\r\n");
    let old = format!("{dir}/Old.ini");
    write(&old, b"[Controls]\nbutton_layout=xbox\n");
    write_pause(&old, false).unwrap();
    assert_eq!(
        read(&old),
        b"[Controls]\npause_on_focus_loss=0\nbutton_layout=xbox\n"
    );
    assert!(!read_pause(&format!("{dir}/missing.ini")));
    let mut library = Library::new(&format!("{dir}/library.json"));
    library.pause_on_focus_loss = true;
    library.mute_on_focus_loss = true;
    let game = Game {
        id: "g".into(),
        title: "Game".into(),
        build_dir: dir.clone(),
        output_name: "Game".into(),
        ..Game::default()
    };
    library.games.push(game.clone());
    save(&library, &mut Store::default()).unwrap();
    let mut back = Library::new(&format!("{dir}/library.json"));
    load(&mut back, &mut Store::default()).unwrap();
    assert!(back.pause_on_focus_loss && back.mute_on_focus_loss && back.play().mute);
    let mut legacy = game_json::to_json(&back.games[0]);
    assert!(legacy.get("pauseOnFocusLoss").is_none());
    legacy["pauseOnFocusLoss"] = "1".into();
    assert!(
        game_json::to_json(&game_json::from_json(&legacy))
            .get("pauseOnFocusLoss")
            .is_none()
    );
    write_game_settings(&game, "switch", &PlaySettings::new(true, true, true, false)).unwrap();
    assert!(read_pause(&game.ini_path()) && read_mute(&game.ini_path()));
    write_game_settings(
        &game,
        "switch",
        &PlaySettings::new(false, true, true, false),
    )
    .unwrap();
    assert!(!read_pause(&game.ini_path()) && read_mute(&game.ini_path()));
    write_play_settings(&game.ini_path(), &PlaySettings::default()).unwrap();
    assert!(!read_mute(&game.ini_path()) && read_docked(&game.ini_path()));
    write_play_settings(
        &game.ini_path(),
        &PlaySettings::new(false, false, false, false),
    )
    .unwrap();
    assert!(!read_docked(&game.ini_path()));
}

#[test]
fn ask_user_ini() {
    let (_t, dir) = tmp();
    let ini = format!("{dir}/Game.ini");
    assert!(!read_ask_user(&ini));
    write_play_settings(&ini, &PlaySettings::new(false, false, true, true)).unwrap();
    assert!(read_ask_user(&ini));
    write_play_settings(&ini, &PlaySettings::default()).unwrap();
    assert!(!read_ask_user(&ini));
}

#[test]
fn screen_mode_per_game_over_default() {
    let (_t, dir) = tmp();
    let mut game = Game {
        build_dir: format!("{dir}/build"),
        output_name: "Game".into(),
        ..Game::default()
    };
    std::fs::create_dir_all(&game.build_dir).unwrap();
    game.screen_mode = "handheld".into();
    write_game_settings(&game, "switch", &PlaySettings::default()).unwrap();
    assert!(!read_docked(&game.ini_path()));
    game.screen_mode.clear();
    write_game_settings(&game, "switch", &PlaySettings::default()).unwrap();
    assert!(read_docked(&game.ini_path()));
    let mut library = Library::new(&format!("{dir}/library.json"));
    library.docked = false;
    assert!(!library.play_for(&game).docked);
    game.screen_mode = "tv".into();
    assert!(library.play_for(&game).docked && !library.play_for(&game).pause);
    // a library from before: a built game whose INI says handheld keeps it as its own
    let old = Game {
        id: "old".into(),
        title: "Old".into(),
        build_dir: format!("{dir}/old"),
        output_name: "Old".into(),
        ..Game::default()
    };
    write(&old.executable(), b"x");
    write(&old.code_module(), b"x");
    std::fs::create_dir_all(old.data_dir()).unwrap();
    write(&old.ini_path(), b"[Display]\ndocked=0\n");
    let mut entry = game_json::to_json(&old);
    entry.as_object_mut().unwrap().remove("screenMode");
    write(
        &format!("{dir}/old.json"),
        serde_json::json!({"schema": 1, "games": [entry]})
            .to_string()
            .as_bytes(),
    );
    let mut before = Library::new(&format!("{dir}/old.json"));
    load(&mut before, &mut Store::default()).unwrap();
    assert_eq!(before.games[0].screen_mode, "handheld");
    write(&old.ini_path(), b"[Display]\ndocked=1\n");
    let mut tv = Library::new(&format!("{dir}/old.json"));
    load(&mut tv, &mut Store::default()).unwrap();
    assert_eq!(tv.games[0].screen_mode, "");
}

#[test]
fn button_layout_ini() {
    let (_t, dir) = tmp();
    let path = format!("{dir}/Game.ini");
    let original = "; keep\r\n[Controls]\r\n; help text\r\nbutton_layout=switch\r\nbutton_map=south:a\r\n[Graphics]\r\nrenderer=opengl\r\n";
    write(&path, original.as_bytes());
    write_button_layout(&path, "xbox").unwrap();
    let expected = original.replace("button_layout=switch", "button_layout=xbox");
    assert_eq!(read(&path), expected.as_bytes());
    assert!(write_button_layout(&path, "sega").is_err() && write_button_layout(&path, "").is_err());
    assert!(write_button_layout(&path, "nintendo").is_err());
    assert_eq!(read(&path), expected.as_bytes());
    write(&path, b"[Controls]\nmouse_invert=0\n");
    write_button_layout(&path, "switch").unwrap();
    assert_eq!(
        read(&path),
        b"[Controls]\nbutton_layout=switch\nmouse_invert=0\n"
    );
    std::fs::remove_file(&path).unwrap();
    write_button_layout(&path, "xbox").unwrap();
    assert!(contains(&read(&path), "[Controls]\nbutton_layout=xbox\n"));
    let mut game = Game {
        build_dir: dir.clone(),
        output_name: "Game".into(),
        renderer: "vulkan".into(),
        button_layout: "switch".into(),
        ..Game::default()
    };
    std::fs::remove_file(&path).unwrap();
    write_game_settings(&game, "xbox", &PlaySettings::default()).unwrap();
    assert!(
        contains(&read(&path), "renderer=vulkan") && contains(&read(&path), "button_layout=switch")
    );
    game.button_layout.clear();
    write_game_settings(&game, "xbox", &PlaySettings::default()).unwrap();
    assert!(
        contains(&read(&path), "button_layout=xbox")
            && !contains(&read(&path), "button_layout=switch")
    );
    let mut f = read(&path);
    f.extend_from_slice(b"\r\n[Display]\r\nfullscreen=0\r\n");
    write(&path, &f);
    write_game_settings(&game, "xbox", &PlaySettings::default()).unwrap();
    assert!(contains(&read(&path), "fullscreen=0") && !contains(&read(&path), "fullscreen=1"));
}
