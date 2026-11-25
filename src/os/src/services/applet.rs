//! The applet manager's IPC path (`appletOE` / `appletAE`) for library applets an SDK starts itself (nn::la over
//! nn::applet), where no takeover by name (`mii.rs` AppendMii, `swkbd.rs`, the controller applet in `hid.rs`)
//! catches it first. The proxy hands out the library applet creator; an
//! applet "runs" and is completed at once, its output is what a player closing it would give:
//!
//! * MiiEdit (0x12): MiiEditAppletOutput { u32 result = 1 (cancelled); s32 index = -1 } in 0x100 bytes. Zeros read as
//!   "a Mii was made at index 0" of an empty database (Mario Kart 8 Deluxe then took a null Mii and crashed in
//!   nn::mii::CreateId::IsValid);
//! * PlayerSelect (0x10): UiReturnArg { u64 result 0; Uid } with the PC's player (option `pc_user`), or the profile
//!   the player picks with `ask_user=1` (result 2 and no Uid when cancelled);
//! * any other applet: its output size of zeros (0x10, or 0x1010 for the web applets' WebCommonReturnValue).
//!
//! IStorage / IStorageAccessor carry the data both ways. The proxy's other interfaces are served by the takeovers by
//! name (oe, applet), so here they answer with the framework's gap line.

use crate::ipc::{Ctx, Service};
use crate::kobj::Event;
use std::sync::{Arc, Mutex};

const MII_EDIT: u32 = 0x12;
const PLAYER_SELECT: u32 = 0x10;
/// nn::am's result for "no data" (module 128, 2).
const R_NO_DATA: u32 = 128 | 2 << 9;
/// nn::am's result for an access outside a storage (module 128, 503).
const R_OUT_OF_RANGE: u32 = 128 | 503 << 9;

/// What a closed applet hands back (the player cancelled, or picked the console user).
pub fn applet_output(id: u32) -> Vec<u8> {
    match id {
        MII_EDIT => {
            let mut r = vec![0u8; 0x100];
            r[0] = 1;
            r[4..8].copy_from_slice(&(-1i32).to_le_bytes());
            r
        }
        PLAYER_SELECT => {
            let mut r = vec![0u8; 0x18];
            match crate::services::account::choose_user() {
                Some(u) => {
                    r[8..16].copy_from_slice(&(crate::services::account::UID0 + u).to_le_bytes());
                    r[16..24].copy_from_slice(&crate::services::account::UID1.to_le_bytes());
                }
                None => r[0] = 2, // cancelled
            }
            r
        }
        0x13 | 0x14 | 0x17 | 0x18 | 0x19 => vec![0u8; 0x1010],
        _ => vec![0u8; 0x10],
    }
}

