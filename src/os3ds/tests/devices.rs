//! hid's scripted ring, cfg's blocks, gsp's queue and interrupts, dsp's address conversion,
//! ldr:ro's module-base write

mod common;

use common::{process, staged, HEAP, TLS};
use os3ds::hle::{parse_input, Request, SharedKind, GSP_VBLANK0, GSP_VBLANK1, R_INVALID_CMD, R_OK};
use os3ds::services::{cfg, dsp, gsp, hid, ldr_ro};
use os3ds::Host;
use runtime3ds::context::{self, Entry, Module};
use runtime3ds::dispatch::Library;
use std::ffi::c_char;

fn request(cmd: u32, params: Vec<u32>, translated: Vec<u32>) -> Request {
    Request { cmd, normal: params.len(), translate: translated.len(), params, translated }
}

/// a shared block the tests map at a free address
const SHARED_AT: u32 = 0x1000_0000;

#[test]
fn the_hid_script_lands_in_successive_ring_entries() {
    let script = parse_input("0 neutral\n10 a+left\n20 neutral\n").unwrap();
    let host = Host::new(staged("hid"), script);
    let process = process();
    let reply = hid::command(&host, &process, TLS, &request(hid::GET_IPC_HANDLES, vec![], vec![]));
    assert_eq!(reply.words[..2], [R_OK, 0x1400_0000]);
    assert!(reply.words[2..8].iter().all(|&handle| handle != 0), "block + five events");
    assert!(host.map_block(&process, reply.words[2], SHARED_AT));
    let memory = &process.memory;
    // the map published entry 0 (neutral)
    assert_eq!(memory.read32(SHARED_AT + 0x10), 0);
    host.input.lock().unwrap().advance(15);
    assert!(host.write_pad(&process));
    // entry 1: A (bit 0) + Left (bit 5) held and newly pressed
    assert_eq!(memory.read32(SHARED_AT + 0x10), 1, "latest index");
    assert_eq!(memory.read32(SHARED_AT + 0x28 + 0x10), 0x21);
    assert_eq!(memory.read32(SHARED_AT + 0x28 + 0x14), 0x21, "pressed");
    assert_eq!(memory.read32(SHARED_AT + 0x1C), 0x21, "raw PAD state");
    let tick1 = memory.read32(SHARED_AT);
    host.input.lock().unwrap().advance(25);
    host.write_pad(&process);
    assert_eq!(memory.read32(SHARED_AT + 0x10), 2);
    assert_eq!(memory.read32(SHARED_AT + 0x28 + 0x20), 0);
    assert_eq!(memory.read32(SHARED_AT + 0x28 + 0x28), 0x21, "released");
    assert_eq!(memory.read32(SHARED_AT + 0x8), tick1, "previous tick");
    assert_eq!(host.input_applied().len(), 3);
}

#[test]
fn a_config_block_lands_in_the_mapped_buffer_and_unknown_ones_are_refused() {
    let host = Host::new(staged("cfg"), Vec::new());
    let process = process();
    process.memory.write_bytes(HEAP, &[0xAA; 8]);
    // 3dbrew CFG:GetConfigInfoBlk2: size, block id; mapped-buffer descriptor + pointer
    let language = request(cfg::GET_CONFIG_INFO_BLK2, vec![1, 0x000A_0002], vec![(1 << 4) | 0xC, HEAP]);
    assert_eq!(cfg::command(&host, &process, TLS, &language).words[0], R_OK);
    assert_eq!(process.memory.read8(HEAP), cfg::LANGUAGE_ENGLISH);
    assert_eq!(process.memory.read8(HEAP + 1), 0xAA, "only the block's size is written");
    let country = request(cfg::GET_CONFIG_INFO_BLK2, vec![4, 0x000B_0000], vec![(4 << 4) | 0xC, HEAP]);
    cfg::command(&host, &process, TLS, &country);
    assert_eq!(process.memory.read8(HEAP + 3), cfg::COUNTRY_USA);
    let unknown = request(cfg::GET_CONFIG_INFO_BLK2, vec![4, 0x0099_0000], vec![(4 << 4) | 0xC, HEAP]);
    assert_ne!(cfg::command(&host, &process, TLS, &unknown).words[0], R_OK);
    assert!(host.gaps().iter().any(|(line, _)| line.contains("0x00990000")));
    let region = cfg::command(&host, &process, TLS, &request(cfg::SECURE_INFO_GET_REGION, vec![], vec![]));
    assert_eq!(region.words, vec![R_OK, cfg::REGION as u32]);
    let not_2ds = cfg::command(&host, &process, TLS, &request(cfg::GET_MODEL_NINTENDO_2DS, vec![], vec![]));
    assert_eq!(not_2ds.words, vec![R_OK, 1]);
}

