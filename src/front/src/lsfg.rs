//! Frame generation (LSFG 3.1) with the player's own Lossless Scaling, and the Lossless Scaling app.
//!
//! The shaders belong to THS / Lossless Scaling and are not shipped: they are read at start from the player's
//! `Lossless.dll` (`lsfg_dll`, else beside the game, else every Steam library's `Lossless Scaling` folder), whose
//! RCDATA resources hold them as SPIR-V. The set is identified as Turbo identifies it (the pass order and binding
//! counts come from lsfg-vk, MIT): the mip pass (one input, seven outputs, 32x32 groups), the generation pass (five
//! inputs, two samplers, one output), then the 23-pass fp32 performance and quality sets checked binding by binding,
//! so another Lossless Scaling version is refused with its reason, never run half-matched. The passes run on the
//! present path in graphics (`nxgfx`'s frame-generation hook, docs/INTERFACES.md front ↔ graphics): `lsfg_target`,
//! `lsfg_multiplier`, `lsfg_flow`, `lsfg_resolution`; `fps_target=120` asks for x2 at 120.
//!
//! `lossless_scaling=1` starts the Lossless Scaling app beside the game instead (`lossless_path`, else Steam).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Spv {
    pub id: u32,
    pub words: Vec<u32>,
    pub f16: bool,
    pub samplers: i32,
    pub inputs: i32,
    pub outputs: i32,
    pub local: (u32, u32),
}

/// The identified passes: mip, generate, then the 23 of the chosen set in lsfg-vk's order.
pub struct Set {
    pub dll: PathBuf,
    pub mip: Spv,
    pub generate: Spv,
    pub passes: Vec<Spv>,
    pub quality: bool,
}

static SET: Mutex<Option<std::sync::Arc<Set>>> = Mutex::new(None);
static STATE: Mutex<String> = Mutex::new(String::new());

pub fn set() -> Option<std::sync::Arc<Set>> {
    SET.lock().unwrap().clone()
}
pub fn state() -> String {
    let s = STATE.lock().unwrap().clone();
    if s.is_empty() {
        "off".into()
    } else {
        s
    }
}

/// (samplers, inputs, outputs) of the 23 passes, performance set and quality set (lsfg-vk v1.0).
const SIG_P: [[i32; 3]; 23] = [
    [2, 5, 3], [2, 6, 1], [1, 3, 2], [1, 2, 2], [1, 2, 2], [2, 4, 1], [1, 3, 2], [1, 2, 2], [1, 2, 2], [2, 4, 1], [1, 1, 1], [1, 1, 1],
    [1, 1, 2], [1, 2, 2], [1, 1, 1], [1, 1, 1], [1, 1, 1], [2, 2, 1], [1, 6, 2], [1, 2, 2], [1, 2, 2], [1, 2, 2], [1, 2, 6],
];
const SIG_Q: [[i32; 3]; 23] = [
    [2, 9, 3], [2, 10, 2], [1, 3, 4], [1, 4, 4], [1, 4, 4], [2, 6, 1], [1, 3, 4], [1, 4, 4], [1, 4, 4], [2, 6, 1], [1, 1, 2], [1, 2, 2],
    [1, 2, 4], [1, 4, 4], [1, 2, 2], [1, 2, 2], [1, 2, 2], [2, 3, 1], [1, 12, 2], [1, 2, 2], [1, 2, 2], [1, 2, 2], [1, 2, 6],
];

