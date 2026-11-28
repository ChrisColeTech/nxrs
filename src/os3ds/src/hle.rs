//! the 3DS OS services over runtime3ds: the IPC session table (ports, service handles, file
//! sessions), the per-command dispatch, the scripted input feed and the gap ledger. The wire
//! format is 3dbrew `IPC`: the request and reply live at TLS+0x80, header bits 16-31 are the
//! command id, bits 6-11 the normal-parameter count, bits 0-5 the translate-word count, and reply
//! word 1 is the result code.

use runtime3ds::Process3ds;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// success
pub const R_OK: u32 = 0;
/// the kernel's stubbed-svc result (3dbrew `SVC`, "stubbed")
pub const R_STUB: u32 = 0xF8C0_07F4;
/// the generic invalid-argument result
pub const R_INVALID: u32 = 0xD8E0_07EE;
/// an unknown IPC command (3dbrew's named refusal page)
pub const R_INVALID_CMD: u32 = 0xD8E0_07F0;
/// a service not registered under that name
pub const R_NOT_FOUND: u32 = 0xD880_4474;
/// FS: the path does not exist
pub const R_PATH_NOT_FOUND: u32 = 0xC880_4478;
/// FS: the file ends before the read
pub const R_END_OF_FILE: u32 = 0xC880_447C;
/// FS: the archive is not open
pub const R_INVALID_ARCHIVE: u32 = 0xC880_4476;

/// the client id this process presents to GSP/HID (the first slot)
pub const CLIENT_ID: u32 = 0;

/// the service behind a session handle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Srv,
    ErrF,
    Apt,
    Fs,
    /// a raw file session (FS opens these and returns them as a handle)
    File(u32),
    /// a raw directory session
    Dir(u32),
    Hid,
    Gsp,
    Dsp,
    Cfg,
    Ptm,
    LdrRo,
    Y2r,
    Soc,
    /// a handshake-only service (mic/cam/boss/news/ndm/ir), named for the log
    Misc(&'static str),
}

/// the service a port/service name maps to
pub fn service_named(name: &str) -> Option<Service> {
    Some(match name {
        "srv:" => Service::Srv,
        "err:f" => Service::ErrF,
        "apt:U" | "APT:U" | "apt:A" | "APT:A" | "apt:s" => Service::Apt,
        "fs:USER" | "fs:user" => Service::Fs,
        "hid:USER" | "hid:user" => Service::Hid,
        "gsp::Gpu" | "gsp::GPU" => Service::Gsp,
        "dsp::DSP" | "dsp::dsp" => Service::Dsp,
        "cfg:s" | "cfg:u" | "cfg:i" => Service::Cfg,
        "ptm:s" | "ptm:u" | "ptm:sysm" => Service::Ptm,
        "ldr:ro" => Service::LdrRo,
        "y2r:u" | "y2r:r" => Service::Y2r,
        "soc:U" | "soc:u" => Service::Soc,
        "mic:u" => Service::Misc("mic:u"),
        "cam:u" => Service::Misc("cam:u"),
        "boss:U" | "boss:u" => Service::Misc("boss:U"),
        "news:s" | "news:u" => Service::Misc("news"),
        "ndm:u" => Service::Misc("ndm:u"),
        "ir:rst" | "ir:USER" | "ir:u" => Service::Misc("ir"),
        _ => return None,
    })
}

/// the applet lifecycle state the APT commands read back
#[derive(Debug, Clone)]
pub struct AptState {
    pub initialized: bool,
    pub enabled: bool,
    pub applet_id: u32,
    pub applet_pos: u32,
    pub preparation: u32,
    pub last_signaled: u32,
    /// is an APT parameter (the initial wakeup) waiting for ReceiveParameter
    pub parameter_pending: bool,
}

impl Default for AptState {
    fn default() -> AptState {
        AptState {
            initialized: false,
            enabled: false,
            // 0x300 = the application itself; POS_APP = 0
            applet_id: 0x300,
            applet_pos: 0,
            preparation: 0,
            last_signaled: 0,
            parameter_pending: true,
        }
    }
}

