//! nn::mii: the Mii database (the six default Miis every console has, and Miis the editor stand-in adds) and the
//! MiiModel system data stand-in. Mario Kart 8 Deluxe initializes a Database at boot, lists the six defaults (and
//! never reads the count) and builds random Miis for its title screen's demo race; their heads come from the
//! MiiModel archive (0100000000000802), which is firmware content: the host writes files of the same layout in
//! which every texture part is one blank 8x4 texture and every shape one degenerate triangle, so heads build and
//! draw nothing.

use crate::guest::{w32, Args};
use crate::hle::{Registry, RES_OK};
use nxrt::cpu::Cpu;
use std::path::PathBuf;
use std::sync::Mutex;

const RES_NOT_FOUND: u64 = 0x87E;
const RES_CANCELED: u64 = 0xE7E;
const RES_TEST_MODE: u64 = 0x1987E;
const CHARINFO: usize = 0x58;
const DEFAULTS: usize = 6;
const MAX: usize = 100;

static DB: Mutex<Vec<[u8; CHARINFO]>> = Mutex::new(Vec::new());
static CHANGED: Mutex<bool> = Mutex::new(false);

fn entropy() -> u64 {
    let mut x = nxrt::rt::now_ns() ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^ (x >> 29)
}

/// The system's default Mii (index ≥ 0: default Mii <index>, a fixed create ID; < 0: a fresh random one).
pub fn default_mii(gender: u32, index: i32) -> [u8; CHARINFO] {
    let mut c = [0u8; CHARINFO];
    let (a, b) = if index >= 0 { (0x4d49_494e_4445_4641u64 + index as u64, 0x554c_5444_4d48_4f53u64) } else { (entropy(), entropy()) };
    c[0..8].copy_from_slice(&a.to_le_bytes());
    c[8..16].copy_from_slice(&b.to_le_bytes());
    c[6] = (c[6] & 0x0f) | 0x40;
    c[8] = (c[8] & 0x3f) | 0x80;
    for (i, ch) in "no name".bytes().enumerate() {
        c[16 + 2 * i] = ch;
    }
    // byte offsets after create_id[16] and nickname[22]: 0x26 font_region, favorite_color, gender, height, build, type,
    // region_move, faceline type/color/wrinkle/make, hair type/color/flip, eye 7, eyebrow 7, nose 3, mouth 5, beard 2,
    // mustache 3, glass 4, mole 4, reserved
    let f = |c: &mut [u8; CHARINFO], off: usize, v: u8| c[off] = v;
    if index >= 0 {
        f(&mut c, 0x27, (index * 2) as u8);
    }
    f(&mut c, 0x28, (gender == 1) as u8);
    f(&mut c, 0x29, 64);
    f(&mut c, 0x2A, 64);
    f(&mut c, 0x31, 33); // hair type
    f(&mut c, 0x32, 1); // hair color
    for (o, v) in [(0x34, 2), (0x35, 8), (0x36, 4), (0x37, 3), (0x38, 4), (0x39, 2), (0x3A, 12)] {
        f(&mut c, o, v); // eye
    }
    for (o, v) in [(0x3B, 6), (0x3C, 1), (0x3D, 4), (0x3E, 3), (0x3F, 6), (0x40, 2), (0x41, 10)] {
        f(&mut c, o, v); // eyebrow
    }
    for (o, v) in [(0x42, 1), (0x43, 4), (0x44, 9)] {
        f(&mut c, o, v); // nose
    }
    for (o, v) in [(0x45, 23), (0x46, 19), (0x47, 4), (0x48, 3), (0x49, 13)] {
        f(&mut c, o, v); // mouth
    }
    f(&mut c, 0x4A, 8); // beard color
    f(&mut c, 0x4D, 4); // mustache scale
    f(&mut c, 0x4E, 10); // mustache y
    f(&mut c, 0x50, 8); // glass color
    f(&mut c, 0x51, 4);
    f(&mut c, 0x52, 10);
    f(&mut c, 0x54, 4); // mole scale
    f(&mut c, 0x55, 2);
    f(&mut c, 0x56, 20);
    c
}

fn get(buf: u64, n: i32, src: u32, stride: u64) -> u32 {
    let db = DB.lock().unwrap();
    let mut k = 0u32;
    if src & 1 != 0 {
        for m in db.iter() {
            if buf == 0 || k as i32 >= n {
                break;
            }
            let e = buf + k as u64 * stride;
            crate::guest::write_bytes(e, m);
            if stride > CHARINFO as u64 {
                w32(e + CHARINFO as u64, 0);
            }
            k += 1;
        }
    }
    if src & 2 != 0 {
        for d in 0..DEFAULTS {
            if buf == 0 || k as i32 >= n {
                break;
            }
            let e = buf + k as u64 * stride;
            crate::guest::write_bytes(e, &default_mii((d >= 3) as u32, d as i32));
            if stride > CHARINFO as u64 {
                w32(e + CHARINFO as u64, 1);
            }
            k += 1;
        }
    }
    k
}

