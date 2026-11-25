# 32 — Remaining engine work and delegated execution ledger

**Status: procedural history and execution ledger.** The active game-outcome plan is now
[`33-REFERENCE-DELTA-ROADMAP.md`](33-REFERENCE-DELTA-ROADMAP.md). Delegation follows
[`31-DELEGATION-PROTOCOL.md`](31-DELEGATION-PROTOCOL.md). This file retains prior assignments, evidence and
resource decisions, but it no longer defines product milestones. Source root: `D:\Projects\RecompLauncherEngine`.

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
- [ ] Scenarios for fresh saves: extend arceus / arceus-base / jamboree through their openings to first gameplay
- [ ] **Engine: no preemption inside long guest loops.** Fresh-save Arceus 99-114 s: low-priority thread 0x1004
      (prio 46) holds virtual core 2 in a loader loop with no SVC; the better threads wait `STEAL_GUEST` (5 s) per
      steal, presents fall to 0.2 FPS. Horizon would preempt it. Fix: a displaced-flag check on translated loop
      back-edges (generalising `rt_spin_gate`) so a displaced holder yields at once; needs a Cpu-struct field and a
      full retranslation of every title. STEAL_GUEST stays 5 s until then (200 ms broke DKB's loaders).
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
- [ ] mkw: race FPS 14-16 of 60. Frame 61-71 ms: pool sync 34-41 ms (parked-queue resumes each scan), descriptor
      sets ~12 ms, replay ~21 ms.
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
- [ ] Other failing titles one at a time, each ending with a fix and its run: mkw180 (task/mesh fault, race stability; GAR-5), plza (cube mismatches, pacing), mk8d
      (stutters, memory), shadow / sonic (resource diagnostics, blacklisted pipelines), smash (pacing)
- [ ] Blocked titles: oot (native NDVE stays off; submission-4764 fault), violet (first-run progression, display reset)
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