/// where a file session reads its bytes: the staged save data in memory, or a read-only range of
/// the dump (the RomFS IVFC image the SDK parses itself)
pub enum FileSource {
    Memory(Vec<u8>),
    Image { path: PathBuf, base: u64, size: u64 },
}

impl FileSource {
    pub fn len(&self) -> u64 {
        match self {
            FileSource::Memory(bytes) => bytes.len() as u64,
            FileSource::Image { size, .. } => *size,
        }
    }

    /// up to `len` bytes at `offset`; an image read seeks the dump
    pub fn read(&self, offset: u64, len: usize) -> Vec<u8> {
        match self {
            FileSource::Memory(bytes) => {
                let start = (offset as usize).min(bytes.len());
                let end = (start + len).min(bytes.len());
                bytes[start..end].to_vec()
            }
            FileSource::Image { path, base, size } => {
                use std::io::{Read, Seek, SeekFrom};
                let available = size.saturating_sub(offset) as usize;
                let mut out = vec![0u8; len.min(available)];
                if out.is_empty() {
                    return out;
                }
                if let Ok(mut file) = std::fs::File::open(path) {
                    if file.seek(SeekFrom::Start(base + offset)).is_ok() {
                        let _ = file.read_exact(&mut out);
                    }
                }
                out
            }
        }
    }
}

/// one open file session
pub struct FileHandle {
    pub source: FileSource,
    pub pos: u64,
    pub path: String,
    pub writable: bool,
    pub dirty: bool,
}

impl FileHandle {
    pub fn write(&mut self, offset: usize, chunk: &[u8]) -> bool {
        let FileSource::Memory(bytes) = &mut self.source else {
            return false;
        };
        if offset + chunk.len() > bytes.len() {
            bytes.resize(offset + chunk.len(), 0);
        }
        bytes[offset..offset + chunk.len()].copy_from_slice(chunk);
        self.dirty = true;
        true
    }

    pub fn set_size(&mut self, size: usize) {
        if let FileSource::Memory(bytes) = &mut self.source {
            bytes.resize(size, 0);
            self.dirty = true;
        }
    }
}

/// one open directory session
pub struct DirHandle {
    pub entries: Vec<String>,
    pub index: usize,
}

/// an opened archive
#[derive(Debug, Clone, Copy)]
pub struct Archive {
    pub kind: u32,
}

/// a shared-memory block the host created for a service (HID/GSP), with its backing bytes
pub struct SharedBlock {
    pub bytes: Vec<u8>,
    pub kind: SharedKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedKind {
    Hid,
    Gsp,
    Other,
}

/// everything the sessions mutate
pub struct State {
    pub sessions: BTreeMap<u32, Service>,
    pub files: BTreeMap<u32, FileHandle>,
    pub dirs: BTreeMap<u32, DirHandle>,
    pub archives: BTreeMap<u32, Archive>,
    pub shared: BTreeMap<u32, SharedBlock>,
    /// block handle -> (guest address, size, kind), set at MapMemoryBlock
    pub mapped: BTreeMap<u32, (u32, u32, SharedKind)>,
    pub next_handle: u32,
    pub next_id: u32,
    pub apt: AptState,
    pub hid_events: Vec<u32>,
    pub gsp_event: Option<u32>,
    pub gsp_shared: Option<u32>,
    pub hid_shared: Option<u32>,
    /// the APT lock handle (a service-owned sync object, see kernel.rs)
    pub apt_lock: Option<u32>,
}

impl State {
    fn new() -> State {
        State {
            sessions: BTreeMap::new(),
            files: BTreeMap::new(),
            dirs: BTreeMap::new(),
            archives: BTreeMap::new(),
            shared: BTreeMap::new(),
            mapped: BTreeMap::new(),
            next_handle: 0x9000,
            next_id: 1,
            apt: AptState::default(),
            hid_events: Vec::new(),
            gsp_event: None,
            gsp_shared: None,
            hid_shared: None,
            apt_lock: None,
        }
    }

