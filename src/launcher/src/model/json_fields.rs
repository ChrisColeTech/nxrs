//! Reading the library file's values as the Qt launcher did: a missing or mistyped value is its default.

use serde_json::{Map, Value};

pub fn int_of(o: &Map<String, Value>, key: &str, default: i64) -> i64 {
    match o.get(key) {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(default),
        _ => default,
    }
}
pub fn str_of(o: &Map<String, Value>, key: &str) -> String {
    match o.get(key) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}
pub fn str_or(o: &Map<String, Value>, key: &str, default: &str) -> String {
    match o.get(key) {
        Some(Value::String(s)) => s.clone(),
        _ => default.to_string(),
    }
}
pub fn bool_of(o: &Map<String, Value>, key: &str, default: bool) -> bool {
    match o.get(key) {
        Some(Value::Bool(b)) => *b,
        _ => default,
    }
}
pub fn f32_of(o: &Map<String, Value>, key: &str, default: f32) -> f32 {
    o.get(key)
        .and_then(Value::as_f64)
        .map(|f| f as f32)
        .unwrap_or(default)
}
/// The entries of a list of strings, empty and duplicate paths left out.
pub fn paths_of(o: &Map<String, Value>, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(Value::Array(list)) = o.get(key) {
        for value in list {
            let path = value.as_str().unwrap_or("");
            if !path.is_empty() && !out.iter().any(|h| super::path_text::same_path(h, path)) {
                out.push(path.to_string());
            }
        }
    }
    out
}
