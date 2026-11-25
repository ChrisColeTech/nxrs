//! Game profiles: nxnative's `dm/games` as data, and the owner's patches for this engine, looked up by title id and
//! module build id.
//!
//! A profile is one game (folder = the base game's title id): its builds (version, display version, module build
//! ids), its modules, its guest memory layout, its per-game settings, and its patch tables. The data is embedded
//! at compile time (`build.rs`); `tools/export.py` imports it from an nxnative checkout. Lookups:
//!
//! - [`lookup`] (title id + main build id) → the profile and, when the build is registered, its build row;
//! - [`by_title`], [`by_build`], [`get`] (folder id), [`all`];
//! - [`patches_for`]: every patch table entry keyed by a module build id, whichever profile holds it (nxnative's
//!   rewriter loads every profile's tables the same way);
//! - [`translator_patches`]: the translator's patch-table text for the entries nxrs applies at build time.

pub mod cheatfile;
pub mod docs;
pub mod mods;
pub mod patches;

pub use patches::PatchFile;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub struct RawProfile {
    pub id: &'static str,
    pub game_tsv: &'static str,
    /// `games/<id>/engine.tsv`: keys the engine's lanes keep by hand (results of their runs), over game.tsv's.
    pub engine_tsv: Option<&'static str>,
    pub patches: Option<&'static str>,
    /// `games/<id>/owner.toml`: the owner's patches, kept by hand
    pub owner: Option<&'static str>,
    /// `games/<id>/recomp.txt`: a static recompilation's code patch table (the translator's format) for one build
    pub recomp: Option<&'static str>,
    /// nxnative's shader tables, zstd-compressed (`tools/export_shaders.py`)
    pub shaders: Option<&'static [u8]>,
}

mod data {
    use super::RawProfile;
    include!(concat!(env!("OUT_DIR"), "/data.rs"));
}

/// Which console the program was built for: it decides the counter frequency the game reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Switch,
    Switch2,
}

impl Platform {
    /// CNTFRQ_EL0 / nn::os::GetSystemTickFrequency: 19.2 MHz on the Switch, 31.25 MHz on the Switch 2 (the Switch 2
    /// SDK's constant).
    pub fn tick_hz(self) -> u64 {
        match self {
            Platform::Switch => 19_200_000,
            Platform::Switch2 => 31_250_000,
        }
    }

    /// Switch 2 application ids start 04; Switch ids 01.
    pub fn of_title(title_id: u64) -> Platform {
        if title_id >> 56 == 0x04 {
            Platform::Switch2
        } else {
            Platform::Switch
        }
    }
}

/// One `build` row: a version the game was brought up on.
#[derive(Debug, Clone, PartialEq)]
pub struct Build {
    /// "v1507328"
    pub version: String,
    /// 1507328
    pub version_num: Option<u32>,
    /// "4.0.1"
    pub display: String,
    /// (module, build id as lowercase hex)
    pub modules: Vec<(String, String)>,
}

impl Build {
    pub fn module(&self, name: &str) -> Option<&str> {
        self.modules
            .iter()
            .find(|(m, _)| m == name)
            .map(|(_, b)| b.as_str())
    }
}

/// The guest address-space partition (nxnative `hle_svc.c`, `memory.md`): what the memory lane reserves.
#[derive(Debug, Clone, PartialEq)]
pub struct Memory {
    pub reserve: u64,
    pub alias: u64,
    pub stack: u64,
    pub app_pool: u64,
    /// GetInfo(UsedMemorySize) beside the heap (default 64 MB)
    pub used: u64,
    /// the NPDM's system resource size, GetInfo(TotalSystemResourceSize) (default 32 MB)
    pub sys_resource: u64,
    /// StandardAllocator's address space with virtual address memory (0: what the alias region has room for)
    pub stdalloc_cap: u64,
    /// MapPhysicalMemory refused past the application pool, as on Horizon
    pub map_limit: bool,
}

