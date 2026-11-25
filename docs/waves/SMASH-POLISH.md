# SMASH-POLISH — Smash: mid-match pipeline hitches, audio underruns, repeated-match memory

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
parallel rules: `docs/waves/PARALLEL-RULES.md` (binding). **GPU 1.** Title: "Super Smash Bros Ultimate" only
(scenario `smash`).

## Brief (lead)

**Problem (recorded, `docs/waves/SONIC-SMASH.md` report, run `build/play/20261125-035528-smash-prewarm`).** Match
58.15 of 60 FPS, 18 frames > 50 ms (worst 271 ms): mid-match pipeline compiles. Prewarm still stops at its 2 GB
budget after 874 of 1,712 keyed pipelines (+2,285 MB not counted as guest memory; `vk/pipe.rs`,
`engine_private_bytes`). 17 audio underruns over the run against 2 in the baseline, not investigated. No
repeated-match memory check yet.

**Goal / done.**
1. Hitches: measure what the prewarmed pipelines really cost against other engine memory; then size the budget from
   that (or scale it to the machine), or prewarm the match's keys first. Done = a match window with 0-few frames
   > 50 ms that are compiles, at 60 FPS, memory bounded.
2. Audio: find why underruns rose (when they happen: boot, menus, match; which voice/renderer path; host buffer
   starvation vs late guest submission) and fix the cause. Done = underruns back at baseline or better.
3. Memory: extend the `smash` route (your scenario only) to play a second match after the first and show memory
   is flat between matches (or fix the slope).

**Exclusions:** other titles; no commits; don't edit docs/32. Pipeline prewarm and audio code are shared by every
title: keep changes general, not Smash-keyed.

**Tests / runs:** `cargo test -p graphics --lib`, `cargo test -p os --lib` as touched;
`py scripts/play.py smash --env NXGFX_GPU=1`; read the sheets. Final numbers come from a run under the machine lock.

**Deadline:** 3 hours wall clock. Report per protocol §5 in this file under "## Report".

## Resume (lead, 2026-11-25, after MKW-PERF5)

The wave now runs **alone** (no other wave in the tree; `vk/mem.rs` is free). Start from the parked report below.
Priority order: (1) the ~100 ms `write watch` calls during the intro movie — MKW-PERF5 measured the same walk at
1.2-4.4 ms in MKW and saw it swing 7.3-11.5 ms a frame between runs, so a fixed ~100 ms is a wait, not work: find
it (`NXGFX_CHAIN_TRACE` and `NXGFX_OP_TIMES` from MKW-PERF5 exist now) and fix it, then re-check MKW's race still
holds ~29 FPS with no new red lines (one run, GPU 0); (2) prewarm order/budget; (3) the second match. Any change to
the pool sync is a memory/coherence change: the lead audits it. GPU: either, machine lock still for perf pairs.

## Report (agent, protocol §5; resumed alone 2026-11-25 10:35-11:45)

**Results:**
- **Intro movie:** 12.8 → 59.9 FPS.
- **Audio underruns:** 17 → 2 over the one-match route, none in the movie and none in either match.
- **First match** [130, 193] s: 58.2 → 60.0 FPS. Frames over 50 ms: 18 (16 of them compiles) → 1 (not a compile).
- **Memory:** flat inside each match. About +110 MB per match follows the random new CPU fighter; three matches
  checked.
- **MKW race:** still 29.1 FPS, with no new red lines.

The machine was mine alone for every run (`build/locks/machine.lock` held for the whole wave, no other wave, game or
build). The Smash runs used GPU 1 and MKW used GPU 0.

### Diagnosis

