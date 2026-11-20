//! Atmosphère-format cheat codes (the dmnt cheat VM) applied to the running game (nxnative `hle_cheats.c`'s design,
//! reimplemented; the VM follows Atmosphère's reference: `dmnt_cheat_vm.cpp`, `docs/features/cheats.md`).
//!
//! * Files: `<data>\cheats\<build>.txt` or the Atmosphère tree `<data>\cheats\atmosphere\contents\<title>\cheats\
//!   <build>.txt` (`NXOS_CHEATS=<folder>` names another folder), the build being the first 16 hex digits of main's
//!   build id. `[Name]` starts a cheat, `{Name}` a master code (always on); an entry without code words is a label.
//! * Toggles: `<build>.enabled` beside the file, one cheat name a line (written when they change); the control
//!   commands `cheats`, `cheat on|off <name>`, `cheats reload`; `NXOS_CHEATS_ON=<name>,...` at launch.
//! * A pass at every present (os's present hook): the registers reset, the masters then the enabled cheats run in
//!   file order sharing them; each cheat runs as its own program (an unclosed conditional cannot swallow the next
//!   cheat), with the reference's word order, conditions, loops and register semantics. Opcodes 0-A and C0-C4;
//!   anything else turns that cheat off with the reason ("unsupported opcode").
//! * Memory: the memory-type nybble picks 0 main, 1 heap, 2 alias, 3 aslr, 4 absolute, 5-F main. A failed access
//!   (an unmapped page, a pointer not set yet) is ignored as on a Switch: a load leaves the register, a condition
//!   compares 0, a store writes nothing; the first failure of a cheat logs one line. Writes into main's code reach the
//!   bytes but not the code the game runs (it was translated at build time), said once.

use std::sync::Mutex;

const STEP_CAP: u64 = 1 << 20;

#[derive(Clone, Debug, Default)]
pub struct Cheat {
    pub name: String,
    pub master: bool,
    pub words: Vec<u32>,
    pub enabled: bool,
    /// Why it cannot run (an unsupported or malformed instruction): off for good until a reload.
    pub broken: Option<String>,
    failed: u64,
}

/// A cheat file's entries (labels, entries without code, left out).
pub fn parse(text: &str) -> Vec<Cheat> {
    let mut out: Vec<Cheat> = vec![];
    let mut cur: Option<Cheat> = None;
    for raw in text.lines() {
        let mut line = raw.trim();
        while !line.is_empty() {
            if line.starts_with('[') || line.starts_with('{') {
                let close = if line.starts_with('[') { ']' } else { '}' };
                let Some(end) = line.find(close) else { break };
                if let Some(c) = cur.take() {
                    if !c.words.is_empty() {
                        out.push(c);
                    }
                }
                cur = Some(Cheat { name: line[1..end].trim().to_string(), master: close == '}', ..Default::default() });
                line = line[end + 1..].trim_start();
                continue;
            }
            // code words: 8 hex digits each, separated by spaces; a header glued to the last word starts a new entry
            let tok_end = line.find(|c: char| c.is_whitespace() || c == '[' || c == '{').unwrap_or(line.len());
            let tok = &line[..tok_end];
            if tok.len() == 8 && tok.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Some(c) = cur.as_mut() {
                    c.words.push(u32::from_str_radix(tok, 16).unwrap());
                }
            } else {
                break; // prose (a credits line): ignored
            }
            line = line[tok_end..].trim_start();
        }
    }
    if let Some(c) = cur.take() {
        if !c.words.is_empty() {
            out.push(c);
        }
    }
    let mut out: Vec<Cheat> = out;
    for c in out.iter_mut() {
        if let Err(e) = validate(&c.words) {
            c.broken = Some(e);
        }
        c.enabled = c.master;
    }
    out
}

fn width_ok(t: u32) -> bool {
    matches!(t, 1 | 2 | 4 | 8)
}
fn width_mask(t: u32) -> u64 {
    if t >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * t)) - 1
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Ins {
    op: u32,
    len: usize,
    t: u32,
    m: u32,
    r: u32,
    s: u32,
    x: u32,
    c: u32,
    o: u32,
    flags: u32,
    rel: u64,
    val: u64,
}

fn op_kind(w: u32) -> u32 {
    let hi = w >> 28;
    if hi == 0xC { w >> 24 } else { hi }
}

