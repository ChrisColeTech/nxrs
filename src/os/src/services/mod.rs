//! The services: nxnative's HLE files, one module each, registering their takeovers by SDK name and their IPC
//! services by sm name.

use crate::hle::Registry;

pub mod account;
pub mod applet;
pub mod audio;
pub mod audren;
pub mod audren_ipc;
pub mod codec;
pub mod codec_video;
pub mod fs;
pub mod hid;
pub mod hid_ext;
pub mod mii;
pub mod movie;
pub mod net;
pub mod nvdrv;
pub mod offline;
pub mod ro;
pub mod socket;
pub mod misc;
pub mod oe;
pub mod os;
pub mod sdkinit;
pub mod settings;
pub mod simple;
pub mod swkbd;
pub mod sf;
pub mod time;
pub mod vi;

/// Every service's registrations.
pub fn register(r: &mut Registry) {
    crate::ipc::register("sm:", || Box::new(crate::ipc::Sm));
    simple::register(r);
    sdkinit::register(r);
    sf::register(r);
    os::register(r);
    fs::register(r);
    account::register(r);
    mii::register(r);
    oe::register(r);
    time::register(r);
    settings::register(r);
    vi::register(r);
    hid::register(r);
    misc::register(r);
    audio::register(r);
    audren::register(r);
    codec::register(r);
    movie::register(r);
    codec_video::register(r);
    swkbd::register(r);
    net::register();
    audren_ipc::register();
    applet::register();
    socket::register(r);
    offline::register(r);
    ro::register(r);
    nvdrv::register(r);
}

/// A host function that returns `v` (a takeover whose answer is constant).
#[macro_export]
macro_rules! ret_fn {
    ($name:ident, $v:expr) => {
        unsafe fn $name(c: *mut nxrt::cpu::Cpu) {
            (*c).x[0] = $v;
        }
    };
}
