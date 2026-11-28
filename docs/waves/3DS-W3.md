# 3DS-W3 - Services: the title past `srv:` to its menu logic, input through `hid`

Plan: `docs/36-3DS-PORT-PLAN.md` §4 (the service table - this wave's spec), §8 W3 row, §9 (`src/os3ds`
file table), §10 R9 (the `os` input/media leaf question). Base evidence: `docs/waves/3DS-W2b.md`
(the boot now parks on the `srv:` reply: `ConnectToPort("srv:")`/`SendSyncRequest` gaps; `err:f`
next). Brief: owner, 2026-11-28. Base `b7aaeea`, worktree
`D:\Projects\RecompLauncherEngine-3DS-W3`, branch `3DS-W3`. Donor read-only at the seams
(`D:\Projects\3dsrecomp`, pin `d143f2c5`).

Unit: the new crate `src/os3ds` (the 3DS OS services) and the boot harness wiring that drives it.
Translator3ds, runtime3ds's interpreter/memory/kernel table, gpu3ds (absent) and the launcher are out
of scope; a defect there is reported with evidence, not fixed.

## Goal

The title gets past `srv:` and reaches its menu *logic* headless: the guest's own call pattern (apt
lifecycle, gsp init, hid polling) appears in the trace, with input delivered through `hid` from a
scripted feed. No rendering, no pixels, no sheets; W4 draws. Deliverable: `src/os3ds` per §9, the
boot driven through it, the menu-state evidence and the gaps summary, committed on `3DS-W3`.

Titles: Sonic Generations only (decrypted `.3ds` at
`P:\Games\Nintendo\DUMPS\Sonic Generations (3ds)\Sonic Generations (USA) Decrypted.3ds`, staged
read-only from `D:\Projects\RecompLauncherEngine\build\DS\W1\sonic\Sonic_data`; re-staged into
`build/DS/W3/` for this wave). Headless: CPU + memory + svc trace, no GPU, no audio, no window, no
`play.py`, no relink, `%LOCALAPPDATA%\nxrs` untouched.

## Expected files

| File | What |
|---|---|
| `src/os3ds/Cargo.toml` | the crate (deps: `runtime3ds`, `translator3ds`) |
| `src/os3ds/src/lib.rs` | crate root: `nxos3ds_main` shape, `install()` (svc hook registration), boot |
| `src/os3ds/src/boot.rs` | the 3DS boot order (srv → apt → gfx init → process start) |
| `src/os3ds/src/kernel.rs` | the kernel svcs this side owns: shared-memory block map/unmap, service-handle close (runtime3ds already answers the rest; not duplicated) |
| `src/os3ds/src/hle.rs` | session table + service registry keyed by port name + the gap ledger (`gaps`) |
| `src/os3ds/src/services/srv.rs` | service locator (`ConnectToPort("srv:")`, `GetServiceHandle`, `err:f`) |
| `src/os3ds/src/services/apt.rs` | applet lifecycle (menu, enable, applet info, parameters) |
| `src/os3ds/src/services/fs.rs` | FSUSER over the build's `romfs/` + `<data>\save` + a read-only SDMC root |
| `src/os3ds/src/services/hid.rs` | pad/touch/circle pad from the scripted feed + the HID shared-memory page |
| `src/os3ds/src/services/gsp.rs` | gsp::Gpu handshake (RegisterInterruptRelayQueue + shared memory); command payloads gap-refused |
| `src/os3ds/src/services/dsp.rs` | dsp handshake; firmware/audio payloads gap-refused |
| `src/os3ds/src/services/ldr_ro.rs` | CRO load/unload writing a module base into the library |
| `src/os3ds/src/services/cfg.rs` | region/language/model config blocks |
| `src/os3ds/src/services/ptm.rs` | battery/play time (handshake + plausible values) |
| `src/os3ds/src/services/y2r.rs` | YUV→RGB (handshake; conversion gap-refused) |
| `src/os3ds/src/services/soc.rs` | sockets (handshake; connect refused) |
| `src/os3ds/src/services/misc.rs` | mic/cam/boss/news/ndm/ir handshakes and refusals with gap lines |
| `src/os3ds/src/bin/boot3ds_os.rs` | the headless harness: stage data, map segments, install os3ds, run the scripted input, write `build/DS/W3/` logs |
| `src/os3ds/tests/*.rs` | service unit tests (srv dispatch, apt state machine, fs paths, hid script, cfg blocks, ldr::ro base write) |
| `Cargo.toml` (root) | one member line: `"src/os3ds"` |
| `docs/waves/3DS-W3.md` | this plan, then the outcome |