fn rd16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn rd32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The RCDATA resources (id, bytes) of a 64-bit PE.
pub fn pe_rcdata(d: &[u8]) -> Option<Vec<(u32, Vec<u8>)>> {
    if d.get(..2)? != b"MZ" {
        return None;
    }
    let pe = rd32(d, 0x3c)? as usize;
    if rd32(d, pe)? != 0x4550 {
        return None;
    }
    let nsec = rd16(d, pe + 6)? as usize;
    let optsz = rd16(d, pe + 20)? as usize;
    let opt = pe + 24;
    if rd16(d, opt)? != 0x20b {
        return None;
    }
    let rsrc = rd32(d, opt + 112 + 2 * 8)?;
    let sect = opt + optsz;
    let rva2off = |r: u32| -> Option<usize> {
        for s in 0..nsec {
            let q = sect + s * 40;
            let (vs, va, rs, ra) = (rd32(d, q + 8)?, rd32(d, q + 12)?, rd32(d, q + 16)?, rd32(d, q + 20)?);
            if r >= va && r < va + vs.max(rs) {
                return Some((r - va + ra) as usize);
            }
        }
        None
    };
    let base = rva2off(rsrc)?;
    let dir_entries = |at: usize| -> Option<Vec<(u32, u32)>> {
        let n = rd16(d, at + 12)? as usize + rd16(d, at + 14)? as usize;
        (0..n).map(|i| Some((rd32(d, at + 16 + 8 * i)?, rd32(d, at + 20 + 8 * i)?))).collect()
    };
    let mut out = vec![];
    for (ty, ptr) in dir_entries(base)? {
        if ty != 10 || ptr & 0x8000_0000 == 0 {
            continue;
        }
        for (id, p2) in dir_entries(base + (ptr & 0x7fff_ffff) as usize)? {
            if p2 & 0x8000_0000 == 0 || id & 0x8000_0000 != 0 {
                continue;
            }
            let langs = dir_entries(base + (p2 & 0x7fff_ffff) as usize)?;
            let Some(&(_, de)) = langs.first() else { continue };
            if de & 0x8000_0000 != 0 {
                continue;
            }
            let e = base + de as usize;
            let (rva, sz) = (rd32(d, e)?, rd32(d, e + 4)? as usize);
            let off = rva2off(rva)?;
            if let Some(b) = d.get(off..off + sz) {
                out.push((id, b.to_vec()));
            }
        }
    }
    Some(out)
}

/// What the passes need of a SPIR-V module: bindings by range (16.. samplers, 32.. inputs, 48.. outputs), the local
/// size, Float16.
pub fn spv_info(id: u32, b: &[u8]) -> Option<Spv> {
    if b.len() < 20 || b.len() % 4 != 0 || rd32(b, 0)? != 0x0723_0203 {
        return None;
    }
    let w: Vec<u32> = b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let (mut ms, mut mi, mut mo) = (-1i32, -1i32, -1i32);
    let mut f16 = false;
    let mut local = (0, 0);
    let mut i = 5;
    while i < w.len() {
        let (op, len) = (w[i] & 0xffff, (w[i] >> 16) as usize);
        if len == 0 || i + len > w.len() {
            return None;
        }
        match op {
            17 if len >= 2 && w[i + 1] == 9 => f16 = true,
            16 if len >= 5 && w[i + 2] == 17 => local = (w[i + 3], w[i + 4]),
            71 if len >= 4 && w[i + 2] == 33 => {
                let bnd = w[i + 3] as i32;
                if (16..32).contains(&bnd) {
                    ms = ms.max(bnd - 16);
                } else if (32..48).contains(&bnd) {
                    mi = mi.max(bnd - 32);
                } else if bnd >= 48 {
                    mo = mo.max(bnd - 48);
                }
            }
            54 => break,
            _ => {}
        }
        i += len;
    }
    Some(Spv { id, words: w, f16, samplers: ms + 1, inputs: mi + 1, outputs: mo + 1, local })
}

