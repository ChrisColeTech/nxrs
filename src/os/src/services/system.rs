//! System services a game opens through sm when its SDK client is not taken over by name, answered by the host:
//! - `psm`: the PC's own battery and charger (a desktop without a battery: 100 % on mains); a session's state-change
//!   event is signalled when a later question finds the charge or the charger changed;
//! - `set`: the language codes (the `language` setting), the region that follows from it, no quest (kiosk) mode, an
//!   empty key code map, and the PC's name as the device nickname;
//! - `set:sys`: the firmware version (12.1.0; 20.1.0 for a Switch 2 program), colour set 0, settings items of size 0;
//! - `fatal:u`: the run ends with the game's error code, as the console's fatal screen ends it;
//! - `spl:` / `spl:mig` / `csrng`: random bytes are the OS's; configuration items read 0;
//! - `mm:u`: clock requests acknowledged;
//! - `bcat:u` / `bcat:a`: an empty delivery cache (a sync completes at once, Done and success; no directory has a
//!   file; opening a file finds nothing);
//! - `aoc:u`: the add-on content installed in the data folder (`fs::aoc_indices`, the list the by-name takeovers give),
//!   its base id, the list-changed event, and the purchase managers (nothing is ever bought).

use crate::ipc::{self, Ctx, Service};
use crate::kobj::{Event, SysEvent};
use std::sync::Mutex;

/// Answers a command quietly with success (one log line per service and command).
fn quiet(ctx: &mut Ctx, name: &str) -> u32 {
    crate::log_once!(
        format!("system {name} {}", ctx.cmd()),
        "ipc: {name} #{}: success with no output (the host has nothing more to say)",
        ctx.cmd()
    );
    0
}

// ---- psm: the PC's battery

/// The PC's charge (0..100) and whether it runs on mains power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    pub percent: u32,
    pub charging: bool,
}

const MAINS: Power = Power {
    percent: 100,
    charging: true,
};

#[cfg(windows)]
pub fn power() -> Power {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // BatteryFlag 128: no system battery; 255 / percent 255: unknown
    if unsafe { GetSystemPowerStatus(&mut s) } == 0
        || s.BatteryFlag & 128 != 0
        || s.BatteryLifePercent > 100
    {
        return MAINS;
    }
    Power {
        percent: s.BatteryLifePercent as u32,
        charging: s.ACLineStatus == 1,
    }
}

/// Linux: the first battery of /sys/class/power_supply, on mains when a Mains supply is online (else by the
/// battery's own status).
#[cfg(not(windows))]
pub fn power() -> Power {
    let num = |p: std::path::PathBuf| {
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
    };
    let (mut bat, mut mains) = (None::<Power>, None::<bool>);
    if let Ok(d) = std::fs::read_dir("/sys/class/power_supply") {
        for e in d.flatten() {
            let p = e.path();
            match std::fs::read_to_string(p.join("type"))
                .unwrap_or_default()
                .trim()
            {
                "Battery" if bat.is_none() => {
                    if let Some(c) = num(p.join("capacity")).filter(|c| *c <= 100) {
                        let st = std::fs::read_to_string(p.join("status")).unwrap_or_default();
                        bat = Some(Power {
                            percent: c,
                            charging: st.trim() != "Discharging",
                        });
                    }
                }
                "Mains" => mains = Some(mains.unwrap_or(false) || num(p.join("online")) == Some(1)),
                _ => {}
            }
        }
    }
    match (bat, mains) {
        (Some(b), Some(m)) => Power { charging: m, ..b },
        (Some(b), None) => b,
        _ => MAINS,
    }
}

static PSM_CHANGED: SysEvent = SysEvent::new();
static PSM_LAST: Mutex<Option<Power>> = Mutex::new(None);

/// The power now; a change since the last question signals the sessions' state-change event.
fn psm_power() -> Power {
    let p = power();
    let mut l = PSM_LAST.lock().unwrap();
    if l.is_some_and(|o| o != p) {
        PSM_CHANGED.signal();
    }
    *l = Some(p);
    p
}

struct Psm;
struct PsmSession;

