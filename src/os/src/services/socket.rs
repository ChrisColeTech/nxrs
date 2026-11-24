//! nn::socket's descriptors, offline: remote networking is unavailable (ENETDOWN), but the transports that stay
//! inside the process work, as offline engines use them for their own threads: event descriptors (a counter
//! written, read and polled: a network library's worker wake-up) and loopback UDP (127.0.0.0/8 or INADDR_ANY:
//! RakNet sends a start-up datagram to itself). No host socket is opened. The descriptors belong to this table
//! (from `FD0`, small enough for a game's fd_set), shared by the by-name takeovers here and the `bsd:u` IPC
//! service (`net.rs`). Constants are the Horizon / FreeBSD ABI: sockaddr_in {u8 length, u8 family, u16 port, u32
//! address, padding}, port and address big-endian; pollfd {i32 fd, i16 events, i16 revents}; fd_set 1024 bits;
//! timeval {i64 seconds, i64 microseconds}. A UDP queue keeps packet boundaries and holds at most 256 packets and
//! the receive buffer size; a full queue drops the datagram (the sender still succeeds, as UDP has no
//! acknowledgement). Endless waits come back every 100 ms (the caller loops), so a guest thread blocked here can
//! still be seen from outside.

use crate::guest::{r32, w32, Args};
use crate::hle::Registry;
use nxrt::cpu::Cpu;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The first descriptor; the table holds `MAX`.
pub const FD0: i32 = 64;
const MAX: usize = 64;

pub const ENETDOWN: i32 = 50;
const EBADF: i32 = 9;
const EAGAIN: i32 = 35;
const EINVAL: i32 = 22;
const EMSGSIZE: i32 = 40;
const EOPNOTSUPP: i32 = 45;
const EADDRINUSE: i32 = 48;
const ENOTCONN: i32 = 57;
const EDESTADDRREQ: i32 = 39;
const EPIPE: i32 = 32;
const EMFILE: i32 = 24;
const ENOPROTOOPT: i32 = 42;

const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const POLLRDNORM: i16 = 0x40;
const POLLNVAL: i16 = 0x20;
const NONBLOCK: i32 = 0x800;
const MSG_PEEK: i32 = 2;
const MSG_DONTWAIT: i32 = 0x80;
const UDP_MAX: usize = 65507;
const QUEUE_MAX: i32 = 1024 * 1024;
const PACKETS_MAX: usize = 256;
const DEFAULT_BUF: i32 = 256 * 1024;
const SOL_SOCKET: i32 = 0xffff;
const LOOPBACK: u32 = 0x7f00_0001;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Kind {
    #[default]
    Free,
    Event,
    Udp,
}

struct Packet {
    addr: u32,
    port: u16,
    data: Vec<u8>,
}

#[derive(Default)]
struct Desc {
    kind: Kind,
    generation: u64,
    /// event descriptor: the counter; its flags (1 semaphore, 2 / 4 non-blocking)
    count: u64,
    /// event descriptor: EventFdFlags; UDP: the file status flags (NONBLOCK)
    flags: i32,
    fdflags: i32,
    addr: u32,
    port: u16,
    peer_addr: u32,
    peer_port: u16,
    connected: bool,
    shut_read: bool,
    shut_write: bool,
    reuse: i32,
    broadcast: bool,
    rcvbuf: i32,
    sndbuf: i32,
    recv_timeout: i64,
    send_timeout: i64,
    queue: VecDeque<Packet>,
    queued: usize,
}

struct Table {
    d: Vec<Desc>,
    generation: u64,
    ephemeral: u16,
}

static T: Mutex<Option<Table>> = Mutex::new(None);
static CV: Condvar = Condvar::new();

thread_local! {
    static ERR: Cell<i32> = const { Cell::new(ENETDOWN) };
}

fn table() -> MutexGuard<'static, Option<Table>> {
    let mut g = T.lock().unwrap();
    if g.is_none() {
        *g = Some(Table {
            d: (0..MAX).map(|_| Desc::default()).collect(),
            generation: 0,
            ephemeral: 49152,
        });
    }
    g
}

/// The calling thread's last socket error (errno).
pub fn errno() -> i32 {
    ERR.with(|e| e.get())
}
pub fn set_errno(e: i32) {
    ERR.with(|x| x.set(e));
}
fn fail(e: i32) -> i32 {
    set_errno(e);
    -1
}

fn slot(fd: i32) -> Option<usize> {
    let k = fd.checked_sub(FD0)? as usize;
    (k < MAX).then_some(k)
}

