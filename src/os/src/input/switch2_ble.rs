//! The Switch 2 controllers' Windows transport (WinRT Bluetooth LE), after mkw-recomp's `switch2_win.c` flow: an
//! advertisement watcher for Nintendo's manufacturer data, the device opened from its address, its GATT services
//! read uncached (that read is what connects it), the command-reply and input characteristics subscribed, the
//! handshake (identity read, bonding in Sync, stick calibration, features, player lights), then reports in and
//! rumble / lights out as writes without response. A pad in Sync is bonded to this PC (fresh random keys), so later
//! a button press wakes it toward this PC and it connects without Sync. `NXRS_SWITCH2=0` turns it off,
//! `NXRS_SWITCH2=2` logs every Nintendo advertisement once. On Linux (`bluez` below) the same flow runs over BlueZ's
//! D-Bus API: LE discovery for the advertisements, Device1.Connect, the GATT characteristics from the object tree,
//! notifications through AcquireNotify sockets and writes without response through AcquireWrite / WriteValue.

use super::switch2 as s2;
use super::{ilog, motion, Vib};
#[cfg(windows)]
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::Sender;
#[cfg(windows)]
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use windows::core::GUID;
#[cfg(windows)]
use windows::Devices::Bluetooth::Advertisement::*;
#[cfg(windows)]
use windows::Devices::Bluetooth::GenericAttributeProfile::*;
#[cfg(windows)]
use windows::Devices::Bluetooth::*;
#[cfg(windows)]
use windows::Foundation::TypedEventHandler;
#[cfg(windows)]
use windows::Storage::Streams::{DataReader, DataWriter, IBuffer};

/// One connected controller, shared between the BLE thread and the input thread.
pub struct S2Dev {
    pub addr: u64,
    pub pid: u16,
    pub serial: String,
    pub cal: [s2::Cal; 2],
    pub report: Mutex<Option<s2::Report>>,
    pub last: Mutex<f64>,
    pub gone: AtomicBool,
    /// The player slot the input layer gave it (-1 none), for motion and lights.
    pub slot: AtomicI32,
    out: Mutex<Sender<Out>>,
}

enum Out {
    Cmd(Vec<u8>),
    Vib(Vec<u8>),
}

impl S2Dev {
    pub fn joy(&self) -> Option<usize> {
        match self.pid {
            s2::PID_JOY_L => Some(0),
            s2::PID_JOY_R => Some(1),
            _ => None,
        }
    }
    pub fn name(&self) -> String {
        format!(
            "{} {} Bluetooth LE",
            match self.pid {
                s2::PID_PRO => "Pro Controller 2",
                s2::PID_JOY_L => "Joy-Con 2 (L)",
                _ => "Joy-Con 2 (R)",
            },
            self.serial
        )
    }
    pub fn lights(&self, p: Option<usize>) {
        let pat = p.map(|p| s2::LED_PATTERN[p.min(7)]).unwrap_or(0);
        let _ = self.out.lock().unwrap().send(Out::Cmd(s2::command(
            s2::CMD_LEDS,
            s2::SUB_LEDS_SET,
            &[pat, 0, 0, 0],
        )));
    }
    pub fn rumble(&self, a: &Vib, b: &Vib, seq: u8) {
        let _ = self
            .out
            .lock()
            .unwrap()
            .send(Out::Vib(s2::rumble_packet(a, b, seq)));
    }
    /// Positional buttons and sticks from the newest report.
    pub fn state(&self) -> Option<(u64, [i32; 4], bool)> {
        let r = (*self.report.lock().unwrap())?;
        let b = s2::buttons(r.buttons, self.joy());
        let mut a = [0i32; 4];
        match self.pid {
            s2::PID_PRO => {
                let (x, y) = s2::apply_cal(&self.cal[0], r.lx, r.ly);
                let (u, v) = s2::apply_cal(&self.cal[1], r.rx, r.ry);
                a = [x, y, u, v];
            }
            s2::PID_JOY_L => {
                let (x, y) = s2::apply_cal(&self.cal[0], r.lx, r.ly);
                a[0] = x;
                a[1] = y;
            }
            _ => {
                let (x, y) = s2::apply_cal(&self.cal[0], r.rx, r.ry);
                a[2] = x;
                a[3] = y;
            }
        }
        Some((b, a, r.buttons & s2::B_HOME != 0))
    }
}

pub static DEVICES: Mutex<Vec<Arc<S2Dev>>> = Mutex::new(Vec::new());

#[cfg(windows)]
fn bytes(b: &IBuffer) -> Vec<u8> {
    let Ok(r) = DataReader::FromBuffer(b) else {
        return vec![];
    };
    let n = r.UnconsumedBufferLength().unwrap_or(0) as usize;
    let mut v = vec![0u8; n];
    let _ = r.ReadBytes(&mut v);
    v
}

#[cfg(windows)]
fn buffer(d: &[u8]) -> windows::core::Result<IBuffer> {
    let w = DataWriter::new()?;
    w.WriteBytes(d)?;
    w.DetachBuffer()
}

#[cfg(windows)]
fn write(c: &GattCharacteristic, d: &[u8]) -> bool {
    buffer(d)
        .and_then(|b| {
            c.WriteValueWithOptionAsync(&b, GattWriteOption::WriteWithoutResponse)?
                .join()
        })
        .map(|s| s == GattCommunicationStatus::Success)
        .unwrap_or(false)
}

#[cfg(windows)]
struct Link {
    dev: BluetoothLEDevice,
    cmd: GattCharacteristic,
    _rsp: GattCharacteristic,
    input: GattCharacteristic,
    vib: Option<GattCharacteristic>,
    replies: Receiver<Vec<u8>>,
    _keep: Vec<GattDeviceService>,
}

