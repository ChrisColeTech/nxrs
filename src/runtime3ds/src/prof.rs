//! the hot-set / miss recorder: where the interpreter had to run and why, written as
//! `<data>\discovered.tsv` (one address per line, bit 0 Thumb — the hints file W1's pipeline
//! reads for the next build) and `<data>\hotset.txt` (the miss counts, W7's websplit side).

use crate::interp::MissLog;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

struct Miss {
    count: u64,
    why: String,
}

pub struct Recorder {
    misses: Mutex<BTreeMap<u32, Miss>>,
    /// addresses the interpreter ran from with no recompiled entry (hints for the next build)
    hints: Mutex<BTreeSet<u32>>,
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder { misses: Mutex::new(BTreeMap::new()), hints: Mutex::new(BTreeSet::new()) }
    }

    fn record(&self, address: u32, why: &str) {
        let mut misses = self.misses.lock().unwrap();
        let miss = misses.entry(address).or_insert_with(|| Miss { count: 0, why: why.to_owned() });
        miss.count += 1;
        if miss.why != why {
            miss.why.push_str(&format!("; {why}"));
        }
    }

    /// an address the interpreter had to run: a function seed for the next build's hints
    pub fn hint(&self, address: u32) {
        self.hints.lock().unwrap().insert(address & !1);
    }

    /// addresses with misses, ascending
    pub fn addresses(&self) -> Vec<(u32, u64, String)> {
        self.misses
            .lock()
            .unwrap()
            .iter()
            .map(|(&address, miss)| (address, miss.count, miss.why.clone()))
            .collect()
    }

    pub fn total(&self) -> u64 {
        self.misses.lock().unwrap().values().map(|m| m.count).sum()
    }

    /// the hints file: one address per line, bit 0 set for Thumb; the miss sites and the
    /// addresses the interpreter ran from (so the next build covers both)
    pub fn write_hints(&self, path: &std::path::Path) -> std::io::Result<usize> {
        let mut sites: BTreeSet<u32> = self.misses.lock().unwrap().keys().copied().collect();
        sites.extend(self.hints.lock().unwrap().iter().copied());
        let mut text = String::new();
        for address in &sites {
            text.push_str(&format!("0x{address:08X}\n"));
        }
        std::fs::write(path, text)?;
        Ok(sites.len())
    }

    /// the hot-set file: address (bit 0 Thumb) and the miss count per line
    pub fn write_hotset(&self, path: &std::path::Path) -> std::io::Result<usize> {
        let mut text = String::new();
        for &(address, count, _) in &self.addresses() {
            text.push_str(&format!("0x{address:08X} {count}\n"));
        }
        std::fs::write(path, text)?;
        Ok(self.addresses().len())
    }
}

/// the per-thread handle the run loop hands the interpreter
pub struct Handle(Arc<Recorder>);

impl Handle {
    pub fn new(recorder: &std::sync::Arc<Recorder>) -> Handle {
        Handle(recorder.clone())
    }
}

impl MissLog for Handle {
    fn miss(&mut self, address: u32, _opcode: u32, why: &str) {
        self.0.record(address, why);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the recorder counts per site, names the reasons and writes both files
    #[test]
    fn the_recorder_counts_and_writes() {
        let recorder = std::sync::Arc::new(Recorder::new());
        {
            let mut log = Handle::new(&recorder);
            log.miss(0x0010_0101, 0, "an msr control field");
            log.miss(0x0010_0101, 0, "an msr control field");
            log.miss(0x0010_0201, 0, "a coprocessor this host does not have");
        }
        assert_eq!(recorder.total(), 3);
        let sites = recorder.addresses();
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].0, 0x0010_0101, "ascending");
        assert_eq!(sites[0].2, "an msr control field", "the same reason is not repeated");
        let dir = std::env::temp_dir().join(format!("runtime3ds-prof-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hints = dir.join("discovered.tsv");
        let hotset = dir.join("hotset.txt");
        assert_eq!(recorder.write_hints(&hints).unwrap(), 2);
        assert_eq!(recorder.write_hotset(&hotset).unwrap(), 2);
        let hints = std::fs::read_to_string(&hints).unwrap();
        assert!(hints.contains("0x00100101\n"), "{hints}");
        assert!(hints.contains("0x00100201\n"), "{hints}");
        let hotset = std::fs::read_to_string(&hotset).unwrap();
        assert!(hotset.contains("0x00100101 2\n"), "{hotset}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
