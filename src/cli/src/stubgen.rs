//! Stubs whose code is assembled from what they read in the module (nxnative's FRAME_STEP / STUBS generators that
//! call `ctx.address` or an anchored rule's find, so the exporter could not record their words): each generator
//! ported, run at build time against the game's main, giving the same record an evaluated stub has (code,
//! relocations, asserted sites, puts) for `profile::stubs` to place.
//!
//! The context mirrors nxnative's (`patch_nso.py`): `here` / `emit` assemble into the patch area, `site` asserts a
//! site's original bytes, `put` writes a site (a branch into the area), `address` is the data address an adrp and
//! the add / load / store after it make. Addresses are module offsets; the area's are `AREA` + offset, the state
//! page `STATE`, so a word that points outside the area carries a relocation.

use profiles::patches::{Put, Reloc, Site, StubBody};
use std::collections::BTreeMap;

pub const AREA: u64 = 1 << 40;
pub const STATE: u64 = 1 << 41;

pub struct Ctx<'a> {
    pub img: &'a [u8],
    pub base: u64,
    pub others: BTreeMap<String, u64>,
    pub body: StubBody,
}

fn reloc(index: usize, op: &str, target: &str, addend: i64) -> Reloc {
    Reloc {
        index,
        op: op.into(),
        target: target.into(),
        addend,
    }
}

