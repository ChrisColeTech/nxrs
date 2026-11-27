# MKW-ATLAS — why Mario Kart World never fills its material atlas after boot

Owner: one delegated agent. Lead: main session. Opened 2026-11-27 ~12:55, worktree
`D:\Projects\RecompLauncherEngine-mkwatlas`, branch `mkwatlas` (base `gameplay-harness` `a60f9fd`: MKW-SHRUB merged).
Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Mario Kart World only, GPU 0. Deadline 4 h (~16:55).

**Outcome in one line:** **the shrubs are still near-black.** The brief's premise does not hold: the table entry
"not in the atlas" (scale 0, min LOD 11, layer 0) is the game's normal fallback, not a missing write: the table is a
copy of a 16x16 cell grid that the game fills for the cells its streamer asks for, and nothing asks for more after
boot. Two real engine copy bugs were found on the shrubs' texture path (an ASTC-as-RGBA8 to BC7 copy blitted into a
BC7 image, and block-compressed copies whose extents break Vulkan's block rule). Both were fixed, the copies
verifiably landed, and the shrubs did not change, so both fixes were taken out (protocol §4; they are in the
history, `f3a44c8` and `61f4d72`). What remains on the branch: diagnostics (a usable write watch, a copy trace).

## Plan (written before code)

1. **Build and baseline**: build this worktree's `dist`, relink MKW from it, run `mkw-pop` with a raw pass dump.
2. **The writer**: arm the existing write watch (control `wtrace`, `src/os/src/wwatch.rs`) on the table's pages
   from boot and log each write's guest function; `NXRS_FNPROF` / the symbol map for the call path.
3. **What stops it**: the OS / NVN results on the writer's thread around the course load; compare with the boot path.
4. **Fix the engine side at source**, with a test of the rule.
5. **Verify**: `mkw-pop` and `mkw`; shrub shots before/after; atlas entries; route checks; FPS; stand-ins; tests.

(MKW-SHRUB's evidence folder `build/play/20261127-112648-sh-base-pop/shrub/` went with its worktree; the table
address was found again: constant bank 9 at guest `0x226033a00`, table at `0x226039f30`, the same in every run.)

## 1. The writer chain (diagnosis)

Every step is a run with the write watch on the named range (runs `at-watch2`..`at-watch11b`, section 5). Guest
addresses are the same in every run.

