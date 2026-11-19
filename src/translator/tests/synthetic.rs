//! End-to-end on a tiny hand-built NSO: discovery, emission and the C text of two functions.

use translator::aot::{generate, GenOptions};
use std::collections::BTreeMap;

/// An uncompressed NSO: .text = words, .rodata = MOD0 + an empty dynamic section, no .data.
fn nso(words: &[u32]) -> Vec<u8> {
    let text: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let ro_off = 0x1000u32;
    let mut ro = vec![0u8; 0x100];
    ro[0..4].copy_from_slice(b"MOD0");
    let rel = |v: u32| (v as i32 - ro_off as i32).to_le_bytes();
    ro[4..8].copy_from_slice(&rel(ro_off + 0x20)); // dynamic: one DT_NULL entry (zeros)
    ro[8..12].copy_from_slice(&rel(0x2000)); // bss start
    ro[12..16].copy_from_slice(&rel(0x2000)); // bss end
    ro[16..20].copy_from_slice(&rel(0)); // eh_frame_hdr: none
    ro[20..24].copy_from_slice(&rel(0));
    ro[24..28].copy_from_slice(&rel(0));
    let data = vec![0u8; 0x10];
    let mut h = vec![0u8; 0x100];
    h[0..4].copy_from_slice(b"NSO0");
    let segs = [(0x100u32, 0u32, &text), (0x100 + text.len() as u32, ro_off, &ro), (0x100 + text.len() as u32 + 0x100, 0x1800, &data)];
    for (i, (foff, moff, d)) in segs.iter().enumerate() {
        h[0x10 + i * 0x10..0x14 + i * 0x10].copy_from_slice(&foff.to_le_bytes());
        h[0x14 + i * 0x10..0x18 + i * 0x10].copy_from_slice(&moff.to_le_bytes());
        h[0x18 + i * 0x10..0x1C + i * 0x10].copy_from_slice(&(d.len() as u32).to_le_bytes());
        h[0x60 + i * 4..0x64 + i * 4].copy_from_slice(&(d.len() as u32).to_le_bytes());
    }
    h[0x40..0x44].copy_from_slice(&[0xab, 0xcd, 0, 0]);
    let mut out = h;
    out.extend(&text);
    out.extend(&ro);
    out.extend(&data);
    out
}

#[test]
fn two_functions() {
    // 0x00: b +8          0x04: MOD0 offset word (0x1000)
    // 0x08: bl 0x10       0x0c: ret
    // 0x10: cmp w0, #3    0x14: b.ne 0x1c   0x18: mov w0, #1   0x1c: ret
    let words = [0x14000002, 0x1000, 0x94000002, 0xd65f03c0, 0x71000c1f, 0x54000041, 0x52800020, 0xd65f03c0];
    let mut exefs = BTreeMap::new();
    exefs.insert("main".to_string(), nso(&words));
    let out = std::env::temp_dir().join(format!("nxrs-synth-{}", std::process::id()));
    let opts = GenOptions { out: out.clone(), jobs: 2, ..Default::default() };
    let r = generate(&exefs, &opts, &|_| {}).unwrap();
    assert_eq!(r.modules[0].functions, 1); // only the BL target: no seeds in this image
    let c = std::fs::read_to_string(out.join("main/main_0000.c")).unwrap();
    assert!(c.contains("void f_7000000010(Cpu *c) {"), "{c}");
    assert!(c.contains("Flags F = {0};"), "{c}");
    assert!(c.contains("if (fc_sub32(&F, CC_NE)) { goto L_700000001c; }"), "{c}");
    let table = std::fs::read_to_string(out.join("main/main_table.c")).unwrap();
    assert!(table.contains("const uint64_t main_fn_count = 1;"));
    let _ = std::fs::remove_dir_all(out);
}