/// The per-game settings of `game.tsv`, each with nxnative's default and checked as `game.py header` checks them.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// 0 = the game's own step; 30 / 60
    pub fps_target: u32,
    /// 60 or 120
    pub fps_max: u32,
    pub fps_hold: bool,
    /// game (as the game sets it) or display
    pub present_interval: String,
    /// console (docked) or handheld
    pub op_mode: String,
    /// follow or boost
    pub perf_mode: String,
    pub core_preempt: bool,
    pub alloc_big_align: bool,
    /// bfttf or ttf; None: decided by the SDK version
    pub shared_font: Option<String>,
    /// NVN API "major.minor"; None: decided by the SDK version
    pub device_api: Option<String>,
    /// gl or zero
    pub depth_mode: String,
    /// simple, full or touch
    pub pad_layout: String,
    pub game_args: String,
    pub game_env: String,
    pub launch_param: String,
    pub screen_aspect: bool,
    /// (docked, handheld) base size of the 3D views
    pub render_base: Option<((u32, u32), (u32, u32))>,
    pub ue_texture_pool: u32,
    pub ue_fps_cvar: String,
    pub draw_barriers: String,
    pub tex_flush_upload: bool,
    pub tex_dirty_upload: bool,
    pub tex_dyn_slots: bool,
    pub vconv_range: bool,
    pub present_snap: bool,
    pub enc_thread: bool,
    pub pso_mtl4: bool,
    /// (offset, length) of control-section bytes shader keys hash as zeros
    pub shader_key_skip: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct Profile {
    /// The folder: the base game's title id, 16 hex digits
    pub id: String,
    pub title_id: u64,
    pub program_id: u64,
    pub name: String,
    pub builds: Vec<Build>,
    /// The version the profile's tables were made on
    pub package: Option<String>,
    /// Code modules loaded, in load order
    pub modules: Vec<String>,
    pub romfs_marker: String,
    pub memory: Memory,
    pub settings: Settings,
    /// A game of several programs: (program index, profile id) of the others
    pub programs: Vec<(u32, String)>,
    /// In the profile of such a program: the first program's profile
    pub program_of: Option<String>,
    /// nxnative's patches.py of this game, as data
    pub patches: Option<PatchFile>,
    /// The owner's patches for this game (`owner.toml`), applied by nxrs builds
    pub owner: Option<PatchFile>,
    /// A code patch table in the translator's own format (`recomp.txt`, ported from a static recompilation such as
    /// mkw-recomp), applied by nxrs builds of the module build it names
    pub recomp: Option<RecompTable>,
    /// Every row in file order (keys such as `build` repeat)
    pub rows: Vec<(String, Vec<String>)>,
}

fn int(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let r = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(h, 16)
    } else {
        s.parse()
    };
    r.map_err(|_| format!("not a number: {s:?}"))
}

fn on_off(v: &str, key: &str) -> Result<bool, String> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("{key} must be on or off: {v}")),
    }
}

fn one_of(v: String, key: &str, allowed: &[&str]) -> Result<String, String> {
    if allowed.contains(&v.as_str()) {
        Ok(v)
    } else {
        Err(format!("{key} must be {}: {v}", allowed.join(" or ")))
    }
}