| level | memory | written by | when |
|---|---|---|---|
| constant bank 9 (the table the shader reads, `c9+25904`) | `0x226039f30` | SDK `memcpy` (sdk+0x78b040) from main `f_70000b2ea0` (+0x361c): `memcpy(obj[0x18]+buf[0x58], obj[0xa0], obj[0xa8])`, the per-frame upload of a parameter block (0xa440 bytes) | every frame; changes only at 11.5 s (init: min LOD 11.0 everywhere) and 15 s |
| the parameter block's CPU copy | `0x2a97a2538` (table at `+0x63a0` = `0x2a97a88d8`) | `memcpy` from `f_70005f4320` (+0x5870), size 0x4000 (256 entries) from the next level; entries 256-273 are other per-frame data | every frame; entries 0-255 never change after 12 s |
| the 16x16 grid | `0x2b7a672d8` = `(**0x70036ce208)->[0xb0]` | `f_700069d320`: rebuilds the grid every frame from the manager's cell array (cell: level/layer int +0x1704, rect +0x1708.., flags +0x1720) | changes only at 13-15 s |
| the cell array (16x16, 0x158 bytes a cell) | `(**0x70036ce208)->[0x68]` (+0x184) | `f_700069d430` (cell state machine, called per frame by `f_700069d290`, the module's update) on job workers; `f_7000b4be70` (the cell listener) on the main thread | 13-14 s only |
| the cell requests | `(**0x70036ce208)->[0xe0]` +0x20/+0x28 | `f_7000b4be70` (handler, vtable +0x28) pushes a request when an event (col, row, type 9 → state 1, type 10 → state 2) changes a cell; `f_700069d430` turns each into a `"%s.bntx"` texture load (`f_70005cff90`) | 14.2 s (4 + 12 requests); none after |
| the events | — | `f_700036c9e0`, a world-streaming dispatcher (lists of 5-byte records: type, col, row) called from `f_700036ec10` ← `f_70004cc100` (graphics system update, `**0x70036cd6b8`) | — |

The cell grid at 120 s and 192 s (layer / flags): only rows 8-11, columns 2-5 are assigned (layers 0-15; the inner
2x2 flags 0x3f, the ring 0x07); every other cell `-1/00`. That is 16 cells for the 16 atlas layers the game fills,
a camera-centred clip of a 16x16 grid (two detail rings). The shrub material's entry 112 (row 7, column 0) is outside
it. The sampled gates (`at-sample10`, every 2 s to 225 s): the update gate `(**0x70036cd6b8)->[0x46c] & ~2 == 1`
holds the whole run (1 or 3); the request count stays 0 after 16 s; no failed file open (`NXOS_FS_TRACE=miss`).

So nothing in the copy chain fails: the streamer produces no events for other cells after boot. Its inputs (the four
view slots of `f_700036ec10`'s object at +0xe4/+0x1b4/+0x284/+0x354, a parameter found by hash 0xf8f5c5c7) were not
reached in time (the control pipe stopped answering when its object's page `0x70036cc000` was peeked: section 6).

## 2. The shrubs' texture path (diagnosis), and the two copy bugs

mkw-recomp (`D:\Projects\mkw-recomp-main`, MIT; `runtime/gfx/vk/tex/vk_tex_upload.c` m16, `vk_sparse.c`) documents
the same black bushes and cacti: they "take their colour from level 11 of the terrain arrays", which MKW fills by
copying small textures into levels 5-11. So the "not in the atlas" entry (sample at (0,0), LOD 11, layer 0) is the
game's design, and the question becomes what those levels hold. `NXGFX_COPY_TRACE=1` (run `at-trace`, 2,301 copies
into sparse textures in 120 s):

- the first set (`ser33398/33401/33404`): 627 copies each of the 8192x8192 default textures (`ser200-202`);
- the sampled set (`ser33391` BC7, `ser33393` BC6H, `ser33395` BC7): 8 copies per material (its levels 0-7 into
  levels 3-10), 16 materials (layers 0-15), at 12.6-12.8 s, none later. **Nothing writes level 11** of the sampled set.

Two engine bugs on that path (both real; neither changed the shrubs):

1. **ASTC kept as RGBA8 into a BC7 image** (`exec.rs` `COPY_T2T`): ASTC textures under 256x256 are decoded to RGBA8,
   larger ones re-encoded to BC7; a copy between them differs in format and fell back to `vkCmdBlitImage`, which
   cannot write a BC7 image. Layer 0 of `ser33395` (the fallback layer) is filled this way: `ser37911`
   R8G8B8A8_SRGB 180x511, levels 0-7 into levels 3-10. Fix (`f3a44c8`): the source's own ASTC blocks of the region,
   decoded and encoded to BC7 (`astc::encode_bc7`) in the upload command buffer, copied in the replay's command
   buffer at the copy's place (`image::copy_astc_into_bc7`), with tests of the region and extent rules. The 8 copies
   logged as encoded (`at-fix1-pop`).
2. **Block-compressed copies of regions smaller than their blocks**: the game's 1x3, 2x7, 7x2, 3x3 levels copied
   into levels 9-10 (8x8, 16x16) of the BC6H / BC7 arrays: Vulkan wants each side a multiple of 4 or reaching the
   level's edge in both images, which no direct copy meets there. Fix (`61f4d72`): through a scratch buffer, read at
   the source's extent and written as whole blocks (`block_copy_plan`, tested); `at-fix3-pop` logged them (copy 1:
   1020x358 → 1020x360 ... copy 64).

Neither changed a shrub: the title shots' shrub box measures luma 34.7 (base) / 34.9 (fix 3) and the race shots stay
black silhouettes (`at-fix3-pop/27-pop.png`). Both were taken out (`664842c`). A third change (one tail memory for
images over one storage) did nothing (no two sampled images share a storage: the "storage 0x5ad80000" in the sparse
log line is the size) and was taken out (`7cb9b63`).

**The open lead** (`at-trace2`): the page past each layer's tiles (storage page `layers x stride + layer`, where the
levels outside the tiles would be) is **unmapped** in all three sampled arrays when the game copies into levels
5-10; each layer's level-4 tile page is mapped (a different physical page per layer). The engine gives those levels
private memory (mkw-recomp does the same). Not established: where the console keeps levels 5-11 of these arrays and
what the shrubs' LOD-11 sample returns there (a clamp to level 10 by the view or sampler, or a tail aliased to the
first set's, whose level 11 holds the default texture). That needs the shader's sampled values (MKW-SHRUB's
instrumented-shader method) with the copies fixed.

## 3. Built (kept on the branch)

