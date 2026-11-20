//! nn::account: local profiles only. One registered user (more slots for additional local users, as nxnative
//! keeps them: a `user<n>.registered` marker in the save folder), sessions per open, the launch user preselected;
//! online accounts are unavailable. The PC's player is the option `pc_user` (1..8, default 1), named `pc_user_name`
//! (`NXOS_NICK` overrides): opened at launch, answered by the user picker, always registered. With `ask_user=1` and
//! more than one registered profile, the game's user selector opens the host's profile picker instead (nxnative
//! `hle_account.c` select_user; `ui::pick_user`); cancelling it answers the selector's "cancelled".

use crate::guest::{r64, w64, w8, Args};
use crate::hle::{Registry, RES_FAIL, RES_OK};
use nxrt::cpu::Cpu;
use std::sync::Mutex;

pub const USERS: u64 = 8;
pub const UID0: u64 = 0x7862_6c00_0000_0001;
pub const UID1: u64 = 0x5357_4e41_5449_5645;

/// The user slot of a Uid at a guest address (None: not a registered user).
pub fn user_of(uid: u64) -> Option<u64> {
    if uid == 0 {
        return None;
    }
    let (v0, v1) = (r64(uid), r64(uid + 8));
    (v1 == UID1 && (UID0..UID0 + USERS).contains(&v0) && registered() & (1 << (v0 - UID0)) != 0).then(|| v0 - UID0)
}

/// The PC's player's slot (option `pc_user`, read live).
pub fn pc_user() -> u64 {
    (nxconfig::int("pc_user").clamp(1, USERS as i64) - 1) as u64
}

/// A bit per registered user: the PC's player always, others when their marker exists in the save folder.
pub fn registered() -> u64 {
    let mut m = 1 << pc_user();
    for i in 1..USERS {
        if crate::services::fs::save_dir().join(format!("user{}.registered", i + 1)).exists() {
            m |= 1 << i;
        }
    }
    m
}

fn nickname(i: u64) -> String {
    if i == pc_user() {
        std::env::var("NXOS_NICK").unwrap_or_else(|_| match nxconfig::string("pc_user_name") {
            n if n.trim().is_empty() => "Player".into(),
            n => n.trim().chars().take(32).collect(),
        })
    } else {
        format!("Player {}", i + 1)
    }
}

struct Sessions {
    open: Vec<(u64, u64)>, // (token, user)
    token: u64,
    last: u64,
}
static S: Mutex<Sessions> = Mutex::new(Sessions { open: Vec::new(), token: 0x5553_4552_0000_0000, last: u64::MAX });

fn put_uid(a: u64, user: u64) {
    w64(a, UID0 + user);
    w64(a + 8, UID1);
}

fn open_user(h: u64, user: u64) -> u64 {
    let mut s = S.lock().unwrap();
    s.token += 1;
    let t = s.token;
    s.open.push((t, user));
    s.last = user;
    put_uid(h, user);
    w64(h + 16, t);
    RES_OK
}

fn list(out: u64, max: i64, mask: u64) -> u64 {
    let mut n = 0;
    for i in 0..USERS {
        if mask & (1 << i) != 0 && (n as i64) < max {
            if out != 0 {
                put_uid(out + 16 * n, i);
            }
            n += 1;
        }
    }
    n
}

macro_rules! accfn {
    ($name:ident, |$a:ident| $body:block) => {
        #[allow(unused_mut, unused_variables)]
        unsafe fn $name(c: *mut Cpu) {
            let mut $a = Args(&mut *c);
            #[allow(clippy::redundant_closure_call)]
            let r: u64 = (|| -> u64 { $body })();
            (*c).x[0] = r;
        }
    };
}

accfn!(init, |a| { 0 });
accfn!(list_open, |a| {
    let mask = S.lock().unwrap().open.iter().fold(0u64, |m, (_, u)| m | 1 << u);
    let n = list(a.x(1), a.i(2) as i64, mask);
    crate::guest::w32(a.x(0), n as u32);
    RES_OK
});
accfn!(list_all, |a| {
    let n = list(a.x(1), a.i(2) as i64, registered());
    crate::guest::w32(a.x(0), n as u32);
    RES_OK
});
accfn!(user_count, |a| {
    crate::guest::w32(a.x(0), registered().count_ones());
    RES_OK
});
accfn!(user_exists, |a| {
    w8(a.x(0), user_of(a.x(1)).is_some() as u8);
    RES_OK
});
accfn!(pop_open, |a| {
    crate::guest::w32(a.x(0), 0);
    0
});
accfn!(push_open, |a| { 0 });
accfn!(try_open_pre, |a| { (open_user(a.x(0), pc_user()) == RES_OK) as u64 });
accfn!(open_pre, |a| { open_user(a.x(0), pc_user()) });
accfn!(open, |a| {
    match user_of(a.x(1)) {
        Some(u) => open_user(a.x(0), u),
        None => RES_FAIL,
    }
});
accfn!(close, |a| {
    let t = r64(a.x(0) + 16);
    S.lock().unwrap().open.retain(|(k, _)| *k != t);
    0
});
accfn!(last_opened, |a| {
    let u = match S.lock().unwrap().last {
        u64::MAX => pc_user(), // none opened yet: the PC's player
        u => u,
    };
    put_uid(a.x(0), u);
    RES_OK
});
accfn!(get_user_id, |a| {
    let h = a.x(1);
    w64(a.x(0), r64(h));
    w64(a.x(0) + 8, r64(h + 8));
    RES_OK
});
accfn!(nickname_fn, |a| {
    match user_of(a.x(1)) {
        Some(u) => {
            crate::guest::zero(a.x(0), 33);
            crate::guest::put_cstr(a.x(0), 33, &nickname(u));
            RES_OK
        }
        None => RES_FAIL,
    }
});
/// nn::account's ResultCancelledByUser (module 124, 1): the player closed the selector.
const RES_CANCELLED: u64 = 124 | (1 << 9);

