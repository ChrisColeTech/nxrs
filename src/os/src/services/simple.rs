//! Takeovers whose whole answer is a constant result, out-values written through argument pointers, and system
//! events bound (never fired) or signalled: the offline stand-ins of services the host does not have (local
//! wireless, friends, telemetry, IR camera, SSL, parental controls, the shop...). The table (`simple_table.rs`)
//! is nxnative's answers for these functions, one row each; a single host function serves every row and finds
//! its row by the name it was bound under.

use crate::hle::Registry;
use nxrt::cpu::Cpu;
use std::collections::HashMap;
use std::sync::OnceLock;

/// An out-value of `u64::MAX` means "zero `size` bytes".
const ZERO_FILL: u64 = u64::MAX;

pub struct S {
    pub name: &'static str,
    pub ret: u64,
    /// (argument index, size in bytes, value)
    pub outs: &'static [(usize, u32, u64)],
    /// arguments that are SystemEventType*: bound to a kernel event that never fires
    pub bind: &'static [usize],
    /// arguments that are SystemEventType*: signalled
    pub signal: &'static [usize],
}

pub static TABLE: &[S] = include!("simple_table.rs");

fn index() -> &'static HashMap<&'static str, &'static S> {
    static I: OnceLock<HashMap<&'static str, &'static S>> = OnceLock::new();
    I.get_or_init(|| TABLE.iter().map(|s| (s.name, s)).collect())
}

unsafe fn answer(c: *mut Cpu) {
    let name = nxrt::hostfn::name_of((*c).pc).unwrap_or_default();
    let Some(s) = index().get(name.as_str()) else {
        nxrt::rt::guest_fatal(c, &format!("simple takeover {name}: no row"));
    };
    let arg = |i: usize| (*c).arg_x(i);
    for &(i, size, v) in s.outs {
        let p = arg(i);
        if p == 0 {
            continue;
        }
        if v == ZERO_FILL {
            crate::guest::zero(p, size as u64);
            continue;
        }
        match size {
            1 => crate::guest::w8(p, v as u8),
            2 => crate::guest::w16(p, v as u16),
            4 => crate::guest::w32(p, v as u32),
            _ => crate::guest::w64(p, v),
        }
    }
    for &i in s.bind {
        crate::services::os::bind_system_event(c, arg(i), false);
    }
    for &i in s.signal {
        crate::services::os::signal_system_event(c, arg(i));
    }
    crate::log_once!(format!("simple {name}"), "offline stand-in: {name} -> {:#x}", s.ret);
    (*c).x[0] = s.ret;
}

pub fn register(r: &mut Registry) {
    for s in TABLE {
        r.force(s.name, "an offline stand-in (nxnative's answer)", answer);
    }
}