impl Service for Psm {
    fn name(&self) -> String {
        "psm".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // GetBatteryChargePercentage
            0 => ctx.push_u32(psm_power().percent),
            // GetChargerType: 1 a charger that gives enough power, 0 none
            1 => ctx.push_u32(psm_power().charging as u32),
            // IsBatteryChargingEnabled (4), and the same byte for 5
            4 | 5 => ctx.push_u8(psm_power().charging as u8),
            // OpenSession
            7 => ctx.object(PsmSession),
            _ => return quiet(ctx, "psm"),
        }
        0
    }
}

impl Service for PsmSession {
    fn name(&self) -> String {
        "IPsmSession".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // BindStateChangeEvent
            0 => {
                ctx.copy_handle(PSM_CHANGED.handout());
                0
            }
            // UnbindStateChangeEvent, SetChargerTypeChangeEventEnabled...
            _ => 0,
        }
    }
}

// ---- set / set:sys

/// The PC's name (the device nickname a game shows), at most 0x7f bytes.
pub fn pc_name() -> String {
    #[cfg(windows)]
    let n = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let n = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok());
    let n = n
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "nxrs".into());
    let mut end = n.len().min(0x7f);
    while !n.is_char_boundary(end) {
        end -= 1;
    }
    n[..end].to_string()
}

struct Set;

impl Service for Set {
    fn name(&self) -> String {
        "set".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        use crate::services::settings::code;
        match ctx.cmd() {
            // GetLanguageCode
            0 => ctx.push_u64(crate::services::oe::language_code()),
            // GetAvailableLanguageCodes (15 at most) / GetAvailableLanguageCodes2 (18)
            1 | 5 => {
                let max = if ctx.cmd() == 1 { 15 } else { 18 };
                let n = (ctx.buf_out(0).size / 8).min(max) as usize;
                let v: Vec<u8> = (0..n).flat_map(|i| code(i as i32).to_le_bytes()).collect();
                ctx.write_out(0, &v);
                ctx.push_u32(n as u32);
            }
            // MakeLanguageCode(Language)
            2 => {
                let v = code(ctx.u32(0) as i32);
                ctx.push_u64(v)
            }
            3 => ctx.push_u32(15),
            6 => ctx.push_u32(18),
            // GetRegionCode
            4 => ctx.push_u32(crate::services::oe::region_code()),
            // GetKeyCodeMap / GetKeyCodeMap2: the host maps no keyboard for the game
            7 | 9 => {
                let n = ctx.buf_out(0).size as usize;
                ctx.write_out(0, &vec![0; n.min(0x10000)]);
            }
            // GetQuestFlag: not a kiosk unit
            8 => ctx.push_u32(0),
            // GetDeviceNickName: the PC's name
            11 => {
                let mut b = vec![0u8; 0x80];
                let n = pc_name();
                b[..n.len()].copy_from_slice(n.as_bytes());
                ctx.write_out(0, &b);
            }
            _ => return quiet(ctx, "set"),
        }
        0
    }
}

/// The firmware the host reports (FirmwareVersion, 0x100 bytes): 12.1.0, or 20.1.0 to a Switch 2 program.
pub fn firmware(switch2: bool) -> [u8; 0x100] {
    let mut f = [0u8; 0x100];
    let (maj, min, ver) = if switch2 {
        (20, 1, "20.1.0")
    } else {
        (12, 1, "12.1.0")
    };
    f[..3].copy_from_slice(&[maj, min, 0]);
    f[4..6].copy_from_slice(&[maj, min]);
    let put =
        |f: &mut [u8; 0x100], at: usize, s: &str| f[at..at + s.len()].copy_from_slice(s.as_bytes());
    put(&mut f, 0x08, "NintendoSDK Firmware for NX");
    put(&mut f, 0x28, &format!("{ver}-1.0"));
    put(&mut f, 0x68, ver);
    put(
        &mut f,
        0x80,
        &format!("NintendoSDK Firmware for NX {ver}-1.0"),
    );
    f
}

struct SetSys;

impl Service for SetSys {
    fn name(&self) -> String {
        "set:sys".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // GetFirmwareVersion / GetFirmwareVersion2
            3 | 4 => {
                let f = firmware(crate::profile::get().program_id >> 56 == 0x04);
                ctx.write_out(0, &f);
            }
            // GetColorSetId, GetDebugModeFlag, 38 GetSettingsItemValue, 90 / 91
            23 | 62 | 90 | 91 => ctx.push_u32(0),
            38 => ctx.push_u64(0),
            // GetSettingsItemValueSize: no item has a value here
            37 => {
                let n = ctx.buf_out(0).size as usize;
                ctx.write_out(0, &vec![0; n.min(0x1000)]);
                ctx.push_u64(0);
            }
            _ => return quiet(ctx, "set:sys"),
        }
        0
    }
}

