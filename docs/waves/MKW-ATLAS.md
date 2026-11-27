# MKW-ATLAS — why Mario Kart World never fills its material atlas after boot

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~12:55, worktree
`D:\Projects\RecompLauncherEngine-mkwatlas`, branch `mkwatlas` (base `gameplay-harness` `a60f9fd`: MKW-SHRUB merged).
Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0. Deadline 4 h (~16:55).

Starting point (MKW-SHRUB §2, §6): every roadside shrub pixel reads its material record → entry 112 of the game's
atlas table (constant bank 9, `c9 + 25904`, 64 bytes an entry), which says "not in the atlas" (scale 0, min LOD 11,
layer 0). Only 21 of 274 entries are filled; the game copies into the atlas arrays only at boot (12-17 s), never in
the course load or the race. The guest memory holds the same values, so the game itself never wrote the course's
entries. MKW-SHRUB's evidence folder (`build/play/20261127-112648-sh-base-pop/shrub/`) went with its worktree, so the
dumps are made again here.

## Plan (written before code)

1. **Build and baseline**: build this worktree's `dist`, relink MKW from it, run `mkw-pop` with a raw pass dump in
   the race (`NXGFX_DUMP_AT_S`, `NXGFX_DUMP_RAW=1`) to get the constant bank 9 address and the table again.
2. **The writer**: arm the existing write watch (control `wtrace <lo> <hi>`, `src/os/src/wwatch.rs`) on the table's
   pages from boot (before 12 s) and log each write's guest function; `NXRS_FNPROF` / the symbol map for the call
   path. If the table is written by the engine on the guest's behalf (a GPU-side copy or command-buffer update
   landed into guest memory), the watch names the host function instead.
3. **What stops it**: on the writer's thread, log the OS / NVN results around the course load (a failed or
   never-answered call: file read, event, fence/query, sparse commit, readback); compare with the boot path.
4. **Fix the engine side at source**, with a test of the rule.
5. **Verify**: `mkw-pop` and `mkw` after; shrub shots before/after; atlas entries filled (constant bank dump);
   route checks; FPS against the baseline; stand-ins; `cargo test --release` for the touched crates.

Expected files: `src/os/src/wwatch.rs` (if the watch needs to arm early / by switch), the service or NVN file that
holds the failing call, `docs/env_switches.tsv` for any new switch.
