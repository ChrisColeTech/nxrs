//! Guest address layout of the loaded modules, computed from their sizes.
//! Modules are placed in Horizon's load order (rtld dropped):
//! main, subsdk0..9, sdk; each base aligned to 2 MiB from 0x7000000000, below 2^39. NRO modules the game loads
//! at run time (`nro_<index>`, `package::nro`) follow, in name order: their place is fixed at build time and the
//! runtime maps each there when the game loads it.

pub const FIRST_BASE: u64 = 0x70_0000_0000;
/// The web build's first base (docs/35): 7.25 GiB, so the compact guest window around the modules (stack and heap
/// block, alias region, the SDK's maps below; the kernel region above) lies inside wasm64's 16 GiB linear memory and a
/// guest address is a linear-memory address. The window's bottom (the modules less 7 GiB: slack, 6 GiB alias, stack
/// and heap) is then 256 MiB, right above the runtime heap's start (`-sINITIAL_MEMORY=256MB`): at 8 GiB the 768 MiB
/// between the heap's break and the window were claimed and never used, and Chrome on Windows commits every byte the
/// linear memory grows over (WEB-WMEM).
pub const WEB_FIRST_BASE: u64 = 0x1_D000_0000;
/// The web engine library's interface with the page (`src/web`: the host files, the JS libraries, the shared-memory
/// blocks) and with this layout. The library carries it as bytes (`os::web::HOST_PROTOCOL`); `nxrs-build --target
/// web` refuses a library without the one it was built with (WEB-W10-MK8D: a library from before the I/O worker was
/// linked with a newer page, and every game died at its first read). Bump the number when that interface changes.
pub const WEB_HOST_PROTOCOL: &str = "nxrs-web-host-protocol=2;";
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
pub fn layout_names(
    names: &[String],
    size_of: &dyn Fn(&str) -> Option<usize>,
) -> Result<Vec<ModLayout>, String> {
    layout_names_at(names, size_of, FIRST_BASE)
}

/// `layout_names` from another first base (`WEB_FIRST_BASE` for a web build).
pub fn layout_names_at(
    names: &[String],
    size_of: &dyn Fn(&str) -> Option<usize>,
    first: u64,
) -> Result<Vec<ModLayout>, String> {
    let mut out = vec![];
    let mut base = first;
    for name in names.iter().cloned() {
        let Some(size) = size_of(&name) else { continue };
        out.push(ModLayout {
            name: name.clone(),
            base,
            size: size as u64,
        });
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
    let list: Vec<String> = mods
        .iter()
        .map(|m| format!("X({}, {})", m.name, m.name.to_uppercase()))
        .collect();
    lines.push(format!("#define MOD_LIST(X) {}", list.join(" ")));
    lines.push(String::new());
    lines.join("\n")
}