impl Profile {
    /// Parses a game.tsv (`key<TAB>values...`, `#` comments) and checks it as nxnative's game.py does.
    pub fn parse(id: &str, text: &str) -> Result<Profile, String> {
        let mut rows = vec![];
        for line in text.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let mut c = line.split('\t');
            let k = c.next().unwrap().trim().to_string();
            rows.push((k, c.map(|v| v.trim().to_string()).collect::<Vec<_>>()));
        }
        let p = Profile {
            id: id.to_string(),
            title_id: 0,
            program_id: 0,
            name: String::new(),
            builds: vec![],
            package: None,
            modules: vec![],
            romfs_marker: String::new(),
            memory: Memory {
                reserve: 0,
                alias: 0,
                stack: 0,
                app_pool: 0,
                used: 0,
                sys_resource: 0,
                stdalloc_cap: 0,
                map_limit: false,
            },
            settings: Settings::default_values(),
            programs: vec![],
            program_of: None,
            patches: None,
            owner: None,
            recomp: None,
            rows,
        };
        let need = |k: &str| {
            p.get(k)
                .ok_or_else(|| format!("{id}: game.tsv has no key {k}"))
        };
        let title_id =
            u64::from_str_radix(&need("title_id")?, 16).map_err(|_| format!("{id}: title_id"))?;
        let program_id = u64::from_str_radix(&need("program_id")?, 16)
            .map_err(|_| format!("{id}: program_id"))?;
        let name = need("name")?;
        let romfs_marker = need("romfs_marker")?;
        let mut builds = vec![];
        for v in p.values("build") {
            if v.len() < 2 {
                return Err(format!(
                    "{id}: a build row needs a version and a display version"
                ));
            }
            let mut modules = vec![];
            for m in &v[2..] {
                let (k, b) = m
                    .split_once('=')
                    .ok_or_else(|| format!("{id}: build row module {m:?} is not name=build id"))?;
                if !b.chars().all(|c| c.is_ascii_hexdigit()) || b.len() < 32 {
                    return Err(format!("{id}: build id {b:?} of {k}"));
                }
                modules.push((k.to_string(), b.to_ascii_lowercase()));
            }
            builds.push(Build {
                version: v[0].clone(),
                version_num: v[0].strip_prefix('v').and_then(|n| n.parse().ok()),
                display: v[1].clone(),
                modules,
            });
        }
        let mem = |k: &str| -> Result<u64, String> {
            int(&need(k)?).map_err(|e| format!("{id}: {k}: {e}"))
        };
        let mem_or = |k: &str, d: u64| -> Result<u64, String> {
            p.get(k)
                .map_or(Ok(d), |v| int(&v).map_err(|e| format!("{id}: {k}: {e}")))
        };
        let memory = Memory {
            reserve: mem("mem_reserve")?,
            alias: mem("mem_alias")?,
            stack: mem("mem_stack")?,
            app_pool: mem("app_pool")?,
            used: mem_or("mem_used", 0x400_0000)?,
            sys_resource: mem_or("sys_resource", 0x200_0000)?,
            stdalloc_cap: mem_or("stdalloc_cap", 0)?,
            map_limit: on_off(&p.get_or("mem_map_limit", "off"), "mem_map_limit")
                .map_err(|e| format!("{id}: {e}"))?,
        };
        let settings = Settings::from(&p).map_err(|e| format!("{id}: {e}"))?;
        let mut programs = vec![];
        for pair in p.words("programs").unwrap_or_default() {
            let (n, other) = pair
                .split_once('=')
                .ok_or_else(|| format!("{id}: programs entry {pair:?}"))?;
            programs.push((
                n.parse()
                    .map_err(|_| format!("{id}: programs index {n:?}"))?,
                other.to_string(),
            ));
        }
        let modules = p.words("modules").unwrap_or_else(|| vec!["main".into()]);
        let (package, program_of) = (p.get("package"), p.get("program_of"));
        Ok(Profile {
            title_id,
            program_id,
            name,
            romfs_marker,
            builds,
            memory,
            settings,
            programs,
            modules,
            package,
            program_of,
            ..p
        })
    }

    /// Every row of a key (game.py `values`).
    pub fn values(&self, key: &str) -> Vec<&Vec<String>> {
        self.rows
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
            .collect()
    }

    /// The first row of a key, its values joined by tabs (game.py `get`).
    pub fn get(&self, key: &str) -> Option<String> {
        self.rows
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.join("\t"))
    }

    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.get(key).unwrap_or_else(|| default.to_string())
    }

    pub fn words(&self, key: &str) -> Option<Vec<String>> {
        self.get(key)
            .map(|v| v.split_whitespace().map(String::from).collect())
    }

    /// Switch 2 when the profile's title id is one (04…).
    pub fn platform(&self) -> Platform {
        Platform::of_title(self.title_id)
    }

    /// The counter frequency the game reads.
    pub fn tick_hz(&self) -> u64 {
        self.platform().tick_hz()
    }

    /// The registered build whose module `module` has this build id.
    pub fn build_with(&self, module: &str, build_id: &str) -> Option<&Build> {
        self.builds.iter().find(|b| {
            b.module(module)
                .is_some_and(|k| patches::build_matches(k, build_id))
        })
    }

    /// The modules a build loads: its build row's, in the profile's load order, else the profile's list.
    pub fn modules_of(&self, build: Option<&Build>) -> Vec<String> {
        match build {
            Some(b) if b.module("main").is_some() => self
                .modules
                .iter()
                .filter(|m| b.module(m).is_some())
                .cloned()
                .collect(),
            _ => self.modules.clone(),
        }
    }

    /// Both patch files of the profile.
    pub fn patch_files(&self) -> impl Iterator<Item = &PatchFile> {
        self.patches.iter().chain(self.owner.iter())
    }
}

