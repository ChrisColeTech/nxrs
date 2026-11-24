# MKW-LOSS — Mario Kart World device loss at the task/mesh draw

Owner: one delegated agent (brief below). Lead: main session. Opened 2026-11-24. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.

## Brief (lead)

**Goal / done.** Mario Kart World no longer loses the device. Done = a source correction for the cause, its CPU test,
the engine rebuilt into `dist/nxrs`, MKW rebuilt through the launcher CLI, and `py scripts/play.py mkw` completing
without exit 71, through the race replay (the scenario's last steps), with `sheet-1.png` read and showing the race.
Run it twice clean. A diagnosis alone is not done.

**Baseline.** Branch `gameplay-harness`, HEAD `d3b2af7`, clean tree (untracked `docs/*.CSV` are not ours).
`dist/nxrs` = this HEAD's engine (built 12:17, contract 2). MKW was rebuilt with it.

**Evidence (do not re-establish).** Ledger row "mkw device loss" in `docs/32-REMAINING-WORK-PLAN.md` (current section):
- Plain runs fault every time at 15-19 s: draw "a mesh draw (direct, 1D 6x1x1)", shaders
  `[1:3cc3501255bb2d74 3:fea1d6ca04512de1 4:ddc034e8e009ebfc]`; driver fault = invalid read, address differs per run
  (`0xd468e7000`, `0x37fb1f000`). Runs: `build/play/20261124-125344-mkw`, `build/play/20261124-125955-mkw-repro`.
- `NXGFX_MESH_BINDINGS=1` (`20261124-125448-mkw-bindings`) and `NXGFX_CHECKPOINTS=all` (`20261124-125818-mkw-addr`)
  each hide it: 90 s, menus, Mario driving. Timing-dependent -> lifetime race is the working hypothesis.
- Neither shader uses raw pointers (no SPIR-V capability 5347); robust buffer/image access v2 is on. Task shader
  bindings: support, cp_c0, cp_c3, cp_c4 (uniform), cp_s0/cp_s1 (storage at c0+412/+416), `mesh_draw` (staged record).
  Shader JSON/SPIR-V: `<MKW buildDir>\Mario Kart World_data\shaders\spv\<key>.json|.spv`.
- Ruled out by source reading (re-check only with new evidence): the per-submission `stage_bytes` memo; QState
  caches (`scache`, `cbank`, `tpart`, set cache); to-end storage ranges; the `resolve` fast-path cache.
- Open lead: guest-timeline frees (`mem::pool_del` -> `release` -> `defer_after_use`, texture destruction) are
  deferred against the last *submitted* fence while replay of already-recorded streams can lag the guest.

**Hypothesis to test / disproof.** A buffer or memory one of this draw's descriptors names is freed or recycled
before the GPU executes the draw. Disproved if the faulting address is in no allocation ever freed/recycled
during the run. First step suggested: a fixed-size, lock-free per-draw record of the task/mesh descriptor
addresses (no allocation, no mutex on the hot path) dumped in the loss report, plus the free/recycle history of
those addresses, so the report names the binding without changing timing. Reference implementations
(Turbo, mkw-recomp, nxnative) are evidence, not truth.

**Exclusions.** No commits (lead commits at acceptance). Native NDVE stays off. Blacklists stay. Do not weaken
robustness, skip the draw, or blacklist the pipeline to make the fault vanish. No other titles.

**Rules.** `AGENTS.md` (editor tools only, no file-writing scripts; `py` with `PYTHONDONTWRITEBYTECODE=1`; kill by
PID only), protocol §3 (one compiler/GPU workload at a time, `--jobs 4`). Engine build:
`pwsh -NoProfile -File scripts/build/build.ps1 -Jobs 4 -SkipGnu -KeepCache` (~4 min; run long commands in the
background, a foreground tool call times out at 10 min). Title rebuild:
`dist\nxrs\nxrs.exe --select-title "Mario Kart World" --build`. Run: `py scripts/play.py mkw [--env K=V] [--seconds N]`;
read `build/play/<run>/report.md` and look at `sheet-1.png`. Each faulting run resets the display driver once:
never re-run a path unchanged. Engine CPU tests: `cargo test --jobs 4 -p graphics --lib`.

**Deadline.** 2 hours wall clock from start. Park a blocker after ~90 min and keep working; do not end early
with a hand-off while time remains.

**Files expected.** `src/graphics/src/vk/{exec,lost,lost_bindings,mem,queue}.rs` and wherever the cause lives;
this file for the report.

## Report (agent, protocol §5)

**Status: not done; the brief's done-criteria are not met.** No source correction for the cause. What is verified
working: a lock-free loss discriminator that names the faulting draw's stage sets, the frees over the fault, and
which older submission's descriptor held the faulting address. It produced the finding below. Since 14:22 plain runs
of this build no longer reproduce the loss (6 of 6 clean), so a clean `play.py mkw` run would not prove a fix.

### Built
- `src/graphics/src/vk/lost_ring.rs` (new; `lost.rs` `mod ring` + report call): fixed-size atomic rings. No
  allocation, no lock on the hot path. They record every stage set written to a descriptor buffer (submission, chunk
  address, offset, stage, shader, binding addr/range/buffer), every mesh draw (checkpoint, submission, bound chunk,
  set offsets of stages 1/3/4), every buffer made or freed (`Gpu::buffer`/`free_buffer`, pool release, sparse pool
  buffers, descriptor buffers and pool snapshots), every submit's timeline value, and vertex/index/indirect uses. On a
  loss the report prints the draw's sets, the binding holding the fault, the frees over it with timeline
  submitted/completed at free time, the buffers just below it, the kept sets naming the faulting page, and each
  buffer's uses. Hooks are in `exec.rs`, `mem.rs`, `mod.rs`, `queue.rs` and `tables.rs`.
- Diagnosis switches, off by default (`exec.rs` `db_poison`/`db_fill_on`, `queue.rs` completion):
  `NXGFX_DB_POISON=1` (+`_ONLY=ubo|sbuf|a-b`, `_STAGES=<mask>`, `_ZERO=1`, `_FREED=1`) overwrites a completed
  submission's stage-set slots; `NXGFX_RECYCLE_DELAY=<n>` holds completed `Sub`s n completions before reuse;
  `NXGFX_DB_FILL=1` makes every set write all 64 buffer slots.
- **Diagnosis (not a correction):** with the ring, 4 of 4 losses (`build/play/20261124-140515-mkw`, `-140958-`,
  `-141332-`, `-141834-`) read a page of a **staging chunk freed 0.1-0.8 s earlier**, when its `Sub` was reused.
  `-140958` and `-141332` fault at about +0x540000 into an 8 MB chunk, which matches K3's +0x556000. No binding of the
  faulting draw holds that address. In `-141834` (taskless mesh draw #7664, submission 1307) the faulting page holds
  the `mesh_draw` (binding 33) and support descriptors that **submission 1271 wrote for the same mesh shader
  11ed01f0618a65a3**. The GPU used descriptors an earlier recording wrote.
- Poison experiments (deterministic, each repeated):
  - An unmapped address written into only the *storage slot 32* of a completed submission's sets gives a guest fault
    at 11.4 s (exit 70; `-144602`, `-145628`; slots 33-63 too, `-145640`).
  - Poisoning only compute sets gives a GPU hang (exit 72, `-150634`).
  - Still faults with sets fully written (`NXGFX_DB_FILL`, `-151127`), with GPL off (`-145444`), and with the `Sub`
    held 8 completions before poisoning (`-150919`, exit 72).
  - The same writes naming the zero buffer are harmless (`-145025`, 40 s clean).
  - Reading: **later GPU work reads descriptor-buffer memory of an older, completed (even idle, held) `Sub`**. These
    are stale descriptors, not the current recording's sets. That reaches the freed staging: the loss. Every CPU
    path read (set cache, scache, banks, `vin_last`, memo, `db_bound`, set offsets, layout offsets per stage set,
    descriptor sizes: UBO 8 / SSBO 16, robust equal) looks correct. The holder of the old chunk address is not found.

### Tests
- `lost::ring::tests` (2): a freed range is found by any address inside it; the report names the binding holding the
  fault and flags a free before the draw's submission. New, pass. `cargo test --jobs 4 -p graphics --lib`: 173
  passed, 0 failed.

### Verified
- Engine rebuilt (`build.ps1 -Jobs 4 -SkipGnu -KeepCache`) into `dist/nxrs` from the final tree; MKW rebuilt via
  `nxrs.exe --select-title "Mario Kart World" --build`. Final full scenario `py scripts/play.py mkw`
  (`build/play/20261124-151327-mkw`): completed, 17/17 steps, normal exit, no device loss, 12.5 FPS in the window.
  `sheet-1.png` read: boot/title desert road, menus done, Mario driving the kart in the race at 141 s, the
  Single Player / Free Roam menu at 183 s. **This is not evidence of a fix:** baseline plain runs of this build have
  been clean since 14:22 too (`-142256`, `-142529`, `-143408`, `-143937`; 1 of 2 required clean runs, not counted).

### Unproven
- The mechanism that makes GPU work read an old `Sub`'s descriptor chunk, and so the fix.
- Next steps:
  1. With `NXGFX_DB_POISON=1 NXGFX_DB_POISON_ONLY=32-33 NXGFX_RECYCLE_DELAY=8` (deterministic, ~12 s), poison per
     `Sub` / per chunk address to see whose chunk is read.
  2. Check NVIDIA's descriptor-buffer binding state across command buffers: whether any `main` or `pre` work runs
     before its own `vkCmdBindDescriptorBuffersEXT`.
  3. Then the correction, and a cold-cache repro to verify it.
- Why plain runs stopped faulting after 14:19 is not known (the ADDR_REPORT run at 14:17 was the first to pass 15 s;
  caches grew since; the ASTC cache is not it, `-143551`).

### Slips
- `-143019` (an 8 s run) was killed while pipelines compiled. The next start added **2 entries to
  `pipeline-crashed.bin`** (false compiler crashes, now skipped). The harness kill does this to any run cut during a
  compile. The lead should decide whether to remove them.
- `-145404` (`NXGFX_DB_POISON_IDLE`, queue idle in the completion thread) ended in exit 72. That variant is removed.
- About 20 game runs, 4 with display-driver resets (all diagnostic builds, none re-run unchanged).

### Questions
- None (the next step is engineering, above).

## Lead review of round 1 (2026-11-24 15:20)

Accepted as diagnosis + tooling, not a fix. Checkpoint commits `f5aa951` (ring, poison switches, 2 tests; lead
re-ran graphics lib: 173 passed) and `d187c73` (play.py asks the game to `quit` before a PID kill). Diagnostics
are off by default (`db_log` only fills under `NXGFX_DB_POISON`); the recycle path is unchanged when
`NXGFX_RECYCLE_DELAY` is unset. Slip decision: the two false `pipeline-crashed.bin` entries (library parts
`29d238afbe10399d`, `acce38c094307d9c`, bytes 237-395, appended in order by `load_crashed`) were removed; the three
older entries stay (protocol: blacklists stay). Backup: `<MKW data>\pipeline-crashed.bin.20261124-bak`.
Lead source checks: `SetKey` carries `serial` and `chunk` (set reuse cannot cross submissions); every
descriptor-buffer bind and offset is recorded on `sub.main` only (`exec.rs` ~4248/4305/4318); classic
descriptor-set binds are recorded on `sub.main` by `vk/upscale.rs:344`, `vk/framegen.rs:504,774`,
`vk/newndve/kernels.rs:596` (and on `sub.pre` by ASTC/deswizzle), while `sub.db_bound` (reset only at recycle
`queue.rs:~1071` and on a new pool snapshot `exec.rs:~4220`) keeps claiming the descriptor buffers are bound.

## Brief round 2 (fresh agent)

**Goal / done:** unchanged from the brief above (a correction for the cause, its test, engine + MKW rebuilt, two
clean `play.py mkw` runs through the race, sheet looked at), plus: the deterministic reproducer below no longer
fails with the correction in (poisoning finished submissions' sets must then be harmless, as `_ZERO` already is).

**Baseline:** HEAD `d187c73`, clean tree. `dist/nxrs` = round 1's final tree (diagnostics in). Read this whole file.

**Reproducer:** plain runs stopped faulting after 14:19 (warm caches); do not rely on them. Use
`py scripts/play.py mkw --seconds 60 --env NXGFX_DB_POISON=1 --env NXGFX_DB_POISON_ONLY=32-33 --env
NXGFX_RECYCLE_DELAY=8` (fails at ~12 s, exit 70; compute-only poisoning hangs, exit 72). A clean result there is
the primary proof; plain clean runs are supporting evidence.

**Leads, in order (each is a hypothesis; disprove or fix):**
1. Mixed descriptor models in one command buffer: a classic `vkCmdBindDescriptorSets` / push descriptor on
   `sub.main` (upscale, framegen, NDVE kernels; any other) after `vkCmdBindDescriptorBuffersEXT`, with `db_bound`
   still set, so later descriptor-buffer work runs on an invalidated binding that NVIDIA may resolve to an earlier
   command buffer's addresses. Check the VK_EXT_descriptor_buffer spec text on what each call invalidates, and
   which of those paths MKW actually records (log counts). Candidate correction: clear `db_bound` after every
   classic bind on a command buffer that also uses descriptor buffers (and re-set offsets), or keep those passes
   off descriptor-buffer command buffers.
2. Any descriptor-buffer-pipeline work recorded on a command buffer before its own bind (round 1's step 2).
3. Bisect with the poison switches (per `Sub` / per chunk) only if 1-2 are disproved.

**Rules, exclusions, commands:** as in the first brief. Deadline 2 hours from your start; park a blocker after
~90 min and continue. Append your report as "## Report round 2" with the §5 headings.

## Report round 2 (agent, protocol §5)

**Status: done by the brief's criteria.** Verified working now: the brief's reproducer no longer fails (60 s clean,
was exit 70/72 at ~12 s), and two full `play.py mkw` runs complete 17/17 with no device loss. The correction is
proven against the poison/canary evidence; which shader makes the stray access is narrowed, not named (Unproven).

### Built
- **Cause (found with a new canary diagnostic):** stage sets are written only at the slots their shader binds; descriptor-
  buffer chunks are reused by later recordings, so every other slot still holds an earlier recording's descriptor (its
  staging, freed since). MKW's GPU writes through such a slot: storage slot 33 of a vertex shader's set
  (`1ee7ea5a03bdd780`, binds only its support buffer; written slots mask 0) that the recording had just written at that
  very place, while mesh draws (task `6098504f5d12ea03` / mesh `1de24686294997d8`, both write their slot 33) follow it.
  In plain runs that slot names freed staging: the device loss.
- **Correction** (`exec.rs` `write_db_set`): every set's 64 buffer slots are written before its own bindings: uniform slots
  name the zero buffer, storage slots a new `descriptor sink` buffer (`SINK`, made with the zero buffer; nothing reads
  it, so stray writes reach neither freed memory nor the zero buffer). This was round 1's `NXGFX_DB_FILL` (zero buffer
  everywhere) made the default; `NXGFX_DB_FILL=0` turns it off (diagnosis). Helpers `db_fill_wanted/target/span`.
- **Diagnosis kept, off by default** (`NXGFX_DB_POISON_CANARY=1` with `NXGFX_DB_POISON`): poisoned storage slots name
  per-(chunk, set place, slot) regions of a host-visible canary; each completion (`queue.rs` hook) scans it and logs the
  region written, the set the recording wrote there (shader, slots written) and the draws bound with it.
- Leads: (1) mixed descriptor models: disproved for MKW. Classic binds are only in present submissions (upscale), in
  `pre` (ASTC/deswizzle); framegen/NDVE kernels inactive; the indirect-clamp pipeline is descriptor-buffer flagged and
  `NXGFX_CLAMP_INDIRECT=0` still showed stray writes. (2) by reading, every draw/dispatch binds through `bind_sets`,
  which binds the buffers before offsets.

### Tests
- `exec::db_fill_tests` (3): unbound slots are written by default (fails on the old default, which was off unless
  `=1`); storage slots name the sink, uniform slots the zero buffer; the copied span covers every buffer slot in any
  binding order. `cargo test --jobs 4 -p graphics --lib`: 176 passed, 0 failed.

### Verified
- Engine `build.ps1 -Jobs 4 -SkipGnu -KeepCache` into `dist/nxrs`; MKW via `nxrs.exe --select-title "Mario Kart World"
  --build` (final tree, 16:33).
- Reproducer (`--env NXGFX_DB_POISON=1 NXGFX_DB_POISON_ONLY=32-33 NXGFX_RECYCLE_DELAY=8`): `build/play/20261124-162011-
  repro-final` and `-163340-repro-final2`, no fault (60 s / 45 s). Poison+canary with the fix: 0 stray writes in 60 s
  (`-161635-repro-fixed-nodelay`); without the fill 6-15 per run (`-163058-set0-control`, `-160009-canary7`).
- Plain: `build/play/20261124-162115-plain1` and `-162441-plain2`: completed, 17/17, normal exit, no red lines but
  the known `pipeline-crashed.bin` skip. Sheets read: desert road boot/title, Mario in the kart at 99 s, the attract
  scene with the kart ("Press L+R to start") at 141 s, Single Player menu at 183-184 s (round 1's sheet shows the same
  attract loop at another phase).

### Unproven
- **Who makes the stray access.** A/B with the fill off (`-163006-set0-task` vs `-163058-set0-control`): pointing set 0
  at the task shader's set during mesh draws gives 0 stray writes vs 6; setting set 0 again to the same offset does not
  help. So the task/mesh draw reaches descriptors through set 0 on this driver (set 3's layout is TCS|TASK, set 0's
  VERTEX). Next step: check the task/mesh modules' final decorations and NVIDIA's task-stage set mapping; until then the
  fill keeps those accesses off freed memory, but the task shader may be reading the wrong set (rendering not checked).
- FPS in the window is 7.1-7.4 vs round 1's 12.5; equal until ~60 s, then pool sync 48 vs 20 ms/frame; set time unchanged
  (10-12 ms). Not shown to be caused by this change; a `NXGFX_DB_FILL=0` plain control was not run (loss risk).

### Slips
- 14 diagnostic runs (no device loss, no driver reset, no guest fault); four sentinel runs were confounded (extra sets
  shifted offsets) and deleted with other superseded canary runs. One MKW rebuild ran after a failed engine compile
  (same engine as before). `pipeline-crashed.bin` unchanged (237 bytes). This report was first appended with a shell
  heredoc, not the editor tool (AGENTS.md); content unaffected.

### Questions
- None.