/// The profile the user selector answers: `pc_user`, or with `ask_user=1` and more than one registered profile the
/// one the player picks (None: cancelled).
pub fn choose_user() -> Option<u64> {
    let users = registered();
    if !nxconfig::flag("ask_user") || users.count_ones() < 2 {
        crate::log_once!("acc select", "account: user selector answered with the PC's player (pc_user {})", pc_user() + 1);
        return Some(pc_user());
    }
    let choices: Vec<(u64, String)> = (0..USERS).filter(|i| users >> i & 1 != 0).map(|i| (i, nickname(i))).collect();
    crate::ui::pick_user(choices, pc_user())
}

accfn!(select, |a| {
    match choose_user() {
        Some(u) => {
            put_uid(a.x(0), u);
            RES_OK
        }
        None => RES_CANCELLED,
    }
});
accfn!(fail, |a| { RES_FAIL });
accfn!(fail_zero_n, |a| {
    crate::guest::out(a.x(0), 0u64);
    RES_FAIL
});
accfn!(nsa_avail, |a| {
    w8(a.x(0), 0);
    RES_OK
});
accfn!(ok, |a| { RES_OK });
accfn!(async_done, |a| {
    w8(a.x(1), 1);
    RES_OK
});
accfn!(self_ret, |a| { a.x(0) });
accfn!(profile_image, |a| {
    let (n, buf, sz) = (a.x(0), a.x(1), a.x(2));
    if buf != 0 {
        crate::guest::write_bytes(buf, &PROFILE_JPEG[..PROFILE_JPEG.len().min(sz as usize)]);
    }
    crate::guest::out(n, PROFILE_JPEG.len() as u64);
    RES_OK
});
accfn!(profile_image_size, |a| {
    crate::guest::out(a.x(0), PROFILE_JPEG.len() as u64);
    RES_OK
});

unsafe fn async_event(c: *mut Cpu) {
    let ev = (*c).x[1];
    crate::services::os::signal_system_event(c, ev);
    (*c).x[0] = RES_OK;
}

/// A plain 256x256 baseline JPEG (670 bytes): every console user has a profile image, and games decode it
/// without checking the result.
static PROFILE_JPEG: [u8; 670] = {
    let head: &[u8] = b"\xff\xd8\xff\xe0\x00\x10\x4a\x46\x49\x46\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xff\xdb\x00\x43\x00\x08\x06\x06\x07\x06\x05\x08\x07\x07\x07\x09\x09\x08\x0a\x0c\x14\x0d\x0c\x0b\x0b\x0c\x19\x12\x13\x0f\x14\x1d\x1a\x1f\x1e\x1d\x1a\x1c\x1c\x20\x24\x2e\x27\x20\x22\x2c\x23\x1c\x1c\x28\x37\x29\x2c\x30\x31\x34\x34\x34\x1f\x27\x39\x3d\x38\x32\x3c\x2e\x33\x34\x32\xff\xdb\x00\x43\x01\x09\x09\x09\x0c\x0b\x0c\x18\x0d\x0d\x18\x32\x21\x1c\x21\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\x32\xff\xc0\x00\x11\x08\x01\x00\x01\x00\x03\x01\x22\x00\x02\x11\x01\x03\x11\x01\xff\xc4\x00\x15\x00\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x05\xff\xc4\x00\x14\x10\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xc4\x00\x15\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x05\xff\xc4\x00\x14\x11\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xda\x00\x0c\x03\x01\x00\x02\x11\x03\x11\x00\x3f\x00\x8e\x02\xe2\x50";
    let mut out = [0u8; 670];
    let mut i = 0;
    while i < head.len() {
        out[i] = head[i];
        i += 1;
    }
    out[667] = 0x3f;
    out[668] = 0xff;
    out[669] = 0xd9;
    out
};

