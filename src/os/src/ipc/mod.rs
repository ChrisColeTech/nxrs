//! The service and IPC framework: host sessions, the services behind them, domains, and the two ways a request
//! reaches them:
//!
//! * the SVCs (`ConnectToNamedPort("sm:")`, `SendSyncRequest` on the thread's TLS message,
//!   `SendSyncRequestWithUserBuffer`), for SDK clients the host does not take over;
//! * the SDK's own HIPC client entry points, taken over by name (`services::sf`), which hand the message buffer
//!   here directly.
//!
//! A session's object answers commands through `Service::call`; a command it has no handler for answers success
//! with zeroed output and records an `UNIMPL Command` gap once. `sm` (`services` registered with `register`)
//! gives sessions by name; a name nobody registered is refused with sm's ResultNotRegistered (0xE15) and an
//! `UNIMPL Service` gap.

pub mod hipc;

use crate::hle::SESSION_BASE;
use crate::log::Gap;
use hipc::{Buf, Request, Response};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const R_NOT_REGISTERED: u32 = 0xE15;
pub const R_INVALID_HANDLE: u32 = 0xE401;
pub const R_SESSION_CLOSED: u32 = 0xF601;
pub const R_UNKNOWN_COMMAND: u32 = 0x25A0B; // cmif ResultUnknownCommandId (sf 10, 0x11A)

/// What a command handler sees: the request, and the answer it builds.
pub struct Ctx<'a> {
    pub req: &'a Request,
    pub resp: Response,
    /// The interface name, for logs.
    pub iface: String,
    new_objects: Vec<Obj>,
}

impl<'a> Ctx<'a> {
    pub fn cmd(&self) -> u32 {
        self.req.cmd
    }
    /// An argument at byte offset `off` of the input (zero past its end).
    pub fn arg<T: Copy + Default>(&self, off: usize) -> T {
        let n = core::mem::size_of::<T>();
        match self.req.data.get(off..off + n) {
            Some(s) => unsafe { core::ptr::read_unaligned(s.as_ptr() as *const T) },
            None => T::default(),
        }
    }
    pub fn u8(&self, off: usize) -> u8 {
        self.arg(off)
    }
    pub fn u32(&self, off: usize) -> u32 {
        self.arg(off)
    }
    pub fn u64(&self, off: usize) -> u64 {
        self.arg(off)
    }
    pub fn input(&self) -> &[u8] {
        &self.req.data
    }
    pub fn pid(&self) -> u64 {
        self.req.pid.unwrap_or(0)
    }
    pub fn push<T: Copy>(&mut self, v: T) {
        let b = unsafe { core::slice::from_raw_parts(&v as *const T as *const u8, core::mem::size_of::<T>()) };
        self.resp.data.extend_from_slice(b);
    }
    pub fn push_u8(&mut self, v: u8) {
        self.push(v)
    }
    pub fn push_u32(&mut self, v: u32) {
        self.push(v)
    }
    pub fn push_u64(&mut self, v: u64) {
        self.push(v)
    }
    pub fn push_bytes(&mut self, b: &[u8]) {
        self.resp.data.extend_from_slice(b)
    }
    /// Pads the output to `n` bytes.
    pub fn pad_to(&mut self, n: usize) {
        if self.resp.data.len() < n {
            self.resp.data.resize(n, 0)
        }
    }
    pub fn copy_handle(&mut self, h: u32) {
        self.resp.copy.push(h)
    }
    pub fn move_handle(&mut self, h: u32) {
        self.resp.moved.push(h)
    }
    /// Returns a new interface object to the client (an out object: a moved session, or an object id in a domain).
    pub fn object(&mut self, s: impl Service + 'static) {
        self.new_objects.push(Arc::new(Mutex::new(Box::new(s))));
    }
    /// The client's input buffer `i`: its A (send) descriptor, or else its X (pointer) descriptor.
    pub fn buf_in(&self, i: usize) -> Buf {
        let a = self.req.a.get(i).copied().filter(|b| b.addr != 0 || b.size != 0);
        a.or_else(|| self.req.x.get(i).copied()).unwrap_or_default()
    }
    /// The client's output buffer `i`: its B (receive) descriptor, or else its C (receive list) entry.
    pub fn buf_out(&self, i: usize) -> Buf {
        let b = self.req.b.get(i).copied().filter(|b| b.addr != 0 || b.size != 0);
        b.or_else(|| self.req.c.get(i).copied()).unwrap_or_default()
    }
    pub fn read_in(&self, i: usize) -> Vec<u8> {
        let b = self.buf_in(i);
        crate::guest::bytes(b.addr, b.size).to_vec()
    }
    /// Writes into output buffer `i` (truncated to it); returns the bytes written.
    pub fn write_out(&self, i: usize, data: &[u8]) -> usize {
        let b = self.buf_out(i);
        let n = data.len().min(b.size as usize);
        if n > 0 {
            crate::guest::write_bytes(b.addr, &data[..n]);
        }
        n
    }
    /// The command has no handler here: zeroed output, success, one gap line.
    pub fn unimplemented(&mut self) -> u32 {
        crate::log::gap(Gap::Command, &format!("{} #{}", self.iface, self.req.cmd), "answered success with zeroed output");
        0
    }
}

