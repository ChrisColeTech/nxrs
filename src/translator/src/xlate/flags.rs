//! Function-local NZCV: a generated function keeps its flags in a local `Flags F` and writes c->nzcv only where it
//! leaves for another function other than by a call or a return.

use regex::{Captures, Regex};
use std::collections::BTreeSet;
use std::sync::LazyLock;

static KIND_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(subs|adds|logic)(32|64)\(c, ").unwrap());
static CARRY_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(adcs|sbcs)(32|64)\(c, ").unwrap());
static CARRY_USE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(adc|sbc)(32|64)\(c, ").unwrap());
static COND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"COND_([A-Z]{2})\(c\)").unwrap());
static VOID_DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(void\)(subs|adds)(32|64)\(c, ").unwrap());
const NZ_DEF: &str = "c->nzcv = ";
const NZ_USE: &str = "(c->nzcv)";

fn kind_name(op: &str) -> &'static str {
    match op {
        "subs" => "sub",
        "adds" => "add",
        _ => "logic",
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct FlagError(pub String);

/// (reads flags, kind of the flags written: "nz", "sub64", ... or None) of one instruction's C.
pub fn effect(code: &str) -> Result<(bool, Option<String>), FlagError> {
    let reads = COND.is_match(code)
        || CARRY_USE.is_match(code)
        || CARRY_DEF.is_match(code)
        || code.contains(NZ_USE);
    if code.contains(NZ_DEF) || CARRY_DEF.is_match(code) {
        return Ok((reads, Some("nz".into())));
    }
    let defs: Vec<Captures> = KIND_DEF.captures_iter(code).collect();
    if defs.len() > 1 {
        return Err(FlagError(format!(
            "several flag definitions in one instruction: {code}"
        )));
    }
    if let Some(d) = defs.first() {
        return Ok((reads, Some(format!("{}{}", kind_name(&d[1]), &d[2]))));
    }
    Ok((reads, None))
}

/// Per instruction: the kind its flag reads use, whether its definition also writes F.nz, and what it publishes.
#[derive(Debug, Default, Clone)]
pub struct Plan {
    pub reader: Vec<Option<String>>,
    pub pack: Vec<bool>,
    pub exit: Vec<Option<String>>,
    pub entry: bool,
    pub entry_read: bool,
    pub after_call: usize,
    pub used: bool,
}

impl Plan {
    fn new(n: usize) -> Plan {
        Plan {
            reader: vec![None; n],
            pack: vec![false; n],
            exit: vec![None; n],
            ..Default::default()
        }
    }

    pub fn decl(&self) -> &'static str {
        if self.entry {
            "Flags F = {.nz = c->nzcv};"
        } else {
            "Flags F = {0};"
        }
    }

    pub fn publish(&self, i: usize) -> String {
        match &self.exit[i] {
            Some(k) => format!("c->nzcv = fnz_{k}(&F); "),
            None => String::new(),
        }
    }

    pub fn rewrite(&self, i: usize, code: &str) -> Result<String, FlagError> {
        let mut code = code.to_string();
        let (_, dk) = effect(&code)?;
        if let Some(rk) = &self.reader[i] {
            code = COND
                .replace_all(&code, |m: &Captures| format!("fc_{rk}(&F, CC_{})", &m[1]))
                .into_owned();
            code = CARRY_DEF
                .replace_all(&code, |m: &Captures| {
                    format!("{}{}f(&F, fc_{rk}(&F, CC_CS), ", &m[1], &m[2])
                })
                .into_owned();
            code = CARRY_USE
                .replace_all(&code, |m: &Captures| {
                    format!("{}{}f(fc_{rk}(&F, CC_CS), ", &m[1], &m[2])
                })
                .into_owned();
            code = code.replace(NZ_USE, &format!("(fnz_{rk}(&F))"));
        }
        match dk.as_deref() {
            Some("nz") => {
                code = code.replace(NZ_DEF, "F.nz = ");
                code = VOID_DEF
                    .replace_all(&code, |m: &Captures| {
                        format!("F.nz = nz_{}{}(", kind_name(&m[1]), &m[2])
                    })
                    .into_owned();
            }
            Some(dk) => {
                code = KIND_DEF
                    .replace_all(&code, |m: &Captures| {
                        format!("f{}{}(&F, ", kind_name(&m[1]), &m[2])
                    })
                    .into_owned();
                if self.pack[i] {
                    code.push_str(&format!(" F.nz = fnz_{dk}(&F);"));
                }
            }
            None => {}
        }
        if code.contains("c->nzcv")
            || COND.is_match(&code)
            || KIND_DEF.is_match(&code)
            || CARRY_USE.is_match(&code)
        {
            return Err(FlagError(format!(
                "flag access left after the rewrite: {code}"
            )));
        }
        Ok(code)
    }
}

/// A reaching definition: the function entry, a call, or an instruction index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Def {
    Entry,
    Call,
    At(usize),
}

/// effects[i]: (reads, kind); succ[i]: successors inside the function; calls: call indices; exits: tail exits.
pub fn plan(
    effects: &[(bool, Option<String>)],
    succ: &[Vec<usize>],
    calls: &BTreeSet<usize>,
    exits: &BTreeSet<usize>,
) -> Plan {
    let n = effects.len();
    let mut p = Plan::new(n);
    if !effects.iter().any(|(r, k)| *r || k.is_some()) {
        return p;
    }
    p.used = true;
    let mut reach: Vec<BTreeSet<Def>> = vec![BTreeSet::new(); n];
    reach[0].insert(Def::Entry);
    let mut work = vec![0usize];
    while let Some(i) = work.pop() {
        let out: BTreeSet<Def> = if effects[i].1.is_some() {
            [Def::At(i)].into()
        } else if calls.contains(&i) {
            [Def::Call].into()
        } else {
            reach[i].clone()
        };
        for &s in &succ[i] {
            if !out.is_subset(&reach[s]) {
                reach[s].extend(out.iter().copied());
                work.push(s);
            }
        }
    }
    let kind = |d: &Def| -> String {
        match d {
            Def::Entry => "nz".into(),
            Def::At(i) => effects[*i].1.clone().unwrap(),
            Def::Call => unreachable!(),
        }
    };
    let use_ = |p: &mut Plan, r: &BTreeSet<Def>| -> String {
        let kinds: BTreeSet<String> = r.iter().map(kind).collect();
        if r.contains(&Def::Entry) {
            p.entry = true;
        }
        if kinds.len() == 1 {
            return kinds.into_iter().next().unwrap();
        }
        for d in r {
            if let Def::At(i) = d {
                p.pack[*i] = true;
            }
        }
        "nz".into()
    };
    for i in 0..n {
        if effects[i].0 {
            p.after_call += reach[i].contains(&Def::Call) as usize;
            let mut r = reach[i].clone();
            r.remove(&Def::Call);
            let k = use_(&mut p, &r);
            p.reader[i] = Some(k);
            p.entry_read |= reach[i].contains(&Def::Entry);
        }
    }
    for &i in exits {
        let mut r = reach[i].clone();
        r.remove(&Def::Call);
        if !r.is_empty() && !(r.len() == 1 && r.contains(&Def::Entry)) {
            let k = use_(&mut p, &r);
            p.exit[i] = Some(k);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects() {
        assert_eq!(
            effect("(void)(subs64(c, c->x[1], 0x3ull));").unwrap(),
            (false, Some("sub64".into()))
        );
        assert_eq!(effect("if (COND_EQ(c)) x").unwrap(), (true, None));
        assert_eq!(
            effect("c->nzcv = 0x0ull;").unwrap(),
            (false, Some("nz".into()))
        );
    }

    #[test]
    fn cmp_then_branch_uses_typed_compare() {
        let eff = vec![(false, Some("sub64".to_string())), (true, None)];
        let succ = vec![vec![1], vec![]];
        let p = plan(&eff, &succ, &BTreeSet::new(), &BTreeSet::new());
        assert!(p.used && !p.entry);
        assert_eq!(p.rewrite(1, "COND_LT(c)").unwrap(), "fc_sub64(&F, CC_LT)");
        assert_eq!(
            p.rewrite(0, "(void)(subs64(c, a, b));").unwrap(),
            "(void)(fsub64(&F, a, b));"
        );
    }
}