pub fn register(r: &mut Registry) {
    const W: &str = "the account service: local profiles";
    for (n, f) in [
        ("_ZN2nn7account10InitializeEv", init as unsafe fn(*mut Cpu)),
        ("_ZN2nn7account13ListOpenUsersEPiPNS0_3UidEi", list_open),
        ("_ZN2nn7account12ListAllUsersEPiPNS0_3UidEi", list_all),
        ("_ZN2nn7account18ListQualifiedUsersEPiPNS0_3UidEi", list_all),
        ("_ZN2nn7account12GetUserCountEPi", user_count),
        ("_ZN2nn7account16GetUserExistenceEPbRKNS0_3UidE", user_exists),
        ("_ZN2nn7account12PopOpenUsersEPiPNS0_10UserHandleEi", pop_open),
        ("_ZN2nn7account13PushOpenUsersEv", push_open),
        ("_ZN2nn7account22TryOpenPreselectedUserEPNS0_10UserHandleE", try_open_pre),
        ("_ZN2nn7account19OpenPreselectedUserEPNS0_10UserHandleE", open_pre),
        ("_ZN2nn7account8OpenUserEPNS0_10UserHandleERKNS0_3UidE", open),
        ("_ZN2nn7account9CloseUserERKNS0_10UserHandleE", close),
        ("_ZN2nn7account17GetLastOpenedUserEPNS0_3UidE", last_opened),
        ("_ZN2nn7account9GetUserIdEPNS0_3UidERKNS0_10UserHandleE", get_user_id),
        ("_ZN2nn7account11GetNicknameEPNS0_8NicknameERKNS0_3UidE", nickname_fn),
        ("_ZN2nn7account16ShowUserSelectorEPNS0_3UidERKNS0_21UserSelectionSettingsE", select),
        ("_ZN2nn7account15ShowUserCreatorEv", ok),
        ("_ZN2nn7account36EnsureNetworkServiceAccountAvailableERKNS0_10UserHandleE", fail),
        ("_ZN2nn7account26GetNetworkServiceAccountIdEPNS0_23NetworkServiceAccountIdERKNS0_10UserHandleE", fail),
        ("_ZN2nn7account37LoadNetworkServiceAccountIdTokenCacheEPmPcmRKNS0_10UserHandleE", fail_zero_n),
        ("_ZN2nn7account40ShowLicenseRequirementsForNetworkServiceERKNS0_10UserHandleE", fail),
        ("_ZN2nn7account29LoadCachedNintendoAccountInfoEPNS0_25CachedNintendoAccountInfoERKNS0_10UserHandleEPvm", fail),
        ("_ZN2nn7account44EnsureNetworkServiceAccountIdTokenCacheAsyncEPNS0_12AsyncContextERKNS0_10UserHandleE", fail),
        ("_ZN2nn7account12AsyncContext14GetSystemEventEPNS_2os11SystemEventE", async_event),
        ("_ZN2nn7account12AsyncContext9GetResultEv", fail),
        ("_ZN2nn7account12AsyncContext6CancelEv", ok),
        ("_ZN2nn7account12AsyncContext7HasDoneEPb", async_done),
        ("_ZN2nn7account12AsyncContextD1Ev", ok),
        ("_ZN2nn7account12AsyncContextaSEOS1_", self_ret),
        ("_ZN2nn7account32IsNetworkServiceAccountAvailableEPbRKNS0_10UserHandleE", nsa_avail),
        ("_ZN2nn7account16LoadProfileImageEPmPvmRKNS0_3UidE", profile_image),
        ("_ZN2nn7account16GetProfileImageSizeEPmRKNS0_3UidE", profile_image_size),
        ("_ZN2nn7account27StoreSaveDataThumbnailImageERKNS0_3UidEPKvm", ok),
        ("_ZN2nn7account28DeleteSaveDataThumbnailImageERKNS0_3UidE", ok),
    ] {
        r.force(n, W, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PC's player is an option: its slot is registered and named, the picker and the launch answer with it.
    /// pc_user_option changes the PC's user for a moment: the tests that read the users wait for it.
    static PC_USER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn pc_user_option() {
        let _g = PC_USER.lock().unwrap_or_else(|p| p.into_inner());
        assert!(nxconfig::apply_arg("--pc_user=3") && nxconfig::apply_arg("--pc_user_name=Chris"));
        assert_eq!(pc_user(), 2);
        assert_ne!(registered() & 0b100, 0);
        if std::env::var("NXOS_NICK").is_err() {
            assert_eq!(nickname(2), "Chris");
        }
        let out = crate::services::applet::applet_output(0x10);
        assert_eq!(u64::from_le_bytes(out[8..16].try_into().unwrap()), UID0 + 2);
        assert!(nxconfig::apply_arg("--pc_user=1") && nxconfig::apply_arg("--pc_user_name=Player"));
    }

    #[test]
    fn handles_and_uids() {
        let _g = PC_USER.lock().unwrap_or_else(|p| p.into_inner());
        let mut h = [0u64; 3];
        let a = h.as_mut_ptr() as u64;
        assert_eq!(open_user(a, 0), RES_OK);
        assert_eq!(h[0], UID0);
        assert_eq!(user_of(a), Some(0));
        let bad = [UID0 + 1, UID1];
        assert_eq!(user_of(bad.as_ptr() as u64), None, "user 2 is not registered");
        let mut out = [0u64; 4];
        assert_eq!(list(out.as_mut_ptr() as u64, 2, 0b11), 2);
        assert_eq!(out, [UID0, UID1, UID0 + 1, UID1]);
        assert_eq!(&PROFILE_JPEG[..2], b"\xff\xd8");
    }
}
