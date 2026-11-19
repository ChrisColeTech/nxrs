//! ARM64 -> C lifter: instruction decoding and lowering (`lifter`, `simd`), function-local flags and registers
//! (`flags`, `regs`) and jump-table resolution (`jumptables`).
//!
//! The output text is kept byte-identical to mkw-recomp's generator, so the two can be compared.

pub mod util;
pub mod lifter;
pub mod simd;
pub mod flags;
pub mod regs;
pub mod jumptables;

pub use lifter::{Insn, Kind, Lifter};
pub use util::Unsupported;
