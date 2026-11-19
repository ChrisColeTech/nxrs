//! cargo run -p translator --release --example symat -- <NSO or NRO file> <offset hex>...
//! The nearest symbol at or below each module offset.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let raw = std::fs::read(&a[0]).expect("read");
    let n = translator::loader::Nso::parse(&raw).expect("parse");
    let mut syms: Vec<(u64, String)> = n.syms.iter().filter(|s| s.value != 0 && !s.name.is_empty()).map(|s| (s.value, s.name.clone())).collect();
    syms.sort();
    for o in &a[1..] {
        let off = u64::from_str_radix(o.trim_start_matches("0x"), 16).unwrap();
        let i = syms.partition_point(|s| s.0 <= off);
        match i.checked_sub(1).map(|k| &syms[k]) {
            Some((v, name)) => println!("{off:#x}: {name} +{:#x}", off - v),
            None => println!("{off:#x}: ?"),
        }
    }
}