// ---- fatal:u

/// A result as the console's error code shows it (2XXX-YYYY: 2000 + module, description).
pub fn error_code(r: u32) -> String {
    format!("{:04}-{:04}", 2000 + (r & 0x1ff), (r >> 9) & 0x1fff)
}

#[cfg(test)]
static FATAL: Mutex<Option<String>> = Mutex::new(None);

struct Fatal;

impl Service for Fatal {
    fn name(&self) -> String {
        "fatal:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        // ThrowFatal (0), ThrowFatalWithPolicy (1), ThrowFatalWithCpuContext (2): the result first
        let r = ctx.u32(0);
        let msg = format!(
            "the game called fatal:u #{} with result {r:#x} (error {})",
            ctx.cmd(),
            error_code(r)
        );
        #[cfg(test)]
        {
            *FATAL.lock().unwrap() = Some(msg);
            0
        }
        #[cfg(not(test))]
        nxrt::rt::fatal(&msg)
    }
}

// ---- spl: / csrng / mm:u

/// Random bytes from the OS.
pub fn os_random(out: &mut [u8]) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        out.chunks_mut(u32::MAX as usize).all(|c| unsafe { BCryptGenRandom(std::ptr::null_mut(), c.as_mut_ptr(), c.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) } >= 0)
    }
    #[cfg(not(windows))]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(out))
            .is_ok()
    }
}

fn random_into(ctx: &mut Ctx, what: &str) -> u32 {
    let mut b = vec![0u8; (ctx.buf_out(0).size as usize).min(1 << 20)];
    if !os_random(&mut b) {
        crate::log!("[RED] {what}: the OS gave no random bytes");
        return crate::hle::RES_FAIL as u32;
    }
    ctx.write_out(0, &b);
    0
}

struct Spl(&'static str);

impl Service for Spl {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // GetConfig(item)
            0 => ctx.push_u64(0),
            // GenerateRandomBytes
            7 => return random_into(ctx, self.0),
            _ => return quiet(ctx, self.0),
        }
        0
    }
}

struct Csrng;

impl Service for Csrng {
    fn name(&self) -> String {
        "csrng".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            0 => random_into(ctx, "csrng"),
            _ => quiet(ctx, "csrng"),
        }
    }
}

struct Mm;

impl Service for Mm {
    fn name(&self) -> String {
        "mm:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // Initialize -> request id; Get(id) -> the setting
            4 | 6 => ctx.push_u32(1),
            _ => return quiet(ctx, "mm:u"),
        }
        0
    }
}

// ---- bcat: an empty delivery cache

/// bcat ResultNotFound (module 122, 2).
pub const R_BCAT_NOT_FOUND: u32 = 122 | 2 << 9;

struct Bcat(&'static str);
struct BcatService;
struct BcatProgress {
    events: Vec<Event>,
}
struct BcatStorage;
struct BcatDir;
struct BcatFile;

impl Service for Bcat {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // CreateBcatService
            0 => ctx.object(BcatService),
            // CreateDeliveryCacheStorageService(ByApplicationId)
            1 | 2 => {
                crate::log_once!("bcat mount", "bcat: the delivery cache is mounted, empty (no data is ever delivered offline)");
                ctx.object(BcatStorage)
            }
            _ => return quiet(ctx, self.0),
        }
        0
    }
}

impl Service for BcatService {
    fn name(&self) -> String {
        "IBcatService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // RequestSyncDeliveryCache(WithDirectoryName), the ByApplicationId forms
            10100 | 10101 | 20100 | 20101 | 20300 | 20301 => {
                ctx.object(BcatProgress { events: vec![] })
            }
            _ => return quiet(ctx, "IBcatService"),
        }
        0
    }
}