    pub fn fresh_handle(&mut self) -> u32 {
        let handle = self.next_handle;
        self.next_handle += 1;
        handle
    }

    pub fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// one scripted pad state change
#[derive(Debug, Clone, Copy)]
pub struct InputStep {
    pub at_ms: u64,
    pub buttons: u32,
    pub cx: i16,
    pub cy: i16,
    pub touch: Option<(u16, u16)>,
    /// the line it came from, for the consumed log
    pub line: &'static str,
}

/// the pad state the HID service serves
#[derive(Debug, Clone, Copy)]
pub struct PadState {
    pub buttons: u32,
    pub cx: i16,
    pub cy: i16,
    pub touch: Option<(u16, u16)>,
    pub tick: u64,
}

impl Default for PadState {
    fn default() -> PadState {
        PadState { buttons: 0, cx: 0, cy: 0, touch: None, tick: 0 }
    }
}

/// the scripted input feed: steps by wall-clock ms from the boot start; each applied step is
/// written into the HID shared memory and logged
pub struct InputScript {
    pub steps: Vec<InputStep>,
    pub cursor: usize,
    pub now: PadState,
    pub applied: Vec<String>,
    pub serves: u64,
    pub updates: u64,
}

impl InputScript {
    pub fn new(steps: Vec<InputStep>) -> InputScript {
        InputScript { steps, cursor: 0, now: PadState::default(), applied: Vec::new(), serves: 0, updates: 0 }
    }

    pub fn empty() -> InputScript {
        InputScript::new(Vec::new())
    }