1. **The ~100 ms `write watch` calls were waits on the address-space lock.**
   - A new field on the slow-call line shows the thread's CPU time over the call: kernel 0 ms, user 0 ms for every
     100-150 ms call (`build/play/20261125-104436-smash-wwcpu`). So the time was spent waiting, not working.
   - `NXGFX_RT_THREAD=0` did not change it (`-104050-smash-rt0`), so MMCSS is not the cause.
   - An outside per-thread sampler (NtQuerySystemInformation, scratch script) found the threads burning kernel time:
     guest threads 0x1026 and 0x1024 used 750 and 285 ms of kernel time a second, only while the movie played
     (`-104851-smash-thr3`).
   - The profiler (`--option profile=1`, `-105012-smash-prof`) put that time in the movie decoder's host calls.
   - Those calls check the client's event pointers with `guest::readable`, which calls `VirtualQuery` 2-4 times per
     event.
   - A timing probe (temporary, removed) measured about 100 `VirtualQuery` calls a second at 13 ms each, up to 100 ms
     (`-105259-smash-vq`). Each one ran on Smash's heap: region `+1.2 GB`.
   - `VirtualQuery` walks the page tables to the end of the address's committed run, holding the process's memory
     lock. `GetWriteWatch(RESET)` needs that lock, so every pool sync waited behind it. Replay prep reached 50-200 ms a
     frame, the game ran at 10 FPS, and AudioOut ran dry.
   - The underruns were in the movie (18-40 s), as the parked report found.
2. **Mid-match hitches came from the shader-part cap, not the prewarm.**
   - With a warm driver cache the prewarm now finishes: 1,804 of 1,809 keys, +1,879 MB (about 1 MB a pipeline).
   - Every run still compiled 15-90 pipelines in the match. They were new keys from the random CPU opponent and
     effects (the key file grows by about 45 keys a run).
   - Those programs' parts are normally made ahead (`prebuild`), but that stops once **process** private memory passes
     the fixed 6 GB `NXGFX_PARTS_PRIVATE_MB`. Smash's process passes 6 GB before every match, because the guest's own
     heap counts toward it (stopped at 48-127 s in all three earlier runs).
   - From then on each new program's parts were compiled at its draw on the replay thread: 136 such parts in the
     match of `-110100-smash-diagpipe`, in frames of 50-330 ms.
   - With the cap raised by environment variable (`-110521-smash-parts16`): 22 parts at draws, 15 pipelines made, and
     1 frame over 50 ms in [130, 193] s.
3. **Loading-screen underruns.** The two left in the final run (107 s and 310 s) come at the match loads. There a guest
   thread holds an emulated core for 140-400 ms (`scheduler: ... took core 2 from thread 0x1019 after 400.2 ms`). This is
   the known missing preemption inside long guest loops (docs/32), not this wave's.

### Built

- **`src/os/src/mem.rs`: `Memory::mapped(a, n)`.**
  - It says whether [a, a+n) is guest memory the os layer keeps mapped, from its own table: the heap within its size,
    MapPhysicalMemory runs, and shared views.
  - A heap page not touched yet counts as mapped. Any thread's first touch commits it through the existing vectored
    exception handler.
- **`src/os/src/guest.rs`: `readable` answers from that table first.**
  - It falls back to `VirtualQuery` (`host_readable`) only for other memory (modules, stacks, TLS, host tables).
  - A side correction: an untouched lazy-heap page, or a page held on the device by the lazy write-back (`vk/lazy.rs`,
    whose fault handler serves host reads), used to count as "not readable".
  - This change fixes the cause: no `VirtualQuery` inside the heap on a hot path.
- **`src/graphics/src/vk/pipe.rs`: shader-part cap scaled to the machine.**
  - `parts_cap` defaults to an eighth of the machine's memory, at least the old 6 GB (`parts_cap_for`). That is 16 GB
    on this 128 GB machine; machines with 48 GB or less keep 6 GB.
  - The env var still overrides it.
- **`src/graphics/src/vk/pipe.rs`: prewarm order and budget.**
  - Order: `newest_last` keeps each key once, at its last place in the file, so the prewarm builds the latest runs' keys
    first instead of in `Debug`-text order.
  - Budget: `prewarm_budget` is a sixteenth of the machine's memory, between 2 and 8 GB, instead of a fixed 2 GB.
  - These matter once the key file outgrows the budget. With a warm cache they did not trip here.
- **`src/graphics/src/vk/mem.rs`: diagnostic only.** The existing `write watch: … took N ms` slow-call line now also
  gives the thread's kernel and user CPU over the call, which separates a wait from work. No pool-sync behaviour
  changed.
