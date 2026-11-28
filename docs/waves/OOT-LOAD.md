# OOT-LOAD — Ocarina of Time: the device loss when the house autosave loads; `oot-saved` as the quick scenario

Owner: one delegated agent. Lead: main session. Opened 2026-11-28, worktree `D:\Projects\RecompLauncherEngine-ootload`,
branch `ootload` (base `gameplay-harness` `eb21946`). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: "The Legend of
Zelda Ocarina of Time" only, GPU 0 (`--env NXGFX_GPU=0` on every run). Deadline 4 h.

## Evidence

`D:\Projects\RecompLauncherEngine\build\play\20261128-083853-oot-saved-probe`: Continue at ~29 s, three A presses
(Continue -> Load list -> Load) start the load of the "Kokiri Forest" autosave; the screen goes black at ~42 s; one
replay frame (2003) takes 3.75 s (4,867 draws, ~1,000 images made and uploaded, 8 descriptor-buffer chunks of 4 MB
made between 47.9 and 50.6 s); the device is lost at 53.5 s (exit 71). The GPU had started a compute dispatch of 80x60x1
groups (compute shader `f8965bffbdf51d78`) and finished everything before it. The driver's fault report: an invalid
read at page `0x539651000`, 0x1000 above the end of the newest 4 MB chunk (`0x539250000 + 0x400000`); no recorded
buffer was ever made or freed over that page, and no kept stage set has a binding over it.

## Plan (written first)

1. **Diagnose with diagnostics added, never the same path unchanged** (protocol §4, device loss):
   - the loss report also names the buffers made *above* the faulting page (today it lists only those below), so a
     read that runs below the start of a buffer (a negative index) is seen as well as one past an end;
   - a ring of descriptor-buffer bindings (the stage-set chunk and the pool set's buffer each `vkCmdBindDescriptorBuffersEXT`
     binds, with their sizes) and the dispatch's own compute set (today only mesh draws are recorded), so the report
     says which buffers the faulting dispatch read its descriptors from and where its set lies in them;
   - a CPU-side check where each set's offset is handed to the GPU: the offset plus the set's size lies inside the
     bound chunk, and the bound chunk is the one the set was written in; a failure names the stage, the shader and the
     offsets (red line), so an out-of-range descriptor offset is caught before the GPU reads it.
   Then one `oot-saved` run on GPU 0 with these on.
2. **Fix the cause in source** (indexing or allocation; no raised limit, no skipped draws), with a unit test that fails
   without the fix.
