//! nxrt: the runtime that executes translated game code. See README.md for the ABI and the module map.
// The `unsafe extern "C"` functions are entry points generated code calls with its own `Cpu`; their contract is the
// ABI in README.md, not a per-function Safety section.
#![allow(clippy::missing_safety_doc)]

pub mod asm;
pub mod boundary;
pub mod cpu;
pub mod dispatch;
pub mod fault;
pub mod fiber;
pub mod hostfn;
pub mod hostprof;
pub mod interp;
pub mod kernel;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod loader;
#[cfg(target_os = "emscripten")]
pub mod web;
pub mod mem;
pub mod patchset;
pub mod process;
pub mod profile;
pub mod rt;
pub mod sjlj;
pub mod svc;
pub mod waitacct;