impl Table {
    fn get(&mut self, fd: i32) -> Option<&mut Desc> {
        let k = slot(fd)?;
        let d = &mut self.d[k];
        (d.kind != Kind::Free).then_some(d)
    }
    fn kind(&self, fd: i32) -> Kind {
        slot(fd).map_or(Kind::Free, |k| self.d[k].kind)
    }
    fn generation(&self, fd: i32) -> u64 {
        slot(fd).map_or(0, |k| self.d[k].generation)
    }
    fn new_desc(&mut self, kind: Kind) -> i32 {
        let Some(k) = self.d.iter().position(|d| d.kind == Kind::Free) else {
            return fail(EMFILE);
        };
        self.generation += 1;
        self.d[k] = Desc {
            kind,
            generation: self.generation,
            rcvbuf: DEFAULT_BUF,
            sndbuf: DEFAULT_BUF,
            ..Default::default()
        };
        FD0 + k as i32
    }
    fn port_free(&self, fd: i32, addr: u32, port: u16) -> bool {
        let me = slot(fd).map_or(0, |k| self.d[k].reuse);
        self.d.iter().enumerate().all(|(i, d)| {
            if FD0 + i as i32 == fd
                || d.kind != Kind::Udp
                || d.port != port
                || !(addr == 0 || d.addr == 0 || addr == d.addr)
            {
                return true;
            }
            let both = d.reuse & me;
            both & 0x200 != 0 || (both & 4 != 0 && addr != d.addr)
        })
    }
    fn bind(&mut self, fd: i32, addr: u32, mut port: u16) -> i32 {
        if self.get(fd).is_some_and(|d| d.port != 0) {
            return fail(EINVAL);
        }
        if port == 0 {
            for _ in 0..16384 {
                let c = self.ephemeral;
                self.ephemeral = if c == 65535 { 49152 } else { c + 1 };
                if self.port_free(fd, addr, c) {
                    port = c;
                    break;
                }
            }
            if port == 0 {
                return fail(EADDRINUSE);
            }
        } else if !self.port_free(fd, addr, port) {
            return fail(EADDRINUSE);
        }
        let d = self.get(fd).unwrap();
        d.addr = addr;
        d.port = port;
        0
    }
}

/// Waits on the table's condition with the guest core released.
fn wait<'a>(g: MutexGuard<'a, Option<Table>>, d: Duration) -> MutexGuard<'a, Option<Table>> {
    crate::sched::without_core(|| CV.wait_timeout(g, d).unwrap().0)
}

// ---- addresses

/// (address, port) of a sockaddr_in; only loopback or INADDR_ANY (anything else is the network: ENETDOWN).
fn addr_read(b: &[u8]) -> Result<(u32, u16), i32> {
    if b.len() < 16 || b[1] != 2 || (b[0] != 0 && b[0] != 16) {
        return Err(fail(EINVAL));
    }
    let port = u16::from_be_bytes([b[2], b[3]]);
    let addr = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    if addr != 0 && addr >> 24 != 127 {
        return Err(fail(ENETDOWN));
    }
    Ok((addr, port))
}
fn addr_bytes(addr: u32, port: u16) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0] = 16;
    b[1] = 2;
    b[2..4].copy_from_slice(&port.to_be_bytes());
    b[4..8].copy_from_slice(&addr.to_be_bytes());
    b
}

// ---- the operations (host side, testable without a guest)

pub fn eventfd(init: u64, flags: i32) -> i32 {
    let mut g = table();
    let t = g.as_mut().unwrap();
    let fd = t.new_desc(Kind::Event);
    if fd >= 0 {
        let d = t.get(fd).unwrap();
        d.count = init;
        d.flags = flags;
        crate::log_once!(
            "socket evfd",
            "socket: event descriptor {fd} (offline: the process's own wake-ups work)"
        );
    }
    fd
}

pub fn socket(family: i32, ty: i32, proto: i32) -> i32 {
    if family != 2 || ty != 2 || (proto != 0 && proto != 17) {
        crate::log_once!(format!("socket {family} {ty} {proto}"), "socket: Socket({family}, {ty}, {proto}): no network (only loopback UDP and event descriptors), ENETDOWN");
        return fail(ENETDOWN);
    }
    let mut g = table();
    let fd = g.as_mut().unwrap().new_desc(Kind::Udp);
    if fd >= 0 {
        crate::log_once!(
            "socket udp",
            "socket: loopback UDP descriptor {fd} (offline: datagrams stay in the process)"
        );
    }
    fd
}

pub fn close(fd: i32) -> i32 {
    let mut g = table();
    let t = g.as_mut().unwrap();
    let Some(d) = t.get(fd) else {
        return fail(ENETDOWN);
    };
    *d = Desc {
        generation: d.generation,
        ..Default::default()
    };
    CV.notify_all();
    0
}

pub fn is_ours(fd: i32) -> bool {
    table().as_ref().unwrap().kind(fd) != Kind::Free
}

/// Reads an event descriptor's counter: blocks while it is 0 unless non-blocking.
pub fn efd_read(fd: i32) -> Result<u64, i32> {
    let mut g = table();
    let gen = g.as_ref().unwrap().generation(fd);
    loop {
        let t = g.as_mut().unwrap();
        if t.kind(fd) != Kind::Event || t.generation(fd) != gen {
            return Err(fail(EBADF));
        }
        let d = t.get(fd).unwrap();
        if d.count != 0 {
            return Ok(if d.flags & 1 != 0 {
                d.count -= 1;
                1
            } else {
                std::mem::take(&mut d.count)
            });
        }
        if d.flags & 6 != 0 {
            return Err(fail(EAGAIN));
        }
        g = wait(g, Duration::from_millis(100));
    }
}

pub fn efd_write(fd: i32, v: u64) -> i32 {
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Event {
        return fail(EBADF);
    }
    let d = t.get(fd).unwrap();
    d.count = d.count.wrapping_add(v);
    CV.notify_all();
    0
}

pub fn bind(fd: i32, a: &[u8]) -> i32 {
    let (addr, port) = match addr_read(a) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Udp {
        return fail(EBADF);
    }
    t.bind(fd, addr, port)
}

