//! cargo run -p translator --release --example symgrep -- <regex> <NSO or NRO file>...
//! The symbols (defined or imported) whose names match, one per line: file, value, name.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let re = regex::Regex::new(&a[0]).expect("regex");
    for f in &a[1..] {
        let Ok(raw) = std::fs::read(f) else { continue };
        let Ok(n) = translator::loader::Nso::parse(&raw) else { continue };
        for s in n.syms.iter().filter(|s| re.is_match(&s.name)) {
            println!("{f}\t{:#x}\t{}", s.value, s.name);
        }
    }
}
