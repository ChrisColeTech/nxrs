//! cfg: the config-savegame blocks and the region/model handshakes. 3dbrew `Config Services`
//! rev 25480. The block data is written into the caller's mapped buffer (the request's
//! translate pointer); a block this host does not hold is refused with a gap line. Config writes
//! and console-unique data are refused.

use crate::hle::{Host, Reply, Request, R_INVALID_CMD, R_OK};
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

/// USA (3dbrew `CFG:SecureInfoGetRegion`: JPN 0, USA 1, EUR 2, ...)
pub const REGION: u8 = 1;
/// Old 3DS (3dbrew `CFG:GetSystemModel`)
pub const MODEL: u8 = 0;

/// USA's country code (3dbrew `Country Code List`)
pub const COUNTRY_USA: u8 = 49;
/// English (3dbrew `Config Savegame` block 0x000A0002)
pub const LANGUAGE_ENGLISH: u8 = 1;

/// a config-savegame block (3dbrew `Config Savegame`), as a fresh USA/English Old 3DS keeps it;
/// `None` for a block this host does not hold
pub fn block(block_id: u32) -> Option<Vec<u8>> {
    Some(match block_id {
        // user time offset (s64 ms): none
        0x0003_0001 => vec![0; 8],
        // backlight controls: power saving off, brightness 3
        0x0005_0001 => vec![0, 3],
        // sound output mode: stereo
        0x0007_0001 => vec![1],
        // username: "NXRS" (UTF-16LE, 0x1C bytes incl. the NUL) + the ng-word version
        0x000A_0000 => {
            let mut bytes = vec![0u8; 0x1C];
            for (i, unit) in "NXRS".encode_utf16().enumerate() {
                bytes[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
            }
            bytes
        }
        // birthday: month, day
        0x000A_0001 => vec![1, 1],
        // language
        0x000A_0002 => vec![LANGUAGE_ENGLISH],
        // country info: byte 2 = state/province, byte 3 = country
        0x000B_0000 => vec![0, 0, 2, COUNTRY_USA],
        // parental controls: none set
        0x000C_0000 => vec![0; 0xC0],
        // COPPACS: none
        0x000C_0001 => vec![0; 0x14],
        // EULA agreed version: 1.0 (the version the SDK compares against)
        0x000D_0000 => vec![0, 0, 1, 0],
        // system model: Old 3DS
        0x000F_0004 => vec![MODEL, 0, 0, 0],
        _ => return None,
    })
}

pub fn command(host: &Host, process: &Process3ds, _tls: u32, request: &Request) -> Reply {
    match request.cmd {
        GET_CONFIG_INFO_BLK2 | GET_CONFIG_INFO_BLK8 => {
            // 3dbrew CFG:GetConfigInfoBlk2 (0x00010082): word 1 = size, word 2 = block id;
            // translate: a mapped-buffer descriptor (size << 4 | 0xC) and the output pointer
            let size = request.word(0) as usize;
            let block_id = request.word(1);
            let output = request.translated.get(1).copied().unwrap_or(0);
            let Some(mut bytes) = block(block_id) else {
                host.gap(format!("cfg: config block {block_id:#010x} ({size} bytes) is not held"));
                return Reply::err(crate::hle::R_INVALID);
            };
            bytes.resize(size.min(0x4000), 0);
            process.memory.write_bytes(output, &bytes);
            host.say(format!("cfg: block {block_id:#010x}, {size} bytes -> {output:#x}"));
            Reply::new(vec![R_OK, request.translated.first().copied().unwrap_or(0), output], 2)
        }
        SET_CONFIG_INFO_BLK4 | UPDATE_CONFIG_NAND_SAVEGAME => {
            host.gap(format!("cfg: command {:#06x} (config write) refused: the config is read-only headless", request.cmd));
            Reply::err(R_INVALID_CMD)
        }
        SECURE_INFO_GET_REGION | SECURE_INFO_GET_REGION_S | SECURE_INFO_GET_REGION_I => {
            Reply::new(vec![R_OK, REGION as u32], 0)
        }
        GET_SYSTEM_MODEL => Reply::new(vec![R_OK, MODEL as u32], 0),
        // 3dbrew CFG:GetModelNintendo2DS: 0 = a 2DS, 1 = any other model
        GET_MODEL_NINTENDO_2DS => Reply::new(vec![R_OK, 1], 0),
        IS_FANGATE_SUPPORTED => Reply::new(vec![R_OK, 0], 0),
        TRANSLATE_COUNTRY_INFO | GET_COUNTRY_CODE_STRING | GET_COUNTRY_CODE_ID => {
            host.gap(format!("cfg: command {:#06x} (country-code table) not implemented", request.cmd));
            Reply::err(R_INVALID_CMD)
        }
        GET_LOCAL_FRIEND_CODE_SEED_DATA
        | GET_LOCAL_FRIEND_CODE_SEED_DATA_I
        | GET_LOCAL_FRIEND_CODE_SEED
        | SECURE_INFO_GET_BYTE101
        | SECURE_INFO_GET_SERIAL_NO => {
            host.gap(format!("cfg: command {:#06x} (console-unique data) refused", request.cmd));
            Reply::err(R_INVALID_CMD)
        }
        other => {
            host.gap(format!("cfg: command {other:#010x} not implemented"));
            Reply::err(R_INVALID_CMD)
        }
    }
}
