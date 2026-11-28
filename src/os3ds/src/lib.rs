//! os3ds: the 3DS OS services. The crate shapes the boot the way the plan's §9 names it
//! (`nxos3ds_main`, `boot`, `kernel`, `hle`, `services/*`): named services through `srv:`, the IPC
//! session table and gap ledger, the kernel svcs this side owns, and the scripted `hid` feed.
//!
//! The boot is driven by the guest's own call pattern: the title connects, asks `srv:` for the
//! services it wants, and each service answers its commands. No rendering, audio or window; the
//! refused payloads are named in the gap ledger (`Host::gaps`).

pub mod boot;
pub mod hle;
pub mod kernel;
pub mod services;

pub use hle::{parse_input, Host, InputScript, InputStep, PadState};
pub use kernel::install;

use runtime3ds::Process3ds;

/// the 3DS `nxos_main` shape: install the services, then run the process headless. A linked 3DS
/// build (3DS-W6) calls this with the generated module table; the W3 harness calls it directly.
pub fn nxos3ds_main(
    host: std::sync::Arc<Host>,
    process: &Process3ds,
    program_id: u64,
    entry: u32,
    stack_top: u32,
    stack_size: u32,
) -> Result<runtime3ds::BootReport, String> {
    boot::run(host, process, program_id, entry, stack_top, stack_size)
}