struct Proxy(&'static str);
struct AppProxy;
struct Creator;
struct Accessor {
    id: u32,
    popped: bool,
    pushed: usize,
}
struct Storage(Arc<Mutex<Vec<u8>>>);
struct StorageAccessor(Arc<Mutex<Vec<u8>>>);
/// A proxy interface the takeovers by name serve: every command is a gap line.
struct Unserved(&'static str);

impl Service for Proxy {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // OpenApplicationProxy (10: Switch 2), OpenLibraryAppletProxy (appletAE 200 / 201)
            0 | 10 | 200 | 201 => {
                ctx.object(AppProxy);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

impl Service for AppProxy {
    fn name(&self) -> String {
        "IApplicationProxy".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        let name = match ctx.cmd() {
            11 => {
                ctx.object(Creator);
                return 0;
            }
            0 => "ICommonStateGetter",
            1 => "ISelfController",
            2 => "IWindowController",
            3 => "IAudioController",
            4 => "IDisplayController",
            10 => "IProcessWindingController",
            20 => "IApplicationFunctions",
            1000 => "IDebugFunctions",
            _ => return ctx.unimplemented(),
        };
        ctx.object(Unserved(name));
        0
    }
}

impl Service for Unserved {
    fn name(&self) -> String {
        self.0.into()
    }
}

impl Service for Creator {
    fn name(&self) -> String {
        "ILibraryAppletCreator".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // CreateLibraryApplet(applet id, mode); 3: SDK 20's form with the same first arguments
            0 | 3 => {
                let (id, mode) = (ctx.u32(0), ctx.u32(4));
                crate::log!("applet: library applet {id:#04x} created over IPC (mode {mode}): completed at once, no applet UI");
                ctx.object(Accessor {
                    id,
                    popped: false,
                    pushed: 0,
                });
                0
            }
            // CreateStorage(size), CreateTransferMemoryStorage(.., size)
            10 | 11 => {
                let size = if ctx.cmd() == 10 {
                    ctx.u64(0)
                } else {
                    ctx.u64(8)
                };
                ctx.object(Storage(Arc::new(Mutex::new(vec![
                    0;
                    size.min(64 << 20)
                        as usize
                ]))));
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

impl Service for Accessor {
    fn name(&self) -> String {
        format!("ILibraryAppletAccessor {:#04x}", self.id)
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // GetAppletStateChangedEvent: completed
            0 => {
                ctx.copy_handle(Event::signalled().readable);
                0
            }
            // IsCompleted
            1 => {
                ctx.push_u8(1);
                0
            }
            // Start: the applet opens and closes at once (the focus goes to it and back)
            10 => {
                crate::services::oe::applet_enter(&format!("{:#04x}", self.id));
                crate::services::oe::applet_leave();
                0
            }
            // RequestExit, Terminate, GetResult (success)
            20 | 25 | 30 => 0,
            // PushInData / PushInteractiveInData: the applet's input is not read
            100 | 103 => {
                self.pushed += 1;
                0
            }
            // PopOutData: the applet's output once, then no data
            101 => {
                if self.popped {
                    return R_NO_DATA;
                }
                self.popped = true;
                let out = applet_output(self.id);
                crate::log!(
                    "applet: library applet {:#04x} closed: {} bytes of output{}",
                    self.id,
                    out.len(),
                    if self.id == MII_EDIT {
                        " (MiiEdit: cancelled)"
                    } else {
                        ""
                    }
                );
                ctx.object(Storage(Arc::new(Mutex::new(out))));
                0
            }
            // PopInteractiveOutData: none (these applets do not talk back)
            104 => R_NO_DATA,
            // GetPopOutDataEvent: there is output
            105 => {
                ctx.copy_handle(Event::signalled().readable);
                0
            }
            // GetPopInteractiveOutDataEvent: never
            106 => {
                ctx.copy_handle(Event::new().readable);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

impl Service for Storage {
    fn name(&self) -> String {
        "IStorage".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // Open / OpenTransferStorage
            0 | 1 => {
                ctx.object(StorageAccessor(self.0.clone()));
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

impl Service for StorageAccessor {
    fn name(&self) -> String {
        "IStorageAccessor".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        let off = ctx.u64(0);
        match ctx.cmd() {
            0 => {
                let n = self.0.lock().unwrap().len() as u64;
                ctx.push_u64(n);
                0
            }
            // Write(offset; A buffer)
            10 => {
                let data = ctx.read_in(0);
                let mut d = self.0.lock().unwrap();
                if off > d.len() as u64 || data.len() as u64 > d.len() as u64 - off {
                    return R_OUT_OF_RANGE;
                }
                d[off as usize..off as usize + data.len()].copy_from_slice(&data);
                0
            }
            // Read(offset; B buffer)
            11 => {
                let d = self.0.lock().unwrap();
                if off > d.len() as u64 {
                    return R_OUT_OF_RANGE;
                }
                let part = d[off as usize..].to_vec();
                drop(d);
                ctx.write_out(0, &part);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

pub fn register() {
    crate::ipc::register("appletOE", || Box::new(Proxy("appletOE")));
    crate::ipc::register("appletAE", || Box::new(Proxy("appletAE")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::hipc::{self, Buf, Builder, T_REQUEST};

    fn send(h: u32, cmd: u32, data: Vec<u8>, a: Vec<Buf>, b: Vec<Buf>) -> hipc::Response {
        let mut m = Builder {
            ty: T_REQUEST,
            cmd,
            data,
            a,
            b,
            ..Default::default()
        }
        .build();
        crate::ipc::process(h, &mut m);
        hipc::parse_response(&m, false).unwrap()
    }

    /// The MiiEdit applet over IPC, as an SDK's nn::la drives it: appletOE's proxy, the creator, the applet (its
    /// input pushed, started, completed), its output popped and read: the reply (cancelled, index -1, 0x100
    /// bytes), once.
    #[test]
    fn mii_edit_applet_over_ipc_answers_cancelled() {
        register();
        let oe = crate::ipc::connect("appletOE").unwrap();
        let r = send(oe, 0, 0u64.to_le_bytes().to_vec(), vec![], vec![]);
        let proxy = r.moved[0];
        let creator = send(proxy, 11, vec![], vec![], vec![]).moved[0];
        // the input: a 0x100-byte MiiEditAppletInput written into a storage
        let st = send(creator, 10, 0x100u64.to_le_bytes().to_vec(), vec![], vec![]).moved[0];
        let acc = send(st, 0, vec![], vec![], vec![]).moved[0];
        let input = [3u8; 0x100];
        let r = send(
            acc,
            10,
            0u64.to_le_bytes().to_vec(),
            vec![Buf {
                addr: input.as_ptr() as u64,
                size: 0x100,
                tag: 0,
            }],
            vec![],
        );
        assert_eq!(r.result, 0);
        let a = send(
            creator,
            0,
            [MII_EDIT.to_le_bytes(), 0u32.to_le_bytes()].concat(),
            vec![],
            vec![],
        )
        .moved[0];
        assert_eq!(send(a, 100, vec![], vec![], vec![]).result, 0, "PushInData");
        assert_eq!(send(a, 10, vec![], vec![], vec![]).result, 0, "Start");
        assert_eq!(send(a, 1, vec![], vec![], vec![]).data[0], 1, "IsCompleted");
        assert_eq!(send(a, 30, vec![], vec![], vec![]).result, 0, "GetResult");
        let r = send(a, 101, vec![], vec![], vec![]);
        assert_eq!((r.result, r.moved.len()), (0, 1), "PopOutData");
        let out_acc = send(r.moved[0], 0, vec![], vec![], vec![]).moved[0];
        let size = u64::from_le_bytes(
            send(out_acc, 0, vec![], vec![], vec![]).data[..8]
                .try_into()
                .unwrap(),
        );
        assert_eq!(size, 0x100);
        let mut got = [0u8; 0x100];
        let r = send(
            out_acc,
            11,
            0u64.to_le_bytes().to_vec(),
            vec![],
            vec![Buf {
                addr: got.as_mut_ptr() as u64,
                size: 0x100,
                tag: 0,
            }],
        );
        assert_eq!(r.result, 0);
        // u8 r[0x100] = {0}; r[0] = 1; r[4..8] = 0xFF
        let mut want = [0u8; 0x100];
        want[0] = 1;
        want[4..8].fill(0xff);
        assert_eq!(got, want);
        assert_eq!(
            send(a, 101, vec![], vec![], vec![]).result,
            R_NO_DATA,
            "the output pops once"
        );
        // a read past the end
        assert_eq!(
            send(out_acc, 11, 0x101u64.to_le_bytes().to_vec(), vec![], vec![]).result,
            R_OUT_OF_RANGE
        );
        // PlayerSelect gives the PC's player (option `pc_user`; account's pc_user_option test changes it in parallel,
        // so read the slot rather than assuming user 1)
        let p = applet_output(PLAYER_SELECT);
        assert_eq!(
            (p.len(), u64::from_le_bytes(p[..8].try_into().unwrap())),
            (0x18, 0)
        );
        let uid = u64::from_le_bytes(p[8..16].try_into().unwrap());
        assert!((crate::services::account::UID0
            ..crate::services::account::UID0 + crate::services::account::USERS)
            .contains(&uid));
    }
}
