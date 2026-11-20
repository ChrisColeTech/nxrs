//! Offline answers with a little state: the Switch 2 libraries for streaming and chatting with other players
//! (streamplay: no guest ever connects; chat: no GameChat, the camera storages hold blank native images), the
//! shop's purchase queue and the friends notifications (events that never fire), play statistics of other games
//! (never played), the IR camera handle, nifm's network request (on hold for one query, then unavailable) and the
//! offline HTML pages of nn::web, which the host answers with the stand-in of a run without a web view: the page
//! loads at once, sends one empty content message and closes when it appears (exit reason 0, no last URL).
//! The constant answers of the same libraries are rows of `simple_table.rs`.

use crate::guest::{r32, r64, w32, w64, Args};
use crate::hle::{Registry, RES_FAIL, RES_OK};
use nxrt::cpu::Cpu;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

macro_rules! ofn {
    ($name:ident, |$c:ident, $a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name($c: *mut Cpu) {
            let mut $a = Args(&mut *$c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*$c).x[0] = r;
        }
    };
}

/// A SystemEventType of the host's (guest memory), bound to a kernel event on first use; it never fires.
unsafe fn quiet_event(c: *mut Cpu, slot: &OnceLock<u64>) -> u64 {
    let ev = *slot.get_or_init(|| nxrt::kernel::get().space.kernel_alloc(0x100, 16));
    crate::services::os::bind_system_event(c, ev, false);
    ev
}

// ---- streamplay: no guest

static STPL_STYLES: AtomicU32 = AtomicU32::new(0);
static STPL_HOLD: AtomicU32 = AtomicU32::new(0);
static STPL_EV: OnceLock<u64> = OnceLock::new();
ofn!(stpl_set_styles, |c, a| {
    STPL_STYLES.store(a.w(0), Ordering::Relaxed);
    0
});
ofn!(stpl_styles, |c, a| { STPL_STYLES.load(Ordering::Relaxed) as u64 });
ofn!(stpl_set_hold, |c, a| {
    STPL_HOLD.store(a.w(0), Ordering::Relaxed);
    0
});
ofn!(stpl_hold, |c, a| { STPL_HOLD.load(Ordering::Relaxed) as u64 });
ofn!(stpl_event, |c, a| { quiet_event(c, &STPL_EV) });

// ---- the shop's purchased-item queue and the friends notifications: events that never fire

static EC_EV: OnceLock<u64> = OnceLock::new();
static FR_NOTIFY_EV: OnceLock<u64> = OnceLock::new();
static FR_INVITE_EV: OnceLock<u64> = OnceLock::new();
ofn!(ec_event, |c, a| { quiet_event(c, &EC_EV) });
ofn!(fr_notify_event, |c, a| { quiet_event(c, &FR_NOTIFY_EV) });
ofn!(fr_invite_event, |c, a| { quiet_event(c, &FR_INVITE_EV) });

// ---- chat: nobody chatting; camera storages of blank native images

/// The chatting user: nobody (a zero Uid, returned in x0 / x1).
unsafe fn chat_uid(c: *mut Cpu) {
    (*c).x[0] = 0;
    (*c).x[1] = 0;
}

/// LocalCameraVideoType / RemoteCameraVideoType (0xe0 bytes, the SDK zeroes it first): the host's
/// {u32 magic, u32 count, u32 image[4]}, each image a blank native image of the storage's size.
const CHAT_MAGIC: u32 = 0x6d43_6d44; // "DmCm"
const CHAT_TYPE: u64 = 0xe0;
ofn!(chat_storage_init, |c, a| {
    let (t, w, h, n) = (a.x(0), a.i(1), a.i(2), a.i(3));
    crate::guest::zero(t, CHAT_TYPE);
    if !(32..=1920).contains(&w) || !(32..=1080).contains(&h) || !(1..=4).contains(&n) {
        crate::log!("[RED] chat: camera video storage {w}x{h} x{n} is out of the SDK's range");
        return 0;
    }
    w32(t, CHAT_MAGIC);
    w32(t + 4, n as u32);
    for i in 0..n as u64 {
        w32(t + 8 + 4 * i, crate::services::codec_video::native_image_blank(w as u32, h as u32));
    }
    crate::log_once!("chat cam", "chat: camera video storage {w}x{h} x{n}: blank images (the host has no camera and no chat members)");
    0
});
ofn!(chat_storage_fini, |c, a| {
    let t = a.x(0);
    if r32(t) == CHAT_MAGIC {
        for i in 0..r32(t + 4).min(4) as u64 {
            crate::services::codec_video::native_image_blank_free(r32(t + 8 + 4 * i));
        }
    }
    crate::guest::zero(t, CHAT_TYPE);
    0
});
ofn!(chat_storage_image, |c, a| {
    let (t, i) = (a.x(0), a.i(1));
    if r32(t) == CHAT_MAGIC && i >= 0 && (i as u32) < r32(t + 4).min(4) {
        r32(t + 8 + 4 * i as u64) as u64
    } else {
        0
    }
});

