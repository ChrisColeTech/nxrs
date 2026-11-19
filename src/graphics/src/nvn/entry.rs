//! The NVN entry points as host functions of the runtime's bridge: each `nvn*` function also gets a wrapper
//! `unsafe fn(*mut Cpu)` that reads its AAPCS64 arguments (integer class from x0-x7 then the stack, floats from
//! v0-v7), calls it and writes the result to x0 / v0. `procs()` lists them all by name.

use nxrt::cpu::Cpu;
use std::sync::OnceLock;

pub type HostFn = unsafe fn(*mut Cpu);

#[derive(Clone, Copy)]
pub struct Entry {
    pub name: &'static str,
    pub f: HostFn,
}

/// An argument of an entry point, taken from the guest registers in AAPCS64 order.
pub trait Arg: Sized {
    /// # Safety
    /// Stack arguments are read from the guest stack.
    unsafe fn take(c: &Cpu, xi: &mut usize, vi: &mut usize) -> Self;
}
macro_rules! int_arg {
    ($($t:ty),*) => {$(
        impl Arg for $t {
            unsafe fn take(c: &Cpu, xi: &mut usize, _vi: &mut usize) -> Self {
                let v = c.arg_x(*xi);
                *xi += 1;
                v as $t
            }
        }
    )*};
}
int_arg!(u8, i8, u16, i16, u32, i32, u64, i64, usize, isize);
impl<T> Arg for *const T {
    unsafe fn take(c: &Cpu, xi: &mut usize, _vi: &mut usize) -> Self {
        let v = c.arg_x(*xi);
        *xi += 1;
        v as *const T
    }
}
impl<T> Arg for *mut T {
    unsafe fn take(c: &Cpu, xi: &mut usize, _vi: &mut usize) -> Self {
        let v = c.arg_x(*xi);
        *xi += 1;
        v as *mut T
    }
}
impl Arg for f32 {
    /// Past v7 a float goes to the next 8-byte stack slot, after the integer arguments that went to the stack before it
    /// (nvnWindowSetHdrDisplayMetadata's 9th and 10th floats).
    unsafe fn take(c: &Cpu, xi: &mut usize, vi: &mut usize) -> Self {
        let v = if *vi < 8 {
            c.arg_s(*vi)
        } else {
            let slot = (*xi).saturating_sub(8) + (*vi - 8);
            f32::from_bits(core::ptr::read_unaligned((c.sp + 8 * slot as u64) as *const u32))
        };
        *vi += 1;
        v
    }
}
impl Arg for super::types::NVNseparateTextureHandle {
    unsafe fn take(c: &Cpu, xi: &mut usize, vi: &mut usize) -> Self {
        Self { value: u64::take(c, xi, vi) }
    }
}
impl Arg for super::types::NVNseparateSamplerHandle {
    unsafe fn take(c: &Cpu, xi: &mut usize, vi: &mut usize) -> Self {
        Self { value: u64::take(c, xi, vi) }
    }
}

/// A result written back to the guest.
pub trait Ret {
    fn put(self, c: &mut Cpu);
}
impl Ret for () {
    fn put(self, _c: &mut Cpu) {}
}
macro_rules! int_ret {
    ($($t:ty),*) => {$(
        impl Ret for $t {
            fn put(self, c: &mut Cpu) { c.ret_x(self as u64); }
        }
    )*};
}
int_ret!(u8, u16, u32, u64, usize);
macro_rules! sint_ret {
    ($($t:ty => $u:ty),*) => {$(
        impl Ret for $t {
            fn put(self, c: &mut Cpu) { c.ret_x(self as $u as u64); }
        }
    )*};
}
sint_ret!(i8 => u8, i16 => u16, i32 => u32, i64 => u64, isize => usize);
impl<T> Ret for *const T {
    fn put(self, c: &mut Cpu) {
        c.ret_x(self as u64);
    }
}
impl<T> Ret for *mut T {
    fn put(self, c: &mut Cpu) {
        c.ret_x(self as u64);
    }
}
impl Ret for f32 {
    fn put(self, c: &mut Cpu) {
        c.ret_s(self);
    }
}
impl Ret for super::types::NVNseparateTextureHandle {
    fn put(self, c: &mut Cpu) {
        c.ret_x(self.value);
    }
}
impl Ret for super::types::NVNseparateSamplerHandle {
    fn put(self, c: &mut Cpu) {
        c.ret_x(self.value);
    }
}