impl Settings {
    fn default_values() -> Settings {
        Settings {
            fps_target: 0,
            fps_max: 60,
            fps_hold: true,
            present_interval: "game".into(),
            op_mode: "console".into(),
            perf_mode: "follow".into(),
            core_preempt: false,
            alloc_big_align: false,
            shared_font: None,
            device_api: None,
            depth_mode: "gl".into(),
            pad_layout: "simple".into(),
            game_args: String::new(),
            game_env: String::new(),
            launch_param: String::new(),
            screen_aspect: false,
            render_base: None,
            ue_texture_pool: 0,
            ue_fps_cvar: "t.MaxFPS".into(),
            draw_barriers: "full".into(),
            tex_flush_upload: false,
            tex_dirty_upload: false,
            tex_dyn_slots: false,
            vconv_range: false,
            present_snap: false,
            enc_thread: false,
            pso_mtl4: false,
            shader_key_skip: None,
        }
    }

    fn from(p: &Profile) -> Result<Settings, String> {
        let g = |k: &str, d: &str| p.get_or(k, d);
        let b = |k: &str, d: &str| on_off(&g(k, d), k);
        let fps_target: u32 = match g("fps_target", "0").as_str() {
            v @ ("0" | "30" | "60") => v.parse().unwrap(),
            v => return Err(format!("fps_target must be 0, 30 or 60: {v}")),
        };
        let fps_max: u32 = match g("fps_max", "60").as_str() {
            v @ ("60" | "120") => v.parse().unwrap(),
            v => return Err(format!("fps_max must be 60 or 120: {v}")),
        };
        let utp = g("ue_texture_pool", "0");
        let ufc = g("ue_fps_cvar", "t.MaxFPS");
        if ufc.is_empty()
            || ufc.len() > 31
            || !ufc.chars().next().unwrap().is_ascii_alphabetic()
            || !ufc
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            return Err(format!(
                "ue_fps_cvar must be a console variable name: {ufc}"
            ));
        }
        let api = p.get("device_api");
        if let Some(a) = &api {
            let ok = a.split_once('.').is_some_and(|(x, y)| {
                !x.is_empty()
                    && !y.is_empty()
                    && x.chars().chain(y.chars()).all(|c| c.is_ascii_digit())
            });
            if !ok {
                return Err(format!("device_api must be major.minor: {a}"));
            }
        }
        let render_base = match p.get("render_base") {
            None => None,
            Some(v) => {
                let dims: Vec<(u32, u32)> = v
                    .split_whitespace()
                    .map(|d| {
                        d.split_once('x')
                            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    })
                    .collect::<Option<_>>()
                    .ok_or_else(|| format!("render_base must be <w>x<h> <w>x<h>: {v}"))?;
                if dims.len() != 2 {
                    return Err(format!("render_base must be <w>x<h> <w>x<h>: {v}"));
                }
                Some((dims[0], dims[1]))
            }
        };
        let shader_key_skip = match p.get("shader_key_skip") {
            None => None,
            Some(v) => {
                let (o, l) = v
                    .split_once('+')
                    .ok_or_else(|| format!("shader_key_skip takes <offset>+<length>: {v}"))?;
                let (o, l) = (int(o)? as u32, int(l)? as u32);
                if o < 0x30 || l == 0 || o + l > 0x880 || (o < 0x760 && o + l > 0x700) {
                    return Err(format!("shader_key_skip out of bounds: {v}"));
                }
                Some((o, l))
            }
        };
        let s = Settings {
            fps_target,
            fps_max,
            fps_hold: b("fps_hold", "on")?,
            present_interval: one_of(
                g("present_interval", "game"),
                "present_interval",
                &["game", "display"],
            )?,
            op_mode: one_of(g("op_mode", "console"), "op_mode", &["console", "handheld"])?,
            perf_mode: one_of(g("perf_mode", "follow"), "perf_mode", &["follow", "boost"])?,
            core_preempt: b("core_preempt", "off")?,
            alloc_big_align: b("alloc_big_align", "off")?,
            shared_font: p
                .get("shared_font")
                .map(|v| one_of(v, "shared_font", &["bfttf", "ttf"]))
                .transpose()?,
            device_api: api,
            depth_mode: one_of(g("depth_mode", "gl"), "depth_mode", &["gl", "zero"])?,
            pad_layout: one_of(
                g("pad_layout", "simple"),
                "pad_layout",
                &["simple", "full", "touch"],
            )?,
            game_args: g("game_args", ""),
            game_env: g("game_env", ""),
            launch_param: g("launch_param", ""),
            screen_aspect: b("screen_aspect", "off")?,
            render_base,
            ue_texture_pool: utp
                .parse()
                .map_err(|_| format!("ue_texture_pool must be megabytes: {utp}"))?,
            ue_fps_cvar: ufc,
            draw_barriers: one_of(
                g("draw_barriers", "full"),
                "draw_barriers",
                &["full", "game"],
            )?,
            tex_flush_upload: b("tex_flush_upload", "off")?,
            tex_dirty_upload: b("tex_dirty_upload", "off")?,
            tex_dyn_slots: b("tex_dyn_slots", "off")?,
            vconv_range: b("vconv_range", "off")?,
            present_snap: b("present_snap", "off")?,
            enc_thread: b("enc_thread", "off")?,
            pso_mtl4: b("pso_mtl4", "off")?,
            shader_key_skip,
        };
        if s.tex_dirty_upload && !s.tex_flush_upload {
            return Err("tex_dirty_upload needs tex_flush_upload on".into());
        }
        if s.pad_layout == "touch" && s.op_mode != "handheld" {
            return Err("pad_layout touch needs op_mode handheld".into());
        }
        Ok(s)
    }
}

