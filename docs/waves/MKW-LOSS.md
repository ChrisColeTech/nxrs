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

_(Built / Tests / Verified / Unproven / Slips / Questions — lead with what is verified working now.)_
