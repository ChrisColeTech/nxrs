//! cfg: the config-savegame blocks and the region/model handshakes. 3dbrew `Config Services`
//! rev 25480. The block data is written into the caller's static buffer (the request's pointer),
//! so the reply carries only the result.

use crate::hle::{Host, Reply, Request, R_OK, R_INVALID_CMD};
use runtime3ds::Process3ds;

pub const GET_CONFIG_INFO_BLK2: u32 = 0x0001;
pub const SECURE_INFO_GET_REGION: u32 = 0x0002;
pub const GET_SYSTEM_MODEL: u32 = 0x0005;
pub const GET_MODEL_NINTENDO_2DS: u32 = 0x0006;
pub const TRANSLATE_COUNTRY_INFO: u32 = 0x0008;
pub const GET_COUNTRY_CODE_STRING: u32 = 0x0009;
pub const GET_COUNTRY_CODE_ID: u32 = 0x000A;
pub const IS_FANGATE_SUPPORTED: u32 = 0x000B;
pub const GET_CONFIG_INFO_BLK8: u32 = 0x0401;
pub const SET_CONFIG_INFO_BLK4: u32 = 0x0402;
pub const UPDATE_CONFIG_NAND_SAVEGAME: u32 = 0x0403;
pub const GET_LOCAL_FRIEND_CODE_SEED_DATA: u32 = 0x0404;
pub const GET_LOCAL_FRIEND_CODE_SEED: u32 = 0x0405;
pub const SECURE_INFO_GET_REGION_S: u32 = 0x0406;
pub const SECURE_INFO_GET_BYTE101: u32 = 0x0407;
pub const SECURE_INFO_GET_SERIAL_NO: u32 = 0x0408;
pub const GET_LOCAL_FRIEND_CODE_SEED_DATA_I: u32 = 0x080F;
pub const SECURE_INFO_GET_REGION_I: u32 = 0x0816;

/// USA
const REGION: u8 = 1;
/// Old3DS
const MODEL: u8 = 0;

/// a config block's bytes: the language/region fields a menu reads are set, the rest zeros
pub fn block(block_id: u32, size: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; size];
    match block_id {
        // the main config block: language byte (English)
        0x0000_0000 => {
            if bytes.len() > 0x04 {
                bytes[0x04] = 1;
            }
        }
        // LCD brightness / power-saving: off
        0x0005_0001 => {
            if bytes.len() > 1 {
                bytes[0] = 0;
            }
        }
        _ => {}
    }
    bytes
}

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        GET_CONFIG_INFO_BLK2 | GET_CONFIG_INFO_BLK8 => {
            let block_id = request.word(0);
            let size = request.word(1) as usize;
            let output = request.word(3);
            let bytes = block(block_id, size.min(0x4000));
            process.memory.write_bytes(output, &bytes);
            host.say(format!("cfg: block {block_id:#010x}, {size} bytes -> {output:#x}"));
            Reply::ok()
        }
        SET_CONFIG_INFO_BLK4 => Reply::ok(),
        UPDATE_CONFIG_NAND_SAVEGAME => Reply::ok(),
        SECURE_INFO_GET_REGION | SECURE_INFO_GET_REGION_S | SECURE_INFO_GET_REGION_I => {
            Reply::new(vec![R_OK, REGION as u32], 0)
        }
        GET_SYSTEM_MODEL => Reply::new(vec![R_OK, MODEL as u32], 0),
        GET_MODEL_NINTENDO_2DS => Reply::new(vec![R_OK, 0], 0),
        TRANSLATE_COUNTRY_INFO => Reply::new(vec![R_OK, 0], 0),
        GET_COUNTRY_CODE_STRING => Reply::new(vec![R_OK, 0], 0),
        GET_COUNTRY_CODE_ID => Reply::new(vec![R_OK, 1], 0),
        IS_FANGATE_SUPPORTED => Reply::new(vec![R_OK, 0], 0),
        GET_LOCAL_FRIEND_CODE_SEED_DATA | GET_LOCAL_FRIEND_CODE_SEED_DATA_I => Reply::ok(),
        GET_LOCAL_FRIEND_CODE_SEED => Reply::new(vec![R_OK, 0, 0], 0),
        SECURE_INFO_GET_BYTE101 => Reply::new(vec![R_OK, 0], 0),
        SECURE_INFO_GET_SERIAL_NO => {
            // a 15-character serial into the caller's buffer (request word 3 is the pointer)
            let serial = b"NXRS00000000000\0";
            process.memory.write_bytes(request.word(2), serial);
            Reply::ok()
        }
        other => {
            host.gap(format!("cfg: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hle::Host;
    use runtime3ds::context;
    use runtime3ds::dispatch::Library;
    use runtime3ds::mem::{Kind, Memory, Perm, State};
    use runtime3ds::BootOptions;
    use std::sync::Arc;

    fn process() -> (Arc<Memory>, Box<Process3ds>) {
        let memory = Arc::new(Memory::new().unwrap());
        memory
            .map(0x0800_0000, 0x0010_0000, Perm::Rw, State::Private, Kind::Heap)
            .unwrap();
        memory.write_bytes(0x0800_0000, &[0xAA; 0x40]);
        let library = unsafe {
            Library::linked(context::ABI, [].as_ptr(), 0, std::ptr::null(), 0, std::ptr::null(), 0)
        };
        (memory.clone(), Box::new(Process3ds::new(library, memory, BootOptions::default())))
    }

    #[test]
    fn the_region_answers_usa() {
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let (_memory, process) = process();
        let request = Request { cmd: SECURE_INFO_GET_REGION, normal: 0, translate: 0, params: vec![], translated: vec![] };
        let reply = command(&host, &process, 0, &request);
        assert_eq!(reply.words, vec![R_OK, REGION as u32]);
    }

    #[test]
    fn a_block_writes_the_callers_buffer() {
        let host = Host::new(std::env::temp_dir(), Vec::new());
        let (memory, process) = process();
        let request = Request {
            cmd: GET_CONFIG_INFO_BLK2,
            normal: 4,
            translate: 2,
            params: vec![0x0000_0000, 0x10, 0, 0x0800_0000],
            translated: vec![2 | (0x10 << 14), 0x0800_0000],
        };
        let reply = command(&host, &process, 0, &request);
        assert_eq!(reply.words[0], R_OK);
        assert_eq!(memory.read8(0x0800_0004), 1, "language byte");
        assert_eq!(memory.read8(0x0800_0000), 0);
    }
}
