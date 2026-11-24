//! Bounded pre-fault native records. Producers never wait for a snapshot consumer.
use std::sync::{Mutex, atomic::{AtomicU64, Ordering}};

const CAPACITY: usize = 256;
#[derive(Clone, Copy, Debug, Default)]
pub struct Record {
    pub id: u64,
    pub serial: u64,
    pub timeline: u64,
    pub submitted: bool,
    pub retired: bool,
    pub shader: u128,
    pub groups: [u32; 3],
    pub local: [u32; 3],
    /// (buffer, offset, full descriptor range), in binding order.
    pub buffers: [[u64; 3]; 5],
    pub views: [u64; 3],
    /// Physical image creation IDs. Zero means no image/unknown, never a guessed epoch.
    pub image_generations: [u64; 3],
    pub backing_generation: u64,
    /// 0 dispatch, 1 image-to-buffer copy.
    pub kind: u8,
}

pub struct Ring {
    records: [Option<Record>; CAPACITY],
    next: u64,
    pub overwritten: u64,
}
impl Ring {
    pub const fn new() -> Self { Self { records: [None; CAPACITY], next: 1, overwritten: 0 } }
    pub fn push(&mut self, mut record: Record) {
        // Do not allow identifier wrap to join two operations.
        let Some(next) = self.next.checked_add(1) else { self.overwritten += 1; return; };
        record.id = self.next;
        let slot = (self.next % CAPACITY as u64) as usize;
        if self.records[slot].is_some() { self.overwritten += 1; }
        self.records[slot] = Some(record);
        self.next = next;
    }
    pub fn link(&mut self, serial: u64, timeline: u64, submitted: bool) {
        for r in self.records.iter_mut().flatten().filter(|r| r.serial == serial) {
            if r.timeline == 0 || r.timeline == timeline {
                r.timeline = timeline;
                r.submitted |= submitted;
            }
        }
    }
    pub fn retire(&mut self, serial: u64, timeline: u64) {
        for r in self.records.iter_mut().flatten().filter(|r| r.serial == serial) {
            if r.submitted && r.timeline == timeline { r.retired = true; }
        }
    }
    pub fn snapshot(&self, serial: u64) -> Vec<Record> {
        let mut v: Vec<_> = self.records.iter().flatten().filter(|r| r.serial == serial).copied().collect();
        v.sort_by_key(|r| r.id);
        v
    }
}
static RING: Mutex<Ring> = Mutex::new(Ring::new());
static OMITTED: AtomicU64 = AtomicU64::new(0);

fn update(f: impl FnOnce(&mut Ring)) {
    match RING.try_lock() {
        Ok(mut ring) => f(&mut ring),
        Err(_) => { OMITTED.fetch_add(1, Ordering::Relaxed); }
    }
}
pub fn record(record: Record) { update(|ring| ring.push(record)); }
pub fn link(serial: u64, timeline: u64, submitted: bool) { update(|r| r.link(serial, timeline, submitted)); }
pub fn retire(serial: u64, timeline: u64) { update(|r| r.retire(serial, timeline)); }
pub fn snapshot(serial: u64) -> Option<(Vec<Record>, u64, u64)> {
    let ring = RING.try_lock().ok()?;
    Some((ring.snapshot(serial), ring.overwritten, OMITTED.load(Ordering::Relaxed)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrap_keeps_unique_ids_and_explicit_omissions() {
        let mut r = Ring::new();
        for serial in 1..=300 { r.push(Record { serial, ..Record::default() }); }
        assert!(r.snapshot(1).is_empty());
        assert_eq!(r.overwritten, 44);
        assert_eq!(r.snapshot(300)[0].id, 300);
        r.link(1, 9, true);
        assert_eq!(r.snapshot(257)[0].timeline, 0);
    }
    #[test]
    fn attempted_failed_or_wrong_submission_cannot_retire() {
        let mut r = Ring::new();
        r.push(Record { serial: 11, ..Record::default() });
        r.link(11, 4, false); r.retire(11, 4);
        assert!(!r.snapshot(11)[0].retired);
        r.link(11, 4, true); r.retire(12, 4); r.retire(11, 3);
        assert!(!r.snapshot(11)[0].retired);
        r.retire(11, 4);
        assert!(r.snapshot(11)[0].retired);
    }
    #[test]
    fn blocked_snapshot_cannot_block_recording() {
        let _guard = RING.lock().unwrap();
        let before = OMITTED.load(Ordering::Relaxed);
        record(Record::default());
        assert_eq!(OMITTED.load(Ordering::Relaxed), before + 1);
    }
}