Nothing outside that: no runtime3ds edit (the `svc::set_handler`/`set_fallback` registry is the hook),
no `os` edit unless the input/media extraction proves necessary (preferred: script input in os3ds),
no gpu3ds/front/launcher/CLI.

## Pinned facts (each with its source)

### IPC wire format (3dbrew `IPC`, rev 24370)

- The request and the reply live at **TLS+0x80**; the kernel copies the request to the server and the
  reply back before `svcSendSyncRequest` returns.
- Header word: bits 0-5 = translate-parameter size in words, bits 6-11 = normal-parameter count,
  bits 16-31 = command id. Normal parameters follow; translate descriptors follow them.
- Reply word 1 is the result code (0 = success) by convention.
- `svcConnectToPort`'s kernel ABI (disassembly of the title's wrapper at `0x45C828`): `r0` = out
  pointer, `r1` = name; outputs `r0` = result, `r1` = session handle (the wrapper stores `r1`).
  This is why W2b's parked `SendSyncRequest(session 0x4a3d6c)` used the *name pointer*: the gap
  handler left `r1` as the name pointer and the wrapper stored it.
- `svcSendSyncRequest` wrapper (`0x449D7C`): command buffer at `TLS+0x80`, session read from a
  global; on success it copies reply word 3 to the caller's out word and returns reply word 1.

### Port/service table (3dbrew `Services` rev 24351, `SRV:GetServiceHandle` rev 24639, `IPC` rev 24370)

| Port / command | 3dbrew id | This wave |
|---|---|---|
| `ConnectToPort("srv:")` | SVC 0x2D | session for the service manager |
| `ConnectToPort("err:f")` | SVC 0x2D | session for ErrDisp; `Throw`/`SetUserString` answered |
| SRV `EnableNotification` | 0x00020000 | success (no notification needed headless) |
| SRV `RegisterClient` | 0x00010002 | success, zero handle |
| SRV `GetServiceHandle` | 0x00050100 | creates a session named by the 8-byte UTF-8 name |
| SRV `IsServiceRegistered` | 0x000E00C0 | true for the registered names |
| unknown SRV | - | gap line, result `0xD8E007F0` |

### APT (3dbrew `NS and APT Services` rev 24076)

