//! the shared fixture: a process with no generated code, a heap window for buffers, a TLS page,
//! and a fresh staged data folder

#![allow(dead_code)]

use runtime3ds::context;
use runtime3ds::dispatch::Library;
use runtime3ds::mem::{Kind, Memory, Perm, State};
use runtime3ds::{BootOptions, Process3ds};
use std::path::PathBuf;
use std::sync::Arc;

/// the heap window the tests put buffers in
pub const HEAP: u32 = 0x0800_0000;
/// a thread-local storage page (the IPC buffer is at TLS+0x80)
pub const TLS: u32 = 0x1FF8_2000;

pub fn process() -> Box<Process3ds> {
    let memory = Arc::new(Memory::new().unwrap());
    memory.map(HEAP, 0x0010_0000, Perm::Rw, State::Private, Kind::Heap).unwrap();
    memory.map(TLS, 0x1000, Perm::Rw, State::Private, Kind::Heap).unwrap();
    // safe: an empty table, no generated code is ever called
    let library = unsafe {
        Library::linked(context::ABI, [].as_ptr(), 0, std::ptr::null(), 0, std::ptr::null(), 0)
    };
    Box::new(Process3ds::new(library, memory, BootOptions::default()))
}

/// a fresh data folder: `romfs/`, `save/`, `sdmc/`
pub fn staged(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("os3ds-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("romfs")).unwrap();
    std::fs::create_dir_all(dir.join("save")).unwrap();
    std::fs::create_dir_all(dir.join("sdmc")).unwrap();
    dir
}

/// writes an IPC request at TLS+0x80: header (cmd, normal count, translate count) + words
pub fn write_request(process: &Process3ds, cmd: u32, normal: &[u32], translate: &[u32]) {
    let header = (cmd << 16) | ((normal.len() as u32) << 6) | translate.len() as u32;
    let base = TLS + 0x80;
    process.memory.write32(base, header);
    for (i, word) in normal.iter().chain(translate.iter()).enumerate() {
        process.memory.write32(base + 4 + 4 * i as u32, *word);
    }
}

/// reads reply word `index` (0 = header, 1 = result)
pub fn reply_word(process: &Process3ds, index: u32) -> u32 {
    process.memory.read32(TLS + 0x80 + 4 * index)
}