/// One instruction at `p` (the words left), the reference's DecodeNextOpcode field by field.
fn decode(p: &[u32]) -> Result<Ins, String> {
    let n = p.len();
    if n == 0 {
        return Err("truncated".into());
    }
    let w = p[0];
    let mut o = Ins { op: op_kind(w), len: 1, ..Default::default() };
    let need = |k: usize| if n < k { Err("truncated".to_string()) } else { Ok(()) };
    let width = |t: u32| if width_ok(t) { Ok(t) } else { Err(format!("width {t}")) };
    match o.op {
        0 => {
            o.t = width((w >> 24) & 0xF)?;
            o.m = (w >> 20) & 0xF;
            o.r = (w >> 16) & 0xF;
            need(2)?;
            o.rel = ((w & 0xFF) as u64) << 32 | p[1] as u64;
            if o.t == 8 {
                need(4)?;
                o.val = (p[2] as u64) << 32 | p[3] as u64;
                o.len = 4;
            } else {
                need(3)?;
                o.val = p[2] as u64;
                o.len = 3;
            }
        }
        1 => {
            o.t = width((w >> 24) & 0xF)?;
            o.m = (w >> 20) & 0xF;
            o.c = (w >> 16) & 0xF;
            if !(1..=6).contains(&o.c) {
                return Err(format!("condition {}", o.c));
            }
            o.flags = ((w >> 12) & 0xF != 0) as u32;
            o.x = (w >> 8) & 0xF;
            need(2)?;
            o.rel = ((w & 0xFF) as u64) << 32 | p[1] as u64;
            if o.t == 8 {
                need(4)?;
                o.val = (p[2] as u64) << 32 | p[3] as u64;
                o.len = 4;
            } else {
                need(3)?;
                o.val = p[2] as u64;
                o.len = 3;
            }
        }
        2 => o.flags = (((w >> 24) & 0xF) == 1) as u32,
        3 => {
            o.flags = (((w >> 24) & 0xF) != 0) as u32;
            o.r = (w >> 16) & 0xF;
            if o.flags == 0 {
                need(2)?;
                o.val = p[1] as u64;
                o.len = 2;
            }
        }
        4 => {
            need(3)?;
            o.r = (w >> 16) & 0xF;
            o.val = (p[1] as u64) << 32 | p[2] as u64;
            o.len = 3;
        }
        5 => {
            o.t = width((w >> 24) & 0xF)?;
            o.m = (w >> 20) & 0xF;
            o.r = (w >> 16) & 0xF;
            o.flags = (w >> 12) & 0xF;
            o.x = (w >> 8) & 0xF;
            if o.flags > 3 {
                return Err("operand".into());
            }
            need(2)?;
            o.rel = ((w & 0xFF) as u64) << 32 | p[1] as u64;
            o.len = 2;
        }
        6 => {
            o.t = width((w >> 24) & 0xF)?;
            need(3)?;
            o.r = (w >> 16) & 0xF;
            o.flags = (((w >> 12) & 0xF) != 0) as u32 | ((((w >> 8) & 0xF) != 0) as u32) << 1;
            o.x = (w >> 4) & 0xF;
            o.val = (p[1] as u64) << 32 | p[2] as u64;
            o.len = 3;
        }
        7 => {
            o.t = width((w >> 24) & 0xF)?;
            o.c = (w >> 12) & 0xF;
            if o.c > 4 {
                return Err("arithmetic".into());
            }
            o.r = (w >> 16) & 0xF;
            need(2)?;
            o.val = p[1] as u64;
            o.len = 2;
        }
        8 => o.val = (w & 0x0FFF_FFFF) as u64,
        9 => {
            o.t = width((w >> 24) & 0xF)?;
            o.c = (w >> 20) & 0xF;
            o.r = (w >> 16) & 0xF;
            o.s = (w >> 12) & 0xF;
            o.flags = (((w >> 8) & 0xF) != 0) as u32;
            o.x = (w >> 4) & 0xF;
            if o.flags != 0 {
                if o.t == 8 {
                    need(3)?;
                    o.val = (p[1] as u64) << 32 | p[2] as u64;
                    o.len = 3;
                } else {
                    need(2)?;
                    o.val = p[1] as u64 & width_mask(o.t);
                    o.len = 2;
                }
            }
        }
        0xA => {
            o.t = width((w >> 24) & 0xF)?;
            o.s = (w >> 20) & 0xF;
            o.r = (w >> 16) & 0xF;
            o.flags = (((w >> 12) & 0xF) != 0) as u32;
            o.o = (w >> 8) & 0xF;
            o.x = (w >> 4) & 0xF;
            if o.o > 5 {
                return Err("operand".into());
            }
            if o.o >= 3 {
                o.m = o.x;
            }
            if matches!(o.o, 2 | 4 | 5) {
                need(2)?;
                o.rel = ((w & 0xF) as u64) << 32 | p[1] as u64;
                o.len = 2;
            }
        }
        0xC0 => {
            o.t = width((w >> 20) & 0xF)?;
            o.c = (w >> 16) & 0xF;
            if !(1..=6).contains(&o.c) {
                return Err(format!("condition {}", o.c));
            }
            o.s = (w >> 12) & 0xF;
            o.o = (w >> 8) & 0xF;
            match o.o {
                0 | 1 => {
                    o.m = (w >> 4) & 0xF;
                    if o.o == 1 {
                        o.x = w & 0xF;
                    } else {
                        need(2)?;
                        o.rel = ((w & 0xF) as u64) << 32 | p[1] as u64;
                        o.len = 2;
                    }
                }
                2 => {
                    o.r = (w >> 4) & 0xF;
                    need(2)?;
                    o.rel = ((w & 0xF) as u64) << 32 | p[1] as u64;
                    o.len = 2;
                }
                3 => {
                    o.r = (w >> 4) & 0xF;
                    o.x = w & 0xF;
                }
                4 => {
                    if o.t == 8 {
                        need(3)?;
                        o.val = (p[1] as u64) << 32 | p[2] as u64;
                        o.len = 3;
                    } else {
                        need(2)?;
                        o.val = p[1] as u64;
                        o.len = 2;
                    }
                }
                5 => o.x = (w >> 4) & 0xF,
                _ => return Err("operand".into()),
            }
        }
        0xC1 => {
            o.r = (w >> 16) & 0xF;
            o.s = (w >> 8) & 0xF;
            o.c = (w >> 4) & 0xF;
        }
        0xC2 => {
            o.c = (w >> 20) & 0xF;
            o.val = (w & 0xFFFF) as u64;
        }
        0xC3 => {
            o.x = (w >> 4) & 0xFF;
            o.r = w & 0xF;
        }
        0xC4 => {
            need(3)?;
            o.flags = (((w >> 20) & 0xF) != 0) as u32;
            o.val = (p[1] as u64) << 32 | p[2] as u64;
            o.len = 3;
        }
        op => return Err(format!("unsupported opcode {op:X}")),
    }
    Ok(o)
}

