//! Programs and shader keys.
//!
//! A stage's key is the first 8 bytes of sha256(control section bytes + code area), computed at SetShaders: NVN lets
//! the caller free the control section once SetShaders returns. NVN: the code area's length at control +0x704, the
//! control bytes in the key are its own length at +0x20 when that is in [0x760, 0x880), else 0x880. NVN2 (magic
//! 0x98761234, version 0xc9): the code length at +0x44, the first 0x48 control bytes with +0x30 hashed as zero.
//! The shader library (lane C) stores each translated stage under this key.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

pub type ShaderKey = [u8; 8];

pub fn key_hex(k: &ShaderKey) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

const NVN2_KEY_CTL: usize = 0x48;

/// # Safety
/// `c` points at a control section of at least 0x48 (NVN2) or 0x708 bytes.
unsafe fn nvn2_ctl(c: *const u8) -> bool {
    let magic = core::ptr::read_unaligned(c as *const u32);
    let ver = core::ptr::read_unaligned(c.add(4) as *const u32);
    magic == 0x98761234 && ver == 0xc9
}

/// The control section's bytes in the key and the code area's length.
/// # Safety
/// As `nvn2_ctl`.
pub unsafe fn ctl_lens(c: *const u8) -> (usize, usize) {
    if nvn2_ctl(c) {
        return (NVN2_KEY_CTL, core::ptr::read_unaligned(c.add(0x44) as *const u32) as usize);
    }
    let ncode = core::ptr::read_unaligned(c.add(0x704) as *const u32) as usize;
    let own = core::ptr::read_unaligned(c.add(0x20) as *const u32) as usize;
    let nctl = if (0x760..0x880).contains(&own) { own } else { 0x880 };
    (nctl, ncode)
}

/// The key of one stage; None when the code area is not readable (`need_mapped`: not inside a known memory pool).
/// # Safety
/// The control section and, when in a pool, the code area are readable.
pub unsafe fn shader_key(sd: &NVNshaderData, need_mapped: bool) -> Option<ShaderKey> {
    if sd.control.is_null() {
        return None;
    }
    let (nctl, ncode) = ctl_lens(sd.control);
    let code = sd.data != 0 && ncode != 0 && ncode < (1 << 24);
    if need_mapped && !(code && crate::vk::mem::addr_known(sd.data, ncode as u64)) {
        return None;
    }
    let mut h = Sha256::new();
    if nvn2_ctl(sd.control) {
        let mut ctl = [0u8; NVN2_KEY_CTL];
        core::ptr::copy_nonoverlapping(sd.control, ctl.as_mut_ptr(), NVN2_KEY_CTL);
        ctl[0x30..0x34].fill(0);
        h.update(ctl);
    } else {
        h.update(core::slice::from_raw_parts(sd.control, nctl));
    }
    if code {
        h.update(core::slice::from_raw_parts(sd.data as *const u8, ncode));
    }
    let d = h.finalize();
    let mut k = [0u8; 8];
    k.copy_from_slice(&d[..8]);
    Some(k)
}

#[derive(Default, Clone)]
struct ProgKeys {
    key: [Option<ShaderKey>; 6],
    /// the control bytes of a stage keyed later (its code area was in no pool yet at SetShaders)
    ctl: [Option<Vec<u8>>; 6],
}
static KEYS: Mutex<Option<HashMap<u64, ProgKeys>>> = Mutex::new(None);

/// The keys of a program's stages, by its serial (a game may copy an NVNprogram after SetShaders and draw with the copy).
/// # Safety
/// `p` is a program the game initialized.
pub unsafe fn prog_keys(p: &prog_t) -> Vec<ShaderKey> {
    let mut out = Vec::new();
    let mut g = KEYS.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    let rec = map.entry(p.serial).or_default();
    for i in 0..(p.nstage as usize).min(6) {
        if rec.key[i].is_none() {
            let mut sd = p.sd[i];
            if let Some(c) = &rec.ctl[i] {
                sd.control = c.as_ptr();
            }
            rec.key[i] = shader_key(&sd, false);
            if let Some(k) = rec.key[i] {
                if sd.data != 0 && crate::vk::mem::addr_known(sd.data, ctl_lens(sd.control).1 as u64) {
                    live_if_missing(&k, &sd);
                }
            }
        }
        if let Some(k) = rec.key[i] {
            out.push(k);
        }
    }
    out
}

/// A stage the shader library lacks goes to the live translator (its control and code copied: the game may free the
/// control section once SetShaders returns).
/// # Safety
/// `sd`'s control section and code area are readable.
unsafe fn live_if_missing(k: &ShaderKey, sd: &NVNshaderData) {
    let Some(g) = crate::vk::Gpu::try_get() else { return };
    if g.shaders.has(k) || sd.control.is_null() || sd.data == 0 {
        return;
    }
    let (nctl, ncode) = ctl_lens(sd.control);
    let ctl = core::slice::from_raw_parts(sd.control, nctl).to_vec();
    let code = core::slice::from_raw_parts(sd.data as *const u8, ncode).to_vec();
    crate::vk::live::request(*k, ctl, code);
}

nvn_fns! {
    fn nvnProgramInitialize(p: *mut NVNprogram, d: *mut NVNdevice) -> NVNboolean {
        let _ = d;
        core::ptr::write_bytes(p as *mut u8, 0, core::mem::size_of::<prog_t>());
        (*(p as *mut prog_t)).magic = MAGIC_PROG;
        1
    }
    fn nvnProgramFinalize(p: *mut NVNprogram) {
        let o = &mut *(p as *mut prog_t);
        crate::vk::Gpu::with(|g| g.prog_forget(o.serial));
        o.magic = 0;
    }
    fn nvnProgramSetDebugLabel(p: *mut NVNprogram, s: *const u8) { let _ = (p, s); }
    fn nvnProgramSetShaders(p: *mut NVNprogram, n: i32, sd: *const NVNshaderData) -> NVNboolean {
        if !(0..=6).contains(&n) {
            crate::log::red(&format!("ProgramSetShaders stage count {n}"));
            return 0;
        }
        let o = &mut *(p as *mut prog_t);
        if o.serial != 0 {
            crate::vk::Gpu::with(|g| g.prog_forget(o.serial));
        }
        o.nstage = n as u32;
        for i in 0..n as usize {
            o.sd[i] = *sd.add(i);
        }
        o.serial = super::new_serial();
        let mut rec = ProgKeys::default();
        for i in 0..n as usize {
            let s = &*sd.add(i);
            rec.key[i] = shader_key(s, true);
            if let Some(k) = rec.key[i] {
                live_if_missing(&k, s);
            }
            if rec.key[i].is_none() && !s.control.is_null() {
                // nctl covers what ctl_lens reads again (+0x704 / +0x44)
                let (nctl, _) = ctl_lens(s.control);
                rec.ctl[i] = Some(core::slice::from_raw_parts(s.control, nctl).to_vec());
            }
        }
        // its pipeline-library parts made ahead (vk::pipe::prebuild)
        if let Some(g) = crate::vk::Gpu::try_get() {
            crate::vk::pipe::prebuild(g, rec.key.iter().flatten().copied().collect());
        }
        KEYS.lock().unwrap().get_or_insert_with(HashMap::new).insert(o.serial, rec);
        super::prog2::note_program(o.serial, core::slice::from_raw_parts(sd, n as usize));
        1
    }
}
