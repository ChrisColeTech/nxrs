//! runtime3ds: the guest CPU host for 3DS titles. The generated C (translator3ds) is written
//! against recomp.h v8; this crate is a host implementing that interface: the run loop over the
//! Entry tables, page-table memory, one host thread per guest thread, the kernel svc table, an
//! ARM11/Thumb-1/VFP interpreter for what the recompiler left, and the hot-set/miss recorder.
//! docs/waves/3DS-W2.md holds the pinned svc/clock/memory facts; the donor's docs/interface.md is
//! the ABI's prose.

pub mod context;
pub mod dispatch;
pub mod interp;
pub mod mem;
