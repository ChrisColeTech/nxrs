//! Function-local general registers: the most used guest registers of a function live in C locals, synced with the
//! Cpu at calls, SVCs, traps, returns and tail exits.

use regex::{Captures, Regex};
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

static REG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"c->x\[(\d+)\]|c->sp\b").unwrap());
// Python: c->(?:x\[(\d+)\]|(sp))\s*=(?!=)  -- the lookahead is checked by hand.
static WRITE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"c->(?:x\[(\d+)\]|(sp))\s*=").unwrap());
/// Lifted instructions that hand the Cpu to the runtime.
pub static ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\brt_(?:svc|brk|udf)\(c\b").unwrap());

pub const SP: u32 = 31;
pub const SPILL: &str = "\x00S";
pub const RELOAD: &str = "\x00R";
pub const RELOAD_CALL: &str = "\x00C";
pub const LOCALS: usize = 6;

pub fn callee_saved() -> BTreeSet<u32> {
    (19..30).chain([SP]).collect()
}

pub fn name(r: u32) -> String {
    if r == SP {
        "sp_".into()
    } else {
        format!("x{r}")
    }
}

pub fn cpu(r: u32) -> String {
    if r == SP {
        "c->sp".into()
    } else {
        format!("c->x[{r}]")
    }
}

fn reg_of(m: &Captures) -> u32 {
    match m.get(1) {
        Some(n) => n.as_str().parse().unwrap(),
        None => SP,
    }
}

/// Registers text reads or writes, once per occurrence.
pub fn refs(text: &str) -> Vec<u32> {
    REG.captures_iter(text).map(|m| reg_of(&m)).collect()
}

pub fn written(text: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    let b = text.as_bytes();
    // A Python regex scans left to right and resumes after each match; a match rejected by the lookahead
    // resumes one character later, which cannot start a new `c->` before its end, so this is equivalent.
    for m in WRITE.captures_iter(text) {
        let end = m.get(0).unwrap().end();
        if b.get(end) == Some(&b'=') {
            continue;
        }
        out.insert(if m.get(2).is_some() {
            SP
        } else {
            m[1].parse().unwrap()
        });
    }
    out
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub regs: Vec<u32>,
    local: BTreeSet<u32>,
    pub spill: Vec<BTreeSet<u32>>,
    reload: String,
    reload_call: String,
}

impl Plan {
    fn new(regs: Vec<u32>, spill: Vec<BTreeSet<u32>>, kept: &BTreeSet<u32>) -> Plan {
        let reload = regs
            .iter()
            .map(|&r| format!(" {} = {};", name(r), cpu(r)))
            .collect();
        let reload_call = regs
            .iter()
            .filter(|r| !kept.contains(r))
            .map(|&r| format!(" {} = {};", name(r), cpu(r)))
            .collect();
        Plan {
            local: regs.iter().copied().collect(),
            regs,
            spill,
            reload,
            reload_call,
        }
    }

    pub fn decl(&self) -> String {
        if self.regs.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = self
            .regs
            .iter()
            .map(|&r| format!("{} = {}", name(r), cpu(r)))
            .collect();
        format!("uint64_t {};", parts.join(", "))
    }

    pub fn resolve(&self, i: usize, text: &str) -> String {
        let text = REG
            .replace_all(text, |m: &Captures| {
                let r = reg_of(m);
                if self.local.contains(&r) {
                    name(r)
                } else {
                    m[0].to_string()
                }
            })
            .into_owned();
        if !text.contains('\x00') {
            return text;
        }
        let spill: String = self.spill[i]
            .iter()
            .map(|&r| format!("{} = {}; ", cpu(r), name(r)))
            .collect();
        text.replace(SPILL, &spill)
            .replace(RELOAD, &self.reload)
            .replace(RELOAD_CALL, &self.reload_call)
    }
}

/// texts[i]: instruction i's C with markers; succ: successors; clean: instructions after which every local equals
/// the Cpu; n_locals: registers kept in locals; kept: registers a call does not reload.
pub fn plan(
    texts: &[String],
    succ: &[Vec<usize>],
    clean: &BTreeSet<usize>,
    n_locals: usize,
    kept: &BTreeSet<u32>,
) -> Plan {
    let n = texts.len();
    let mut uses: HashMap<u32, usize> = HashMap::new();
    for t in texts {
        for r in refs(t) {
            *uses.entry(r).or_default() += 1;
        }
    }
    let mut ranked: Vec<(u32, usize)> = uses.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let regs: BTreeSet<u32> = ranked.iter().take(n_locals).map(|x| x.0).collect();
    let writes: Vec<BTreeSet<u32>> = texts.iter().map(|t| &written(t) & &regs).collect();
    let mut dirty: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); n];
    let mut seen = vec![false; n];
    let mut work = vec![];
    if n > 0 {
        work.push(0);
        seen[0] = true;
    }
    while let Some(i) = work.pop() {
        let out: BTreeSet<u32> = if clean.contains(&i) {
            BTreeSet::new()
        } else {
            &dirty[i] | &writes[i]
        };
        for &s in &succ[i] {
            if !seen[s] || !out.is_subset(&dirty[s]) {
                seen[s] = true;
                dirty[s].extend(out.iter().copied());
                work.push(s);
            }
        }
    }
    let every: BTreeSet<u32> = writes.iter().flatten().copied().collect();
    for i in 0..n {
        if !seen[i] {
            dirty[i] = every.clone();
        }
    }
    let spill = (0..n)
        .map(|i| {
            if texts[i].contains(SPILL) {
                &dirty[i] | &writes[i]
            } else {
                BTreeSet::new()
            }
        })
        .collect();
    Plan::new(regs.into_iter().collect(), spill, kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_skips_compare() {
        assert_eq!(
            written("c->x[3] = 1; if (c->sp == 0) c->sp = 2;"),
            [3, SP].into()
        );
    }

    #[test]
    fn locals_and_spill() {
        let texts = vec![
            "c->x[0] = (uint64_t)(c->x[1] + 0x1ull);".to_string(),
            format!("{SPILL}return;"),
        ];
        let p = plan(
            &texts,
            &[vec![1], vec![]],
            &BTreeSet::new(),
            6,
            &callee_saved(),
        );
        assert_eq!(p.decl(), "uint64_t x0 = c->x[0], x1 = c->x[1];");
        assert_eq!(p.resolve(0, &texts[0]), "x0 = (uint64_t)(x1 + 0x1ull);");
        assert_eq!(p.resolve(1, &texts[1]), "c->x[0] = x0; return;");
    }
}