/// Identifies the LSFG 3.1 set in the DLL's modules.
pub fn identify(dll: &Path, res: Vec<(u32, Vec<u8>)>, quality: bool) -> Result<Set, String> {
    let mut sv: Vec<Spv> = res.iter().filter_map(|(id, b)| spv_info(*id, b)).collect();
    sv.sort_by_key(|s| s.id);
    let m = (0..sv.len().saturating_sub(47))
        .find(|&i| sv[i].inputs == 1 && sv[i].outputs == 7 && sv[i].local.0 == 32 && !(0..48).any(|k| sv[i + k].f16))
        .ok_or_else(|| format!("{} does not contain the LSFG 3.1 shaders ({} SPIR-V modules)", dll.display(), sv.len()))?;
    let gen = &sv[m + 1];
    if gen.inputs != 5 || gen.outputs != 1 || gen.samplers != 2 {
        return Err("the generation shader does not match LSFG 3.1".into());
    }
    let (base, sig) = if quality { (m + 2, &SIG_Q) } else { (m + 25, &SIG_P) };
    for (k, want) in sig.iter().enumerate() {
        let s = sv.get(base + k).ok_or("the pass set is incomplete")?;
        if [s.samplers, s.inputs, s.outputs] != *want {
            return Err(format!("shader {} (pass {k}) does not match LSFG 3.1 ({}/{}/{}): another Lossless Scaling version", s.id, s.samplers, s.inputs, s.outputs));
        }
    }
    Ok(Set { dll: dll.to_path_buf(), mip: sv[m].clone(), generate: gen.clone(), passes: sv[base..base + 23].to_vec(), quality })
}

/// Steam's library folders (the registry's SteamPath, its libraryfolders.vdf, the default places).
pub fn steam_libraries() -> Vec<PathBuf> {
    let mut libs = vec![];
    if let Some(p) = reg_string("Software\\Valve\\Steam", "SteamPath") {
        libs.push(PathBuf::from(p));
    }
    libs.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    libs.push(PathBuf::from(r"C:\Program Files\Steam"));
    if let Ok(vdf) = std::fs::read_to_string(libs[0].join("steamapps").join("libraryfolders.vdf")) {
        for l in vdf.lines() {
            let t = l.trim();
            if let Some(rest) = t.strip_prefix("\"path\"") {
                let v = rest.trim().trim_matches('"').replace("\\\\", "\\");
                libs.push(PathBuf::from(v));
            }
        }
    }
    libs.dedup();
    libs
}

fn reg_string(key: &str, value: &str) -> Option<String> {
    use crate::WideZ;
    use windows_sys::Win32::System::Registry::*;
    let k = key.encode_wide_z();
    let v = value.encode_wide_z();
    let mut buf = [0u16; 600];
    let mut sz = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut sz) };
    if r != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn find_in_steam(file: &str) -> Option<PathBuf> {
    steam_libraries().into_iter().map(|l| l.join("steamapps").join("common").join("Lossless Scaling").join(file)).find(|p| p.is_file())
}

pub fn find_dll() -> Option<PathBuf> {
    let set = nxconfig::string("lsfg_dll");
    if !set.is_empty() && Path::new(&set).is_file() {
        return Some(PathBuf::from(set));
    }
    for n in ["Lossless.dll", "lossless.dll"] {
        let p = crate::game().exe_dir.join(n);
        if p.is_file() {
            return Some(p);
        }
    }
    find_in_steam("Lossless.dll")
}