    /// applies every step due at `elapsed_ms`; says whether a step landed
    pub fn advance(&mut self, elapsed_ms: u64) -> bool {
        let mut changed = false;
        while self.cursor < self.steps.len() && self.steps[self.cursor].at_ms <= elapsed_ms {
            let step = self.steps[self.cursor];
            self.now = PadState {
                buttons: step.buttons,
                cx: step.cx,
                cy: step.cy,
                touch: step.touch,
                tick: self.now.tick.wrapping_add(1),
            };
            self.applied.push(format!(
                "{} ms: {} (buttons {:#06x}, circle {},{}{})",
                elapsed_ms,
                step.line,
                step.buttons,
                step.cx,
                step.cy,
                step.touch.map(|(x, y)| format!(", touch {x},{y}")).unwrap_or_default()
            ));
            self.cursor += 1;
            changed = true;
        }
        changed
    }
}

/// parse the input script text: `<ms> <keys>` lines; `neutral`, `a`, `start`, `left`, `a+left`,
/// `touch:x,y` (pixel coordinates), `#` comments
pub fn parse_input(text: &str) -> Result<Vec<InputStep>, String> {
    let mut steps = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((at, keys)) = line.split_once(char::is_whitespace) else {
            return Err(format!("line {}: expected `<ms> <keys>`", number + 1));
        };
        let at_ms: u64 = at.trim().parse().map_err(|_| format!("line {}: bad time {at}", number + 1))?;
        let (buttons, cx, cy, touch) = parse_keys(keys.trim())?;
        let held: &'static str = Box::leak(keys.trim().to_owned().into_boxed_str());
        steps.push(InputStep { at_ms, buttons, cx, cy, touch, line: held });
    }
    steps.sort_by_key(|s| s.at_ms);
    Ok(steps)
}

/// key names into the HID PAD bits (3dbrew `HID Shared Memory`: A 0, B 1, Select 2, Start 3,
/// Right 4, Left 5, Up 6, Down 7, R 8, L 9, X 10, Y 11), circle pad and touch
pub fn parse_keys(spec: &str) -> Result<(u32, i16, i16, Option<(u16, u16)>), String> {
    let mut buttons = 0u32;
    let (mut cx, mut cy) = (0i16, 0i16);
    let mut touch = None;
    if spec.eq_ignore_ascii_case("neutral") || spec.eq_ignore_ascii_case("none") || spec.is_empty() {
        return Ok((0, 0, 0, None));
    }
    for key in spec.split(['+', ',', ' ']).filter(|k| !k.is_empty()) {
        let lower = key.to_ascii_lowercase();
        let bit = match lower.as_str() {
            "a" => 0,
            "b" => 1,
            "select" => 2,
            "start" => 3,
            "right" => 4,
            "left" => 5,
            "up" => 6,
            "down" => 7,
            "r" => 8,
            "l" => 9,
            "x" => 10,
            "y" => 11,
            _ => {
                if let Some(rest) = lower.strip_prefix("touch:") {
                    let (x, y) = rest
                        .split_once(',')
                        .ok_or_else(|| format!("touch needs `touch:x,y`: {key}"))?;
                    let x = x.parse().map_err(|_| format!("bad touch x: {key}"))?;
                    let y = y.parse().map_err(|_| format!("bad touch y: {key}"))?;
                    touch = Some((x, y));
                    continue;
                }
                if let Some(rest) = lower.strip_prefix("circle:") {
                    let (x, y) = rest
                        .split_once(',')
                        .ok_or_else(|| format!("circle needs `circle:x,y`: {key}"))?;
                    cx = x.parse().map_err(|_| format!("bad circle x: {key}"))?;
                    cy = y.parse().map_err(|_| format!("bad circle y: {key}"))?;
                    continue;
                }
                return Err(format!("unknown key {key}"));
            }
        };
        buttons |= 1 << bit;
    }
    Ok((buttons, cx, cy, touch))
}

/// one IPC request read out of TLS+0x80
pub struct Request {
    pub cmd: u32,
    pub normal: usize,
    pub translate: usize,
    /// the normal parameters (word 1 onward)
    pub params: Vec<u32>,
    /// the translate words (descriptors and their payload)
    pub translated: Vec<u32>,
}

impl Request {
    pub fn word(&self, index: usize) -> u32 {
        self.params.get(index).copied().unwrap_or(0)
    }
}

/// one IPC reply: `words[0]` is the result (reply word 1); `translate` counts the trailing words
/// that form the translate parameters
pub struct Reply {
    pub words: Vec<u32>,
    pub translate: usize,
}

impl Reply {
    pub fn ok() -> Reply {
        Reply { words: vec![R_OK], translate: 0 }
    }
    pub fn err(code: u32) -> Reply {
        Reply { words: vec![code], translate: 0 }
    }
    pub fn new(words: Vec<u32>, translate: usize) -> Reply {
        Reply { words, translate }
    }
}

/// the os3ds host: one per boot; the services, the input feed, the gap ledger
pub struct Host {
    /// the RomFS root (the staged `romfs/`, read-only)
    pub romfs_dir: PathBuf,
    /// the save root (`<data>\save`, writable)
    pub save_dir: PathBuf,
    /// the synthetic SDMC root (read-only in this wave)
    pub sdmc_dir: PathBuf,
    pub state: Mutex<State>,
    gaps: Mutex<Vec<(String, u64)>>,
    log: Mutex<Vec<String>>,
    pub input: Mutex<InputScript>,
    began: Instant,
    process: AtomicUsize,
    input_running: AtomicBool,
    /// GSP/HID shared-memory addresses found at map time, read by the input thread
    pub hid_address: AtomicU32,
    pub gsp_address: AtomicU32,
    pub gsp_event: AtomicU32,
    pub hid_event: AtomicU32,
    pub frames: AtomicU64,
    last_buttons: AtomicU64,
    /// the guest thread index of the last SendSyncRequest (APT's lock ownership needs it)
    pub caller_thread: AtomicUsize,
    /// the RomFS IVFC image (dump path, level-3 offset, size), served as the all-zero lowpath file
    romfs_image: Mutex<Option<(PathBuf, u64, u64)>>,
}
impl Host {
    pub fn new(data_dir: impl AsRef<Path>, script: Vec<InputStep>) -> Arc<Host> {
        let data = data_dir.as_ref().to_path_buf();
        Host::new_with(data.join("romfs"), data.join("save"), data.join("sdmc"), script)
    }