#[cfg(windows)]
impl Link {
    /// One command and its reply (the controller has one reply channel).
    fn command(&self, cmd: u8, sub: u8, payload: &[u8]) -> Option<Vec<u8>> {
        while self.replies.try_recv().is_ok() {}
        if !write(&self.cmd, &s2::command(cmd, sub, payload)) {
            return None;
        }
        let end = std::time::Instant::now() + Duration::from_secs(2);
        while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
            match self.replies.recv_timeout(left) {
                Ok(r) if s2::reply_is(&r, cmd) => return Some(r),
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        None
    }
    fn read(&self, len: u8, addr: u32) -> Option<Vec<u8>> {
        let r = self.command(
            s2::CMD_MEMORY,
            s2::SUB_MEMORY_READ,
            &s2::memory_payload(len, addr),
        )?;
        r.get(s2::REPLY_DATA..).map(|d| d.to_vec())
    }
}

#[cfg(windows)]
fn open(addr: u64) -> Result<Link, String> {
    let dev = BluetoothLEDevice::FromBluetoothAddressAsync(addr)
        .and_then(|o| o.join())
        .map_err(|e| format!("Windows did not open it: {e}"))?;
    let res = dev
        .GetGattServicesWithCacheModeAsync(BluetoothCacheMode::Uncached)
        .and_then(|o| o.join())
        .map_err(|e| format!("its services were not read: {e}"))?;
    if res.Status().ok() != Some(GattCommunicationStatus::Success) {
        return Err("its services were not read (the connection failed)".into());
    }
    let (mut cmd, mut rsp, mut input, mut vib) = (None, None, None, None);
    let mut keep = vec![];
    for svc in res.Services().map_err(|e| e.to_string())? {
        let Ok(cr) = svc
            .GetCharacteristicsWithCacheModeAsync(BluetoothCacheMode::Uncached)
            .and_then(|o| o.join())
        else {
            continue;
        };
        let Ok(chars) = cr.Characteristics() else {
            continue;
        };
        let mut any = false;
        for c in chars {
            let u = c.Uuid().unwrap_or_default();
            let slot = if u == GUID::from_u128(s2::UUID_CMD) {
                &mut cmd
            } else if u == GUID::from_u128(s2::UUID_RSP) {
                &mut rsp
            } else if u == GUID::from_u128(s2::UUID_INPUT) {
                &mut input
            } else if [s2::UUID_VIB_R, s2::UUID_VIB_L, s2::UUID_VIB_PRO]
                .iter()
                .any(|&v| u == GUID::from_u128(v))
            {
                &mut vib
            } else {
                continue;
            };
            if slot.is_none() {
                *slot = Some(c);
                any = true;
            }
        }
        if any {
            keep.push(svc);
        }
    }
    let (Some(cmd), Some(rsp), Some(input)) = (cmd, rsp, input) else {
        return Err("not the GATT service of a Switch 2 controller".into());
    };
    let (tx, rx) = channel();
    rsp.ValueChanged(&TypedEventHandler::<
        GattCharacteristic,
        GattValueChangedEventArgs,
    >::new(move |_, a| {
        if let Some(a) = a.as_ref() {
            if let Ok(b) = a.CharacteristicValue() {
                let _ = tx.send(bytes(&b));
            }
        }
        Ok(())
    }))
    .map_err(|e| e.to_string())?;
    rsp.WriteClientCharacteristicConfigurationDescriptorAsync(
        GattClientCharacteristicConfigurationDescriptorValue::Notify,
    )
    .and_then(|o| o.join())
    .map_err(|e| format!("no replies: {e}"))?;
    Ok(Link {
        dev,
        cmd,
        _rsp: rsp,
        input,
        vib,
        replies: rx,
        _keep: keep,
    })
}

#[cfg(windows)]
fn handshake(
    l: &Link,
    addr: u64,
    adv_pid: u16,
    pair: bool,
    host: Option<[u8; 6]>,
) -> Result<Arc<S2Dev>, String> {
    let info = l
        .read(0x40, s2::ADDR_INFO)
        .ok_or("it did not answer the identity read")?;
    if info.len() < 0x16 || u16::from_le_bytes([info[18], info[19]]) != s2::VENDOR {
        return Err("its identity is not Nintendo's".into());
    }
    let pid = u16::from_le_bytes([info[20], info[21]]);
    let pid = if matches!(pid, s2::PID_PRO | s2::PID_JOY_L | s2::PID_JOY_R) {
        pid
    } else {
        adv_pid
    };
    let serial: String = info[2..16]
        .iter()
        .map(|&c| {
            if (0x20..0x7f).contains(&c) {
                c as char
            } else {
                '?'
            }
        })
        .collect();
    if let (true, Some(h)) = (pair, host) {
        let mut ok = l
            .command(s2::CMD_PAIR, s2::SUB_PAIR_HOST, &s2::pair_host_payload(&h))
            .is_some();
        for k in 0..2 {
            let mut key = [0u8; 17];
            for (i, b) in key[1..].iter_mut().enumerate() {
                use std::hash::{BuildHasher, Hasher};
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u64(addr ^ i as u64);
                *b = h.finish() as u8;
            }
            ok = ok
                && l.command(
                    s2::CMD_PAIR,
                    if k == 0 {
                        s2::SUB_PAIR_KEY1
                    } else {
                        s2::SUB_PAIR_KEY2
                    },
                    &key,
                )
                .is_some();
        }
        ok = ok && l.command(s2::CMD_PAIR, s2::SUB_PAIR_DONE, &[0]).is_some();
        ilog!(
            "Switch 2 controller {addr:012X}: {}",
            if ok {
                "bonded to this PC (it wakes toward it from now on)"
            } else {
                "bonding failed; Sync it again next time"
            }
        );
    }
    let mut cal = [s2::Cal::default(); 2];
    let sticks = if pid == s2::PID_PRO { 2 } else { 1 };
    for (i, (blk, user)) in [
        (s2::ADDR_STICK1, s2::ADDR_USER_STICK1),
        (s2::ADDR_STICK2, s2::ADDR_USER_STICK2),
    ]
    .iter()
    .enumerate()
    .take(sticks)
    {
        let b = l.read(s2::STICK_BLOCK as u8, *blk);
        let u = l.read(0x0b, *user);
        let (c, kind) = s2::cal_from_flash(b.as_deref(), u.as_deref());
        cal[i] = c;
        ilog!(
            "Switch 2 controller {addr:012X}: stick {} calibration {}",
            i + 1,
            ["none (typical travel)", "factory", "user"][kind as usize]
        );
    }
    // a Joy-Con 2's mouse sensor too: the game reads it when it asks (hid_ext's Jcms / Nmouse)
    let feat = [
        s2::FEATURES
            | if pid == s2::PID_PRO {
                0
            } else {
                s2::FEATURE_MOUSE
            },
        0,
        0,
        0,
    ];
    l.command(s2::CMD_FEATURE, s2::SUB_FEATURE_SET, &feat);
    l.command(s2::CMD_FEATURE, s2::SUB_FEATURE_ENABLE, &feat)
        .ok_or("enabling its reports was refused")?;
    let (otx, orx) = channel::<Out>();
    let d = Arc::new(S2Dev {
        addr,
        pid,
        serial,
        cal,
        report: Mutex::new(None),
        last: Mutex::new(motion::now_s()),
        gone: AtomicBool::new(false),
        slot: AtomicI32::new(-1),
        out: Mutex::new(otx),
    });
    let dd = d.clone();
    l.input
        .ValueChanged(&TypedEventHandler::<
            GattCharacteristic,
            GattValueChangedEventArgs,
        >::new(move |_, a| {
            let Some(a) = a.as_ref() else { return Ok(()) };
            let v = bytes(&a.CharacteristicValue()?);
            if let Some(r) = s2::parse_report(&v) {
                let now = motion::now_s();
                let prev = *dd.report.lock().unwrap();
                let p = dd.slot.load(Ordering::Acquire);
                if let (Some(side), Some(prev)) = (dd.joy(), prev) {
                    let present = r.mouse != [0, 0] || r.mouse_q != 0 || r.mouse_s != 0;
                    if p >= 0
                        && present
                        && crate::services::hid_ext::MOUSE_WANTED.load(Ordering::Acquire) != 0
                    {
                        let dx = r.mouse[0].wrapping_sub(prev.mouse[0]) as i32;
                        let dy = r.mouse[1].wrapping_sub(prev.mouse[1]) as i32;
                        let st = crate::services::hid_ext::MOUSE_CONNECTED
                            | if r.mouse_s < s2::MOUSE_SURFACE_MAX {
                                crate::services::hid_ext::MOUSE_ON_SURFACE
                            } else {
                                0
                            };
                        crate::services::hid_ext::feed_mouse(p as usize, side, dx, dy, st);
                    }
                }
                *dd.report.lock().unwrap() = Some(r);
                *dd.last.lock().unwrap() = now;
                if p >= 0 && r.raw != [0; 6] {
                    let (acc, rad) = s2::imu_switch(&r.raw, dd.pid == s2::PID_JOY_R);
                    let m = super::motion();
                    let st = match dd.joy() {
                        Some(j) => &m.joy[p as usize][j],
                        None => &m.pad[p as usize],
                    };
                    st.lock().unwrap().add(now, acc, rad, 0.5);
                }
            }
            Ok(())
        }))
        .map_err(|e| e.to_string())?;
    l.input
        .WriteClientCharacteristicConfigurationDescriptorAsync(
            GattClientCharacteristicConfigurationDescriptorValue::Notify,
        )
        .and_then(|o| o.join())
        .map_err(|e| format!("its reports could not be subscribed: {e}"))?;
    // the writer: commands and rumble from the input thread, in order, off it
    let (cmd, vib, dg) = (l.cmd.clone(), l.vib.clone(), d.clone());
    std::thread::Builder::new()
        .name("switch2 out".into())
        .spawn(move || {
            while let Ok(o) = orx.recv() {
                if dg.gone.load(Ordering::Acquire) {
                    break;
                }
                match o {
                    Out::Cmd(c) => {
                        write(&cmd, &c);
                    }
                    Out::Vib(v) => {
                        if let Some(vc) = &vib {
                            write(vc, &v);
                        }
                    }
                }
            }
        })
        .ok();
    Ok(d)
}

#[cfg(windows)]
fn adapter_host() -> Option<[u8; 6]> {
    let a = BluetoothAdapter::GetDefaultAsync()
        .and_then(|o| o.join())
        .ok()?;
    if !a.IsLowEnergySupported().unwrap_or(false) {
        ilog!("Switch 2 controllers: this Bluetooth adapter has no LE");
        return None;
    }
    let addr = a.BluetoothAddress().ok()?;
    let b = addr.to_le_bytes();
    Some([b[0], b[1], b[2], b[3], b[4], b[5]])
}

/// Starts the watcher (once) on a thread of its own.
#[cfg(windows)]
pub fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if std::env::var("NXRS_SWITCH2").is_ok_and(|v| v == "0") || STARTED.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let trace = std::env::var("NXRS_SWITCH2").is_ok_and(|v| v == "2");
    std::thread::Builder::new()
        .name("switch2".into())
        .spawn(move || {
            let host = adapter_host();
            if host.is_none() {
                ilog!("Switch 2 controllers: no Bluetooth LE adapter answered; Pro Controller 2 / Joy-Con 2 are not looked for");
                return;
            }
            let (tx, rx) = channel::<(u64, s2::Advert)>();
            let w = match BluetoothLEAdvertisementWatcher::new() {
                Ok(w) => w,
                Err(e) => {
                    ilog!("Switch 2 controllers: no advertisement watcher: {e}");
                    return;
                }
            };
            let _ = w.SetScanningMode(BluetoothLEScanningMode::Active);
            let seen = Arc::new(Mutex::new(HashSet::new()));
            let r = w.Received(&TypedEventHandler::<BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementReceivedEventArgs>::new(move |_, a| {
                let Some(a) = a.as_ref() else { return Ok(()) };
                let addr = a.BluetoothAddress()?;
                for md in a.Advertisement()?.ManufacturerData()? {
                    if md.CompanyId()? != s2::COMPANY {
                        continue;
                    }
                    let d = bytes(&md.Data()?);
                    let adv = s2::parse_advert(&d);
                    if trace && seen.lock().unwrap().insert(addr) {
                        ilog!("Switch 2 trace: {addr:012X} Nintendo advertisement, {} bytes{}", d.len(), if adv.is_some() { ", a Switch 2 controller" } else { "" });
                    }
                    if let Some(adv) = adv {
                        let _ = tx.send((addr, adv));
                    }
                }
                Ok(())
            }));
            if let Err(e) = r.and_then(|_| w.Start()) {
                ilog!("Switch 2 controllers: the advertisement watcher did not start: {e}");
                return;
            }
            ilog!("Switch 2 controllers: looking for Pro Controller 2 / Joy-Con 2 over Bluetooth LE (hold Sync to pair)");
            let mut links: Vec<(u64, Link)> = vec![];
            let mut failed: Vec<(u64, f64)> = vec![];
            loop {
                // drop links whose controller went quiet or disconnected
                links.retain(|(addr, l)| {
                    let st = l.dev.ConnectionStatus().ok();
                    let alive = st != Some(BluetoothConnectionStatus::Disconnected);
                    if !alive {
                        if let Some(d) = DEVICES.lock().unwrap().iter().find(|d| d.addr == *addr) {
                            d.gone.store(true, Ordering::Release);
                        }
                        DEVICES.lock().unwrap().retain(|d| d.addr != *addr);
                        ilog!("Switch 2 controller {addr:012X} disconnected");
                    }
                    alive
                });
                let Ok((addr, adv)) = rx.recv_timeout(Duration::from_millis(500)) else { continue };
                let usage = s2::advert_use(&adv, host);
                let now = motion::now_s();
                if usage == s2::Use::Ignore || links.iter().any(|(a, _)| *a == addr) || failed.iter().any(|(a, t)| *a == addr && now < *t) {
                    continue;
                }
                ilog!("Switch 2 controller {addr:012X} (product {:04x}) {}, connecting", adv.pid, if adv.pairing { "in Sync" } else { "woke toward this PC" });
                let r = open(addr).and_then(|l| handshake(&l, addr, adv.pid, usage == s2::Use::Pair, host).map(|d| (l, d)));
                match r {
                    Ok((l, d)) => {
                        ilog!("{} connected", d.name());
                        DEVICES.lock().unwrap().push(d);
                        links.push((addr, l));
                        super::device_changed();
                    }
                    Err(e) => {
                        ilog!("Switch 2 controller {addr:012X}: {e}");
                        failed.push((addr, now + 10.0));
                    }
                }
            }
        })
        .ok();
}