#[test]
fn gsp_runs_cpu_gx_commands_and_refuses_gpu_ones() {
    let host = Host::new(staged("gsp"), Vec::new());
    let process = process();
    let register = request(gsp::REGISTER_INTERRUPT_RELAY_QUEUE, vec![1], vec![0, process.create_event(0)]);
    let reply = gsp::command(&host, &process, TLS, &register);
    assert_eq!(reply.words[0], gsp::R_FIRST_INITIALIZATION);
    assert!(host.map_block(&process, reply.words[3], SHARED_AT));
    let memory = &process.memory;
    let queue = SHARED_AT + gsp::COMMAND_QUEUE;
    // command 0: DMA HEAP -> HEAP+0x100 (4 bytes); command 1: memory fill of buffer 0, 32-bit
    memory.write_bytes(HEAP, b"gsp!");
    for (i, word) in [0, HEAP, HEAP + 0x100, 4].iter().enumerate() {
        memory.write32(queue + 0x20 + 4 * i as u32, *word);
    }
    for (i, word) in [2, HEAP + 0x200, 0x1122_3344, HEAP + 0x208, 0, 0, 0, 0x201].iter().enumerate() {
        memory.write32(queue + 0x40 + 4 * i as u32, *word);
    }
    // command 2: a display transfer (PICA): refused
    memory.write32(queue + 0x60, 3);
    memory.write8(queue + 1, 3);
    gsp::command(&host, &process, TLS, &request(gsp::TRIGGER_CMD_REQ_QUEUE, vec![], vec![]));
    let mut out = [0u8; 4];
    memory.read_bytes(HEAP + 0x100, &mut out);
    assert_eq!(&out, b"gsp!");
    assert_eq!(memory.read32(HEAP + 0x204), 0x1122_3344);
    assert_eq!(memory.read8(queue + 1), 0, "drained");
    // the interrupt queue: DMA (6), then PSC0 (0); no PPF for the refused transfer
    assert_eq!(memory.read8(SHARED_AT + 1), 2);
    assert_eq!(memory.read8(SHARED_AT + 0xC), gsp::IRQ_DMA);
    assert_eq!(memory.read8(SHARED_AT + 0xD), gsp::IRQ_PSC0);
    assert!(host.gaps().iter().any(|(line, _)| line.contains("display transfer")));
    // the frame clock pushes the two VBlanks
    host.frame(&process);
    assert_eq!(memory.read8(SHARED_AT + 1), 4);
    assert_eq!(memory.read8(SHARED_AT + 0xE), GSP_VBLANK0);
    assert_eq!(memory.read8(SHARED_AT + 0xF), GSP_VBLANK1);
    let regs = gsp::command(&host, &process, TLS, &request(gsp::WRITE_HW_REGS, vec![0x400000, 4], vec![]));
    assert_eq!(regs.words[0], R_INVALID_CMD);
    let _ = SharedKind::Gsp;
}

#[test]
fn dsp_converts_dram_addresses_and_refuses_the_pipe() {
    let host = Host::new(staged("dsp"), Vec::new());
    let process = process();
    let convert = request(dsp::CONVERT_PROCESS_ADDRESS_FROM_DSP_DRAM, vec![0x8000], vec![]);
    assert_eq!(dsp::command(&host, &process, TLS, &convert).words, vec![R_OK, 0x1FF5_0000]);
    let pipe = request(dsp::WRITE_PROCESS_PIPE, vec![2, 4], vec![]);
    assert_eq!(dsp::command(&host, &process, TLS, &pipe).words[0], R_INVALID_CMD);
    assert!(!host.gaps().is_empty());
}

#[repr(C)]
struct RawModule {
    name: *const c_char,
    base: *mut u32,
    size: u32,
    count: u32,
    entries: *const Entry,
}

#[test]
fn ldr_ro_writes_a_module_base_where_the_code_reads_it_and_refuses_loads() {
    let name = b"Battle.cro\0";
    let mut base: u32 = 0;
    let raw = RawModule { name: name.as_ptr() as *const c_char, base: &mut base, size: 0x1000, count: 0, entries: std::ptr::null() };
    // safe: one module record laid out like the generated table, no entries
    let mut library = unsafe {
        Library::linked(context::ABI, [].as_ptr(), 0, &raw as *const RawModule as *const Module, 1, std::ptr::null(), 0)
    };
    assert!(ldr_ro::place_module(&mut library, "Battle.cro", 0x0012_3400));
    assert_eq!(base, 0x0012_3400);
    assert!(!ldr_ro::place_module(&mut library, "Missing.cro", 0));
    let host = Host::new(staged("ldr"), Vec::new());
    let process = process();
    let load = ldr_ro::command(&host, &process, TLS, &request(ldr_ro::LOAD_CRO, vec![], vec![]));
    assert_eq!(load.words[0], R_INVALID_CMD, "no CRO mapping yet: refused, not faked");
}