/// Connect (a family of 0 disconnects); binds an ephemeral port first when the socket has none.
pub fn connect(fd: i32, a: &[u8]) -> i32 {
    let disconnect = a.len() >= 2 && a[1] == 0;
    let (addr, port) = if disconnect {
        (0, 0)
    } else {
        match addr_read(a) {
            Ok(v) => v,
            Err(e) => return e,
        }
    };
    if !disconnect && port == 0 {
        return fail(EINVAL);
    }
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Udp {
        return fail(EBADF);
    }
    if t.get(fd).unwrap().port == 0 && t.bind(fd, 0, 0) != 0 {
        return -1;
    }
    let d = t.get(fd).unwrap();
    d.connected = !disconnect;
    d.peer_addr = if addr != 0 { addr } else { LOOPBACK };
    d.peer_port = port;
    0
}

/// The socket's own (or its peer's) address.
pub fn name(fd: i32, peer: bool) -> Result<[u8; 16], i32> {
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Udp {
        return Err(fail(EBADF));
    }
    let d = t.get(fd).unwrap();
    if peer && !d.connected {
        return Err(fail(ENOTCONN));
    }
    Ok(if peer {
        addr_bytes(d.peer_addr, d.peer_port)
    } else {
        addr_bytes(d.addr, d.port)
    })
}

pub fn shutdown(fd: i32, how: i32) -> i32 {
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Udp {
        return fail(EBADF);
    }
    let d = t.get(fd).unwrap();
    if !(0..=2).contains(&how) {
        return fail(EINVAL);
    }
    if !d.connected {
        return fail(ENOTCONN);
    }
    if how != 1 {
        d.shut_read = true;
    }
    if how != 0 {
        d.shut_write = true;
    }
    CV.notify_all();
    0
}

/// F_GETFD 1, F_SETFD 2, F_GETFL 3, F_SETFL 4.
pub fn fcntl(fd: i32, cmd: i32, value: i32) -> i32 {
    let mut g = table();
    let t = g.as_mut().unwrap();
    let Some(d) = t.get(fd) else {
        return fail(EBADF);
    };
    let event = d.kind == Kind::Event;
    match cmd {
        1 => d.fdflags,
        2 => {
            d.fdflags = value & 1;
            0
        }
        3 if event => {
            if d.flags & 6 != 0 {
                NONBLOCK
            } else {
                0
            }
        }
        3 => d.flags,
        4 if event => {
            d.flags = (d.flags & !6) | if value & NONBLOCK != 0 { 2 } else { 0 };
            0
        }
        4 => {
            d.flags = value & NONBLOCK;
            0
        }
        _ => fail(EINVAL),
    }
}

pub fn setopt(fd: i32, level: i32, opt: i32, v: &[u8]) -> i32 {
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Udp {
        return fail(EBADF);
    }
    let d = t.get(fd).unwrap();
    if level != SOL_SOCKET {
        return fail(ENOPROTOOPT);
    }
    if (opt == 0x1006 || opt == 0x1005) && v.len() >= 16 {
        let (s, us) = (
            i64::from_le_bytes(v[0..8].try_into().unwrap()),
            i64::from_le_bytes(v[8..16].try_into().unwrap()),
        );
        if s < 0 || s > i64::MAX / 1_000_000_000 - 1 || !(0..1_000_000).contains(&us) {
            return fail(EINVAL);
        }
        let ns = s * 1_000_000_000 + us * 1000;
        if opt == 0x1006 {
            d.recv_timeout = ns;
        } else {
            d.send_timeout = ns;
        }
        return 0;
    }
    if v.len() < 4 {
        return fail(ENOPROTOOPT);
    }
    let x = i32::from_le_bytes(v[0..4].try_into().unwrap());
    match opt {
        4 | 0x200 => {
            d.reuse &= !opt;
            if x != 0 {
                d.reuse |= opt;
            }
        }
        0x20 => d.broadcast = x != 0,
        0x1001 | 0x1002 => {
            if x <= 0 {
                return fail(EINVAL);
            }
            let x = x.min(QUEUE_MAX);
            if opt == 0x1001 {
                d.sndbuf = x;
            } else {
                d.rcvbuf = x;
            }
        }
        _ => return fail(ENOPROTOOPT),
    }
    0
}

/// The option's value (at most `size` bytes wanted).
pub fn getopt(fd: i32, level: i32, opt: i32, size: u32) -> Result<Vec<u8>, i32> {
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Udp {
        return Err(fail(EBADF));
    }
    let d = t.get(fd).unwrap();
    if level != SOL_SOCKET {
        return Err(fail(ENOPROTOOPT));
    }
    if (opt == 0x1006 || opt == 0x1005) && size >= 16 {
        let ns = if opt == 0x1006 {
            d.recv_timeout
        } else {
            d.send_timeout
        };
        let mut v = (ns / 1_000_000_000).to_le_bytes().to_vec();
        v.extend_from_slice(&(ns % 1_000_000_000 / 1000).to_le_bytes());
        return Ok(v);
    }
    if size < 4 {
        return Err(fail(EINVAL));
    }
    let x: i32 = match opt {
        0x1008 => 2,
        0x1007 => 0,
        0x1016 => 17,
        4 | 0x200 => (d.reuse & opt != 0) as i32,
        0x20 => d.broadcast as i32,
        0x1001 => d.sndbuf,
        0x1002 => d.rcvbuf,
        _ => return Err(fail(ENOPROTOOPT)),
    };
    Ok(x.to_le_bytes().to_vec())
}