impl Service for BcatProgress {
    fn name(&self) -> String {
        "IDeliveryCacheProgressService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // GetEvent: the sync is already over
            0 => {
                let e = Event::signalled();
                ctx.copy_handle(e.readable);
                self.events.push(e);
            }
            // GetImpl: DeliveryCacheProgressImpl, status 9 (Done), result success, nothing downloaded
            1 => {
                let mut b = vec![0u8; (ctx.buf_out(0).size as usize).min(0x200)];
                if b.len() >= 8 {
                    b[..4].copy_from_slice(&9u32.to_le_bytes());
                }
                ctx.write_out(0, &b);
            }
            _ => return quiet(ctx, "IDeliveryCacheProgressService"),
        }
        0
    }
}

impl Service for BcatStorage {
    fn name(&self) -> String {
        "IDeliveryCacheStorageService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            0 => ctx.object(BcatFile),
            1 => ctx.object(BcatDir),
            // EnumerateDeliveryCacheDirectory: none
            10 => ctx.push_u32(0),
            _ => return quiet(ctx, "IDeliveryCacheStorageService"),
        }
        0
    }
}

impl Service for BcatDir {
    fn name(&self) -> String {
        "IDeliveryCacheDirectoryService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // Open(name): an empty directory
            0 => {}
            // Read / GetCount: no files
            1 | 2 => ctx.push_u32(0),
            _ => return quiet(ctx, "IDeliveryCacheDirectoryService"),
        }
        0
    }
}

impl Service for BcatFile {
    fn name(&self) -> String {
        "IDeliveryCacheFileService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // Open(directory, file): the cache holds no file
            0 => return R_BCAT_NOT_FOUND,
            1 | 2 => ctx.push_u64(0),
            // GetDigest
            3 => ctx.push_bytes(&[0; 16]),
            _ => return quiet(ctx, "IDeliveryCacheFileService"),
        }
        0
    }
}

// ---- aoc:u

/// aoc: the offset is past the list (module 128, 150).
pub const R_AOC_OFFSET: u32 = 128 | 150 << 9;
/// ec: no purchase to pop (module 164, 400).
pub const R_NO_PURCHASE: u32 = 164 | 400 << 9;

static AOC_CHANGED: SysEvent = SysEvent::new();

/// The add-on content a program owns: the installed indices when it is this game, none for another.
fn aoc_of(app: u64) -> Vec<u32> {
    let own = crate::profile::get().program_id;
    if app == 0 || app == own {
        crate::services::fs::aoc_indices()
    } else {
        vec![]
    }
}

struct Aoc;
struct PurchaseEvents {
    event: Option<Event>,
}

impl Service for Aoc {
    fn name(&self) -> String {
        "aoc:u".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // CountAddOnContentByApplicationId(app) / CountAddOnContent(pid)
            0 | 2 => {
                let app = if ctx.cmd() == 0 { ctx.u64(0) } else { 0 };
                let n = aoc_of(app).len() as u32;
                crate::log_once!("aoc ipc count", "aoc: CountAddOnContent -> {n}");
                ctx.push_u32(n);
            }
            // ListAddOnContentByApplicationId(offset, count, app) / ListAddOnContent(offset, count, pid)
            1 | 3 => {
                let (off, count) = (ctx.u32(0) as usize, ctx.u32(4) as usize);
                let app = if ctx.cmd() == 1 { ctx.u64(8) } else { 0 };
                let v = aoc_of(app);
                if off > v.len() {
                    return R_AOC_OFFSET;
                }
                let n = count
                    .min((ctx.buf_out(0).size / 4) as usize)
                    .min(v.len() - off);
                let b: Vec<u8> = v[off..off + n]
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect();
                ctx.write_out(0, &b);
                crate::log_once!(
                    "aoc ipc list",
                    "aoc: ListAddOnContent offset {off} count {count} -> {n}"
                );
                ctx.push_u32(n as u32);
            }
            // GetAddOnContentBaseId(ByApplicationId)
            4..=6 => ctx.push_u64((crate::profile::get().program_id & !0xfff) + 0x1000),
            // PrepareAddOnContent: installed content is always ready
            7 | 9 => {
                let i = ctx.u32(0);
                if !crate::services::fs::aoc_indices().contains(&i) {
                    crate::log_once!(format!("aoc prepare {i}"), "[RED] aoc: PrepareAddOnContent {i}: not installed (answered ready; mounting it fails)");
                }
            }
            // GetAddOnContentListChangedEvent(WithProcessId): the list never changes during a run
            8 | 10 => ctx.copy_handle(AOC_CHANGED.handout()),
            // Notify(Un)MountAddOnContent, CheckAddOnContentMountStatus...
            11..=13 | 50 => {}
            // CreateEcPurchasedEventManager / CreatePermanentEcPurchasedEventManager
            100 | 101 => ctx.object(PurchaseEvents { event: None }),
            _ => return quiet(ctx, "aoc:u"),
        }
        0
    }
}

