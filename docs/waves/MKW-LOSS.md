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