/// An interface object a session (or a domain object id) names.
pub trait Service: Send {
    fn name(&self) -> String;
    /// Answers one command; the result goes to the client (output is dropped on failure).
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        ctx.unimplemented()
    }
    /// The session (or domain object) is closed.
    fn close(&mut self) {}
}

pub type Obj = Arc<Mutex<Box<dyn Service>>>;

struct Session {
    /// Domain object 1 is the session's own object; ids from 2 are objects it returned.
    objects: HashMap<u32, Obj>,
    next: u32,
    domain: bool,
}

struct Sessions {
    slots: Vec<Option<Arc<Mutex<Session>>>>,
}

static SESSIONS: Mutex<Sessions> = Mutex::new(Sessions { slots: Vec::new() });

type Factory = fn() -> Box<dyn Service>;
static REGISTRY: Mutex<Vec<(String, Factory)>> = Mutex::new(Vec::new());

/// Registers a service sm (and the SDK's HIPC resolution) gives sessions of.
pub fn register(name: &str, f: Factory) {
    let mut r = REGISTRY.lock().unwrap();
    r.retain(|(n, _)| n != name);
    r.push((name.to_string(), f));
}

pub fn lookup(name: &str) -> Option<Factory> {
    REGISTRY.lock().unwrap().iter().find(|(n, _)| n == name).map(|(_, f)| *f)
}

fn new_session(obj: Obj, domain: bool) -> u32 {
    let mut s = SESSIONS.lock().unwrap();
    let sess = Arc::new(Mutex::new(Session { objects: HashMap::from([(1, obj)]), next: 2, domain }));
    let i = s.slots.iter().position(|x| x.is_none()).unwrap_or_else(|| {
        s.slots.push(None);
        s.slots.len() - 1
    });
    s.slots[i] = Some(sess);
    SESSION_BASE + i as u32
}

/// A new host session to `svc` (its handle).
pub fn open(svc: Box<dyn Service>) -> u32 {
    new_session(Arc::new(Mutex::new(svc)), false)
}

/// A session to the service registered as `name`, or sm's refusal (with its gap line).
pub fn connect(name: &str) -> Result<u32, u32> {
    match lookup(name) {
        Some(f) => {
            let h = open(f());
            crate::log_once!(format!("connect {name}"), "sm: {name} -> host session {h:#x}");
            Ok(h)
        }
        None => {
            crate::log::gap(Gap::Service, name, "sm refused it (ResultNotRegistered)");
            Err(R_NOT_REGISTERED)
        }
    }
}

fn session(h: u32) -> Option<Arc<Mutex<Session>>> {
    let i = h.wrapping_sub(SESSION_BASE) as usize;
    SESSIONS.lock().unwrap().slots.get(i).and_then(|s| s.clone())
}

pub fn is_session(h: u32) -> bool {
    session(h).is_some()
}

/// Closes a session handle (every object of it).
pub fn close(h: u32) -> bool {
    let i = h.wrapping_sub(SESSION_BASE) as usize;
    let s = {
        let mut g = SESSIONS.lock().unwrap();
        match g.slots.get_mut(i) {
            Some(slot) => slot.take(),
            None => None,
        }
    };
    match s {
        Some(s) => {
            let objs: Vec<Obj> = s.lock().unwrap().objects.drain().map(|(_, o)| o).collect();
            for o in objs {
                if Arc::strong_count(&o) == 1 {
                    o.lock().unwrap().close();
                }
            }
            true
        }
        None => false,
    }
}

/// Makes a session a domain (its object becomes object 1): the control request the SDK's client sends.
pub fn convert_to_domain(h: u32) -> Option<u32> {
    session(h)?.lock().unwrap().domain = true;
    Some(1)
}

/// A new session to the same object as `h` (CloneCurrentObject).
pub fn duplicate(h: u32) -> Option<u32> {
    let s = session(h)?;
    let (o, d) = {
        let g = s.lock().unwrap();
        (g.objects.get(&1).cloned()?, g.domain)
    };
    Some(new_session(o, d))
}