    pub fn new_with(
        romfs_dir: impl AsRef<Path>,
        save_dir: impl AsRef<Path>,
        sdmc_dir: impl AsRef<Path>,
        script: Vec<InputStep>,
    ) -> Arc<Host> {
        Arc::new(Host {
            romfs_dir: romfs_dir.as_ref().to_path_buf(),
            save_dir: save_dir.as_ref().to_path_buf(),
            sdmc_dir: sdmc_dir.as_ref().to_path_buf(),
            state: Mutex::new(State::new()),
            gaps: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
            input: Mutex::new(InputScript::new(script)),
            began: Instant::now(),
            process: AtomicUsize::new(0),
            input_running: AtomicBool::new(false),
            hid_address: AtomicU32::new(0),
            gsp_address: AtomicU32::new(0),
            gsp_event: AtomicU32::new(0),
            hid_event: AtomicU32::new(0),
            frames: AtomicU64::new(0),
            last_buttons: AtomicU64::new(0),
            caller_thread: AtomicUsize::new(0),
            romfs_image: Mutex::new(None),
        })
    }

    pub fn say(&self, line: String) {
        self.log.lock().unwrap().push(line);
    }

    /// the gap ledger: deduped, counted (the shape os's watch.rs keeps)
    pub fn gap(&self, line: impl Into<String>) {
        let line = line.into();
        let mut gaps = self.gaps.lock().unwrap();
        for (known, count) in gaps.iter_mut() {
            if *known == line {
                *count += 1;
                return;
            }
        }
        gaps.push((line, 1));
    }

    pub fn gaps(&self) -> Vec<(String, u64)> {
        self.gaps.lock().unwrap().clone()
    }

    pub fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.began.elapsed().as_millis() as u64
    }

    // ---------- sessions ----------

    pub fn connect(&self, name: &str) -> Option<u32> {
        let service = service_named(name)?;
        let mut state = self.state.lock().unwrap();
        let handle = state.fresh_handle();
        state.sessions.insert(handle, service);
        self.say(format!("srv: session {handle:#x} -> {name}"));
        Some(handle)
    }

    pub fn service_known(&self, name: &str) -> bool {
        service_named(name).is_some()
    }

    /// names the dump's RomFS IVFC level-3 image the SDK's all-zero lowpath file reads
    pub fn set_romfs_image(&self, path: impl Into<PathBuf>, base: u64, size: u64) {
        *self.romfs_image.lock().unwrap() = Some((path.into(), base, size));
    }

    pub fn romfs_image(&self) -> Option<(PathBuf, u64, u64)> {
        self.romfs_image.lock().unwrap().clone()
    }

    /// opens a raw file session over a source; the handle is the session
    pub fn open_file_session(&self, source: FileSource, path: String, writable: bool) -> (u32, u32) {
        let mut state = self.state.lock().unwrap();
        let id = state.fresh_id();
        state.files.insert(id, FileHandle { source, pos: 0, path, writable, dirty: false });
        let handle = state.fresh_handle();
        state.sessions.insert(handle, Service::File(id));
        (handle, id)
    }

    /// opens a read-only file session over a range of the dump (the RomFS image)
    pub fn open_image_session(&self, path: PathBuf, base: u64, size: u64) -> (u32, u32) {
        self.open_file_session(FileSource::Image { path, base, size }, String::new(), false)
    }

    pub fn open_dir_session(&self, entries: Vec<String>) -> (u32, u32) {
        let mut state = self.state.lock().unwrap();
        let id = state.fresh_id();
        state.dirs.insert(id, DirHandle { entries, index: 0 });
        let handle = state.fresh_handle();
        state.sessions.insert(handle, Service::Dir(id));
        (handle, id)
    }

    pub fn service_of(&self, handle: u32) -> Option<Service> {
        self.state.lock().unwrap().sessions.get(&handle).copied()
    }