Commands this wave answers with a small state machine (the table's ids): `GetLockHandle`
0x00010040, `Initialize` 0x00020080, `Enable` 0x00030040, `Finalize` 0x00040040,
`GetAppletManInfo` 0x00050040, `GetAppletInfo` 0x00060040, `GetLastSignaledAppletId`
0x00070000, `CountRegisteredApplet` 0x00080000, `IsRegistered` 0x00090040, `GetAttribute`
0x000A0040, `InquireNotification` 0x000B0040, `ReceiveParameter` 0x000D0080, `GlanceParameter`
0x000E0080, `CancelParameter` 0x000F0100, `GetPreparationState` 0x00130000,
`SetPreparationState` 0x00140040, `NotifyToWait` 0x00430040, `GetSharedFont` 0x00440000,
`GetProgramID` 0x00580002, `CheckNew3DS` 0x01020000, `GetTargetPlatform` 0x01010000,
`IsStandardMemoryLayout` 0x01040000, `GetApplicationRunningMode` 0x01030000.
`Notification`/`Command`/`AppletPos`/`AppletAttr` enums are the page's. Unknown -> gap.

### FS (3dbrew `FS:OpenFile` rev 22759, `Filesystem services`)

FSUSER commands this wave implements: `Initialize` 0x000100C2, `OpenFile` 0x080100C2,
`OpenFileDirectly` 0x080200C2, `OpenDirectory` 0x080800C2, `OpenArchive` 0x080900C2,
`CloseArchive` 0x080A0040, `GetFreeBytes` 0x080B0082, `GetSdmcArchiveResource` 0x080C0040,
`CreateDirectory` 0x080600C2, `DeleteFile` 0x08030082, `RenameFile` 0x080400C2.
File-session commands: `Read` 0x000100C2, `Write` 0x000200C2, `GetSize` 0x000300C2,
`SetSize` 0x000400C2, `GetAttributes` 0x000500C2, `Flush` 0x000600C2.
Archive kinds: SDMC 0, RomFS 3, SaveData 4. Path type 1 = ASCII, 3 = UTF-16LE. Paths resolve under
the staged data folder's `romfs/` (RomFS), `<data>\save` (SaveData; created, never a real save),
and a synthetic empty `sdmc/` root (read-only in this wave).

### HID (3dbrew `HID Services` rev 24160, `HID Shared Memory` rev 24161)

- `hid:USER` command `GetIPCHandles` 0x000A0000 returns the shared-memory handle plus the event
  handles; the app maps the shared memory through SVC `MapMemoryBlock` (0x1F).
- Shared memory layout (size 0x2B0+): PAD state at 0x18 (current) and the 8-entry array at 0x28;
  entry 0x0 current pad, 0x4 pressed, 0x8 released, 0xC circle pad (low s16 X, high s16 Y); touch
  at 0xA8 (+0x18 raw, +0x20 8 entries, entry 0x4 u8 = 1 when present); 3D slider f32 at 0x18.
- Pad bits: 0 A, 1 B, 2 Select, 3 Start, 4 Right, 5 Left, 6 Up, 7 Down, 8 R, 9 L, 10 X, 11 Y.
- Input script: one line `<ms> <keys>` (`neutral`, `a`, `start`, `a+start`, `left`, ...) read into a
  timed state list; the host applies each step on wall time, writing the PAD entries. Every applied
  step is logged (`hid script: ...`) and every shared-memory serve is counted, so the evidence shows
  the scripted values the guest was served.

### GSP / DSP (3dbrew `GSP Shared Memory` rev 24158, `GSPGPU:RegisterInterruptRelayQueue` rev 24155)

- `gsp::Gpu` `RegisterInterruptRelayQueue` 0x00130042: request flags + a KEvent handle; reply word 2
  = GSP thread index, word 4 = shared-memory handle. This wave creates a real 0x1000-byte shared
  block (interrupt queue at `base + client*0x40`, framebuffer info at `+0x200`, command queue at
  `+0x800`) and returns its handle; `TriggerCmdReqQueue` 0x000B0000 answers success and the GX
  command payload is gap-refused in the ledger (W4 owns it). No command executes.
- `dsp::DSP` handshake (`LoadComponent` 0x000100C2 etc.) answers the state transitions and
  gap-refuses firmware/audio payloads visibly (W5 owns them). No audio is produced.

### The kernel split (plan §4, `svc.rs`'s table)

`runtime3ds` already answers the kernel table (ControlMemory/QueryMemory/threads/sync/tick/…).
`os3ds` adds only what the named services need: `MapMemoryBlock` (0x1F) and `UnmapMemoryBlock`
(0x20) for service shared memory, and service-owned session handles in `CloseHandle` (0x23). No
kernel answer is re-implemented.

## Input script

The harness names the script file (`--input <file>`); the wave's evidence uses
`build/DS/W3/input.txt`:

```
0 neutral
2500 start
3500 neutral
5000 a
6000 neutral
```

`neutral` is no buttons and the circle pad centred; `start`, `a`, `left` map to the PAD bits above.
The script advances on wall time from the boot start and writes the HID shared memory the guest
mapped; each applied step and each serve is logged.

## Menu-proof definition

Menu *logic* is proven when the trace shows, in order, with no unexpected gap in the core set
(srv/apt/fs/hid/cfg):

1. `srv:` and `err:f` connected; `GetServiceHandle` for at least `fs:USER`, `hid:USER`,
   `gsp::Gpu`, `dsp::DSP`, `cfg:s` (or `cfg:u`), `apt:U`/`APT:A`;
2. the apt lifecycle calls (Initialize/Enable/GetAppletInfo/GetAppletManInfo, the menu state the
   title reads) answered;
3. gsp init (`RegisterInterruptRelayQueue`) and dsp init requested;
4. `hid:USER` handles fetched and the shared memory mapped, followed by the title's own steady
   polling loop (GetSystemTick / wait / vblank);
5. the scripted input served in the shared memory with the title's loop continuing (and, if the
   title's state changes with it, the changed call in the trace).

The gaps summary then lists exactly what remains (gsp command payloads, dsp firmware/audio, soc,
y2r, etc.) - that is the W4/W5 list.

## Plan

1. Wave doc (this file), root member line, crate skeleton with `hle.rs` sessions + gap ledger.
2. `srv` + `err:f` + `ConnectToPort`/`SendSyncRequest` hook, enough to leave the W2b park; run and
   read the trace.
3. `apt` + `cfg`; run; `fs` over the staged tree; `hid` (script) and its shared memory; `ldr::ro`;
   `gsp`/`dsp` handshakes; `ptm`/`y2r`/`soc`/`misc` refusals.
4. Unit tests per service; `cargo test --release -p os3ds`; workspace suite.
5. The boot: `build/DS/W2b` baseline vs `build/DS/W3`; save `boot.log`, the gap summary, the input
   script and consumed log under `build/DS/W3/`.

## Verification

- `cargo test --release -p os3ds` green (srv dispatch, apt state machine, fs paths over a staged
  tree, hid scripted states, cfg blocks, ldr::ro base write), `cargo test --workspace --exclude
  renhdrum` green.
- Before/after headless boot: the `srv:` gaps gone, the menu-phase sequence in the trace, the input
  script consumed, the gaps summary listing the remaining gsp/dsp/soc rows.
- Diff limited to `src/os3ds/`, this doc, the root member line, and (only if used) a minimal `os`
  leaf extraction.

## Review of the inherited work

A previous worker stopped mid-wave and left the crate uncommitted; it was committed as-is first
(`2193ed8`, "inherited work-in-progress (checkpoint)") and then reviewed (second worker, 2026-11-28).

**State.** `cargo build -p os3ds` and `cargo clippy -p os3ds` were clean (clippy: 4 style warnings in
os3ds; the clippy *errors* in the run are `clippy::eq_op` in dependency crates, not os3ds). Every file
of the expected-file table existed except `src/os3ds/tests/` (only in-module tests, two of them
failing: the key parser split `touch:x,y` on `,`, and a script test asserted the wrong boolean). The
inherited boot already passed `srv:` (W2b's park) and stopped on `svcBreak` in the APT thread.

**Pinned facts in this doc that were wrong** (the code had several right; corrected against 3dbrew):
FS command ids are `0x0801` Initialize, `0x0802` OpenFile, `0x0803` OpenFileDirectly, `0x0804`
DeleteFile, `0x0805` RenameFile, `0x0808` CreateFile, `0x0809` CreateDirectory, `0x080B` OpenDirectory,
`0x080C` OpenArchive, `0x080E` CloseArchive (the table above lists older/wrong ids); archive ids are
SelfNCCH 3, SaveData 4, ExtSaveData 6, SharedExtSaveData 7, SystemSaveData 8, SDMC 9 (not "SDMC 0,
RomFS 3"); GSP `TriggerCmdReqQueue` is `0x000C`, not `0x000B` (`0x000B` is SetLcdForceBlack); DSP
`LoadComponent` is `0x0011` (`0x0001` is RecvData); the HID PAD section is tick `0x0`, previous tick
`0x8`, latest index `0x10`, raw PAD `0x1C`, raw circle pad `0x20`, entries from `0x28`; touch entries
are at `0xA8+0x20` (8 bytes: x, y, valid).

**Real / stubbed / wrong / missing in the inherited code:**

| Unit | Finding | Now |
|---|---|---|
| IPC (`hle.rs`) | header decode/encode, TLS+0x80, reply word 1 = result: right | kept |
| srv | GetServiceHandle right; `EnableNotification` returned no semaphore (the title's srv thread waited on handle 0); `RegisterClient` answered extra translate words; unknown err:f command answered success | semaphore created; RegisterClient = result only; err:f Throw put in the gap ledger, unknown = refusal |
| apt | state machine and ids right; lock modelled service-side (see kernel note below); `AppletUtility`, `GetProgramInfo`, `GetCaptureInfo`, `HardwareResetAsync` answered success with nothing behind them | utilities ledgered, the rest refused; parameter event consumed on ReceiveParameter; reply shape per 3dbrew (static buffer from TLS+0x184) |
| fs | ids right; `DeleteFile/RenameFile/DeleteDirectory/RenameDirectory` answered success without doing anything (fake); `CreateDirectory` succeeded on unknown archives; a missing file opened for write was created without the create flag; RomFS without an image fell back to a *directory* session served as a file; `Write` reply lacked the byte count | delete/rename/create real on writable archives, refused on read-only; open flags honoured; SelfNCCH serves only the RomFS lowpath from the dump's IVFC level-3 range, else a gap refusal; Write returns bytes written |
| hid | GetIPCHandles right; the input wrote only entry 0 and never advanced index/ticks; events sticky | real ring (index, tick, previous tick, pressed/released), one-shot events, a 16 ms frame clock |
| gsp | `RegisterInterruptRelayQueue` right; no VBlank ever reached the interrupt queue (the "vblank" signal fired the event with an empty queue, only when `elapsed % 16 == 0`); GX commands drained wholesale | VBlank0/1 pushed into the queue each frame; DMA and memory fill done for real with their DMA/PSC interrupts; command list / display transfer / texture copy refused in the ledger with no completion interrupt (W4); cache/rights/QoS/priorities answered (no host action needed); register writes refused (W4) |
| dsp | **every command id wrong** (`LoadComponent` was `0x0001`) | 3dbrew ids; address conversion, semaphore event, headphone status real; LoadComponent accepted + ledgered; pipe/semaphore traffic refused (W5) |
| cfg | `GetConfigInfoBlk2` read size/id/pointer from the wrong words (wrote to address 0); `GetModelNintendo2DS` claimed a 2DS; config writes and serial/friend seed answered success | word order fixed, a block table (language, country, username, birthday, model, EULA, sound, backlight, parental, time offset), unknown blocks refused + ledgered; writes and console-unique data refused |
| ptm | ids 1-6 wrong (battery level was `0x0002`, which is SetRtcAlarm) | 3dbrew ids, plausible host values, alarms refused |
| soc / y2r / ldr:ro | wrong ids; `GetSocket`, `StartConversion`, `LoadCRO` answered success (fake) | only driver open/close answered; everything else refused + ledgered; `place_module` kept |
| misc | every command answered success ("refused" in the text, OK in the result) | ndm:u daemon controls answered (no daemons exist headless), everything else refused |
| kernel.rs | Connect/SendSync/Map/Unmap/Close right; `GetProcessId` (0x35) answered here (a kernel answer outside os3ds's split - runtime3ds does not answer it); wait/release probes unbounded | probes bounded to 400 lines; SignalEvent probe added; GetProcessId kept and named here for the runtime3ds owner |

## Outcome

### Built

`src/os3ds` per §9 with the fixes in the review table; `src/os3ds/tests/` (`srv_apt.rs`, `fs_paths.rs`,
`devices.rs`, fixture in `common/mod.rs`). Commits on `3DS-W3`: `2193ed8` (checkpoint), `fcf08b4`
(review fixes + tests), and the doc/gsp-ledger commit after it.

### Tests

`cargo test --release -p os3ds`: 23 passed (8 in-module + 15 in `tests/`), well under a second.
`srv_apt`: GetServiceHandle through the real TLS+0x80 buffer (reply header `0x00050042`, session is
`hid:USER`), unregistered name refused + ledgered, EnableNotification's single semaphore, unknown srv
command refused, the APT lifecycle (Initialize events, Enable, Glance keeps / Receive consumes the
Wakeup, then "no parameter"), AppletUtility ledgered. `fs_paths`: save create/write/read-back with
open flags, delete/rename on disk (second delete is not-found), `..` escape refused, SelfNCCH RomFS
image read + write refused + ASCII lowpath refused. `devices`: HID ring entries/index/ticks from the
script, cfg block word order (only the block's size written) + unknown block refused, GSP DMA/memory
fill + their interrupts + refused display transfer + VBlank pair, DSP address conversion + pipe
refused, ldr:ro base write + LoadCRO refused. Before the fixes the inherited in-module tests had 2
failures (key parser, wrong assertion); the cfg test passed only because it built its request in the
same wrong word order as the code.

### Verified - the headless boot

Same harness and DLL as W2b (`fnv1a:782A2BA22EF1144F`), decrypted Sonic, RomFS image from the dump
(`+0x2ab600`, `0x256c25fc` bytes), input `build/DS/W3/input.txt`, 20 s cap. Log:
`build/DS/W3/sonic/boot.log` (+ `gaps.txt`, `input-consumed.txt`).

The title now runs its whole service init through os3ds, in this order (trace lines):

```
srv: session 0x9000 -> srv:            ipc Srv cmd 0x0001 / 0x0002 (RegisterClient, EnableNotification -> semaphore 0x1003)
srv: session 0x9001 -> APT:U           apt: GetLockHandle -> 0x9002
apt: Initialize appID 0x300 (notification 0x1005, parameter 0x1006)
ndm:u daemon controls 0x0014, 0x0006   srv: session 0x9005 -> fs:USER, ipc Fs cmd 0x0801
fs: SelfNCCH RomFS -> image file 0x9007 (0x256c25fc bytes); ipc File(1) cmd 0x0802 x6 (RomFS reads)
apt: GlanceParameter -> Wakeup         (APT thread 2)
hid: GetIPCHandles -> shared 0x100b, 5 events; MapMemoryBlock 0x100b at 0x10000000 (Hid)
ipc Apt cmd 0x0003 (Enable), 0x0043 (NotifyToWait), apt: ReceiveParameter -> Wakeup, AppletUtility 0x7/0x4
fs: OpenArchive 0x7 (shared extdata f000000b), CreateFile/OpenFile "/gamecoin.dat", GetSize, Write, Close
ipc Ptm cmd 0x000c / 0x000b (step count, step history)
srv: gsp::Gpu; AcquireRight; RegisterInterruptRelayQueue -> 0x2a07, shared 0x1012; MapMemoryBlock at 0x10002000
gsp WriteHWRegs x64 / WithMask x4 (refused, W4); TriggerCmdReqQueue x3 (command list + display transfer refused)
ipc Apt cmd 0x000b (InquireNotification, APT thread) -> svcBreak thread 2 reason 0 at 0x45c824
```

Menu-proof items 1-3 hold (srv/err-free service handles for apt/fs/hid/gsp/ptm/ndm; apt
Initialize/Enable/Glance/Receive/NotifyToWait answered; gsp RegisterInterruptRelayQueue answered).
Item 4 holds for the handles and the mapping (hid block mapped, 3 ring entries published before the
stop). Items 4's steady loop and 5 (scripted input consumed) are **not** reached on the committed
tree: the boot stops at 0.2 s on the `svcBreak` above.

### The boundary: a runtime3ds wait answer (out of scope, reported)

The `svcBreak` is the APT thread's panic on notification type 0 (disassembly: the loop at
`0x44AB30` calls `WaitSynchronizationN(&index, {notification, parameter, own}, 3, 0, timeout)`,
panics on a negative result, then switches on `index`: 0 -> `InquireNotification` (`0x45409C`) ->
the jump table at `0x44AD10`, whose entry 0 is `bl 0x330958` (panic)). The thread parked with
`timeout Some(4294967295)` ns (RUNTIME3DS_PROBE), which `park_thread` scales to ~43 ms of host time;
when it returns, `runtime3ds/src/lib.rs::park_wait` answers `r0 = 0x09401BFE, r1 = 0` for **every**
parked wait - a timeout reports index 0 (the notification) instead of -1, and a signaled wake reports
a timeout with index 0 instead of `R_OK` + the signaled handle's index (Citra/3dbrew: timeout sets
the output to -1). Related runtime3ds kernel defects seen in the same review: an immediately
satisfied wait does not take ownership of a mutex, decrement a semaphore or reset a one-shot event
(`wait_ready` only tests), which is why the APT lock is modelled service-side (ledgered).

Experiment (not committed; `build/DS/W3/runtime3ds-wait-experiment.diff`, reverted): with
`park_wait` answering `R_OK` + the waking handle's index, or `0x09401BFE` + `-1` on timeout, the
same boot runs the full 20 s with no break (`build/DS/W3/sonic-waitfix/boot.log`): GSP thread 3
serves 1,216 VBlank waits (`WaitSynchronizationN -> 0` + `ClearEvent`, 1,216 each), the APT thread
polls on its timeout (648 timeouts), the HID ring gets 1,216 entries and all five script steps are
applied (`2503 ms: start`, `5008 ms: a`, ...). The main thread parks in `ArbitrateAddress` at
`0x2A7B24` (2,430 arbiter calls) after `TriggerCmdReqQueue` submitted a process-command-list and a
display-transfer: it waits for their completion (P3D/PPF), which only W4's PICA path can raise
(forcing those interrupts would be a faked completion). So past the runtime3ds fix, the next gap is
W4, and the title does not reach a loop that reads the pad before it.

### Gaps summary (committed tree, `build/DS/W3/sonic/gaps.txt`)

```
[1x]  apt: the APT lock is a service-side lock (runtime3ds waits do not acquire kernel mutexes)
[1x]  apt: AppletUtility 0x7 has no applet manager behind it (answered 0)
[1x]  apt: AppletUtility 0x4 has no applet manager behind it (answered 0)
[64x] gsp: command 0x0001 (GPU registers/display) refused (3DS-W4)
[4x]  gsp: command 0x0002 (GPU registers/display) refused (3DS-W4)
[1x]  gsp: GX process command list (1) not executed (PICA is 3DS-W4)
[1x]  gsp: GX display transfer (3) not executed (PICA is 3DS-W4)
```

No srv/fs/hid/cfg/ptm gap: every core-service call the title made was answered for real.

### Unproven / next

1. **runtime3ds (owner of the kernel table):** `park_wait`'s post-wake answer (index of the signaling
   handle + `R_OK`; `-1` + `0x09401BFE` on timeout); acquire/decrement/reset in `wait_ready`'s
   immediate path; the `/100` timeout scaling; `GetProcessId` (0x35). The experiment diff is the
   smallest version of the first item. With it, the APT lock can become a real kernel mutex.
2. **W4:** GX process command list / display transfer completion (P3D/PPF interrupts), the GPU
   register writes (`WriteHWRegs` 0x400004..0x400524 at boot). The menu logic (the loop that reads
   HID) sits behind the main thread's wait on those completions.
3. Input reaching the title's own logic is therefore unproven; the ring and events are tested and
   were published 1,216 times in the experiment run.