- `src/os/src/wwatch.rs` (diagnostic, control `wtrace` / `wpage`): pages made read-only instead of guard pages
  (a guard page faults on reads, and a page read every frame let other threads' writes through unseen: run
  `at-watch6` missed every write of the grid's values); only writes inside the asked bytes are logged; each line
  carries the writer's host thread, guest x0-x2, lr and frame chain, and the 16 bytes after the write;
  `NXOS_WWATCH_CHANGED=1` logs only writes that change the bytes, `NXOS_WWATCH_LINES` the limit. Test extended
  (a write outside the asked bytes is not counted). Known hole: something resets a watched heap page to read-write
  during boot (re-arming every 0.25 s from the control pipe caught the writes: `at-watch8`).
- `src/graphics/src/vk/exec.rs`, `sparse.rs`: `NXGFX_COPY_TRACE=1` logs every texture copy into a sparse texture
  (source serial / format / size / levels, both regions, and the physical pages behind the destination layer's last
  tile and tail page, `sparse::layer_pages`).
- `docs/env_switches.tsv`: `NXOS_WWATCH_CHANGED`, `NXOS_WWATCH_LINES`, `NXGFX_COPY_TRACE`.
- No engine behaviour change remains.

## 4. Pictures

| what | base (`at-base-pop`, base build) | after the copy fixes (`at-fix3-pop`) |
|---|---|---|
| title demo, shrubs left of the road | `build/play/20261127-141436-at-base-pop/18-title.png` | `build/play/20261127-142643-at-fix3-pop/18-title.png` |
| race, near shrub | `20261127-141436-at-base-pop/27-pop.png` (black silhouette) | `20261127-142643-at-fix3-pop/27-pop.png` (black silhouette) |
| race sheets | `20261127-141436-at-base-pop/sheet-2..9.png` | `20261127-142643-at-fix3-pop/sheet-2..9.png` |

**What still looks wrong** (literal): every roadside shrub, at the title demo and in the race, is a near-black
silhouette with only faint dark-green leaf shading; no run of this wave changed that.

## 5. Runs (GPU 0; other agents' builds and games ran on the machine throughout)

| run (`build/play/`) | build | purpose | result |
|---|---|---|---|
| `20261127-130146-at-watch1` | watch v1 | table watch | driver armed before the command existed; quit |
| `-130216-at-watch2` | watch v1 | table watch, threads, peeks | 5000-line budget spent on a 5.9 s clear; run stopped from outside at 147 s |
| `-130551-at-watch3` | changed-only | table watch to 300 s | bank table written at 11.5 s and 15 s only (memcpy from `f_70000b2ea0`) |
| `-131207-at-watch4` | + x0-x2 | source of the copy | parameter block `0x2a97a2538`; entries 0-255 unchanged after 12 s; quit early |
| `-131259-at-watch5` | same | source grid | copied from `0x2b7a672d8` (manager `+0xb0`); no change 22-226 s |
| `-131745-at-watch6`, `-132239-at-watch7` | guard / read-only | grid from boot | the values' writes unseen (watch hole) |
| `-132556-at-watch8` | read-only + re-arm loop | grid from boot | writer `f_700069d320`, main thread, 13-15 s |
| `-132745-at-watch9` | same | cell array | writers `f_700069d430` / `f_7000b4be70`, 13-14 s; grid snapshot at 192 s |
| `-133152-at-sample10` | same | gates every 2 s | gate holds; no requests after 16 s |
| `-133428-at-watch11` | same | (overlapped run 10: slip) | driver attached to run 10's process; nothing logged |
| `-133554-at-watch11b` | same | request list | producer `f_7000b4be70` at 14.2 s via `f_700036c9e0` |
| `-134310-at-x-shadow0` | same | `NXRS_PATCH_MKW_SHADOW=0` | grid unchanged (not the shadow patch) |
| `-134648-at-sample13`, `-135155-at-sample14` | same | streamer object | control pipe hung on the first peek of page `0x70036cc000` |
| `-140054-at-fix1-pop` | + ASTC→BC7 copy | `mkw-pop --faults` | 8 copies encoded; shrubs black; 52.2 / 42.0 FPS; 2/2 on route; normal exit |
| `-140804-at-fix2-pop` | + tail sharing | `mkw-pop --faults` | no sharing happened; shrubs black; 39.3 / 23.5; 2/2 on route |
| `-141436-at-base-pop` | **base** (`a60f9fd` graphics) | `mkw-pop --faults` (baseline, taken late) | shrubs black; **46.5 / 24.9**; 2/2 on route; 1 stand-in (the known boot one); normal exit |
| `-142058-at-trace` | + copy trace | 120 s | 2,301 copies into sparse textures (section 2) |
| `-142643-at-fix3-pop` | + block copies | `mkw-pop --faults` | copies made as blocks; shrubs black; 47.9 / 29.0; 2/2 on route; normal exit |
| `-143337-at-trace2` | + layer pages | 50 s | tail pages unmapped |
| `-143834-at-final-mkw` | final (`664842c`) | `mkw` | **58.5 / 53.5** [250, 325]; **3/3 on route**; 1 stand-in (the known boot one); normal exit |

FPS (window [245, 290] s of `mkw-pop`, mean / 5 % low) moved between 39 and 52 with the machine's other load; no
change of this wave touches the frame path when its switch is off.