/// Every instruction decodes, word boundary by word boundary.
fn validate(words: &[u32]) -> Result<(), String> {
    let mut i = 0;
    while i < words.len() {
        let ins = decode(&words[i..]).map_err(|e| format!("{e} at word {i}"))?;
        i += ins.len;
    }
    Ok(())
}

/// What the VM reaches: region bases, memory reads / writes, the keys held.
pub trait Mem {
    fn base(&self, m: u32) -> u64;
    fn read(&mut self, a: u64, width: u32) -> Option<u64>;
    fn write(&mut self, a: u64, width: u32, v: u64) -> bool;
}

/// The VM's state of one pass (the registers and saved registers reset each pass; the static registers kept).
#[derive(Default)]
pub struct Vm {
    pub reg: [u64; 16],
    pub saved: [u64; 16],
    pub statics: Vec<u64>,
    pub held: u64,
    pub keyold: u64,
}

fn cond_met(c: u32, got: u64, want: u64) -> bool {
    match c {
        1 => got > want,
        2 => got >= want,
        3 => got < want,
        4 => got <= want,
        5 => got == want,
        _ => got != want,
    }
}

fn float_math(c: u32, t: u32, a: u64, b: u64) -> u64 {
    match t {
        4 => {
            let (x, y) = (f32::from_bits(a as u32), f32::from_bits(b as u32));
            let r = match c {
                10 => x + y,
                11 => x - y,
                12 => x * y,
                _ => x / y,
            };
            r.to_bits() as u64
        }
        8 => {
            let (x, y) = (f64::from_bits(a), f64::from_bits(b));
            let r = match c {
                10 => x + y,
                11 => x - y,
                12 => x * y,
                _ => x / y,
            };
            r.to_bits()
        }
        _ => 0,
    }
}

