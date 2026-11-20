//! Host functions that game patches call, keyed by the patched module's build id. A profile's code patch table
//! (`games/<id>/recomp.txt` or `turbo.toml` cuts, applied by the translator at build time) puts a `bl` to the patch
//! page after `main`; `install` makes those addresses run the functions here (`nxrt::hostfn::alias`).
//!
//! - **Mario Kart World 1.8.0** (main build id d00e4f399e411d06): mkw-recomp's game layer (MIT): its patch sets
//!   (`gamefix/mkw.rs`: render-buffer pool, camera, seed, output size, frame rate; `gamefix/mkw_zstd.rs`: zstd on
//!   the host; `gamefix/mkw_view.rs`: wider and taller outputs; `gamefix/mkw_romfs.rs`: the internal resolution's RomFS
//!   overlay).

mod mkw;
mod mkw_romfs;
mod mkw_view;
mod mkw_zstd;

/// After the modules are loaded: the host functions of the patches that apply to these modules.
pub fn install(loaded: &[nxrt::loader::Loaded]) {
    mkw::install(loaded);
}

/// The value of a patch set's switch as the game's code reads it (`set <name> <env> <default>`: an empty or missing
/// variable takes the default, `0` turns it off, anything else on), so a host function knows whether its set runs.
pub(crate) fn set_on(env: &str, default: bool) -> bool {
    match std::env::var(env) {
        Ok(v) if !v.is_empty() => !v.starts_with('0'),
        _ => default,
    }
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn env_str(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}
