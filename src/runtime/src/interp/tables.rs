//! The estimate, CRC and AES tables of the generated code's `simd.h`, read from that header itself (the translator
//! vendors it as `src/translator/runtime/simd.h`), so the interpreter and the lifted code share one copy.

use std::sync::OnceLock;

const SIMD_H: &str = include_str!("../../../translator/runtime/simd.h");

pub struct Tables {
    pub recip: Vec<u8>,
    pub rsqrt: Vec<u8>,
    pub crc32: Vec<u32>,
    pub crc32c: Vec<u32>,
    pub aes_sbox: Vec<u8>,
    pub aes_isbox: Vec<u8>,
}

/// The values of `static const <type> <name>[<n>] = { ... };` in simd.h.
fn table(name: &str, n: usize) -> Vec<u64> {
    let head = format!("{name}[{n}] = {{");
    let at = SIMD_H.find(&head).unwrap_or_else(|| panic!("simd.h has no table {name}[{n}]")) + head.len();
    let end = at + SIMD_H[at..].find("};").expect("table end");
    let v: Vec<u64> = SIMD_H[at..end]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match s.strip_prefix("0x") {
            Some(h) => u64::from_str_radix(h, 16).expect("hex entry"),
            None => s.parse().expect("decimal entry"),
        })
        .collect();
    assert_eq!(v.len(), n, "simd.h table {name}");
    v
}

pub fn get() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let b = |name, n| table(name, n).into_iter().map(|v| v as u8).collect();
        let w = |name, n| table(name, n).into_iter().map(|v| v as u32).collect();
        Tables {
            recip: b("simd_recip_tab", 256),
            rsqrt: b("simd_rsqrt_tab", 384),
            crc32: w("simd_crc32_tab", 256),
            crc32c: w("simd_crc32c_tab", 256),
            aes_sbox: b("simd_aes_sbox", 256),
            aes_isbox: b("simd_aes_isbox", 256),
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn tables_parse() {
        let t = super::get();
        assert_eq!(t.recip[0], 255);
        assert_eq!(t.rsqrt[383], 0);
        assert_eq!(t.crc32[1], 0x77073096);
        assert_eq!(t.crc32c[1], 0xf26b8303);
        assert_eq!(t.aes_sbox[0], 0x63);
        assert_eq!(t.aes_isbox[0], 0x52);
    }
}