/// Defines entry points: each `fn nvnX(args) -> R { body }` becomes `pub unsafe fn nvnX` plus a bridge wrapper listed
/// by the module's `entries()`.
macro_rules! nvn_fns {
    ($( $(#[$m:meta])* fn $name:ident ( $($a:ident : $t:ty),* $(,)? ) $(-> $r:ty)? $body:block )*) => {
        $(
            $(#[$m])*
            #[allow(non_snake_case, clippy::missing_safety_doc, unused_unsafe)]
            pub unsafe fn $name($($a: $t),*) $(-> $r)? { unsafe { $body } }
        )*
        #[allow(unused_variables, unused_mut, non_snake_case)]
        pub(crate) fn entries(v: &mut Vec<$crate::nvn::entry::Entry>) {
            $(
                {
                    #[allow(non_snake_case, unused_mut, unused_variables)]
                    unsafe fn w(c: *mut nxrt::cpu::Cpu) {
                        let c = &mut *c;
                        let mut xi = 0usize;
                        let mut vi = 0usize;
                        $( let $a = <$t as $crate::nvn::entry::Arg>::take(c, &mut xi, &mut vi); )*
                        let r = $name($($a),*);
                        $crate::nvn::entry::Ret::put(r, c);
                    }
                    v.push($crate::nvn::entry::Entry { name: stringify!($name), f: w });
                }
            )*
        }
    };
}
pub(crate) use nvn_fns;

/// Every implemented NVN entry point, by name. Lane A binds each to a guest address (`hostfn::register`) and gives the
/// addresses back through `set_proc_resolver`, which nvnBootstrapLoader / nvnDeviceGetProcAddress answer from.
pub fn procs() -> &'static [Entry] {
    static P: OnceLock<Vec<Entry>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v = Vec::new();
        super::device::entries(&mut v);
        super::pool::entries(&mut v);
        super::tex::entries(&mut v);
        super::prog::entries(&mut v);
        super::sync::entries(&mut v);
        super::queue::entries(&mut v);
        super::cmd::entries(&mut v);
        super::cmd2::entries(&mut v);
        super::cmd2::hints::entries(&mut v);
        super::state::entries(&mut v);
        super::window::entries(&mut v);
        super::nops::entries(&mut v);
        super::query::entries(&mut v);
        super::prog2::entries(&mut v);
        super::dlss::entries(&mut v);
        super::devbuf::entries(&mut v);
        v.sort_by_key(|e| e.name);
        for w in v.windows(2) {
            assert!(w[0].name != w[1].name, "NVN entry point {} defined twice", w[0].name);
        }
        v
    })
}

pub fn find(name: &str) -> Option<HostFn> {
    let p = procs();
    p.binary_search_by_key(&name, |e| e.name).ok().map(|i| p[i].f)
}

type Resolver = fn(&str) -> u64;
static RESOLVER: OnceLock<Resolver> = OnceLock::new();

/// Lane A's map from an NVN name to the guest address its host function is bound at (0 = none).
pub fn set_proc_resolver(f: fn(&str) -> u64) {
    let _ = RESOLVER.set(f);
}

/// nvnBootstrapLoader / nvnDeviceGetProcAddress: the guest address of `name`, or 0 with a red line.
pub fn resolve(name: &str) -> u64 {
    match RESOLVER.get() {
        Some(f) => {
            let a = f(name);
            if a == 0 {
                crate::log::red_once(&format!("NVN address lookup of a name with no implementation: {name}"));
            }
            a
        }
        None => {
            crate::log::red_once("NVN address lookup before the bridge set a resolver (graphics::nvn::set_proc_resolver)");
            0
        }
    }
}

/// `nvnBootstrapLoader(const char *name)`: the host function lane A overrides the game's import with.
/// # Safety
/// Guest calling convention.
pub unsafe fn nvn_bootstrap_loader(c: *mut Cpu) {
    let c = &mut *c;
    let name = super::cstr(c.arg_x(0) as *const u8);
    c.ret_x(resolve(&name));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_is_unique_and_wrappers_read_registers() {
        let p = procs();
        assert!(p.len() > 250, "{} entry points", p.len());
        // nvnSamplerBuilderSetLodClamp(b, float lo, float hi): pointer in x0, floats in v0 / v1.
        let mut b = crate::nvn::types::NVNsamplerBuilder::boxed();
        let mut c = Cpu::new_boxed();
        c.x[0] = b.0.as_mut_ptr() as u64;
        c.v[0] = 1.5f32.to_bits() as u128;
        c.v[1] = 7.25f32.to_bits() as u128;
        unsafe { find("nvnSamplerBuilderSetLodClamp").unwrap()(&mut *c) };
        let (mut lo, mut hi) = (0f32, 0f32);
        unsafe { crate::nvn::tex::nvnSamplerBuilderGetLodClamp(&*b, &mut lo, &mut hi) };
        assert_eq!((lo, hi), (1.5, 7.25));
        // a boolean result in w0
        c.x[0] = 0;
        let mut v = crate::nvn::types::NVNtextureView::boxed();
        c.x[0] = v.0.as_mut_ptr() as u64;
        let mut min = 0i32;
        let mut n = 0i32;
        c.x[1] = &mut min as *mut i32 as u64;
        c.x[2] = &mut n as *mut i32 as u64;
        unsafe { find("nvnTextureViewGetLayers").unwrap()(&mut *c) };
        assert_eq!(c.x[0], 0);
    }
}
