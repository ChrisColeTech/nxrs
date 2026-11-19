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
        }
    }
}

/// Built-in rows (nxnative `dm/games/<title id>/game.tsv`).
const BUILTIN: &[(u64, &str)] = &[(
    0x0100_1520_0002_2000,
    "name\tMario Kart 8 Deluxe\napp_pool\t0xCD500000\nsys_resource\t0x0\n",
)];

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
            let Some((k, v)) = line.split_once('\t') else { continue };
            match k {
                "name" => self.name = v.trim().to_string(),
                "program_id" => self.program_id = u64::from_str_radix(v.trim(), 16).unwrap_or(self.program_id),
                "app_pool" => self.pool = num(v).unwrap_or(self.pool),
                "mem_used" => self.used_beside_heap = num(v).unwrap_or(self.used_beside_heap),
                "sys_resource" => self.sys_resource = num(v).unwrap_or(self.sys_resource),
                "tick_hz" => self.tick_hz = num(v).unwrap_or(self.tick_hz as u64) as u32,
                "core_mask" => self.core_mask = num(v).unwrap_or(self.core_mask),
                "mem_map_limit" => self.map_limit = v.trim() == "on",
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
        if let Some(id) = npdm.and_then(npdm_program_id) {
            p.program_id = id;
            if let Some((_, row)) = BUILTIN.iter().find(|(t, _)| *t == id) {
                p.apply(row);
            } else {
                p.from_profiles(id);
            }
        }
        if let Some(t) = override_tsv {
            p.apply(t);
        }
        p
    }
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
    Some(u64::from_le_bytes(npdm.get(aci + 0x10..aci + 0x18)?.try_into().ok()?))
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
        let s2 = Profile::load(None, Some("switch2\t1\n"));
        assert_eq!(s2.tick_hz, 31_250_000);
    }
}
