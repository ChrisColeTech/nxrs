//! the 3DS boot order: srv -> apt -> gfx init -> process start. The services are demand-driven
//! (the guest connects through `srv:`), so the order here is the hook installation and the first
//! log line; the actual sequence is the title's own call pattern, recorded in the log.

use crate::hle::Host;
use runtime3ds::{BootReport, Process3ds};
use std::sync::Arc;

pub fn run(
    host: Arc<Host>,
    process: &Process3ds,
    program_id: u64,
    entry: u32,
    stack_top: u32,
    stack_size: u32,
) -> Result<BootReport, String> {
    crate::kernel::install(Arc::clone(&host));
    host.attach_process(process);
    host.start_input();
    host.say(format!(
        "boot: srv -> apt -> gfx init -> process start (entry {entry:#x}, stack {stack_top:#x}, {stack_size:#x})"
    ));
    runtime3ds::start(process, program_id, entry, stack_top, stack_size)
}
