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
//!   compares 0, a store writes nothing; the first failure of a cheat logs one line.
//! * Code cheats (risk R2): the game runs code translated at build time, so the build compiles a cheat's stores into
//!   main's code in as a patch set of its own (cli `cheats.rs`, `profiles::cheatfile`); the game hands its switch
//!   table to the runtime (`nxrt::patchset`) and the cheat's toggle flips its set. A code cheat the build did not
//!   compile in is said in red (rebuild with the file in place); its writes reach the bytes only.
//! * `<build>.meta.tsv`, `<build>.values` and the panel (F6, Home + Minus): below.

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
    // ---- from `<build>.meta.tsv` (all empty without one)
    pub section: String,
    pub note: String,
    /// The value's type ("u32", "s16", "f32"...).
    pub ty: String,
    /// "set": runs one pass, then turns itself off.
    pub once: bool,
    /// The off half of an on / off pair just switched off: runs one pass (the restore), then off.
    pub restore_once: bool,
    /// The off (restore) half of a pair, and the index of its partner.
    pub pair_off: bool,
    pub pair: Option<usize>,
    pub pair_name: String,
    /// "ask": a value the player may set (bounds), held in the program words `vw` (an 8-byte value: high, low).
    pub ask: Option<(Option<f64>, Option<f64>)>,
    pub vw: Vec<usize>,
    /// Never-decrease: (region 0 main / 1 heap, offset) of a typed value the host keeps at its highest.
    pub nd: Option<(u32, u64)>,
    pub nd_max: Option<u64>,
    /// The switch of its compiled-in patch set (`profiles::cheatfile::env_name`): the build made one when the cheat
    /// writes main's code; `code` once the game's code has that set (the toggle flips it).
    pub env: String,
    pub code: bool,
    /// Every write is compiled in (stores into main's code only): the VM has nothing left to do for it.
    pub code_only: bool,
}

/// A cheat file's entries (labels, entries without code, left out).
pub fn parse(text: &str) -> Vec<Cheat> {
    let es = profiles::cheatfile::entries(text);
    let envs = profiles::cheatfile::env_names(&es);
    es.into_iter()
        .zip(envs)
        .map(|(e, env)| {
            let broken = validate(&e.words).err();
            Cheat {
                name: e.name,
                enabled: e.master,
                master: e.master,
                words: e.words,
                broken,
                env,
                ..Default::default()
            }
        })
        .collect()
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
    if hi == 0xC {
        w >> 24
    } else {
        hi
    }
}