/// Sends a datagram to `to` (or the connected peer): queued on the loopback socket bound to that port.
pub fn sendto(fd: i32, data: &[u8], flags: i32, to: Option<&[u8]>) -> i64 {
    if data.len() > UDP_MAX {
        return fail(EMSGSIZE) as i64;
    }
    if flags & !(MSG_DONTWAIT | 4 | 0x20000) != 0 {
        return fail(EOPNOTSUPP) as i64;
    }
    let (mut addr, mut port) = (0, 0);
    if let Some(a) = to {
        match addr_read(a) {
            Ok(v) => (addr, port) = v,
            Err(e) => return e as i64,
        }
    }
    let mut g = table();
    let t = g.as_mut().unwrap();
    if t.kind(fd) != Kind::Udp {
        return fail(EBADF) as i64;
    }
    let d = t.get(fd).unwrap();
    if d.shut_write {
        return fail(EPIPE) as i64;
    }
    if to.is_none() {
        if !d.connected {
            return fail(EDESTADDRREQ) as i64;
        }
        (addr, port) = (d.peer_addr, d.peer_port);
    }
    if port == 0 {
        return fail(EINVAL) as i64;
    }
    if addr == 0 {
        addr = LOOPBACK;
    }
    if t.get(fd).unwrap().port == 0 && t.bind(fd, 0, 0) != 0 {
        return -1;
    }
    let me = t.get(fd).unwrap();
    let (src, sport) = (if me.addr != 0 { me.addr } else { LOOPBACK }, me.port);
    let mut target = None;
    let mut best = -1;
    for (i, r) in t.d.iter().enumerate() {
        if r.kind != Kind::Udp || r.shut_read || r.port != port || !(r.addr == 0 || r.addr == addr)
        {
            continue;
        }
        if r.connected && (r.peer_addr != src || r.peer_port != sport) {
            continue;
        }
        let score = (r.addr != 0) as i32 + 2 * r.connected as i32;
        if score > best {
            best = score;
            target = Some(i);
        }
    }
    if let Some(i) = target {
        let r = &mut t.d[i];
        let cap = r.rcvbuf.max(0) as usize;
        if r.queue.len() < PACKETS_MAX && data.len() <= cap && r.queued <= cap - data.len() {
            r.queue.push_back(Packet {
                addr: src,
                port: sport,
                data: data.to_vec(),
            });
            r.queued += data.len();
            CV.notify_all();
        }
    }
    data.len() as i64
}

/// Receives the oldest datagram (at most `n` bytes of it) and its sender; an empty one after a read shutdown.
pub fn recvfrom(fd: i32, n: usize, flags: i32) -> Result<(Vec<u8>, [u8; 16]), i32> {
    if flags & !(MSG_DONTWAIT | MSG_PEEK) != 0 {
        return Err(fail(EOPNOTSUPP));
    }
    let mut g = table();
    if g.as_ref().unwrap().kind(fd) != Kind::Udp {
        return Err(fail(EBADF));
    }
    let gen = g.as_ref().unwrap().generation(fd);
    let start = Instant::now();
    loop {
        let t = g.as_mut().unwrap();
        if t.kind(fd) != Kind::Udp || t.generation(fd) != gen {
            return Err(fail(EBADF));
        }
        let d = t.get(fd).unwrap();
        if let Some(p) = d.queue.front() {
            let out = (
                p.data[..n.min(p.data.len())].to_vec(),
                addr_bytes(p.addr, p.port),
            );
            if flags & MSG_PEEK == 0 {
                let p = d.queue.pop_front().unwrap();
                d.queued -= p.data.len();
            }
            return Ok(out);
        }
        if d.shut_read {
            return Ok((vec![], [0; 16]));
        }
        if flags & MSG_DONTWAIT != 0 || d.flags & NONBLOCK != 0 {
            return Err(fail(EAGAIN));
        }
        let left = if d.recv_timeout != 0 {
            let l = d.recv_timeout - start.elapsed().as_nanos() as i64;
            if l <= 0 {
                return Err(fail(EAGAIN));
            }
            Duration::from_nanos(l as u64)
        } else {
            Duration::from_millis(100)
        };
        g = wait(g, left);
    }
}

