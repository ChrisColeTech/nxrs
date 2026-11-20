//! JSON written as Qt's `QJsonDocument::toJson()` writes it: keys sorted, 4-space indent, `"key": value`,
//! an empty array as `[` newline indent `]`, integral numbers without a fraction, non-ASCII as UTF-8.

use serde_json::Value;

pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, 0, &mut out);
    out.push('\n');
    out
}

fn indent(n: usize, out: &mut String) {
    for _ in 0..n {
        out.push_str("    ");
    }
}

fn write_number(n: &serde_json::Number, out: &mut String) {
    if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else {
        let f = n.as_f64().unwrap_or(0.0);
        if f.fract() == 0.0 && f.abs() < 9.0e15 {
            out.push_str(&(f as i64).to_string());
        } else {
            out.push_str(&format!("{f}"));
        }
    }
}

fn write_value(value: &Value, level: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(n, out),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(level + 1, out);
                write_value(item, level + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(level, out);
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("{\n");
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.iter().enumerate() {
                indent(level + 1, out);
                write_string(key, out);
                out.push_str(": ");
                if let Some(v) = map.get(*key) {
                    write_value(v, level + 1, out);
                }
                out.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
            }
            indent(level, out);
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
