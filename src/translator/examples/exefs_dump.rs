//! cargo run -p translator --release --example exefs_dump -- <input> [--keys path] [--out dir] [--list]
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut input, mut keys, mut out, mut listing) = (None, None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--keys" => keys = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--list" => listing = true,
            _ => input = Some(PathBuf::from(a)),
        }
    }
    let Some(input) = input else {
        eprintln!("usage: exefs_dump <input> [--keys path] [--out dir] [--list]");
        std::process::exit(2);
    };
    if listing {
        match translator::package::list(&input, keys.as_deref()) {
            Ok(lines) => lines.iter().for_each(|l| println!("{l}")),
            Err(e) => eprintln!("list failed: {e}"),
        }
    }
    let started = std::time::Instant::now();
    let exefs = match translator::package::open_exefs(&input, keys.as_deref()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    println!("kind:     {:?}", exefs.kind);
    println!("title id: {}", exefs.title_id.map_or("unknown".into(), |t| format!("{t:016X}")));
    println!("version:  {}", exefs.version.map_or("unknown".into(), |v| v.to_string()));
    for (name, bytes) in &exefs.files {
        println!("  {name:<12} {:>12}", bytes.len());
    }
    match exefs.main_build_id() {
        Some(id) => println!("main build id: {}", id.iter().map(|b| format!("{b:02X}")).collect::<String>()),
        None => println!("main build id: (no NSO main)"),
    }
    println!("read in {:.2?}", started.elapsed());
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir).expect("create out dir");
        for (name, bytes) in &exefs.files {
            std::fs::write(dir.join(name), bytes).expect("write file");
        }
        println!("wrote {} files to {}", exefs.files.len(), dir.display());
    }
}