/// Polls `fds` ({fd, events, revents}); `ms` < 0 waits at most 100 ms (the caller loops). Descriptors this table
/// never made are never ready; one closed during the wait reports POLLNVAL.
pub fn poll(fds: &mut [(i32, i16, i16)], ms: i32) -> i32 {
    let mut g = table();
    let gens: Vec<Option<u64>> = fds
        .iter()
        .map(|&(fd, _, _)| {
            let t = g.as_ref().unwrap();
            (t.kind(fd) != Kind::Free).then(|| t.generation(fd))
        })
        .collect();
    for f in fds.iter_mut() {
        f.2 = 0;
    }
    let limit = Duration::from_millis(if ms < 0 { 100 } else { ms as u64 });
    if gens.iter().all(|x| x.is_none()) {
        drop(g);
        crate::sched::without_core(|| std::thread::sleep(limit));
        return 0;
    }
    let start = Instant::now();
    loop {
        let t = g.as_mut().unwrap();
        let mut ready = 0;
        for (f, gen) in fds.iter_mut().zip(&gens) {
            f.2 = 0;
            let Some(gen) = *gen else { continue };
            if t.kind(f.0) == Kind::Free || t.generation(f.0) != gen {
                f.2 = POLLNVAL;
                ready += 1;
                continue;
            }
            let d = t.get(f.0).unwrap();
            let event = d.kind == Kind::Event;
            let readable = if event {
                d.count != 0
            } else {
                !d.queue.is_empty() || d.shut_read
            };
            if readable {
                f.2 |= f.1 & (POLLIN | POLLRDNORM);
            }
            if event || !d.shut_write {
                f.2 |= f.1 & POLLOUT;
            }
            if f.2 != 0 {
                ready += 1;
            }
        }
        if ready > 0 {
            return ready;
        }
        let left = limit.saturating_sub(start.elapsed());
        if left.is_zero() {
            return 0;
        }
        g = wait(g, left);
    }
}

/// select over 1024-bit fd_sets (16 words each); `timeout_ms` < 0 waits as poll does.
pub fn select(nfds: i32, sets: [Option<&mut [u64; 16]>; 3], timeout_ms: i64) -> i32 {
    if nfds < 0 {
        return fail(EINVAL);
    }
    let nfds = nfds.min(1024);
    let ask = [POLLIN, POLLOUT, 0];
    let mut p: Vec<(i32, i16, i16)> = vec![];
    for fd in FD0..(FD0 + MAX as i32).min(nfds) {
        let mut ev = 0;
        let mut any = false;
        for (k, s) in sets.iter().enumerate() {
            if let Some(s) = s {
                if s[(fd >> 6) as usize] >> (fd & 63) & 1 != 0 {
                    any = true;
                    ev |= ask[k];
                }
            }
        }
        if any {
            p.push((fd, ev, 0));
        }
    }
    poll(&mut p, timeout_ms.clamp(-1, i32::MAX as i64) as i32);
    let [mut r, mut w, mut e] = sets;
    for s in [&mut r, &mut w, &mut e].into_iter().flatten() {
        for fd in 0..nfds {
            s[(fd >> 6) as usize] &= !(1u64 << (fd & 63));
        }
    }
    let mut n = 0;
    for &(fd, _, rev) in &p {
        if rev & POLLNVAL != 0 {
            return fail(EBADF);
        }
        if rev & (POLLIN | POLLRDNORM) != 0 {
            if let Some(r) = r.as_deref_mut() {
                r[(fd >> 6) as usize] |= 1 << (fd & 63);
                n += 1;
            }
        }
        if rev & POLLOUT != 0 {
            if let Some(w) = w.as_deref_mut() {
                w[(fd >> 6) as usize] |= 1 << (fd & 63);
                n += 1;
            }
        }
    }
    n
}

// ---- the guest entry points

unsafe fn ret(c: *mut Cpu, v: i64) {
    (*c).x[0] = v as u64;
}

/// A guest sockaddr of `n` bytes (empty when null).
fn sa(a: u64, n: u32) -> Vec<u8> {
    if a == 0 {
        return vec![];
    }
    crate::guest::bytes(a, n.min(128) as u64).to_vec()
}
/// Writes an address to (sockaddr *a, socklen_t *n), truncated to *n; *n becomes 16.
fn sa_out(a: u64, n: u64, b: &[u8; 16]) -> i32 {
    if a == 0 {
        return 0;
    }
    if n == 0 {
        return fail(EINVAL);
    }
    let cap = r32(n).min(16) as usize;
    crate::guest::write_bytes(a, &b[..cap]);
    w32(n, 16);
    0
}

