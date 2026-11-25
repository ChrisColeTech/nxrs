//! The game's settings the os layer reads (ini keys, published by `nxconfig`): `docked`, `language`,
//! `keyboard_text`, `audio_slowdown`. An environment variable `NXOS_<KEY>` (upper case) overrides the ini for a test.
//! Changes made while the game runs (the ini edited, a hotkey) apply live: the operation mode (with the console's
//! notification to the game) and the sound's slowdown.

/// A setting's value (the ini's, else the default).
pub fn get(key: &str) -> String {
    if let Ok(v) = std::env::var(format!("NXOS_{}", key.to_ascii_uppercase())) {
        return v;
    }
    nxconfig::string(key)
}

pub fn int(key: &str) -> i64 {
    get(key).trim().parse().unwrap_or(0)
}

/// A per-game fact of the running title's profile (`game.tsv`, nxnative's game.py keys: `game_env`,
/// `alloc_big_align`, `perf_mode`, `op_mode`, `launch_param`, `fps_hold`, `fps_max`, ...): `NXOS_<KEY>` for a test,
/// else the profile's value, else `default` (game.py's). Game facts, not player settings: no ini key.
pub fn game(key: &str, default: &str) -> String {
    if let Ok(v) = std::env::var(format!("NXOS_{}", key.to_ascii_uppercase())) {
        return v;
    }
    profiles::by_title(crate::profile::get().program_id)
        .and_then(|p| p.get(key))
        .unwrap_or_else(|| default.to_string())
}

/// A game fact that is `on` / `off` (or 1 / 0 in an override).
pub fn game_flag(key: &str, default: bool) -> bool {
    flag_value(&game(key, if default { "on" } else { "off" }))
}

fn flag_value(v: &str) -> bool {
    matches!(v.trim(), "on" | "1" | "true" | "yes")
}

/// The bytes of a profile's `launch_param` (nxnative hle_oe.c): `lang8` (the 8-byte language code
/// GetDesiredLanguage gives), `f32:<value>`, `u32:<value>` (0x for hex), `hex:<bytes>`; at most 256 bytes. Fields
/// not understood are named in the second value.
pub fn launch_param_bytes(spec: &str, lang8: u64) -> (Vec<u8>, Vec<String>) {
    let (mut p, mut bad) = (Vec::new(), Vec::new());
    for t in spec.split_whitespace() {
        if p.len() >= 256 - 8 {
            break;
        }
        if t == "lang8" {
            p.extend_from_slice(&lang8.to_le_bytes());
        } else if let Some(v) = t.strip_prefix("f32:") {
            p.extend_from_slice(&v.parse::<f32>().unwrap_or(0.0).to_le_bytes());
        } else if let Some(v) = t.strip_prefix("u32:") {
            let n = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
                Some(h) => u32::from_str_radix(h, 16).unwrap_or(0),
                None => v.parse().unwrap_or(0),
            };
            p.extend_from_slice(&n.to_le_bytes());
        } else if let Some(h) = t.strip_prefix("hex:") {
            for i in (0..h.len() / 2).map(|i| i * 2) {
                if p.len() < 256 {
                    p.push(u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0));
                }
            }
        } else {
            bad.push(t.to_string());
        }
    }
    (p, bad)
}

/// The `language` numbers (nn::settings::Language order) as language codes.
pub const LANGUAGES: [&str; 15] = [
    "ja", "en-US", "fr", "de", "it", "es", "zh-CN", "ko", "nl", "pt", "ru", "zh-TW", "en-GB",
    "fr-CA", "es-419",
];

/// The language code of a `language` setting (a number, or a code as such).
pub fn language_code(v: &str) -> String {
    match v.trim().parse::<usize>() {
        Ok(n) => LANGUAGES.get(n).unwrap_or(&"en-US").to_string(),
        Err(_) if !v.trim().is_empty() => v.trim().to_string(),
        Err(_) => "en-US".into(),
    }
}

/// Live changes: called once at start.
pub fn watch() {
    nxconfig::on_change(|k, v| match k {
        "audio_slowdown" => crate::media::sink::set_smooth(v.trim() != "0"),
        "docked" => crate::services::oe::set_opmode(if v.trim() == "0" { 0 } else { 1 }),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn languages() {
        assert_eq!(super::language_code("1"), "en-US");
        assert_eq!(super::language_code("3"), "de");
        assert_eq!(super::language_code("fr-CA"), "fr-CA");
        assert_eq!(super::language_code("99"), "en-US");
    }

    /// nxnative hle_oe.c's launch_param fields (Super Mario 3D All-Stars' programs: `lang8 f32:1.12 f32:2.24`).
    #[test]
    fn launch_param_as_nxnative() {
        let lang = u64::from_le_bytes(*b"en-US\0\0\0");
        let (b, bad) = super::launch_param_bytes("lang8 f32:1.12 f32:2.24", lang);
        let mut want = b"en-US\0\0\0".to_vec();
        want.extend_from_slice(&1.12f32.to_le_bytes());
        want.extend_from_slice(&2.24f32.to_le_bytes());
        assert_eq!((b, bad), (want, vec![]));
        let (b, bad) = super::launch_param_bytes("u32:0x10 u32:7 hex:a0ff zz", 0);
        assert_eq!(b, vec![0x10, 0, 0, 0, 7, 0, 0, 0, 0xa0, 0xff]);
        assert_eq!(bad, vec!["zz".to_string()]);
    }

    /// Profile facts: the running title's game.tsv value, else game.py's default; on / off flags.
    #[test]
    fn game_facts_from_the_profile() {
        assert!(
            super::flag_value("on")
                && super::flag_value("1")
                && !super::flag_value("off")
                && !super::flag_value("0")
        );
        // Absolum (0100A83022A22000): stdalloc_cap 0xa0000000; the default title: game.py's defaults
        let absolum = profiles::by_title(0x0100_A830_22A2_2000).unwrap();
        assert_eq!(absolum.get("stdalloc_cap").as_deref(), Some("0xa0000000"));
        assert_eq!(
            super::game("perf_mode", "follow"),
            "follow",
            "no profile for the default program"
        );
    }
}
