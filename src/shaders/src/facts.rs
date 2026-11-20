//! Translation facts (nxnative `dm/gfx/tools/dmsh_facts.py`): the profile's answer table for the translator's queries
//! the Switch machine code cannot answer, one row per shader key: the dimension type of textures and images (`tex`,
//! `img`), image formats, the component types of vertex attributes (`attr`) and fragment outputs (`fout`), a compute
//! shader's thread group (`local`) and shared memory bytes (`shared`).
//!
//! Two tables in the shader folder's `profile\` (written there by `nxrs-build` from the game's profile):
//! `shader_facts.tsv` (generated) and `shader_facts_manual.tsv` (hand-written answers: Link's Awakening, FINAL
//! FANTASY Resonance), merged over it fact by fact. Row: `key <TAB> JSON`; `#` lines are comments. A job's facts go
//! into its translator job (`apply`), so a changed answer translates the shader again (the job's hash covers them);
//! keys with no row keep the translator's defaults.

use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;

/// The facts a job takes from the table.
const FACTS: [&str; 6] = ["tex", "img", "attr", "fout", "local", "shared"];

pub type Facts = HashMap<String, Map<String, Value>>;

/// Rows of one table's text (bad rows are left out).
pub fn parse(text: &str) -> Facts {
    let mut out = Facts::new();
    for l in text.lines() {
        let l = l.trim_end_matches('\r');
        if l.trim().is_empty() || l.starts_with('#') {
            continue;
        }
        let Some((k, v)) = l.split_once('\t') else { continue };
        if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(v) {
            out.insert(k.trim().to_string(), m);
        }
    }
    out
}

/// `manual` merged over `generated`, fact by fact.
pub fn merge(mut generated: Facts, manual: Facts) -> Facts {
    for (k, m) in manual {
        let row = generated.entry(k).or_default();
        for (f, v) in m {
            row.insert(f, v);
        }
    }
    generated
}

/// The tables in `dir` (the shader folder's `profile\`); empty when there are none.
pub fn load(dir: &Path) -> Facts {
    let read = |n: &str| std::fs::read_to_string(dir.join(n)).map(|t| parse(&t)).unwrap_or_default();
    merge(read("shader_facts.tsv"), read("shader_facts_manual.tsv"))
}

/// Puts the row's facts into a translator job (its JSON). `local` / `shared` only for a compute job (they are its
/// thread group). Returns how many facts were applied.
pub fn apply(job: &mut Map<String, Value>, row: &Map<String, Value>) -> usize {
    let compute = job.contains_key("local");
    let mut n = 0;
    for f in FACTS {
        let Some(v) = row.get(f) else { continue };
        if matches!(f, "local" | "shared") && !compute {
            continue;
        }
        if matches!(f, "tex" | "img" | "attr" | "fout") && !v.is_object() {
            continue;
        }
        job.insert(f.to_string(), v.clone());
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tables_merge_and_apply_to_jobs() {
        let gen = parse("# generated\n08145fb333797be6\t{\"local\":[32,1,1],\"shared\":0}\n1871a290791d9148\t{\"tex\":{\"8\":\"2DArray\"},\"fout\":{\"0\":\"uint\"}}\nbad row\n");
        let man = parse("1871a290791d9148\t{\"tex\":{\"8\":\"Cube\"}}\r\nffff000011112222\t{\"attr\":{\"3\":\"sint\"}}\n");
        let f = merge(gen, man);
        assert_eq!(f.len(), 3);
        assert_eq!(f["1871a290791d9148"]["tex"], json!({"8": "Cube"}), "manual wins");
        assert_eq!(f["1871a290791d9148"]["fout"], json!({"0": "uint"}), "the generated facts it does not name stay");
        // a pixel job: its texture and output facts, no thread group
        let mut ps: Map<String, Value> = serde_json::from_value(json!({"key": "1871a290791d9148", "tex": {}, "img": {}, "attr": {}, "fout": {}})).unwrap();
        assert_eq!(apply(&mut ps, &f["1871a290791d9148"]), 2);
        assert_eq!(ps["tex"], json!({"8": "Cube"}));
        let mut ps2 = ps.clone();
        assert_eq!(apply(&mut ps2, &f["08145fb333797be6"]), 0, "local / shared only for compute");
        let mut cs: Map<String, Value> = serde_json::from_value(json!({"local": [64, 1, 1], "shared": 16, "tex": {}})).unwrap();
        assert_eq!(apply(&mut cs, &f["08145fb333797be6"]), 2);
        assert_eq!(cs["local"], json!([32, 1, 1]));
        let dir = std::env::temp_dir().join(format!("nxshaders-facts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shader_facts.tsv"), "aa\t{\"img\":{\"1\":\"R32ui\"}}\n").unwrap();
        std::fs::write(dir.join("shader_facts_manual.tsv"), "aa\t{\"tex\":{\"2\":\"3D\"}}\n").unwrap();
        let l = load(&dir);
        assert_eq!(l["aa"].len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