impl Vm {
    /// Skip a failed / finished conditional block (nested ones counted); a not-taken if stops at its else.
    fn skip(words: &[u32], idx: &mut usize, depth: &mut i32, is_if: bool) -> Result<(), String> {
        let desired = *depth - 1;
        while *depth > desired && *idx < words.len() {
            let ins = decode(&words[*idx..])?;
            if matches!(ins.op, 1 | 8 | 0xC0 | 0xC4) {
                *depth += 1;
            } else if ins.op == 2 {
                if ins.flags == 0 {
                    *depth -= 1;
                } else if is_if && *depth - 1 == desired {
                    *idx += ins.len;
                    return Ok(());
                }
            }
            *idx += ins.len;
        }
        Ok(())
    }

    /// One run of a cheat's program; Err: it cannot run (turned off). `failed` counts failed accesses.
    pub fn run(&mut self, words: &[u32], mem: &mut dyn Mem, failed: &mut u64) -> Result<(), String> {
        if self.statics.len() < 256 {
            self.statics.resize(256, 0);
        }
        let (mut idx, mut depth, mut steps) = (0usize, 0i32, 0u64);
        let mut looptop = [0usize; 16];
        while idx < words.len() {
            steps += 1;
            if steps > STEP_CAP {
                return Err(format!("over {STEP_CAP} instructions in one pass: a runaway loop"));
            }
            let i = decode(&words[idx..])?;
            // a region not set up yet (the heap before the game asks for one): this pass of the cheat stops there
            let uses_region = match i.op {
                0 | 1 => true,
                5 => matches!(i.flags, 0 | 3),
                0xC0 => i.o <= 1,
                0xA => i.o >= 3,
                _ => false,
            };
            if uses_region && i.m != 4 && mem.base(i.m) == 0 {
                return Ok(());
            }
            let rd = |mem: &mut dyn Mem, a: u64, t: u32, failed: &mut u64| -> u64 {
                match mem.read(a, t) {
                    Some(v) => v,
                    None => {
                        *failed += 1;
                        0
                    }
                }
            };
            match i.op {
                0 => {
                    if !mem.write(mem.base(i.m) + i.rel + self.reg[i.r as usize], i.t, i.val & width_mask(i.t)) {
                        *failed += 1;
                    }
                }
                1 | 8 | 0xC4 => {
                    depth += 1;
                    let met = match i.op {
                        8 => self.held & i.val == i.val,
                        0xC4 => {
                            let now = self.held & i.val == i.val;
                            if i.flags != 0 { now } else { now && self.keyold & i.val != i.val }
                        }
                        _ => {
                            let a = mem.base(i.m) + i.rel + if i.flags != 0 { self.reg[i.x as usize] } else { 0 };
                            let got = rd(mem, a, i.t, failed);
                            cond_met(i.c, got & width_mask(i.t), i.val & width_mask(i.t))
                        }
                    };
                    if !met {
                        idx += i.len;
                        Self::skip(words, &mut idx, &mut depth, true)?;
                        continue;
                    }
                }
                0xC0 => {
                    depth += 1;
                    let got = self.reg[i.s as usize] & width_mask(i.t);
                    let want = match i.o {
                        0 => { let a = mem.base(i.m) + i.rel; rd(mem, a, i.t, failed) }
                        1 => { let a = mem.base(i.m) + self.reg[i.x as usize]; rd(mem, a, i.t, failed) }
                        2 => rd(mem, self.reg[i.r as usize] + i.rel, i.t, failed),
                        3 => rd(mem, self.reg[i.r as usize] + self.reg[i.x as usize], i.t, failed),
                        4 => i.val,
                        _ => self.reg[i.x as usize],
                    };
                    if !cond_met(i.c, got, want & width_mask(i.t)) {
                        idx += i.len;
                        Self::skip(words, &mut idx, &mut depth, true)?;
                        continue;
                    }
                }
                2 => {
                    if i.flags != 0 {
                        if depth == 0 {
                            return Err("an else at conditional depth 0".into());
                        }
                        idx += i.len;
                        Self::skip(words, &mut idx, &mut depth, false)?;
                        continue;
                    }
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                3 => {
                    let r = i.r as usize;
                    if i.flags == 0 {
                        self.reg[r] = i.val;
                        looptop[r] = idx + 2;
                        idx += i.len;
                        continue;
                    }
                    self.reg[r] = self.reg[r].wrapping_sub(1);
                    if self.reg[r] != 0 {
                        idx = looptop[r];
                        continue;
                    }
                }
                4 => self.reg[i.r as usize] = i.val,
                5 => {
                    let a = match i.flags {
                        0 => mem.base(i.m) + i.rel,
                        1 => self.reg[i.r as usize].wrapping_add(i.rel),
                        2 => self.reg[i.x as usize].wrapping_add(i.rel),
                        _ => mem.base(i.m) + self.reg[i.x as usize] + i.rel,
                    };
                    // a narrower load replaces the register's low bytes, keeping the upper ones (the reference)
                    match mem.read(a, i.t) {
                        Some(v) => {
                            let r = &mut self.reg[i.r as usize];
                            *r = (*r & !width_mask(i.t)) | (v & width_mask(i.t));
                        }
                        None => *failed += 1,
                    }
                }
                6 => {
                    let a = self.reg[i.r as usize].wrapping_add(if i.flags & 2 != 0 { self.reg[i.x as usize] } else { 0 });
                    if !mem.write(a, i.t, i.val & width_mask(i.t)) {
                        *failed += 1;
                    }
                    if i.flags & 1 != 0 {
                        self.reg[i.r as usize] = self.reg[i.r as usize].wrapping_add(i.t as u64);
                    }
                }
                7 => {
                    let (v, r) = (i.val, self.reg[i.r as usize]);
                    let x = match i.c {
                        0 => r.wrapping_add(v),
                        1 => r.wrapping_sub(v),
                        2 => r.wrapping_mul(v),
                        3 => r << (v & 63),
                        _ => r >> (v & 63),
                    };
                    self.reg[i.r as usize] = x & width_mask(i.t);
                }
                9 => {
                    let a = self.reg[i.s as usize];
                    let b = if i.flags != 0 { i.val } else { self.reg[i.x as usize] };
                    let r = match i.c {
                        0 => a.wrapping_add(b),
                        1 => a.wrapping_sub(b),
                        2 => a.wrapping_mul(b),
                        3 => a << (b & 63),
                        4 => a >> (b & 63),
                        5 => a & b,
                        6 => a | b,
                        7 => !a,
                        8 => a ^ b,
                        9 => a,
                        10..=13 => float_math(i.c, i.t, a, b),
                        _ => 0,
                    };
                    self.reg[i.r as usize] = r & width_mask(i.t);
                }
                0xA => {
                    let (r, x) = (self.reg[i.r as usize], self.reg[i.x as usize]);
                    let a = match i.o {
                        0 => r,
                        1 => r.wrapping_add(x),
                        2 => r.wrapping_add(i.rel),
                        3 => mem.base(i.m) + r,
                        4 => mem.base(i.m) + i.rel,
                        _ => mem.base(i.m) + r + i.rel,
                    };
                    if !mem.write(a, i.t, self.reg[i.s as usize] & width_mask(i.t)) {
                        *failed += 1;
                    }
                    if i.flags != 0 {
                        self.reg[i.r as usize] = self.reg[i.r as usize].wrapping_add(i.t as u64);
                    }
                }
                0xC1 => match i.c {
                    1 => self.saved[i.r as usize] = self.reg[i.s as usize],
                    2 => self.saved[i.r as usize] = 0,
                    3 => self.reg[i.r as usize] = 0,
                    _ => self.reg[i.r as usize] = self.saved[i.s as usize],
                },
                0xC2 => {
                    for k in 0..16 {
                        if i.val & (1 << k) == 0 {
                            continue;
                        }
                        match i.c {
                            1 => self.saved[k] = self.reg[k],
                            2 => self.saved[k] = 0,
                            3 => self.reg[k] = 0,
                            _ => self.reg[k] = self.saved[k],
                        }
                    }
                }
                0xC3 => {
                    if i.x < 0x80 {
                        self.reg[i.r as usize] = self.statics[i.x as usize];
                    } else {
                        self.statics[i.x as usize] = self.reg[i.r as usize];
                    }
                }
                op => return Err(format!("unsupported opcode {op:X}")),
            }
            idx += i.len;
        }
        Ok(())
    }

    /// One pass: registers reset, then each cheat in order (`cheats`: the masters and the enabled ones).
    pub fn pass(&mut self, cheats: &mut [Cheat], mem: &mut dyn Mem, held: u64) {
        self.reg = [0; 16];
        self.saved = [0; 16];
        self.held = held;
        for c in cheats.iter_mut() {
            if !c.enabled || c.broken.is_some() {
                continue;
            }
            let before = c.failed;
            if let Err(e) = self.run(&c.words, mem, &mut c.failed) {
                crate::log!("[RED] cheat {:?} turned off: {e}", c.name);
                c.broken = Some(e);
                c.enabled = false;
            } else if before == 0 && c.failed > 0 {
                crate::log!("[RED] cheat {:?}: a memory access failed (a pointer not set yet?); it stays on and retries", c.name);
            }
        }
        self.keyold = held;
    }
}

// ---- the game's memory and keys

struct GuestMem {
    main: u64,
    main_text: (u64, u64),
}

impl Mem for GuestMem {
    fn base(&self, m: u32) -> u64 {
        let space = crate::mem::try_get().map(|m| m.space);
        match m {
            1 => space.map_or(0, |s| s.heap.base),
            2 => space.map_or(0, |s| s.alias.base),
            3 => space.map_or(0, |s| s.aslr.base),
            4 => 0,
            _ => self.main,
        }
    }
    fn read(&mut self, a: u64, w: u32) -> Option<u64> {
        if a < 0x1_0000 || !nxrt::mem::readable(a, w as u64) {
            return None;
        }
        let b = crate::guest::bytes(a, w as u64);
        let mut v = [0u8; 8];
        v[..w as usize].copy_from_slice(b);
        Some(u64::from_le_bytes(v))
    }
    fn write(&mut self, a: u64, w: u32, v: u64) -> bool {
        if a < 0x1_0000 || !writable(a, w as u64) {
            return false;
        }
        if a >= self.main_text.0 && a < self.main_text.1 {
            crate::log_once!("cheat code write", "[RED] cheats: a cheat writes main's code at {a:#x}: the game runs code translated at build time, so the change has no effect");
        }
        crate::guest::write_bytes(a, &v.to_le_bytes()[..w as usize]);
        true
    }
}

#[cfg(target_os = "linux")]
fn writable(a: u64, n: u64) -> bool {
    nxrt::linux::writable(a, n)
}

#[cfg(windows)]
fn writable(a: u64, n: u64) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, PAGE_WRITECOPY};
    let mut p = a;
    while p < a + n {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { VirtualQuery(p as *const _, &mut mbi, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 || mbi.State != 0x1000 {
            return false;
        }
        if mbi.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE | PAGE_WRITECOPY) == 0 {
            return false;
        }
        p = mbi.BaseAddress as u64 + mbi.RegionSize as u64;
    }
    true
}