// ---- play statistics, IR camera handle

// QueryApplicationPlayStatistics(ApplicationPlayStatistics *out, uid, const ApplicationId *ids, int n): each
// {application id, play time 0, launches 0}: the host keeps no play log of other games.
ofn!(pdm_stats, |c, a| {
    let (out, ids, n) = (a.x(0), a.x(2), a.i(3));
    if out != 0 && ids != 0 {
        for i in 0..n.max(0) as u64 {
            w64(out + 0x18 * i, r64(ids + 8 * i));
            w64(out + 0x18 * i + 8, 0);
            w64(out + 0x18 * i + 16, 0);
        }
    }
    RES_OK
});
// GetIrCameraHandle(const NpadId &): the player number in the low byte.
ofn!(irs_handle, |c, a| { (r32(a.x(0)) & 0xff) as u64 });

// ---- nifm's network request: on hold for the first query after its submission, then settled (unavailable)

static NIFM_HOLD: AtomicI32 = AtomicI32::new(0);
ofn!(nifm_submit, |c, a| {
    NIFM_HOLD.store(1, Ordering::Relaxed);
    0
});
ofn!(nifm_cancel, |c, a| {
    NIFM_HOLD.store(0, Ordering::Relaxed);
    0
});
// Whether the request is on hold: true once after its submission.
fn nifm_hold_take() -> bool {
    let mut h = NIFM_HOLD.load(Ordering::Relaxed);
    while h > 0 {
        match NIFM_HOLD.compare_exchange_weak(h, h - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(x) => h = x,
        }
    }
    false
}
ofn!(nifm_on_hold, |c, a| { nifm_hold_take() as u64 });

// ---- nn::web offline HTML pages: the stand-in (no web view on the host)

const TLV_DOCUMENT_PATH: u16 = 6;
const URL_SIZE: u64 = 0x1000;

/// One page at a time. state: 0 none, 1 started (hidden), 2 shown, 3 ended (the exit event fired).
#[derive(Default)]
struct Web {
    state: u32,
    reason: u64,
    msgs: VecDeque<Vec<u8>>,
}
static WEB: Mutex<Web> = Mutex::new(Web { state: 0, reason: 0, msgs: VecDeque::new() });
static WEB_CV: Condvar = Condvar::new();
static WEB_EV: OnceLock<u64> = OnceLock::new();

fn web_ev() -> u64 {
    *WEB_EV.get_or_init(|| nxrt::kernel::get().space.kernel_alloc(0x100, 16))
}

/// The document path of a ShowOfflineHtmlPageArg (u16 entry count, entries from +8: {u16 type, u16 size, u32} + data).
fn web_doc(arg: u64) -> String {
    if arg == 0 {
        return String::new();
    }
    let n = crate::guest::r16(arg) as u64;
    let mut o = 8u64;
    for _ in 0..n {
        if o + 8 > 0x2000 {
            break;
        }
        let (t, sz) = (crate::guest::r16(arg + o), crate::guest::r16(arg + o + 2) as u64);
        if o + 8 + sz > 0x2000 {
            break;
        }
        if t == TLV_DOCUMENT_PATH {
            return crate::guest::cstr(arg + o + 8, sz as usize);
        }
        o += 8 + sz;
    }
    String::new()
}

/// The page ends: the exit event fires and the waiters wake.
fn web_end(w: &mut Web, reason: u64) {
    if w.state == 0 || w.state == 3 {
        return;
    }
    w.reason = reason;
    w.state = 3;
    crate::services::os::signal_bound(web_ev());
    WEB_CV.notify_all();
}

unsafe fn web_start(c: *mut Cpu, arg: u64) {
    let ev = web_ev();
    crate::services::os::bind_system_event(c, ev, false);
    crate::services::os::clear_system_event(ev);
    let doc = web_doc(arg);
    let mut w = WEB.lock().unwrap();
    w.msgs.clear();
    w.reason = 0;
    w.state = 1;
    // the stand-in's ready message: a game waits for any message before Appear
    w.msgs.push_back(vec![]);
    crate::log!("web: offline page {doc}: the host has no web view; the stand-in answers (it loads at once and closes when it appears)");
    crate::services::notice::offline_page(&doc);
}