- **`scripts/scenarios.toml` `[smash]`:** the route continues through the results into a second match (`seconds = 560`,
  `window = [130, 280]` = the first match). It compares `private_mb` between the matches.
- **Docs:** `docs/env_switches.tsv` (meanings of `NXGFX_PARTS_PRIVATE_MB` and `NXGFX_PREWARM_BUDGET_MB`),
  `docs/reference/shaders.md`, and the prewarm line of `docs/INTERFACES.md`.

### Tests

- **`os` (`cargo test --release -p os --lib`): 166 passed, 1 ignored.**
  - New test: `mem::tests::mapped_memory_comes_from_the_table`.
  - It checks that an untouched heap page is mapped and that `guest::readable` says so (before the fix `readable` was
    false there: `VirtualQuery` saw MEM_RESERVE).
  - It checks that the host's read commits that page, that the heap's end and an overflowing range are not mapped, and
    that physical maps count while mapped and stop counting after unmap.
- **`graphics` (`cargo test --release -p graphics --lib`): 202 passed, 5 ignored.**
  - New test: `prewarm_builds_the_latest_keys_first` (dedup keeps the last place; the newest is popped first).
  - New test: `prewarm_budget_scales_with_the_machine` (2/4/8 GB at 16/64/128 GB and up; the parts cap is 6 GB up to
    48 GB and 16 GB at 128).
  - Both are new functions, so neither test could fail before.

### Verified

- **Build steps:** `build.ps1 -KeepCache -SkipGnu` under `build/locks/dist.lock`, then `nxrs.exe --select-title "Super
  Smash Bros Ultimate" --build` and `--select-title "Mario Kart World" --build`. The dist and both titles are on the
  final tree, built 11:11.
- **Runs:** `py scripts/play.py smash --env NXGFX_GPU=1` and `py scripts/play.py mkw --env NXGFX_GPU=0`.
- **Smash final run `build/play/20261125-111130-smash-polish-final`** (two matches, 25/25 steps, normal exit, 0 red
  lines). Figures from `frames-*.csv` / `metrics-*.csv` against the pre-wave run `-035528-smash-prewarm`:

  | | before | after |
  |---|---|---|
  | intro movie [8, 40] s | 12.8 FPS, 157 frames > 50 ms | 59.9 FPS, 1 frame > 50 ms (70 ms, no compile) |
  | write-watch scan per frame in the movie | 78-89 ms | 1.4-2.0 ms; no `write watch` slow call in the run |
  | match 1 [130, 193] s | 58.2 FPS, 18 > 50 ms (16 compiles), worst 271 ms | 60.0 FPS, 1 > 50 ms (53 ms, no compile) |
  | match 1 whole [130, 280] s | (route ended at 193 s) | 59.9 FPS, 5% low 59.2; 3 > 50 ms (one 317 ms frame: 11 pipelines of a new effect at 206.6 s) |
  | match 2 [340, 470] s | (no second match) | 59.9 FPS; 4 > 50 ms (compiles of the new opponent, worst 132 ms) |
  | audio underruns | 17 (14 of them in the movie) | 2 by 303 s (107 s match load), 5 by 470 s (310 s second load); 0 in the movie and matches |

- **The sheet** (`sheet-1.png`) shows:
  - boot, then the character select;
  - Mario vs Fox on Norfair, timer 2:05 → 1:36, with damage percentages moving;
  - the results (Fox wins);
  - a second match, Mario vs Link, timer 2:14 → 1:16 → 0:18.