/// A row of the player-facing settings list (`shared/settings.tsv`, nxnative's settings.tsv; its header has the columns).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSetting {
    pub variable: String,
    pub kind: String,
    pub values: String,
    pub default: String,
    pub shown: String,
    pub section: String,
    pub label: String,
    pub note: String,
    pub meta: String,
}

pub fn player_settings() -> Vec<PlayerSetting> {
    data::SETTINGS_TSV
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            let f = |i: usize| c.get(i).copied().unwrap_or("").to_string();
            PlayerSetting {
                variable: f(0),
                kind: f(1),
                values: f(2),
                default: f(3),
                shown: f(4),
                section: f(5),
                label: f(6),
                note: f(7),
                meta: f(8),
            }
        })
        .collect()
}

/// The nxnative commit the data came from.
pub fn source() -> &'static str {
    data::SOURCE.trim()
}

struct Db {
    profiles: Vec<Profile>,
    errors: Vec<String>,
    sdk: PatchFile,
    engine: PatchFile,
}

fn db() -> &'static Db {
    static DB: OnceLock<Db> = OnceLock::new();
    DB.get_or_init(|| {
        let mut profiles = vec![];
        let mut errors = vec![];
        for r in data::GAMES {
            // engine.tsv's rows come first: a key takes its first row
            let text = match r.engine_tsv {
                Some(e) => format!(
                    "{e}
{}",
                    r.game_tsv
                ),
                None => r.game_tsv.to_string(),
            };
            let parsed = Profile::parse(r.id, &text).and_then(|mut p| {
                p.patches = r
                    .patches
                    .map(PatchFile::parse)
                    .transpose()
                    .map_err(|e| format!("{}: patches.toml: {e}", r.id))?;
                p.owner = r
                    .owner
                    .map(PatchFile::parse)
                    .transpose()
                    .map_err(|e| format!("{}: owner.toml: {e}", r.id))?;
                p.recomp = r
                    .recomp
                    .map(RecompTable::parse)
                    .transpose()
                    .map_err(|e| format!("{}: recomp.txt: {e}", r.id))?;
                Ok(p)
            });
            match parsed {
                Ok(p) => profiles.push(p),
                Err(e) => errors.push(e),
            }
        }
        let mut shared = |name: &str, t: &str| {
            PatchFile::parse(t).unwrap_or_else(|e| {
                errors.push(format!("shared/{name}: {e}"));
                PatchFile::default()
            })
        };
        let sdk = shared("sdk_patches.toml", data::SDK_PATCHES);
        let engine = shared("engine_patches.toml", data::ENGINE_PATCHES);
        Db {
            profiles,
            errors,
            sdk,
            engine,
        }
    })
}