impl Service for PurchaseEvents {
    fn name(&self) -> String {
        "IPurchaseEventManager".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // GetPurchasedEventReadableHandle: nothing is bought, it never fires
            2 => {
                let e = *self.event.get_or_insert_with(Event::new);
                ctx.copy_handle(e.readable);
                0
            }
            // PopPurchasedProductInfo(WithUid)
            3 | 4 => R_NO_PURCHASE,
            _ => 0,
        }
    }
}

pub fn register() {
    ipc::register("psm", || Box::new(Psm));
    ipc::register("set", || Box::new(Set));
    ipc::register("set:sys", || Box::new(SetSys));
    ipc::register("fatal:u", || Box::new(Fatal));
    ipc::register("spl:", || Box::new(Spl("spl:")));
    ipc::register("spl:mig", || Box::new(Spl("spl:mig")));
    ipc::register("csrng", || Box::new(Csrng));
    ipc::register("mm:u", || Box::new(Mm));
    ipc::register("bcat:u", || Box::new(Bcat("bcat:u")));
    ipc::register("bcat:a", || Box::new(Bcat("bcat:a")));
    ipc::register("aoc:u", || Box::new(Aoc));
}

#[cfg(test)]
mod tests {
    use crate::ipc::hipc::{self, Buf, Builder, T_REQUEST};

    fn send(h: u32, cmd: u32, data: Vec<u8>, b: Vec<Buf>) -> hipc::Response {
        let mut m = Builder {
            ty: T_REQUEST,
            cmd,
            data,
            b,
            ..Default::default()
        }
        .build();
        crate::ipc::process(h, &mut m);
        hipc::parse_response(&m, false).unwrap()
    }
    fn u32_at(r: &hipc::Response, o: usize) -> u32 {
        u32::from_le_bytes(r.data[o..o + 4].try_into().unwrap())
    }
    fn u64_at(r: &hipc::Response, o: usize) -> u64 {
        u64::from_le_bytes(r.data[o..o + 8].try_into().unwrap())
    }
    fn out(v: &mut [u8]) -> Vec<Buf> {
        vec![Buf {
            addr: v.as_mut_ptr() as u64,
            size: v.len() as u64,
            tag: 0,
        }]
    }
    fn open(name: &str) -> u32 {
        super::register();
        crate::ipc::connect(name).unwrap()
    }

    /// psm from the PC's own power state: 0 the charge, 1 the charger (1 on mains), 7 a session
    /// whose command 0 gives the state-change event.
    #[test]
    fn psm_reports_the_pcs_power() {
        crate::mem::tests::test_kernel();
        let p = super::power();
        assert!(p.percent <= 100);
        let h = open("psm");
        assert_eq!(u32_at(&send(h, 0, vec![], vec![]), 0), p.percent);
        assert_eq!(u32_at(&send(h, 1, vec![], vec![]), 0), p.charging as u32);
        let r = send(h, 7, vec![], vec![]);
        assert_eq!((r.result, r.moved.len()), (0, 1));
        let r = send(r.moved[0], 0, vec![], vec![]);
        assert_eq!((r.result, r.copy.len()), (0, 1), "BindStateChangeEvent");
    }