/// Answers the request in `msg` (the client's message buffer) on session `h`. Returns the SVC result (the
/// command's own result is in the answer).
pub fn process(h: u32, msg: &mut [u8]) -> u32 {
    crate::metric!("ipc_requests", 1);
    let Some(sess) = session(h) else {
        crate::log_once!(format!("bad session {h:#x}"), "[RED] ipc: request on {h:#x}, not a host session");
        return R_INVALID_HANDLE;
    };
    let domain = sess.lock().unwrap().domain;
    let req = match hipc::parse(msg, domain) {
        Ok(r) => r,
        Err(e) => {
            crate::log!("[RED] ipc: unreadable request on {h:#x}: {e:?}");
            return R_INVALID_HANDLE;
        }
    };
    if crate::trace::on("ipc") {
        crate::log!("ipc: {h:#x} type {} cmd {} domain {:?} data {} bytes", req.ty, req.cmd, req.domain, req.data.len());
    }
    match req.ty {
        hipc::T_CLOSE => {
            close(h);
            return R_SESSION_CLOSED;
        }
        hipc::T_CONTROL | hipc::T_CONTROL_CTX => {
            let mut resp = Response::default();
            match req.cmd {
                0 => {
                    // ConvertCurrentObjectToDomain: the session's object becomes object 1
                    sess.lock().unwrap().domain = true;
                    resp.data = 1u32.to_le_bytes().to_vec();
                }
                1 => {
                    // CopyFromCurrentDomain(object id) -> a session of that object
                    let id = u32::from_le_bytes(req.data.get(0..4).unwrap_or(&[0; 4]).try_into().unwrap());
                    let obj = sess.lock().unwrap().objects.get(&id).cloned();
                    match obj {
                        Some(o) => resp.moved.push(new_session(o, false)),
                        None => resp.result = R_INVALID_HANDLE,
                    }
                }
                2 | 4 => {
                    // CloneCurrentObject(Ex): a new session to the same object
                    let o = sess.lock().unwrap().objects.get(&1).cloned().unwrap();
                    resp.moved.push(new_session(o, domain));
                }
                3 => resp.data = 0x8000u16.to_le_bytes().to_vec(), // QueryPointerBufferSize
                n => {
                    crate::log::gap(Gap::Command, &format!("control #{n}"), "");
                }
            }
            hipc::write_response(msg, &resp, false, req.version, req.token);
            return 0;
        }
        hipc::T_REQUEST | hipc::T_REQUEST_CTX => {}
        t => {
            crate::log!("[RED] ipc: message type {t} not served");
            return R_INVALID_HANDLE;
        }
    }
    let target = req.domain.as_ref().map_or(1, |d| d.object);
    if let Some(d) = &req.domain {
        if d.kind == hipc::D_CLOSE {
            let o = sess.lock().unwrap().objects.remove(&d.object);
            if let Some(o) = o {
                if Arc::strong_count(&o) == 1 {
                    o.lock().unwrap().close();
                }
            }
            hipc::write_response(msg, &Response::default(), true, 0, 0);
            return 0;
        }
    }
    let obj = sess.lock().unwrap().objects.get(&target).cloned();
    let Some(obj) = obj else {
        crate::log!("[RED] ipc: domain object {target} of {h:#x} does not exist");
        hipc::write_response(msg, &Response { result: R_INVALID_HANDLE, ..Default::default() }, domain, req.version, req.token);
        return 0;
    };
    let mut ctx = Ctx { req: &req, resp: Response::default(), iface: String::new(), new_objects: vec![] };
    let result = {
        let mut o = obj.lock().unwrap();
        ctx.iface = o.name();
        o.call(&mut ctx)
    };
    ctx.resp.result = result;
    if crate::trace::on("ipc") || (result != 0 && crate::log::first(&format!("fail {} {} {result:#x}", ctx.iface, req.cmd))) {
        crate::log!("ipc: {} #{} -> {result:#x}", ctx.iface, req.cmd);
    }
    let news = std::mem::take(&mut ctx.new_objects);
    if result == 0 {
        for o in news {
            if domain {
                let mut s = sess.lock().unwrap();
                let id = s.next;
                s.next += 1;
                s.objects.insert(id, o);
                ctx.resp.objects.push(id);
            } else {
                let nh = new_session(o, false);
                ctx.resp.moved.push(nh);
            }
        }
    }
    hipc::write_response(msg, &ctx.resp, domain, req.version, req.token);
    0
}

/// `sm:`, the service manager.
pub struct Sm;

impl Service for Sm {
    fn name(&self) -> String {
        "sm:".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            0 | 4 => 0, // RegisterClient, DetachClient
            1 => {
                let raw: u64 = ctx.arg(0);
                let name: String = raw.to_le_bytes().iter().take_while(|&&b| b != 0).map(|&b| b as char).collect();
                match connect(&name) {
                    Ok(h) => {
                        ctx.move_handle(h);
                        0
                    }
                    Err(r) => r,
                }
            }
            _ => ctx.unimplemented(),
        }
    }
}