/// nxnative's shader tables of a profile (`games/<id>/shaders.tsv.zst`), each file byte for byte: `pso_manifest.tsv`
/// (the prewarm seed: `S <key> <stage>`, `R <Metal pipeline key>`, `C <key>`), `pso_pairs.tsv` and
/// `shader_pairs.tsv` (vertex / pixel key pairs), `shader_facts.tsv` and `shader_facts_manual.tsv` (answers to the
/// shader translator's queries). Empty when the profile has none.
pub fn shader_tables(id: &str) -> Result<Vec<(String, String)>, String> {
    let Some(raw) = data::GAMES
        .iter()
        .find(|r| r.id.eq_ignore_ascii_case(id))
        .and_then(|r| r.shaders)
    else {
        return Ok(vec![]);
    };
    let text = zstd::stream::decode_all(raw).map_err(|e| format!("{id}: shaders.tsv.zst: {e}"))?;
    let text = String::from_utf8(text).map_err(|_| format!("{id}: shaders.tsv.zst: not UTF-8"))?;
    let mut out: Vec<(String, String)> = vec![];
    for line in text.split_inclusive('\n') {
        if let Some(name) = line
            .strip_prefix("## ")
            .filter(|n| n.trim_end().ends_with(".tsv"))
        {
            out.push((name.trim_end().to_string(), String::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push_str(line);
        } else {
            return Err(format!("{id}: shaders.tsv.zst: no table header"));
        }
    }
    Ok(out)
}

/// The prewarm seed of a profile's `pso_manifest.tsv`: shader keys in the order a play-through used them, with the
/// stage digit (`S` rows) or `c` for compute (`C` rows). nxnative's render pipeline keys (`R` rows) are Metal's and
/// are left out.
pub fn prewarm_shaders(id: &str) -> Result<Vec<(String, String)>, String> {
    let tables = shader_tables(id)?;
    let Some((_, m)) = tables.iter().find(|(n, _)| n == "pso_manifest.tsv") else {
        return Ok(vec![]);
    };
    Ok(m.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            match f[..] {
                ["S", key, stage, ..] => Some((key.to_string(), stage.to_string())),
                ["C", key, ..] => Some((key.to_string(), "c".to_string())),
                _ => None,
            }
        })
        .collect())
}

/// Every profile that parsed, by folder id.
pub fn all() -> &'static [Profile] {
    &db().profiles
}

/// How many profiles are embedded, and the errors of those that did not parse (none expected).
pub fn check() -> (usize, &'static [String]) {
    (data::GAMES.len(), &db().errors)
}

/// The shared patch tables: Nintendo SDK module builds, and middleware code (any game's module).
pub fn shared_patches() -> [&'static PatchFile; 2] {
    [&db().sdk, &db().engine]
}

pub fn get(id: &str) -> Option<&'static Profile> {
    all().iter().find(|p| p.id.eq_ignore_ascii_case(id))
}

