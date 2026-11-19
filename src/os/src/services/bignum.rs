//! The SDK's hand-written big-number routines (nn::crypto, 32-bit words), as host functions.
//!
//! They are assembly with untyped symbols, reached only through the SDK's PLT, so the translator does not seed
//! them as functions and a call to one lands in the middle of the code lifted before it (requested in
//! docs/INTERFACES.md, translator). Taking them over by name routes the PLT calls here. Semantics as the
//! assembly computes them: words are u32, carries u32.

use crate::guest::{r32, w32};
use crate::hle::Registry;
use nxrt::cpu::Cpu;

/// r[i] = a[i] * w + r[i] + carry; returns the carry.
pub fn mul_add(r: &mut [u32], a: &[u32], w: u32) -> u32 {
    let mut c = 0u64;
    for (ri, &ai) in r.iter_mut().zip(a) {
        let t = ai as u64 * w as u64 + *ri as u64 + c;
        *ri = t as u32;
        c = t >> 32;
    }
    c as u32
}

/// r[i] = a[i] * w + carry; returns the carry.
pub fn mul(r: &mut [u32], a: &[u32], w: u32) -> u32 {
    let mut c = 0u64;
    for (ri, &ai) in r.iter_mut().zip(a) {
        let t = ai as u64 * w as u64 + c;
        *ri = t as u32;
        c = t >> 32;
    }
    c as u32
}

/// r = a + b; returns the carry out.
pub fn add(r: &mut [u32], a: &[u32], b: &[u32]) -> u32 {
    let mut c = 0u64;
    for i in 0..r.len() {
        let t = a[i] as u64 + b[i] as u64 + c;
        r[i] = t as u32;
        c = t >> 32;
    }
    c as u32
}

/// r = a - b; returns the borrow out.
pub fn sub(r: &mut [u32], a: &[u32], b: &[u32]) -> u32 {
    let mut borrow = 0i64;
    for i in 0..r.len() {
        let t = a[i] as i64 - b[i] as i64 - borrow;
        r[i] = t as u32;
        borrow = (t < 0) as i64;
    }
    borrow as u32
}

/// r[0..2n] = a * b (n words each).
pub fn mul_full(r: &mut [u32], a: &[u32], b: &[u32]) {
    r.iter_mut().for_each(|x| *x = 0);
    let n = a.len();
    for (j, &bj) in b.iter().enumerate() {
        let c = mul_add(&mut r[j..j + n], a, bj);
        r[j + n] = c;
    }
}

fn load(p: u64, n: usize) -> Vec<u32> {
    (0..n as u64).map(|i| r32(p + 4 * i)).collect()
}
fn store(p: u64, v: &[u32]) {
    for (i, &x) in v.iter().enumerate() {
        w32(p + 4 * i as u64, x);
    }
}

unsafe fn h_mul_add(c: *mut Cpu) {
    let (rp, ap, n, w) = ((*c).x[0], (*c).x[1], (*c).x[2] as u32 as usize, (*c).x[3] as u32);
    let mut r = load(rp, n);
    let carry = mul_add(&mut r, &load(ap, n), w);
    store(rp, &r);
    (*c).x[0] = carry as u64;
}
unsafe fn h_mul(c: *mut Cpu) {
    let (rp, ap, n, w) = ((*c).x[0], (*c).x[1], (*c).x[2] as u32 as usize, (*c).x[3] as u32);
    let mut r = vec![0; n];
    let carry = mul(&mut r, &load(ap, n), w);
    store(rp, &r);
    (*c).x[0] = carry as u64;
}
unsafe fn h_add(c: *mut Cpu) {
    let (rp, ap, bp, n) = ((*c).x[0], (*c).x[1], (*c).x[2], (*c).x[3] as u32 as usize);
    let mut r = vec![0; n];
    let carry = add(&mut r, &load(ap, n), &load(bp, n));
    store(rp, &r);
    (*c).x[0] = carry as u64;
}
unsafe fn h_sub(c: *mut Cpu) {
    let (rp, ap, bp, n) = ((*c).x[0], (*c).x[1], (*c).x[2], (*c).x[3] as u32 as usize);
    let mut r = vec![0; n];
    let borrow = sub(&mut r, &load(ap, n), &load(bp, n));
    store(rp, &r);
    (*c).x[0] = borrow as u64;
}
unsafe fn h_mul_comba8(c: *mut Cpu) {
    let (rp, ap, bp) = ((*c).x[0], (*c).x[1], (*c).x[2]);
    let mut r = vec![0; 16];
    mul_full(&mut r, &load(ap, 8), &load(bp, 8));
    store(rp, &r);
}
unsafe fn h_sqr_comba8(c: *mut Cpu) {
    let (rp, ap) = ((*c).x[0], (*c).x[1]);
    let a = load(ap, 8);
    let mut r = vec![0; 16];
    mul_full(&mut r, &a, &a);
    store(rp, &r);
}

pub fn register(r: &mut Registry) {
    const W: &str = "untyped assembly the translator does not seed (reached through the PLT)";
    r.force("nndetailCryptoBignumMulAddWords", W, h_mul_add);
    r.force("nndetailCryptoBignumMulWords", W, h_mul);
    r.force("nndetailCryptoBignumAddWords", W, h_add);
    r.force("nndetailCryptoBignumSubWords", W, h_sub);
    r.force("nndetailCryptoBignum_mul_comba8", W, h_mul_comba8);
    r.force("nndetailCryptoBignum_sqr_comba8", W, h_sqr_comba8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic() {
        let mut r = [1u32, 2];
        assert_eq!(mul_add(&mut r, &[0xFFFF_FFFF, 3], 2), 0);
        assert_eq!(r, [0xFFFF_FFFF, 9], "0xffffffff*2+1 = 0x1_ffffffff: low word, carry 1 into the next");
        let mut r = [0u32; 2];
        assert_eq!(add(&mut r, &[0xFFFF_FFFF, 0xFFFF_FFFF], &[1, 0]), 1);
        assert_eq!(r, [0, 0]);
        assert_eq!(sub(&mut r, &[0, 0], &[1, 0]), 1);
        assert_eq!(r, [0xFFFF_FFFF, 0xFFFF_FFFF]);
        let mut p = [0u32; 4];
        mul_full(&mut p, &[0xFFFF_FFFF, 0xFFFF_FFFF], &[0xFFFF_FFFF, 0xFFFF_FFFF]);
        // (2^64-1)^2 = 2^128 - 2^65 + 1
        assert_eq!(p, [1, 0, 0xFFFF_FFFE, 0xFFFF_FFFF]);
    }
}