macro_rules! miifn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

miifn!(init, |a| {
    crate::log_once!("mii init", "mii: Database::Initialize (the six default Miis, {} added)", DB.lock().unwrap().len());
    RES_OK
});
miifn!(fin, |a| { 0 });
miifn!(avail, |a| { 1 });
miifn!(updated, |a| {
    if a.w(1) & 1 == 0 {
        return 0;
    }
    let mut g = CHANGED.lock().unwrap();
    let r = *g;
    *g = false;
    r as u64
});
miifn!(full, |a| { (DB.lock().unwrap().len() >= MAX) as u64 });
miifn!(count, |a| {
    let src = a.w(1);
    (if src & 1 != 0 { DB.lock().unwrap().len() } else { 0 } + if src & 2 != 0 { DEFAULTS } else { 0 }) as u64
});
miifn!(get_e, |a| {
    let n = get(a.x(2), a.i(3), a.w(4), CHARINFO as u64 + 4);
    w32(a.x(1), n);
    RES_OK
});
miifn!(get_c, |a| {
    let n = get(a.x(2), a.i(3), a.w(4), CHARINFO as u64);
    w32(a.x(1), n);
    RES_OK
});
miifn!(index, |a| {
    let id = crate::guest::bytes(a.x(2), 16).to_vec();
    match DB.lock().unwrap().iter().position(|m| m[..16] == id[..]) {
        Some(i) => {
            w32(a.x(1), i as u32);
            RES_OK
        }
        None => {
            w32(a.x(1), u32::MAX);
            RES_NOT_FOUND
        }
    }
});
miifn!(latest, |a| {
    let info = a.x(1);
    let id = crate::guest::bytes(info, 16).to_vec();
    match DB.lock().unwrap().iter().find(|m| m[..16] == id[..]) {
        Some(m) if a.w(2) & 1 != 0 => {
            crate::guest::write_bytes(info, m);
            RES_OK
        }
        _ => RES_NOT_FOUND,
    }
});
miifn!(random, |a| {
    let desc = a.x(2);
    let gender = if desc != 0 { crate::guest::r32(desc + 4) } else { 0 };
    crate::guest::write_bytes(a.x(1), &default_mii(gender, -1));
    RES_OK
});
miifn!(build_default, |a| {
    let i = a.i(2);
    let i = if (0..DEFAULTS as i32).contains(&i) { i } else { 0 };
    crate::guest::write_bytes(a.x(1), &default_mii((i >= 3) as u32, i));
    RES_OK
});
miifn!(ok, |a| { RES_OK });
miifn!(test_mode, |a| { RES_TEST_MODE });
miifn!(append, |a| {
    let mut db = DB.lock().unwrap();
    if db.len() >= MAX {
        return RES_CANCELED;
    }
    db.push(default_mii(0, -1));
    *CHANGED.lock().unwrap() = true;
    crate::guest::out(a.x(0), (db.len() - 1) as u32);
    crate::log!("mii: AppendMii: the default Mii is added as database entry {}", db.len() - 1);
    RES_OK
});

// ---- the MiiModel stand-in

const TEX_PARTS: usize = 327;
const TEX_SIZE: usize = 0x64 + 0x1c * TEX_PARTS;
const SHAPE_PARTS: usize = 857;
const MESH_AT: usize = 0x14de0;
const MESH: usize = 0x20;
const TEXELS_AT: usize = 0x2430;

/// zlib: one stored block of 32 zero bytes, with its Adler-32.
fn mesh_z() -> Vec<u8> {
    let mut v = vec![0x78, 0x01, 0x01, MESH as u8, 0, !(MESH as u8), 0xff];
    v.extend(std::iter::repeat_n(0u8, MESH));
    v.extend([0x00, MESH as u8, 0x00, 0x01]);
    v
}

