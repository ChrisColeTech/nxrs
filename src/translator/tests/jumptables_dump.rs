//! Differential check of `translator::xlate::jumptables` against mkw-recomp's jumptables.py on every br of real modules.
//! Generate the dumps first (scripts/jt/dump_py.py, see its header), then:
//!   cargo test --release -p translator --test jumptables_dump -- --ignored --nocapture
//! Env: JT_DUMP_DIR (default <repo>/build/jt, one <module>[.variant].txt per run), JT_EXEFS (default <repo>/build/ref/exefs),
//! JT_MODULES (comma list; default every dump present).

use std::path::PathBuf;

use translator::xlate::jumptables::JumpTables;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore]
fn jumptables_match_python() {
    let dir = std::env::var("JT_DUMP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo().join("build/jt"));
    let exefs = std::env::var("JT_EXEFS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo().join("build/ref/exefs"));
    let mut names: Vec<String> = match std::env::var("JT_MODULES") {
        Ok(s) => s.split(',').map(str::to_string).collect(),
        Err(_) => std::fs::read_dir(&dir)
            .expect("dump dir")
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_suffix(".txt"))
                    .map(str::to_string)
            })
            .collect(),
    };
    names.sort();
    assert!(!names.is_empty());
    let mut total_bad = 0;
    for name in names {
        let text = std::fs::read_to_string(dir.join(format!("{name}.txt"))).unwrap();
        let mut lines = text.lines();
        let base =
            u64::from_str_radix(lines.next().unwrap().strip_prefix("# base 0x").unwrap(), 16)
                .unwrap();
        let module = name.split('.').next().unwrap(); // <module>[.variant].txt
        let raw = std::fs::read(exefs.join(module)).unwrap();
        let m = translator::loader::Module::new(
            module,
            translator::loader::Nso::parse(&raw).unwrap(),
            base,
        );
        let mut jt = JumpTables::new(&m);
        let t0 = std::time::Instant::now();
        let (mut ok, mut bad, mut resolved) = (0, 0, 0);
        for line in lines {
            let f: Vec<&str> = line.split(' ').collect();
            let h = |s: &str| u64::from_str_radix(s, 16).unwrap();
            let (br, entry, end) = (h(f[0]), h(f[1]), h(f[2]));
            let r = jt.resolve(br, entry, end);
            let c = jt.is_computed(br, entry);
            let rs = match &r {
                None => "None".to_string(),
                Some(v) => v
                    .iter()
                    .map(|t| format!("{t:x}"))
                    .collect::<Vec<_>>()
                    .join(","),
            };
            let got = format!("{} {}", c as u8, rs);
            let want = format!("{} {}", f[3], f[4]);
            if r.is_some() {
                resolved += 1;
            }
            if got == want {
                ok += 1;
            } else {
                bad += 1;
                if bad <= 5 {
                    let short = |s: &str| s.chars().take(200).collect::<String>();
                    println!("  MISMATCH {name} br={br:x} entry={entry:x} end={end:x}\n    py:   {}\n    rust: {}", short(&want), short(&got));
                }
            }
        }
        println!(
            "{name}: {ok} match, {bad} mismatch, {resolved} resolved ({:.1}s)",
            t0.elapsed().as_secs_f64()
        );
        total_bad += bad;
    }
    assert_eq!(total_bad, 0);
}
