//! The option table: every key Turbo's runtime reads from the game's ini, its default, and the comment written above
//! it. The order is the file's order. Values are normalised by `normalise` (Turbo's ranges) before anything reads them.

pub struct Opt {
    pub key: &'static str,
    pub default: &'static str,
    pub help: &'static str,
}

pub struct Section {
    pub title: &'static str,
    pub opts: &'static [Opt],
}

macro_rules! o {
    ($k:expr, $d:expr, $h:expr) => {
        Opt { key: $k, default: $d, help: $h }
    };
}

pub const SECTIONS: &[Section] = &[
    Section {
        title: "Display",
        opts: &[
            o!("docked", "1", "1 = TV mode: the game draws for a television (Switch: up to 1920x1080; Switch 2: up to 2560x1440 and more),\n;   0 = handheld mode: the console's own screen (Switch 1280x720, Switch 2 1920x1080), softer on a monitor"),
            o!("fullscreen", "1", "1 = start in fullscreen (F11 or Alt+Enter switches while playing; the choice is kept here)"),
            o!("language", "1", "Game language: 0 Japanese, 1 English (US), 2 French, 3 German, 4 Italian, 5 Spanish,\n;   6 Chinese, 7 Korean, 8 Dutch, 9 Portuguese, 10 Russian, 11 Taiwanese, 12 English (UK),\n;   13 French (Canada), 14 Spanish (Latin America)"),
            o!("keyboard_text", "Player", "The text the on-screen keyboard answers when the game asks for one (a name...)"),
            o!("keyboard_box", "1", "1 = when the game asks for text a box opens over it to type in (keyboard_text already in it);
;   0 = keyboard_text is answered at once"),
        ],
    },
    Section {
        title: "Controls",
        opts: &[
            o!("mouse_right_stick", "0", "Right stick: 0 = the I J K L keys, 1 = the mouse (F1 changes it)"),
            o!("mouse_sensitivity", "1.00", "Mouse speed, 0.10 to 5.00 (F1: 10% to 500%, on a curve: 10% = 5x, 100% about 14x, 500% = 1250x)"),
            o!("mouse_invert", "0", "Mouse direction: 0 = normal, 1 = left / right inverted, 2 = up / down inverted, 3 = both"),
            o!("button_layout", "switch", "Face buttons: switch = by position as on a Switch (right = A, bottom = B, top = X, left = Y;\n;   Circle = A, Cross = B on PlayStation), xbox = by the Xbox labels (bottom = A, right = B, left = X, top = Y;\n;   Cross = A, Square = X on PlayStation). Applies live"),
            o!("button_map", "", "Optional changes on top of button_layout, physical:guest pairs, e.g. south:a east:b.\n;   Physical: south east west north (or cross circle square triangle), l r zl zr lstick rstick plus minus up down\n;   left right. Guest: a b x y l r zl zr lstick rstick plus minus up down left right none"),
            o!("controller_prompt", "1", "1 = when a game wants more players than are connected, it waits with a prompt until they join
;   (A or Return: done, B or Esc: cancel); 0 = it gets the players there are at once"),
            o!("pc_user", "1", "The local profile (1..8) the game runs as: picked at launch and by any in-game user picker (a PC app has
;   its player, not a console user list). Others exist when a game made them (their save folder markers)"),
            o!("pc_user_name", "Player", "The name the game shows for that profile (NXOS_NICK overrides)"),
            o!("home_quit", "1", "1 = holding a controller's Home (logo) button 2 s asks whether to quit; 0 = off"),
            o!("pause_on_focus_loss", "0", "0 = the game runs on when its window is behind others (a PC app); 1 = the game is told it lost
;   the focus (many pause, as on a console) when the window is in the background or a prompt is open over it"),
        ],
    },
    Section {
        title: "Graphics",
        opts: &[
            o!("renderer", "vulkan", "Graphics back end: vulkan (NVN on Vulkan; the only renderer of this engine)"),
            o!("resolution", "1", "Internal 3D resolution: 1 = native, 2 = double, 0.75 / 0.5 = faster (applies live)"),
            o!("dynamic_resolution", "0", "1 = lower the resolution by itself when the GPU cannot keep up (down to dynamic_resolution_min)"),
            o!("dynamic_resolution_min", "0.6", "The lowest internal resolution dynamic_resolution goes to"),
            o!("game_dynamic_resolution", "auto", "The game's own dynamic resolution and frame pacing, as on the console: auto = on (the game is told the GPU's\n;   real times), 1 = on, 0 = off (the game is told the GPU takes no time: always its largest size, for a fast graphics card)"),
            o!("shadows", "1", "0 = no shadow maps (more FPS)"),
        ],
    },
    Section {
        title: "Performance",
        opts: &[
            o!("fps_target", "60", "Experimental: 30 or 20 = draw only every 2nd / 3rd frame while the game keeps its speed (less GPU work;\n;   effects updated on skipped frames can flicker). 120 = frame generation x2 at 120 FPS (see lsfg). 60 = normal"),
            o!("fps_limit", "1", "1 = 60 FPS, 30 = show at most 30 images a second while the game keeps its 60 Hz steps, 0 = unlimited.\n;   F4 switches between 60 and unlimited"),
            o!("forced_vsync", "0", "1 = always wait for the display's refresh (can cost a lot of FPS on slow PCs)"),
            o!("present_interval", "0", "0 = the game chooses how many vsyncs a frame lasts; 1 = force every vsync (a 30 FPS game drawn at 60:
;   only for games that keep their speed, see the profile), 2 = force every second vsync"),
            o!("null_trap", "0", "1 = a write through a NULL pointer stops the game with a precise log (diagnostics; 0 = normal)"),
            o!("memory_gb", "0", "Memory the game sees: 0 = automatic (as the console, more for games that need it), 4 = console,\n;   6 or 8 = extended (as yuzu's extended memory layout)"),
            o!("driver_exclusive", "auto", "auto = on for Intel graphics (its driver crashes when compiling and drawing at the same time), 1 = always, 0 = never"),
            o!("shader_thread", "1", "1 = build pipelines on threads of their own (no freezes), 0 = on the GPU thread (short freezes;\n;   try it if the game crashes inside the video driver)"),
            o!("low_priority_shader_compiler", "1", "1 = build shaders and pipelines at low priority (less stutter on weak CPUs)"),
        ],
    },
    Section {
        title: "Frame generation",
        opts: &[
            o!("lsfg", "1", "Frame generation: in-between frames shown while the game is below lsfg_target (1 = on, 0 = off)"),
            o!("frame_gen_type", "builtin", "builtin = the engine's own (motion estimated on the shown frames, needs nothing else);
;   lsfg = LSFG with the Lossless.dll of YOUR Lossless Scaling (copied beside the game, set in lsfg_dll, or found in Steam);
;   without a usable Lossless.dll the built-in one is used"),
            o!("lsfg_target", "60", "The FPS frame generation aims for"),
            o!("lsfg_multiplier", "2", "2 = x2, 3 = x3, 4 = x4"),
            o!("lsfg_flow", "0.5", "Flow scale: 1 = best quality, 0.5 = faster (for integrated GPUs)"),
            o!("lsfg_resolution", "720", "The height frame generation works at"),
            o!("lsfg_dll", "", "Path of Lossless.dll (empty = look for it)"),
            o!("lossless_scaling", "0", "1 = also start the Lossless Scaling app with the game (not needed for lsfg)"),
            o!("lossless_path", "", "Path of LosslessScaling.exe (empty = look in Steam)"),
        ],
    },
    Section { title: "Audio", opts: &[o!("audio_slowdown", "1", "1 = slow the sound down smoothly when the PC cannot keep up, keeping every queued sample (F1)")] },
    Section {
        title: "Recording",
        opts: &[
            o!("smooth_recording", "1", "1 = F7 videos are always smooth: the game runs in slow motion while recording if the PC cannot\n;   keep up, and the video plays at normal speed with no stutter"),
            o!("recording_fps", "60", "The video's FPS (constant, 15 to 60)"),
        ],
    },
    Section {
        title: "Debug",
        opts: &[
            o!("log", "1", "Log detail: 1 = normal, 2 = everything (slow)"),
            o!("diag", "0", "1 = automatic diagnostic dumps (F5 always works)"),
            o!("profile", "0", "1 = write profile.txt every 10 seconds (CPU profiler)"),
            o!("metrics", "0", "1 = write metrics-<time>.csv (each second: CPU, waits by kind, dropped work) and frames-<time>.csv\n;   (each frame) beside the log"),
        ],
    },
];

/// Turbo's Spanish key names (still read, written back under the current name).
pub const ALIASES: &[(&str, &str)] = &[
    ("modo_tv", "docked"),
    ("idioma", "language"),
    ("resolucion", "resolution"),
    ("resolucion_dinamica", "dynamic_resolution"),
    ("resolucion_minima", "dynamic_resolution_min"),
    ("resolucion_dinamica_del_juego", "game_dynamic_resolution"),
    ("sombras", "shadows"),
    ("perfil", "profile"),
    ("compilador_prioridad_baja", "low_priority_shader_compiler"),
    ("vsync_forzado", "forced_vsync"),
    ("grabacion_fluida", "smooth_recording"),
    ("grabacion_fps", "recording_fps"),
    ("lsfg_objetivo", "lsfg_target"),
    ("lsfg_multiplicador", "lsfg_multiplier"),
    ("lsfg_flujo", "lsfg_flow"),
    ("lsfg_resolucion", "lsfg_resolution"),
    ("lossless_ruta", "lossless_path"),
    ("controles", "touch_controls"),
    ("controles_opacidad", "touch_controls_opacity"),
];

/// Keys Turbo removed: read and dropped.
pub const REMOVED: &[&str] = &["vsync"];

pub fn canonical(key: &str) -> &str {
    ALIASES.iter().find(|(a, _)| *a == key).map(|(_, k)| *k).unwrap_or(key)
}

pub fn find(key: &str) -> Option<&'static Opt> {
    SECTIONS.iter().flat_map(|s| s.opts.iter()).find(|o| o.key == key)
}

fn clamp_f(v: &str, lo: f64, hi: f64, def: f64) -> String {
    let x = v.trim().parse::<f64>().unwrap_or(def);
    let x = if x.is_finite() { x.clamp(lo, hi) } else { def };
    trim_float(x)
}

pub fn trim_float(x: f64) -> String {
    let s = format!("{x:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

fn int(v: &str) -> i64 {
    // atoi: leading digits, 0 when none
    let t = v.trim();
    let (neg, d) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let n: i64 = d.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
    if neg {
        -n
    } else {
        n
    }
}

/// A value as the engine uses it (Turbo's ranges and fallbacks), or `None` to drop the key.
pub fn normalise(key: &str, v: &str) -> Option<String> {
    let v = v.trim();
    Some(match key {
        "button_layout" => match v {
            "" | "switch" | "nintendo" | "position" | "positions" => "switch".into(),
            "xbox" | "label" | "labels" => "xbox".into(),
            other => other.into(), // logged by the input layer, which falls back to switch
        },
        "resolution" => clamp_f(v, 0.25, 4.0, 1.0),
        "dynamic_resolution_min" => clamp_f(v, 0.25, 1.0, 0.6),
        "mouse_sensitivity" => {
            let x = v.parse::<f64>().unwrap_or(1.0);
            let x = if x > 0.0 { x.clamp(0.1, 5.0) } else { 1.0 };
            format!("{x:.2}")
        }
        "mouse_invert" => (int(v) & 3).to_string(),
        "recording_fps" => int(v).clamp(15, 60).to_string(),
        "fps_limit" => {
            let n = int(v);
            (if n != 30 && n != 0 && n < 60 { if n == 1 { 1 } else { 60 } } else { n }).to_string()
        }
        "lsfg_flow" => clamp_f(v, 0.25, 1.0, 0.5),
        "frame_gen_type" => if v.eq_ignore_ascii_case("lsfg") { "lsfg".into() } else { "builtin".into() },
        "lsfg_multiplier" => int(v).clamp(2, 4).to_string(),
        "lsfg_resolution" => int(v).clamp(240, 2160).to_string(),
        "lsfg_target" => trim_float(v.parse::<f64>().unwrap_or(60.0).max(30.0)),
        "game_dynamic_resolution" | "driver_exclusive" | "shader_process" => {
            if v == "auto" {
                "auto".into()
            } else {
                (int(v) != 0).then_some("1").unwrap_or("0").into()
            }
        }
        "renderer" => {
            if v != "vulkan" && !v.is_empty() {
                // OpenGL and Eden are Turbo's other renderers; this engine has the NVN layer on Vulkan only (the
                // owner decides whether OpenGL comes back): said once, not silently
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| eprintln!("[config] renderer={v}: this engine has no {v} renderer; Vulkan is used"));
                "vulkan".into()
            } else {
                "vulkan".into()
            }
        }
        k if REMOVED.contains(&k) => return None,
        _ => v.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges() {
        assert_eq!(normalise("resolution", "9").unwrap(), "4");
        assert_eq!(normalise("resolution", "0.75").unwrap(), "0.75");
        assert_eq!(normalise("fps_limit", "45").unwrap(), "60");
        assert_eq!(normalise("fps_limit", "30").unwrap(), "30");
        assert_eq!(normalise("fps_limit", "1").unwrap(), "1");
        assert_eq!(normalise("fps_limit", "0").unwrap(), "0");
        assert_eq!(normalise("fps_limit", "144").unwrap(), "144");
        assert_eq!(normalise("mouse_invert", "7").unwrap(), "3");
        assert_eq!(normalise("button_layout", "labels").unwrap(), "xbox");
        assert_eq!(normalise("game_dynamic_resolution", "auto").unwrap(), "auto");
        assert_eq!(normalise("recording_fps", "100").unwrap(), "60");
        assert!(normalise("vsync", "1").is_none());
        assert_eq!(canonical("idioma"), "language");
    }
}