/// The ended page's result into the OfflineHtmlPageReturnValue {u64 exit reason, last URL[0x1000], u64 URL size};
/// the session is over.
fn web_return(rv: u64) {
    let mut w = WEB.lock().unwrap();
    if rv != 0 {
        w64(rv, w.reason);
        crate::guest::zero(rv + 8, URL_SIZE);
        w64(rv + 8 + URL_SIZE, 0);
    }
    w.msgs.clear();
    w.state = 0;
    drop(w);
    crate::services::os::clear_system_event(web_ev());
}

fn web_wait_end() {
    crate::sched::without_core(|| {
        let mut w = WEB.lock().unwrap();
        while w.state == 1 || w.state == 2 {
            w = WEB_CV.wait(w).unwrap();
        }
    });
}

ofn!(webs_nothing, |c, a| { 0 });
ofn!(webs_work_size, |c, a| { 0xd0 });
ofn!(webs_start, |c, a| {
    let (ev_out, arg) = (a.x(1), a.x(2));
    web_start(c, arg);
    if ev_out != 0 {
        w64(ev_out, web_ev());
    }
    RES_OK
});
ofn!(webs_appear, |c, a| {
    let mut w = WEB.lock().unwrap();
    if w.state == 1 || w.state == 2 {
        crate::log_once!("web appear", "web::OfflineWebSession::Appear: the stand-in closes at once (no page shown)");
        w.state = 2;
        web_end(&mut w, 0);
    }
    1
});
ofn!(webs_request_exit, |c, a| {
    crate::log!("web::OfflineWebSession::RequestExit");
    web_end(&mut WEB.lock().unwrap(), 2);
    0
});
ofn!(webs_wait, |c, a| {
    web_wait_end();
    web_return(a.x(1));
    RES_OK
});
ofn!(webs_fini, |c, a| {
    let open = matches!(WEB.lock().unwrap().state, 1 | 2);
    if open {
        web_end(&mut WEB.lock().unwrap(), 2);
        web_return(0);
    }
    0
});
// TrySendContentMessage: no page reads it (the stand-in), taken.
ofn!(webs_send, |c, a| { 1 });
// TryReceiveContentMessage(size_t *out, char *buf, size_t n): the oldest message from the page, without its zero.
ofn!(webs_recv, |c, a| {
    let (out, buf, n) = (a.x(1), a.x(2), a.x(3));
    let Some(m) = WEB.lock().unwrap().msgs.pop_front() else { return 0 };
    let k = (m.len() as u64).min(n.saturating_sub(1));
    if buf != 0 && n != 0 {
        crate::guest::write_bytes(buf, &m[..k as usize]);
        crate::guest::w8(buf + k, 0);
    }
    crate::guest::out(out, k);
    1
});
ofn!(webs_needs_wait, |c, a| { (WEB.lock().unwrap().state != 0) as u64 });
// ShowOfflineHtmlPage(rv, arg): the synchronous form fails without a page shown.
ofn!(web_show_offline, |c, a| {
    web_start(c, a.x(1));
    web_return(0);
    crate::log!("web::ShowOfflineHtmlPage: no page shown (the host has no web view)");
    RES_FAIL
});