/// The profile of a title: its title or program id, else (an update's 0x800, an add-on's id) the base game's.
pub fn by_title(title_id: u64) -> Option<&'static Profile> {
    let exact = all()
        .iter()
        .find(|p| p.title_id == title_id || p.program_id == title_id);
    exact.or_else(|| {
        let base = title_id & !0xFFF;
        all().iter().find(|p| p.title_id == base).or_else(|| {
            all().iter().find(|p| {
                p.title_id == base.wrapping_sub(0x1000)
                    && title_id & 0xFFF != 0
                    && title_id & 0x1000 != 0
            })
        })
    })
}

/// The profile and build row that register this main build id (a release of the same build under another id too).
pub fn by_build(main_build_id: &str) -> Option<(&'static Profile, &'static Build)> {
    all()
        .iter()
        .find_map(|p| p.build_with("main", main_build_id).map(|b| (p, b)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// The main build is one of the profile's build rows: the profile holds as it is.
    Build,
    /// Only the title matched: another version than the profile was made on (nxnative derives a profile then).
    Title,
}

#[derive(Debug, Clone, Copy)]
pub struct Match {
    pub profile: &'static Profile,
    pub build: Option<&'static Build>,
    pub how: How,
}

impl Match {
    pub fn tick_hz(&self) -> u64 {
        self.profile.tick_hz()
    }
}

/// The profile of a program: by its main build id first (exact), else by title id.
pub fn lookup(title_id: Option<u64>, main_build_id: Option<&str>) -> Option<Match> {
    if let Some(bid) = main_build_id {
        let by_tid = title_id.and_then(by_title);
        if let Some(p) = by_tid {
            if let Some(b) = p.build_with("main", bid) {
                return Some(Match {
                    profile: p,
                    build: Some(b),
                    how: How::Build,
                });
            }
        }
        if let Some((p, b)) = by_build(bid) {
            return Some(Match {
                profile: p,
                build: Some(b),
                how: How::Build,
            });
        }
    }
    title_id.and_then(by_title).map(|p| Match {
        profile: p,
        build: None,
        how: How::Title,
    })
}

/// A patch-table entry that names a module build.
#[derive(Debug, Clone, Copy)]
pub enum Entry<'a> {
    Cut(&'a patches::Cut),
    Obs(&'a patches::Obs),
    Stub(&'a patches::Stub),
    Capability(&'a patches::Capability),
}

/// Every entry keyed by this module build id, from every profile and the shared tables, with the file it is in.
pub fn patches_for(build_id: &str) -> Vec<(&'static str, Entry<'static>)> {
    let mut out = vec![];
    let files = all()
        .iter()
        .flat_map(|p| p.patch_files())
        .chain(shared_patches());
    for f in files {
        let src = f.source.as_str();
        out.extend(
            f.cut
                .iter()
                .filter(|c| patches::build_matches(&c.build, build_id))
                .map(|c| (src, Entry::Cut(c))),
        );
        out.extend(
            f.obs
                .iter()
                .filter(|c| patches::build_matches(&c.build, build_id))
                .map(|c| (src, Entry::Obs(c))),
        );
        out.extend(
            f.stub
                .iter()
                .filter(|c| patches::build_matches(&c.build, build_id))
                .map(|c| (src, Entry::Stub(c))),
        );
        out.extend(
            f.capability
                .iter()
                .filter(|c| patches::build_matches(&c.build, build_id))
                .map(|c| (src, Entry::Capability(c))),
        );
    }
    out
}

/// `games/<id>/recomp.txt`: a code patch table in the translator's own format (`set`, `site`, `span`, `spin`), for
/// the module build its `build <module> <build id>` line names (the line is not passed on).
#[derive(Debug, Clone, PartialEq)]
pub struct RecompTable {
    pub module: String,
    pub build: String,
    /// The table without its `build` line
    pub text: String,
}

impl RecompTable {
    pub fn parse(text: &str) -> Result<RecompTable, String> {
        let mut key = None;
        let mut body = String::new();
        for line in text.lines() {
            let f: Vec<&str> = line
                .split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            if f.first() == Some(&"build") {
                if f.len() != 3 || key.is_some() {
                    return Err(format!("bad or second build line: {line:?}"));
                }
                key = Some((f[1].to_string(), f[2].to_ascii_lowercase()));
                continue;
            }
            body.push_str(line);
            body.push('\n');
        }
        let (module, build) = key.ok_or("no `build <module> <build id>` line")?;
        if build.len() < 32 {
            return Err(format!("build id {build} is shorter than 32 digits"));
        }
        Ok(RecompTable {
            module,
            build,
            text: body,
        })
    }

    /// (sets, sites and spans, spin waits) as the translator counts them
    pub fn counts(&self) -> (usize, usize, usize) {
        let n = |k: &str| {
            self.text
                .lines()
                .filter(|l| l.split_whitespace().next() == Some(k))
                .count()
        };
        (n("set"), n("site") + n("span"), n("spin"))
    }
}

/// The translator's patch table (src/translator `aot/patches.rs` format) for the cuts nxrs applies at build time
/// (`apply = "build"`) to these modules (name, build id): one `set` per patch set, then its `site` lines. Empty
/// when nothing applies. A cut that is not whole words is an error: the translator patches instructions.
pub fn translator_patches(modules: &[(&str, &str)]) -> Result<(String, Vec<String>), String> {
    let mut sets: BTreeMap<String, String> = BTreeMap::new();
    let mut sites = vec![];
    let mut said = vec![];
    let files = all()
        .iter()
        .flat_map(|p| p.patch_files())
        .chain(shared_patches());
    for f in files {
        for c in f.cut.iter().filter(|c| c.apply.as_deref() == Some("build")) {
            for (m, bid) in modules {
                if c.module.as_deref().is_some_and(|cm| cm != *m)
                    || !patches::build_matches(&c.build, bid)
                {
                    continue;
                }
                let (Some(o), Some(r)) = (
                    patches::words_of(&c.original),
                    patches::words_of(&c.replacement),
                ) else {
                    return Err(format!(
                        "{}: the cut at {m}+{:#x} is not whole instructions",
                        f.source, c.offset
                    ));
                };
                if o.len() != r.len() || c.offset % 4 != 0 {
                    return Err(format!(
                        "{}: the cut at {m}+{:#x} changes its length or is misaligned",
                        f.source, c.offset
                    ));
                }
                let set = c.set.clone().unwrap_or_else(|| "profile".into());
                sets.entry(set.clone()).or_insert_with(|| {
                    c.env.clone().unwrap_or_else(|| "NXRS_PATCH_PROFILE".into())
                });
                for (i, (ow, rw)) in o.iter().zip(&r).enumerate() {
                    sites.push(format!(
                        "site {set} {m} {:#x} {ow:08x} {rw:08x}  # {}",
                        c.offset + 4 * i as u64,
                        c.reason.lines().next().unwrap_or("")
                    ));
                }
                said.push(c.reason.clone());
            }
        }
    }
    let mut tables = vec![];
    for p in all() {
        let Some(t) = &p.recomp else { continue };
        if modules
            .iter()
            .any(|(m, bid)| *m == t.module && patches::build_matches(&t.build, bid))
        {
            let (k, n, spins) = t.counts();
            said.push(format!(
                "{}: {k} code patch sets, {n} sites, {spins} spin waits (recomp.txt)",
                p.name
            ));
            tables.push(t.text.as_str());
        }
    }
    if sites.is_empty() && tables.is_empty() {
        return Ok((String::new(), said));
    }
    let mut s = String::from(
        "# Written by nxrs from the game's profile: the patches applied at build time.\n",
    );
    for (name, env) in &sets {
        s.push_str(&format!("set {name} {env} 1\n"));
    }
    for l in sites {
        s.push_str(&l);
        s.push('\n');
    }
    for t in tables {
        s.push_str(t);
    }
    Ok((s, said))
}

#[cfg(test)]
mod tests;