/// A service whose every command succeeds with zeroed output (an offline stand-in that keeps no state).
pub struct Silent(pub &'static str);

impl Service for Silent {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        crate::log_once!(format!("silent {} {}", self.0, ctx.cmd()), "ipc: {} #{}: success with zeroed output (offline stand-in)", self.0, ctx.cmd());
        0
    }
}

#[cfg(test)]
mod tests {
    use super::hipc::{Builder, T_CONTROL, T_REQUEST};
    use super::*;

    struct Counter(u32);
    impl Service for Counter {
        fn name(&self) -> String {
            "test:c".into()
        }
        fn call(&mut self, ctx: &mut Ctx) -> u32 {
            match ctx.cmd() {
                0 => {
                    self.0 += ctx.u32(0);
                    ctx.push_u32(self.0);
                    0
                }
                1 => {
                    ctx.object(Counter(100));
                    0
                }
                2 => {
                    ctx.copy_handle(0x77);
                    0
                }
                3 => 0x1234,
                _ => ctx.unimplemented(),
            }
        }
    }

    fn send(h: u32, b: Builder, domain: bool) -> (u32, Response) {
        let mut m = b.build();
        let r = process(h, &mut m);
        (r, hipc::parse_response(&m, domain).unwrap())
    }

    #[test]
    fn sm_and_dispatch() {
        register("test:c", || Box::new(Counter(0)));
        let sm = open(Box::new(Sm));
        let mut name = [0u8; 8];
        name[..6].copy_from_slice(b"test:c");
        let (r, resp) = send(sm, Builder { ty: T_REQUEST, cmd: 1, data: name.to_vec(), ..Default::default() }, false);
        assert_eq!((r, resp.result), (0, 0));
        let h = resp.moved[0];
        assert!(is_session(h));
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 0, data: 5u32.to_le_bytes().to_vec(), ..Default::default() }, false);
        assert_eq!(&resp.data[..4], &5u32.to_le_bytes());
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 0, data: 2u32.to_le_bytes().to_vec(), ..Default::default() }, false);
        assert_eq!(&resp.data[..4], &7u32.to_le_bytes(), "the session keeps its object's state");
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 2, ..Default::default() }, false);
        assert_eq!(resp.copy, vec![0x77]);
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 3, ..Default::default() }, false);
        assert_eq!(resp.result, 0x1234);
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 99, ..Default::default() }, false);
        assert_eq!(resp.result, 0, "an unknown command answers success");
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 1, ..Default::default() }, false);
        let sub = resp.moved[0];
        let (_, resp) = send(sub, Builder { ty: T_REQUEST, cmd: 0, data: 1u32.to_le_bytes().to_vec(), ..Default::default() }, false);
        assert_eq!(&resp.data[..4], &101u32.to_le_bytes(), "an out object is a session of its own");
        name[..6].copy_from_slice(b"nope:x");
        let (_, resp) = send(sm, Builder { ty: T_REQUEST, cmd: 1, data: name.to_vec(), ..Default::default() }, false);
        assert_eq!(resp.result, R_NOT_REGISTERED);
        assert!(close(sub) && !is_session(sub));
        assert_eq!(process(sub, &mut [0u8; 0x100]), R_INVALID_HANDLE);
    }

    #[test]
    fn domains() {
        let h = open(Box::new(Counter(0)));
        let (_, resp) = send(h, Builder { ty: T_CONTROL, cmd: 0, ..Default::default() }, false);
        assert_eq!(&resp.data[..4], &1u32.to_le_bytes());
        let (_, resp) = send(h, Builder { ty: T_CONTROL, cmd: 3, ..Default::default() }, false);
        assert_eq!(&resp.data[..2], &0x8000u16.to_le_bytes());
        // in the domain: object 1 opens a sub-object, which gets id 2
        let mut m = Builder { ty: T_REQUEST, cmd: 1, domain_object: Some(1), ..Default::default() }.build();
        assert_eq!(process(h, &mut m), 0);
        let resp = hipc::parse_response(&m, true).unwrap();
        assert_eq!(resp.result, 0);
        assert!(resp.moved.is_empty(), "no session moved in a domain");
        let id = u32::from_le_bytes(resp.data[..4].try_into().unwrap());
        assert_eq!(id, 2);
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 0, data: 3u32.to_le_bytes().to_vec(), domain_object: Some(2), ..Default::default() }, true);
        assert_eq!(&resp.data[..4], &103u32.to_le_bytes());
        let (_, resp) = send(h, Builder { ty: T_REQUEST, cmd: 0, data: 3u32.to_le_bytes().to_vec(), domain_object: Some(9), ..Default::default() }, true);
        assert_eq!(resp.result, R_INVALID_HANDLE);
    }
}