    /// the APT lock is a service-owned sync object (the SDK releases it from several threads;
    /// runtime3ds's kernel mutex needs a same-thread acquire, so the service models it here)
    pub fn set_apt_lock(&self, handle: u32) {
        self.state.lock().unwrap().apt_lock = Some(handle);
    }

    pub fn apt_lock(&self) -> Option<u32> {
        self.state.lock().unwrap().apt_lock
    }

    pub fn is_apt_lock(&self, handle: u32) -> bool {
        self.state.lock().unwrap().apt_lock == Some(handle)
    }

    pub fn release_apt_lock(&self) {
        self.state.lock().unwrap().apt_lock = None;
    }

    pub fn close_session(&self, handle: u32) -> bool {
        if self.is_apt_lock(handle) {
            self.release_apt_lock();
            return true;
        }
        let mut state = self.state.lock().unwrap();
        if let Some(service) = state.sessions.remove(&handle) {
            match service {
                Service::File(id) => {
                    if let Some(file) = state.files.remove(&id) {
                        if file.dirty {
                            if let FileSource::Memory(bytes) = &file.source {
                                let _ = std::fs::write(&file.path, bytes);
                            }
                        }
                    }
                }
                Service::Dir(id) => {
                    state.dirs.remove(&id);
                }
                _ => {}
            }
            return true;
        }
        false
    }

    // ---------- IPC ----------

    pub fn send_sync(&self, process: &Process3ds, tls: u32, thread: usize, site: u32, session: u32) -> bool {
        self.caller_thread.store(thread, Ordering::Release);
        let request = read_request(process, tls);
        let Some(service) = self.service_of(session) else {
            self.gap(format!("SendSyncRequest on an unknown session {session:#x} at TLS {tls:#x}"));
            return false;
        };
        let reply = dispatch(self, process, tls, session, service, &request);
        self.say(format!("ipc {service:?} cmd {:#06x} -> {:#010x} at {site:#x}", request.cmd, reply.words[0]));
        write_reply(process, tls, request.cmd, &reply);
        true
    }

