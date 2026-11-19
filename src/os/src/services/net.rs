//! Services a game opens at start that the host answers offline (Turbo 13a2693, 362b172):
//! - `btm:u` (Bluetooth LE): command 0 hands out IBtmUserCore, whose four Acquire…Event commands (scan, connection,
//!   service discovery, MTU) answer "valid" with an event that never fires; everything else succeeds and finds
//!   nothing (Pokémon Violet 4.0.0 aborted at start without the interface, for the Pokémon GO Plus +);
//! - `bt` (IBluetoothUser): command 9 RegisterBleEvent hands out an event that never fires, the rest succeed (Violet
//!   4.0.0 opens it after btm:u and aborted when sm refused it; Turbo answers it with its generic stub);
//! - `bsd:u` / `bsd:s`: no network (a socket other than loopback UDP answers -1 with ENETDOWN), but event descriptors
//!   and loopback UDP work (`socket.rs`, the table the by-name takeovers share; EventFd is 31):
//!   a counter written, read, closed and polled, as newer network libraries make one for their worker's wake-up at
//!   start and abort without it (Ocarina of Time's NPLN); SDK 22's registrations past command 33 succeed.

use crate::ipc::{self, Ctx, Service};
use crate::kobj::Event;
use crate::services::socket;

struct Btm;
struct BtmCore {
    events: Vec<Event>,
}

impl Service for Btm {
    fn name(&self) -> String {
        "btm:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        if ctx.cmd() == 0 {
            crate::log_once!("btm", "btm:u: Bluetooth LE interface handed out (no adapter: scans find nothing)");
            ctx.object(BtmCore { events: vec![] });
        }
        0
    }
}

impl Service for BtmCore {
    fn name(&self) -> String {
        "IBtmUserCore".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        if matches!(ctx.cmd(), 0 | 17 | 26 | 33) {
            let e = Event::new();
            ctx.push_u8(1);
            ctx.copy_handle(e.readable);
            self.events.push(e);
        }
        0
    }
}

struct Bt {
    events: Vec<Event>,
}

impl Service for Bt {
    fn name(&self) -> String {
        "bt".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        crate::log_once!("bt", "bt: Bluetooth user service opened (no adapter: its BLE event never fires)");
        if ctx.cmd() == 9 {
            let e = Event::new();
            ctx.copy_handle(e.readable);
            self.events.push(e);
        }
        0
    }
}

/// `bsd:u` / `bsd:s` / `bsd:a`: the IPC face of the offline descriptor table (`socket.rs`), the one the by-name
/// takeovers use, so a descriptor made either way works both ways.
struct Bsd(&'static str);

impl Bsd {
    fn ret(ctx: &mut Ctx, v: i64) -> u32 {
        ctx.push_u32(v as i32 as u32);
        ctx.push_u32(if v < 0 { socket::errno() as u32 } else { 0 });
        0
    }
    fn ret_err(ctx: &mut Ctx, errno: i32) -> u32 {
        socket::set_errno(errno);
        Self::ret(ctx, -1)
    }
}

impl Service for Bsd {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // RegisterClient (pid, transfer memory) / StartMonitoring
            0 => {
                ctx.push_u64(0);
                0
            }
            1 => 0,
            // Socket(domain, type, protocol)
            2 | 3 => {
                let r = socket::socket(ctx.u32(0) as i32, ctx.u32(4) as i32, ctx.u32(8) as i32);
                Self::ret(ctx, r as i64)
            }
            // EventFd(initial value, flags)
            31 => {
                let r = socket::eventfd(ctx.u64(0), ctx.u32(8) as i32);
                Self::ret(ctx, r as i64)
            }
            // Write(fd; buffer): an event descriptor's counter, else a datagram to the connected peer
            24 => {
                let fd = ctx.u32(0) as i32;
                let b = ctx.read_in(0);
                if !socket::is_ours(fd) {
                    return Self::ret_err(ctx, socket::ENETDOWN);
                }
                let r = match socket::efd_write(fd, b.get(..8).map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap()))) {
                    0 if b.len() >= 8 => 8,
                    _ if socket::errno() == 9 => socket::sendto(fd, &b, 0, None),
                    _ => -1,
                };
                Self::ret(ctx, r)
            }
            // Read(fd) -> buffer
            25 => {
                let fd = ctx.u32(0) as i32;
                let cap = ctx.buf_out(0).size as usize;
                if !socket::is_ours(fd) {
                    return Self::ret_err(ctx, socket::ENETDOWN);
                }
                match socket::efd_read(fd) {
                    Ok(v) if cap >= 8 => {
                        ctx.write_out(0, &v.to_le_bytes());
                        Self::ret(ctx, 8)
                    }
                    Ok(_) => Self::ret_err(ctx, 22),
                    Err(_) if socket::errno() == 9 => match socket::recvfrom(fd, cap, 0) {
                        Ok((d, _)) => {
                            ctx.write_out(0, &d);
                            Self::ret(ctx, d.len() as i64)
                        }
                        Err(_) => Self::ret(ctx, -1),
                    },
                    Err(_) => Self::ret(ctx, -1),
                }
            }
            // Close
            26 => {
                let r = socket::close(ctx.u32(0) as i32);
                Self::ret(ctx, r as i64)
            }
            // Fcntl(fd, command, value)
            20 => {
                let r = socket::fcntl(ctx.u32(0) as i32, ctx.u32(4) as i32, ctx.u32(8) as i32);
                Self::ret(ctx, r as i64)
            }
            // Ioctl on a descriptor of the table: accepted
            19 if socket::is_ours(ctx.u32(0) as i32) => Self::ret(ctx, 0),
            // Poll(nfds, timeout ms; pollfd[] in, out)
            6 => {
                let (n, timeout) = (ctx.u32(0) as usize, ctx.u32(4) as i32);
                let fds = ctx.read_in(0);
                if n > 64 || fds.len() < n * 8 {
                    return Self::ret_err(ctx, 22);
                }
                let mut v: Vec<(i32, i16, i16)> = (0..n)
                    .map(|i| (i32::from_le_bytes(fds[8 * i..8 * i + 4].try_into().unwrap()), i16::from_le_bytes(fds[8 * i + 4..8 * i + 6].try_into().unwrap()), 0))
                    .collect();
                let r = socket::poll(&mut v, timeout);
                let mut out = fds[..n * 8].to_vec();
                for (i, f) in v.iter().enumerate() {
                    out[8 * i + 6..8 * i + 8].copy_from_slice(&f.2.to_le_bytes());
                }
                ctx.write_out(0, &out);
                Self::ret(ctx, r as i64)
            }
            c if c >= 33 => {
                ctx.push_u32(0);
                ctx.push_u32(0);
                0
            }
            _ => Self::ret_err(ctx, socket::ENETDOWN),
        }
    }
}

pub fn register() {
    ipc::register("btm:u", || Box::new(Btm));
    ipc::register("bt", || Box::new(Bt { events: vec![] }));
    ipc::register("bsd:u", || Box::new(Bsd("bsd:u")));
    ipc::register("bsd:s", || Box::new(Bsd("bsd:s")));
    ipc::register("bsd:a", || Box::new(Bsd("bsd:a")));
}
