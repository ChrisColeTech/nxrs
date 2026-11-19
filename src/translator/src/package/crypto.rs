//! AES primitives as NCAs use them: ECB, section AES-CTR, and AES-XTS with Nintendo's big-endian sector tweak.
use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;

pub const MEDIA: usize = 0x200;

fn cipher(key: &[u8; 16]) -> Aes128 {
    Aes128::new(GenericArray::from_slice(key))
}

pub fn ecb_decrypt(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let c = cipher(key);
    let mut out = data.to_vec();
    for block in out.chunks_exact_mut(16) {
        c.decrypt_block(GenericArray::from_mut_slice(block));
    }
    out
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn ecb_encrypt(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let c = cipher(key);
    let mut out = data.to_vec();
    for block in out.chunks_exact_mut(16) {
        c.encrypt_block(GenericArray::from_mut_slice(block));
    }
    out
}

/// AES-CTR over `data` located at `offset` in the NCA; `upper` is the section's 8 counter bytes (big-endian
/// order), the low 8 bytes are offset/16 big-endian.
pub fn ctr(key: &[u8; 16], upper: &[u8; 8], offset: u64, data: &mut [u8]) {
    let c = cipher(key);
    let mut block_no = offset / 16;
    let mut skip = (offset % 16) as usize;
    let mut i = 0;
    while i < data.len() {
        let mut ks = [0u8; 16];
        ks[..8].copy_from_slice(upper);
        ks[8..].copy_from_slice(&block_no.to_be_bytes());
        c.encrypt_block(GenericArray::from_mut_slice(&mut ks));
        while skip < 16 && i < data.len() {
            data[i] ^= ks[skip];
            i += 1;
            skip += 1;
        }
        skip = 0;
        block_no += 1;
    }
}

/// AES-XTS over whole sectors with Nintendo's tweak (sector number big-endian). `key` = data key || tweak key.
pub fn xts(key: &[u8; 32], data: &mut [u8], first_sector: u64, sector_size: usize, encrypt: bool) {
    let k1 = cipher(key[..16].try_into().unwrap());
    let k2 = cipher(key[16..].try_into().unwrap());
    for (n, sector) in data.chunks_mut(sector_size).enumerate() {
        let mut tweak = ((first_sector + n as u64) as u128).to_be_bytes();
        k2.encrypt_block(GenericArray::from_mut_slice(&mut tweak));
        let mut t = u128::from_le_bytes(tweak);
        for block in sector.chunks_exact_mut(16) {
            let tb = t.to_le_bytes();
            for j in 0..16 {
                block[j] ^= tb[j];
            }
            let ga = GenericArray::from_mut_slice(block);
            if encrypt {
                k1.encrypt_block(ga)
            } else {
                k1.decrypt_block(ga)
            }
            for j in 0..16 {
                block[j] ^= tb[j];
            }
            let carry = t >> 127;
            t <<= 1;
            if carry != 0 {
                t ^= 0x87;
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes128_fips197_vector() {
        // FIPS-197 appendix C.1
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let pt: Vec<u8> = (0..16u8).map(|i| i * 0x11).collect();
        let ct = ecb_encrypt(&key, &pt);
        assert_eq!(hex(&ct), "69c4e0d86a7b0430d8cdb78070b4c55a");
        assert_eq!(ecb_decrypt(&key, &ct), pt);
    }

    #[test]
    fn xts_ieee1619_vector_1() {
        // IEEE 1619 vector 1: zero keys, data unit 0, 32 zero bytes (unit 0 encodes the same in either endianness).
        let key = [0u8; 32];
        let mut data = [0u8; 32];
        xts(&key, &mut data, 0, 32, true);
        assert_eq!(hex(&data), "917cf69ebd68b2ec9b9fe9a3eadda692cd43d2f59598ed858c02c2652fbf922e");
        xts(&key, &mut data, 0, 32, false);
        assert_eq!(data, [0u8; 32]);
    }

    #[test]
    fn xts_nintendo_tweak_is_big_endian_and_roundtrips() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let plain: Vec<u8> = (0..0x600).map(|i| (i * 7 % 251) as u8).collect();
        let mut data = plain.clone();
        xts(&key, &mut data, 0, MEDIA, true);
        assert_ne!(data, plain);
        // sector 1 on its own (first_sector = 1) matches the second sector of the whole run
        let mut s1 = plain[MEDIA..2 * MEDIA].to_vec();
        xts(&key, &mut s1, 1, MEDIA, true);
        assert_eq!(&s1[..], &data[MEDIA..2 * MEDIA]);
        // first block of sector 1, by hand with a big-endian tweak (00 .. 00 01)
        let k1 = cipher(key[..16].try_into().unwrap());
        let k2 = cipher(key[16..].try_into().unwrap());
        let mut t = [0u8; 16];
        t[15] = 1;
        k2.encrypt_block(GenericArray::from_mut_slice(&mut t));
        let mut blk: [u8; 16] = plain[MEDIA..MEDIA + 16].try_into().unwrap();
        blk.iter_mut().zip(t).for_each(|(b, x)| *b ^= x);
        k1.encrypt_block(GenericArray::from_mut_slice(&mut blk));
        blk.iter_mut().zip(t).for_each(|(b, x)| *b ^= x);
        assert_eq!(&blk[..], &data[MEDIA..MEDIA + 16]);
        xts(&key, &mut data, 0, MEDIA, false);
        assert_eq!(data, plain);
    }

    #[test]
    fn ctr_unaligned_matches_aligned() {
        let key = [3u8; 16];
        let upper = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut whole = vec![0u8; 100];
        ctr(&key, &upper, 0x1000, &mut whole);
        let mut part = vec![0u8; 50];
        ctr(&key, &upper, 0x1000 + 37, &mut part);
        assert_eq!(&part[..], &whole[37..87]);
        // first keystream block is AES(upper || (0x1000/16) BE)
        let mut ctrblk = [0u8; 16];
        ctrblk[..8].copy_from_slice(&upper);
        ctrblk[8..].copy_from_slice(&(0x100u64).to_be_bytes());
        assert_eq!(&ecb_encrypt(&key, &ctrblk)[..], &whole[..16]);
    }
}