    /// the input thread: applies the script by wall time, writes the HID shared memory and signals
    /// the HID/GSP events so the title's wait/poll loop advances
    pub fn start_input(self: &Arc<Self>) {
        if self.input_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let host = Arc::clone(self);
        std::thread::Builder::new()
            .name("os3ds-input".into())
            .spawn(move || loop {
                let process = host.process.load(Ordering::Acquire);
                if process != 0 {
                    // safe: the harness keeps the process alive until start() joins every thread
                    let process = unsafe { &*(process as *const Process3ds) };
                    if process.stopped() {
                        break;
                    }
                    let elapsed = host.elapsed_ms();
                    let changed = host.input.lock().unwrap().advance(elapsed);
                    if changed {
                        host.write_pad(process);
                    }
                    // signal the HID update and the GSP "vblank" events every 16 ms
                    let hid_event = host.hid_event.load(Ordering::Relaxed);
                    if changed && hid_event != 0 {
                        process.signal_event(hid_event);
                    }
                    let gsp_event = host.gsp_event.load(Ordering::Relaxed);
                    if gsp_event != 0 && elapsed % 16 == 0 {
                        process.signal_event(gsp_event);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(4));
            })
            .ok();
    }

    /// attaches the process for the input thread (raw pointer; the boot outlives the thread)
    pub fn attach_process(&self, process: *const Process3ds) {
        self.process.store(process as usize, Ordering::Release);
    }

    /// writes the current pad state into the HID shared memory the guest mapped
    pub fn write_pad(&self, process: &Process3ds) {
        let address = self.hid_address.load(Ordering::Acquire);
        if address == 0 {
            return;
        }
        let pad = self.input.lock().unwrap().now;
        let memory = &process.memory;
        // current PAD state at 0x1C, raw circle pad at 0x20, and the 8-entry array at 0x28
        memory.write32(address + 0x18, 0); // 3D slider f32 0.0
        memory.write32(address + 0x1C, pad.buttons);
        memory.write32(address + 0x20, (pad.cx as u16 as u32) | ((pad.cy as u16 as u32) << 16));
        // entry 0: current, pressed, released, circle pad
        let previous_buttons = self.last_buttons.swap(pad.buttons as u64, Ordering::AcqRel) as u32;
        let pressed = pad.buttons & !previous_buttons;
        let released = previous_buttons & !pad.buttons;
        memory.write32(address + 0x28, pad.buttons);
        memory.write32(address + 0x2C, pressed);
        memory.write32(address + 0x30, released);
        memory.write32(
            address + 0x34,
            (pad.cx as u16 as u32) | ((pad.cy as u16 as u32) << 16),
        );
        // touch: raw entry at 0xA8+0x18 and pixel entries at 0xA8+0x20
        let (x, y, present) = pad.touch.map(|(x, y)| (x, y, 1u32)).unwrap_or((0, 0, 0));
        memory.write32(address + 0xA8 + 0x18, (x as u32) | ((y as u32) << 16));
        memory.write32(address + 0xA8 + 0x20, (x as u32) | ((y as u32) << 16));
        memory.write32(address + 0xA8 + 0x24, present);
        let mut input = self.input.lock().unwrap();
        input.updates += 1;
        self.frames.fetch_add(1, Ordering::Relaxed);
    }

    /// the serve counter: every read of the HID shared memory the service accounts for
    pub fn note_serve(&self) {
        self.input.lock().unwrap().serves += 1;
    }

    pub fn input_applied(&self) -> Vec<String> {
        self.input.lock().unwrap().applied.clone()
    }

    // ---------- shared memory ----------

    /// creates a host-owned shared block and returns the kernel block handle the guest maps
    pub fn register_shared(&self, process: &Process3ds, kind: SharedKind, size: u32) -> Option<u32> {
        let handle = process.create_memory_block(0, size);
        let mut state = self.state.lock().unwrap();
        state.shared.insert(handle, SharedBlock { bytes: vec![0; size as usize], kind });
        match kind {
            SharedKind::Hid => state.hid_shared = Some(handle),
            SharedKind::Gsp => state.gsp_shared = Some(handle),
            SharedKind::Other => {}
        }
        Some(handle)
    }

    pub fn map_block(&self, process: &Process3ds, handle: u32, address: u32) -> bool {
        let (size, kind, mut bytes) = {
            let state = self.state.lock().unwrap();
            let Some(block) = state.shared.get(&handle) else {
                return false;
            };
            (block.bytes.len() as u32, block.kind, block.bytes.clone())
        };
        let size = size.max(0x1000).next_multiple_of(0x1000);
        if address == 0 {
            return false;
        }
        if process
            .memory
            .map(address, size, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Shared, runtime3ds::mem::Kind::Shared)
            .is_err()
        {
            // the region may already be mapped by a previous map of the same block
            if process.memory.query(address).base != address {
                return false;
            }
        }
        process.memory.populate(address, size);
        if bytes.len() < size as usize {
            bytes.resize(size as usize, 0);
        }
        process.memory.write_bytes(address, &bytes);
        {
            let mut state = self.state.lock().unwrap();
            state.mapped.insert(handle, (address, size, kind));
        }
        match kind {
            SharedKind::Hid => {
                self.hid_address.store(address, Ordering::Release);
                self.write_pad(process);
            }
            SharedKind::Gsp => self.gsp_address.store(address, Ordering::Release),
            SharedKind::Other => {}
        }
        self.say(format!("MapMemoryBlock {handle:#x} at {address:#x} ({size:#x} bytes, {kind:?})"));
        true
    }

    pub fn unmap_block(&self, handle: u32) -> bool {
        let mut state = self.state.lock().unwrap();
        state.mapped.remove(&handle).is_some()
    }
}

/// reads a NUL-terminated port name from guest memory
pub(crate) fn read_cstr(process: &Process3ds, address: u32, max: usize) -> String {
    let mut bytes = Vec::new();
    for i in 0..max {
        let byte = process.memory.read8(address.wrapping_add(i as u32));
        if byte == 0 {
            break;
        }
        bytes.push(byte);
    }
    String::from_utf8_lossy(&bytes).trim_end_matches('\0').to_owned()
}

/// reads the IPC buffer at TLS+0x80
pub fn read_request(process: &Process3ds, tls: u32) -> Request {
    let base = tls.wrapping_add(0x80);
    let header = process.memory.read32(base);
    let cmd = header >> 16;
    let normal = ((header >> 6) & 0x3F) as usize;
    let translate = (header & 0x3F) as usize;
    let params = (0..normal)
        .map(|i| process.memory.read32(base.wrapping_add(4 + 4 * i as u32)))
        .collect();
    let start = 4 + 4 * normal as u32;
    let translated = (0..translate)
        .map(|i| process.memory.read32(base.wrapping_add(start + 4 * i as u32)))
        .collect();
    Request { cmd, normal, translate, params, translated }
}

/// writes the reply at TLS+0x80: header, result, the rest
pub fn write_reply(process: &Process3ds, tls: u32, cmd: u32, reply: &Reply) {
    let base = tls.wrapping_add(0x80);
    let normal = reply.words.len().saturating_sub(reply.translate) as u32;
    let header = (cmd << 16) | (normal << 6) | reply.translate as u32;
    process.memory.write32(base, header);
    for (i, word) in reply.words.iter().enumerate() {
        process.memory.write32(base.wrapping_add(4 + 4 * i as u32), *word);
    }
}

fn dispatch(
    host: &Host,
    process: &Process3ds,
    tls: u32,
    session: u32,
    service: Service,
    request: &Request,
) -> Reply {
    use crate::services;
    match service {
        Service::Srv => services::srv::command(host, process, tls, session, request),
        Service::ErrF => services::srv::errf_command(host, process, tls, request),
        Service::Apt => services::apt::command(host, process, tls, request),
        Service::Fs => services::fs::command(host, process, tls, request),
        Service::File(id) => services::fs::file_command(host, process, tls, id, request),
        Service::Dir(id) => services::fs::dir_command(host, process, tls, id, request),
        Service::Hid => services::hid::command(host, process, tls, request),
        Service::Gsp => services::gsp::command(host, process, tls, request),
        Service::Dsp => services::dsp::command(host, process, tls, request),
        Service::Cfg => services::cfg::command(host, process, tls, request),
        Service::Ptm => services::ptm::command(host, process, tls, request),
        Service::LdrRo => services::ldr_ro::command(host, process, tls, request),
        Service::Y2r => services::y2r::command(host, process, tls, request),
        Service::Soc => services::soc::command(host, process, tls, request),
        Service::Misc(name) => services::misc::command(host, process, tls, name, request),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_script_parses_the_named_keys() {
        let steps = parse_input("0 neutral\n2500 start\n5000 a+left\n6000 touch:160,120\n").unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].buttons, 0);
        assert_eq!(steps[1].at_ms, 2500);
        assert_eq!(steps[1].buttons, 1 << 3, "Start");
        assert_eq!(steps[2].buttons, (1 << 0) | (1 << 5), "A + Left");
        assert_eq!(steps[3].touch, Some((160, 120)));
    }

    #[test]
    fn the_script_advances_by_wall_time() {
        let mut script = InputScript::new(parse_input("0 neutral\n100 a\n200 start\n").unwrap());
        assert!(!script.advance(0) || script.cursor == 1);
        assert!(script.advance(99));
        assert_eq!(script.now.buttons, 0);
        assert!(script.advance(150));
        assert_eq!(script.now.buttons, 1 << 0);
        assert_eq!(script.applied.len(), 2);
        assert!(script.advance(250));
        assert_eq!(script.now.buttons, 1 << 3);
    }

    #[test]
    fn gaps_are_deduped_and_counted() {
        let host = Host::new(std::env::temp_dir(), Vec::new());
        host.gap("a missing thing");
        host.gap("a missing thing");
        let gaps = host.gaps();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].1, 2);
    }
}