fn standin_file(parts: usize, size: usize) -> Vec<u8> {
    let mut b = vec![0u8; size];
    let z = mesh_z();
    for i in 0..parts {
        b[0xc + 4 * i..0x10 + 4 * i].copy_from_slice(&0x1000u32.to_le_bytes());
        b[0xc + 4 * parts + 4 * i..0x10 + 4 * parts + 4 * i].copy_from_slice(&0x1000u32.to_le_bytes());
    }
    let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    if parts == 11 {
        for i in 0..TEX_PARTS {
            let e = 0x64 + 0x1c * i;
            put(&mut b, e, TEXELS_AT as u32);
            put(&mut b, e + 4, MESH as u32);
            put(&mut b, e + 8, z.len() as u32);
            b[e + 0x14..e + 0x16].copy_from_slice(&8u16.to_le_bytes());
            b[e + 0x16..e + 0x18].copy_from_slice(&4u16.to_le_bytes());
            b[e + 0x18] = 0;
            b[e + 0x19] = 1;
            b[e + 0x1a] = 0;
        }
        b[TEXELS_AT..TEXELS_AT + z.len()].copy_from_slice(&z);
    } else {
        for i in 0..SHAPE_PARTS {
            let e = 0x6c + 0x58 * i;
            put(&mut b, e, MESH_AT as u32);
            put(&mut b, e + 4, MESH as u32);
            put(&mut b, e + 8, z.len() as u32);
            put(&mut b, e + 4 * 9, 8);
            put(&mut b, e + 4 * 14, 0x10);
            put(&mut b, e + 4 * 15, 6);
        }
        b[MESH_AT..MESH_AT + z.len()].copy_from_slice(&z);
    }
    b
}

/// The folder a system data archive is served from, when the host has a stand-in for it.
pub fn system_data(id: u64) -> Option<PathBuf> {
    if id != 0x0100_0000_0000_0802 && id != 0x0400_0000_0000_0849 {
        return None;
    }
    static DONE: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut g = DONE.lock().unwrap();
    if let Some(p) = g.as_ref() {
        return Some(p.clone());
    }
    let dir = crate::services::fs::save_dir().join("system").join("mii-standin");
    std::fs::create_dir_all(&dir).ok()?;
    let tex_size = TEXELS_AT + mesh_z().len();
    let shape_size = MESH_AT + mesh_z().len();
    let _ = TEX_SIZE;
    for n in ["NXTextureLowLinear.dat", "NXTextureLowSRGB.dat", "NXTextureMidLinear.dat", "NXTextureMidSRGB.dat"] {
        std::fs::write(dir.join(n), standin_file(11, tex_size)).ok()?;
    }
    for n in ["ShapeMid.dat", "ShapeHigh.dat"] {
        std::fs::write(dir.join(n), standin_file(12, shape_size)).ok()?;
    }
    crate::log!("mii: Mii model system data is the host's stand-in (Mii heads draw nothing) in {}", dir.display());
    *g = Some(dir.clone());
    Some(dir)
}

pub fn register(r: &mut Registry) {
    const W: &str = "mii:u: the host's database (the six default Miis)";
    for (n, f) in [
        ("_ZN2nn3mii8Database10InitializeEv", init as unsafe fn(*mut Cpu)),
        ("_ZN2nn3mii8Database8FinalizeEv", fin),
        ("_ZNK2nn3mii8Database11IsAvailableEv", avail),
        ("_ZN2nn3mii8Database9IsUpdatedEi", updated),
        ("_ZNK2nn3mii8Database14IsFullDatabaseEv", full),
        ("_ZNK2nn3mii8Database8GetCountEi", count),
        ("_ZN2nn3mii8Database3GetEPiPNS0_15CharInfoElementEii", get_e),
        ("_ZN2nn3mii8Database3GetEPiPNS0_8CharInfoEii", get_c),
        ("_ZNK2nn3mii8Database8GetIndexEPiRKNS0_8CharInfoE", index),
        ("_ZNK2nn3mii8Database12UpdateLatestEPNS0_8CharInfoEi", latest),
        ("_ZN2nn3mii8Database11BuildRandomEPNS0_8CharInfoERKNS0_10RandomDescE", random),
        ("_ZNK2nn3mii8Database12BuildDefaultEPNS0_8CharInfoEi", build_default),
        ("_ZN2nn3mii8Database6FormatEv", ok),
        ("_ZN2nn3mii8Database6ImportEPKvm", test_mode),
        ("_ZNK2nn3mii8Database6ExportEPvm", test_mode),
        ("_ZN2nn3mii9AppendMiiEPi", append),
        ("_ZN2nn3mii11ShowMiiEditEv", ok),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_standin() {
        let a = default_mii(0, 2);
        assert_eq!(a, default_mii(0, 2), "a default Mii keeps its create ID");
        assert_ne!(default_mii(0, -1)[..16], default_mii(0, -1)[..16]);
        assert_eq!(a[6] >> 4, 4, "UUID version 4");
        let mut buf = vec![0u8; 6 * (CHARINFO + 4)];
        assert_eq!(get(buf.as_mut_ptr() as u64, 6, 2, CHARINFO as u64 + 4), 6);
        assert_eq!(buf[CHARINFO], 1, "source: default");
        let t = standin_file(11, TEXELS_AT + mesh_z().len());
        assert_eq!(u32::from_le_bytes(t[0x64..0x68].try_into().unwrap()), TEXELS_AT as u32);
        assert!(TEXELS_AT >= TEX_SIZE);
    }
}
