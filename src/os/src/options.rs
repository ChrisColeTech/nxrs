//! The game's settings the os layer reads (Turbo's ini keys, published by `nxconfig`): `docked`, `language`,
//! `keyboard_text`, `audio_slowdown`. An environment variable `NXOS_<KEY>` (upper case) overrides the ini for a test.
//! Changes made while the game runs (the ini edited, a hotkey) apply live: the operation mode (with the console's
//! notification to the game) and the sound's slowdown.

/// A setting's value (the ini's, else Turbo's default).
pub fn get(key: &str) -> String {
    if let Ok(v) = std::env::var(format!("NXOS_{}", key.to_ascii_uppercase())) {
        return v;
    }
    nxconfig::string(key)
}

pub fn int(key: &str) -> i64 {
    get(key).trim().parse().unwrap_or(0)
}

/// Turbo's `language` numbers (nn::settings::Language order) as language codes.
pub const LANGUAGES: [&str; 15] =
    ["ja", "en-US", "fr", "de", "it", "es", "zh-CN", "ko", "nl", "pt", "ru", "zh-TW", "en-GB", "fr-CA", "es-419"];

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
}
