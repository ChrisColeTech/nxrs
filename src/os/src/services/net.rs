//! Services a game opens at start that the host answers offline (Turbo 13a2693, 362b172):
//! - `btm:u` (Bluetooth LE): command 0 hands out IBtmUserCore, whose four Acquire…Event commands (scan, connection,
//!   service discovery, MTU) answer "valid" with an event that never fires; everything else succeeds and finds
//!   nothing (Pokémon Violet 4.0.0 aborted at start without the interface, for the Pokémon GO Plus +);
//! - `bsd:u` / `bsd:s`: no network (Socket answers -1 with ENETDOWN), but event descriptors work (EventFd, 31):
//!   a counter written, read, closed and polled, as newer network libraries make one for their worker's wake-up at
//!   start and abort without it (Ocarina of Time's NPLN); SDK 22's registrations past command 33 succeed.

use crate::ipc::{self, Ctx, Service};
use crate::kobj::Event;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

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

const EVFD0: i32 = 64;
const NEVFD: usize = 32;
const ENETDOWN: u32 = 50;
const EAGAIN: u32 = 11;
const EMFILE: u32 = 24;

static EVFD: Mutex<[(bool, u64); NEVFD]> = Mutex::new([(false, 0); NEVFD]);
static EVFD_CV: Condvar = Condvar::new();

fn evfd(fd: i32) -> Option<usize> {
    let k = fd.checked_sub(EVFD0)? as usize;
    (k < NEVFD && EVFD.lock().unwrap()[k].0).then_some(k)
}

struct Bsd(&'static str);

impl Bsd {
    fn ret(ctx: &mut Ctx, v: i32, errno: u32) -> u32 {
        ctx.push_u32(v as u32);
        ctx.push_u32(errno);
        0
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
            // EventFd(initial value, flags)
            31 => {
                let init = ctx.u64(0);
                let mut g = EVFD.lock().unwrap();
                match g.iter().position(|e| !e.0) {
                    Some(k) => {
                        g[k] = (true, init);
                        drop(g);
                        crate::log_once!("bsd evfd", "bsd: event descriptor {} (no network: only its wake-ups work)", EVFD0 + k as i32);
                        Self::ret(ctx, EVFD0 + k as i32, 0)
                    }
                    None => Self::ret(ctx, -1, EMFILE),
                }
            }
            // Write(fd; buffer)
            24 => match evfd(ctx.u32(0) as i32) {
                Some(k) => {
                    let b = ctx.read_in(0);
                    let v = b.get(..8).map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap()));
                    EVFD.lock().unwrap()[k].1 += v;
                    EVFD_CV.notify_all();
                    Self::ret(ctx, 8, 0)
                }
                None => Self::ret(ctx, -1, ENETDOWN),
            },
            // Read(fd) -> buffer
            25 => match evfd(ctx.u32(0) as i32) {
                Some(k) => {
                    let v = std::mem::take(&mut EVFD.lock().unwrap()[k].1);
                    if v == 0 || ctx.buf_out(0).size < 8 {
                        return Self::ret(ctx, -1, EAGAIN);
                    }
                    ctx.write_out(0, &v.to_le_bytes());
                    Self::ret(ctx, 8, 0)
                }
                None => Self::ret(ctx, -1, ENETDOWN),
            },
            // Close
            26 => {
                if let Some(k) = evfd(ctx.u32(0) as i32) {
                    EVFD.lock().unwrap()[k] = (false, 0);
                    return Self::ret(ctx, 0, 0);
                }
                Self::ret(ctx, -1, ENETDOWN)
            }
            // Fcntl, Ioctl on an event descriptor: accepted
            19 | 20 if evfd(ctx.u32(0) as i32).is_some() => Self::ret(ctx, 0, 0),
            // Poll(nfds, timeout ms; pollfd[] in, out)
            6 => {
                let (n, timeout) = (ctx.u32(0) as usize, ctx.u32(4) as i32);
                let fds = ctx.read_in(0);
                if n > 64 || fds.len() < n * 8 {
                    return Self::ret(ctx, -1, ENETDOWN);
                }
                let t0 = Instant::now();
                let mut out = fds.clone();
                loop {
                    let mut ready = 0;
                    {
                        let g = EVFD.lock().unwrap();
                        for i in 0..n {
                            let fd = i32::from_le_bytes(fds[8 * i..8 * i + 4].try_into().unwrap());
                            let ev = i16::from_le_bytes(fds[8 * i + 4..8 * i + 6].try_into().unwrap());
                            let mut rev = 0i16;
                            if let Some(k) = fd.checked_sub(EVFD0).map(|k| k as usize).filter(|&k| k < NEVFD && g[k].0) {
                                if ev & 1 != 0 && g[k].1 != 0 {
                                    rev |= 1;
                                }
                                if ev & 4 != 0 {
                                    rev |= 4;
                                }
                            }
                            out[8 * i + 6..8 * i + 8].copy_from_slice(&rev.to_le_bytes());
                            if rev != 0 {
                                ready += 1;
                            }
                        }
                    }
                    let left = if timeout < 0 { Duration::from_millis(50) } else { Duration::from_millis(timeout as u64).saturating_sub(t0.elapsed()) };
                    if ready > 0 || timeout == 0 || (timeout > 0 && left.is_zero()) {
                        ctx.write_out(0, &out[..n * 8]);
                        return Self::ret(ctx, ready, 0);
                    }
                    crate::sched::without_core(|| {
                        let g = EVFD.lock().unwrap();
                        let _ = EVFD_CV.wait_timeout(g, left.min(Duration::from_millis(50)));
                    });
                }
            }
            c if c >= 33 => {
                ctx.push_u32(0);
                ctx.push_u32(0);
                0
            }
            _ => Self::ret(ctx, -1, ENETDOWN),
        }
    }
}

