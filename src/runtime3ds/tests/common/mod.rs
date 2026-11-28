//! the fixtures the integration tests share: a synthetic decrypted .cxi (the format facts the
//! translator3ds test fixtures pin), built from instruction words, translated and compiled into
//! a DLL, loaded through the mirror. The compiler lanes ride the env gate, like translator3ds's.

#![allow(dead_code)]

use runtime3ds::dispatch::Library;
use runtime3ds::mem::{Kind, Memory, Perm, State};
use translator3ds::gen::GenOptions3ds;
use translator3ds::Mods;

/// title id for test titles, no real game has it
pub const PROGRAM_ID: u64 = 0x0004_0000_0FF3_DE00;
/// the synthetic text's base, the donor fixtures'
pub const BASE: u32 = 0x0010_0000;

fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

pub fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// a decrypted .cxi: NCCH header (never encrypted) at 0x100, ExeFS at 0x600 with .code as its
/// only file, the code all text at 0x00100000
pub fn cxi(code: &[u8]) -> Vec<u8> {
    let mut out = vec![0; 0x800];
    put(&mut out, 0x100, b"NCCH");
    put(&mut out, 0x118, &PROGRAM_ID.to_le_bytes());
    out[0x18F] = 0x04;
    put(&mut out, 0x1A0, &3u32.to_le_bytes());
    put(&mut out, 0x200, b"test");
    let text = [BASE, code.len().div_ceil(0x1000) as u32, code.len() as u32];
    put(&mut out, 0x210, &text.map(u32::to_le_bytes).concat());
    put(&mut out, 0x600, b".code");
    put(&mut out, 0x60C, &(code.len() as u32).to_le_bytes());
    out.extend_from_slice(code);
    out
}

/// writes the fixture and translates it; the generated C lands in dir
pub fn generate(name: &str, code: &[u32]) -> (std::path::PathBuf, translator3ds::gen::Generated, translator3ds::rom::Title) {
    let path = std::env::temp_dir().join(format!("runtime3ds-{name}-{}.cxi", std::process::id()));
    std::fs::write(&path, cxi(&words(code))).unwrap();
    let opened = translator3ds::title::open(&path).unwrap();
    let dir = std::env::temp_dir().join(format!("runtime3ds-{name}-gen-{}", std::process::id()));
    let generated = translator3ds::gen::generate(
        &opened.title,
        Mods::default(),
        GenOptions3ds { dir: &dir, overrides: None, hints: Vec::new() },
        &|_| {},
    )
    .unwrap();
    (path, generated, opened.title)
}

/// compiles the generated C into a DLL with the donor's flags (the env gate keeps the default
/// suite seconds-fast; the host-run proofs are the wave's evidence, so they gate on their own)
pub fn compile(name: &str, dir: &std::path::Path, sources: &[String]) -> Option<std::path::PathBuf> {
    if std::env::var_os("NXRS_3DS_CC_TESTS").is_none() {
        return None;
    }
    let compiler = translator3ds::compile::check(None).ok()?;
    let library = dir.join(format!("runtime3ds_{name}.dll"));
    translator3ds::compile::compile(&compiler, dir, sources, &library, &|_, _| true, false).ok()?;
    Some(library)
}

/// a code library loaded through the mirror (skips its test when the compile lanes are off)
pub fn loaded_library(name: &str, code: &[u32]) -> Option<(Library, std::path::PathBuf, std::path::PathBuf, translator3ds::rom::Title)> {
    let (dump, generated, title) = generate(name, code);
    let library = compile(name, &generated.dir, &generated.sources)?;
    let loaded = Library::open(&library).expect("the mirror loads the DLL");
    Some((loaded, library, dump, title))
}

/// the memory a synthetic boot runs in: the text read-only at its base, a page of scratch,
/// a stack, and the TLS slot
pub fn test_memory(code_bytes: &[u8]) -> Arc<Memory> {
    let memory = Arc::new(Memory::new().unwrap());
    let rounded = code_bytes.len().next_multiple_of(0x1000) as u32;
    memory.map(BASE, rounded.max(0x1000), Perm::R, State::Code, Kind::Exe).unwrap();
    memory.load_bytes(BASE, code_bytes);
    memory.map(0x1000_0000, 0x1_0000, Perm::Rw, State::Private, Kind::Heap).unwrap();
    memory.map(0x0FF0_0000, 0x1_0000, Perm::Rw, State::Private, Kind::Stack).unwrap();
    memory.map(0x1FF8_2000, 0x1000, Perm::Rw, State::Private, Kind::Tls).unwrap();
    memory
}

use std::sync::Arc;
