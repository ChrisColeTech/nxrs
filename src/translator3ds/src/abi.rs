//! the contract between the generated C and the host that runs it: `recomp.h` v8, verbatim
//! (the donor's `abi/recomp.h` at the pin). We do not fork the ABI; every host the 3DS port builds is an
//! implementation of this header, docs/interface.md is its prose. The four mirror items below are the only
//! ones codegen needs from the donor's Rust twin (`abi/src/lib.rs`); the full twin arrives with runtime3ds.

/// recomp.h, the generated code includes it
pub const HEADER: &str = include_str!("abi/recomp.h");

/// interface version, code has to match it to run (recomp.h's RECOMP_ABI)
pub const ABI: u32 = 8;

/// codegen version, bumped when its code runs way better or does more (the donor's mirror of the header's)
pub const GENERATION: u32 = 6;

/// an entry's owner when handwritten code runs it
pub const NO_ORIGIN: u32 = u32::MAX;

/// hash of the bytes a func came from, 64 bit FNV1a over its spans back to back
#[derive(Debug, Clone, Copy)]
pub struct CodeHash(u64);

impl Default for CodeHash {
    fn default() -> Self {
        CodeHash(0xCBF2_9CE4_8422_2325)
    }
}

impl CodeHash {
    pub fn add(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ byte as u64).wrapping_mul(0x0100_0000_01B3);
        }
    }

    pub fn value(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the hash the donor's twin gives the same bytes (its origins tables' fixtures depend on this order)
    #[test]
    fn the_code_hash_is_the_donor_fnv1a() {
        let mut hash = CodeHash::default();
        hash.add(&[0x00, 0x00, 0x00, 0xEB, 0x1E, 0xFF, 0x2F, 0xE1]);
        assert_eq!(hash.value(), 0xABC3_A7C1_D93C_ED8F);
    }
}