/// Player 1's keys as the cheat VM numbers them (nn::hid::NpadButton: the buttons, then the sticks' directions).
fn held_keys() -> u64 {
    let (b, a) = crate::input::peek_player1();
    let mut k = b & 0xFFFF;
    let d = 16384;
    let dirs = [(a[0] < -d, 16), (a[1] > d, 17), (a[0] > d, 18), (a[1] < -d, 19), (a[2] < -d, 20), (a[3] > d, 21), (a[2] > d, 22), (a[3] < -d, 23)];
    for (on, bit) in dirs {
        if on {
            k |= 1 << bit;
        }
    }
    k
}

struct State {
    file: std::path::PathBuf,
    build: String,
    cheats: Vec<Cheat>,
    vm: Vm,
    mem: GuestMem,
}

static STATE: Mutex<Option<Option<State>>> = Mutex::new(None);

fn folder() -> std::path::PathBuf {
    std::env::var("NXOS_CHEATS").map(Into::into).unwrap_or_else(|_| crate::boot::try_process().map(|p| p.data_dir.join("cheats")).unwrap_or_default())
}

fn load() -> Option<State> {
    let m = crate::fpslock::main()?;
    let build: String = m.build[..16.min(m.build.len())].to_ascii_uppercase();
    let dir = folder();
    let tid = format!("{:016X}", crate::profile::get().program_id);
    let candidates = [dir.join(format!("{build}.txt")), dir.join("atmosphere").join("contents").join(&tid).join("cheats").join(format!("{build}.txt"))];
    let file = candidates.into_iter().find(|p| p.is_file())?;
    let text = std::fs::read_to_string(&file).ok()?;
    let mut cheats = parse(&text);
    let enabled: Vec<String> = std::fs::read_to_string(file.with_extension("enabled")).unwrap_or_default().lines().map(|l| l.trim().to_string()).collect();
    let on: Vec<String> = std::env::var("NXOS_CHEATS_ON").unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    for c in cheats.iter_mut() {
        if !c.master && (enabled.contains(&c.name) || on.contains(&c.name)) {
            c.enabled = true;
        }
    }
    let n_on = cheats.iter().filter(|c| c.enabled && !c.master).count();
    let broken: Vec<String> = cheats.iter().filter_map(|c| c.broken.as_ref().map(|b| format!("{} ({b})", c.name))).collect();
    crate::log!(
        "cheats: {} ({} cheats, {} masters, {n_on} on){}",
        file.display(),
        cheats.iter().filter(|c| !c.master).count(),
        cheats.iter().filter(|c| c.master).count(),
        if broken.is_empty() { String::new() } else { format!("; off for good: {}", broken.join(", ")) }
    );
    Some(State { file, build, cheats, vm: Vm::default(), mem: GuestMem { main: m.base, main_text: (m.base, m.base + m.text_end) } })
}

