//! H.264 and AAC on the web, under the Windows module's name (`mf`) so its users are the same. The browser decodes
//! both (WebCodecs' VideoDecoder / AudioDecoder) from the page; until that bridge exists (docs/35, after W4) the
//! decoders refuse to open with the reason, and the codec services report it to the game as they do any open failure.

use super::yuv::Picture;

const NOT_YET: &str = "H.264 / AAC decoding in a web build comes through the browser's WebCodecs, not wired yet (docs/35)";

pub fn version() -> String {
    "none (web build: WebCodecs not wired yet)".into()
}

pub struct H264 {
    pub out: std::collections::VecDeque<Picture>,
    pub errors: u32,
}

impl H264 {
    pub fn new() -> Result<H264, String> {
        Err(NOT_YET.into())
    }
    pub fn decode(&mut self, _au: &[u8], _pts_us: i64) -> Result<(), String> {
        Err(NOT_YET.into())
    }
    pub fn drain(&mut self) {}
    pub fn flush(&mut self) {}
}

pub struct Aac {
    pub channels: u32,
    pub rate: u32,
}

impl Aac {
    pub fn new(_rate: u32, _channels: u32, _asc: &[u8]) -> Result<Aac, String> {
        Err(NOT_YET.into())
    }
    pub fn decode(&mut self, _frame: &[u8]) -> Result<Vec<u8>, String> {
        Err(NOT_YET.into())
    }
    pub fn flush(&mut self) {}
}

/// The AudioSpecificConfig for AAC-LC of a rate and channel count (when a stream brings none).
pub fn aac_lc_config(rate: u32, channels: u32) -> [u8; 2] {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let fi = RATES.iter().position(|&r| r == rate).unwrap_or(3) as u8;
    [
        (2 << 3) | (fi >> 1),
        ((fi & 1) << 7) | (((channels & 15) as u8) << 3),
    ]
}
