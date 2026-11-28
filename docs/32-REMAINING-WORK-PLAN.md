# 32 — Remaining engine work and delegated execution ledger

**Status: procedural history and execution ledger.** The active game-outcome plan is now
[`33-REFERENCE-DELTA-ROADMAP.md`](33-REFERENCE-DELTA-ROADMAP.md). Delegation follows
[`31-DELEGATION-PROTOCOL.md`](31-DELEGATION-PROTOCOL.md). This file retains prior assignments, evidence and
resource decisions, but it no longer defines product milestones. Source root: `D:\Projects\RecompLauncherEngine`.

## 3DS port — 2026-11-27, opened (plan `docs/36-3DS-PORT-PLAN.md`, branch `gameplay-harness`)

The owner directed a 3DS port plan: `.3ds`/`.cia` in, native + wasm out. The plan was written by wave 3DS-PLAN
(`docs/36-3DS-PORT-PLAN.md`); the donor is `D:\Projects\3dsrecomp` pinned at `d143f2c51834bd89f32c0ac79465f1855d7d6f8e`
(MIT). Waves branch from `gameplay-harness` as they open; the lead adds each wave's result here. Owner decisions
D1–D7 (plan §10) run on their recorded recommendations until the owner says otherwise.

| Wave | State | Scope | Exit evidence (plan §8) / lead result |
|---|---|---|---|
| 3DS-W0 Feasibility | merged 2026-11-27 (`22b5ca3`, ses_ddff6262affexI7O1fDKacO0sX) | vendor the donor front half into `src/translator3ds`, synthetic fixtures, clang + emcc compile of the generated C | 77 tests green (lead re-ran); clang 21.1.8 → `translator3ds_w0.a`, emcc 7.0.1 → wasm objects (`\0asm` verified); vendor byte-identical at the pin except four documented files; no Switch crate touched. Evidence `build/DS/W0/`. Slips: vendor via `Copy-Item` (house precedent, hashes verified), shared emsdk cache touched (tool state only). Pre-existing reported: plain `cargo test` at root links `renhdrum`'s test target, which cannot link on Windows (7 `opus_*` symbols; proven at base) — ledger row owed, not this wave's fix. No dist rebuild: translator3ds links into no shipped artifact yet; first linking wave rebuilds. |
| 3DS-W1 Input | merged 2026-11-27 (`aa4acab`, ses_ddfce2c01ffeTdSURoKKEFdGu8) | `.3ds`/`.cia` accepted end to end (translate-only), data staging, launcher import | Real Sonic Generations .3ds: `--no-build` staged 178 chunks (8 s), 15,283 functions, 22-file RomFS — **cross-check vs the donor identical path-for-path**; `control.smdh` + `romfs.ds` record staged; launcher imports both real dumps (temp library) with SMDH art; launcher tests + clippy green (lead re-ran all suites). **The owner's named Pokémon Moon .cia is retail-encrypted (flags `01 03`): refused with the donor's exact line per plan §2 — the .cia staging is proven by a synthetic decrypted .cia through the real CLI; owner question D2 stands (decrypted .cia or a key lane).** Deviations accepted: `engine.rs BUILD_ICONS` + `control.smdh`, `sources3ds_tests.rs` (caps). Slip: `cargo fmt -p translator3ds` reflowed vendored files mid-wave — restored before commit, vendor hashes verified by the lead. Known limit reported: 3DS RomFS names Windows cannot take refuse extraction (no `romfs.bin` fallback on the 3DS backend yet). |
| 3DS-W2 Guest host | pending W1 | `runtime3ds`: run loop, dispatch, page-table memory, threads, ARM11/Thumb/VFP interpreter, svc dispatch | title entry + first threads run headless; interpreter differential 0 mismatches on the tested classes |
| 3DS-W3 Services | pending W2 | `os3ds`: srv/apt/fs/hid/cfg/ptm/ldr::ro | title reaches its menu, input moves it (sheets read); gaps summary |
| 3DS-W4 Graphics (native) | pending W3 | `gpu3ds`: GX command lists, VS/GS+TEV → SPIR-V, Vulkan executor, `front::ds` | first correct pixels: menu drawn, both screens presented (sheets read) |
| 3DS-W5 Audio + the rest | pending W4 | dsp into the media sink, y2r/soc | audio audible, underruns counted; first controllable gameplay, native |
| 3DS-W6 Pipeline polish | pending W5 | `profile.rs` 3DS table, per-title overrides, doctor line | second title builds and plays native; contract unchanged |
| 3DS-W7 Web | pending W6 | wasm arms of runtime3ds/os3ds/gpu3ds, page two-screen layout + touch, WGSL shaders | title's menu and gameplay run in Chrome |
| 3DS-W8 Validation + verify lane | pending W7 | library validation columns; recompiled-vs-interpreter differential as a build-time check | results named per title, failures listed |

## Current — 2026-11-24, gameplay harness reset (branch `gameplay-harness`)

The owner removed the incident scaffolding: HWiNFO/thermal monitoring, the build supervisor and provenance, GPU
stop latches and journals, machine leases, job/lifecycle layers, save transactions, the Wonder wrappers and the old
`run_game.py` / `play_scenario.py` / `gamectl.py` / `library_play.py` / triage scripts. Every hold, lease and
authorization gate below is retired; `build/GPU-STOP.json` files are inert evidence nothing reads. Gameplay runs
use `scripts/play.py` (see `AGENTS.md`); builds use `scripts/build/build.ps1` (4 workers).

Engine state carried forward (commits on `main` up to `a4499ee`): the GPU-alignment candidate plus the review
fixes (recoverable mesh/raw-table conditions skip instead of status 72); graphics 171 / shaders 75 CPU tests pass.
`dist/nxrs` is still the 2026-11-23 release; the candidate is not packaged and no game is rebuilt with it.
Open: GAR-5 raw-writeback producer delta (DKB/MKW), indirect mesh count validation, the OoT/MKW/Violet faults.

### To do (in order)