/// One instruction at `p` (the words left), the reference's DecodeNextOpcode field by field.
fn decode(p: &[u32]) -> Result<Ins, String> {
    let n = p.len();
    if n == 0 {
        return Err("truncated".into());
    }
    let w = p[0];
    let mut o = Ins {
        op: op_kind(w),
        len: 1,
        ..Default::default()
    };
    let need = |k: usize| {
        if n < k {
            Err("truncated".to_string())
        } else {
            Ok(())
        }
    };
    let width = |t: u32| {
        if width_ok(t) {
            Ok(t)
        } else {
            Err(format!("width {t}"))
        }
    };
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
    pub fn run(
        &mut self,
        words: &[u32],
        mem: &mut dyn Mem,
        failed: &mut u64,
    ) -> Result<(), String> {
        if self.statics.len() < 256 {
            self.statics.resize(256, 0);
        }
        let (mut idx, mut depth, mut steps) = (0usize, 0i32, 0u64);
        let mut looptop = [0usize; 16];
        while idx < words.len() {
            steps += 1;
            if steps > STEP_CAP {
                return Err(format!(
                    "over {STEP_CAP} instructions in one pass: a runaway loop"
                ));
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
                    if !mem.write(
                        mem.base(i.m) + i.rel + self.reg[i.r as usize],
                        i.t,
                        i.val & width_mask(i.t),
                    ) {
                        *failed += 1;
                    }
                }
                1 | 8 | 0xC4 => {
                    depth += 1;
                    let met = match i.op {
                        8 => self.held & i.val == i.val,
                        0xC4 => {
                            let now = self.held & i.val == i.val;
                            if i.flags != 0 {
                                now
                            } else {
                                now && self.keyold & i.val != i.val
                            }
                        }
                        _ => {
                            let a = mem.base(i.m)
                                + i.rel
                                + if i.flags != 0 {
                                    self.reg[i.x as usize]
                                } else {
                                    0
                                };
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
                        0 => {
                            let a = mem.base(i.m) + i.rel;
                            rd(mem, a, i.t, failed)
                        }
                        1 => {
                            let a = mem.base(i.m) + self.reg[i.x as usize];
                            rd(mem, a, i.t, failed)
                        }
                        2 => rd(mem, self.reg[i.r as usize] + i.rel, i.t, failed),
                        3 => rd(
                            mem,
                            self.reg[i.r as usize] + self.reg[i.x as usize],
                            i.t,
                            failed,
                        ),
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
                    let a = self.reg[i.r as usize].wrapping_add(if i.flags & 2 != 0 {
                        self.reg[i.x as usize]
                    } else {
                        0
                    });
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
                    let b = if i.flags != 0 {
                        i.val
                    } else {
                        self.reg[i.x as usize]
                    };
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
    /// One pass; true when a one-pass cheat ("set", a pair's restore) turned itself off (the toggles to save).
    pub fn pass(&mut self, cheats: &mut [Cheat], mem: &mut dyn Mem, held: u64) -> bool {
        self.reg = [0; 16];
        self.saved = [0; 16];
        self.held = held;
        let mut fired = false;
        for c in cheats.iter_mut() {
            if !(c.enabled || c.restore_once) || c.broken.is_some() {
                continue;
            }
            if c.nd.is_some() {
                nd_step(c, mem);
                continue;
            }
            if c.code_only {
                continue;
            }
            if (c.once || c.restore_once) && !c.master {
                c.enabled = false;
                c.restore_once = false;
                fired = true;
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
        fired
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

/// The web: committed linear memory is writable (no page protection).
#[cfg(target_os = "emscripten")]
fn writable(a: u64, n: u64) -> bool {
    nxrt::mem::readable(a, n)
}

#[cfg(windows)]
fn writable(a: u64, n: u64) -> bool {
    use windows_sys::Win32::System::Memory::{
        VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE,
        PAGE_WRITECOPY,
    };
    let mut p = a;
    while p < a + n {
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            VirtualQuery(
                p as *const _,
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
            || mbi.State != 0x1000
        {
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
    let dirs = [
        (a[0] < -d, 16),
        (a[1] > d, 17),
        (a[0] > d, 18),
        (a[1] < -d, 19),
        (a[2] < -d, 20),
        (a[3] > d, 21),
        (a[2] > d, 22),
        (a[3] < -d, 23),
    ];
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
    /// Cheats the meta file says its compiler left out: (name, why).
    unresolved: Vec<(String, String)>,
    vm: Vm,
    mem: GuestMem,
}

static STATE: Mutex<Option<Option<State>>> = Mutex::new(None);

fn folder() -> std::path::PathBuf {
    std::env::var("NXOS_CHEATS")
        .map(Into::into)
        .unwrap_or_else(|_| {
            crate::boot::try_process()
                .map(|p| p.data_dir.join("cheats"))
                .unwrap_or_default()
        })
}

fn load() -> Option<State> {
    let m = crate::fpslock::main()?;
    let build: String = m.build[..16.min(m.build.len())].to_ascii_uppercase();
    let dir = folder();
    let tid = format!("{:016X}", crate::profile::get().program_id);
    let candidates = [
        dir.join(format!("{build}.txt")),
        dir.join("atmosphere")
            .join("contents")
            .join(&tid)
            .join("cheats")
            .join(format!("{build}.txt")),
    ];
    let file = candidates.into_iter().find(|p| p.is_file())?;
    let text = std::fs::read_to_string(&file).ok()?;
    let mut cheats = parse(&text);
    let meta = std::fs::read_to_string(file.with_extension("meta.tsv")).unwrap_or_default();
    let unresolved = meta_load(&mut cheats, &meta);
    link_pairs(&mut cheats);
    values_load(
        &mut cheats,
        &std::fs::read_to_string(file.with_extension("values")).unwrap_or_default(),
    );
    let enabled: Vec<String> = std::fs::read_to_string(file.with_extension("enabled"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .collect();
    let on: Vec<String> = std::env::var("NXOS_CHEATS_ON")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for c in cheats.iter_mut() {
        if !c.master && !c.pair_off && (enabled.contains(&c.name) || on.contains(&c.name)) {
            c.enabled = true;
        }
    }
    bind_code(&mut cheats, (m.text_end, nxrt::patchset::has));
    let n_on = cheats.iter().filter(|c| c.enabled && !c.master).count();
    let broken: Vec<String> = cheats
        .iter()
        .filter_map(|c| c.broken.as_ref().map(|b| format!("{} ({b})", c.name)))
        .collect();
    crate::log!(
        "cheats: {} ({} cheats, {} masters, {n_on} on){}",
        file.display(),
        cheats.iter().filter(|c| !c.master).count(),
        cheats.iter().filter(|c| c.master).count(),
        if broken.is_empty() {
            String::new()
        } else {
            format!("; off for good: {}", broken.join(", "))
        }
    );
    Some(State {
        file,
        build,
        cheats,
        unresolved,
        vm: Vm::default(),
        mem: GuestMem {
            main: m.base,
            main_text: (m.base, m.base + m.text_end),
        },
    })
}

fn save_enabled(st: &State) {
    let names: Vec<&str> = st
        .cheats
        .iter()
        .filter(|c| c.enabled && !c.master && !c.once && !c.pair_off)
        .map(|c| c.name.as_str())
        .collect();
    let _ = std::fs::write(st.file.with_extension("enabled"), names.join("\n") + "\n");
    let vals: Vec<String> = st
        .cheats
        .iter()
        .filter(|c| c.ask.is_some())
        .filter_map(|c| value_get(c).map(|v| format!("{}\t{v}", c.name)))
        .collect();
    if !vals.is_empty() {
        let _ = std::fs::write(st.file.with_extension("values"), vals.join("\n") + "\n");
    }
}

/// Each present: one pass of the masters and the enabled cheats.
pub fn present() {
    let mut g = STATE.lock().unwrap();
    let st = g.get_or_insert_with(load);
    let Some(st) = st.as_mut() else { return };
    let held = held_keys();
    let State {
        cheats, vm, mem, ..
    } = st;
    if vm.pass(cheats, mem, held) {
        save_enabled(st);
    }
}

/// The control pipe's cheat commands.
pub fn register_commands() {
    nxconfig::ctl::register(
        "cheats",
        "[reload]: the cheats of this build and whether each is on (reload: read the file again)",
        |a| {
            let mut g = STATE.lock().unwrap();
            if a.first() == Some(&"reload") {
                *g = Some(load());
            }
            match g.as_ref().and_then(|s| s.as_ref()) {
                None => format!("no cheat file for this build in {}\n", folder().display()),
                Some(st) => {
                    let mut s = format!("{} (build {})\n", st.file.display(), st.build);
                    for c in &st.cheats {
                        let mut extra = String::new();
                        if !c.section.is_empty() {
                            extra.push_str(&format!(" [{}]", c.section));
                        }
                        if c.once {
                            extra.push_str(" (one pass)");
                        }
                        if c.pair_off {
                            extra.push_str(" (the restore of its pair)");
                        }
                        if c.nd.is_some() {
                            extra.push_str(&format!(" (never decreases, {})", c.ty));
                        }
                        if let Some(v) = value_get(c) {
                            extra.push_str(&format!(" = {v} {}", c.ty));
                        }
                        if !c.note.is_empty() {
                            extra.push_str(&format!(" -- {}", c.note));
                        }
                        s.push_str(&format!(
                            "{} {}{extra}{}\n",
                            if c.master {
                                "master"
                            } else if c.enabled {
                                "on    "
                            } else {
                                "off   "
                            },
                            c.name,
                            c.broken
                                .as_ref()
                                .map(|b| format!(" (off: {b})"))
                                .unwrap_or_default()
                        ));
                    }
                    for (n, why) in &st.unresolved {
                        s.push_str(&format!("left out {n}: {why}\n"));
                    }
                    s
                }
            }
        },
    );
    nxconfig::ctl::register("cheat", "on|off <name> | set <name> <value>: turn a cheat on or off (kept in <build>.enabled), set an ask cheat's value (<build>.values)", |a| {
        let (Some(what), name) = (a.first(), a[1.min(a.len())..].join(" ")) else { return "cheat on|off <name>\n".into() };
        let mut g = STATE.lock().unwrap();
        let Some(Some(st)) = g.as_mut() else { return "no cheats loaded\n".into() };
        let r = match *what {
            "set" => {
                let (name, value) = name.rsplit_once(' ').unwrap_or((name.as_str(), ""));
                match st.cheats.iter_mut().find(|c| c.name == name) {
                    Some(c) => match value_set(c, value) {
                        Ok(()) => format!("{name} = {value}\n"),
                        Err(e) => return format!("{name}: {e}\n"),
                    },
                    None => return format!("no cheat {name:?}\n"),
                }
            }
            _ => match toggle(&mut st.cheats, &name, *what == "on") {
                Ok(r) => r,
                Err(e) => return e,
            },
        };
        save_enabled(st);
        crate::log!("cheats: {}", r.trim());
        r
    });
}

// ---- `<build>.meta.tsv` (nxnative hle_cheats.c meta_load; nxnative's scripts/cheat_anchor.py writes it beside a
// compiled <build>.txt): per entry its section, note, type, an on / off pair, a value the player may set ("ask", kept
// in <build>.values), "set" cheats that run one pass and turn themselves off, and never-decrease, a host action that
// keeps a typed value at the highest seen. Columns are read by their header names; rows go to the entries of the same
// name in file order; "unresolved" rows (cheats the compiler left out) are listed.

// ---- the cheat panel: a small window over the game (the prompts' window, `ui.rs`) listing the cheats with their state
// and values; opened from the front end (F6, or Home + Minus on a controller). It acts through the same functions as
// the control commands (`toggle`, `value_set`, `save_enabled`).

/// What one step of the panel asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAct {
    None,
    /// Turn row r's cheat on / off.
    Toggle(usize),
    /// Change row r's value by a step (-1 / +1).
    Step(usize, i32),
    Close,
}

/// The panel's navigation over its rows: D-pad up / down or the arrows move (wrapping), A or Return toggles, left /
/// right steps a value, B or Escape closes. Pads count on the press (a button held as it opened, or held on, counts
/// once).
pub struct CheatPanel {
    pub cursor: usize,
    /// A, B, up, down, left, right as last seen.
    held: [bool; 6],
}

impl CheatPanel {
    pub fn open(held: [bool; 6]) -> CheatPanel {
        CheatPanel { cursor: 0, held }
    }
    /// One step over `n` rows: the pads (A, B, up, down, left, right), Return / Escape, the vertical and horizontal
    /// arrows typed since the last step.
    pub fn step(
        &mut self,
        n: usize,
        pads: [bool; 6],
        confirm: bool,
        cancel: bool,
        vert: i32,
        horiz: i32,
    ) -> PanelAct {
        let e: Vec<bool> = (0..6).map(|i| pads[i] && !self.held[i]).collect();
        self.held = pads;
        if e[1] || cancel {
            return PanelAct::Close;
        }
        if n == 0 {
            return PanelAct::None;
        }
        self.cursor = self.cursor.min(n - 1);
        if e[2] || vert < 0 {
            self.cursor = (self.cursor + n - 1) % n;
        }
        if e[3] || vert > 0 {
            self.cursor = (self.cursor + 1) % n;
        }
        if e[0] || confirm {
            return PanelAct::Toggle(self.cursor);
        }
        if e[4] || horiz < 0 {
            return PanelAct::Step(self.cursor, -1);
        }
        if e[5] || horiz > 0 {
            return PanelAct::Step(self.cursor, 1);
        }
        PanelAct::None
    }
}

/// The rows the panel lists: every cheat but the restore halves of pairs (their on half is the switch).
pub fn panel_rows(cheats: &[Cheat]) -> Vec<usize> {
    (0..cheats.len()).filter(|&i| !cheats[i].pair_off).collect()
}

/// The panel's text: up to 10 rows around the cursor (the selected one marked), then the keys.
pub fn panel_text(cheats: &[Cheat], rows: &[usize], cursor: usize) -> String {
    if rows.is_empty() {
        return "No cheats for this build.\nB / Esc: close".into();
    }
    let first = cursor.saturating_sub(4).min(rows.len().saturating_sub(10));
    let mut s = String::new();
    for (k, &i) in rows.iter().enumerate().skip(first).take(10) {
        let c = &cheats[i];
        let state = if c.master {
            "always"
        } else if c.broken.is_some() {
            "cannot run"
        } else if c.enabled {
            "ON"
        } else {
            "off"
        };
        let value = value_get(c)
            .map(|v| format!("   < {v} >"))
            .unwrap_or_default();
        let mark = if k == cursor { "\u{25b6} " } else { "   " };
        s.push_str(&format!(
            "{mark}{}{}  [{state}]{value}\n",
            if c.section.is_empty() {
                String::new()
            } else {
                format!("{}: ", c.section)
            },
            c.name
        ));
    }
    s.push_str(&format!(
        "{} of {}   A / Return: on-off   Left / Right: value   B / Esc: close",
        cursor + 1,
        rows.len()
    ));
    s
}

/// A value one step up or down in its type (1 for integers and floats), within its bounds.
pub fn value_step(c: &mut Cheat, d: i32) -> Result<(), String> {
    let v: f64 = value_get(c)
        .ok_or("takes no value")?
        .parse()
        .map_err(|_| "no value".to_string())?;
    let n = v + d as f64;
    let text = if c.ty.starts_with('f') {
        format!("{n}")
    } else {
        format!("{}", n as i128)
    };
    value_set(c, &text)
}

static PANEL_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Opens the cheat panel over the game (on a thread of its own; nothing if it is open already). While it is open
/// the game reads no pads and is out of focus, as under the console's menus.
pub fn panel_open() {
    if PANEL_OPEN.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if std::thread::Builder::new()
        .name("nxos cheat panel".into())
        .spawn(panel_run)
        .is_err()
    {
        PANEL_OPEN.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

fn panel_pads() -> [bool; 6] {
    let n = crate::ui::pads_nav();
    let (l, r) = crate::ui::pads_lr();
    [n[0], n[1], n[2], n[3], l, r]
}

fn panel_run() {
    crate::log!("cheats: panel open");
    crate::services::oe::applet_enter("cheat panel");
    crate::input::set_prompt(true);
    let mut p = CheatPanel::open(panel_pads());
    let mut shown = String::new();
    let _ = crate::ui::keys();
    let _ = crate::ui::nav_keys();
    let _ = crate::ui::hnav_keys();
    loop {
        let (confirm, cancel) = crate::ui::keys();
        let (vert, _) = crate::ui::nav_keys();
        let horiz = crate::ui::hnav_keys();
        let text = {
            let mut g = STATE.lock().unwrap();
            let st = g.get_or_insert_with(load);
            let rows = st
                .as_ref()
                .map(|s| panel_rows(&s.cheats))
                .unwrap_or_default();
            match p.step(rows.len(), panel_pads(), confirm, cancel, vert, horiz) {
                PanelAct::Close => break,
                PanelAct::Toggle(r) => {
                    if let Some(st) = st.as_mut() {
                        let i = rows[r];
                        if !st.cheats[i].master {
                            let (name, on) = (st.cheats[i].name.clone(), !st.cheats[i].enabled);
                            match toggle(&mut st.cheats, &name, on) {
                                Ok(r) => crate::log!("cheats (panel): {}", r.trim()),
                                Err(e) => crate::log!("cheats (panel): {}", e.trim()),
                            }
                            save_enabled(st);
                        }
                    }
                }
                PanelAct::Step(r, d) => {
                    if let Some(st) = st.as_mut() {
                        let i = rows[r];
                        if value_step(&mut st.cheats[i], d).is_ok() {
                            save_enabled(st);
                        }
                    }
                }
                PanelAct::None => {}
            }
            match st.as_ref() {
                Some(s) => panel_text(&s.cheats, &rows, p.cursor),
                None => panel_text(&[], &[], 0),
            }
        };
        if text != shown {
            crate::ui::show("Cheats", &text);
            shown = text;
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    crate::ui::hide();
    // the press that closed it never reaches the game
    while panel_pads()[0] || panel_pads()[1] {
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    crate::input::set_prompt(false);
    crate::services::oe::applet_leave();
    crate::log!("cheats: panel closed");
    PANEL_OPEN.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Marks the cheats the game's code has a patch set for (`has`: the runtime's table) and sets each set to the
/// cheat's state; `code_only` when every write of the cheat is a store into main's code (below `text_end`).
pub fn bind_code(cheats: &mut [Cheat], (text_end, has): (u64, fn(&str) -> bool)) {
    let mut n = 0;
    for c in cheats.iter_mut() {
        if c.env.is_empty() || !has(&c.env) {
            continue;
        }
        c.code = true;
        c.code_only = profiles::cheatfile::main_writes(&c.words)
            .is_some_and(|w| w.iter().all(|(o, b)| o + b.len() as u64 <= text_end));
        sync_code(c);
        n += 1;
    }
    if n > 0 {
        crate::log!("cheats: {n} code cheats compiled into this build, switched by their toggles");
    }
    for c in cheats.iter().filter(|c| {
        !c.code
            && profiles::cheatfile::main_writes(&c.words)
                .is_some_and(|w| w.iter().any(|(o, _)| *o < text_end))
    }) {
        crate::log!("[RED] cheats: {:?} writes main's code but is not compiled into this build (rebuild with the cheat file in place): its code writes have no effect", c.name);
    }
}

/// The compiled-in set of a code cheat follows its state (a master: always on).
fn sync_code(c: &Cheat) {
    if c.code {
        nxrt::patchset::set(&c.env, c.enabled || c.master);
    }
}

/// A meta type's width in bytes ("u8" .. "u64", "s16", "f32", "f64"); 0 unknown.
fn type_width(t: &str) -> u32 {
    match t.get(1..).and_then(|b| b.parse::<u32>().ok()) {
        Some(b @ (8 | 16 | 32 | 64)) => b / 8,
        _ => 0,
    }
}

/// Whether program word `k` is a value word (an immediate of opcodes 0 / 1 / 4 / 6 / 7, C0's immediate form, 9's
/// immediate): a value set may never rewrite an opcode word.
fn value_word(words: &[u32], k: usize) -> bool {
    let mut a = 0;
    while a < words.len() {
        let Ok(i) = decode(&words[a..]) else {
            return false;
        };
        let first = match i.op {
            0 | 1 => Some(a + 2),
            4 | 6 | 7 => Some(a + 1),
            0xC0 if i.o == 4 => Some(a + 1),
            9 if i.flags != 0 => Some(a + 1),
            _ => None,
        };
        if first.is_some_and(|f| k >= f && k < a + i.len) {
            return true;
        }
        a += i.len;
    }
    false
}

/// Applies a meta file's rows to the cheats; returns the unresolved ones (name, why).
pub fn meta_load(cheats: &mut Vec<Cheat>, text: &str) -> Vec<(String, String)> {
    let mut unresolved = vec![];
    let cols = [
        "name",
        "kind",
        "section",
        "type",
        "action",
        "ask",
        "value_words",
        "once",
        "pair",
        "resolved",
        "note",
        "address",
    ];
    let mut at: Option<Vec<Option<usize>>> = None;
    let mut cursor = 0;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').map(|x| x.trim_end_matches('\r')).collect();
        let Some(idx) = at.as_ref() else {
            let h: Vec<Option<usize>> =
                cols.iter().map(|c| f.iter().position(|x| x == c)).collect();
            if h[0].is_none() {
                break;
            }
            at = Some(h);
            continue;
        };
        let col = |k: usize| idx[k].and_then(|i| f.get(i).copied()).unwrap_or("");
        let (name, kind) = (col(0), col(1));
        if kind == "unresolved" {
            unresolved.push((name.to_string(), col(10).to_string()));
            continue;
        }
        let j = match (cursor..cheats.len()).find(|&j| cheats[j].name == name) {
            Some(j) => j,
            // a never-decrease entry has no code words (the file's entry is empty, so parsing left it out)
            None if col(4) == "never-decrease" => {
                cheats.push(Cheat {
                    name: name.to_string(),
                    ..Default::default()
                });
                cheats.len() - 1
            }
            None => continue,
        };
        cursor = j + 1;
        let c = &mut cheats[j];
        if c.section.is_empty() {
            c.section = col(2).to_string();
        }
        c.ty = col(3).to_string();
        c.once = col(7) == "1";
        c.pair_off = kind == "patch-off";
        let (res, note) = (col(9), col(10));
        c.note = [res, note]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("; ");
        if !col(5).is_empty() {
            let a = col(5);
            let (lo, hi) = a.split_once("..").unwrap_or((a, ""));
            c.ask = Some((lo.trim().parse().ok(), hi.trim().parse().ok()));
            c.vw = col(6)
                .split(|ch: char| !ch.is_ascii_digit())
                .filter_map(|s| s.parse().ok())
                .filter(|&v: &usize| v < c.words.len())
                .collect();
            let w = type_width(&c.ty);
            if w == 0
                || c.vw.is_empty()
                || (w == 8 && c.vw.len() % 2 != 0)
                || c.broken.is_some()
                || !c.vw.iter().all(|&k| value_word(&c.words, k))
            {
                c.ask = None;
            }
        }
        if col(4) == "never-decrease" && c.words.is_empty() {
            if let Some((r, off)) = col(11).split_once(':') {
                let off =
                    u64::from_str_radix(off.trim_start_matches("0x").trim_start_matches("0X"), 16)
                        .ok();
                if let (Ok(r @ 0..=1), Some(off), true) =
                    (r.trim().parse::<u32>(), off, type_width(&c.ty) != 0)
                {
                    c.nd = Some((r, off));
                }
            }
        }
        c.pair_name = col(8).to_string();
    }
    // pairs named by the meta file
    for i in 0..cheats.len() {
        if !cheats[i].pair_name.is_empty() {
            cheats[i].pair = cheats.iter().position(|c| c.name == cheats[i].pair_name);
        }
    }
    unresolved
}

/// "X - on" / "X - off" (also "X on", "X: off", "X (off)"): the base name and whether it is the off half.
fn onoff_split(name: &str) -> Option<(String, bool)> {
    let n = name.trim_end();
    let n = n
        .strip_suffix(')')
        .or_else(|| n.strip_suffix(']'))
        .unwrap_or(n);
    let low = n.to_ascii_lowercase();
    let (rest, off) = if low.ends_with("off") {
        (&n[..n.len() - 3], true)
    } else if low.ends_with("on") {
        (&n[..n.len() - 2], false)
    } else {
        return None;
    };
    let base = rest.trim_end_matches([' ', '-', ':', '_', '(', '[']);
    (base.len() < rest.len() && !base.is_empty()).then(|| (base.to_string(), off))
}

/// Links on / off pairs: the meta file's first, then the naming convention; both halves plain runnable cheats.
pub fn link_pairs(cheats: &mut [Cheat]) {
    for i in 0..cheats.len() {
        if cheats[i].pair.is_some() || cheats[i].master {
            continue;
        }
        let Some((base, off)) = onoff_split(&cheats[i].name) else {
            continue;
        };
        let j = (0..cheats.len()).find(|&j| {
            j != i
                && !cheats[j].master
                && onoff_split(&cheats[j].name)
                    .is_some_and(|(b, o)| b.eq_ignore_ascii_case(&base) && o != off)
        });
        if let Some(j) = j {
            cheats[i].pair = Some(j);
            cheats[i].pair_off = off;
        }
    }
    for i in 0..cheats.len() {
        if let Some(j) = cheats[i].pair {
            if cheats[j].pair.is_none() {
                cheats[j].pair = Some(i);
                cheats[j].pair_off = !cheats[i].pair_off;
            }
        }
    }
}

/// Turns a cheat on or off: turning the on half of a pair off runs its off half once (the restore); the off half is
/// never on by itself (turning it on or off acts on its partner). The reply line, or why not.
pub fn toggle(cheats: &mut [Cheat], name: &str, on: bool) -> Result<String, String> {
    let Some(mut i) = cheats.iter().position(|c| c.name == name && !c.master) else {
        return Err(format!("no cheat {name:?}\n"));
    };
    if cheats[i].pair_off {
        if let Some(j) = cheats[i].pair {
            i = j;
        }
    }
    if let Some(b) = &cheats[i].broken {
        return Err(format!("{}: cannot run ({b})\n", cheats[i].name));
    }
    let was = cheats[i].enabled;
    cheats[i].enabled = on;
    if cheats[i].nd.is_some() && on && !was {
        cheats[i].nd_max = None;
    }
    if let Some(j) = cheats[i].pair {
        cheats[j].enabled = false;
        cheats[j].restore_once = was && !on;
    }
    sync_code(&cheats[i]);
    Ok(format!(
        "{}: {}{}\n",
        cheats[i].name,
        if on { "on" } else { "off" },
        if was && !on && cheats[i].pair.is_some() {
            " (its restore runs once)"
        } else {
            ""
        }
    ))
}

fn width_bits(w: u32) -> u64 {
    if w >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * w)) - 1
    }
}

/// Sets an ask cheat's value from text in its type, within its bounds ("0x" hexadecimal, else decimal).
pub fn value_set(c: &mut Cheat, text: &str) -> Result<(), String> {
    let w = type_width(&c.ty);
    let Some((lo, hi)) = c.ask else {
        return Err("takes no value".into());
    };
    let t = text.trim();
    let int = |t: &str| -> Option<i128> {
        let (neg, d) = match t.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let v = match d.strip_prefix("0x").or_else(|| d.strip_prefix("0X")) {
            Some(h) => i128::from_str_radix(h, 16).ok()?,
            None => d.parse::<i128>().ok()?,
        };
        Some(if neg { -v } else { v })
    };
    let (raw, d): (u64, f64) = match c.ty.chars().next() {
        Some('f') => {
            let d: f64 = t.parse().map_err(|_| format!("not a number: {t}"))?;
            (
                if w == 4 {
                    (d as f32).to_bits() as u64
                } else {
                    d.to_bits()
                },
                d,
            )
        }
        Some('s') => {
            let v = int(t).ok_or(format!("not a number: {t}"))?;
            let b = 8 * w as i128;
            if v < -(1i128 << (b - 1)) || v >= 1i128 << (b - 1) {
                return Err(format!("{t} does not fit {}", c.ty));
            }
            (v as u64 & width_bits(w), v as f64)
        }
        _ => {
            let v = int(t).ok_or(format!("not a number: {t}"))?;
            if v < 0 || v > width_bits(w) as i128 {
                return Err(format!("{t} does not fit {}", c.ty));
            }
            (v as u64, v as f64)
        }
    };
    if lo.is_some_and(|m| d < m) || hi.is_some_and(|m| d > m) {
        return Err(format!("{t} is out of bounds"));
    }
    let per = if w == 8 { 2 } else { 1 };
    for k in c.vw.chunks(per).filter(|k| k.len() == per) {
        if per == 2 {
            c.words[k[0]] = (raw >> 32) as u32;
            c.words[k[1]] = raw as u32;
        } else {
            c.words[k[0]] = raw as u32;
        }
    }
    Ok(())
}

/// An ask cheat's value as text in its type.
pub fn value_get(c: &Cheat) -> Option<String> {
    let w = type_width(&c.ty);
    c.ask?;
    let raw = if w == 8 {
        (c.words[*c.vw.first()?] as u64) << 32 | c.words[*c.vw.get(1)?] as u64
    } else {
        c.words[*c.vw.first()?] as u64 & width_bits(w)
    };
    Some(match c.ty.chars().next() {
        Some('f') if w == 4 => format!("{}", f32::from_bits(raw as u32)),
        Some('f') => format!("{}", f64::from_bits(raw)),
        Some('s') => format!("{}", ((raw << (64 - 8 * w)) as i64) >> (64 - 8 * w)),
        _ => format!("{raw}"),
    })
}

/// `<build>.values`: one "name<TAB>value" a line (a value that no longer fits keeps the file's program words).
fn values_load(cheats: &mut [Cheat], text: &str) {
    for l in text.lines() {
        if let Some((n, v)) = l.split_once('\t') {
            if let Some(c) = cheats.iter_mut().find(|c| c.name == n && c.ask.is_some()) {
                let _ = value_set(c, v);
            }
        }
    }
}

/// a < b in the cheat's type: unsigned, signed or float (a NaN compares false, so it is never written back).
fn value_less(ty: &str, w: u32, a: u64, b: u64) -> bool {
    match ty.chars().next() {
        Some('f') if w == 4 => f32::from_bits(a as u32) < f32::from_bits(b as u32),
        Some('f') => f64::from_bits(a) < f64::from_bits(b),
        Some('s') => {
            let sh = 64 - 8 * w;
            ((a << sh) as i64 >> sh) < ((b << sh) as i64 >> sh)
        }
        _ => a < b,
    }
}

/// One never-decrease pass: the first notes the value; then a lower one is put back to the highest seen, a higher one
/// becomes the highest (the VM keeps no state between passes, so this is the host's action).
fn nd_step(c: &mut Cheat, mem: &mut dyn Mem) {
    let Some((r, off)) = c.nd else { return };
    let w = type_width(&c.ty);
    let base = mem.base(r);
    if w == 0 || base == 0 {
        return;
    }
    let a = base + off;
    let Some(cur) = mem.read(a, w) else { return };
    match c.nd_max {
        Some(m) if value_less(&c.ty, w, cur, m) => {
            mem.write(a, w, m);
        }
        _ => c.nd_max = Some(cur),
    }
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
            if m == 4 {
                0
            } else {
                self.base
            }
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
        let mut m = Fake {
            mem: HashMap::new(),
            base: 0x1000_0000,
        };
        for a in 0..0x100 {
            m.mem.insert(0x1000_0000 + a, 0);
        }
        m
    }

    #[test]
    fn parse_entries() {
        let t = "{Master}\n580F0000 01234567\n[Infinite Money]\n04000000 00ABCDEF 0098967F\nby someone\n[Label only]\n[99 Lives][Glued]\n04000000 00000010 00000063\n";
        let c = parse(t);
        assert_eq!(
            c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            vec!["Master", "Infinite Money", "Glued"]
        );
        assert!(c[0].master && c[0].enabled && !c[1].enabled);
        assert_eq!(c[1].words, vec![0x0400_0000, 0x00AB_CDEF, 0x0098_967F]);
        let bad = parse("[Pause]\nFF000000\n");
        assert!(bad[0]
            .broken
            .as_deref()
            .unwrap()
            .contains("unsupported opcode"));
    }

    /// Opcode by opcode, as the reference runs them.
    #[test]
    fn vm_opcodes() {
        let mut m = fake();
        let mut vm = Vm::default();
        let mut failed = 0;
        // 0: store 4 bytes 0x63 at main + 0x10; 1: if [main+0x10] == 0x63 then store 2 bytes 7 at +0x20, else 9
        let p = [
            0x0400_0000,
            0x10,
            0x63,
            0x1405_0000,
            0x10,
            0x63,
            0x0200_0000,
            0x20,
            7,
            0x2100_0000,
            0x0200_0000,
            0x20,
            9,
            0x2000_0000,
        ];
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(
            (m.read(0x1000_0010, 4), m.read(0x1000_0020, 2)),
            (Some(0x63), Some(7))
        );
        // 4 / 5 / 7 / 9 / A: r0 = r1 = main + 0x10; r1 = [r1] (5, from a register); r1 += 5 (7); r2 = r1 * 2 (9 immediate);
        // [r0 + 8] = r2 (A, offset type 2)
        let p = [
            0x4000_0000,
            0,
            0x1000_0010,
            0x4001_0000,
            0,
            0x1000_0010,
            0x5401_1000,
            0,
            0x7401_0000,
            5,
            0x9422_1100,
            2,
            0xA420_0200,
            8,
        ];
        let mut vm = Vm::default();
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(vm.reg[1], 0x68);
        assert_eq!(m.read(0x1000_0018, 4), Some(0xd0));
        // 3: a loop of 4 writing [r0] with 6 and post increment (I=1)
        let p = [
            0x4000_0000,
            0,
            0x1000_0040,
            0x3001_0000,
            4,
            0x6100_1000,
            0,
            0xAA,
            0x3101_0000,
        ];
        let mut vm = Vm::default();
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(m.read(0x1000_0040, 4), Some(0xAAAA_AAAA));
        // 8 / C4: keys
        let p = [0x8000_0003, 0x0400_0000, 0x80, 1, 0x2000_0000];
        let mut vm = Vm {
            held: 1,
            ..Default::default()
        };
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(m.read(0x1000_0080, 4), Some(0), "A alone: A+B not held");
        vm.held = 3;
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(m.read(0x1000_0080, 4), Some(1));
        // C0 against a static value, C1 save / restore, C3 statics
        let p = [
            0x4000_0000,
            0,
            5,
            0xC042_0400,
            5,
            0x4001_0000,
            0,
            9,
            0x2000_0000,
            0xC101_0110,
            0xC300_0801,
        ];
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
        let p = [
            0x4000_0000,
            0,
            1.5f32.to_bits(),
            0x94A0_0100,
            2.25f32.to_bits(),
        ];
        let mut vm = Vm::default();
        vm.run(&p, &mut m, &mut failed).unwrap();
        assert_eq!(f32::from_bits(vm.reg[0] as u32), 3.75);
        // an else at depth 0 turns the cheat off
        assert!(Vm::default()
            .run(&[0x2100_0000], &mut m, &mut failed)
            .is_err());
    }

    /// <build>.meta.tsv as nxnative reads it: section / type / note, an ask value (bounds, value words, set and read
    /// back, <build>.values), a one-pass "set" cheat, an on / off pair by name (turning it off runs the restore once),
    /// a never-decrease host action on an entry with no code, and the compiler's unresolved list.
    #[test]
    fn meta_tsv() {
        let txt = "[Coins]
04000000 00000010 00000005
[Max once]
04000000 00000014 00000007
[Infinite HP - on]
04000000 00000018 000003E7
[Infinite HP - off]
04000000 00000018 00000064
[Score floor]
";
        let meta = "# build 0123456789ABCDEF 1.0.0
name	kind	section	type	action	ask	value_words	once	pair	resolved	note	address
Coins	cheat	Items	u32		0..9999	2			strong	the purse	
Max once	cheat		u32				1				
Score floor	cheat		u32	never-decrease							0:0x20
Lost one	unresolved									no anchor	
";
        let mut c = parse(txt);
        assert_eq!(c.len(), 4, "the empty entry is left out by the parser");
        let un = meta_load(&mut c, meta);
        link_pairs(&mut c);
        assert_eq!(un, vec![("Lost one".to_string(), "no anchor".to_string())]);
        assert_eq!(
            (c[0].section.as_str(), c[0].ty.as_str(), c[0].note.as_str()),
            ("Items", "u32", "strong; the purse")
        );
        assert_eq!(
            (c[0].ask, c[0].vw.clone()),
            (Some((Some(0.0), Some(9999.0))), vec![2])
        );
        assert_eq!(value_get(&c[0]).as_deref(), Some("5"));
        assert!(value_set(&mut c[0], "10000").is_err() && value_set(&mut c[0], "-1").is_err());
        values_load(
            &mut c,
            "Coins	0x4D2
",
        );
        assert_eq!(c[0].words[2], 1234);
        assert!(c[1].once);
        assert_eq!(
            (c[2].pair, c[2].pair_off, c[3].pair, c[3].pair_off),
            (Some(3), false, Some(2), true)
        );
        let nd = c.iter().position(|x| x.name == "Score floor").unwrap();
        assert_eq!(c[nd].nd, Some((0, 0x20)));

        let mut m = fake();
        let mut vm = Vm::default();
        let rd = |m: &mut Fake, o: u64| m.read(0x1000_0000 + o, 4).unwrap();
        c[1].enabled = true;
        assert!(
            vm.pass(&mut c, &mut m, 0),
            "a one-pass cheat turned itself off"
        );
        assert_eq!((rd(&mut m, 0x14), c[1].enabled), (7, false));
        toggle(&mut c, "Infinite HP - on", true).unwrap();
        vm.pass(&mut c, &mut m, 0);
        assert_eq!(rd(&mut m, 0x18), 999);
        assert!(toggle(&mut c, "Infinite HP - on", false)
            .unwrap()
            .contains("restore"));
        assert!(vm.pass(&mut c, &mut m, 0));
        assert_eq!(rd(&mut m, 0x18), 100, "the restore ran");
        m.write(0x1000_0018, 4, 5);
        vm.pass(&mut c, &mut m, 0);
        assert_eq!(rd(&mut m, 0x18), 5, "once only");
        toggle(&mut c, "Score floor", true).unwrap();
        m.write(0x1000_0020, 4, 50);
        vm.pass(&mut c, &mut m, 0);
        m.write(0x1000_0020, 4, 30);
        vm.pass(&mut c, &mut m, 0);
        assert_eq!(rd(&mut m, 0x20), 50, "put back to the highest");
        m.write(0x1000_0020, 4, 80);
        vm.pass(&mut c, &mut m, 0);
        m.write(0x1000_0020, 4, 79);
        vm.pass(&mut c, &mut m, 0);
        assert_eq!(rd(&mut m, 0x20), 80);
        assert_eq!(
            onoff_split("God mode (off)"),
            Some(("God mode".to_string(), true))
        );
        assert_eq!(onoff_split("Wagon"), None);
    }

    /// A code cheat compiled into the build: its set in the game's switch table (handed to the runtime) follows its
    /// toggle; the VM leaves a cheat whose every write is compiled in alone; a master's set is on; a code cheat the
    /// build did not compile in is not bound.
    #[test]
    fn code_cheat_toggle_flips_its_set() {
        let txt = "[No damage]
04000000 00001000 D503201F
{Always}
04000000 00001010 D503201F
[Not built]
04000000 00001020 D503201F
[Data]
04000000 00009000 00000001
";
        let mut c = parse(txt);
        let envs: Vec<std::ffi::CString> = c[..2]
            .iter()
            .map(|x| std::ffi::CString::new(x.env.clone()).unwrap())
            .collect();
        let names: Vec<*const std::ffi::c_char> = envs.iter().map(|e| e.as_ptr()).collect();
        static mut ON: [u8; 2] = [0, 0];
        unsafe {
            nxrt::patchset::nxrt_patch_table(
                std::ptr::addr_of_mut!(ON) as *mut u8,
                names.as_ptr(),
                2,
            )
        };
        bind_code(&mut c, (0x8000, nxrt::patchset::has));
        assert!(c[0].code && c[0].code_only && c[1].code && !c[2].code && !c[3].code);
        assert_eq!(unsafe { ON }, [0, 1], "off by default, the master on");
        toggle(&mut c, "No damage", true).unwrap();
        assert_eq!(nxrt::patchset::get(&c[0].env), Some(true));
        let mut m = fake();
        m.base = 0x1000_0000;
        for a in 0..0x10 {
            m.mem.insert(0x1000_1000 + a, 0xAA);
        }
        Vm::default().pass(&mut c, &mut m, 0);
        assert_eq!(
            m.read(0x1000_1000, 4),
            Some(0xAAAA_AAAA),
            "the VM leaves a compiled-in cheat to its set"
        );
        toggle(&mut c, "No damage", false).unwrap();
        assert_eq!(nxrt::patchset::get(&c[0].env), Some(false));
    }

    /// The panel: rows leave out a pair's restore half; arrows / D-pad move and wrap, A / Return toggle (the toggle
    /// acts as the command does), left / right step a value within its bounds, a held button counts once, B / Escape
    /// close; the text marks the selected row and shows states and values.
    #[test]
    fn cheat_panel() {
        let txt = "[Coins]\n04000000 00000010 00000005\n[HP - on]\n04000000 00000018 000003E7\n[HP - off]\n04000000 00000018 00000064\n{Always}\n04000000 00000020 00000001\n";
        let mut c = parse(txt);
        meta_load(
            &mut c,
            "name\ttype\task\tvalue_words\nCoins\tu32\t0..6\t2\n",
        );
        link_pairs(&mut c);
        let rows = panel_rows(&c);
        assert_eq!(rows, vec![0, 1, 3], "the restore half is not a row");
        let mut p = CheatPanel::open([true, false, false, false, false, false]);
        assert_eq!(
            p.step(
                3,
                [true, false, false, false, false, false],
                false,
                false,
                0,
                0
            ),
            PanelAct::None,
            "A held as it opened"
        );
        assert_eq!(p.step(3, [false; 6], false, false, -1, 0), PanelAct::None);
        assert_eq!(p.cursor, 2, "wraps up");
        p.step(
            3,
            [false, false, false, true, false, false],
            false,
            false,
            0,
            0,
        );
        assert_eq!(p.cursor, 0);
        assert_eq!(
            p.step(3, [false; 6], false, false, 0, 1),
            PanelAct::Step(0, 1)
        );
        assert!(value_step(&mut c[0], 1).is_ok());
        assert_eq!(value_get(&c[0]).as_deref(), Some("6"));
        assert!(value_step(&mut c[0], 1).is_err(), "bounded");
        p.step(3, [false; 6], false, false, 1, 0);
        assert_eq!(
            p.step(3, [false; 6], true, false, 0, 0),
            PanelAct::Toggle(1)
        );
        let name = c[rows[1]].name.clone();
        toggle(&mut c, &name, true).unwrap();
        let t = panel_text(&c, &rows, p.cursor);
        assert!(
            t.contains("\u{25b6} HP - on  [ON]")
                && t.contains("Coins  [off]   < 6 >")
                && t.contains("Always  [always]"),
            "{t}"
        );
        assert_eq!(
            p.step(
                3,
                [false, true, false, false, false, false],
                false,
                false,
                0,
                0
            ),
            PanelAct::Close
        );
        assert_eq!(
            CheatPanel::open([false; 6]).step(0, [false; 6], false, true, 0, 0),
            PanelAct::Close,
            "Escape"
        );
    }
}
