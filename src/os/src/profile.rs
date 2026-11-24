//! The per-title facts the os layer needs (nxnative's game profile `game.tsv`, the parts the OS uses): program id,
//! memory figures, the counter rate and the cores. Read from `<data dir>/profile.tsv` when present (the
//! `game.tsv` format: `key<TAB>value` lines, `#` comments), else from the built-in table, else defaults for a
//! Switch 1 title. The title is identified by the program id in `main.npdm`.

use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub program_id: u64,
    /// GetInfo TotalMemorySize (`app_pool`).
    pub pool: u64,
    /// Memory in use besides the heap (`mem_used`, 64 MB by default).
    pub used_beside_heap: u64,
    /// NPDM system resource (`sys_resource`): virtual address memory when not 0.
    pub sys_resource: u64,
    /// The system counter rate: 19.2 MHz (Switch 1) or 31.25 MHz (Switch 2).
    pub tick_hz: u32,
    /// GetInfo CoreMask: cores 0-2 on Switch 1.
    pub core_mask: u64,
    pub switch2: bool,
    /// A map past the application pool fails with ResultLimitReached (`mem_map_limit on`).
    pub map_limit: bool,
    /// The present interval the profile forces (5A.6: `present_interval display` = every vsync, or 1..3), 0 = the
    /// game's own.
    pub present_interval: i32,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            name: "unknown title".into(),
            program_id: 0x0100_0000_0000_1000,
            pool: 0xCD50_0000,
            used_beside_heap: 64 << 20,
            sys_resource: 0,
            tick_hz: 19_200_000,
            core_mask: 0x7,
            switch2: false,
            map_limit: false,
            present_interval: 0,
        }
    }
}

/// Built-in rows (nxnative `dm/games/<title id>/game.tsv`).
const BUILTIN: &[(u64, &str)] = &[(
    0x0100_1520_0002_2000,
    "name\tMario Kart 8 Deluxe\napp_pool\t0xCD500000\nsys_resource\t0x0\n",
)];

/// A profile's `present_interval`: `display` forces every vsync (1), a number 1..3 that interval, `game` (or
/// anything else) leaves the game's own.
fn present_interval(v: &str) -> i32 {
    match v.trim() {
        "display" => 1,
        n => n
            .parse::<i32>()
            .ok()
            .filter(|n| (1..=3).contains(n))
            .unwrap_or(0),
    }
}

fn num(v: &str) -> Option<u64> {
    let v = v.trim();
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

impl Profile {
    pub fn apply(&mut self, tsv: &str) {
        for line in tsv.lines() {
            let line = line.trim_end();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('\t') else {
                continue;
            };
            match k {
                "name" => self.name = v.trim().to_string(),
                "program_id" => {
                    self.program_id = u64::from_str_radix(v.trim(), 16).unwrap_or(self.program_id)
                }
                "app_pool" => self.pool = num(v).unwrap_or(self.pool),
                "mem_used" => self.used_beside_heap = num(v).unwrap_or(self.used_beside_heap),
                "sys_resource" => self.sys_resource = num(v).unwrap_or(self.sys_resource),
                "tick_hz" => self.tick_hz = num(v).unwrap_or(self.tick_hz as u64) as u32,
                "core_mask" => self.core_mask = num(v).unwrap_or(self.core_mask),
                "mem_map_limit" => self.map_limit = v.trim() == "on",
                "present_interval" => self.present_interval = present_interval(v),
                "switch2" => {
                    self.switch2 = v.trim() == "1";
                    if self.switch2 {
                        self.tick_hz = 31_250_000;
                    }
                }
                _ => {}
            }
        }
    }

    /// The title's facts from the engine's profiles (nxnative's), and the platform from its title id: a Switch 2
    /// program (04...) counts at 31.25 MHz and gets the runtime's application pool (`nxrt::process::app_pool`).
    fn from_profiles(&mut self, id: u64) {
        let program = nxrt::process::Program { program_id: id };
        if let Some(pr) = profiles::by_title(id) {
            self.name = pr.name.clone();
            if pr.memory.app_pool != 0 {
                self.pool = pr.memory.app_pool;
            }
            if pr.memory.used != 0 {
                self.used_beside_heap = pr.memory.used;
            }
            self.sys_resource = pr.memory.sys_resource;
            self.map_limit = pr.memory.map_limit;
            self.present_interval = present_interval(&pr.settings.present_interval);
        }
        if program.switch2() {
            self.switch2 = true;
            self.tick_hz = 31_250_000;
            if profiles::by_title(id).is_none_or(|pr| pr.memory.app_pool == 0) {
                self.pool = nxrt::process::app_pool(program);
            }
        }
    }

    /// The profile for a program, from its NPDM and the data folder's override.
    pub fn load(npdm: Option<&[u8]>, override_tsv: Option<&str>) -> Profile {
        let mut p = Profile::default();
        // the NPDM's SystemResourceSize (META +0x14): virtual address memory for a title no profile describes
        // (Anime Girls: Sun of a Beach, Beach Bounce Remastered, Sonic X Shadow Generations reserve address regions
        // in their static initialisers); a profile's value replaces it
        if let Some(n) = npdm
            .filter(|n| n.get(..4) == Some(&b"META"[..]))
            .and_then(|n| n.get(0x14..0x18))
        {
            p.sys_resource = u32::from_le_bytes(n.try_into().unwrap()) as u64;
        }
        if let Some(id) = npdm.and_then(npdm_program_id) {
            p.program_id = id;
            if let Some((_, row)) = BUILTIN.iter().find(|(t, _)| *t == id) {
                p.apply(row);
            } else {
                p.from_profiles(id);
            }
        }
        // a Switch 2 program's cores are its NPDM's (ThreadInfo: Mario Kart World's are 0-5, GetInfo CoreMask 0x3f as
        // mkw-recomp's kernel answers); a Switch 1 program keeps cores 0-2
        if p.switch2 {
            if let Some(m) = npdm.and_then(npdm_core_mask) {
                p.core_mask = m;
            }
        }
        if let Some(t) = override_tsv {
            p.apply(t);
        }
        p
    }
}

/// The core mask of an NPDM's ThreadInfo kernel capability (ACI0, low bits 0b0111: lowest and highest core at bits
/// 16-23 and 24-31).
pub fn npdm_core_mask(npdm: &[u8]) -> Option<u64> {
    let rd = |o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(npdm.get(o..o + 4)?.try_into().ok()?))
    };
    let aci = rd(0x70)? as usize;
    if npdm.get(aci..aci + 4)? != b"ACI0" {
        return None;
    }
    let (off, size) = (aci + rd(aci + 0x30)? as usize, rd(aci + 0x34)? as usize);
    for i in 0..size / 4 {
        let c = rd(off + 4 * i)?;
        if c & 0xF == 0x7 {
            let (lo, hi) = ((c >> 16) & 0xFF, (c >> 24) & 0xFF);
            if lo <= hi && hi < 8 {
                return Some(((1u64 << (hi + 1)) - 1) & !((1u64 << lo) - 1));
            }
        }
    }
    None
}

