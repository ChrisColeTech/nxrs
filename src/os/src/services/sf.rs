//! The SDK's own HIPC client (nn::sf::hipc), for services whose clients a game links into its own code: their
//! entry points are taken over where they reach the kernel, and their sessions are the host's (`ipc`). Services
//! the host serves offline answer every request with success and zeroed output (olsc:u reads as "backup off",
//! the capture services as an empty album, notif:a as no alarms, mnpp:app as events acknowledged).
//!
//! The Switch 2's camera context (nn::camera::sdk::CreateCameraContext, a session to the camera service the SDK
//! gets through the graphics system's session) is a host session to a console with no camera plugged in: opening
//! the device (3) and closing it (4) succeed, the camera's event (21) is one that never fires, and every other
//! command (QueryDeviceConnection's 29 among them) answers 0x7c4, "no camera device".

use crate::guest::{w16, w32, Args};
use crate::hle::Registry;
use crate::ipc::{self, Silent};
use nxrt::cpu::Cpu;

unsafe fn init_res(c: *mut Cpu) {
    (*c).x[0] = 0;
}
unsafe fn fin_res(_c: *mut Cpu) {}

unsafe fn connect(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    let (out, name) = (a.x(0), crate::guest::cstr(a.x(1), 8));
    match ipc::connect(&name) {
        Ok(h) => {
            w32(out, h);
            a.ret(0);
        }
        Err(r) => a.ret(r as u64),
    }
}

unsafe fn close(c: *mut Cpu) {
    let h = (*c).x[0] as u32;
    if !ipc::close(h) {
        crate::log!("[RED] sf: CloseClientSessionHandle on {h:#x}, not a host session");
    }
}

unsafe fn ptr_buf(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    if !ipc::is_session(a.w(1)) {
        a.ret(crate::hle::RES_FAIL);
        return;
    }
    w16(a.x(0), 0);
    a.ret(0);
}

unsafe fn to_domain(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    match ipc::convert_to_domain(a.w(1)) {
        Some(id) => {
            w32(a.x(0), id);
            a.ret(0);
        }
        None => a.ret(crate::hle::RES_FAIL),
    }
}

unsafe fn dup(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    match ipc::duplicate(a.w(1)) {
        Some(h) => {
            w32(a.x(0), h);
            a.ret(0);
        }
        None => a.ret(crate::hle::RES_FAIL),
    }
}

/// DuplicateSessionEx(out, session, tag): a clone, the tag unused.
unsafe fn dup_ex(c: *mut Cpu) {
    dup(c)
}

struct Camera {
    events: Vec<crate::kobj::Event>,
}

impl ipc::Service for Camera {
    fn name(&self) -> String {
        "camera".into()
    }
    fn call(&mut self, ctx: &mut ipc::Ctx) -> u32 {
        match ctx.cmd() {
            3 | 4 => 0,
            21 => {
                let e = crate::kobj::Event::new();
                ctx.copy_handle(e.readable);
                self.events.push(e);
                0
            }
            n => {
                if n != 29 {
                    crate::log_once!(
                        "camera cmd",
                        "sf: camera command {n} answered 0x7c4 (no camera device)"
                    );
                }
                0x7c4
            }
        }
    }
}

/// CreateCameraContext(UniqueNativeHandle *out): a host session with no camera.
unsafe fn camera_context(c: *mut Cpu) {
    let out = (*c).x[0];
    let h = ipc::open(Box::new(Camera { events: vec![] }));
    w32(out, h);
    crate::log!("sf: camera context -> host session {h:#x} (no camera)");
    (*c).x[0] = 0;
}

unsafe fn send(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    let (h, m, n) = (a.w(0), a.x(1), a.x(2));
    let msg = crate::guest::bytes_mut(m, n.min(0x10000));
    let r = ipc::process(h, msg);
    a.ret(r as u64);
}

pub fn register(r: &mut Registry) {
    for name in [
        "olsc:u", "caps:su", "caps:u", "caps:a", "caps:c", "caps:sc", "caps:ss", "notif:a",
        "mnpp:app",
    ] {
        ipc::register(name, || Box::new(Silent("offline")));
    }
    r.force(
        "_ZN2nn2sf4hipc31InitializeHipcServiceResolutionEv",
        "service name resolution (sm): the host resolves its own services",
        init_res,
    );
    r.force(
        "_ZN2nn2sf4hipc29FinalizeHipcServiceResolutionEv",
        "service name resolution (sm)",
        fin_res,
    );
    r.force(
        "_ZN2nn2sf4hipc20ConnectToHipcServiceEPNS_3svc6HandleEPKc",
        "connect through sm: a host session",
        connect,
    );
    r.force(
        "_ZN2nn2sf4hipc24CloseClientSessionHandleENS_3svc6HandleE",
        "close a host session",
        close,
    );
    r.force(
        "_ZN2nn2sf4hipc6client22QueryPointerBufferSizeEPtNS_3svc6HandleE",
        "pointer buffer size of a host session: 0",
        ptr_buf,
    );
    r.force("_ZN2nn2sf4hipc6client22ConvertSessionToDomainEPNS0_4cmif18CmifDomainObjectIdENS_3svc6HandleE", "a host session as a domain", to_domain);
    r.force(
        "_ZN2nn2sf4hipc6client16DuplicateSessionEPNS_3svc6HandleES4_",
        "clone a host session",
        dup,
    );
    r.force(
        "_ZN2nn2sf4hipc15SendSyncRequestENS_3svc6HandleEPvm",
        "a request on a host session",
        send,
    );
    r.force(
        "_ZN2nn2sf4hipc6client18DuplicateSessionExEPNS_3svc6HandleES4_j",
        "clone a host session (the tag unused)",
        dup_ex,
    );
    r.force(
        "_ZN2nn6camera3sdk19CreateCameraContextEPNS_5gsdrv18UniqueNativeHandleE",
        "a camera session with no camera",
        camera_context,
    );
}