/// The web: a page cannot scan Bluetooth LE on its own (Web Bluetooth needs a user prompt per device); Switch 2
/// controllers are not looked for.
#[cfg(target_os = "emscripten")]
pub fn start() {
    ilog!("Switch 2 controllers over Bluetooth are not looked for in a web build");
}

/// Starts the watcher (once) on a thread of its own (Linux: BlueZ over the system D-Bus).
#[cfg(target_os = "linux")]
pub fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if std::env::var("NXRS_SWITCH2").is_ok_and(|v| v == "0") || STARTED.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let trace = std::env::var("NXRS_SWITCH2").is_ok_and(|v| v == "2");
    std::thread::Builder::new()
        .name("switch2".into())
        .spawn(move || {
            if let Err(e) = bluez::run(trace) {
                ilog!("Switch 2 controllers: {e}; Pro Controller 2 / Joy-Con 2 are not looked for");
            }
        })
        .ok();
}

/// The BlueZ transport (mkw-recomp's flow, as on Windows): every D-Bus call blocks this module's own threads only.
#[cfg(target_os = "linux")]
mod bluez {
    use super::{ilog, motion, s2, Out, S2Dev, DEVICES};
    use std::collections::{HashMap, HashSet};
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const BLUEZ: &str = "org.bluez";
    type Props = HashMap<String, OwnedValue>;

