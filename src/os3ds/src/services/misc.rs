//! misc: the small services a title may only open (mic, cam, boss, news, ndm, ir). Every command is
//! answered and gap-logged so the ledger names what was refused; none of the owner's titles needs
//! their payload yet (plan §4's "defer/refuse" rows).

use crate::hle::{Host, Reply, Request, R_OK};
use runtime3ds::Process3ds;

pub fn command(
    host: &Host,
    _process: &Process3ds,
    _tls: u32,
    name: &'static str,
    request: &Request,
) -> Reply {
    host.gap(format!(
        "{name}: command {:#010x} refused (this service is deferred)",
        request.cmd
    ));
    let _ = R_OK;
    Reply::ok()
}
