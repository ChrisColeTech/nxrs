//! The display service and the GPU driver's memory handles over IPC, for a client that does not go through nn::vi
//! and NVN (nn::vi is taken over by name, `vi.rs`): a raw vi client that draws on the CPU into nvmap memory and
//! queues it through the binder buffer queue (the way a software framebuffer works).
//!
//! - `nvdrv` / `nvdrv:a` / `nvdrv:s` / `nvdrv:t`: Open / Ioctl / Close / Initialize / QueryEvent. `/dev/nvmap` is
//!   real: Create (a handle of a size), Alloc (the client's own memory at an address behind it), FromId / GetId,
//!   Param (size, alignment, base, heap, kind), Free. `/dev/nvhost-ctrl` keeps the host's syncpoints: no GPU work of
//!   such a client increments them, so they read 0, a wait already reached succeeds and any other times out. Other
//!   devices do not open (NotSupported): their client is the NVN driver, which the host replaces.
//! - `vi:u` / `vi:s` / `vi:m` (0-3: IApplicationDisplayService): one display ("Default", the docked or handheld
//!   size), layers (OpenLayer, CreateStrayLayer) whose native window is the binder queue's parcel, the vsync event
//!   (signalled at each present), the relay service (IHOSBinderDriver), the system / manager display services, and
//!   the indirect layer's picture (2450 / 2451 transparent, 2460 its size).
//! - IHOSBinderDriver: TransactParcel(Auto) on the queue (Android's IGraphicBufferProducer): CONNECT, the buffer
//!   slots SET_PREALLOCATED_BUFFER registers (their GraphicBuffer: the nvmap id, format, size, pitch, layout and
//!   block height), REQUEST_BUFFER, DEQUEUE_BUFFER (a free slot, no fence), QUEUE_BUFFER (crop and transform kept;
//!   the slot's pixels, pitch-linear or block-linear, converted to RGBA8 and shown through graphics
//!   `present_rgba`), CANCEL_BUFFER, QUERY, DISCONNECT, GET_BUFFER_HISTORY (none); AdjustRefcount; GetNativeHandle
//!   (the buffer-available event).

use crate::ipc::{self, Ctx, Service};
use crate::kobj::{Event, SysEvent};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex};

// ---- nvdrv: nvmap and nvhost-ctrl

/// NvError: NotSupported, BadParameter, Timeout, NotInitialized? (the driver's own codes).
const NV_OK: u32 = 0;
const NV_NOT_SUPPORTED: u32 = 2;
const NV_BAD_VALUE: u32 = 4;
const NV_TIMEOUT: u32 = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NvMap {
    pub size: u32,
    pub align: u32,
    pub kind: u8,
    pub heap: u32,
    /// The client's memory behind it (0 until Alloc).
    pub addr: u64,
    pub id: u32,
}

struct Maps {
    by_handle: HashMap<u32, NvMap>,
    next: u32,
}

static MAPS: std::sync::LazyLock<Mutex<Maps>> = std::sync::LazyLock::new(|| {
    Mutex::new(Maps {
        by_handle: HashMap::new(),
        next: 1,
    })
});

/// The nvmap object an id names (vi's GraphicBuffer carries the id).
pub fn nvmap_by_id(id: u32) -> Option<NvMap> {
    MAPS.lock()
        .unwrap()
        .by_handle
        .values()
        .find(|m| m.id == id)
        .copied()
}