- [x] Commit the dirty tree as subsystem commits; independent review of the alignment candidate; review fixes
- [x] Replace the harness with `scripts/play.py` (smoke run on Wonder's old build: completed, 58 FPS)
- [x] Build the candidate into `dist/nxrs` (`build.ps1 -Jobs 4 -SkipGnu -KeepCache`: ~4 min, contract 2)
- [x] Rebuild Wonder with it; `play.py wonder`: completed 30/30, 60.1 FPS (5% low 59.4), no red lines, first
      course renders and the run/jump replay moves the camera (`build/play/20261124-121843-wonder`)
- [x] Rebuild + run the other known-pass titles (`build/play/20261124-12*`): agsob PASS (shooting, kills 5->8,
      60 FPS); beach-bounce PASS (VN advances, 60 FPS); arceus / arceus-base / jamboree render correctly and exit
      normally but stop inside their fresh-save openings (Arceus dialogue, Jamboree rules tutorial). Every title
      starts from an empty save (none of them has one), and a fresh save asks for a name: the harness now sets
      `NXOS_SWKBD=auto` (the first arceus run hung 15 s in a host text box nobody typed into). Jamboree's 12 GB is
      its reservation from second 30, flat, not a leak.
- [x] Arceus base repointed (2026-11-25): source, its 1.1.1 update package (copied too, hash-verified), buildDir and
      cover now under `P:\Games\Nintendo\DUMPS\Pokemon Legends Arceus base [01001F5010DFA000][v0]\`. `--source` does
      not move an entry's buildDir (no CLI option does): it was edited in `library.json` (backup
      `library.pre-arceus-repoint.json`) and the old build copied across so the rebuild relinked (12 s).
      `build/play/20261125-025127-arceus-base`: completed 28/28, 30.0 FPS (5% low 29.5), 0 red lines, same
      fresh-save opening as before. Nothing in the library names `D:\Projects\Switch2RECOMP-main` any more.
- [x] **CLEANUP-S2R** (2026-11-25, `docs/waves/CLEANUP-S2R.md`, accepted): `SWR_*` -> `NXRS_*` (no aliases),
      `swr_ctl` pipe dropped, launcher "turbo" engine and the copy-saves-from-Turbo feature removed (old `turbo` /
      engine-less entries load as nxrs), `turbo.toml` -> `owner.toml` (Z-A and OoT), Turbo attribution comments
      rewritten, `scripts/pkg/` deleted, Arceus test repointed to P:. Lead re-checked: the done-search finds only the
      deliberate `"turbo"` load test; launcher tests pass. Wonder smoke `build/play/20261125-043916-smoke` 60 FPS through
      the new pipe. Kept on purpose: `src/cli/launcher/switchrecomp.py` (Qt launcher imports it by name, contract v1),
      `toolRoot` in library.json (Qt format). Unproven: Linux `ctl.rs` not built (CI will).
- [ ] **RENHDRUM phases 0-1** (2026-11-25, `docs/waves/RENHDRUM.md`; built, **awaiting the owner's feel test** —
      checklist in the report): RenHDrum 0.5.0 vendored (`src/os/third_party/renhdrum`, rlib, nxos's one libopus),
      `input/rumble.rs` claims writable Pro / Joy-Con pads (1 ms thread, 250 ms silence cut-off, 40/150 ms restates,
      mute on focus loss/prompt/permission, neutral at exit), `system::`/`detail::PermitVibration` taken over,
      `hd_rumble` ini key (default 1, live), `nxinput rumble --hd`; unclaimed Nintendo pads use RenHDrum's encoder.
      os 165 passed (lead re-ran). No Nintendo pad on this machine: no real HD packet sent yet;
      `build/play/20261125-045606-dkb` (legacy path, no pad) 58.5 FPS, 0 red lines, sheet read. Open: real USB/BT
      packets and Joy-Con, BT input lag, `IsOunceVibrationDeviceMounted` trace, untaken-over send functions
      (`detail::SendVibrationValue`, `Enqueue/FlushEnqueuedVibrationValues`), F1 row (owner's call).
- [x] **GFX-ROWS** (2026-11-25, `docs/waves/GFX-ROWS.md`, accepted after an audit fix): (1) startup check of the shared
      layout against per-stage / per-layout / update-after-bind / descriptor-buffer limits, pool set sized from the
      room left (RTX 5090 unchanged: 262144 / 16384; a device that cannot fit gets one red line naming the limits);
      (2) MKW ID 337: a 3D texture read by a 2D fetch binds a 2D twin of slice 0 (`TwinKind::Slice`); (4)
      `materialize_all` cap (256 images / 32 MB a frame, live first) built but **off by default** after the lead's A/B
      on Jamboree alone: capped opening 15-39 FPS vs 38-49 all-at-once, frames > 50 ms 1295 vs 709, replay 146 vs 94 s
      (per-frame pool churn) against one ~0.7 s boot frame saved (`build/play/20261125-054831-ab-capped` vs
      `-055600-ab-allatonce`); `NXGFX_LAZY_ALL_FRAME=<n>` turns it on. Capped peak memory was 7.1 GB vs 12.2 GB —
      a lead for a cheaper bound (make pool-indexed images without churning the table every frame). (3) MS declarations over single-sample views: **parked**, needs per-mask module variants (plan in
      the wave doc). Audit HIGH fixed by the lead: a lazily registered slot kept its previous descriptor (possibly a
      destroyed view) or zeros while the budget left it waiting; it now names the zero stand-in at registration
      (`image.rs` `reg_tex`; not a placeholder mark, so a bound use still materializes). Verified: graphics 200 passed;
      `build/play/20261125-052405-lazy-slot-fix` MKW 43/43, 28.9 FPS, 1 known red line, sheet read. Jamboree check
      (the pool-indexing title) requested from FRESH-SAVE. Audit LOWs: `db_fit` checks one chunk/one pool buffer
      (necessary, not sufficient); `maxUpdateAfterBindDescriptorsInAllPools` unchecked without descriptor buffers;
      the 32 MB cap overshoots by one texture; finalized registrations can starve behind a stream of live ones.
- [x] **FRESH-SAVE** (2026-11-25, `docs/waves/FRESH-SAVE.md`, accepted): routes reach first gameplay from a fresh
      save; sheets show the player moving under input. arceus `build/play/20261125-053050-arceus-final2` 30.0 FPS
      (5% low 30.0) of 30; arceus-base `-053830-arceus-base-final2` 30.0 (29.5); jamboree `-053049-jamboree-relinked`
      59.6 (58.0) of 60. The Arceus route walks three times before its window to absorb the 5 s loader stalls (engine
      row below). **arceus-base has no library entry any more** (it and "Mario Kart World v1.8.0" vanished from
      `library.json` between 02:49 and ~05:13; no launcher code found that removes entries; the build is intact on P:):
      the run used `--exe`. **Restored** by the lead on the owner's go-ahead ("fix the other issues"), with the P:
      paths (backup `library.pre-arceus-restore.json`); "Mario Kart World v1.8.0" left out (owner's call). `scripts/inputs/hold_lr.txt` writes 0x192, not L+R
      (unused now).
- [x] Scenarios for fresh saves (done by FRESH-SAVE above): extend arceus / arceus-base / jamboree through their openings to first gameplay
- [ ] **Parallel waves opened 2026-11-25** (owner: fix the other issues, then the preemption fix and rebuild last;
      shared rules `docs/waves/PARALLEL-RULES.md`): MKW-PERF5 (race to 60, GPU 0), SMASH-POLISH (hitches, audio
      underruns, repeated-match memory), ROUTES-SS (Shadow/Sonic moving through the window), VIOLET (first gameplay
      on today's engine), OOT-PIPE (the `d15370a382630d4e` compile). Owner: "stop those three and run them after mkw":
      SMASH-POLISH, VIOLET and OOT-PIPE parked minutes in (each wave doc has a parked report) and ran after
      MKW-PERF5 (the lead then ran them one at a time; that was the lead's choice, not an owner rule). MKW-RECORD (record-time translation, `docs/waves/MKW-RECORD.md`) opened 2026-11-25:
      phase 1 design done and accepted (replay-time worker pool, not record-time: entry state and ~10 of 15 ms
      depend on replay-time state; walker thread for the 4.4 ms write-watch walk; target replay ~11-13 ms/frame);
      phase 2 code after OOT-NAN. Queued: GFX row 3 (MS
      declaration variants). Last: the preemption row below, then retranslate Arceus and relink the rest.
- [x] **OOT-PIPE** (2026-11-25, `docs/waves/OOT-PIPE.md`, **accepted**; lead read the sheet). Independent audit of
      `vk/mem.rs` (`placement_for`: physical pools get a buffer) and `vk/lazy.rs`: no HIGH; no double coverage (an
      alias virtual pool is never a pool; sparse ranges resolve to the physical pool). **MEDIUM fixed by the lead:**
      GPU writes through a sparse virtual pool were never written back (`note_gpu_write_version` got the virtual
      address) — it and `sync_range` now `vpool::translate` first (graphics 203 passed). Recorded LOWs: `vpool::copy`
      reads possibly-held physical pages without `lazy::touch` (fault -> bring_back under the POOLS read lock: a
      spike); all physical pools now write-watched/scanned (perf-check a sparse-pool title: MKW asked of OOT-RGB9E5);
      Vram physical pools' GPU writes never reach guest memory (pre-existing); `make_lazy` ignores a `protect_n`
      failure after a concurrent decommit (page listed held but reachable). Original findings: the `d15370a382630d4e` compile is long but finite (~66 s
      cold, 195,001 SPIR-V instructions from `dmsh_xlate`; the dispatch waits, nothing skipped). Fixes: lazy
      write-back no longer holds never-committed heap pages (New Game lost 11,616 pages / 45 MB and logged 11,616 red
      lines; now 0); physical memory pools get GPU access (a 305 MB virtual pool over a physical pool read zeros).
      Route: title -> New Game -> opening -> Kokiri Forest. `build/play/20261125-131719-oot-final` 139/139: title,
      story, nightmare, Link's house correct; **forest black/white behind subtitles** — 630 red lines: E5B9G9R9
      storage images the GPU cannot store -> zero stand-in. Next: wave OOT-RGB9E5 (opened, running). Slip: the
      agent's restore copied the wrong way and **lost the original 152 MB `pipeline-cache.bin`** (regenerable cache;
      the live one is this session's 357 MB); keys and crashed lists restored byte-identical. Owner option: drop the
      old-machine `pipeline-crashed.bin` entry that now compiles cleanly (`-xoot5-noblack`).
- [x] **OOT-NAN** (2026-11-25, `docs/waves/OOT-NAN.md`, commit `9903fc7`; lead re-ran graphics 209/209 and read the
      oot and mkw sheets). **Audit done: no HIGH**; c0 layout, platform gate, every cache path (bank cache, set key,
      patching) checked correct. MEDIUM M1 gap (an R32_UINT image with an E5B9G9R9 view still gets a pool storage
      view) and LOW dump crash on sparse pools handed to OOT-FOREST2. Recorded LOWs: M3 queries only the 2D shape;
      capture replays never set `SWITCH2` (NVN2 captures lose dispatch words and deswizzle wrong); Bits32 read alpha;
      `SWITCH2` gated on program-id prefix 04 (a Switch 2 Edition under 01 needs the `switch2 1` override). Cause of the white forest: **NVN2 compute shaders read the
      dispatch group counts at c0 + 0x4; nxrs wrote only 0x200** (NVN1's place) -> the sky irradiance compute
      divided by 0 -> Inf/NaN spherical harmonics -> NaN in the forward pass, spread by fog. `c0_bytes` now writes
      both for Switch 2 titles (MKW too). The `0x13dc17000` suspect was the game's own transient aliasing, not a bug.
      Also fixed OOT-RGB9E5's audit M1-M3, L3, L5, L7 (L1 not done). `build/play/20261125-153150-oot-nan-fix`: Deku
      Tree, Navi, roots render (white before), 2 known red lines; `-172447-oot-route7` to Link dressing in his house
      (~1284 s, still a cutscene). MKW `-154021-mkw-nan` race 29.2 FPS, 1 known red line, sheet correct. Lead's
      sheet read: the forest haze looks too strong. Open -> wave **OOT-FOREST2** (indirect dispatches still put CPU
      counts in c0; the haze; forest frames 70-380 ms with ~56 MB uploaded per frame and ~20 s without presents
      entering the house; route to Link moving). Slip: one shader `.spv` swapped for a trap run, restored
      byte-identical (SHA-256 checked).
- [x] **OOT-FOREST2** (2026-11-25, `docs/waves/OOT-FOREST2.md`; lead re-ran graphics 212/212 and read the final
      sheet). **Audit (in the wave doc): snapshot recycling and indirect c0 correct; upload skip has 2 HIGH** (raw-
      pointer shader writes forget the hash only after the next upload pass, never for pools > 64 MB; `forget` stops
      early under nested pools) **+ MEDIUM** (hash and upload read guest memory at different times; skipped bytes
      still charge the upload budget) -> **lead turned the skip off by default** (`NXGFX_UPLOAD_SKIP=1` for A/B;
      graphics 220/220); fixing it joins the GPU-written re-upload work after MKW-RECORD (`gmem`/`queue.rs`). LOW
      L1 (M1-gap slot keeps an old storage view, should be null) handed to NVN2-PARITY. Indirect dispatches: private c0 per dispatch, the GPU copies the record
      to 0x200 (and NVN2 0x4) before it. Texture-pool snapshots recycled with only changed descriptors re-copied
      (were 8-24 a frame at 13-17 ms: 135 ms frames) -> ~0.1 ms. `upskip.rs`: identical re-uploads skipped
      (~8.5 of ~56 MB/frame); the other ~46 MB/frame are pages noted GPU-written every frame and need a per-write
      sequence through the write-back (`queue.rs`, after MKW-RECORD). **Link moves under input** in his house:
      `build/play/20261125-192048-oot-f2-final` window [845, 915] **13.7 FPS (5% low 10.0) of 59**; forest 14.8
      (was 7.0), house 15.1 (was 3-5); GPU 8.7 ms, replay 12.4 ms, **pool sync ~36 ms a frame** (the limit). Haze:
      not item 1; added by the game's bloom composite, engine inputs correct as far as checked; unproven without a
      reference capture. Wonder regression 59.5 FPS, 0 red. Slips: three runs overlapped MKW-RECORD's machine lock
      (their FPS indicative only); one crash from its own dump-log line (fixed, dump-only).
- [x] **NVN2-PARITY done** (2026-11-25 ~20:50; lead re-ran graphics 220/220, read the dkb and oot sheets;
      **independent audit running**). Fixed with tests: push banks c24-c29 readable, handle decode (`bank_u64`,
      Switch 2), compute c0 uniform table as handles, `ucb_handle` size cap; timestamp on the guest clock; two-word
      texture handles ORed (5 sites); CPU handle reads see the submission's queued uniform updates; uniform size
      rounded to 256 (both NVN versions); device items 0x06=16, 0x62=32; emulated storage slots get a stand-in.
      Measured, nothing to fix: `UPDATE_UB` ends a pass only 2-4 times a frame; no sparse tiles live in DKB/OoT.
      Runs: DKB 58.7 -> 59.2 (5% low 53.0 -> 53.8) of 60; OoT 13.7 -> 15.0 of 59; Wonder 60.0. **Render size is not
      the gap** (`NXOS_DISPLAY_MAX=1` no effect; DKB already scales to 2095x1179): OoT's ~70 ms frames are replay
      24-27 ms + **pool sync 26-35 ms** + GPU 10-11 ms. **Pop-in not reproduced in these scenarios (before or
      after)**: needs the owner's scene, and a Mario Kart World check once MKW-RECORD has merged this.
- [x] **NVN2-FIX** (2026-11-25, `docs/waves/NVN2-FIX.md`, commit `40ff148`): the NVN2-PARITY audit's MEDIUMs —
      set cache keyed by an inline-uniform-update generation for sets reading storage addresses from c3-c18;
      sparse storage bindings held to the mapped contiguous run (`vpool::run_len`); handle sizes kept; stand-in
      storage views off the storage list; NVN2 c0 0x210-0x310 always decoded. DKB 59.3, OoT 14.9, Wonder 60.0, sheets
      unchanged (none of the scenarios exercises these paths). **Its audit: no HIGH, MEDIUM fixed by the lead**
      (`handle_bound`: a handle's size held to its pool window outside sparse pools — a rounded size past the pool
      end failed to resolve and fell to the zero buffer; saturated size = unknown; `sparse_bound` also held to
      `mem::room`); graphics 224/224, `build/play/20261125-215058-dkb` 59.3 FPS (5% low 54.0), 0 red, sheet correct.
      Recorded LOWs: sparse page table walked 3x per binding; update generation bumps every `UPDATE_UB`
      (unmeasured on MKW); a sparse remap within a submission does not invalidate cached sets (pre-existing).
- [ ] **NVN2-PARITY** (opened 2026-11-25, `docs/waves/NVN2-PARITY.md`, starts after OOT-FOREST2). Owner: every
      Switch 2 title misses 60 FPS and pops textures; Switch 1 titles fine. Read-only parity audit against nxnative
      (38 NVN2 items): missing/different — pushed buffer addresses (c24-c29) and uniform-buffer handles resolve to
      nothing; compute c0 uniform table raw addresses instead of handles; `GetCurrentTimestampInNanoseconds` on its
      own clock; two-word texture handles take the first word only; `UPDATE_UB` breaks passes and CPU handle reads
      miss queued updates; uniform size not rounded to 256; sparse tile slots freed per mapping; device items
      0x06/0x62; DLSS is a plain blit; ray tracing absent; c0 sample table. Indirect dispatch sizes and pool
      snapshots are OOT-FOREST2's.
- [ ] **MKW-RECORD steps 1-3 done** (branch `mkw-record`, commits `3fadfd6`, `58286e3`, `f89bd0a`, report `fd5c2b3`;
      lead read the `195700-s3c-on` sheet: correct). Pool walker thread: replay+prep 32.5 -> 26.0 ms a frame
      (paired), write-watch scan 8-12 -> 3.6 ms; segmented replay (off by default, `NXGFX_TRANSLATE_THREADS`): 0
      entry-state differences, +0.8-2 ms. **No FPS gain yet (~29 of 60)**: the gain is step 4 (worker lanes); the
      largest segment (~1,300 draws, a third of translation) needs step 5's mid-pass cuts. Write-watch scan cost
      flips 3.5 <-> 9 ms run to run (FPS noise of 1-2). Steps 4-6 handed to a fresh agent (after merging
      `gameplay-harness`); independent audit of steps 1-3 running.
- [~] **2026-11-26 day** (lead). UPLOAD-SYNC (`5056038`) + audit fixes (`2f29f99`): OoT 14.1 → ~18.8 FPS of 59; second
      audit found HIGH-1 (a copy's mid-recording sync keeps a hash the GPU no longer holds) → UPSYNC2 running in
      worktree `RecompLauncherEngine-upsync2` (OoT, GPU 1). HARNESS-DIAG (`1a3bcca`): anomaly table, route checks,
      `--faults`; open: race route checks are too strict (MKW race shots read OFF-ROUTE while on course). TRANSLATE-COST
      (`09cac5b`): lead quiet A/B MKW GPU 0 off 29.4/25.7, on 30.7/29.0 (30 cap); view sharing and rare split
      opt-in; audit: default path sound, sampler (`hostprof.rs`, opt-in only) M1 allocates while a thread is
      suspended, M2 unwinds while suspended, L1 addr cache should be off under `NXGFX_TRANSLATE_GATE=all`, L2 skip
      own thread — open. Next lever for MKW: the completion thread holds the pool lock ~4 ms while landing
      write-backs (lanes wait ~20 %), after UPSYNC2 merges. Open bug, unowned: OoT textures sampled in the frame they
      are registered (zero stand-ins at scene changes → one-frame black/red frames; also with lanes off).
      docs/31 rewritten (giga-cut pattern, worktree per wave, no locks); CI: feature-branch CI, release only on main.
- [~] **2026-11-26/27 engine waves** (lead; all merged into gameplay-harness up to `2e807c6`, wave docs in
      `docs/waves/`): REF-ARCH, USE-SYNC, SET-FAST, MKW-PIPE, MKW-AHEAD, MKW-SETS, MKW-SYNC, OOT-DESC, OOT-OCC,
      OOT-REPLAY, OOT-COMBO, DIALOGS. Lead's quiet runs on the main dist (console, 60 Hz, nothing else running):
      MKW 42.7 / 5% low 29.4 of 60 (`20261127-065536-mkw`, on route, sheets correct; was ~30 / 28); MK8D
      regression 59.9 / 59.0 (`20261127-070129-mk8d`, no black frames with the occlusion lag on). OoT (agents'
      numbers): house ~28.8 / 27.5 of 59; full `oot` runs still hit the 32 GB cap at ~840-895 s (texture images grow
      ~7/s in the house). Open: MKW's default pacer trial still fails on frame spikes (replay thread 17-24 ms);
      OoT pool-lock waits behind the completion thread's landings; OoT white house frame (~3-5/min, pre-existing);
      MKW near-black roadside shrubs (pre-existing).
- [~] **OOT-FIX + OOT-FPS** (2026-11-27, merged into gameplay-harness; `docs/waves/OOT-FIX.md`, `OOT-FPS.md`).
      OOT-FIX: washed-out colours fixed (light-probe pools now backed and in the gmem table; pipeline-crashed.bin
      lists only real crashes), ~3.6x more draws. OOT-FPS: write-backs compared on the GPU (`NXGFX_WB_DIFF`),
      landings in page runs, raw write log read once from host-cached memory. `oot` 845-915 s: 27.3 / 5% low 22.6
      (`20261127-185906-fps-base-prof`) -> 29.6 / 27.3 of 59 (`20261127-210135-fps-final2-prof`, on route, sheets
      read by the lead); peak memory 29.5 GB. Open: replay thread ~19.5 ms against ~15.8 for one vsync (set
      building ~5.2 ms, hashing CPU rewrites ~3.4 ms); Fault A white house frames (10 in `-211650-fps-final2-house`);
      memory item (table-only pools) not done.
- [~] **OOT-FPS2** (2026-11-28, merged; `docs/waves/OOT-FPS2.md`). Pool tables skip the ~4,500-pool walk while
      `POOL_GEN` stands (`NXGFX_GMEM_WALK_SKIP`, gmem::bindings 1.25 -> 0.07 ms); page hashing batched, 12 threads
      (`NXGFX_HASH_THREADS`). `oot` 845-915 s: 29.5 / 26.0 (`20261127-220112-fps2-base-prof`) -> 29.8 / 28.0 of 59
      (`20261128-000649-fps2-final-prof`, on route, sheets read by the lead); replay+prep 19.3 -> 17.2 ms. Walker
      hashing, scan-time hashing and lazy-image waits were built, moved nothing, removed. Open: the game's own frame
      takes ~1.3 vsyncs (main thread 90% idle, blocked on other guest threads: next, profile what those wait on);
      opening's held-back writes 16 -> 138 MB on one line (pictures correct); Fault A (14 in `-002206-fps2-final-house`).
- [~] **OOT-WAIT** (2026-11-28, merged; `docs/waves/OOT-WAIT.md`). Wait accounting (`NXRS_WAIT_ACCT=1`) found the
      main thread waiting 12.1 ms a frame on job threads that were 10-15 % busy: guest 20 us sleeps took ~0.58 ms on
      the Windows timer, and CPU flushes waited up to 5 ms for the pool lock. Fixes: SleepThread lasts as asked (host
      sleep then yield for the last `NXRS_SLEEP_SPIN_US`=1000 us); flushes queued when the pool lock is held
      (`NXGFX_FLUSH_QUEUE`), taken at the next lock before pending ranges are read. `oot` 845-915 s: 29.7 / 27.0
      (`20261128-010456-wait-base`) -> 47.1 / 30.0 of 59 (`20261128-014336-wait-fix1`, on route, sheets read by the
      lead); house 44.4 / 41.0 (`-015941-wait-house`). Open: replay thread ~17 ms (pool-lock waits behind the
      completion thread's landings, ~3 ms); peak 30.7 GB near the 32 GB cap; yield cost on host CPU; the sleep
      change is global (other titles need a regression run).
- [ ] **MKW-RECORD steps 4-6** (2026-11-25 late, branch `mkw-record` up to `2d02e3b`, report in the worktree's
      `docs/waves/MKW-RECORD.md`): 6 translation lanes by default, in-pass cuts, walker walks count as scans. MKW
      30.1 FPS (5% low 28.0) of 60, was 26.9/22.0; replay thread 23.4 ms (was 30.3). Smash 59.8/59.0 unchanged.
      Lead: graphics 235/235, race and Smash sheets correct. Independent audit: HIGH H1 (pre buffers run in
      segment order but were recorded in lane time order → stale pool bytes / twin copied before upload: a
      pop-in candidate), MEDIUM M1 (image takeover not gated), LOW L1-L6.
- [x] **MKW-RECORD audit fixes + dark turn** (2026-11-26, `ea5dcbf`, `06f2a6e`; merged into gameplay-harness as
      `16e1592`). H1: one shared pre buffer per parallel run, recorded under a lock in real time order. M1: takeovers
      recorded per run, earlier segments see the taken-over image (gating it cost 26.8 FPS). L1-L5 fixed; L6 (lazy
      bring-back reset without WATCH_MU) open: taking the lock there can deadlock. Dark banked turn in
      `233849-audit-perf` was a lanes fault: a later segment sampled a texture before an earlier one made its image
      → zero stand-in (shadow map). Fix: such a lookup asks the gate; race stand-ins 27-69 → 0 (serial 0). MKW
      ~30 FPS (5% low 27-28) of 60, replay thread 21.6 ms; Smash 59.8/59.0. Lead: graphics 238/238, sheets read
      (fix-1 crosses the turn lit). Open: bindless pool slots (`prepare_pool_images`) not ordered by the fix (no
      stand-ins seen); one loading submission uses all 256 timestamp queries (red line, nothing refused in race).
- [ ] **Running 2026-11-25 evening:** MKW-RECORD phase 2 (steps 1-3, 8 h) in worktree
      `D:\Projects\RecompLauncherEngine-mkw` (branch `mkw-record`, own target/dist, relinks only MKW, main-tree
      locks); OOT-FOREST2 in the main tree (OoT only, regression `wonder`, no `vk/queue.rs`); the OOT-NAN audit.
- [~] **OOT-RGB9E5** (2026-11-25, `docs/waves/OOT-RGB9E5.md`; lead re-ran graphics 208/208 and read both sheets;
      accepted pending OOT-NAN's audit fixes). New `vk/storemu.rs`: emulated storage formats (E5B9G9R9 via an R32_UINT view;
      the game's own E5B9G9R9 view gets the RGB9E5 pack, its own `r32ui` view the raw bits), `image.rs`
      `EXTENDED_USAGE`, shader variant kind 5, dispatch pipeline created after storage preparation; `NXGFX_STOREMU=0`
      off. **Audit (done): no HIGH; 3 MEDIUM handed to OOT-NAN** (which owns these files): M1 bindless image pool
      gets the R32_UINT emulated view but pool shaders are never emulated (a NaN suspect); M2 view remap by
      requested format vs emulation by image format; M3 no format-properties check for E5B9G9R9+STORAGE (a driver
      that refuses loses the texture). LOWs: failed dispatch still marks storage writes; constant texels refused;
      Bits32 read alpha; per-draw scan cost for all titles; rescale drops `EXTENDED_USAGE` for sRGB storage
      (pre-existing); `NXGFX_STOREMU` missing from `docs/env_switches.tsv`. Maths and SPIR-V rewrite checked correct.
      `build/play/20261125-140847-oot`: **red lines 632 -> 2** (both known), no device loss; title/story/
      nightmare correct, **forest still white** — a separate NaN: forward pass `9f36b8ee91e736c6` writes NaN into
      42-57% of the HDR target, fog draw `888c1068ac8f7043` spreads it. MKW regression `-142730-mkw` 29.6 FPS (was
      29.1), 1 known red line, sheet correct. Lead: the "dozen textures at `0x13dc17000`" (physical pool base) is
      the first NaN suspect. Next: wave **OOT-NAN** (opened, running, `docs/waves/OOT-NAN.md`). OoT's
      `pipeline-crashed.bin` pipeline compiled cleanly with the file aside (restored byte-identical) — owner option
      to drop it. Slip: one `sed` edit of the report header.
- [x] **VIOLET** (2026-11-25, `docs/waves/VIOLET.md`, accepted; lead read the sheets and the `begin_pass` change):
      fresh save to the bedroom with the player walking, `build/play/20261125-120752-violet-final` 29.9 FPS (5% low
      29.5) of 30, **0 red lines**, memory flat. Route stops were game behaviour ("Choose a look" has no default; A
      re-opens the Adventure Guide, B closes it). Engine (classification only, same draws skipped): a draw with no
      target set at all and no side effects (no written storage, image or xfb) logs one `[vk]` line instead of red;
      a target that fails to attach or a program with side effects stays red with slot counts; a non-texture colour
      pointer in `SET_RT` now red (was dropped silently). Unproven: ~60 s route margin before "Get Up"; leaving the
      house not revisited; history's 21 red lines did not reproduce.
- [x] **SMASH-POLISH** (2026-11-25, `docs/waves/SMASH-POLISH.md`, **accepted**; lead read the final sheet — two
      matches, results; os 166 / graphics 202 passed, lead re-ran). Independent audit of `guest::readable` /
      `Memory::mapped`: no HIGH/MEDIUM (fault handlers commit/restore on any thread, both platforms; the memory lock
      is a leaf). LOWs: (3) `in_phys` copied+sorted the run list per call — fixed by the lead (walks the sorted, merged
      `st.phys` in place); (4) the doc said "committed" — fixed (yes ≠ committed; OS calls must `mem::prepare`
      first); recorded, not changed: (1) movie `ev_ok` accepts a stale pointer into untouched heap (signals a zeroed
      struct), (2) audio renderer accepts a garbage wave-buffer range inside the heap (commits zeros).
      Causes: the movie decoder's `guest::readable` called `VirtualQuery` over the 1.2 GB heap under the process memory
      lock (13-100 ms) and every write-watch scan waited behind it; mid-match hitches were the fixed 6 GB shader-part
      cap. Fixes: `Memory::mapped` (os table) answers guest ranges first; parts cap = RAM/8 (min 6 GB); prewarm
      most-recent keys first, budget RAM/16 (2-8 GB); `[smash]` plays a second match. `build/play/20261125-111130-
      smash-polish-final`: movie 12.8 -> 59.9 FPS, match 58.2 -> 60.0 (frames > 50 ms 18 -> 1), second match 59.9,
      underruns 17 -> 2 (at match loads: the preemption row). Memory +~110 MB a match with a changing random opponent
      (unproven either way). MKW 29.1 (`-112011-mkw-smashpolish`), peak 17.4 GB vs 17.0. Next: `engine_private_bytes`
      still walks the heap per pipeline at boot.
- [x] **MKW-PERF5** (2026-11-25, `docs/waves/MKW-PERF5.md`, accepted, no audit owed: only two opt-in diagnostics
      remain, `NXGFX_CHAIN_TRACE` and `NXGFX_OP_TIMES`; lead read the final sheet — correct race, lap 2/3): no FPS
      gain, 28.9 (5% low 25.1) `build/play/20261125-101138-mkw-perf5-final`. Corrected diagnosis: main waits on the
      *previous* frame's fence; the limit is the replay thread's own ~28 ms of serial work a frame (draw translation
      ~15 ms over ~2,200 indexed draws, write-watch ~7 ms, passes ~2.4 ms; the op's 7 sub-submissions chain on events).
      Four cuts tried and reverted (parallel walks worse, present-only pool, clean-pool skip, P-core pinning).
      Checkpoints cost 0.5-0.7 ms (29.7 off vs 28.9 on; default stays on). Hold 2 is near saturation (scan swings
      7.3-11.5 ms between runs -> 26-29 FPS): a steady 3-5 ms cut would hold a stable 30. **Route to 60: translate
      command buffers when recorded on the game's worker threads (`CallCommands`) — its own multi-day design wave,
      queued after GFX row 3.**
- [x] **ROUTES-SS** (2026-11-25, `docs/waves/ROUTES-SS.md`, accepted; lead read both sheets): stage jump is B, not A
      (old replays pressed A); Shadow's menu toggle fixed (R, R, Right). shadow `build/play/20261125-095656-shadow-final`
      30.0 FPS (5% low 29.4) of 30, Shadow moving on every window shot (loops the stage: the way past the road's edge
      is not found); sonic `-100648-sonic-final` 31.5 (31.0) of 30, eight different Green Hill places incl. the loop.
- [ ] **Engine: two processes of one title share `pipeline-*.bin`** (found by ROUTES-SS): a second process reads the
      first's in-flight compile list as a crashed session and blacklists those pipelines (`pipe.rs` `load_crashed`;
      `build/play/20261125-085055-sonic-route1/run.log` line 26), and the cache write failed. Fix: per-process
      in-flight file (or a lock) so only a dead owner's list counts. Until then: never run two copies of one title.
      **Owner decision:** the 2 false entries it added to Sonic X Shadow's `pipeline-crashed.bin` (no compiler
      crashed) — drop them or keep them.
- [ ] Shadow: 2x `texture ID 356: target 1 bound where the shader samples target 2` at the stage load; Shadow
      Generations main menu / post-title dialog show button glyphs without text (`-090643-xmenu/x-n1.png`,
      `-091344-xmenu3/x-s3.png`) — possible text-rendering defect.
- [ ] **Engine: no preemption inside long guest loops.** Fresh-save Arceus 99-114 s: low-priority thread 0x1004
      (prio 46) holds virtual core 2 in a loader loop with no SVC; the better threads wait `STEAL_GUEST` (5 s) per
      steal, presents fall to 0.2 FPS. Horizon would preempt it. Fix: a displaced-flag check on translated loop
      back-edges (generalising `rt_spin_gate`) so a displaced holder yields at once; needs a Cpu-struct field (appended
      at the end, so existing offsets hold) and a retranslation of each title that needs the check (Arceus first);
      titles not retranslated keep their current code and only relink. STEAL_GUEST stays 5 s until then (200 ms broke DKB's loaders).
- [x] dkb (`build/play/20261124-124926-dkb`): completed 13/13, 59.4 FPS (5% low 53) over 100-150 s, 0 red lines
      (the texture-target mismatch is gone), 0 audio underruns; fresh save: mode select, opening, lit mine tunnel,
      walking moves the camera. All three recorded issues are resolved on this engine; scenario now ends with a
      punch at the fresh-save "Y Punch" prompt (~141 s).
- [x] **mkw device loss: fixed (wave `docs/waves/MKW-LOSS.md`, two delegated rounds).** Cause: descriptor-buffer
      stage sets wrote only their shader's own slots; chunks are reused by later recordings, so the other slots kept
      an earlier recording's descriptors naming its (since freed) staging, and a stray GPU access through one (storage
      slot 33 of a vertex shader's set during task/mesh draws) read freed memory. Correction: every set's 64 buffer
      slots are written (unused uniform -> zero buffer, unused storage -> a write-only sink), then its own bindings
      (`exec.rs` `write_db_set`; `NXGFX_DB_FILL=0` restores the old behaviour for diagnosis). Verified: the
      deterministic poison reproducer runs clean (`20261124-162011-repro-final`, `-163340-repro-final2`; canary 0
      stray writes vs 6-15 without), two plain runs 17/17 with no loss (`-162115-plain1`, `-162441-plain2`, sheet read:
      road, kart, title, Single Player menu). graphics lib 176 passed (lead re-ran).
- [x] mkw follow-ups (wave `docs/waves/MKW-RACE.md`): the scenario reaches and drives a Grand Prix race (fresh
      save: unskippable opening, A through the menus); stray slot access fixed (every graphics stage set's layout
      names every graphics stage incl. task/mesh; canary 0 strays with the fill off); race FPS 7.2-9.9 -> 14.0-16.1
      (crashed-pipeline draws skipped without per-draw relinking: 17 ms -> 0.3 ms; pool scans skipped when an
      earlier scan covers the op and no CPU event signal came since). Audited; audit HIGH fixed by the lead.
- [ ] **mkw: race FPS 29.1 of 60 — the frame is a 30 FPS hold, the real chain is ~28 ms.** MKW-PERF4 (2026-11-25,
      `docs/waves/MKW-PERF4.md`, `build/play/20261125-043019-mkw-perf4-final` 28.7 FPS, sheet read by the lead, runtime
      46 passed): the game threads are mostly idle; the frame step holds each frame for 2 vsyncs, and a 1-vsync frame
      really costs 27.4-28.8 ms (game work then replay in series: main waits in `nvnSyncWait` for the replay to reach
      its fence; replay ~15 ms = pool sync 8-9 + sets ~7). 60 FPS needs that chain under ~18.3 ms; smaller cuts do not
      move FPS. MKW-PERF3's "game waits 7.4 ms / its threads take 27 ms" read an incomplete counter. Kept: per-thread
      split in the CPU profiler (`profile=1`). Scan-ahead thread tried and reverted (no lead time: the game queues just
      before the replay takes). Next: which fence main waits on; if its own frame's, cut replay latency (translate
      command buffers as recorded on the worker threads — large). Owner decision pending: keep `fps_hold` (steady 30)
      or drop it (~35-36 uneven); lead recommends keeping it.
      MKW-PERF3 (2026-11-25, `docs/waves/MKW-PERF3.md`, **accepted**: independent audit
      found draw-word keying, module facts and write-watch batching safe; one LOW, `NXGFX_TEX_WB=1` only: a resetting
      scan over a <= 1 MiB gap clears the watch bits `cpu_written` reads for a texture stored in that gap, so a
      write-back could overwrite newer CPU bytes — pre-existing with the old gap rule, now unbounded in total gap
      length per call; fix if `TEX_WB` is ever turned on by default): scans
      11.0 -> 8.5 ms a frame (write-watch batches skip gaps > 1 MiB), FPS 28.8 -> 29.1 (within run-to-run noise);
      `build/play/20261125-034357-mkw-perf3-final`, sheet read by the lead, graphics lib 193 passed (lead re-ran).
      **The replay thread is no longer the limit:** the game waits only ~7.4 ms a frame on syncs; its own threads take
      ~27 of ~34 ms. Next: profile the game threads (engine calls on them, the pool lock the completion thread holds
      ~6 ms a frame, recompiled guest code). Cross-submission patching (~520 whole sets, ~2 ms) parked.
      Earlier: (MKW-PERF2: sets reused/patched, 22.9 -> 28.8, audited; MKW-PERF: 17.7 -> 22.9, 5% low 13 -> 19; GPU event
      landings no longer void scan coverage, parked resumes reuse covering scans). Left a frame: ~3 write-watch scans
      ~14.5 ms (the frame's first must ask every pool), descriptor sets ~12 ms (~5,000 a frame, ~4% reused: per-draw
      uniform buffers change the key), replay ~8 ms; the replay thread is the bottleneck. Next: descriptor sets.
- [x] Texture targets (wave `docs/waves/CUBE-TARGET.md`): MKW's ~414 `target 1 ... target 6` lines (6 = 2D MS
      array, not a cube) -> 0 via a one-layer 2D-array view (the descriptor decides the fetch); Z-A's 34 `1 ... 8`
      (cube) lines -> 0 via a six-face cube twin of its one-layer 32x32 fallback texture (`NXGFX_CUBE_TWIN=0` off).
- [x] **plza PASS** (`build/play/20261124-182844-plza-final`): fresh save through appearance and confirm, train
      arrival, platform with the Move HUD, walking into the station hall; 166/166, 59.6 FPS (5% low 57.9), 0 red lines.
- [x] **mk8d PASS** (wave `docs/waves/MK-PAIR.md`, `build/play/20261124-193052-mk8d-lazykeep`): fresh save to a
      driven Grand Prix through lap 2/3, 59.9 FPS (5% low 59), 0 red lines (were 49), 0 zero-stand-in texture binds
      (were 4314: finalized-but-unused lazy registrations are kept), peak memory 4950 MB (was 5228); the remaining
      >50 ms frames are the harness's own screenshots. A one-layer 2D-array view sampled as a cube gets the cube twin.
- [x] **mkw180 races** (`-193941-mkw180-final`, 16.9 FPS) with the base game's route. Owner question: this entry
      builds the same base package as `[mkw]` (no 1.8.0 update attached or on disk).
- [ ] **Pool finalize ordering: shelved** (wave `docs/waves/POOL-ORDER.md`, diff `docs/waves/POOL-ORDER.patch`). The
      redesign fixed the audit's three findings and MK8D's "in no memory pool" lines, but hangs MKW a few frames after
      a screenshot (3/3; HEAD 0/1). MK8D passes without it (those lines are cosmetic, at transitions). Revisit only
      with the hang explained. Separate: the lazy-keep `materialize_all` spike (MK-PAIR audit).
- [ ] mkw `texture ID 337: target 2 bound where the shader samples target 1` (3D noise read by a 2D instruction;
      fetch semantics unconfirmed). Risk to check off NVIDIA: single-sample views bound where SPIR-V declares MS.
- [ ] Startup check of per-stage descriptor limits (`maxPerStageDescriptor*`, `maxPerStageResources`) for the shared
      layout: every graphics stage now sees all five stage sets (~880 descriptors); fine on NVIDIA, unchecked elsewhere.
- [ ] _(done, history)_ mkw follow-ups as first listed: (a) name the shader that reaches set 0's slot 33 during task/mesh draws (with the fill its
      writes land in the sink: possibly lost rendering data; A/B in the wave report points at the task/mesh draw
      reading set 0); (b) FPS in the window 7.1-7.4 vs round 1's 12.5 (pool sync 48 vs 20 ms/frame after ~60 s;
      not shown to be caused by the fill); (c) the scenario reaches the attract loop, not a race: the title's
      "Press L+R" appears at ~141 s, after the scripted L+R at 49 s.
- [ ] _(history)_ mkw device loss investigation notes: `build/play/20261124-125344-mkw`: exit 71 at 19.1 s, draw
      #7708 direct 1D 6x1x1 mesh, task `fea1d6ca04512de1` / mesh `ddc034e8e009ebfc` (same pair as before; the payload
      fix did not cure it). Driver: invalid read `0xd468e7000`, "0 recorded device ranges of the replay hold it".
      Neither shader uses raw pointers (no capability 5347), robust access v2 is on, so a descriptor in this draw
      points at memory no live allocation holds: a stale/recycled buffer. Same run with `NXGFX_MESH_BINDINGS=1`
      (CPU-only bookkeeping, slower mesh draws) did **not** fault: title, menus, Mario driving at 90 s
      (`20261124-125448-mkw-bindings`). Timing-dependent -> lifetime race: something released or recycled on the
      CPU timeline before the GPU's last use. Reproduced plain (`20261124-125955-mkw-repro`: exit 71 at
      15.0 s, same draw shape, invalid read `0x37fb1f000`; the address moves run to run). `NXGFX_CHECKPOINTS=all`
      (address report) also hides it (90 s, no fault). "0 recorded device ranges" only means the address report
      was off (`note_use` needs level 2), not that nothing held the address.
      Ruled out by source: `stage_bytes` memo (per submission, cleared at recycle); `QState` caches `scache` /
      `cbank` / `tpart` / set cache (all serial- or POOL_GEN-guarded); to-end storage ranges (`room` stops at the
      window, `resolve` backs every granule of the widened range); the resolve fast-path cache (stores window /
      bound-granule spans; granules are only released with their pool, which bumps POOL_GEN); the recorder does
      not disable set reuse. Task shader bindings: support, cp_c0, cp_c3, cp_c4 (uniform), cp_s0/cp_s1 (storage
      from c0+412/+416), `mesh_draw` (the staged {count, first} record). No textures.
      Open lead: guest-timeline frees (`pool_del`, texture destruction) defer with `defer_after_use` against the
      last *submitted* fence, while replay of already-recorded command streams can lag behind the guest.
      Next: a lock-free, fixed-size per-draw descriptor-address ring for task/mesh draws (no allocation, no mutex)
      so the fault report names the binding without changing timing.
- [ ] **SONIC-SMASH** (2026-11-25, `docs/waves/SONIC-SMASH.md`, **accepted** after an audit fix; all on GPU 1). Audit:
      3D twin, vpool refusal and the Windows budget safe; MEDIUM fixed by the lead: on Linux a guest commit is only an
      `mprotect`, so subtracting committed bytes from resident+swap could keep the prewarm budget from ever tripping
      (OOM risk) — the guest subtraction is now Windows-only (`pipe.rs` `engine_private_bytes`). LOWs recorded: a 3D/cube
      twin copied in `sub.pre` shows the previous submission's content if rendered and sampled in one submission; an
      unsupported 3D format is re-queried every submission; an aligned unmap after a refused one in the same batch is
      not applied (pre-existing); budget checks walk the guest ranges per pipeline: Shadow 29.99 of 30
      FPS, red lines 218 -> 1 (the blacklist notice): `SXSG-B01` is the game's own unmap passing 64 KB tile counts as
      byte offsets (refused as before, now logged once per pool, no rebind for refused requests); texture 519 gets a
      one-slice 3D twin. Sonic 31.5 FPS, Green Hill. Smash 58.15 of 60, frames > 50 ms 117 -> 18, worst 830 -> 271 ms:
      the stutters were mid-match pipeline compiles — prewarm's 2 GB budget counted the guest's own memory growth
      (fixed: subtracts guest heap + physical maps); 874 of 1,712 pipelines still prewarmed, 18 hitches left.
      Lead read all three sheets: in the final windows Shadow and Sonic stand still (route input ends earlier; timers
      run, rings collected before) — controllability over the window is thin evidence. Open: Smash prewarm budget
      sizing, Smash audio underruns (17 over the run), repeated-match memory, Shadow/Sonic routes that keep moving.
- [ ] Other failing titles one at a time, each ending with a fix and its run: mkw180 (task/mesh fault, race stability; GAR-5), plza (cube mismatches, pacing), mk8d
      (stutters, memory), shadow / sonic (resource diagnostics, blacklisted pipelines), smash (pacing)
- [ ] Blocked titles: oot (native NDVE stays off; submission-4764 fault), violet (first-run progression, display reset).
      2026-11-25 OOT-PIPE (parked after one run): **the `d15370a382630d4e` compile no longer hangs** — made in ~70 ms
      (likely a pipeline-cache hit), 1,316 presents, title screen drawn (`build/play/20261125-084021-oot-pipe-base`).
      Owed: a cold-compile timing and the menus toward first gameplay. Violet: relink interrupted
      (`buildIncomplete: true`), its scenario needs a ~10 min fresh-save route.
- [ ] Final pass: every scenario, one run each

## Execution update — 2026-11-23, thermal monitoring (history)

**Reference-alignment implementation update:** session `ses_df6763ff3ffey6Q3EHUCcHhGwN` wrote source corrections
and CPU regression sources; reviewer `ses_df650e263ffe4Dd0lemSXGP0HS` rechecked GAR-1/2/3/4/6 and GAR-R1 closures.
GAR-5 is an open, separable existing raw-writeback defect, not a passing ignored test. Reports:
`waves/GPU-ALIGNMENT-PATCHES.md` and `waves/GPU-ALIGNMENT-REVIEW.md`. Candidate is accepted for gated CPU compile/test
only; no compilation, tests or deployment yet. Live CSV header928/rows930 mismatch and another session's
four-worker Leonida build block execution. Owner was asked for a fresh `docs/cpu_reading_live.CSV`; only the old
file is currently present. No implementation/review agent or owned workload is running; native NDVE/GPU hold stays.

**Post-reboot planning update:** the owner reported a hypervisor-error crash, disabled an unspecified Windows
feature and restarted. Session `ses_df69d14e5ffemcb1w3x2275zNt` completed the resumed source-only reference review:
[`waves/GPU-REFERENCE-PLAN.md`](waves/GPU-REFERENCE-PLAN.md). Lead reviewed the report and primary source anchors.
First proposed implementation is exact operand/view resolution plus a prepared private NDVE shadow transaction;
no patch or execution was performed. No external package is proven missing. GPU/build holds remain; exact crash
causality and postboot telemetry are unverified. Planning agent is complete, with no owned workload running.

**11:05 update:** the subsequently authorized Wonder rebuild/run stopped before creating any workload child:
CPU package 91 C at preflight, then lead verified advancing 93–95 C readings. A separate OpenCode session was
running a 12-worker Leonida shader/build pipeline. This session did not terminate it. Wonder artifacts, library,
saves and shared stops are unchanged; no gameplay validation occurred and no retry is queued. Agent session
`ses_df748c0d1ffeNBIDicHO542s0O` is finished; report `waves/THERMAL-WONDER.md`. Thermal/concurrent-workload review
is the active blocker; earlier build completion below remains valid.

This update supersedes the historical active-lease and baseline statements below. Current source baseline is
`143515097a0fa134c6fc4be839971692a39ac4de` plus preserved extensive dirty/untracked work. **Owner hold on games
and GPU workloads remains active.** A separately authorized monitored production release rebuild is now complete;
that exception is consumed. Current incident status is `waves/THERMAL-INCIDENT.md`.

| Wave | Session / state | Scope | Lead result / remaining work |
|---|---|---|---|
| THERMAL-HWINFO | `ses_df748c0d1ffeNBIDicHO542s0O`; complete, accepted monitoring/CPU-test scope | CSV source/snapshot/verification scripts, harness provider wiring, idle regressions, ignored local monitor config, `waves/THERMAL-HWINFO.md` | 37 focused passed; full Python 157 passed / 1 optional skip. Actual advancing HWiNFO feed verified at 50–52 C. Lead source/test/evidence review complete. Existing pending GPU-stop journal retained. Production deployment and GPU verification remain blocked by owner hold. |
| THERMAL-BUILD | Same session; complete, deployed and independently verified | Build script Jobs/staged promotion, watched build supervisor/provenance, CPU-idle tests, `waves/THERMAL-BUILD.md` | Owner authorized build only. New `dist/nxrs` contract 2 and four artifact hashes verified; CPU/GPU peaks 68/61 C across 282 observations. 20 supervisor tests passed. Corrected ownership and completion-ordering failures preserved in history. Root target removed; one 249.7 MiB prior release retained. Game binaries unchanged; game/GPU hold and stops remain. |

No active agent or compiler/GPU lease for these assignments. Agent final read-only census at 10:22:52 local found
no matching worker processes; owner HWiNFO PID 27844 remained running. Lead independently compared deployed
hashes and confirmed root target absent. No commit or production-save change.

## 1. Baseline and acceptance target

- HEAD: `7688e33ef5ef47e64322c7aed0f84c471e9c8b99`; extensive pre-existing uncommitted work, preserved.
- Last measured candidate library: `target/OOTWAIT/release/nxfront.lib`, SHA-256
  `6827ce497d42a30b199ae82a659666330ca9390dd03afd8977ba28f43ba83895`, recorded in
  `build/HARNESS3/dkb-fixed/link.json`. Exact build-completion time was not recorded; do not invent one.
- Last checks: graphics 95 passed / 3 ignored; Python 34 run / 1 optional skip; release front built.
- Evidence: `build/HARNESS3/outcome.json`, `build/NDVE/validation.json`, `build/OOTWAIT/investigation.json`.
- **GPU gate blocked:** `build/HARNESS3/GPU-STOP.json` records the MKW reset at 13:57:10. No GPU lease is granted
  initially. No baseline/candidate re-run, replay, or GPU test while that gate is blocked.
- Target: each owned title reaches first controllable gameplay with correct readable rendering, target-rate pacing,
  acceptable audio/memory, no fatal/device loss and no unexplained skipped work/resource errors in the declared window.
  Short gameplay acceptance does not certify a whole story/board/minigame. Diagnostic profiling is followed by an
  uninstrumented acceptance run; unknown/missing evidence never passes. No all-library pass currently exists.

## 2. Ordered work packages

| ID / priority | Specialist and deliverable | Dependencies | Exit evidence |
|---|---|---|---|
| **DG-01 / P0** | Graphics: offline MKW fault analysis; distinguish descriptor/resource lifetime from OOB arithmetic; fix a demonstrated invariant or add a bounded discriminator | Retained HARNESS3 fault, no GPU | Source-backed causal chain or narrowed unknowns, meaningful CPU regression, exact next GPU discriminator; independent audit before resumption |
| **DH-01 / P0** | Harness: enforce persistent fault-stop across generic scenario entrypoints; reliable stop/cleanup and provenance | Protocol; current reset evidence | Failure-injection tests for latch/refusal, cleanup and save restoration; old fault evidence retained; no game needed |
| **DS-01 / P1** | Shaders: resolve NDVE eight-versus-sixteen-product rounding contract against retained translation and reference; keep replacement disabled | CPU/offline evidence; no GPU needed for initial work | Explicit arithmetic target, adversarial CPU oracle, source/SPIR-V derivation; no unsupported console-equivalence claim |
| **DV-01 / P1** | Runtime/verification: resolve the pre-existing public raw-pointer clippy blocker under its own narrow lease | Recorded `rt.rs:119` diagnostic; no GPU | Correct ABI/safety contract and targeted regression as warranted; graphics clippy can actually reach the changed crate; no blanket lint suppression |
| **DG-02 / P1** | Graphics: DKB first failing render stage—geometry outputs, attachments, lighting and final compose—using retained captures before another game run | DG-01 review; existing DKB captures; future GPU grant for replay | First divergent producer/consumer demonstrated, regression fix, same-state visual comparison, eventual input-responsive scene |
| **DS-02 / P1** | Shaders/graphics: OoT compilation and NDVE integration | DS-01; arithmetic + resource/layout preflight; GPU gate | Compiler blocker removed or certified replacement; actual guest layout/bindings, sampling and shadow-frame comparison; no skipped convolution/fake fence success |
| **DG-03 / P1** | Graphics: MKW haze and CPU pool/descriptor cost | DG-01 corrective evidence and reviewed GPU resumption | Matched-state correctness and performance runs; no reset, no unexplained skips; native-rate usable race |
| **DR-01 / P2** | Runtime/OS: title diagnostics, audio and progression regressions only where attributable | Named game findings from DG/DS | Minimal ABI/state/service regression; input/audio evidence; no speculative service-success overrides |
| **DC-01 / P2** | Campaign operator: affected-title regression then remaining library sweep | Corrective waves accepted; GPU gate reopened for specific runs | One guarded title at a time, visual review, complete telemetry window, saves/cleanup; per-title pass/fail/unknown |
| **DA-01 / P2** | Fresh audit: reconcile implementation/acceptance with outstanding port parity | Wave reports and campaign evidence | Findings document; every claimed completion tied to evidence; unresolved items remain open |

### Immediate execution sequence

1. DG-01 offline graphics wave; lead tests/review. Diagnose the captured invalid read before any GPU experiment.
2. DH-01 harness wave; lead failure-injection tests. Ensure another entrypoint cannot bypass the retained fault stop.
3. DS-01 arithmetic wave; lead CPU/offline checks. Separate an exact translated contract from an approximate reference.
4. Fresh audit of these patches and evidence; return defects to their original agents.
5. Decide next wave from findings. A successful CPU wave is not automatic permission to reopen the GPU gate.

Each subsequent package is split into a small diagnosable wave with its own file table and explicit leases. Runtime,
launcher or unrelated shader modules are not assigned merely because they are in the worktree.

## 3. Library acceptance backlog

| Title/group | Current unresolved evidence | Next acceptance action after relevant fixes |
|---|---|---|
| DKB | Black world/HUD only; 20.845 FPS; 30 window skipped draws in latest profiled run | Isolate first bad render stage; matched scene, then uninstrumented control test |
| MKW | Direct mesh invalid read/reset despite robustness2; earlier haze/~6.6 FPS and blacklists | Resolve fault before GPU reuse; resource trace then rendering/performance |
| OoT | Driver compute call `d15370a382630d4e` does not return before bounded shutdown | Arithmetic/preflight + pipeline solution; first gameplay remains owed |
| MK8D, Arceus, Z-A, Shadow | Specific visual fixes verified; residual shader/resource diagnostics | Recheck affected scenes on final build; classify/fix remaining diagnostics |
| Jamboree | Plaza progression fixed; 36 red diagnostics | Short controllable plaza verification and diagnostic closure |
| Smash | Gameplay but 56.105 FPS/39 stutters; longer memory evidence missing | Same stage/opponent pacing comparison; steady-state memory test if still indicated |
| Wonder | Blacklist, loading-memory trend, underrun and incomplete window | Complete declared gameplay window; separate loading from sustained growth |
| Anime Girls | Banding, no trustworthy console/reference comparison | Matched reference or demonstrated shader defect; no global dithering workaround |
| Sonic Generations | Gameplay historically reached; tint not certified corrected | View current scene and shader/color evidence |
| Violet, Beach Bounce and remaining inventory | Historical runs, no current clean-library certification | Enumerate `library_play.py`/campaign inventory and run short acceptance after shared fixes |

## 4. Active leases and wave ledger

Global compiler slot: **DG-02 agent, source-fix lease**. GPU slot: **NONE until a concrete correction passes CPU review; DC-02 attempt remains consumed**.
Active subagents: **DG-02 owns DKB through concrete correction**. No game/claim/compiler currently remains.
Lead owns `AGENTS.md`, this plan, `31-DELEGATION-PROTOCOL.md`, and current-summary sections of `PORT_PLAN.md` /
`INTERFACES.md`. Agents own only their assigned wave record and implementation paths.

| Wave | Session / state | Writable scope | Compile / GPU grant | Lead result / next action |
|---|---|---|---|---|
| DG-01 | `ses_e00753fb0ffePfUHwApE6hTWL9`; **accepted CPU/reviewed diagnostic scope** | `exec.rs`, `lost.rs`, `lost_bindings.rs`; wave record/evidence | Lease released; GPU NONE | Lead graphics **107 passed / 3 ignored**. Audit recheck closes A01-1 and A01-2. Cache evidence survives production transitions; setup-span/explicit-marker selection preserves relevant draws. MKW cause and device behavior remain unproven |
| DH-01 | `ses_e00679dc8ffeSxuFrY62kiMmkn`; **accepted CPU/reviewed scope** | `run_game.py`, new `gpu_stop.py` / `test_gpu_stop.py`, wave record/evidence, seeded `build/GPU-STOP.json` | Lease released; GPU NONE | Lead Python **64 run / 1 optional skip**; 30 new failure-injection tests. Audit no blocking finding. Persistent latch active; documented OS/successor limitations retained |
| DS-01 | `ses_e005ca25bffe0qKZI5N7jDm3rs`; **accepted CPU/offline/reviewed scope** | NDVE prototype/oracle/contract tests/ignored GPU expectations and shader comments; wave record/evidence | Lease released; GPU NONE | Lead graphics **105 passed / 3 ignored**; symbolic 128-output ordered 16-product trace and offline SPIR-V checks pass. Audit no blocking finding. GPU/guest parity unproven; replacement off |
| Audit-01 | `ses_e004e33b2ffek9Z6WAgDtzmPF4`; **complete, findings closed** | `docs/waves/Audit-01.md` only | Lease released; no Cargo or GPU | Two initial P2 DG defects returned and corrected; recheck found no remaining actionable defect in scope. DH/DS CPU/offline dispositions stand |
| DV-01 | `ses_e0010c591ffevWw2qVdzuhnTNr`; **accepted CPU/doc/reviewed scope** | `rt.rs`, `image.rs`, `shot.rs`, `nvn/tex.rs`; queue unchanged; wave record/evidence | Lease released; GPU NONE | Runtime **45 + 1 doc**, graphics **116 passed / 3 ignored + 2 doc**. Both clippy commands exit 0 with baseline warnings. Failed readback retains pending staging and cannot produce successful capture; Audit-02 no introduced blocker |
| Audit-02 | `ses_dfff69fb9ffeE2MBrmexgS9dsR`; complete | `docs/waves/Audit-02.md` only | Lease released, no compiler/tests/GPU | Accept DV-01 scoped correction. Separate baseline P2 debt: partial allocation cleanup (`scale.rs`/`vk/mod.rs`) and crop-end overflow (`shot.rs`) |
| DG-02 | `ses_e00753fb0ffePfUHwApE6hTWL9`; **accepted CPU/reviewed diagnostic scope** | `exec.rs`, `render_chain.rs`, `image.rs`; wave record/evidence | Lease released; GPU NONE | Lead **123/3**, Audit-03 closes A03-1 after actual owner-promotion regression. Current DKB causal fix remains open; bounded discriminator technically ready |
| Audit-03 | `ses_dffd66cafffeXYIGYnCp0WM0HO`; complete, A03-1 closed | `docs/waves/Audit-03.md` only | Lease released; no compiler/tests/GPU | Second recheck accepts immutable creation provenance and supported diagnostic scope; no GPU authorization by reviewer |
| DC-02 | `ses_dffa6ce08ffeUQz6oFaYGobfFc`; attempt consumed; **recovered/closed** | Own evidence/recovery scripts and staged save only; originals read-only | Lease released; GPU NONE; no retry | DKB exit 0, 165.5 s, no reset, frame 5716 / seven completed raw files. Clock defect preserved historically and corrected for future code; **5 recovery tests passed**. Staged save restored/verified across 8 files, post-run state retained separately; originals/latches/receipts unchanged |

### DC-02 lead resumption decision (one attempt)

Purpose: distinguish missing depth/geometry coverage from absent later colour production using the newly audited
actual-view and immutable-backing evidence. Retained 439 PNGs cannot answer this; they omit depth/raw HDR and predate
subsequent fixes. This is a new evidence discriminator, not a cooldown-only repeat or an MKW retry.

Proposed scope: one DKB process, 170 s ceiling, one gameplay dump frame, selected passes 18/27 (roles verified from
concurrent metadata, never assumed), extents 1440x810/2880x1620, 16 copies/64 MiB payload maximum. Disable legacy PNG
and texture dumps; preserve rendering options and blacklists. Source copies and SHA-256 save/cache provenance,
exclusive guard, no overlapping builds, first fault stop, no automatic retry. The lead must record candidate hashes,
exact command, archived shared latch and bounded lease before the driver may start. Restore a shared stop after
the attempt even if it succeeds; original HARNESS3 latch stays untouched throughout.

**Authorized by lead after the above gates, 2026-11-21, candidate verified at 18:43:44 -05:00.** Release build
`cargo build --release -p front --jobs 4` completed successfully in 48.77 s; isolated relink completed. Library
SHA-256 `f05e631ccc1731370e6b407258c731435e38893acede3ad5fdb098487100984a`, DLL
`1a4679a6be50c4bc5b395550069c5fdd19233171b86db34cbedf3b5bc9d5744d`. Full decision and identities:
`build/DELEGATION/DC-02/lead-authorization.json`. Exact lead command, cwd `D:\Projects\RecompLauncherEngine`:
`py D:\Projects\RecompLauncherEngine\build\DELEGATION\DC-02\prepare.py run --receipt D:\Projects\RecompLauncherEngine\build\DELEGATION\DC-02\lead-authorization.json`.
Only this validated process redirects the shared pending-journal lookup; the durable root pending marker blocks
fresh generic runners even if the operator dies. After the attempt, shared STOP is reinstated and no further GPU
grant exists. MKW is not part of this resumption. No automatic second attempt if capture is missing or inconclusive.

### Remaining-work checklist (current continuation)

- [x] DV-01: runtime raw-pointer contract and blocked lint gate; readback lifetime correction CPU/doc-tested and audited.
- [ ] DG-02: isolate DKB's first incorrect render stage; correct a demonstrated defect.
- [ ] DS-02: guest tensor layouts, bindings/residency/alias checks and full-graph preflight, replacement still off.
- [ ] DG-03: MKW mesh resource/address fault, then haze and CPU-side costs.
- [ ] Independent audit and closure of blockers for each new corrective wave.
- [ ] Explicitly reviewed, bounded GPU resumption; preserve fault history and stop on any new loss.
- [ ] Corrected NDVE GPU parity and actual guest-frame shadow comparison before replacement enablement.
- [ ] DKB/MKW/OoT first controllable gameplay with correct visuals and complete telemetry windows.
- [ ] Remaining library shader/resource, pacing, audio, memory and colour findings (section 3).
- [ ] Affected-title regressions and full remaining inventory acceptance, including save/cleanup audits.
- [ ] Port-parity and launcher/package acceptance reconciliation; current wave/status documentation.
- [ ] Separate Audit-02 baseline debt: partial-allocation cleanup and screenshot crop-end overflow.

### Corrective-work rule after DC-02

For unresolved titles, diagnostics are inputs and cannot complete a wave. DG-02, DS-02 and DG-03 must identify a
specific source defect, implement and CPU-test the correction, then proceed to a guarded game/GPU verification when
safe. A report that only proposes another recorder, probe or evidence run is returned unless a genuinely external
blocker makes source correction impossible. Reviews focus on correctness of the fix, not iterative refinement of
diagnostic tooling. Current priority is DKB correction from the already captured frame-5716 raw chain, followed by
OoT integration and the MKW mesh fault.

### Gate and send-back record

DG-01 CPU acceptance and independent review completed; GPU/live gates remain open. The graphics clippy dependency
blocker (`src/runtime/src/rt.rs:119`, `not_unsafe_ptr_arg_deref`) is pre-existing and outside DG-01; it remains an
explicit blocked gate, not a clean lint claim. Each wave records its file table, exact gates and eight report headings.

Audit-01 send-back: A01-1 evidence must live with cached descriptor identity across compute and uncacheable
interludes; A01-2 selection must preserve explicitly reported markers and draw/setup associations despite pre/main
numeric reordering, with explicit capped omissions. These are diagnostic defects, not evidence of MKW's cause.
Original agent corrected both; lead full graphics suite passed 107/3; independent reviewer closed both IDs.

### First execution cycle outcome and next dispatch

- Protocol and plan are in force through `AGENTS.md`. Three implementation agents were run serially, followed by
  an independent audit, original-agent send-back, lead test rerun and independent recheck.
- Accepted uncommitted work: DG-01 bounded mesh-binding discriminator; DH-01 persistent generic-runner stop;
  DS-01 retained-Vulkan 16-product prototype contract. None is a clean-game or GPU-parity certification.
- Latest lead commands (cwd `D:\Projects\RecompLauncherEngine`): `cargo test -p graphics --lib --jobs 4`
  with `CARGO_TARGET_DIR=D:\Projects\RecompLauncherEngine\target\OOTWAIT`: **107 passed / 3 ignored**;
  `py -m unittest discover -s scripts -p "test_*.py"`: **64 run / 1 optional skip**. No GPU tests were executed.
- Offline shader validation passed in DS-01. Graphics clippy is still blocked/unassessed by the pre-existing
  runtime diagnostic; DV-01 owns that debt. No new release build/relink, commit, push or GPU resumption occurred.
- Next dispatch: DV-01 CPU-only lint/safety-contract blocker, then DG-02 retained-capture analysis and DS-02 guest
  layout/preflight in separate leases. DG-01's proposed mesh discriminator is reviewed but **not authorized to run**;
  any future GPU slot requires the explicit resumption entry, finite watchdog and preserved fault history in protocol §3.
- Remaining title outcomes are unchanged by this offline cycle. Both `build/GPU-STOP.json` and
  `build/HARNESS3/GPU-STOP.json` remain active. No queued package is running in the background.

## 5. Completion and handoff

An accepted wave records scope, tests, build identity, inspected artifacts, open gates and the next owner. A clean
game needs visible correctness and control evidence, not just a synthetic test or survival with blacklisted draws.
No commits/releases were requested. Every session ends with actual active-agent/process/claim state and retained
GPU-stop state; queued work is not described as running in the background.