unsafe fn g_socket(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = socket(a.i(0), a.i(1), a.i(2));
    ret(c, r as i64);
}
unsafe fn g_close(c: *mut Cpu) {
    let r = close((*c).x[0] as i32);
    ret(c, r as i64);
}
unsafe fn g_bind(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = bind(a.i(0), &sa(a.x(1), a.w(2)));
    ret(c, r as i64);
}
unsafe fn g_connect(c: *mut Cpu) {
    let a = Args(&mut *c);
    let b = sa(a.x(1), a.w(2));
    let r = if b.is_empty() {
        fail(EINVAL)
    } else {
        connect(a.i(0), &b)
    };
    ret(c, r as i64);
}
unsafe fn g_name(c: *mut Cpu, peer: bool) {
    let a = Args(&mut *c);
    let (fd, out, n) = (a.i(0), a.x(1), a.x(2));
    let r = if out == 0 || n == 0 {
        fail(EINVAL)
    } else {
        match name(fd, peer) {
            Ok(b) => sa_out(out, n, &b),
            Err(e) => e,
        }
    };
    ret(c, r as i64);
}
unsafe fn g_sockname(c: *mut Cpu) {
    g_name(c, false)
}
unsafe fn g_peername(c: *mut Cpu) {
    g_name(c, true)
}
unsafe fn g_shutdown(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = shutdown(a.i(0), a.i(1));
    ret(c, r as i64);
}
/// Fcntl(fd, cmd, ...): the variadic value is in x2 (Switch code passes variadic arguments in registers).
unsafe fn g_fcntl(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = fcntl(a.i(0), a.i(1), a.i(2));
    ret(c, r as i64);
}
unsafe fn g_setopt(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (fd, level, opt, v, n) = (a.i(0), a.i(1), a.i(2), a.x(3), a.w(4));
    let r = if v == 0 {
        fail(EINVAL)
    } else {
        setopt(fd, level, opt, crate::guest::bytes(v, n.min(64) as u64))
    };
    ret(c, r as i64);
}
unsafe fn g_getopt(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (fd, level, opt, v, n) = (a.i(0), a.i(1), a.i(2), a.x(3), a.x(4));
    let r = if v == 0 || n == 0 {
        fail(EINVAL)
    } else {
        match getopt(fd, level, opt, r32(n)) {
            Ok(b) => {
                crate::guest::write_bytes(v, &b);
                w32(n, b.len() as u32);
                0
            }
            Err(e) => e,
        }
    };
    ret(c, r as i64);
}
fn g_send_impl(fd: i32, buf: u64, n: u64, flags: i32, to: Option<Vec<u8>>) -> i64 {
    if buf == 0 && n != 0 {
        return fail(EINVAL) as i64;
    }
    if n as usize > UDP_MAX {
        return fail(EMSGSIZE) as i64;
    }
    let data = if n == 0 {
        &[][..]
    } else {
        crate::guest::bytes(buf, n)
    };
    sendto(fd, data, flags, to.as_deref())
}
unsafe fn g_send(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = g_send_impl(a.i(0), a.x(1), a.x(2), a.i(3), None);
    ret(c, r);
}
unsafe fn g_sendto(c: *mut Cpu) {
    let a = Args(&mut *c);
    let to = (a.x(4) != 0).then(|| sa(a.x(4), a.w(5)));
    let r = g_send_impl(a.i(0), a.x(1), a.x(2), a.i(3), to);
    ret(c, r);
}
fn g_recv_impl(fd: i32, buf: u64, n: u64, flags: i32, from: u64, from_n: u64) -> i64 {
    if (buf == 0 && n != 0) || (from != 0 && from_n == 0) {
        return fail(EINVAL) as i64;
    }
    match recvfrom(fd, n as usize, flags) {
        Ok((data, src)) => {
            if !data.is_empty() {
                crate::guest::write_bytes(buf, &data);
            }
            if src[1] != 0 {
                sa_out(from, from_n, &src);
            }
            data.len() as i64
        }
        Err(e) => e as i64,
    }
}
unsafe fn g_recv(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = g_recv_impl(a.i(0), a.x(1), a.x(2), a.i(3), 0, 0);
    ret(c, r);
}
unsafe fn g_recvfrom(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = g_recv_impl(a.i(0), a.x(1), a.x(2), a.i(3), a.x(4), a.x(5));
    ret(c, r);
}
/// Read / Write: an event descriptor's counter (8 bytes), else a datagram.
unsafe fn g_read(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (fd, buf, n) = (a.i(0), a.x(1), a.x(2));
    let event = table().as_ref().unwrap().kind(fd) == Kind::Event;
    let r = if !event {
        g_recv_impl(fd, buf, n, 0, 0, 0)
    } else if buf == 0 || n < 8 {
        fail(EINVAL) as i64
    } else {
        match efd_read(fd) {
            Ok(v) => {
                crate::guest::w64(buf, v);
                8
            }
            Err(e) => e as i64,
        }
    };
    ret(c, r);
}
unsafe fn g_write(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (fd, buf, n) = (a.i(0), a.x(1), a.x(2));
    let event = table().as_ref().unwrap().kind(fd) == Kind::Event;
    let r = if !event {
        g_send_impl(fd, buf, n, 0, None)
    } else if buf == 0 || n < 8 {
        fail(EINVAL) as i64
    } else if efd_write(fd, crate::guest::r64(buf)) != 0 {
        -1
    } else {
        8
    };
    ret(c, r);
}
unsafe fn g_poll(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (p, n, ms) = (a.x(0), a.x(1), a.i(2));
    if p == 0 && n != 0 || n > 4096 {
        ret(c, fail(EINVAL) as i64);
        return;
    }
    let mut v: Vec<(i32, i16, i16)> = (0..n)
        .map(|i| {
            (
                r32(p + 8 * i) as i32,
                crate::guest::r16(p + 8 * i + 4) as i16,
                0,
            )
        })
        .collect();
    let r = poll(&mut v, ms);
    for (i, f) in v.iter().enumerate() {
        crate::guest::w16(p + 8 * i as u64 + 6, f.2 as u16);
    }
    ret(c, r as i64);
}
unsafe fn g_select(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (nfds, tv) = (a.i(0), a.x(4));
    let ms = if tv == 0 {
        -1
    } else {
        let (s, us) = (
            crate::guest::r64(tv) as i64,
            crate::guest::r64(tv + 8) as i64,
        );
        if s < 0 || !(0..1_000_000).contains(&us) {
            ret(c, fail(EINVAL) as i64);
            return;
        }
        s.saturating_mul(1000)
            .saturating_add(us / 1000)
            .min(i32::MAX as i64)
    };
    let read = |p: u64| -> Option<[u64; 16]> {
        (p != 0).then(|| core::array::from_fn(|i| crate::guest::r64(p + 8 * i as u64)))
    };
    let ptrs = [a.x(1), a.x(2), a.x(3)];
    let mut sets = ptrs.map(read);
    let [s0, s1, s2] = &mut sets;
    let r = select(nfds, [s0.as_mut(), s1.as_mut(), s2.as_mut()], ms);
    for (p, s) in ptrs.iter().zip(&sets) {
        if let Some(s) = s {
            for (i, w) in s.iter().enumerate() {
                crate::guest::w64(p + 8 * i as u64, *w);
            }
        }
    }
    ret(c, r as i64);
}
unsafe fn g_eventfd(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = eventfd(a.x(0), a.i(1));
    ret(c, r as i64);
}
unsafe fn g_efd_read(c: *mut Cpu) {
    let a = Args(&mut *c);
    let (fd, out) = (a.i(0), a.x(1));
    let r = match efd_read(fd) {
        Ok(v) => {
            crate::guest::out(out, v);
            0
        }
        Err(e) => e,
    };
    ret(c, r as i64);
}
unsafe fn g_efd_write(c: *mut Cpu) {
    let a = Args(&mut *c);
    let r = efd_write(a.i(0), a.x(1));
    ret(c, r as i64);
}
unsafe fn g_errno(c: *mut Cpu) {
    ret(c, errno() as i64);
}
unsafe fn g_set_errno(c: *mut Cpu) {
    set_errno((*c).x[0] as i32);
}
/// Remote name lookups and the resolver: the network is down.
unsafe fn g_netdown_null(c: *mut Cpu) {
    set_errno(ENETDOWN);
    ret(c, 0);
}
unsafe fn g_netdown(c: *mut Cpu) {
    ret(c, fail(ENETDOWN) as i64);
}
/// Listen / Accept: no stream sockets.
unsafe fn g_no_stream(c: *mut Cpu) {
    ret(c, fail(ENETDOWN) as i64);
}
unsafe fn g_gai_str(c: *mut Cpu) {
    static S: OnceLock<u64> = OnceLock::new();
    let p = *S.get_or_init(|| {
        let p = nxrt::kernel::get().space.kernel_alloc(32, 16);
        crate::guest::put_cstr(p, 32, "network unavailable");
        p
    });
    (*c).x[0] = p;
}