- **Memory (`private_mb`):**
  - Match 1: 6,569 → 6,692 MB over 150 s. Match 2: 6,991 → 7,032 MB over 130 s. Flat inside each match.
  - Between the matches: +300 MB. Of that, +256 MB is one new block in the Vulkan allocator, taken at the results
    screen (896 → 1,152 MB in blocks while the allocated amount went back to 725-769 MB). The rest is the new
    opponent's pipelines and parts.
  - **Third-match check, `build/play/20261125-112615-smash-3match`** (the scenario briefly had three matches; 33/33
    steps, normal exit, 0 red lines). Sheet: Mario vs Samus, then Pikachu, then Yoshi, all on Norfair.
    - Matches [130, 280], [340, 480] and [550, 690]: 59.7 / 59.9 / 60.0 FPS. Frames over 50 ms: 7 / 5 / 3, of them
      compiles 5 / 4 / 2; the worst frames 267 / 144 / 55 ms.
    - `private_mb` per match: 6,585 → 6,770, 6,775 → 6,822, 6,895 → 6,911 (means 6,688 / 6,796 / 6,911).
    - The Vulkan allocator stayed at 896 MB in blocks throughout. The 256 MB block of the two-match run is a
      high-water mark that does not come back every match.
    - About +110 MB a match remains, from the CPU fighter changing every match (new pipelines, shader parts and game
      data). Inside each match memory is flat (+16 to +47 MB over 140 s).
    - Not proven: whether the +110 MB is all new content. A same-opponent route would settle it, but the replay's
      character select leaves the CPU fighter random.
- **MKW re-check `build/play/20261125-112011-mkw-smashpolish`** (43/43 steps, normal exit, only the known
  pipeline-crashed red line; sheet shows the race):
  - Race [250, 325] s: 29.1 FPS (5% low 26.0) against 28.9 (25.1) in `-101138-mkw-perf5-final`.
  - Frames over 50 ms in the race: 42 against 67 (0 compiles against 1).
  - Pool-sync write-watch scan in the race: 6.4-10.5 ms a frame, same as before (6.4-9.9).
  - Parts made ahead now run until 160 s instead of stopping at 9 s. Peak private memory is 17.4 GB against 17.0.
  - The pre-race stretch [140, 250] s was 27.5 FPS against 28.8, with 154 frames over 50 ms against 88. The worst ones
    are course-load texture uploads (548 ms), not compiles.
  - **The parts cap is not the cause.** The same build with the old cap (`NXGFX_PARTS_PRIVATE_MB=6144`,
    `-113755-mkw-parts6g`; parts stopped at 7 s): pre-race 26.6 FPS with 187 frames over 50 ms (24 of them compiles),
    race 29.4 FPS (42/49 frames over 50 ms). The new default is no worse, so the pre-race difference is run-to-run
    variance.

### Unproven

- **Hitches that are left:**
  - A program whose parts cannot be made ahead because it is set in the same frame it draws (a new effect: 11
    pipelines, 317 ms at 206.6 s).
  - A new opponent's first match (4 frames of 64-132 ms).
  - The next run's prewarm covers both. A first sight can only be helped by a seeded or asynchronous path, and draws
    are never skipped.
- **The prewarm budget is still measured with `engine_private_bytes`.** It walks the heap and alias regions with
  `VirtualQuery`, under the same lock as above, once per pipeline built. It runs during boot (0.6 s here, 5.8 s
  cold), so it was not the movie's stall, but it can delay the replay's write-watch scans while it runs. A cheap
  guest-commit count (the os layer counting first-touch commits) would remove it. Not done here.
- **The other `VirtualQuery` paths** (`vk/mem.rs` `host_writable`, `alloc_of`) run on pool creation and texture
  write-back, not per frame in Smash. They can hit the same lock cost on a big heap.
- **The 2 loading-screen underruns** come from the scheduler's missing preemption inside long guest loops (docs/32).

### Slips

- A first background `play.py` launch used `py -I`, which fails to import the harness. It exited at once and was
  re-run. It is not an extra game run.
- The tests ran with `--release` (the warm target), not the debug profile.
- `scripts/scenarios.toml` was temporarily set to three matches for one memory check (`-112615-smash-3match`), then
  put back to two.

### Questions

- None. Both caps now scale with the machine. If the lead prefers keeping the parts cap fixed, `NXGFX_PARTS_PRIVATE_MB`
  still sets it. **The lead should audit `guest::readable` / `Memory::mapped`:** it is a memory-lifetime check on host
  reads of guest pointers.