fn save_enabled(st: &State) {
    let names: Vec<&str> = st.cheats.iter().filter(|c| c.enabled && !c.master).map(|c| c.name.as_str()).collect();
    let _ = std::fs::write(st.file.with_extension("enabled"), names.join("\n") + "\n");
}

/// Each present: one pass of the masters and the enabled cheats.
pub fn present() {
    let mut g = STATE.lock().unwrap();
    let st = g.get_or_insert_with(load);
    let Some(st) = st.as_mut() else { return };
    let held = held_keys();
    let State { cheats, vm, mem, .. } = st;
    vm.pass(cheats, mem, held);
}

/// The control pipe's cheat commands.
pub fn register_commands() {
    nxconfig::ctl::register("cheats", "[reload]: the cheats of this build and whether each is on (reload: read the file again)", |a| {
        let mut g = STATE.lock().unwrap();
        if a.first() == Some(&"reload") {
            *g = Some(load());
        }
        match g.as_ref().and_then(|s| s.as_ref()) {
            None => format!("no cheat file for this build in {}\n", folder().display()),
            Some(st) => {
                let mut s = format!("{} (build {})\n", st.file.display(), st.build);
                for c in &st.cheats {
                    s.push_str(&format!(
                        "{} {}{}\n",
                        if c.master { "master" } else if c.enabled { "on    " } else { "off   " },
                        c.name,
                        c.broken.as_ref().map(|b| format!(" (off: {b})")).unwrap_or_default()
                    ));
                }
                s
            }
        }
    });
    nxconfig::ctl::register("cheat", "on|off <name>: turn a cheat on or off (kept in <build>.enabled)", |a| {
        let (Some(what), name) = (a.first(), a[1.min(a.len())..].join(" ")) else { return "cheat on|off <name>\n".into() };
        let mut g = STATE.lock().unwrap();
        let Some(Some(st)) = g.as_mut() else { return "no cheats loaded\n".into() };
        let Some(c) = st.cheats.iter_mut().find(|c| c.name == name && !c.master) else { return format!("no cheat {name:?}\n") };
        if c.broken.is_some() {
            return format!("{name}: cannot run ({})\n", c.broken.as_ref().unwrap());
        }
        c.enabled = *what == "on";
        let r = format!("{name}: {}\n", if c.enabled { "on" } else { "off" });
        save_enabled(st);
        crate::log!("cheats: {}", r.trim());
        r
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fake {
        mem: HashMap<u64, u8>,
        base: u64,
    }
    impl Mem for Fake {
        fn base(&self, m: u32) -> u64 {
            if m == 4 { 0 } else { self.base }
        }
        fn read(&mut self, a: u64, w: u32) -> Option<u64> {
            let mut v = 0u64;
            for k in 0..w as u64 {
                v |= (*self.mem.get(&(a + k))? as u64) << (8 * k);
            }
            Some(v)
        }
        fn write(&mut self, a: u64, w: u32, v: u64) -> bool {
            if a == 0xdead0000 {
                return false;
            }
            for k in 0..w as u64 {
                self.mem.insert(a + k, (v >> (8 * k)) as u8);
            }
            true
        }
    }
    fn fake() -> Fake {
        let mut m = Fake { mem: HashMap::new(), base: 0x1000_0000 };
        for a in 0..0x100 {
            m.mem.insert(0x1000_0000 + a, 0);
        }
        m
    }

    #[test]
    fn parse_entries() {
        let t = "{Master}\n580F0000 01234567\n[Infinite Money]\n04000000 00ABCDEF 0098967F\nby someone\n[Label only]\n[99 Lives][Glued]\n04000000 00000010 00000063\n";
        let c = parse(t);
        assert_eq!(c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["Master", "Infinite Money", "Glued"]);
        assert!(c[0].master && c[0].enabled && !c[1].enabled);
        assert_eq!(c[1].words, vec![0x0400_0000, 0x00AB_CDEF, 0x0098_967F]);
        let bad = parse("[Pause]\nFF000000\n");
        assert!(bad[0].broken.as_deref().unwrap().contains("unsupported opcode"));
    }

    /// Opcode by opcode, as the reference runs them.
    #[test]
    fn vm_opcodes() {
        let mut m = fake();
        let mut vm = Vm::default();
        let mut failed = 0;
        // 0: store 4 bytes 0x63 at main + 0x10; 1: if [main+0x10] == 0x63 then store 2 bytes 7 at +0x20, else 9
        let p = [0x0400_0000, 0x10, 0x63, 0x1405_0000, 0x10, 0x63, 0x0200_0000, 0x20, 7, 0x2100_0000, 0x0200_0000, 0x20, 9, 0x2000_0000];
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!((m.read(0x1000_0010, 4), m.read(0x1000_0020, 2)), (Some(0x63), Some(7)));
        // 4 / 5 / 7 / 9 / A: r0 = r1 = main + 0x10; r1 = [r1] (5, from a register); r1 += 5 (7); r2 = r1 * 2 (9 immediate);
        // [r0 + 8] = r2 (A, offset type 2)
        let p = [0x4000_0000, 0, 0x1000_0010, 0x4001_0000, 0, 0x1000_0010, 0x5401_1000, 0, 0x7401_0000, 5, 0x9422_1100, 2, 0xA420_0200, 8];
        let mut vm = Vm::default();
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(vm.reg[1], 0x68);
        assert_eq!(m.read(0x1000_0018, 4), Some(0xd0));
        // 3: a loop of 4 writing [r0] with 6 and post increment (I=1)
        let p = [0x4000_0000, 0, 0x1000_0040, 0x3001_0000, 4, 0x6100_1000, 0, 0xAA, 0x3101_0000];
        let mut vm = Vm::default();
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(m.read(0x1000_0040, 4), Some(0xAAAA_AAAA));
        // 8 / C4: keys
        let p = [0x8000_0003, 0x0400_0000, 0x80, 1, 0x2000_0000];
        let mut vm = Vm { held: 1, ..Default::default() };
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(m.read(0x1000_0080, 4), Some(0), "A alone: A+B not held");
        vm.held = 3;
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(m.read(0x1000_0080, 4), Some(1));
        // C0 against a static value, C1 save / restore, C3 statics
        let p = [0x4000_0000, 0, 5, 0xC042_0400, 5, 0x4001_0000, 0, 9, 0x2000_0000, 0xC101_0110, 0xC300_0801];
        let mut vm = Vm::default();
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!((vm.reg[1], vm.saved[1], vm.statics[0x80]), (9, 9, 9));
        // a failed write is counted, nothing stops
        let p = [0x4000_0000, 0, 0xdead0000, 0x6400_0000, 0, 1];
        let mut vm = Vm::default();
        let mut f = 0;
        vm.run(&p, &mut m, &mut f).unwrap();
        assert_eq!(f, 1);
        // float add (9, C = 10, T = 4)
        let p = [0x4000_0000, 0, 1.5f32.to_bits(), 0x94A0_0100, 2.25f32.to_bits()];
        let mut vm = Vm::default();
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(f32::from_bits(vm.reg[0] as u32), 3.75);
        // an else at depth 0 turns the cheat off
        assert!(Vm::default().run(&[0x2100_0000], &mut m, &mut failed).is_err());
    }
}