/// The program id of an NPDM (META → ACI0 at +0x70, its program id at +0x10).
pub fn npdm_program_id(npdm: &[u8]) -> Option<u64> {
    if npdm.get(0..4)? != b"META" {
        return None;
    }
    let aci = u32::from_le_bytes(npdm.get(0x70..0x74)?.try_into().ok()?) as usize;
    if npdm.get(aci..aci + 4)? != b"ACI0" {
        return None;
    }
    Some(u64::from_le_bytes(
        npdm.get(aci + 0x10..aci + 0x18)?.try_into().ok()?,
    ))
}

static PROFILE: OnceLock<Profile> = OnceLock::new();

pub fn set(p: Profile) -> &'static Profile {
    let _ = PROFILE.set(p);
    get()
}

pub fn get() -> &'static Profile {
    PROFILE.get_or_init(Profile::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_npdm() {
        let mut npdm = vec![0u8; 0x200];
        npdm[0..4].copy_from_slice(b"META");
        npdm[0x70..0x74].copy_from_slice(&0x100u32.to_le_bytes());
        npdm[0x100..0x104].copy_from_slice(b"ACI0");
        npdm[0x110..0x118].copy_from_slice(&0x0100_1520_0002_2000u64.to_le_bytes());
        let p = Profile::load(Some(&npdm), Some("# comment\nmem_used\t0x8000000\n"));
        assert_eq!(p.program_id, 0x0100_1520_0002_2000);
        assert_eq!(p.name, "Mario Kart 8 Deluxe");
        assert_eq!(p.pool, 0xCD50_0000);
        assert_eq!(p.used_beside_heap, 0x800_0000);
        assert_eq!(p.tick_hz, 19_200_000);
        npdm[0x14..0x18].copy_from_slice(&0x100_0000u32.to_le_bytes());
        npdm[0x110..0x118].copy_from_slice(&0x0100_C680_1E0F_A000u64.to_le_bytes());
        assert_eq!(Profile::load(Some(&npdm), None).sys_resource, 0x100_0000);
        let s2 = Profile::load(None, Some("switch2\t1\n"));
        assert_eq!(s2.tick_hz, 31_250_000);
    }

    /// 5A.6: the profile's `present_interval` reaches graphics' forced interval (`set_profile_interval`) when the
    /// ini leaves it to the profile (`present_interval=0`), and the ini's own value wins over it.
    #[test]
    fn present_interval_reaches_graphics() {
        assert_eq!(
            Profile::load(None, Some("present_interval\tdisplay\n")).present_interval,
            1
        );
        assert_eq!(
            Profile::load(None, Some("present_interval\t2\n")).present_interval,
            2
        );
        assert_eq!(
            Profile::load(None, Some("present_interval\tgame\n")).present_interval,
            0
        );
        assert_eq!(
            Profile::load(None, None).present_interval,
            0,
            "off by default"
        );
        let p = Profile::load(None, Some("present_interval\tdisplay\n"));
        crate::nvn::apply_present_interval(&p);
        assert_eq!(nxgfx::nvn::window::forced_interval(), Some(1));
        assert!(nxconfig::apply_arg("--present_interval=2"));
        assert_eq!(
            nxgfx::nvn::window::forced_interval(),
            Some(2),
            "the ini wins"
        );
        assert!(nxconfig::apply_arg("--present_interval=0"));
        crate::nvn::apply_present_interval(&Profile::default());
        assert_eq!(nxgfx::nvn::window::forced_interval(), None);
    }
}