3. **`oot-saved`**: find the presses from the title to Link under control in his house; shots dense enough to see the
   load; a standing window (Link still, as `oot-still`'s h2-h7) and a walk with `replay oot_house_walk.txt`; route
   checks; references with `py scripts/play.py checkpoint <run>` from a run whose sheets were read. Target ~3-4 min.
4. **Verify:** two `oot-saved` runs load and play with no device loss; one full `oot` run (window [845, 915]) against
   `20261128-044556-r2-base` (~44.9/30 walk) for regressions; every sheet read; `cargo test --release -p graphics`;
   `py -m unittest scripts.test_play`.

Exclusions: Fault A (the white house floor); live native NDVE stays off.

## Diagnosis (runs in this worktree's `build/play/`)

- `20261128-090152-diag1` (the dispatch's descriptor buffers recorded, buffers above the fault listed, the CPU offset
  check on): the same loss at 53.0 s, a read at `0x520c52000`, again just past the newest 4 MB chunk (made at
  50.064 s). But the faulting NDSR dispatch (`f8965bffbdf51d78`, recorded at 50.038 s) read its stage set from a
  different chunk (`0x54ea20000`, set at +0x238c0, in range), bound no pool set, and the CPU check never fired over
  the whole run. **The descriptor-index hypothesis is disproved**: no descriptor read ran past its buffer; the newest
  chunk was only allocated over memory freed just before.
- `20261128-090743-diag2` (`NXGFX_ADDR_REPORT=1`, `NXGFX_LOG_SHADER=f8965bffbdf51d78`, images actually freed recorded
  with the timeline): the fault lies **inside IMAGE ser18252 (2560x1440), bound at 47.89 s, UNBOUND at 50.38 s**. Its
  memory was freed at 50.38 s with timeline completed = submitted = 14468, while the dispatch reading over that
  storage was recorded at 50.383 s into the submission given timeline 14469, **submitted at 50.408 s**: the memory was
  freed before the submission that read it was even submitted. ser18252 is one of the images over storage
  `0x13fa17400` made again with storage use (`upgrade_storage`, a texture used as an image without the IMAGE flag)
  during the load; their old backing goes through `defer_image_free` -> `Queue::defer_after_use`.
- **Cause.** `defer_after_use` held a release for "the submission being recorded now" with one global flag
  (`recording`, set in `make_sub`, cleared in `submit_sub`). `chain_parked` (parked streams run on after a pool sync)
  begins the next submission (`chained_sub`) *before* it submits the previous one. That submit took the next
  submission's releases with its own fence and cleared the flag, so every release asked while the next submission
  recorded went to `defer(last_fence)` and ran as soon as the previous submission completed: the old images' memory was
  freed under the GPU work still to read it. The load is where many parked streams chain while images are made again;
  the normal `oot` route's chained submissions had no such release to make.

## Fix

`queue.rs` `AfterUse`: each release asked while submissions are recorded is tagged with the newest submission begun
(its serial); a submit takes only the releases tagged up to its own serial and its children's (begun after it,
submitted with it); the rest wait for the later submission's fence. "Recording" is "a submission begun is not yet
submitted", so the previous one's submit no longer ends the next one's recording. Unit tests
`after_use_tests::a_release_asked_while_the_next_submission_records_waits_for_it` (the chained order: with one flag
the previous submit took the later release and the next ones ran at once) and `children_are_covered_by_their_parent_submit`.

Kept from the diagnosis (cheap, always on): the loss ring's dispatch record (its stage-set chunk, set offset, pool set
buffer, and where the fault lies against each), buffers starting above the faulting page, the images actually
destroyed with the timeline then, and a CPU check where each set's offset is handed to the GPU (the bound chunk is the
one the set was written in and holds it: a red line naming the stage and shader otherwise).

## Verified (GPU 0; runs in this worktree's `build/play/`; other waves were building and running on the machine)

| run | scenario | result | FPS (report.md) |
|---|---|---|---|
| `20261128-091353-fix1` | old oot-saved steps | loaded, Link in his house by 65 s, no loss | [110, 150] 58.3 / 5 % low 53.8 |
| `20261128-091717-saved-r1` | oot-saved | loaded, stood, walked, no loss; references taken | [80, 145] 56.6 / 51.3 |
| `20261128-092149-saved-walk-r2` | oot-saved-walk | as r1; references taken | [165, 233] 43.5 / 29.5 |
| `20261128-092605-oot-regr` | oot (full) | completed, 6/6 checks on route, no loss | [845, 915] 43.1 / 29.6 (base `20261128-044556-r2-base` 44.9 / 30) |
| `20261128-094134-saved-r3` | oot-saved | 5/5 checks on route, no loss | [80, 155] 56.6 / 50.0 |

The full run's standing stretch [660, 820] from its frames csv (the same calculation for both): 46.7 FPS / 5 % low 41.5
against the baseline's 48.1 / 43.4 (indicative: the machine was shared). Every sheet read: title, Load list, Load /
Cancel, ~20 s of black load, Link under control in his house with Navi; the walks move through the house. Red lines
in the saved runs: the known "draw with no render target" and one "used all its occlusion queries" at ~57 s.
`cargo test --release -p graphics`: 329 passed, 6 ignored; `py -m unittest scripts.test_play`: 12 OK.

## Files expected

- `src/graphics/src/vk/lost_ring.rs` (bind ring, report above the fault), `src/graphics/src/vk/exec.rs` (bind
  records, the offset check, the fix), possibly `src/graphics/src/vk/queue.rs` / `tables.rs` (descriptor-buffer
  allocation) and `lost.rs`.
- `scripts/scenarios.toml` (`oot-saved`), `scripts/checkpoints.json` (its references), possibly `scripts/play.py` /
  `scripts/test_play.py`.
- This document (results and report).
