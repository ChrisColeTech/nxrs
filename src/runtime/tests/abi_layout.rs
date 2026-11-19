//! The structs shared with C have the same size, alignment and field offsets on both sides: a small C program
//! compiled with clang against the vendored `cpu.h` and the runtime's `nxrt.h` prints them, and they are compared
//! with Rust's `size_of`/`align_of`/`offset_of!`. Fails loudly on any mismatch (and when clang is missing).

use core::mem::{align_of, offset_of, size_of};
use nxrt::cpu::Cpu;
use nxrt::dispatch::RtModule;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn clang() -> PathBuf {
    if let Ok(c) = std::env::var("NXRT_CLANG") {
        return c.into();
    }
    let p = Path::new(r"C:\Program Files\LLVM\bin\clang.exe");
    if p.is_file() {
        return p.into();
    }
    "clang".into()
}

const PROBE: &str = r#"
#include <stddef.h>
#include <stdio.h>
#include "cpu.h"
#include "nxrt.h"
volatile int g_fnprof;
uint8_t rt_patch_on[1];
#define S(T) printf(#T ".sizeof %zu\n" #T ".alignof %zu\n", sizeof(T), _Alignof(T));
#define F(T, f) printf(#T "." #f " %zu\n", offsetof(T, f));
int main(void) {
    S(Cpu) F(Cpu, x) F(Cpu, sp) F(Cpu, pc) F(Cpu, nzcv) F(Cpu, tpidr) F(Cpu, tpidrro) F(Cpu, excl_addr)
    F(Cpu, excl_val) F(Cpu, thread) F(Cpu, fpcr) F(Cpu, fpsr) F(Cpu, excl_val_hi) F(Cpu, sj) F(Cpu, excl_seq) F(Cpu, v)
    S(RtModule) F(RtModule, name) F(RtModule, base) F(RtModule, size) F(RtModule, addrs) F(RtModule, fns)
    F(RtModule, count)
    return 0;
}
"#;

#[test]
fn c_and_rust_layouts_agree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!("nxrt_abi_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("probe.c"), PROBE).unwrap();
    let exe = dir.join("probe.exe");
    let out = Command::new(clang())
        .args(["-O0", "-march=x86-64-v3", "-w"])
        .arg(format!("-I{}", root.join("../translator/runtime").display()))
        .arg(format!("-I{}", root.join("include").display()))
        .arg(dir.join("probe.c"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang (set NXRT_CLANG) is needed for the ABI layout check");
    assert!(out.status.success(), "probe failed to compile:\n{}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new(&exe).output().unwrap();
    let c: BTreeMap<String, usize> = String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(|l| {
            let (k, v) = l.split_once(' ').unwrap();
            (k.to_string(), v.trim().parse().unwrap())
        })
        .collect();
    let rust: BTreeMap<String, usize> = [
        ("Cpu.sizeof", size_of::<Cpu>()),
        ("Cpu.alignof", align_of::<Cpu>()),
        ("Cpu.x", offset_of!(Cpu, x)),
        ("Cpu.sp", offset_of!(Cpu, sp)),
        ("Cpu.pc", offset_of!(Cpu, pc)),
        ("Cpu.nzcv", offset_of!(Cpu, nzcv)),
        ("Cpu.tpidr", offset_of!(Cpu, tpidr)),
        ("Cpu.tpidrro", offset_of!(Cpu, tpidrro)),
        ("Cpu.excl_addr", offset_of!(Cpu, excl_addr)),
        ("Cpu.excl_val", offset_of!(Cpu, excl_val)),
        ("Cpu.thread", offset_of!(Cpu, thread)),
        ("Cpu.fpcr", offset_of!(Cpu, fpcr)),
        ("Cpu.fpsr", offset_of!(Cpu, fpsr)),
        ("Cpu.excl_val_hi", offset_of!(Cpu, excl_val_hi)),
        ("Cpu.sj", offset_of!(Cpu, sj)),
        ("Cpu.excl_seq", offset_of!(Cpu, excl_seq)),
        ("Cpu.v", offset_of!(Cpu, v)),
        ("RtModule.sizeof", size_of::<RtModule>()),
        ("RtModule.alignof", align_of::<RtModule>()),
        ("RtModule.name", offset_of!(RtModule, name)),
        ("RtModule.base", offset_of!(RtModule, base)),
        ("RtModule.size", offset_of!(RtModule, size)),
        ("RtModule.addrs", offset_of!(RtModule, addrs)),
        ("RtModule.fns", offset_of!(RtModule, fns)),
        ("RtModule.count", offset_of!(RtModule, count)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(c.len(), rust.len(), "the probe printed {c:?}");
    let mut mismatches = vec![];
    for (k, v) in &rust {
        match c.get(k) {
            Some(cv) if cv == v => {}
            other => mismatches.push(format!("{k}: C {other:?}, Rust {v}")),
        }
    }
    assert!(mismatches.is_empty(), "C and Rust disagree on the shared layouts:\n  {}", mismatches.join("\n  "));
}