    /// A 128-bit UUID constant in BlueZ's text form.
    pub fn uuid_text(u: u128) -> String {
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (u >> 96) as u32,
            (u >> 80) as u16,
            (u >> 64) as u16,
            (u >> 48) as u16,
            u & 0xffff_ffff_ffff
        )
    }

    /// "AA:BB:CC:DD:EE:FF" as the 48-bit number Windows calls the Bluetooth address.
    pub fn parse_addr(s: &str) -> Option<u64> {
        let p: Vec<&str> = s.split(':').collect();
        if p.len() != 6 {
            return None;
        }
        p.iter().try_fold(0u64, |a, b| {
            u8::from_str_radix(b, 16).ok().map(|v| a << 8 | v as u64)
        })
    }

    /// The manufacturer data of Nintendo's company id in a Device1 property map.
    fn nintendo_data(props: &Props) -> Option<Vec<u8>> {
        let v = props.get("ManufacturerData")?;
        let m: HashMap<u16, OwnedValue> = v.try_clone().ok()?.try_into().ok()?;
        let d = m.get(&s2::COMPANY)?;
        Vec::<u8>::try_from(d.try_clone().ok()?).ok()
    }

    fn prop<T: TryFrom<OwnedValue>>(p: &Props, k: &str) -> Option<T> {
        p.get(k)
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| T::try_from(v).ok())
    }

    fn objects(c: &Connection) -> zbus::Result<HashMap<OwnedObjectPath, HashMap<String, Props>>> {
        let om = zbus::blocking::fdo::ObjectManagerProxy::builder(c)
            .destination(BLUEZ)?
            .path("/")?
            .build()?;
        let o = om.get_managed_objects().map_err(zbus::Error::from)?;
        Ok(o.into_iter()
            .map(|(p, ifs)| {
                (
                    p,
                    ifs.into_iter()
                        .map(|(i, props)| (i.to_string(), props))
                        .collect(),
                )
            })
            .collect())
    }

    fn proxy<'a>(c: &Connection, path: &str, iface: &'a str) -> zbus::Result<Proxy<'a>> {
        Proxy::new(c, BLUEZ, path.to_string(), iface)
    }

    /// A characteristic's write side: its AcquireWrite socket, else WriteValue (type command: without response).
    struct Writer {
        c: Connection,
        path: String,
        fd: Option<OwnedFd>,
    }

    impl Writer {
        fn new(c: &Connection, path: &str) -> Writer {
            let fd = proxy(c, path, "org.bluez.GattCharacteristic1")
                .and_then(|p| {
                    p.call::<_, _, (zbus::zvariant::OwnedFd, u16)>(
                        "AcquireWrite",
                        &(HashMap::<String, Value>::new(),),
                    )
                })
                .ok()
                .map(|(fd, _mtu)| OwnedFd::from(fd));
            Writer {
                c: c.clone(),
                path: path.to_string(),
                fd,
            }
        }
        fn write(&self, d: &[u8]) -> bool {
            if let Some(fd) = &self.fd {
                // SAFETY: a write of our buffer to our socket
                let n = unsafe { libc::write(fd.as_raw_fd(), d.as_ptr() as *const _, d.len()) };
                return n == d.len() as isize;
            }
            let mut o: HashMap<&str, Value> = HashMap::new();
            o.insert("type", Value::from("command"));
            proxy(&self.c, &self.path, "org.bluez.GattCharacteristic1")
                .and_then(|p| p.call_method("WriteValue", &(d.to_vec(), o)))
                .is_ok()
        }
    }

    /// Notifications of a characteristic, each to `f` on a thread of its own: an AcquireNotify socket, else
    /// StartNotify and its Value changes. Ends when the link goes (`stop`) or the socket closes.
    fn subscribe(
        c: &Connection,
        path: &str,
        stop: Arc<AtomicBool>,
        f: impl Fn(&[u8]) + Send + 'static,
    ) -> Result<(), String> {
        let p = proxy(c, path, "org.bluez.GattCharacteristic1").map_err(|e| e.to_string())?;
        match p.call::<_, _, (zbus::zvariant::OwnedFd, u16)>(
            "AcquireNotify",
            &(HashMap::<String, Value>::new(),),
        ) {
            Ok((fd, mtu)) => {
                let fd = OwnedFd::from(fd);
                std::thread::Builder::new()
                    .name("switch2 notify".into())
                    .spawn(move || {
                        let mut b = vec![0u8; (mtu as usize).max(512)];
                        while !stop.load(Ordering::Acquire) {
                            let mut pf = libc::pollfd {
                                fd: fd.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            };
                            // SAFETY: one pollfd of ours
                            if unsafe { libc::poll(&mut pf, 1, 250) } <= 0 {
                                continue;
                            }
                            // SAFETY: reading into our buffer
                            let n = unsafe {
                                libc::read(fd.as_raw_fd(), b.as_mut_ptr() as *mut _, b.len())
                            };
                            if n <= 0 {
                                break;
                            }
                            f(&b[..n as usize]);
                        }
                    })
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            Err(_) => {
                p.call_method("StartNotify", &())
                    .map_err(|e| format!("notifications refused: {e}"))?;
                let rule = zbus::MatchRule::builder()
                    .msg_type(zbus::message::Type::Signal)
                    .sender(BLUEZ)
                    .and_then(|r| r.interface("org.freedesktop.DBus.Properties"))
                    .and_then(|r| r.member("PropertiesChanged"))
                    .and_then(|r| r.path(path.to_string()))
                    .map_err(|e| e.to_string())?
                    .build();
                let it = zbus::blocking::MessageIterator::for_match_rule(rule, c, Some(256))
                    .map_err(|e| e.to_string())?;
                std::thread::Builder::new()
                    .name("switch2 notify".into())
                    .spawn(move || {
                        for m in it {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            let Ok(m) = m else { break };
                            let Ok((_, ch, _)) =
                                m.body().deserialize::<(String, Props, Vec<String>)>()
                            else {
                                continue;
                            };
                            if let Some(v) = prop::<Vec<u8>>(&ch, "Value") {
                                f(&v);
                            }
                        }
                    })
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        }
    }

    struct Link {
        dev: String,
        cmd: Arc<Writer>,
        vib: Option<Arc<Writer>>,
        input: String,
        replies: Receiver<Vec<u8>>,
        stop: Arc<AtomicBool>,
    }

    impl Link {
        /// One command and its reply (the controller has one reply channel).
        fn command(&self, cmd: u8, sub: u8, payload: &[u8]) -> Option<Vec<u8>> {
            while self.replies.try_recv().is_ok() {}
            if !self.cmd.write(&s2::command(cmd, sub, payload)) {
                return None;
            }
            let end = Instant::now() + Duration::from_secs(2);
            while let Some(left) = end.checked_duration_since(Instant::now()) {
                match self.replies.recv_timeout(left) {
                    Ok(r) if s2::reply_is(&r, cmd) => return Some(r),
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            None
        }
        fn read(&self, len: u8, addr: u32) -> Option<Vec<u8>> {
            let r = self.command(
                s2::CMD_MEMORY,
                s2::SUB_MEMORY_READ,
                &s2::memory_payload(len, addr),
            )?;
            r.get(s2::REPLY_DATA..).map(|d| d.to_vec())
        }
        fn connected(&self, c: &Connection) -> bool {
            proxy(c, &self.dev, "org.bluez.Device1")
                .and_then(|p| p.get_property::<bool>("Connected"))
                .unwrap_or(false)
        }
    }

    impl Drop for Link {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
        }
    }

    fn open(c: &Connection, dev: &str) -> Result<Link, String> {
        let d = proxy(c, dev, "org.bluez.Device1").map_err(|e| e.to_string())?;
        d.call_method("Connect", &())
            .map_err(|e| format!("BlueZ did not connect it: {e}"))?;
        let end = Instant::now() + Duration::from_secs(10);
        while !d.get_property::<bool>("ServicesResolved").unwrap_or(false) {
            if Instant::now() > end {
                return Err("its services were not read (the connection failed)".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let (u_cmd, u_rsp, u_in) = (
            uuid_text(s2::UUID_CMD),
            uuid_text(s2::UUID_RSP),
            uuid_text(s2::UUID_INPUT),
        );
        let u_vib = [s2::UUID_VIB_R, s2::UUID_VIB_L, s2::UUID_VIB_PRO].map(uuid_text);
        let (mut cmd, mut rsp, mut input, mut vib) = (None, None, None, None);
        let prefix = format!("{dev}/");
        let mut chars: Vec<(String, String)> = objects(c)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|(p, _)| p.as_str().starts_with(&prefix))
            .filter_map(|(p, ifs)| {
                ifs.get("org.bluez.GattCharacteristic1")
                    .and_then(|pr| prop::<String>(pr, "UUID"))
                    .map(|u| (p.to_string(), u.to_lowercase()))
            })
            .collect();
        chars.sort();
        for (p, u) in chars {
            let slot = if u == u_cmd {
                &mut cmd
            } else if u == u_rsp {
                &mut rsp
            } else if u == u_in {
                &mut input
            } else if u_vib.contains(&u) {
                &mut vib
            } else {
                continue;
            };
            if slot.is_none() {
                *slot = Some(p);
            }
        }
        let (Some(cmd), Some(rsp), Some(input)) = (cmd, rsp, input) else {
            return Err("not the GATT service of a Switch 2 controller".into());
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        subscribe(c, &rsp, stop.clone(), move |v| {
            let _ = tx.lock().unwrap().send(v.to_vec());
        })
        .map_err(|e| format!("no replies: {e}"))?;
        Ok(Link {
            dev: dev.to_string(),
            cmd: Arc::new(Writer::new(c, &cmd)),
            vib: vib.map(|v| Arc::new(Writer::new(c, &v))),
            input,
            replies: rx,
            stop,
        })
    }

    /// One input report (Windows: the input characteristic's ValueChanged handler).
    fn on_input(dd: &S2Dev, v: &[u8]) {
        let Some(r) = s2::parse_report(v) else { return };
        let now = motion::now_s();
        let prev = *dd.report.lock().unwrap();
        let p = dd.slot.load(Ordering::Acquire);
        if let (Some(side), Some(prev)) = (dd.joy(), prev) {
            let present = r.mouse != [0, 0] || r.mouse_q != 0 || r.mouse_s != 0;
            if p >= 0
                && present
                && crate::services::hid_ext::MOUSE_WANTED.load(Ordering::Acquire) != 0
            {
                let dx = r.mouse[0].wrapping_sub(prev.mouse[0]) as i32;
                let dy = r.mouse[1].wrapping_sub(prev.mouse[1]) as i32;
                let st = crate::services::hid_ext::MOUSE_CONNECTED
                    | if r.mouse_s < s2::MOUSE_SURFACE_MAX {
                        crate::services::hid_ext::MOUSE_ON_SURFACE
                    } else {
                        0
                    };
                crate::services::hid_ext::feed_mouse(p as usize, side, dx, dy, st);
            }
        }
        *dd.report.lock().unwrap() = Some(r);
        *dd.last.lock().unwrap() = now;
        if p >= 0 && r.raw != [0; 6] {
            let (acc, rad) = s2::imu_switch(&r.raw, dd.pid == s2::PID_JOY_R);
            let m = super::super::motion();
            let st = match dd.joy() {
                Some(j) => &m.joy[p as usize][j],
                None => &m.pad[p as usize],
            };
            st.lock().unwrap().add(now, acc, rad, 0.5);
        }
    }

    fn handshake(
        c: &Connection,
        l: &Link,
        addr: u64,
        adv_pid: u16,
        pair: bool,
        host: Option<[u8; 6]>,
    ) -> Result<Arc<S2Dev>, String> {
        let info = l
            .read(0x40, s2::ADDR_INFO)
            .ok_or("it did not answer the identity read")?;
        if info.len() < 0x16 || u16::from_le_bytes([info[18], info[19]]) != s2::VENDOR {
            return Err("its identity is not Nintendo's".into());
        }
        let pid = u16::from_le_bytes([info[20], info[21]]);
        let pid = if matches!(pid, s2::PID_PRO | s2::PID_JOY_L | s2::PID_JOY_R) {
            pid
        } else {
            adv_pid
        };
        let serial: String = info[2..16]
            .iter()
            .map(|&c| {
                if (0x20..0x7f).contains(&c) {
                    c as char
                } else {
                    '?'
                }
            })
            .collect();
        if let (true, Some(h)) = (pair, host) {
            let mut ok = l
                .command(s2::CMD_PAIR, s2::SUB_PAIR_HOST, &s2::pair_host_payload(&h))
                .is_some();
            for k in 0..2 {
                let mut key = [0u8; 17];
                for (i, b) in key[1..].iter_mut().enumerate() {
                    use std::hash::{BuildHasher, Hasher};
                    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                    h.write_u64(addr ^ i as u64);
                    *b = h.finish() as u8;
                }
                ok = ok
                    && l.command(
                        s2::CMD_PAIR,
                        if k == 0 {
                            s2::SUB_PAIR_KEY1
                        } else {
                            s2::SUB_PAIR_KEY2
                        },
                        &key,
                    )
                    .is_some();
            }
            ok = ok && l.command(s2::CMD_PAIR, s2::SUB_PAIR_DONE, &[0]).is_some();
            ilog!(
                "Switch 2 controller {addr:012X}: {}",
                if ok {
                    "bonded to this PC (it wakes toward it from now on)"
                } else {
                    "bonding failed; Sync it again next time"
                }
            );
        }
        let mut cal = [s2::Cal::default(); 2];
        let sticks = if pid == s2::PID_PRO { 2 } else { 1 };
        for (i, (blk, user)) in [
            (s2::ADDR_STICK1, s2::ADDR_USER_STICK1),
            (s2::ADDR_STICK2, s2::ADDR_USER_STICK2),
        ]
        .iter()
        .enumerate()
        .take(sticks)
        {
            let b = l.read(s2::STICK_BLOCK as u8, *blk);
            let u = l.read(0x0b, *user);
            let (cl, kind) = s2::cal_from_flash(b.as_deref(), u.as_deref());
            cal[i] = cl;
            ilog!(
                "Switch 2 controller {addr:012X}: stick {} calibration {}",
                i + 1,
                ["none (typical travel)", "factory", "user"][kind as usize]
            );
        }
        let feat = [
            s2::FEATURES
                | if pid == s2::PID_PRO {
                    0
                } else {
                    s2::FEATURE_MOUSE
                },
            0,
            0,
            0,
        ];
        l.command(s2::CMD_FEATURE, s2::SUB_FEATURE_SET, &feat);
        l.command(s2::CMD_FEATURE, s2::SUB_FEATURE_ENABLE, &feat)
            .ok_or("enabling its reports was refused")?;
        let (otx, orx) = channel::<Out>();
        let d = Arc::new(S2Dev {
            addr,
            pid,
            serial,
            cal,
            report: Mutex::new(None),
            last: Mutex::new(motion::now_s()),
            gone: AtomicBool::new(false),
            slot: AtomicI32::new(-1),
            out: Mutex::new(otx),
        });
        let dd = d.clone();
        subscribe(c, &l.input, l.stop.clone(), move |v| on_input(&dd, v))
            .map_err(|e| format!("its reports could not be subscribed: {e}"))?;
        // the writer: commands and rumble from the input thread, in order, off it
        let (cmd, vib, dg) = (l.cmd.clone(), l.vib.clone(), d.clone());
        std::thread::Builder::new()
            .name("switch2 out".into())
            .spawn(move || {
                while let Ok(o) = orx.recv() {
                    if dg.gone.load(Ordering::Acquire) {
                        break;
                    }
                    match o {
                        Out::Cmd(c) => {
                            cmd.write(&c);
                        }
                        Out::Vib(v) => {
                            if let Some(vc) = &vib {
                                vc.write(&v);
                            }
                        }
                    }
                }
            })
            .ok();
        Ok(d)
    }

    /// The first powered adapter: its object path and its address as the handshake's host bytes.
    fn adapter(c: &Connection) -> Result<(String, [u8; 6]), String> {
        let objs = objects(c).map_err(|e| format!("BlueZ is not running ({e})"))?;
        let mut found = None;
        for (p, ifs) in &objs {
            if let Some(a) = ifs.get("org.bluez.Adapter1") {
                let addr = prop::<String>(a, "Address").and_then(|s| parse_addr(&s));
                let powered = prop::<bool>(a, "Powered").unwrap_or(false);
                if let Some(addr) = addr {
                    if found.is_none() || powered {
                        let b = addr.to_le_bytes();
                        found =
                            Some((p.to_string(), [b[0], b[1], b[2], b[3], b[4], b[5]], powered));
                    }
                    if powered {
                        break;
                    }
                }
            }
        }
        let (p, host, powered) = found.ok_or("no Bluetooth adapter")?;
        if !powered {
            return Err(format!("the Bluetooth adapter {p} is off"));
        }
        Ok((p, host))
    }

    /// The advertisements: Device1 properties as they change (DuplicateData: every advertisement) and new devices.
    fn watch(
        c: &Connection,
        tx: Sender<(String, u64, s2::Advert)>,
        trace: bool,
    ) -> Result<(), String> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(BLUEZ)
            .map_err(|e| e.to_string())?
            .build();
        let it = zbus::blocking::MessageIterator::for_match_rule(rule, c, Some(1024))
            .map_err(|e| e.to_string())?;
        let c2 = c.clone();
        std::thread::Builder::new()
            .name("switch2 adverts".into())
            .spawn(move || {
                let mut seen = HashSet::new();
                let mut send = |path: &str, props: &Props| {
                    let Some(d) = nintendo_data(props) else {
                        return;
                    };
                    let addr = prop::<String>(props, "Address")
                        .or_else(|| {
                            proxy(&c2, path, "org.bluez.Device1")
                                .ok()
                                .and_then(|p| p.get_property::<String>("Address").ok())
                        })
                        .and_then(|s| parse_addr(&s));
                    let Some(addr) = addr else { return };
                    let adv = s2::parse_advert(&d);
                    if trace && seen.insert(addr) {
                        ilog!(
                            "Switch 2 trace: {addr:012X} Nintendo advertisement, {} bytes{}",
                            d.len(),
                            if adv.is_some() {
                                ", a Switch 2 controller"
                            } else {
                                ""
                            }
                        );
                    }
                    if let Some(adv) = adv {
                        let _ = tx.send((path.to_string(), addr, adv));
                    }
                };
                for m in it {
                    let Ok(m) = m else { break };
                    let h = m.header();
                    let member = h.member().map(|m| m.to_string()).unwrap_or_default();
                    if member == "PropertiesChanged" {
                        let path = h.path().map(|p| p.to_string()).unwrap_or_default();
                        if let Ok((iface, ch, _)) =
                            m.body().deserialize::<(String, Props, Vec<String>)>()
                        {
                            if iface == "org.bluez.Device1" {
                                send(&path, &ch);
                            }
                        }
                    } else if member == "InterfacesAdded" {
                        if let Ok((path, ifs)) = m
                            .body()
                            .deserialize::<(OwnedObjectPath, HashMap<String, Props>)>()
                        {
                            if let Some(p) = ifs.get("org.bluez.Device1") {
                                send(path.as_str(), p);
                            }
                        }
                    }
                }
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn run(trace: bool) -> Result<(), String> {
        let c = Connection::system().map_err(|e| format!("no system D-Bus ({e})"))?;
        let (ad, host) = adapter(&c)?;
        let (tx, rx) = channel::<(String, u64, s2::Advert)>();
        watch(&c, tx.clone(), trace)?;
        let a = proxy(&c, &ad, "org.bluez.Adapter1").map_err(|e| e.to_string())?;
        let mut filter: HashMap<&str, Value> = HashMap::new();
        filter.insert("Transport", Value::from("le"));
        filter.insert("DuplicateData", Value::from(true));
        let _ = a.call_method("SetDiscoveryFilter", &(filter,));
        a.call_method("StartDiscovery", &())
            .map_err(|e| format!("LE discovery did not start ({e})"))?;
        // controllers BlueZ already knows: their last advertisement counts as seen now
        if let Ok(objs) = objects(&c) {
            for (p, ifs) in objs {
                if let Some(d) = ifs.get("org.bluez.Device1") {
                    if let (Some(data), Some(addr)) = (
                        nintendo_data(d),
                        prop::<String>(d, "Address").and_then(|s| parse_addr(&s)),
                    ) {
                        if let Some(adv) = s2::parse_advert(&data) {
                            let _ = tx.send((p.to_string(), addr, adv));
                        }
                    }
                }
            }
        }
        ilog!("Switch 2 controllers: looking for Pro Controller 2 / Joy-Con 2 over Bluetooth LE (BlueZ, {ad}; hold Sync to pair)");
        let host = Some(host);
        let mut links: Vec<(u64, Link)> = vec![];
        let mut failed: Vec<(u64, f64)> = vec![];
        loop {
            // drop links whose controller disconnected
            links.retain(|(addr, l)| {
                let alive = l.connected(&c);
                if !alive {
                    if let Some(d) = DEVICES.lock().unwrap().iter().find(|d| d.addr == *addr) {
                        d.gone.store(true, Ordering::Release);
                    }
                    DEVICES.lock().unwrap().retain(|d| d.addr != *addr);
                    ilog!("Switch 2 controller {addr:012X} disconnected");
                }
                alive
            });
            let Ok((path, addr, adv)) = rx.recv_timeout(Duration::from_millis(500)) else {
                continue;
            };
            let usage = s2::advert_use(&adv, host);
            let now = motion::now_s();
            if usage == s2::Use::Ignore
                || links.iter().any(|(a, _)| *a == addr)
                || failed.iter().any(|(a, t)| *a == addr && now < *t)
            {
                continue;
            }
            failed.retain(|(_, t)| now < *t);
            ilog!(
                "Switch 2 controller {addr:012X} (product {:04x}) {}, connecting",
                adv.pid,
                if adv.pairing {
                    "in Sync"
                } else {
                    "woke toward this PC"
                }
            );
            let r = open(&c, &path).and_then(|l| {
                handshake(&c, &l, addr, adv.pid, usage == s2::Use::Pair, host).map(|d| (l, d))
            });
            match r {
                Ok((l, d)) => {
                    ilog!("{} connected", d.name());
                    DEVICES.lock().unwrap().push(d);
                    links.push((addr, l));
                    super::super::device_changed();
                }
                Err(e) => {
                    ilog!("Switch 2 controller {addr:012X}: {e}");
                    failed.push((addr, now + 10.0));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn uuid_and_address_forms() {
            assert_eq!(
                super::uuid_text(super::s2::UUID_CMD),
                "649d4ac9-8eb7-4e6c-af44-1ea54fe5f005"
            );
            assert_eq!(
                super::uuid_text(super::s2::UUID_INPUT),
                "ab7de9be-89fe-49ad-828f-118f09df7fd2"
            );
            assert_eq!(
                super::parse_addr("98:B6:E9:01:02:0A"),
                Some(0x98B6_E901_020A)
            );
            assert_eq!(super::parse_addr("98:B6:E9"), None);
        }

        /// Against this machine's system bus: an adapter, or the reason there is none (never a panic or a hang).
        #[test]
        fn adapter_lookup_answers() {
            if let Ok(c) = super::Connection::system() {
                match super::adapter(&c) {
                    Ok((p, h)) => eprintln!("adapter {p} host {h:02x?}"),
                    Err(e) => eprintln!("no adapter: {e}"),
                }
            }
        }
    }
}