/// At start: reads and identifies the set when frame generation is wanted.
pub fn start() {
    let (on, mult, target) = nxconfig::lsfg();
    if !on {
        *STATE.lock().unwrap() = "off (lsfg=0)".into();
        return;
    }
    std::thread::Builder::new()
        .name("lsfg".into())
        .spawn(move || {
            let t = std::time::Instant::now();
            let Some(dll) = find_dll() else {
                *STATE.lock().unwrap() = "no Lossless.dll".into();
                crate::flog!("lsfg: no Lossless.dll (copy it from your Lossless Scaling into the game folder, or set lsfg_dll=path in the .ini); no frame generation");
                return;
            };
            let res = match std::fs::read(&dll).ok().and_then(|d| pe_rcdata(&d)) {
                Some(r) => r,
                None => {
                    *STATE.lock().unwrap() = "unreadable Lossless.dll".into();
                    crate::flog!("lsfg: {} could not be read as a Lossless Scaling DLL", dll.display());
                    return;
                }
            };
            match identify(&dll, res, false) {
                Ok(s) => {
                    crate::flog!(
                        "lsfg: LSFG 3.1 shaders from {} (mip {}, generate {}, passes {}..{}, performance set, flow {}, x{mult}, target {target}, height {}) in {:.0} ms; frame generation runs on graphics' present hook",
                        dll.display(),
                        s.mip.id,
                        s.generate.id,
                        s.passes[0].id,
                        s.passes[22].id,
                        nxconfig::f32("lsfg_flow"),
                        nxconfig::int("lsfg_resolution"),
                        t.elapsed().as_secs_f64() * 1000.0
                    );
                    *STATE.lock().unwrap() = format!("ready x{mult}");
                    *SET.lock().unwrap() = Some(std::sync::Arc::new(s));
                }
                Err(e) => {
                    *STATE.lock().unwrap() = "unsupported Lossless.dll".into();
                    crate::flog!("lsfg: {e}");
                }
            }
        })
        .ok();
}

/// `lossless_scaling=1`: starts the Lossless Scaling app (minimised, it captures the game's window) unless it runs.
pub fn launch_app() {
    if !nxconfig::flag("lossless_scaling") {
        return;
    }
    use crate::WideZ;
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
    let title = "Lossless Scaling".encode_wide_z();
    if !unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) }.is_null() {
        crate::flog!("lossless scaling: already running");
        return;
    }
    let set = nxconfig::string("lossless_path");
    let exe = if !set.is_empty() && Path::new(&set).is_file() { Some(PathBuf::from(set)) } else { find_in_steam("LosslessScaling.exe") };
    let Some(exe) = exe else {
        crate::flog!("lossless scaling: not found (install it from Steam, or set lossless_path=C:\\...\\LosslessScaling.exe in the .ini)");
        return;
    };
    match std::process::Command::new(&exe).current_dir(exe.parent().unwrap_or(Path::new("."))).spawn() {
        Ok(_) => crate::flog!("lossless scaling: started ({})", exe.display()),
        Err(e) => crate::flog!("lossless scaling: could not open {}: {e}", exe.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(id: u32, samplers: i32, inputs: i32, outputs: i32, local: u32, f16: bool) -> (u32, Vec<u8>) {
        let mut w = vec![0x0723_0203u32, 0x10000, 0, 100, 0];
        if f16 {
            w.extend([2 << 16 | 17, 9]);
        }
        w.extend([5 << 16 | 16, 1, 17, local, local]);
        let mut var = 10;
        for (base, n) in [(16, samplers), (32, inputs), (48, outputs)] {
            for k in 0..n {
                w.extend([4 << 16 | 71, var, 33, (base + k) as u32]);
                var += 1;
            }
        }
        w.push(1 << 16 | 54);
        (id, w.iter().flat_map(|x| x.to_le_bytes()).collect())
    }

    #[test]
    fn identifies_the_performance_set() {
        let mut res = vec![];
        // an fp16 set first (skipped), then the fp32 one: mip, generate, 23 quality, 23 performance
        res.push(module(100, 1, 1, 7, 32, true));
        let mut id = 200;
        res.push(module(id, 1, 1, 7, 32, false));
        id += 1;
        res.push(module(id, 2, 5, 1, 16, false));
        for s in SIG_Q.iter().chain(SIG_P.iter()) {
            id += 1;
            res.push(module(id, s[0], s[1], s[2], 8, false));
        }
        let set = identify(Path::new("x.dll"), res.clone(), false).unwrap();
        assert_eq!((set.mip.id, set.generate.id, set.passes[0].id), (200, 201, 225));
        let q = identify(Path::new("x.dll"), res.clone(), true).unwrap();
        assert_eq!(q.passes[0].id, 202);
        res.pop();
        assert!(identify(Path::new("x.dll"), res, false).is_err(), "a short set is refused");
    }
}