pub fn register(r: &mut Registry) {
    for (n, w, f) in [
        ("_ZN2nn10streamplay29SetSupportedGuestNpadStyleSetENS_4util10BitFlagSetILi32ENS_3hid12NpadStyleTagEEE", "streamplay: kept for the getter", stpl_set_styles as unsafe fn(*mut Cpu)),
        ("_ZN2nn10streamplay29GetSupportedGuestNpadStyleSetEv", "streamplay: what the game set", stpl_styles),
        ("_ZN2nn10streamplay23SetGuestNpadJoyHoldTypeENS_3hid15NpadJoyHoldTypeE", "streamplay: kept for the getter", stpl_set_hold),
        ("_ZN2nn10streamplay23GetGuestNpadJoyHoldTypeEv", "streamplay: what the game set", stpl_hold),
        ("_ZN2nn10streamplay30GetConnectionStateChangedEventEv", "streamplay: no guest connects, the event never fires", stpl_event),
        ("_ZN2nn2ec17GetPurchasedEventEv", "no offline purchases: the event never fires", ec_event),
        ("_ZN2nn7friends17NotificationQueue14GetSystemEventEv", "friends notifications: the event never fires", fr_notify_event),
        ("_ZN2nn7friends36GetFriendInvitationNotificationEventEv", "friend invitations: the event never fires", fr_invite_event),
        ("_ZN2nn4chat14GetChattingUidEv", "GameChat: nobody chatting", chat_uid),
        ("_ZN2nn4chat33InitializeLocalCameraVideoStorageEPNS0_20LocalCameraVideoTypeEiiiPvm", "GameChat camera storage: blank images", chat_storage_init),
        ("_ZN2nn4chat34InitializeRemoteCameraVideoStorageEPNS0_21RemoteCameraVideoTypeEiiiPvm", "GameChat camera storage: blank images", chat_storage_init),
        ("_ZN2nn4chat31FinalizeLocalCameraVideoStorageEPNS0_20LocalCameraVideoTypeE", "GameChat camera storage: images released", chat_storage_fini),
        ("_ZN2nn4chat32FinalizeRemoteCameraVideoStorageEPNS0_21RemoteCameraVideoTypeE", "GameChat camera storage: images released", chat_storage_fini),
        ("_ZN2nn4chat25GetLocalCameraImageHandleEPKNS0_20LocalCameraVideoTypeEi", "GameChat camera storage: its blank image", chat_storage_image),
        ("_ZN2nn4chat26GetRemoteCameraImageHandleEPKNS0_21RemoteCameraVideoTypeEi", "GameChat camera storage: its blank image", chat_storage_image),
        ("_ZN2nn3pdm30QueryApplicationPlayStatisticsEPNS0_25ApplicationPlayStatisticsERKNS_7account3UidEPKNS_13ApplicationIdEi", "never played (no play log)", pdm_stats),
        ("_ZN2nn8irsensor17GetIrCameraHandleERKj", "the IR camera handle of a controller", irs_handle),
        ("_ZN2nn4nifm20SubmitNetworkRequestEv", "offline: on hold for one query, then unavailable", nifm_submit),
        ("_ZN2nn4nifm20CancelNetworkRequestEv", "offline: the request dropped", nifm_cancel),
        ("_ZN2nn4nifm22IsNetworkRequestOnHoldEv", "offline: on hold for one query", nifm_on_hold),
        ("_ZN2nn3web17OfflineWebSessionC1Ev", "offline HTML session: the stand-in", webs_nothing),
        ("_ZN2nn3web17OfflineWebSessionD1Ev", "offline HTML session: the stand-in", webs_nothing),
        ("_ZN2nn3web17OfflineWebSession10InitializeEPvm", "offline HTML session: the work buffer is not used", webs_nothing),
        ("_ZN2nn3web17OfflineWebSession17GetWorkBufferSizeEv", "offline HTML session: the SDK's work size", webs_work_size),
        ("_ZN2nn3web17OfflineWebSession5StartEPPNS_2os15SystemEventTypeERKNS0_22ShowOfflineHtmlPageArgE", "offline HTML page: the stand-in", webs_start),
        ("_ZN2nn3web17OfflineWebSession6AppearEv", "offline HTML page: the stand-in closes when it appears", webs_appear),
        ("_ZN2nn3web17OfflineWebSession11RequestExitEv", "offline HTML page: closed at the game's request", webs_request_exit),
        ("_ZN2nn3web17OfflineWebSession11WaitForExitEPNS0_26OfflineHtmlPageReturnValueE", "offline HTML page: its exit reason", webs_wait),
        ("_ZN2nn3web17OfflineWebSession8FinalizeEv", "offline HTML session: a page still open is closed", webs_fini),
        ("_ZN2nn3web17OfflineWebSession21TrySendContentMessageEPKcm", "offline HTML page: the stand-in reads nothing", webs_send),
        ("_ZN2nn3web17OfflineWebSession24TryReceiveContentMessageEPmPcm", "offline HTML page: its messages", webs_recv),
        ("_ZNK2nn3web17OfflineWebSession16NeedsWaitForExitEv", "offline HTML page: started and not waited for", webs_needs_wait),
        ("_ZN2nn3web19ShowOfflineHtmlPageEPNS0_26OfflineHtmlPageReturnValueERKNS0_22ShowOfflineHtmlPageArgE", "offline HTML page: none shown", web_show_offline),
    ] {
        r.force(n, w, f);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nifm_request_holds_once() {
        use std::sync::atomic::Ordering;
        super::NIFM_HOLD.store(1, Ordering::Relaxed);
        let q = super::nifm_hold_take;
        assert!(q());
        assert!(!q());
    }
}
