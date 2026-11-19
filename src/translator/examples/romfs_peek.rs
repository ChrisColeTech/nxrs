//! cargo run -p translator --release --example romfs_peek -- <game> <romfs path> <offset hex> <len hex> [out file]
//! Hex dump (or a copy) of a RomFS file's bytes.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 4 {
        eprintln!("usage: romfs_peek <game> <romfs path> <offset hex> <len hex> [out file]");
        std::process::exit(2);
    }
    let g = translator::package::open_game(std::path::Path::new(&a[0]), &Default::default()).expect("open");
    let h = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap();
    let d = g.romfs.read(&a[1], h(&a[2]), h(&a[3])).expect("read");
    if let Some(out) = a.get(4) {
        std::fs::write(out, &d).unwrap();
        return;
    }
    for (i, c) in d.chunks(16).enumerate() {
        println!("{:08x}: {}", h(&a[2]) + 16 * i as u64, c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
    }
}
