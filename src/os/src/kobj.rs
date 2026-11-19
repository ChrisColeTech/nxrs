//! Kernel objects the os layer makes for the guest, on the runtime kernel: events (a service's system events, the
//! read end copied to the client), signalled and cleared by the host. They are ordinary kernel events, so the
//! guest waits on them with WaitSynchronization like any other.

fn kernel() -> &'static nxrt::kernel::Kernel {
    nxrt::kernel::get()
}

/// An event made by the host: it keeps the write end and hands out the read end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub writable: u32,
    pub readable: u32,
}

impl Event {
    pub fn new() -> Event {
        let (writable, readable) = kernel().create_event();
        Event { writable, readable }
    }
    /// A new event, already signalled (a state the client reads as "there is something").
    pub fn signalled() -> Event {
        let e = Event::new();
        e.signal();
        e
    }
    pub fn signal(&self) {
        kernel().signal_event(self.writable);
    }
    pub fn clear(&self) {
        kernel().clear_event(self.writable);
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
