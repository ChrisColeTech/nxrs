//! Kernel objects the os layer makes for the guest, on the runtime kernel: events (a service's system events, the
//! read end copied to the client), signalled and cleared by the host. They are ordinary kernel events, so the
//! guest waits on them with WaitSynchronization like any other.

use nxrt::cpu::Cpu;

fn ksvc(imm: u32, args: &[u64]) -> [u64; 3] {
    let mut c = Cpu::new_boxed();
    for (i, &a) in args.iter().enumerate() {
        c.x[i] = a;
    }
    if !nxrt::kernel::svc(&mut c, imm) {
        nxrt::rt::fatal(&format!("os: kernel svc {imm:#x} is not handled"));
    }
    [c.x[0], c.x[1], c.x[2]]
}

/// An event made by the host: it keeps the write end and hands out the read end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub writable: u32,
    pub readable: u32,
}

impl Event {
    pub fn new() -> Event {
        let r = ksvc(0x45, &[]);
        assert_eq!(r[0], 0, "CreateEvent");
        Event { writable: r[1] as u32, readable: r[2] as u32 }
    }
    /// A new event, already signalled (a state the client reads as "there is something").
    pub fn signalled() -> Event {
        let e = Event::new();
        e.signal();
        e
    }
    pub fn signal(&self) {
        ksvc(0x11, &[self.writable as u64]);
    }
    pub fn clear(&self) {
        ksvc(0x12, &[self.writable as u64]);
    }
}

impl Default for Event {
    fn default() -> Self {
        Event::new()
    }
}

/// A system event a service owns and hands to any number of clients: each hand-out is an event of its own (a
/// client closes its read end when it is done), and signalling or clearing reaches all of them.
#[derive(Default)]
pub struct SysEvent {
    ends: std::sync::Mutex<(bool, Vec<Event>)>,
}

impl SysEvent {
    pub const fn new() -> SysEvent {
        SysEvent { ends: std::sync::Mutex::new((false, Vec::new())) }
    }
    /// A read end for a client, in the event's current state.
    pub fn handout(&self) -> u32 {
        let mut g = self.ends.lock().unwrap();
        let e = Event::new();
        if g.0 {
            e.signal();
        }
        g.1.push(e);
        e.readable
    }
    pub fn signal(&self) {
        let mut g = self.ends.lock().unwrap();
        g.0 = true;
        for e in &g.1 {
            e.signal();
        }
    }
    pub fn clear(&self) {
        let mut g = self.ends.lock().unwrap();
        g.0 = false;
        for e in &g.1 {
            e.clear();
        }
    }
    pub fn is_signalled(&self) -> bool {
        self.ends.lock().unwrap().0
    }
}
