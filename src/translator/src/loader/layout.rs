//! Guest address layout of the loaded modules, computed from their sizes.
//! Modules are placed in Horizon's load order (rtld dropped):
//! main, subsdk0..9, sdk; each base aligned to 2 MiB from 0x7000000000, below 2^39. NRO modules the game loads
//! at run time (`nro_<index>`, `package::nro`) follow, in name order: their place is fixed at build time and the
//! runtime maps each there when the game loads it.

pub const FIRST_BASE: u64 = 0x70_0000_0000;
pub const ALIGN: u64 = 0x20_0000;
pub const ADDRESS_LIMIT: u64 = 1 << 39;

pub fn order() -> Vec<String> {
    let mut v = vec!["main".to_string()];
    v.extend((0..10).map(|i| format!("subsdk{i}")));
    v.push("sdk".into());
    v
}

/// A module the game loads itself at run time (not mapped at start).
pub fn is_nro(name: &str) -> bool {
    name.starts_with("nro_")
}

/// `order()` then the NRO modules among `names`, sorted.
pub fn order_with<'a>(names: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    let mut v = order();
    let mut nros: Vec<String> = names.into_iter().filter(|n| is_nro(n)).cloned().collect();
    nros.sort();
    v.extend(nros);
    v
}

pub fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

#[derive(Debug, Clone)]
pub struct ModLayout {
    pub name: String,
    pub base: u64,
    pub size: u64,
}

/// Layout for the modules present; `size_of(name)` gives the NSO image size of a present module.
pub fn layout(size_of: &dyn Fn(&str) -> Option<usize>) -> Result<Vec<ModLayout>, String> {
    layout_names(&order(), size_of)
}

/// The same over `names` in their order (`order_with`).
pub fn layout_names(names: &[String], size_of: &dyn Fn(&str) -> Option<usize>) -> Result<Vec<ModLayout>, String> {
    let mut out = vec![];
    let mut base = FIRST_BASE;
    for name in names.iter().cloned() {
        let Some(size) = size_of(&name) else { continue };
        out.push(ModLayout { name: name.clone(), base, size: size as u64 });
        base = align_up(base + size as u64, ALIGN);
    }
    if base > ADDRESS_LIMIT {
        return Err(format!("modules end at {base:#x}, above 2^39"));
    }
    Ok(out)
}

/// layout.json text, as Python's `json.dumps(..., indent=1) + '\n'`.
pub fn layout_json(mods: &[ModLayout]) -> String {
    let mut s = String::from("[");
    for (i, m) in mods.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            "\n {{\n  \"name\": \"{}\",\n  \"base\": \"{:#x}\",\n  \"size\": \"{:#x}\"\n }}",
            m.name, m.base, m.size
        ));
    }
    s.push_str("\n]\n");
    s
}

/// layout.h text for the runtime.
pub fn layout_h(mods: &[ModLayout]) -> String {
    let mut lines = vec![
        "// Module layout chosen by recomp/layout.py. Generated; do not edit.".to_string(),
        "#pragma once".into(),
        String::new(),
    ];
    for m in mods {
        let u = m.name.to_uppercase();
        lines.push(format!("#define MOD_{u}_BASE {:#x}ull", m.base));
        lines.push(format!("#define MOD_{u}_SIZE {:#x}ull", m.size));
    }
    lines.push(String::new());
    lines.push(format!("#define MOD_COUNT {}", mods.len()));
    let list: Vec<String> = mods.iter().map(|m| format!("X({}, {})", m.name, m.name.to_uppercase())).collect();
    lines.push(format!("#define MOD_LIST(X) {}", list.join(" ")));
    lines.push(String::new());
    lines.join("\n")
}
