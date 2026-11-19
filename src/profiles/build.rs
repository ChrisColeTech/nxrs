// Embeds every profile under games/ and the shared tables (shared/) into the library: data.rs holds one
// include_str! per file, so the engine carries the profiles with no files beside it.
use std::fmt::Write;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let games = root.join("games");
    println!("cargo:rerun-if-changed={}", games.display());
    println!("cargo:rerun-if-changed={}", root.join("shared").display());
    let mut ids: Vec<String> = std::fs::read_dir(&games)
        .expect("games/")
        .flatten()
        .filter(|e| e.path().join("game.tsv").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    let lit = |p: &Path| format!("include_str!({:?})", p.display().to_string());
    let opt = |p: std::path::PathBuf| if p.is_file() { format!("Some({})", lit(&p)) } else { "None".into() };
    let opt_bytes = |p: std::path::PathBuf| if p.is_file() { format!("Some(include_bytes!({:?}))", p.display().to_string()) } else { "None".into() };
    let mut s = String::from("pub static GAMES: &[RawProfile] = &[\n");
    for id in &ids {
        let d = games.join(id);
        println!("cargo:rerun-if-changed={}", d.display());
        for f in ["game.tsv", "patches.toml", "turbo.toml", "shaders.tsv.zst"] {
            println!("cargo:rerun-if-changed={}", d.join(f).display());
        }
        writeln!(
            s,
            "    RawProfile {{ id: {id:?}, game_tsv: {}, patches: {}, turbo: {}, shaders: {} }},",
            lit(&d.join("game.tsv")),
            opt(d.join("patches.toml")),
            opt(d.join("turbo.toml")),
            opt_bytes(d.join("shaders.tsv.zst"))
        )
        .unwrap();
    }
    s.push_str("];\n");
    for (name, file) in [("SDK_PATCHES", "sdk_patches.toml"), ("ENGINE_PATCHES", "engine_patches.toml"), ("SETTINGS_TSV", "settings.tsv"), ("SOURCE", "source.txt")] {
        let p = root.join("shared").join(file);
        println!("cargo:rerun-if-changed={}", p.display());
        writeln!(s, "pub static {name}: &str = {};", lit(&p)).unwrap();
    }
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("data.rs"), s).unwrap();
}