pub fn register(r: &mut Registry) {
    const W: &str = "the BSD descriptors offline: loopback UDP and event descriptors";
    for (n, f) in [
        ("_ZN2nn6socket6SocketEiii", g_socket as unsafe fn(*mut Cpu)),
        (
            "_ZN2nn6socket6SocketENS0_6FamilyENS0_4TypeENS0_8ProtocolE",
            g_socket,
        ),
        ("nnsocketSocket", g_socket),
        ("_ZN2nn6socket5CloseEi", g_close),
        ("nnsocketClose", g_close),
        ("_ZN2nn6socket4BindEiPK8sockaddrj", g_bind),
        ("_ZN2nn6socket4BindEiPKNS0_8SockAddrEj", g_bind),
        ("nnsocketBind", g_bind),
        ("_ZN2nn6socket7ConnectEiPK8sockaddrj", g_connect),
        ("_ZN2nn6socket7ConnectEiPKNS0_8SockAddrEj", g_connect),
        ("nnsocketConnect", g_connect),
        ("_ZN2nn6socket11GetSockNameEiP8sockaddrPj", g_sockname),
        ("_ZN2nn6socket11GetSockNameEiPNS0_8SockAddrEPj", g_sockname),
        ("nnsocketGetSockName", g_sockname),
        ("_ZN2nn6socket11GetPeerNameEiP8sockaddrPj", g_peername),
        ("_ZN2nn6socket11GetPeerNameEiPNS0_8SockAddrEPj", g_peername),
        ("nnsocketGetPeerName", g_peername),
        ("_ZN2nn6socket8ShutdownEii", g_shutdown),
        ("_ZN2nn6socket8ShutdownEiNS0_14ShutdownMethodE", g_shutdown),
        ("nnsocketShutdown", g_shutdown),
        ("_ZN2nn6socket5FcntlEiiz", g_fcntl),
        ("_ZN2nn6socket5FcntlEiNS0_12FcntlCommandEz", g_fcntl),
        ("nnsocketFcntl", g_fcntl),
        ("_ZN2nn6socket10SetSockOptEiiiPKvj", g_setopt),
        (
            "_ZN2nn6socket10SetSockOptEiNS0_5LevelENS0_6OptionEPKvj",
            g_setopt,
        ),
        ("nnsocketSetSockOpt", g_setopt),
        ("_ZN2nn6socket10GetSockOptEiiiPvPj", g_getopt),
        (
            "_ZN2nn6socket10GetSockOptEiNS0_5LevelENS0_6OptionEPvPj",
            g_getopt,
        ),
        ("nnsocketGetSockOpt", g_getopt),
        ("_ZN2nn6socket4SendEiPKvmi", g_send),
        ("_ZN2nn6socket4SendEiPKvmNS0_7MsgFlagE", g_send),
        ("nnsocketSend", g_send),
        ("_ZN2nn6socket6SendToEiPKvmiPK8sockaddrj", g_sendto),
        (
            "_ZN2nn6socket6SendToEiPKvmNS0_7MsgFlagEPKNS0_8SockAddrEj",
            g_sendto,
        ),
        ("nnsocketSendTo", g_sendto),
        ("_ZN2nn6socket4RecvEiPvmi", g_recv),
        ("_ZN2nn6socket4RecvEiPvmNS0_7MsgFlagE", g_recv),
        ("nnsocketRecv", g_recv),
        ("_ZN2nn6socket8RecvFromEiPvmiP8sockaddrPj", g_recvfrom),
        (
            "_ZN2nn6socket8RecvFromEiPvmNS0_7MsgFlagEPNS0_8SockAddrEPj",
            g_recvfrom,
        ),
        ("nnsocketRecvFrom", g_recvfrom),
        ("_ZN2nn6socket4ReadEiPvm", g_read),
        ("nnsocketRead", g_read),
        ("_ZN2nn6socket5WriteEiPKvm", g_write),
        ("nnsocketWrite", g_write),
        ("_ZN2nn6socket4PollEP6pollfdmi", g_poll),
        ("_ZN2nn6socket4PollEPNS0_6PollFdEmi", g_poll),
        ("nnsocketPoll", g_poll),
        ("poll", g_poll),
        ("_ZN2nn6socket6SelectEiP6fd_setS2_S2_P7timeval", g_select),
        (
            "_ZN2nn6socket6SelectEiPNS0_5FdSetES2_S2_PNS0_7TimeValE",
            g_select,
        ),
        ("_ZN2nn6socket7EventFdEmNS0_12EventFdFlagsE", g_eventfd),
        ("_ZN2nn6socket11EventFdReadEiPm", g_efd_read),
        ("_ZN2nn6socket12EventFdWriteEim", g_efd_write),
        ("_ZN2nn6socket12GetLastErrnoEv", g_errno),
        ("_ZN2nn6socket12GetLastErrorEv", g_errno),
        ("nnsocketGetLastErrno", g_errno),
        ("nnsocketSetLastErrno", g_set_errno),
        ("_ZN2nn6socket16GetHostEntByNameEPKc", g_netdown_null),
        (
            "_ZN2nn6socket16GetHostEntByNameEPKcPKNS0_14ResolverOptionEm",
            g_netdown_null,
        ),
        ("nnsocketGetHostByNameWithOptions", g_netdown_null),
        (
            "_ZN2nn6socket17ResolverGetOptionEPNS0_14ResolverOptionENS0_17ResolverOptionKeyE",
            g_netdown,
        ),
        (
            "_ZN2nn6socket17ResolverSetOptionERKNS0_14ResolverOptionE",
            g_netdown,
        ),
        ("nnsocketResolverGetOption", g_netdown),
        ("nnsocketResolverSetOption", g_netdown),
        ("_ZN2nn6socket6ListenEii", g_no_stream),
        ("_ZN2nn6socket6AcceptEiPNS0_8SockAddrEPj", g_no_stream),
        ("_ZN2nn6socket11GAIStrErrorENS0_7AiErrnoE", g_gai_str),
        ("nnsocketGAIStrError", g_gai_str),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sin(port: u16) -> [u8; 16] {
        addr_bytes(LOOPBACK, port)
    }

    #[test]
    fn loopback_udp_and_events() {
        let a = socket(2, 2, 0);
        let b = socket(2, 2, 17);
        assert!(a >= FD0 && b >= FD0);
        assert_eq!(bind(a, &sin(40001)), 0);
        assert_eq!(bind(b, &sin(40001)), -1, "the port is taken");
        assert_eq!(errno(), EADDRINUSE);
        assert_eq!(sendto(b, b"hello", 0, Some(&sin(40001))), 5);
        let me = name(b, false).unwrap();
        let (data, from) = recvfrom(a, 64, 0).unwrap();
        assert_eq!(data, b"hello");
        assert_eq!(from[2..4], me[2..4], "the sender is b's ephemeral port");
        assert_eq!(from[4..8], LOOPBACK.to_be_bytes(), "from loopback");
        assert_eq!(fcntl(a, 4, NONBLOCK), 0);
        assert!(recvfrom(a, 64, 0).is_err());
        assert_eq!(errno(), EAGAIN);
        assert_eq!(socket(2, 1, 0), -1, "no streams: the network is down");
        assert_eq!(errno(), ENETDOWN);
        assert_eq!(
            sendto(a, b"x", 0, Some(&addr_bytes(0x0a00_0001, 80))),
            -1,
            "not loopback"
        );

        let e = eventfd(0, 4);
        let mut fds = [(e, POLLIN, 0), (a, POLLIN | POLLOUT, 0)];
        assert_eq!(poll(&mut fds, 0), 1, "only the socket is writable");
        assert_eq!(efd_write(e, 3), 0);
        assert_eq!(poll(&mut fds, 0), 2);
        assert_eq!(efd_read(e), Ok(3));
        assert!(efd_read(e).is_err(), "non-blocking, empty");
        let mut set = [0u64; 16];
        set[(e >> 6) as usize] |= 1 << (e & 63);
        assert_eq!(efd_write(e, 1), 0);
        assert_eq!(select(FD0 + MAX as i32, [Some(&mut set), None, None], 0), 1);
        for fd in [a, b, e] {
            assert_eq!(close(fd), 0);
        }
        assert_eq!(close(a), -1);
    }
}