impl<'a> Ctx<'a> {
    pub fn new(img: &'a [u8], base: u64, others: BTreeMap<String, u64>) -> Ctx<'a> {
        Ctx {
            img,
            base,
            others,
            body: StubBody {
                evaluated: true,
                ..Default::default()
            },
        }
    }
    pub fn here(&self) -> u64 {
        AREA + 4 * self.body.code.len() as u64
    }
    pub fn emit(&mut self, w: u32) {
        self.body.code.push(w);
    }
    fn word(&self, off: u64) -> Result<u32, String> {
        self.img
            .get(off as usize..off as usize + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or(format!("{off:#x}: outside the module"))
    }
    /// adrp x<rd> of `to`'s page (a module offset, `STATE` or an area address)
    pub fn adrp(&mut self, rd: u32, to: u64) {
        let i = self.body.code.len();
        let (t, a) = if to >= STATE {
            ("state", 0)
        } else if to >= AREA {
            ("area", ((to - AREA) & !0xFFF) as i64)
        } else {
            ("module", (to & !0xFFF) as i64)
        };
        self.body.relocs.push(reloc(i, "adrp", t, a));
        self.emit(0x9000_0000 | rd);
    }
    fn branch(&mut self, op: u32, name: &str, to: u64) {
        let i = self.body.code.len();
        if to >= AREA && to < STATE {
            let d = (to as i64 - self.here() as i64) >> 2;
            self.emit(op | (d as u32 & 0x3FF_FFFF));
        } else {
            self.body.relocs.push(reloc(i, name, "module", to as i64));
            self.emit(op);
        }
    }
    pub fn b(&mut self, to: u64) {
        self.branch(0x1400_0000, "b", to)
    }
    pub fn bl(&mut self, to: u64) {
        self.branch(0x9400_0000, "bl", to)
    }
    /// cbnz w<t>, an area address
    pub fn cbnzw(&mut self, t: u32, to: u64) {
        let d = (to as i64 - self.here() as i64) >> 2;
        self.emit(0x3500_0000 | ((d as u32 & 0x7FFFF) << 5) | t);
    }
    pub fn site(&mut self, off: u64, original: &str) -> Result<(), String> {
        let want: Vec<u8> = (0..original.len() / 2)
            .map(|i| u8::from_str_radix(&original[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let have = self
            .img
            .get(off as usize..off as usize + want.len())
            .ok_or(format!("site {off:#x} outside the module"))?;
        if have != want.as_slice() {
            return Err(format!("site {off:#x} differs from {original}"));
        }
        self.body.sites.push(Site {
            base: "module".into(),
            offset: off as i64,
            original: original.into(),
        });
        Ok(())
    }
    /// One word at `off`: a branch to the area address `start`.
    pub fn put_b(&mut self, off: u64, start: u64, reason: &str) {
        self.body.puts.push(Put {
            base: "module".into(),
            offset: off as i64,
            words: vec![0x1400_0000],
            reason: reason.into(),
            relocs: vec![reloc(0, "b", "area", (start - AREA) as i64)],
        });
    }
    /// The data address the adrp at `off` and the add / load / store at `lo_off` (default `off + 4`) make.
    pub fn address(&self, off: u64, lo_off: Option<u64>) -> Result<u64, String> {
        let lo_off = lo_off.unwrap_or(off + 4);
        let (a, b) = (self.word(off)?, self.word(lo_off)?);
        if a & 0x9F00_0000 != 0x9000_0000 {
            return Err(format!("no adrp at {off:#x}"));
        }
        let mut imm = (((((a >> 5) & 0x7FFFF) << 2) | ((a >> 29) & 3)) as i64) << 12;
        if imm & (1 << 32) != 0 {
            imm -= 1 << 33;
        }
        let page = ((off & !0xFFF) as i64 + imm) as u64;
        if (b >> 5) & 31 != a & 31 {
            return Err(format!(
                "the low instruction at {lo_off:#x} does not use the adrp register"
            ));
        }
        if b & 0xFFC0_0000 == 0x9100_0000 {
            return Ok(page + ((b >> 10) & 0xFFF) as u64);
        }
        if b & 0x3B00_0000 != 0x3900_0000 {
            return Err(format!("no add, load or store after the adrp at {off:#x}"));
        }
        let scale = (1u64 << (b >> 30))
            * if b & 0x0480_0000 == 0x0480_0000 && b >> 30 == 0 {
                16
            } else {
                1
            };
        Ok(page + ((b >> 10) & 0xFFF) as u64 * scale)
    }
}

fn ldrx(t: u32, n: u32, imm12: u64) -> u32 {
    0xF940_0000 | (imm12 as u32) << 10 | n << 5 | t
}
fn strx(t: u32, n: u32, imm12: u64) -> u32 {
    0xF900_0000 | (imm12 as u32) << 10 | n << 5 | t
}
fn stp(op: u32, t: u32, t2: u32, n: u32, off: i64) -> u32 {
    op | ((((off / 8) & 0x7F) as u32) << 15) | t2 << 10 | n << 5 | t
}

// ---------------- Hyrule Warriors: Age of Calamity (01002B00111A2000): the 60 fps frame step

/// nxnative's `stub(ctx, kids, nn_main, scene, block, device, rate)`: the frame step at the KIDS frame task, nnMain's
/// device config, the scene update's event counters, the game's frame-rate block and the KTGL device's global.
pub fn hwaoc(
    ctx: &mut Ctx,
    kids: u64,
    nn_main: u64,
    scene: u64,
    block: u64,
    device: u64,
    rate: Option<u64>,
) -> Result<(), String> {
    if block % 4 != 0 || (block & 0xFFF) + 0xC >= 0x1000 || device % 8 != 0 {
        return Err("frame-rate block or device not as reviewed".into());
    }
    let more = rate.is_none() as u32;
    let (s_task, s_steps, s_device) = (kids + 0x134, kids + 0x2e0, nn_main + 0x1e60);
    ctx.site(s_task, "68734339")?;
    ctx.site(s_steps, "e83640b9")?;
    ctx.site(s_device, "4900c0f2")?;
    let start = ctx.here();
    ctx.adrp(16, STATE);
    for w in [
        0xb940_0210,
        0x52a8_3e11,
        0x6b11_021f,
        0x5400_006d,
        0xf2c0_0029,
    ] {
        ctx.emit(w);
    }
    ctx.b(s_device + 4);
    ctx.emit(0xf2c0_0049);
    ctx.b(s_device + 4);
    ctx.put_b(s_device, start, "KTGL device config present interval: 1 above a 30 fps cap, else the game's 2 (state page target fps)");
    let start = ctx.here();
    ctx.adrp(16, STATE);
    for w in [
        0xbd40_0210,
        0x1e20_2208,
        0x5400_01ad,
        0x1e38_0208,
        0xb900_3268,
        0x5280_0791,
        0x1ac8_0a31,
        0xb900_2a11,
    ] {
        ctx.emit(w);
    }
    ctx.adrp(16, block);
    let lo = (block & 0xFFF) as u32;
    for w in [
        0xb900_0208 | (lo / 4) << 10,
        0xb900_0211 | ((lo + 4) / 4) << 10,
        0xb900_0208 | ((lo + 8) / 4) << 10,
        0x1e22_0111,
        0xbd00_0211 | ((lo + 12) / 4) << 10,
        0x1400_0007,
        0xb900_2a1f,
        0xb940_3268,
        0x3400_0008 | (12 + more) << 5,
        0x5280_0791,
        0x1ac8_0a31,
        0x3400_0011 | (9 + more) << 5,
    ] {
        ctx.emit(w);
    }
    ctx.adrp(16, device);
    ctx.emit(0xf940_0210 | (((device & 0xFFF) / 8) as u32) << 10);
    ctx.emit(0xb400_0010 | (6 + more) << 5);
    match rate {
        Some(r) => {
            ctx.adrp(8, r);
            ctx.emit(0xbd40_0110 | (((r & 0xFFF) / 4) as u32) << 10);
        }
        None => {
            let bits = 59.94f32.to_bits();
            ctx.emit(0x5280_0008 | (bits & 0xFFFF) << 5);
            ctx.emit(0x72a0_0008 | (bits >> 16) << 5);
            ctx.emit(0x1e27_0110);
        }
    }
    for w in [0x1e23_0231, 0x1e31_1a10, 0xbd32_d210, 0x3943_7368] {
        ctx.emit(w);
    }
    ctx.b(s_task + 4);
    ctx.put_b(s_task, start, "KIDS frame task: fps and the frame-rate block = the cap, vsyncs per update marked; device frame rate 59.94 / vsyncs (state page target fps; 0 = the game's 30)");
    let start = ctx.here();
    ctx.emit(0xbd40_36f0);
    ctx.adrp(16, STATE);
    for w in [
        0xb940_2a11,
        0x3400_01f1,
        0xf940_1a08,
        0x8b31_4108,
        0xf900_1a08,
        0xb940_4ae8,
        0x3400_0148,
        0x1b08_7e31,
        0x1e23_0231,
        0x5280_0788,
        0x1e23_0112,
        0x1e32_1a31,
        0xbd40_46e0,
        0x1e31_0a10,
        0x1e31_0800,
        0x1400_0002,
        0xbd40_46e0,
        0x1e26_0208,
        0xb900_1a68,
    ] {
        ctx.emit(w);
    }
    ctx.b(s_steps + 0xc);
    ctx.put_b(s_steps, start, "KIDS frame task: the frame-rate object's steps × its fps × vsyncs per update / 60 (state page target fps; 0 = as the game sets them)");
    ctx.site(scene + 0x86c, "4a050011")?;
    ctx.site(scene + 0x890, "4a050051")?;
    for (site, step, what) in [
        (scene + 0x86c, 0x1100_054a, "frame"),
        (scene + 0x890, 0x5100_054a, "frames left"),
    ] {
        let start = ctx.here();
        ctx.adrp(16, STATE);
        for w in [
            0xb940_2a11,
            0x7100_063f,
            0x5400_0061,
            0xf940_1a11,
            0x3700_0051,
            step,
        ] {
            ctx.emit(w);
        }
        ctx.b(site + 4);
        ctx.put_b(site, start, &format!("event cutscene's {what}: stepped on every other update at a 60 fps cap (state page vsyncs per update; 30 fps frames, as on the Switch)"));
    }
    Ok(())
}

/// The matched rule: the KIDS task (base), nnMain, the scene update and the frame-rate block writer; the KTGL device's
/// global from the task's own load (+0xd70), the block from the writer's store of the fps (+0x3c).
pub fn hwaoc_matched(ctx: &mut Ctx) -> Result<(), String> {
    let g = |k: &str| ctx.others.get(k).copied().ok_or(format!("{k} not found"));
    let (nn, scene, writer) = (
        g("nnMain")?,
        g("scene update")?,
        g("frame-rate block writer")?,
    );
    let block = ctx.address(writer + 0x3c, None)?;
    let device = ctx.address(ctx.base + 0xd70, None)?;
    let kids = ctx.base;
    hwaoc(ctx, kids, nn, scene, block, device, None)
}

// ---------------- Ori and the Will of the Wisps (01008DD013200000): Unity's constant-buffer table under its mutex

const ORI_LOCK: u64 = 0xcbebd0;
const ORI_UNLOCK: u64 = 0xcbebe0;
const ORI_MUTEX: u64 = 0x2df18;
const ORI_WRAPPED: [(u64, &str, u32, &str); 10] = [
    (0x2d640, "ff4302d1", 0, "insert a layout"),
    (
        0x34f80,
        "ffc301d1",
        0,
        "register a layout and its members (main thread)",
    ),
    (0x353c0, "f60f1df8", 0, "look up a member (main thread)"),
    (
        0x24550,
        "ff0302d1",
        1,
        "apply a program's parameters (render thread, per draw)",
    ),
    (
        0x24de0,
        "ffc301d1",
        0,
        "set a constant buffer (render thread)",
    ),
    (
        0x2d200,
        "ff0302d1",
        0,
        "set several constant buffers (render thread)",
    ),
    (
        0x35510,
        "ff0301d1",
        0,
        "write a buffer's values (render thread)",
    ),
    (
        0x25110,
        "fc6fbaa9",
        0,
        "move a layout's bound slots (render thread)",
    ),
    (0x29330, "083858f9", 0, "reset the layouts' slots"),
    (
        0x29120,
        "f40f1ef8",
        1,
        "copy the table into a graphics-job device",
    ),
];

pub fn ori_registry_lock(ctx: &mut Ctx) -> Result<(), String> {
    let slot = ctx.address(ORI_MUTEX, None)?;
    let mutex_call = |ctx: &mut Ctx, f: u64| {
        ctx.adrp(0, slot);
        ctx.emit(ldrx(0, 0, (slot & 0xFFF) / 8));
        ctx.bl(f);
    };
    for (f, first, dev, what) in ORI_WRAPPED {
        ctx.site(f, first)?;
        let start = ctx.here();
        let orig = start + 4 * 23;
        ctx.emit(0xb96f_8810 | dev << 5);
        ctx.cbnzw(16, orig);
        ctx.emit(stp(0xA980_0000, 29, 30, 31, -0x60));
        ctx.emit(0x9100_03fd);
        for i in 0..4 {
            ctx.emit(stp(
                0xA900_0000,
                2 * i,
                2 * i + 1,
                31,
                0x10 + 0x10 * i as i64,
            ));
        }
        mutex_call(ctx, ORI_LOCK);
        for i in 0..4 {
            ctx.emit(stp(
                0xA940_0000,
                2 * i,
                2 * i + 1,
                31,
                0x10 + 0x10 * i as i64,
            ));
        }
        ctx.bl(orig);
        ctx.emit(strx(0, 31, 2));
        mutex_call(ctx, ORI_UNLOCK);
        ctx.emit(ldrx(0, 31, 2));
        ctx.emit(stp(0xA8C0_0000, 29, 30, 31, 0x60));
        ctx.emit(0xd65f_03c0);
        if ctx.here() != orig {
            return Err("the wrapper is not 23 words".into());
        }
        let w = u32::from_le_bytes(
            (0..4)
                .map(|i| u8::from_str_radix(&first[2 * i..2 * i + 2], 16).unwrap())
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
        );
        ctx.emit(w);
        ctx.b(f + 4);
        ctx.put_b(f, start, &format!("Unity's constant-buffer table: {what} on the shared device under the registry mutex"));
    }
    Ok(())
}

// ---------------- Xenoblade Chronicles X: Definitive Edition (0100453019AA8000): the camera's angular step

pub fn xenoblade_camera_step(ctx: &mut Ctx) -> Result<(), String> {
    let g = |k: &str| ctx.others.get(k).copied().ok_or(format!("{k} not found"));
    let (setter, elapsed_fn) = (g("interval setter")?, g("elapsed seconds")?);
    g("clamped task step")?;
    // the setter interleaves two adrps with their two loads: its first pair addresses the interval's GOT slot
    let interval = ctx.address(setter, Some(setter + 8))?;
    let elapsed = ctx.address(elapsed_fn, None)?;
    if interval % 8 != 0 || elapsed % 8 != 0 {
        return Err("interval or elapsed time not 8-aligned".into());
    }
    let site = ctx.base + 0x1ec;
    ctx.site(site, "a8b640bd")?;
    let start = ctx.here();
    ctx.emit(0xbd40_b6a8);
    ctx.adrp(8, interval);
    ctx.emit(ldrx(8, 8, (interval & 0xFFF) / 8));
    for w in [
        0xb940_0108,
        0x7100_011f,
        0x5280_0049,
        0x1a88_0128,
        0x5280_0789,
        0x1ac8_0d28,
        0x1e22_0101,
        0x1e2e_1002,
        0x1e21_1841,
    ] {
        ctx.emit(w);
    }
    ctx.adrp(8, elapsed);
    ctx.emit(ldrx(8, 8, (elapsed & 0xFFF) / 8));
    for w in [
        0x1e20_4022,
        0xb400_0048,
        0xbd62_6d02,
        0x1e21_2825,
        0x1e25_2040,
        0x1e22_cca5,
        0x1e21_2040,
        0x1e25_4c22,
        0x1e27_d001,
        0x1e21_0842,
        0x1e22_0908,
    ] {
        ctx.emit(w);
    }
    ctx.b(site + 4);
    ctx.put_b(
        site,
        start,
        "camera angular integration: use the game's clamped elapsed frame time",
    );
    ctx.body.fps_ready = Some(true);
    Ok(())
}

// ---------------- 0100A00019DE0000: the render size times the host's render scale

fn is_size(w: &[u32], i: usize) -> Option<u32> {
    (w[i] & 0xffff_ffe0 == 0xd280_a000 && i + 1 < w.len() && w[i + 1] == 0xf2c0_5a00 | (w[i] & 31))
        .then_some(w[i] & 31)
}

/// The anchored find: the one function calling the import `calls` (through its PLT stub), with one 1280x720 size
/// after that call and nowhere else in main; returns (the function, the size's offset in it, its register).
pub fn render_size_find(
    nso: &translator::loader::nso::Nso,
    calls: &str,
) -> Result<(u64, u64, u32), String> {
    use translator::loader::nso::R_AARCH64_JUMP_SLOT;
    let img = &nso.image;
    let end = nso.text.0 + nso.text.1;
    let w = |a: usize| u32::from_le_bytes(img[a..a + 4].try_into().unwrap());
    let sym = nso
        .syms
        .iter()
        .position(|s| s.name == calls)
        .ok_or(format!("{calls} not imported"))?;
    let slot = nso
        .relocs
        .iter()
        .find(|r| r.typ == R_AARCH64_JUMP_SLOT && r.sym as usize == sym)
        .ok_or("no PLT slot")?
        .off;
    // the PLT stub: adrp x16, slot page; ldr x17, [x16, #lo]; add x16, x16, #lo; br x17
    let mut plt = None;
    let mut a = nso.text.0;
    while a + 16 <= end {
        let p = w(a);
        if p & 0x9F00_001F == 0x9000_0010 && w(a + 12) == 0xd61f_0220 {
            let page = ((a as i64 & !0xFFF)
                + (((((((p >> 5) & 0x7FFFF) << 2) | ((p >> 29) & 3)) as i64) << 44) >> 32))
                as u64;
            let l = w(a + 4);
            if l & 0xFFC0_03FF == 0xF940_0211 && page + ((l >> 10) & 0xFFF) as u64 * 8 == slot {
                plt = Some(a);
                break;
            }
        }
        a += 4;
    }
    let plt = plt.ok_or("no PLT stub for the import")? as i64;
    let image = translator::aot::nso_match::Image::new(img, end).map_err(|e| e.0)?;
    let mut callers = vec![];
    for (&s, &e) in &image.functions {
        for c in (s..e).step_by(4) {
            let u = w(c);
            if u & 0xFC00_0000 == 0x9400_0000
                && c as i64 + ((((u & 0x3FF_FFFF) << 6) as i32 >> 4) as i64) == plt
            {
                callers.push((s, e, c));
            }
        }
    }
    let fns: std::collections::BTreeSet<usize> = callers.iter().map(|c| c.0).collect();
    if fns.len() != 1 {
        return Err(format!("{} functions call {calls}", fns.len()));
    }
    let (s, e, call) = callers[0];
    let words: Vec<u32> = (s..e).step_by(4).map(w).collect();
    let sites: Vec<(usize, u32)> = (0..words.len())
        .filter_map(|i| is_size(&words, i).map(|rd| (4 * i, rd)))
        .collect();
    if sites.len() != 1 {
        return Err(format!(
            "{} 1280x720 sizes in the function at {s:#x} (need one)",
            sites.len()
        ));
    }
    let (off, rd) = sites[0];
    if callers.iter().filter(|c| c.0 == s).count() != 1 || call > s + off {
        return Err("the size does not follow the one CreateLayer call".into());
    }
    let text: Vec<u32> = (nso.text.0..end).step_by(4).map(w).collect();
    let n = (0..text.len())
        .filter(|&i| is_size(&text, i).is_some())
        .count();
    if n != 1 {
        return Err(format!("{n} 1280x720 sizes in main's text (need one)"));
    }
    let movk = (s + off + 4) as i64;
    if (s..e)
        .step_by(4)
        .any(|c| translator::aot::patches::branch_target(w(c), c as u64) == Some(movk as u64))
    {
        return Err("a branch lands on the movk".into());
    }
    // the stub writes x16, x17 and v16-v18: the function must not use them
    for u in &words {
        let regs = [u & 31, (u >> 5) & 31, (u >> 10) & 31, (u >> 16) & 31];
        if regs.iter().any(|&r| (16..=18).contains(&r))
            && (u & 0x1F00_0000 == 0x1E00_0000 || regs[..2].iter().any(|&r| r == 16 || r == 17))
        {
            return Err(format!(
                "the function at {s:#x} uses x16 / x17 / v16-v18, which the stub writes"
            ));
        }
    }
    Ok((s as u64, off as u64, rd))
}

pub fn render_size(ctx: &mut Ctx, off: u64, rd: u32) -> Result<(), String> {
    let site = ctx.base + off;
    let orig: String = (0xd280_a000u32 | rd)
        .to_le_bytes()
        .iter()
        .chain((0xf2c0_5a00u32 | rd).to_le_bytes().iter())
        .map(|b| format!("{b:02x}"))
        .collect();
    ctx.site(site, &orig)?;
    let start = ctx.here();
    ctx.adrp(16, STATE);
    for w in [
        0xbd40_1210,
        0x1e2e_1011,
        0x1e31_2200,
        0x5400_01e0,
        0x1e20_2208,
        0x5400_01ad,
        0x5280_2811,
        0x1e22_0231,
        0x1e30_0a31,
        0x1e20_0231,
        0x5280_1690,
        0x1e22_0212,
        0x1e30_0a52,
        0x1e20_0250,
        0x531e_7631,
        0x531e_7610,
        0xaa10_8220 | rd,
    ] {
        ctx.emit(w);
    }
    ctx.b(site + 8);
    ctx.emit(0xd280_a000 | rd);
    ctx.emit(0xf2c0_5a00 | rd);
    ctx.b(site + 8);
    ctx.put_b(site, start, "render size: 1280x720 times the host's render scale (state +0x10; 1.0 = the game's 1280x720)");
    ctx.body.render_scale = Some(3);
    Ok(())
}

/// The generator of a stub body the exporter could not evaluate: by the rule's name or the build (None: none ported).
pub fn generate(
    what: &str,
    nso: &translator::loader::nso::Nso,
    base: u64,
    others: &BTreeMap<String, u64>,
    calls: Option<&str>,
) -> Option<Result<StubBody, String>> {
    let mut ctx = Ctx::new(&nso.image, base, others.clone());
    let r = if what.starts_with("60 fps frame step: the KIDS frame task") {
        hwaoc_matched(&mut ctx)
    } else if what == "11c33c7c2dbd3c5491841f67495c7ee8" {
        ori_registry_lock(&mut ctx)
    } else if what.starts_with("camera angular") || others.contains_key("clamped task step") {
        xenoblade_camera_step(&mut ctx)
    } else if let Some(c) = calls.filter(|c| c.contains("CreateLayer")) {
        render_size_find(nso, c).and_then(|(s, off, rd)| {
            ctx.base = s;
            render_size(&mut ctx, off, rd)
        })
    } else {
        return None;
    };
    Some(r.map(|_| ctx.body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(hex: &str) -> Vec<u8> {
        (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }
    fn put(img: &mut [u8], at: u64, bytes: &[u8]) {
        img[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    }
    fn adrp_word(pc: u64, target: u64, rd: u32) -> u32 {
        let imm = ((target >> 12) as i64 - (pc >> 12) as i64) as u32 & 0x1F_FFFF;
        0x9000_0000 | (imm & 3) << 29 | (imm >> 2) << 5 | rd
    }
    /// The words a relocation does not rewrite, and the relocations.
    fn shape(b: &StubBody) -> (Vec<(usize, u32)>, Vec<(usize, String, String, i64)>) {
        let fixed: Vec<usize> = b.relocs.iter().map(|r| r.index).collect();
        (
            b.code
                .iter()
                .enumerate()
                .filter(|(i, _)| !fixed.contains(i))
                .map(|(i, w)| (i, *w))
                .collect(),
            b.relocs
                .iter()
                .map(|r| (r.index, r.op.clone(), r.target.clone(), r.addend))
                .collect(),
        )
    }

    /// The port of Hyrule Warriors' generator, run at 1.3.0's addresses, gives the body nxnative recorded for that
    /// build: the same words (apart from what relocations fill in), relocations, asserted sites and puts.
    #[test]
    fn hwaoc_matches_the_recorded_build_entry() {
        let prof = profiles::by_title(0x0100_2B00_111A_2000).expect("profile");
        let rec = prof
            .patch_files()
            .flat_map(|f| f.stub.iter())
            .find(|s| s.kind == "frame_step")
            .expect("recorded")
            .body
            .clone();
        let mut img = vec![0u8; 0x2c0_0000];
        for s in &rec.sites {
            put(&mut img, s.offset as u64, &le(&s.original));
        }
        let mut ctx = Ctx::new(&img, 0, BTreeMap::new());
        hwaoc(
            &mut ctx,
            0x166d980,
            0x8d94b0 - 0x1e60,
            0x857510,
            0x269e150,
            0x2bac798,
            Some(0x1ef5f3c),
        )
        .unwrap();
        let b = ctx.body;
        assert_eq!(b.code.len(), rec.code.len());
        assert_eq!(shape(&b), shape(&rec));
        let sites = |b: &StubBody| {
            b.sites
                .iter()
                .map(|s| (s.offset, s.original.clone()))
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(sites(&b), sites(&rec));
        let puts = |b: &StubBody| {
            b.puts
                .iter()
                .map(|p| (p.offset, p.relocs[0].addend, p.reason.clone()))
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(puts(&b), puts(&rec));
    }

    /// Ori's lock: the registry mutex's GOT slot read from the job devices' adrp + ldr, then each of the ten
    /// functions wrapped (23 words, its first instruction, a branch back).
    #[test]
    fn ori_wraps_the_ten_functions() {
        let mut img = vec![0u8; 0xd0_0000];
        put(
            &mut img,
            ORI_MUTEX,
            &adrp_word(ORI_MUTEX, 0x7e48000, 8).to_le_bytes(),
        );
        put(
            &mut img,
            ORI_MUTEX + 4,
            &(0xF940_0108u32 | (0x960 / 8) << 10).to_le_bytes(),
        ); // ldr x8, [x8, #0x960]
        for (f, first, _, _) in ORI_WRAPPED {
            put(&mut img, f, &le(first));
        }
        let mut ctx = Ctx::new(&img, 0, BTreeMap::new());
        ori_registry_lock(&mut ctx).unwrap();
        let b = ctx.body;
        assert_eq!(
            (b.code.len(), b.puts.len(), b.sites.len()),
            (10 * 25, 10, 10)
        );
        assert_eq!(
            b.code[9],
            0xF940_0000 | (0x960 / 8) << 10,
            "ldr x0, [x0, #slot lo]"
        );
        assert!(b
            .relocs
            .iter()
            .any(|r| r.op == "adrp" && r.addend == 0x7e48000));
        assert!(
            b.relocs
                .iter()
                .any(|r| r.op == "bl" && r.addend == ORI_LOCK as i64)
                && b.relocs
                    .iter()
                    .any(|r| r.op == "bl" && r.addend == ORI_UNLOCK as i64)
        );
        assert_eq!(
            b.code[23],
            u32::from_le_bytes(le("ff4302d1").try_into().unwrap()),
            "the original first instruction after the wrapper"
        );
        assert!(b
            .relocs
            .iter()
            .any(|r| r.index == 24 && r.op == "b" && r.addend == 0x2d644));
    }

    /// Xenoblade's camera step: the interval's GOT slot from the setter's interleaved adrp / ldr, the frame object's
    /// from its getter; the gain load at base + 0x1ec replaced.
    #[test]
    fn xenoblade_camera_step_reads_its_globals() {
        let mut img = vec![0u8; 0x10_0000];
        let (base, setter, elapsed) = (0x1000u64, 0x3000u64, 0x4000u64);
        put(&mut img, base + 0x1ec, &le("a8b640bd"));
        put(
            &mut img,
            setter,
            &adrp_word(setter, 0x8_0000, 8).to_le_bytes(),
        );
        put(
            &mut img,
            setter + 8,
            &(0xF940_0108u32 | (0x18 / 8) << 10).to_le_bytes(),
        );
        put(
            &mut img,
            elapsed,
            &adrp_word(elapsed, 0x9_0000, 9).to_le_bytes(),
        );
        put(
            &mut img,
            elapsed + 4,
            &(0xF940_0129u32 | (0x20 / 8) << 10).to_le_bytes(),
        );
        let others = BTreeMap::from([
            ("interval setter".into(), setter),
            ("elapsed seconds".into(), elapsed),
            ("clamped task step".into(), 0x5000),
        ]);
        let mut ctx = Ctx::new(&img, base, others);
        xenoblade_camera_step(&mut ctx).unwrap();
        let b = ctx.body;
        assert_eq!(b.code[2], 0xF940_0108 | (0x18 / 8) << 10);
        assert!(
            b.relocs
                .iter()
                .any(|r| r.op == "adrp" && r.addend == 0x8_0000)
                && b.relocs
                    .iter()
                    .any(|r| r.op == "adrp" && r.addend == 0x9_0000)
        );
        assert_eq!(
            (b.puts[0].offset, b.fps_ready),
            ((base + 0x1ec) as i64, Some(true))
        );
        assert!(b
            .relocs
            .iter()
            .any(|r| r.op == "b" && r.addend == (base + 0x1f0) as i64));
    }

    /// The render size: the one 1280x720 pair, its register kept; the stub reads the state page's render scale.
    #[test]
    fn render_size_stub() {
        let words = [0xd503_201f, 0xd280_a008, 0xf2c0_5a08, 0xd65f_03c0];
        assert_eq!((is_size(&words, 1), is_size(&words, 0)), (Some(8), None));
        let mut img = vec![0u8; 0x1000];
        for (i, w) in words.iter().enumerate() {
            put(&mut img, 0x100 + 4 * i as u64, &w.to_le_bytes());
        }
        let mut ctx = Ctx::new(&img, 0x100, BTreeMap::new());
        render_size(&mut ctx, 4, 8).unwrap();
        let b = ctx.body;
        assert_eq!(b.relocs[0].target, "state");
        assert_eq!(b.code[17], 0xaa10_8228, "orr x8, x17, x16, lsl #32");
        assert_eq!((b.render_scale, b.puts[0].offset), (Some(3), 0x104));
    }
}
