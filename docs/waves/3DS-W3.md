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

## Outcome

(filled in after the work)