    /// set and set:sys: language codes (15 / 18 of nn::settings' table), MakeLanguageCode, the counts, the region,
    /// the quest flag 0, an empty key code map, the device nickname (the PC's name), and set:sys's firmware block (version bytes, platform, hash, display version and
    /// title at 0x08 / 0x28 / 0x68 / 0x80), colour set 0, a settings item of size 0.
    #[test]
    fn set_and_set_sys_answers() {
        use crate::services::settings::code;
        let h = open("set");
        assert_eq!(
            u64_at(&send(h, 0, vec![], vec![]), 0),
            crate::services::oe::language_code()
        );
        let mut b = [0u8; 8 * 20];
        let r = send(h, 1, vec![], out(&mut b));
        assert_eq!(u32_at(&r, 0), 15);
        assert_eq!(&b[..5], b"ja\0\0\0");
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), code(1));
        assert_eq!(
            &b[15 * 8..15 * 8 + 8],
            &[0; 8],
            "GetAvailableLanguageCodes stops at 15"
        );
        let r = send(h, 5, vec![], out(&mut b));
        assert_eq!(u32_at(&r, 0), 18);
        assert_eq!(&b[17 * 8..17 * 8 + 5], b"pt-BR");
        assert_eq!(
            u64_at(&send(h, 2, 12u32.to_le_bytes().to_vec(), vec![]), 0),
            code(12),
            "MakeLanguageCode en-GB"
        );
        assert_eq!(u32_at(&send(h, 3, vec![], vec![]), 0), 15);
        assert_eq!(u32_at(&send(h, 6, vec![], vec![]), 0), 18);
        assert_eq!(
            u32_at(&send(h, 4, vec![], vec![]), 0),
            crate::services::oe::region_code()
        );
        assert_eq!(u32_at(&send(h, 8, vec![], vec![]), 0), 0, "not a kiosk");
        let mut km = [0xAAu8; 0x40];
        assert_eq!(send(h, 7, vec![], out(&mut km)).result, 0);
        assert_eq!(km, [0; 0x40], "an empty key code map");
        let mut nick = [0xAAu8; 0x80];
        assert_eq!(send(h, 11, vec![], out(&mut nick)).result, 0);
        let name = super::pc_name();
        assert!(!name.is_empty());
        assert_eq!(&nick[..name.len()], name.as_bytes());
        assert_eq!(nick[name.len()], 0);

        let s = open("set:sys");
        let mut fw = [0u8; 0x100];
        assert_eq!(send(s, 3, vec![], out(&mut fw)).result, 0);
        assert_eq!(&fw[..8], &[12, 1, 0, 0, 12, 1, 0, 0]);
        assert_eq!(
            crate::guest::cstr(fw.as_ptr() as u64 + 0x08, 0x20),
            "NintendoSDK Firmware for NX"
        );
        assert_eq!(
            crate::guest::cstr(fw.as_ptr() as u64 + 0x28, 0x40),
            "12.1.0-1.0"
        );
        assert_eq!(
            crate::guest::cstr(fw.as_ptr() as u64 + 0x68, 0x18),
            "12.1.0"
        );
        assert_eq!(
            crate::guest::cstr(fw.as_ptr() as u64 + 0x80, 0x80),
            "NintendoSDK Firmware for NX 12.1.0-1.0"
        );
        assert_eq!(
            &super::firmware(true)[..3],
            &[20, 1, 0],
            "a Switch 2 program sees a Switch 2 firmware"
        );
        assert_eq!(u32_at(&send(s, 23, vec![], vec![]), 0), 0, "colour set");
        let mut item = [0xAAu8; 8];
        let r = send(s, 37, vec![], out(&mut item));
        assert_eq!(
            (r.result, u64_at(&r, 0), item),
            (0, 0, [0; 8]),
            "settings item size 0"
        );
    }

    /// fatal:u ends the run with the game's result ("The game called fatal:u (code ...)").
    #[test]
    fn fatal_u_ends_the_run_with_the_code() {
        let h = open("fatal:u");
        send(h, 0, [0x2A2u32.to_le_bytes(), [0; 4]].concat(), vec![]);
        let m = super::FATAL.lock().unwrap().take().unwrap();
        assert!(m.contains("0x2a2") && m.contains("2162-0001"), "{m}");
    }

    /// spl: GetConfig 0 and GenerateRandomBytes (the OS's bytes); csrng the same; mm:u 4 and 6
    /// answer 1.
    #[test]
    fn spl_csrng_mm() {
        let h = open("spl:");
        assert_eq!(
            u64_at(&send(h, 0, 5u32.to_le_bytes().to_vec(), vec![]), 0),
            0
        );
        let mut r1 = [0u8; 64];
        let mut r2 = [0u8; 64];
        assert_eq!(send(h, 7, vec![], out(&mut r1)).result, 0);
        assert_eq!(send(open("csrng"), 0, vec![], out(&mut r2)).result, 0);
        assert!(r1 != [0; 64] && r2 != [0; 64] && r1 != r2, "random bytes");
        let m = open("mm:u");
        assert_eq!(u32_at(&send(m, 4, vec![0; 12], vec![]), 0), 1);
        assert_eq!(u32_at(&send(m, 6, vec![0; 4], vec![]), 0), 1);
    }

    /// bcat: an empty delivery cache: the storage service, no directory listed, a directory with
    /// no file, a file open answering not found (0x47a); a sync's progress whose event is signalled and whose state
    /// is Done (9) with success.
    #[test]
    fn bcat_mounts_an_empty_cache() {
        crate::mem::tests::test_kernel();
        let h = open("bcat:u");
        let r = send(h, 1, vec![0; 8], vec![]);
        assert_eq!(
            (r.result, r.moved.len()),
            (0, 1),
            "CreateDeliveryCacheStorageService"
        );
        let st = r.moved[0];
        assert_eq!(u32_at(&send(st, 10, vec![], vec![]), 0), 0, "no directory");
        let dir = send(st, 1, vec![], vec![]).moved[0];
        assert_eq!(send(dir, 0, vec![0; 32], vec![]).result, 0);
        assert_eq!(u32_at(&send(dir, 2, vec![], vec![]), 0), 0, "no file");
        let file = send(st, 0, vec![], vec![]).moved[0];
        assert_eq!(
            send(file, 0, vec![0; 64], vec![]).result,
            super::R_BCAT_NOT_FOUND
        );
        assert_eq!(super::R_BCAT_NOT_FOUND, 0x47a);
        let svc = send(h, 0, vec![0; 8], vec![]).moved[0];
        let prog = send(svc, 10100, vec![], vec![]).moved[0];
        let r = send(prog, 0, vec![], vec![]);
        assert_eq!(r.copy.len(), 1);
        assert_eq!(
            nxrt::kernel::get().event_signaled(r.copy[0]),
            Some(true),
            "the progress event is signalled"
        );
        let mut imp = [0xAAu8; 0x200];
        send(prog, 1, vec![], out(&mut imp));
        assert_eq!(
            (&imp[..4], &imp[4..8]),
            (&9u32.to_le_bytes()[..], &[0u8; 4][..]),
            "Done, success"
        );
        assert!(imp[8..].iter().all(|&x| x == 0));
    }

    /// aoc:u over the installed list: count and list (the ByApplicationId forms for
    /// another program find none), an offset past the end fails (128-150), the base id, prepare, the list-changed
    /// event, and a purchase manager whose event never fires and which has nothing to pop (164-400).
    #[test]
    fn aoc_u_lists_the_installed_content() {
        crate::mem::tests::test_kernel();
        let h = open("aoc:u");
        let have = crate::services::fs::aoc_indices();
        assert_eq!(
            u32_at(&send(h, 2, vec![0; 8], vec![]), 0) as usize,
            have.len()
        );
        assert_eq!(
            u32_at(
                &send(
                    h,
                    0,
                    0x0100_0000_0000_9000u64.to_le_bytes().to_vec(),
                    vec![]
                ),
                0
            ),
            0,
            "another program's"
        );
        let mut l = [0u8; 16];
        let r = send(
            h,
            3,
            [0u32, 4, 0, 0]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
            out(&mut l),
        );
        assert_eq!((r.result, u32_at(&r, 0) as usize), (0, have.len().min(4)));
        let r = send(
            h,
            3,
            [have.len() as u32 + 1, 4, 0, 0]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
            out(&mut l),
        );
        assert_eq!(r.result, super::R_AOC_OFFSET);
        assert_eq!(
            u64_at(&send(h, 5, vec![0; 8], vec![]), 0),
            (crate::profile::get().program_id & !0xfff) + 0x1000
        );
        assert_eq!(send(h, 9, vec![0; 16], vec![]).result, 0);
        let r = send(h, 8, vec![], vec![]);
        assert_eq!(r.copy.len(), 1);
        assert_eq!(
            nxrt::kernel::get().event_signaled(r.copy[0]),
            Some(false),
            "the list never changes"
        );
        let pm = send(h, 100, vec![], vec![]).moved[0];
        assert_eq!(send(pm, 2, vec![], vec![]).copy.len(), 1);
        assert_eq!(send(pm, 3, vec![], vec![]).result, super::R_NO_PURCHASE);
        assert_eq!(super::R_NO_PURCHASE, 164 | 400 << 9);
    }
}