fn rd(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4)
        .map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn wr(b: &mut [u8], o: usize, v: u32) {
    if let Some(s) = b.get_mut(o..o + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

/// One nvmap ioctl on `io` (in and out, as the driver's in-out structure). Returns the NvError.
fn nvmap_ioctl(cmd: u32, io: &mut [u8]) -> u32 {
    let mut m = MAPS.lock().unwrap();
    match cmd {
        // Create {size; handle out}
        0xC008_0101 => {
            let size = rd(io, 0);
            if size == 0 {
                return NV_BAD_VALUE;
            }
            let h = m.next;
            m.next += 1;
            // ids are the handles' own numbers here (the driver's ids are global, handles per client)
            m.by_handle.insert(
                h,
                NvMap {
                    size,
                    align: 0x1000,
                    id: h,
                    ..Default::default()
                },
            );
            wr(io, 4, h);
            NV_OK
        }
        // FromId {id; handle out}
        0xC008_0103 => match m.by_handle.iter().find(|(_, v)| v.id == rd(io, 0)) {
            Some((h, _)) => {
                let h = *h;
                wr(io, 4, h);
                NV_OK
            }
            None => NV_BAD_VALUE,
        },
        // Alloc {handle, heap mask, flags, align, kind, pad[7], address}
        0xC020_0104 => {
            let h = rd(io, 0);
            let addr = u64::from_le_bytes(io[0x18..0x20].try_into().unwrap());
            let Some(o) = m.by_handle.get_mut(&h) else {
                return NV_BAD_VALUE;
            };
            o.heap = rd(io, 4);
            o.align = rd(io, 0xc).max(0x1000);
            o.kind = io[0x10];
            o.addr = addr;
            NV_OK
        }
        // Free {handle, pad; address, size, flags out}
        0xC018_0105 => {
            let Some(o) = m.by_handle.remove(&rd(io, 0)) else {
                return NV_BAD_VALUE;
            };
            io[8..16].copy_from_slice(&o.addr.to_le_bytes());
            wr(io, 0x10, o.size);
            wr(io, 0x14, 0);
            NV_OK
        }
        // Param {handle, param; result out}: 1 size, 2 alignment, 3 base, 4 heap, 5 kind, 6 compression
        0xC00C_0109 => {
            let Some(o) = m.by_handle.get(&rd(io, 0)).copied() else {
                return NV_BAD_VALUE;
            };
            let v = match rd(io, 4) {
                1 => o.size,
                2 => o.align,
                3 => o.addr as u32,
                4 => o.heap,
                5 => o.kind as u32,
                6 => 0,
                _ => return NV_BAD_VALUE,
            };
            wr(io, 8, v);
            NV_OK
        }
        // GetId {id out; handle}
        0xC008_010E => {
            let Some(o) = m.by_handle.get(&rd(io, 4)).copied() else {
                return NV_BAD_VALUE;
            };
            wr(io, 0, o.id);
            NV_OK
        }
        _ => {
            crate::log_once!(
                format!("nvmap {cmd:#x}"),
                "[RED] nvdrv: /dev/nvmap ioctl {cmd:#x} is not served"
            );
            NV_NOT_SUPPORTED
        }
    }
}

/// nvhost-ctrl: the host's syncpoints (all 0: no GPU work of an IPC client increments them).
fn ctrl_ioctl(cmd: u32, io: &mut [u8]) -> u32 {
    match cmd {
        // SyncptRead {id; value out}
        0xC008_0014 => {
            wr(io, 4, 0);
            NV_OK
        }
        // SyncptWait {id, threshold, timeout} / SyncptWaitEx (+ value out): reached when the threshold is 0
        0xC00C_0016 | 0xC010_0019 => {
            if rd(io, 4) == 0 {
                wr(io, 0xc, 0);
                NV_OK
            } else {
                NV_TIMEOUT
            }
        }
        // EventWait(Async) {fence id, value, timeout, event}
        0xC010_001D | 0xC010_001E => {
            if rd(io, 4) == 0 {
                NV_OK
            } else {
                NV_TIMEOUT
            }
        }
        _ => {
            crate::log_once!(
                format!("nvhost-ctrl {cmd:#x}"),
                "[RED] nvdrv: /dev/nvhost-ctrl ioctl {cmd:#x} is not served"
            );
            NV_NOT_SUPPORTED
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dev {
    Map,
    Ctrl,
}

struct NvDrv {
    name: &'static str,
    fds: HashMap<u32, Dev>,
    next: u32,
    events: Vec<Event>,
}

impl Service for NvDrv {
    fn name(&self) -> String {
        self.name.into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // Open(path) -> fd, error
            0 => {
                let p = ctx.read_in(0);
                let path = String::from_utf8_lossy(
                    &p[..p.iter().position(|&b| b == 0).unwrap_or(p.len())],
                )
                .to_string();
                let dev = match path.as_str() {
                    "/dev/nvmap" => Some(Dev::Map),
                    "/dev/nvhost-ctrl" => Some(Dev::Ctrl),
                    _ => None,
                };
                match dev {
                    Some(d) => {
                        let fd = self.next;
                        self.next += 1;
                        self.fds.insert(fd, d);
                        ctx.push_u32(fd);
                        ctx.push_u32(NV_OK);
                    }
                    None => {
                        crate::log_once!(format!("nvdrv open {path}"), "nvdrv: {path} does not open (NotSupported: the NVN driver's devices are the host's graphics)");
                        ctx.push_u32(u32::MAX);
                        ctx.push_u32(NV_NOT_SUPPORTED);
                    }
                }
                0
            }
            // Ioctl(fd, command; in, out) -> error (Ioctl2 / Ioctl3: 11 / 12, extra buffers unused here)
            1 | 11 | 12 => {
                let (fd, cmd) = (ctx.u32(0), ctx.u32(4));
                let size = ((cmd >> 16) & 0x3fff) as usize;
                let mut io = ctx.read_in(0);
                io.resize(size.max(io.len()), 0);
                let r = match self.fds.get(&fd) {
                    Some(Dev::Map) => nvmap_ioctl(cmd, &mut io),
                    Some(Dev::Ctrl) => ctrl_ioctl(cmd, &mut io),
                    None => NV_BAD_VALUE,
                };
                ctx.write_out(0, &io[..size.min(io.len())]);
                ctx.push_u32(r);
                0
            }
            // Close(fd)
            2 => {
                let r = if self.fds.remove(&ctx.u32(0)).is_some() {
                    NV_OK
                } else {
                    NV_BAD_VALUE
                };
                ctx.push_u32(r);
                0
            }
            // Initialize(transfer memory size; process, transfer memory) / SetAruid / the firmware margin
            3 | 8 | 13 => {
                ctx.push_u32(NV_OK);
                0
            }
            // QueryEvent(fd, event id): an event the client may wait on; nothing of the host's fires it
            4 => {
                let e = Event::new();
                ctx.copy_handle(e.readable);
                self.events.push(e);
                ctx.push_u32(NV_OK);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

// ---- the buffer queue (IGraphicBufferProducer over binder parcels)

/// A slot's buffer, from its GraphicBuffer.
#[derive(Clone, Debug, Default)]
pub struct Slot {
    /// The flattened GraphicBuffer as the client gave it (REQUEST_BUFFER gives it back).
    pub raw: Vec<u8>,
    /// Android pixel format (1 RGBA8888, 2 RGBX8888, 4 RGB565, 5 BGRA8888).
    pub format: u32,
    pub nvmap_id: u32,
    pub width: u32,
    pub height: u32,
    /// Bytes per row (pitch layout) / the block-linear surface's row bytes.
    pub pitch: u32,
    pub offset: u32,
    /// 1 pitch, 3 block-linear.
    pub layout: u32,
    pub block_height_log2: u32,
    dequeued: bool,
}

impl Slot {
    /// From a flattened GraphicBuffer's data: format at +0x10; the NvGraphicBuffer's nvmap id at +0x2c; its first
    /// plane at +0x5c: width, height, colour format (8 bytes), layout +0x10, pitch +0x14, offset +0x1c, block height
    /// (log 2) +0x24.
    pub fn parse(g: &[u8]) -> Option<Slot> {
        if g.len() < 0x5c + 0x28 {
            return None;
        }
        let pl = 0x5c;
        Some(Slot {
            raw: g.to_vec(),
            format: rd(g, 0x10),
            nvmap_id: rd(g, 0x2c),
            width: rd(g, pl),
            height: rd(g, pl + 4),
            layout: rd(g, pl + 0x10),
            pitch: rd(g, pl + 0x14),
            offset: rd(g, pl + 0x1c),
            block_height_log2: rd(g, pl + 0x24),
            dequeued: false,
        })
    }
}

/// The byte offset of (x bytes, y) in a block-linear surface: 64-byte x 8-row GOBs of 512 bytes, `1 << bhl` GOBs
/// stacked in a block, blocks in rows of `ceil(row_bytes / 64)`.
pub fn block_linear_offset(xb: u32, y: u32, row_bytes: u32, bhl: u32) -> usize {
    let bh = 1u32 << bhl.min(5);
    let gobs_x = row_bytes.div_ceil(64);
    let block = (y / (8 * bh)) * gobs_x + xb / 64;
    let in_block = (y % (8 * bh)) / 8;
    let (x, yy) = (xb % 64, y % 8);
    let in_gob = (x / 32) * 256 + (yy / 2) * 64 + ((x % 32) / 16) * 32 + (yy % 2) * 16 + (x % 16);
    (block * 512 * bh + in_block * 512 + in_gob) as usize
}

/// The slot's picture as RGBA8 rows (top first), read from `mem` (the nvmap memory from the plane's offset).
pub fn to_rgba(s: &Slot, mem: &[u8]) -> Option<Vec<u8>> {
    let bpp = match s.format {
        1 | 2 | 5 => 4,
        4 => 2,
        _ => return None,
    };
    let (w, h) = (s.width, s.height);
    let row_bytes = if s.pitch != 0 { s.pitch } else { w * bpp };
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let xb = x * bpp;
            let o = if s.layout == 3 {
                block_linear_offset(xb, y, row_bytes, s.block_height_log2)
            } else {
                (y * row_bytes + xb) as usize
            };
            let p = mem.get(o..o + bpp as usize)?;
            let d = ((y * w + x) * 4) as usize;
            let px: [u8; 4] = match s.format {
                1 => [p[0], p[1], p[2], p[3]],
                2 => [p[0], p[1], p[2], 255],
                5 => [p[2], p[1], p[0], p[3]],
                _ => {
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
                    [
                        (r * 255 / 31) as u8,
                        (g * 255 / 63) as u8,
                        (b * 255 / 31) as u8,
                        255,
                    ]
                }
            };
            out[d..d + 4].copy_from_slice(&px);
        }
    }
    Some(out)
}

/// Where a queued picture goes: graphics' window (tests take it themselves).
type Sink = fn(u32, u32, &[u8]) -> u64;
static SINK: Mutex<Sink> = Mutex::new(present_to_window);

fn present_to_window(w: u32, h: u32, rgba: &[u8]) -> u64 {
    nxgfx::vk::present::present_rgba(w, h, rgba)
}

struct Queue {
    slots: Vec<Option<Slot>>,
    connected: bool,
    /// The last crop (left, top, right, bottom) and transform queued.
    crop: [i32; 4],
    transform: u32,
    presents: u64,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    slots: Vec::new(),
    connected: false,
    crop: [0; 4],
    transform: 0,
    presents: 0,
});
static FREED: Condvar = Condvar::new();
static BUFFER_FREE: SysEvent = SysEvent::new();
static VSYNC: SysEvent = SysEvent::new();

fn display_size() -> (u32, u32) {
    if crate::services::oe::opmode() == 1 {
        (1920, 1080)
    } else {
        (1280, 720)
    }
}

/// A parcel being read: its data, the position.
struct In<'a> {
    d: &'a [u8],
    pos: usize,
}

impl In<'_> {
    fn u32(&mut self) -> u32 {
        let v = rd(self.d, self.pos);
        self.pos += 4;
        v
    }
    /// The interface token: strict-mode policy, then a String16 (length, characters with a NUL, 4-aligned).
    fn token(&mut self) {
        self.u32();
        let n = self.u32() as i32;
        if n > 0 {
            self.pos += ((n as usize + 1) * 2 + 3) & !3;
        }
    }
    /// A flattened object: (length, fd count), then its bytes (4-aligned).
    fn flat(&mut self) -> &[u8] {
        let n = self.u32() as usize;
        self.u32();
        let s = &self.d[self.pos.min(self.d.len())..(self.pos + n).min(self.d.len())];
        self.pos += (n + 3) & !3;
        s
    }
}

#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.u32(v as u32)
    }
    fn flat(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.u32(0);
        self.0.extend_from_slice(b);
        while self.0.len() % 4 != 0 {
            self.0.push(0);
        }
    }
    /// QueueBufferOutput: width, height, transform hint, pending buffers.
    fn qbo(&mut self) {
        let (w, h) = display_size();
        for v in [w, h, 0, 0] {
            self.u32(v);
        }
    }
    /// The parcel: header {data size, data offset 0x10, objects size 0, objects offset}, then the data.
    fn parcel(&self) -> Vec<u8> {
        let n = self.0.len() as u32;
        let mut p = Vec::with_capacity(0x10 + self.0.len());
        for v in [n, 0x10, 0, 0x10 + n] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.extend_from_slice(&self.0);
        p
    }
}

/// Android status codes the queue answers.
const BAD_VALUE: i32 = -22;
const NO_INIT: i32 = -19;
const WOULD_BLOCK: i32 = -11;

/// Shows the slot's picture; the status.
fn present_slot(s: &Slot) -> i32 {
    let Some(m) = nvmap_by_id(s.nvmap_id) else {
        crate::log_once!(
            "binder nvmap",
            "[RED] vi: a queued buffer names nvmap id {} the client never made",
            s.nvmap_id
        );
        return BAD_VALUE;
    };
    if m.addr == 0 || s.offset as u64 >= m.size as u64 {
        return BAD_VALUE;
    }
    let mem = crate::guest::bytes(m.addr + s.offset as u64, (m.size - s.offset) as u64);
    let Some(rgba) = to_rgba(s, mem) else {
        crate::log_once!(
            format!("binder fmt {}", s.format),
            "[RED] vi: a queued buffer of pixel format {} ({}x{}, layout {}) is not shown",
            s.format,
            s.width,
            s.height,
            s.layout
        );
        return BAD_VALUE;
    };
    let sink = *SINK.lock().unwrap();
    sink(s.width, s.height, &rgba);
    crate::nvn::note_present(0, 0);
    VSYNC.signal();
    0
}

/// One IGraphicBufferProducer transaction: the reply's data.
fn transact(code: u32, input: &[u8]) -> Vec<u8> {
    // the parcel: header {data size, data offset, ...}
    let (n, off) = (rd(input, 0) as usize, rd(input, 4) as usize);
    let data = input.get(off..off + n).unwrap_or(&[]);
    let mut r = In { d: data, pos: 0 };
    r.token();
    let mut o = Out::default();
    let mut q = QUEUE.lock().unwrap();
    if q.slots.len() < 64 {
        q.slots.resize(64, None);
    }
    match code {
        // REQUEST_BUFFER(slot) -> non-null, GraphicBuffer, status
        1 => {
            let i = r.u32() as usize;
            match q.slots.get(i).and_then(|s| s.as_ref()) {
                Some(s) => {
                    o.u32(1);
                    o.flat(&s.raw);
                }
                None => o.u32(0),
            }
            o.i32(0);
        }
        // SET_BUFFER_COUNT, the disconnect / allocate forms
        2 | 12 | 13 => o.i32(0),
        // DEQUEUE_BUFFER(format, width, height, timestamps, usage) -> slot, a fence (none), status
        3 => {
            if !q.connected {
                o.i32(NO_INIT);
                return o.parcel();
            }
            let pick = |q: &Queue| {
                q.slots
                    .iter()
                    .position(|s| s.as_ref().is_some_and(|s| !s.dequeued))
            };
            let mut slot = pick(&q);
            if slot.is_none() && q.slots.iter().any(|s| s.is_some()) {
                q = FREED
                    .wait_timeout(q, std::time::Duration::from_millis(100))
                    .unwrap()
                    .0;
                slot = pick(&q);
            }
            match slot {
                Some(i) => {
                    q.slots[i].as_mut().unwrap().dequeued = true;
                    o.i32(i as i32);
                    o.u32(1);
                    // NvMultiFence: 0 fences (each {id 0xffffffff, value 0} unused)
                    let mut f = vec![0u8; 0x24];
                    for k in 0..4 {
                        f[4 + 8 * k..8 + 8 * k].copy_from_slice(&u32::MAX.to_le_bytes());
                    }
                    o.flat(&f);
                    o.i32(0);
                }
                None => o.i32(WOULD_BLOCK),
            }
        }
        // QUEUE_BUFFER(slot, QueueBufferInput) -> QueueBufferOutput, status
        7 => {
            let i = r.u32() as usize;
            let qi = r.flat().to_vec();
            if qi.len() >= 0x24 {
                q.crop = [
                    rd(&qi, 0xc) as i32,
                    rd(&qi, 0x10) as i32,
                    rd(&qi, 0x14) as i32,
                    rd(&qi, 0x18) as i32,
                ];
                q.transform = rd(&qi, 0x20);
            }
            let status = match q.slots.get_mut(i).and_then(|s| s.as_mut()) {
                Some(s) if s.dequeued => {
                    s.dequeued = false;
                    let s = s.clone();
                    q.presents += 1;
                    if q.presents == 1 {
                        crate::log!("vi: first buffer queued through the binder queue: {}x{} format {} layout {} (block height {})", s.width, s.height, s.format, s.layout, 1 << s.block_height_log2);
                    }
                    drop(q);
                    let st = present_slot(&s);
                    st
                }
                _ => BAD_VALUE,
            };
            FREED.notify_all();
            BUFFER_FREE.signal();
            o.qbo();
            o.i32(status);
        }
        // CANCEL_BUFFER(slot, fence)
        8 => {
            let i = r.u32() as usize;
            if let Some(s) = q.slots.get_mut(i).and_then(|s| s.as_mut()) {
                s.dequeued = false;
            }
            FREED.notify_all();
            BUFFER_FREE.signal();
            o.i32(0);
        }
        // QUERY(what) -> value, status: 0 width, 1 height, 2 format, 3 min undequeued buffers
        9 => {
            let (w, h) = display_size();
            let v = match r.u32() {
                0 => w,
                1 => h,
                2 => 1,
                3 => 1,
                _ => 0,
            };
            o.u32(v);
            o.i32(0);
        }
        // CONNECT(listener, api, controlled by app) -> QueueBufferOutput, status
        10 => {
            q.connected = true;
            o.qbo();
            o.i32(0);
        }
        // DISCONNECT
        11 => {
            q.connected = false;
            for s in q.slots.iter_mut().flatten() {
                s.dequeued = false;
            }
            o.i32(0);
        }
        // SET_PREALLOCATED_BUFFER(slot, has buffer, GraphicBuffer)
        14 => {
            let i = r.u32() as usize;
            let has = r.u32();
            if i >= 64 {
                o.i32(BAD_VALUE);
                return o.parcel();
            }
            q.slots[i] = if has != 0 {
                Slot::parse(r.flat())
            } else {
                None
            };
            if let Some(s) = &q.slots[i] {
                crate::log!("vi: buffer slot {i}: {}x{} format {} pitch {} layout {} block height {} nvmap id {}", s.width, s.height, s.format, s.pitch, s.layout, 1 << s.block_height_log2, s.nvmap_id);
            }
            o.i32(0);
        }
        // GET_BUFFER_HISTORY: none
        17 => {
            o.u32(0);
            o.i32(0);
        }
        c => {
            crate::log_once!(
                format!("binder {c}"),
                "[RED] vi: binder transaction {c} is not served (status BAD_VALUE)"
            );
            o.i32(BAD_VALUE);
        }
    }
    o.parcel()
}

struct Binder;

impl Service for Binder {
    fn name(&self) -> String {
        "IHOSBinderDriver".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // TransactParcel(Auto)(binder id, code, flags; parcel in, parcel out)
            0 | 3 => {
                let code = ctx.u32(4);
                let reply = transact(code, &ctx.read_in(0));
                ctx.write_out(0, &reply);
                0
            }
            // AdjustRefcount
            1 => 0,
            // GetNativeHandle(binder id, kind): the buffer-available event
            2 => {
                ctx.copy_handle(BUFFER_FREE.handout());
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

// ---- the display services

/// The native window of a layer: a parcel holding the flat binder object {type 2, 1, binder id, 0..., "dispdrv"}.
fn native_window(binder: u32) -> Vec<u8> {
    let mut o = Out::default();
    let mut d = [0u8; 0x28];
    d[0..4].copy_from_slice(&2u32.to_le_bytes());
    d[4..8].copy_from_slice(&1u32.to_le_bytes());
    d[8..12].copy_from_slice(&binder.to_le_bytes());
    d[0x18..0x20].copy_from_slice(b"dispdrv\0");
    o.0.extend_from_slice(&d);
    let mut p = o.parcel();
    // one object, at the start of the data
    p[8..12].copy_from_slice(&4u32.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p
}

const BINDER_ID: u32 = 1;

struct AppDisplay;

impl Service for AppDisplay {
    fn name(&self) -> String {
        "IApplicationDisplayService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        let (w, h) = display_size();
        match ctx.cmd() {
            // GetRelayService / GetIndirectDisplayTransactionService
            100 | 103 => ctx.object(Binder),
            101 => ctx.object(SysDisplay),
            102 => ctx.object(MgrDisplay),
            // ListDisplays -> DisplayInfo {name[0x40], has limited layers, max layers, width, height}
            1000 => {
                let mut b = [0u8; 0x60];
                b[..7].copy_from_slice(b"Default");
                b[0x40] = 1;
                b[0x48..0x50].copy_from_slice(&1u64.to_le_bytes());
                b[0x50..0x58].copy_from_slice(&(w as u64).to_le_bytes());
                b[0x58..0x60].copy_from_slice(&(h as u64).to_le_bytes());
                let n = (ctx.write_out(0, &b) >= 0x60) as u64;
                ctx.push_u64(n);
            }
            // OpenDisplay(name) -> display id; OpenDefaultDisplay
            1010 => {
                if !ctx.input().starts_with(b"Default\0") {
                    // vi ResultNotFound (114, 7)
                    return 114 | 7 << 9;
                }
                ctx.push_u64(1);
            }
            1011 => ctx.push_u64(1),
            1020 | 1101 | 2021 | 2031 | 2101 => {}
            // GetDisplayResolution
            1102 => {
                ctx.push_u64(w as u64);
                ctx.push_u64(h as u64);
            }
            // OpenLayer(name, layer id, aruid; native window out) -> its size
            2020 => {
                let n = ctx.write_out(0, &native_window(BINDER_ID));
                ctx.push_u64(n as u64);
            }
            // CreateStrayLayer(flags, display id) -> layer id, native window size
            2030 => {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                ctx.push_u64(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                let n = ctx.write_out(0, &native_window(BINDER_ID));
                ctx.push_u64(n as u64);
            }
            // ConvertScalingMode: the client's mode in its service numbering
            2102 => {
                let m = ctx.u32(0);
                ctx.push_u64(match m {
                    0 => 3,
                    1 => 0,
                    2 => 1,
                    3 => 2,
                    _ => 4,
                });
            }
            // GetIndirectLayerImageMap / CropMap: transparent (the host's keyboard is a window of its own)
            2450 | 2451 => {
                let n = ctx.buf_out(0).size;
                if n != 0 && n <= 64 << 20 {
                    ctx.write_out(0, &vec![0; n as usize]);
                }
                ctx.push_u64(0);
                ctx.push_u64(0);
            }
            // GetIndirectLayerImageRequiredMemoryInfo(width, height) -> size, alignment
            2460 => {
                let (iw, ih) = (ctx.u64(0), ctx.u64(8));
                let sz = if iw == 0 || ih == 0 || iw > 8192 || ih > 8192 {
                    0x1000
                } else {
                    ((iw * 4 + 63) & !63) * ((ih + 127) & !127)
                };
                ctx.push_u64((sz + 0x1ffff) & !0x1ffff);
                ctx.push_u64(0x1000);
            }
            // GetDisplayVsyncEvent(ForDebug): signalled at each present
            5202 | 5203 => ctx.copy_handle(VSYNC.handout()),
            _ => return ctx.unimplemented(),
        }
        0
    }
}

struct SysDisplay;

impl Service for SysDisplay {
    fn name(&self) -> String {
        "ISystemDisplayService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        let (w, h) = display_size();
        match ctx.cmd() {
            // GetZOrderCountMin / Max
            1200 => ctx.push_u64(0),
            1202 => ctx.push_u64(255),
            1203 => {
                ctx.push_u64(w as u64);
                ctx.push_u64(h as u64);
            }
            // GetDisplayMode: width, height, refresh rate (60.0), unknown
            3200 => {
                ctx.push_u32(w);
                ctx.push_u32(h);
                ctx.push_u32(60f32.to_bits());
                ctx.push_u32(0);
            }
            2205 | 2207 => {}
            _ => return ctx.unimplemented(),
        }
        0
    }
}

struct MgrDisplay;

impl Service for MgrDisplay {
    fn name(&self) -> String {
        "IManagerDisplayService".into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // CreateManagedLayer -> layer id
            2010 => {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100);
                ctx.push_u64(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                0
            }
            2011 | 6000 | 6002 => 0,
            _ => ctx.unimplemented(),
        }
    }
}

struct Vi(&'static str);

impl Service for Vi {
    fn name(&self) -> String {
        self.0.into()
    }
    fn call(&mut self, ctx: &mut Ctx) -> u32 {
        match ctx.cmd() {
            // GetDisplayService (0 vi:u, 1 vi:s, 2 vi:m, 3 the privileged form)
            0..=3 => {
                ctx.object(AppDisplay);
                0
            }
            _ => ctx.unimplemented(),
        }
    }
}

pub fn register() {
    for n in ["nvdrv", "nvdrv:a", "nvdrv:s", "nvdrv:t"] {
        ipc::register(n, || {
            Box::new(NvDrv {
                name: "nvdrv",
                fds: HashMap::new(),
                next: 1,
                events: vec![],
            })
        });
    }
    ipc::register("vi:u", || Box::new(Vi("vi:u")));
    ipc::register("vi:s", || Box::new(Vi("vi:s")));
    ipc::register("vi:m", || Box::new(Vi("vi:m")));
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
    fn bin(v: &[u8]) -> Vec<Buf> {
        vec![Buf {
            addr: v.as_ptr() as u64,
            size: v.len() as u64,
            tag: 0,
        }]
    }
    fn bout(v: &mut [u8]) -> Vec<Buf> {
        vec![Buf {
            addr: v.as_mut_ptr() as u64,
            size: v.len() as u64,
            tag: 0,
        }]
    }
    fn u32s(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    fn ioctl(h: u32, fd: u32, cmd: u32, io: &mut [u8]) -> u32 {
        let src = io.to_vec();
        let r = send(h, 1, u32s(&[fd, cmd]), bin(&src), bout(io));
        u32::from_le_bytes(r.data[..4].try_into().unwrap())
    }

    /// The block-linear address: a GOB's 16-byte pieces as the hardware lays them out (row pairs, 32-byte halves),
    /// and blocks of `1 << bhl` GOBs.
    #[test]
    fn block_linear_addresses() {
        assert_eq!(block_linear_offset(0, 0, 256, 0), 0);
        assert_eq!(block_linear_offset(16, 0, 256, 0), 32);
        assert_eq!(block_linear_offset(0, 1, 256, 0), 16);
        assert_eq!(block_linear_offset(32, 0, 256, 0), 256);
        assert_eq!(block_linear_offset(0, 2, 256, 0), 64);
        assert_eq!(
            block_linear_offset(64, 0, 256, 0),
            512,
            "the next GOB to the right"
        );
        assert_eq!(
            block_linear_offset(0, 8, 256, 0),
            4 * 512,
            "a row of 4 GOBs"
        );
        assert_eq!(
            block_linear_offset(0, 8, 256, 1),
            512,
            "the second GOB of a 2-GOB block"
        );
        assert_eq!(block_linear_offset(64, 0, 256, 1), 1024);
    }

    static SEEN: Mutex<Vec<(u32, u32, Vec<u8>)>> = Mutex::new(Vec::new());
    fn catch(w: u32, h: u32, rgba: &[u8]) -> u64 {
        SEEN.lock().unwrap().push((w, h, rgba.to_vec()));
        1
    }

    /// A raw vi client end to end, as a software framebuffer drives it: nvdrv opens /dev/nvmap, makes a handle over
    /// its own memory and its id; vi opens the default display and a stray layer (the native window names binder 1);
    /// the binder queue connects, takes a block-linear BGRA slot, dequeues it, the client draws, queues it, and the
    /// picture shown (graphics' entry taken by the test) is the client's pixels as RGBA; then cancel / query /
    /// request / an unknown nvmap id; the vsync event fires; nvhost-ctrl's syncpoints read 0.
    #[test]
    fn raw_vi_client_presents_through_the_binder_queue() {
        crate::mem::tests::test_kernel();
        register();
        *SINK.lock().unwrap() = catch;
        let nv = crate::ipc::connect("nvdrv").unwrap();
        let path = b"/dev/nvmap\0";
        let r = send(nv, 0, vec![], bin(path), vec![]);
        let fd = u32::from_le_bytes(r.data[..4].try_into().unwrap());
        assert_eq!(&r.data[4..8], &[0; 4]);
        let r = send(nv, 0, vec![], bin(b"/dev/nvhost-gpu\0"), vec![]);
        assert_eq!(
            u32::from_le_bytes(r.data[4..8].try_into().unwrap()),
            NV_NOT_SUPPORTED
        );
        // a 64x16 BGRA surface, block-linear, block height 2 GOBs: 256 bytes a row, 2 blocks of 2 GOBs down
        let (w, h) = (64u32, 16u32);
        let mut mem = vec![0u8; 0x8000];
        let mut io = u32s(&[mem.len() as u32, 0]);
        assert_eq!(ioctl(nv, fd, 0xC008_0101, &mut io), 0);
        let handle = rd(&io, 4);
        let mut io = vec![0u8; 0x20];
        io[..4].copy_from_slice(&handle.to_le_bytes());
        io[0x18..0x20].copy_from_slice(&(mem.as_ptr() as u64).to_le_bytes());
        assert_eq!(ioctl(nv, fd, 0xC020_0104, &mut io), 0);
        let mut io = u32s(&[0, handle]);
        assert_eq!(ioctl(nv, fd, 0xC008_010E, &mut io), 0);
        let id = rd(&io, 0);
        let mut io = u32s(&[handle, 1, 0]);
        assert_eq!(ioctl(nv, fd, 0xC00C_0109, &mut io), 0);
        assert_eq!(rd(&io, 8), 0x8000, "Param size");
        let ctrl = u32::from_le_bytes(
            send(nv, 0, vec![], bin(b"/dev/nvhost-ctrl\0"), vec![]).data[..4]
                .try_into()
                .unwrap(),
        );
        let mut io = u32s(&[7, 0, 0, 0]);
        assert_eq!(
            ioctl(nv, ctrl, 0xC010_0019, &mut io),
            0,
            "a wait already reached"
        );
        let mut io = u32s(&[7, 5, 0, 0]);
        assert_eq!(ioctl(nv, ctrl, 0xC010_0019, &mut io), NV_TIMEOUT);

        let vi = crate::ipc::connect("vi:u").unwrap();
        let ads = send(vi, 0, vec![0; 4], vec![], vec![]).moved[0];
        let mut name = vec![0u8; 0x40];
        name[..7].copy_from_slice(b"Default");
        assert_eq!(
            u64::from_le_bytes(
                send(ads, 1010, name, vec![], vec![]).data[..8]
                    .try_into()
                    .unwrap()
            ),
            1
        );
        let mut nw = vec![0u8; 0x100];
        let r = send(
            ads,
            2030,
            [0u64.to_le_bytes(), 1u64.to_le_bytes()].concat(),
            vec![],
            bout(&mut nw),
        );
        assert_eq!(u64::from_le_bytes(r.data[8..16].try_into().unwrap()), 0x3c);
        assert_eq!(
            (rd(&nw, 0x10), rd(&nw, 0x18), &nw[0x28..0x30]),
            (2, BINDER_ID, &b"dispdrv\0"[..])
        );
        let vsync = send(ads, 5202, 1u64.to_le_bytes().to_vec(), vec![], vec![]).copy[0];
        let binder = send(ads, 100, vec![], vec![], vec![]).moved[0];

        let tx = |code: u32, body: &[u8]| -> Vec<u8> {
            let mut d = Out::default();
            d.u32(0x100);
            d.u32(11);
            d.0.extend_from_slice(&[0u8; 24]); // "android.gui.IGraphicBufferProducer" stand-in: 11 chars + NUL
            d.0.extend_from_slice(body);
            let p = d.parcel();
            let mut out = vec![0u8; 0x200];
            let r = send(
                binder,
                0,
                u32s(&[BINDER_ID, code, 0]),
                bin(&p),
                bout(&mut out),
            );
            assert_eq!(r.result, 0);
            let n = rd(&out, 0) as usize;
            out[0x10..0x10 + n].to_vec()
        };
        // the status is the reply's last word
        let status = |r: &[u8]| rd(r, r.len() - 4) as i32;
        assert_eq!(
            status(&tx(3, &u32s(&[1, w, h, 0, 0x100]))),
            NO_INIT,
            "dequeue before connect"
        );
        let r = tx(10, &u32s(&[0, 2, 0]));
        assert_eq!((rd(&r, 0), status(&r)), (display_size().0, 0));
        // GraphicBuffer: format 5 (BGRA) at +0x10, nvmap id at +0x2c, plane at +0x5c
        let mut gb = vec![0u8; 0x16c];
        gb[..4].copy_from_slice(b"GBFR");
        gb[0x10..0x14].copy_from_slice(&5u32.to_le_bytes());
        gb[0x2c..0x30].copy_from_slice(&id.to_le_bytes());
        for (o, v) in [
            (0x5c, w),
            (0x60, h),
            (0x6c, 3),
            (0x70, w * 4),
            (0x78, 0),
            (0x80, 1),
        ] {
            gb[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut body = u32s(&[0, 1, gb.len() as u32, 0]);
        body.extend_from_slice(&gb);
        assert_eq!(status(&tx(14, &body)), 0);
        let r = tx(3, &u32s(&[1, w, h, 0, 0x100]));
        assert_eq!(
            (rd(&r, 0), rd(&r, 4), status(&r)),
            (0, 1, 0),
            "slot 0, a fence"
        );
        let r = tx(1, &u32s(&[0]));
        assert_eq!(
            (rd(&r, 0), rd(&r, 4) as usize, &r[0xc..0x10]),
            (1, gb.len(), &b"GBFR"[..]),
            "REQUEST_BUFFER gives the GraphicBuffer back"
        );
        // the client draws: pixel (x, y) = BGRA (x, y, 7, 255)
        for y in 0..h {
            for x in 0..w {
                let o = block_linear_offset(x * 4, y, w * 4, 1);
                mem[o..o + 4].copy_from_slice(&[7, y as u8, x as u8, 255]);
            }
        }
        let mut qi = vec![0u8; 0x54];
        qi[0x0c..0x1c].copy_from_slice(&u32s(&[0, 0, w, h]));
        qi[0x30..0x34].copy_from_slice(&0u32.to_le_bytes());
        let mut body = u32s(&[0, qi.len() as u32, 0]);
        body.extend_from_slice(&qi);
        let r = tx(7, &body);
        assert_eq!((rd(&r, 0), status(&r)), (display_size().0, 0));
        let seen = SEEN.lock().unwrap().pop().unwrap();
        assert_eq!((seen.0, seen.1), (w, h));
        for (x, y) in [(0u32, 0u32), (63, 0), (5, 9), (63, 15)] {
            let o = ((y * w + x) * 4) as usize;
            assert_eq!(
                &seen.2[o..o + 4],
                &[x as u8, y as u8, 7, 255],
                "pixel {x},{y} as RGBA"
            );
        }
        assert_eq!(
            nxrt::kernel::get().event_signaled(vsync),
            Some(true),
            "vsync at the present"
        );
        assert_eq!(status(&tx(7, &body)), BAD_VALUE, "a slot not dequeued");
        tx(3, &u32s(&[1, w, h, 0, 0x100]));
        assert_eq!(status(&tx(8, &u32s(&[0]))), 0, "cancel");
        let r = tx(9, &u32s(&[1]));
        assert_eq!(rd(&r, 0), display_size().1);
        assert_eq!(
            send(binder, 2, u32s(&[BINDER_ID, 0xf]), vec![], vec![])
                .copy
                .len(),
            1,
            "the buffer event"
        );
        assert_eq!(status(&tx(11, &[])), 0);
        // a GraphicBuffer naming an id nobody made is not shown
        let mut bad = gb.clone();
        bad[0x2c..0x30].copy_from_slice(&999u32.to_le_bytes());
        assert_eq!(
            present_slot(&Slot {
                dequeued: true,
                ..Slot::parse(&bad).unwrap()
            }),
            BAD_VALUE
        );
        *SINK.lock().unwrap() = present_to_window;
    }

    /// Pitch-linear RGB565 and RGBA8888 slots become RGBA rows.
    #[test]
    fn pixel_formats() {
        let s = Slot {
            format: 4,
            width: 2,
            height: 1,
            pitch: 8,
            layout: 1,
            ..Default::default()
        };
        let m = [0x00u8, 0xF8, 0xE0, 0x07, 0, 0, 0, 0];
        assert_eq!(
            to_rgba(&s, &m).unwrap(),
            vec![255, 0, 0, 255, 0, 255, 0, 255]
        );
        let s = Slot {
            format: 1,
            width: 1,
            height: 2,
            pitch: 16,
            layout: 1,
            ..Default::default()
        };
        let mut m = [0u8; 32];
        m[16..20].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(to_rgba(&s, &m).unwrap()[4..], [1, 2, 3, 4]);
        assert!(to_rgba(
            &Slot {
                format: 9,
                width: 1,
                height: 1,
                ..Default::default()
            },
            &m
        )
        .is_none());
    }
}