// nn::socket's event descriptors by name (SDK 22's network libraries call them directly): the same counters.
unsafe fn sock_evfd(c: *mut nxrt::cpu::Cpu) {
    let init = (*c).x[0];
    let mut g = EVFD.lock().unwrap();
    (*c).x[0] = match g.iter().position(|e| !e.0) {
        Some(k) => {
            g[k] = (true, init);
            crate::log_once!("sock evfd", "socket: event descriptor {} (no network: only its wake-ups work)", EVFD0 + k as i32);
            (EVFD0 + k as i32) as u32 as u64
        }
        None => (-1i32) as u32 as u64,
    };
}
unsafe fn sock_evfd_read(c: *mut nxrt::cpu::Cpu) {
    let (fd, out) = ((*c).x[0] as i32, (*c).x[1]);
    (*c).x[0] = match evfd(fd) {
        Some(k) => {
            let v = std::mem::take(&mut EVFD.lock().unwrap()[k].1);
            crate::guest::out(out, v);
            if v == 0 { (-1i32) as u32 as u64 } else { 0 }
        }
        None => (-1i32) as u32 as u64,
    };
}
unsafe fn sock_evfd_write(c: *mut nxrt::cpu::Cpu) {
    let (fd, v) = ((*c).x[0] as i32, (*c).x[1]);
    (*c).x[0] = match evfd(fd) {
        Some(k) => {
            EVFD.lock().unwrap()[k].1 += v;
            EVFD_CV.notify_all();
            0
        }
        None => (-1i32) as u32 as u64,
    };
}

pub fn register_names(r: &mut crate::hle::Registry) {
    const W: &str = "socket event descriptors: counters on the host";
    r.force("_ZN2nn6socket7EventFdEmNS0_12EventFdFlagsE", W, sock_evfd);
    r.force("_ZN2nn6socket11EventFdReadEiPm", W, sock_evfd_read);
    r.force("_ZN2nn6socket12EventFdWriteEim", W, sock_evfd_write);
}

pub fn register() {
    ipc::register("btm:u", || Box::new(Btm));
    ipc::register("bsd:u", || Box::new(Bsd("bsd:u")));
    ipc::register("bsd:s", || Box::new(Bsd("bsd:s")));
    ipc::register("bsd:a", || Box::new(Bsd("bsd:a")));
}
