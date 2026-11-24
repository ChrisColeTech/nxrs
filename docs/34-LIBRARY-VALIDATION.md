# 34 - Production library validation

**Campaign status: COMPLETE (2026-11-22).** The owner-authorized target is the real 15-entry library at
`C:\Users\Risky Biz\AppData\Local\nxrs\library.json`, built sequentially through the production launcher into each
configured external `buildDir`, then driven to controllable gameplay by the guarded harness. Evidence root:
`C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-library-validation`.

Production release identity:

| Component | Path | SHA-256 | UTC mtime | Bytes |
|---|---|---|---|---:|
| Launcher | `dist/nxrs/nxrs.exe` | `ad25fe0afd458824237b26fd5d751afef660139b290ca94236f9f453b4b2ccfb` | Agent 2 final lean candidate | - |
| Engine | `dist/nxrs/nxrs-engine/nxrs-build.exe` | `e84634fcafca68f90b9999ebe21fce15239684c5dafad3d060c871fda677bf5f` | Agent 2 final lean candidate | - |
| Source baseline | `D:\Projects\RecompLauncherEngine` | HEAD `143515097a0fa134c6fc4be839971692a39ac4de` with pre-existing dirty worktree | - | - |

## Actual inventory

`Selected update` is the library's `updateChoice`; an empty value means no selected update.
Final source reachability: entries 1-6 and 8-15 were reachable; entry 7's configured `dump` folder was missing.
The table's final `Source` result is therefore reachable for those 14 entries and missing for entry 7; its `turbo`
column records the initial engine before the campaign (all entries are now `nxrs`).

| # | Title | ID | Source | Build directory / output name | Selected update | Initial engine | Initial preflight |
|---:|---|---|---|---|---|---|---|
| 1 | Donkey Kong Bananza | `8fc87c19-c097-42db-ad91-1619315313af` | `P:/Games/Nintendo/DUMPS/Donkey Kong Bananza [040074A01BF12000]/Donkey Kong Bananza [040074A01BF12000][v458752].nup` | `P:/Games/Nintendo/DUMPS/Donkey Kong Bananza [040074A01BF12000]/SwitchRecomp Builds/Donkey Kong Bananza` / `Donkey Kong Bananza` | none | turbo | pending |
| 2 | Mario Kart World | `a5c15383-8644-4c18-beab-40c92165082d` | `P:/Games/Nintendo/DUMPS/Mario Kart World [0400C3F00006E000][v0]/Mario Kart World [0400C3F00006E000][v0].nup` | `P:/Games/Nintendo/DUMPS/Mario Kart World [0400C3F00006E000][v0]/SwitchRecomp Builds/Mario Kart World` / `Mario Kart World` | none | turbo | pending |
| 3 | Pokemon Legends Z-A | `ad5e650c-1503-4d44-8e44-bb6de2d074cd` | `P:/Games/Nintendo/DUMPS/Pokemon Legends Z-A Switch 2 Edition [0100F43008C44000]/Pokemon Legends Z-A Switch 2 Edition [0100F43008C44000].nup` | `P:/Games/Nintendo/DUMPS/Pokemon Legends Z-A Switch 2 Edition [0100F43008C44000]/SwitchRecomp Builds/Pokemon Legends Z-A Switch 2 Edition` / `Pokemon Legends Z-A Switch 2 Edition` | none | turbo | pending |
| 4 | Super Mario Bros Wonders | `5afdfd1c-3c79-4576-ab73-761b7b1852b8` | `P:/Games/Nintendo/DUMPS/Super Mario Bros Wonders Switch 2 Edition [010015100B514000]/Super Mario Bros Wonders Switch 2 Edition [010015100B514000].nup` | `P:/Games/Nintendo/DUMPS/Super Mario Bros Wonders Switch 2 Edition [010015100B514000]/SwitchRecomp Builds/Super Mario Bros Wonders Switch 2 Edition` / `Super Mario Bros Wonders Switch 2 Edition` | none | turbo | pending |
| 5 | The Legend of Zelda Ocarina of Time | `a0767514-6902-4bad-a938-e19b8c52bd6d` | `P:/Games/Nintendo/DUMPS/ZORCT ROM/The Legend of Zelda Ocarina of Time [0400486024DEC000][v131072].nup` | `P:/Games/Nintendo/DUMPS/ZORCT ROM/SwitchRecomp Builds/The Legend of Zelda Ocarina of Time` / `The Legend of Zelda Ocarina of Time` | none | turbo | pending |
| 6 | Mario Kart World v1.8.0 | `acc057e7-d9d3-4f55-9dc3-7a3891dfae60` | `P:/Games/Nintendo/DUMPS/Mario Kart World [0400C3F00006E000][v0]/Mario Kart World [0400C3F00006E000][v0].nup` | `P:/Games/Nintendo/DUMPS/Mario Kart World [0400C3F00006E000][v0]/SwitchRecomp Builds/Mario Kart World 180` / `Mario Kart World 180` | none | turbo | pending |
| 7 | Mario Kart 8 Deluxe | `98b95642-b9b2-4c07-990d-07b4dbc3c0b7` | `P:/Games/Nintendo/DUMPS/Mario Kart 8 Deluxe/dump` | `P:/Games/Nintendo/DUMPS/Mario Kart 8 Deluxe/SwitchRecomp Builds/Mario Kart 8 Deluxe 1G1U1D` / `Mario Kart 8 Deluxe 1G1U1D` | none | turbo | pending |
| 8 | Pokemon Legends Arceus (1G+1U) | `ee531f7a-3efa-4ad8-9b8a-bcd25385eb87` | `P:/Games/Nintendo/DUMPS/Pokemon Legends Arceus [01001F5010DFA800][v262144]/Pokémon Legends - Arceus [01001F5010DFA000][v262144] (1G+1U).nsp` | `P:/Games/Nintendo/DUMPS/Pokemon Legends Arceus [01001F5010DFA800][v262144]/SwitchRecomp Builds/Pokemon Legends - Arceus 1G1U` / `Pokemon Legends - Arceus 1G1U` | none | turbo | pending |
| 9 | Pokemon Violet | `0b2fd331-ecd6-4a53-8baa-6eaa38c52551` | `P:/Games/Nintendo/DUMPS/Pokemon Violet [01008F6008C5E800]/Pokémon Violet [01008F6008C5E000][v786432] (1G+1U+1D).nsp` | `P:/Games/Nintendo/DUMPS/Pokemon Violet [01008F6008C5E800]/SwitchRecomp Builds/Pokemon Violet 1G1U1D` / `Pokemon Violet 1G1U1D` | none | turbo | pending |
| 10 | Pokemon Legends Arceus base | `b52af08d-5a2d-4d0a-bff6-9b9cfa03f872` | `D:/Projects/Switch2RECOMP-main/build/evidence/updates/packages/Pokemon Legends Arceus base [01001F5010DFA000][v0].nsp` | `D:/Projects/Switch2RECOMP-main/build/evidence/updates/packages/SwitchRecomp Builds/Pokemon Legends Arceus base` / `Pokemon Legends Arceus base` | none (`builtUpdate` previously points at 1.1.1 package) | turbo | pending |
| 11 | Anime Girls Sun of a Beach | `dc537ed7-ed5a-44ed-aa20-eb87af14b573` | `P:/Games/Nintendo/DUMPS/Anime Girls Sun of a Beach [0100C6801E0FA000][v0]/Anime Girls Sun of a Beach [0100C6801E0FA000][v0][US].nsp` | `P:/Games/Nintendo/DUMPS/Anime Girls Sun of a Beach [0100C6801E0FA000][v0]/SwitchRecomp Builds/Anime Girls Sun of a Beach` / `Anime Girls Sun of a Beach` | none | turbo | pending |
| 12 | Beach Bounce Remastered | `a3c6d21d-de68-474d-a80d-2ec4603f80f6` | `P:/Games/Nintendo/DUMPS/Beach Bounce Remastered [01004060145C0000][v0]/Beach Bounce Remastered [01004060145C0000][v0] (nsw2u).nsp` | `P:/Games/Nintendo/DUMPS/Beach Bounce Remastered [01004060145C0000][v0]/SwitchRecomp Builds/Beach Bounce Remastered nsw2u` / `Beach Bounce Remastered nsw2u` | none | turbo | pending |
| 13 | SONIC X SHADOW GENERATIONS | `957e9102-21d2-4f78-81c3-99b2e2936947` | `P:/Games/Nintendo/DUMPS/SONIC X SHADOW GENERATIONS [01005EA01C0FC000]/SONIC X SHADOW GENERATIONS [01005EA01C0FC000][v0][US].nsp` | `P:/Games/Nintendo/DUMPS/SONIC X SHADOW GENERATIONS [01005EA01C0FC000]/SwitchRecomp Builds/SONIC X SHADOW GENERATIONS` / `SONIC X SHADOW GENERATIONS` | none | turbo | pending |
| 14 | Super Mario Party Jamboree | `7debe889-e02b-4452-99ab-7e6ca228c455` | `P:/Games/Nintendo/DUMPS/Super Mario Party Jamboree [0100965017338800]/Super Mario Party Jamboree v1.0.0[0100965017338000].nsp` | `P:/Games/Nintendo/DUMPS/Super Mario Party Jamboree [0100965017338800]/SwitchRecomp Builds/Super Mario Party Jamboree v100` / `Super Mario Party Jamboree v100` | none | turbo | pending |
| 15 | Super Smash Bros Ultimate | `f2b4cca8-e963-48bb-b82c-c5d493241bf5` | `P:/Games/Nintendo/DUMPS/Super Smash Bros Ultimate[01006A800016E000]/Super Smash Bros Ultimate[01006A800016E000][US][v0].nsp` | `P:/Games/Nintendo/DUMPS/Super Smash Bros Ultimate[01006A800016E000]/SwitchRecomp Builds/Super Smash Bros Ultimate` / `Super Smash Bros Ultimate` | none | turbo | pending |

## Checklist template

Each detailed result records `PASS`, `FAIL`, or `BLOCKED` for every item.

Build: source reachable; production CLI build succeeded; library is `engine=nxrs` with current `builtSource` and
`builtUpdate`; external exe, `_code.dll`, `_data`, INI and generation marker exist; production saves unchanged by
build.

Runtime: guarded process lifecycle has no fatal/device loss; title/menu/intro passed; first controllable gameplay
reached; scripted input visibly changes gameplay; main scene is readable; audio active/non-silent and underruns
recorded; native FPS plus frame p50/p95/p99/max and stutters recorded over gameplay; peak and slope memory recorded
with scope; shader/pipeline/diagnostic/service gaps recorded; screenshots visually reviewed; production saves
unchanged while temporary save changes are external; owned process and claim cleaned up.

## Rolling status

| # | Entry | Status | Build | Gameplay | Evidence / note |
|---:|---|---|---|---|---|
| 1 | Donkey Kong Bananza | FAIL | PASS | FAIL | visible controllable tunnel; pacing/audio and one live target mismatch remain |
| 2 | Mario Kart World | FAIL | PASS | FAIL | device loss/reset at direct mesh draw; `MKW-B01` |
| 3 | Pokemon Legends Z-A | FAIL | PASS | FAIL | free movement reached; live cube mismatches and tutorial pacing remain |
| 4 | Super Mario Bros Wonders | PASS | PASS | PASS | profile pairs reconciled to manifest; clean responsive gameplay |
| 5 | The Legend of Zelda Ocarina of Time | FAIL | PASS | BLOCKED | branch patch fixed/build current; NDVE gameplay gate remains |
| 6 | Mario Kart World v1.8.0 | BLOCKED | PASS | BLOCKED | same base/source workload stopped by `MKW-B01` latch |
| 7 | Mario Kart 8 Deluxe | FAIL | PASS | FAIL | supported CLI selected verified 4.0.1 NSP; readable race, residual resources/pacing |
| 8 | Pokemon Legends Arceus (1G+1U) | PASS | PASS | PASS | branch patch fixed; readable field control at native 30 FPS |
| 9 | Pokemon Violet | FAIL | PASS | BLOCKED | shader packaging fixed; no control; new external display-reset latch |
| 10 | Pokemon Legends Arceus base | PASS | PASS | PASS | independent build and readable field control |
| 11 | Anime Girls Sun of a Beach | PASS | PASS | PASS | exact Vulkan conservative raster mapping; clean responsive gameplay |
| 12 | Beach Bounce Remastered | PASS | PASS | PASS | 16 runtime Unity variants promoted; clean 59.95 FPS interactive story |
| 13 | SONIC X SHADOW GENERATIONS | FAIL | PASS | FAIL | both programs reach gameplay; Shadow diagnostics remain; `SXSG-B01` |
| 14 | Super Mario Party Jamboree | PASS | PASS | PASS | clean controllable plaza at 59.48 FPS |
| 15 | Super Smash Bros Ultimate | FAIL | PASS | FAIL | zero diagnostics; completed match at 58.39 FPS, pacing/memory scope remains |

## Per-entry results

Results are appended here after each entry completes; failures link to the bug register.

### Donkey Kong Bananza

Build command: `nxrs.exe --library <real-library> --select-title "Donkey Kong Bananza" --engine nxrs --build`.
Result: **PASS**, 373.7 s by build-log creation/write interval; `Done` and `Build complete` at build-log lines
99/101. Library now has `engine=nxrs`, `[nxrs]` `buildDir`, current `builtSource`, empty/current
`builtUpdate`, `buildIncomplete=false`, and `lastBuilt=2026-11-22T03:14:58`.

Build checklist: **PASS** source reachable; **PASS** production CLI build; **PASS** current library metadata;
**PASS** exe/DLL/data/INI/gen marker; **PASS** production save unchanged (4 files, tree SHA-256
`aaa54582fb628381b2bac808c873abbc3e52df6a2034417c043503d48c79154d` before/after).

Artifact SHA-256: exe `cecdc4fd2b85952d2118facc78fc6e7f87dfb9bc6b57d351f1aa6acefc5a4832`;
DLL `66f91fe8d7dd826477288b5081e534f01f9edfb391654e479523d103da2217f2`; INI
`743863ad5f190bf4abc752d8ed1b8c7f9436057f83f504fd7debae8c5143bc9c`; marker
`1ed0fcbe28039938236e39accfae1407a0d3d52cc12685fe6f7e6dc49be930c1`.

Runtime checklist (`runs/01-dkb-valid`): **PASS** process exit 0/no device loss; **PASS** title/menu/intro passed;
**PASS** controllable mining-tunnel state reached; **PASS** scripted walk/punch visibly changed HUD counters and dialogue;
**FAIL** scene readability (world entirely black, UI only, visually reviewed across four gameplay PNGs); **PASS** audio
active/non-silent, 0 gameplay-window and 13 lifetime underruns; **FAIL** native 60 FPS (22.239 FPS, p50 33.542,
p95 74.140, p99 191.646, max 484.936 ms, 359 stutters over 100-150 s); **PASS** gameplay-window memory recorded
(7295 start/7318 peak/7315 end MB, 33.22 MB/min; short steady-state scope); **FAIL** diagnostics (23 red lines,
15 unique, chiefly missing memory-pool storage bindings and a texture target mismatch; 0 missing/refused shaders);
**PASS** screenshots visually reviewed; **PASS** production save unchanged; **PASS** claim/process cleanup.

`DKB-B01` (critical visual, title-specific with shared resource paths): controllable gameplay renders only HUD over
black. Evidence: `runs/01-dkb-valid/0002-replay_gameplay.png` through `0005-shot_end.png`, `summary.json:88-215`.
Ranked hunches: (1) `graphics/src/vk/exec.rs` descriptor construction binds private zero scratch for the 12 reported
stage-5 storage buffers, so scene compute/material outputs never reach their real pools; check those shader keys
against pool registration at first red timestamp. (2) `graphics/src/vk/image.rs` target/view selection mishandles the
reported target-1 versus shader-target-2 texture, blanking the scene chain; disprove by tracing that texture ID to
the raw scene attachment and final sampler. (3) `graphics/src/vk/present.rs` / `upscale.rs` terminal NDSR ownership
or blit regressed in the production binary, leaving scene black while front-end UI composes; disprove by capturing
the pre-present source and terminal output owners in the same frame. (4) `shaders/src/xlate.rs` translated a reached
compute/material shader successfully but with wrong resource reflection despite no refusal; disprove by comparing
the 12 keys' SPIR-V bindings to profile facts. Agent 2 first action: correlate the first missing-pool key to its guest
descriptor/pool generation and raw output before changing presentation globally.

Current production rerun `nxrs-agent2/runs/20261122-213544-dkb-visible-final` supersedes the older black result. The
Omap correction and terminal NDSR host blit now produce a fully visible lit tunnel; walking/punching changes the scene
and destruction effect. A bounded pass log identifies current material pass 14 (`96b8d5693bf9cd24` /
`2e3d6148f0b413da`, Omap `0x6f`) with populated downstream HDR. Acceptance remains FAIL on 57.61 FPS, 11 gameplay
audio underruns and one live 2D->3D target mismatch, not black rendering.

### Mario Kart World

Build: **PASS**, production CLI command with `--select-title "Mario Kart World" --engine nxrs --build`, exit 0 in
228.0 s. Library metadata is current (`engine=nxrs`, `[nxrs]`, `buildIncomplete=false`, `lastBuilt
2026-11-22T03:25:48`); all required artifacts and marker exist. Production save: **PASS**, unchanged tree SHA-256
`17439b7295fd24d1212fe4cb74166bf0003e01d3809ea8dda045f7fc3b1c6f45`. Artifact SHA-256 exe/DLL/INI/marker:
`6f6754754e7e0990d30240b029db0a8cf00a7e9a359fdff9c64137febf8163fe`,
`900652e9606997bb3b6da22afffc3f717b76ddb2f877fa0d9c87fd5ff91f05e0`,
`079843a367a4e519195dc5fc40b701323458d89a72c62e865415cca624aeaf63`,
`37a152ca008a041a2c4b3364d9410c202f8b3846b64f717d27e9ddc5b31dd438`.

Runtime checklist (`runs/02-mkw`): **FAIL** process/device lifecycle (exit 71 and display reset at 13:34:36/39);
**BLOCKED** title/menu/video completion and controllable race (only readable world-map loading frame reached);
**BLOCKED** gameplay input response; **PASS** reviewed map readability before fault; **FAIL** audio completeness
(output opened, non-silent unknown, 4 underruns); **BLOCKED** gameplay FPS/percentiles/stutters and steady memory
(fault before 300-580 s window; loading peak 8668 MB only); **FAIL** diagnostics (invalid read `0xd59256000`, faulting
direct mesh draw #5211, shaders task `fea1d6ca04512de1`, mesh `ddc034e8e009ebfc`, 60 red unique); **PASS** screenshot
visually reviewed; **PASS** production save unchanged; **PASS** owned process/claim cleanup.

`MKW-B01` (critical shared GPU-safety): device loss/reset at a direct 6-group mesh draw, 26.87 s. Evidence:
`runs/02-mkw/run.log:601-659`, `summary.json:257-267`, campaign latch. Ranked hunches: (1)
`graphics/src/vk/exec.rs` native task/mesh direct path still exposes unsafe global-memory loads for this shader pair;
disprove by offline reflection/SPIR-V and descriptor-range validation for `fea1...`/`ddc...`. (2)
`shaders/src/mesh.rs` clamps emitted group count but does not correct task-payload record indexing/initialization for
this six-group pair; disprove with a payload-layout fixture against the translated SPIR-V. (3)
`graphics/src/vk/gmem.rs` or `tables.rs` supplies a stale physical range/generation, matching the invalid read and
zero replay-held ranges; disprove by immutable submission-time range snapshots at draw #5211. (4)
`graphics/src/vk/queue.rs` releases/recycles a task/mesh resource before submission completion, consistent with the
47 recent destructions; disprove by correlating the invalid address with retired allocations and fence epochs.
Agent 2 first action: treat this as the active safety blocker, analyze the exact new shader pair/draw and implement
the proven range/payload/lifetime correction or guarded compute fallback before any MKW retry.

Agent 2 correction/rerun (2026-11-22): **bounded GPU safety PASS; whole-title acceptance remains FAIL.** The exact
pair declared only its 1,408-byte live prefix as the Vulkan task-payload allocation, while the NVN2 task interface is
a fixed 4 KiB payload. `shaders/src/mesh.rs` now allocates and initializes all 1,024 payload words, copies only the
declared live prefix, retains the 32-launch bound, and uses identity `mesh 5`. Shader tests passed (73 pass, 1 ignored,
plus 2 corpus tests); strict clippy remains blocked by 35 pre-existing `translator` warnings. Lean production release
SHA-256 is launcher `cd737ffe3ca5a051b02b5771823bb75a78fc5aa2e2f2affad2c63360047c84ac6`, engine
`128b162747d707e4ffc60824106c6fedf77784bc1fb2dcae8dc42bbc101b611f7`; repository `target/` was removed. Both MKW
entries rebuilt through the real launcher, each regenerated and validated 26 shaders with 0 failures/invalid modules.
The exact pair changed to task SPIR-V `85cf5fe6...a0a626` and mesh SPIR-V `b9377c4d...d48c6d`.

Safety evidence: external `nxrs-agent2/runs/20261122-145357-mkw-safety-mesh5-r1`. The corrected production workload
passed the old 26.87 s fault point and continued through 44.32 s/frame 282 with no invalid read, reset, device loss or
new GPU latch. A 22.08 s frame was CPU pipeline creation (21.02 s), after which replay and mesh draws resumed; it was
not a GPU stall. Startup peak was 12,815 MB, four audio underruns were recorded, and CPU replay/pool/pipeline cost is
still far from native pacing. No acceptance screenshot completed before the bounded timeout. Production save trees
for both entries remained empty with SHA-256 `e3b0c442...b855`; the run used external `NXOS_SAVE`. Next: the required
longer route into a readable controllable race, then source-backed CPU pacing work.

### Mario Kart 8 Deluxe

Build checklist: **BLOCKED** configured source path unreachable; **FAIL** production CLI build (exit 1 in 1.0 s);
**FAIL** current build metadata (`engine=nxrs`, `[nxrs]`, empty `builtSource`); **FAIL** required artifacts; **PASS**
production save unchanged, tree SHA-256 `de5502abd1b5fc7c03708caa6c277ad9fa3b66eb6f7440705a857ad4dadcacba`.
Runtime checklist: all 13 items **BLOCKED**, no current executable. The configured `.../Mario Kart 8 Deluxe/dump`
does not exist; its parent contains a separately named `Mario Kart 8 Deluxe ... (1G+1U+1D).nsp`, but changing the
entry's source without a product flow was outside this validation.

`MK8D-B01` (high, title-specific): launcher entry points to a deleted/moved extracted source. Ranked hunches:
(1) the original `dump` folder was replaced by the adjacent consolidated NSP; check package title/version with
`nxrs-build update-info` or launcher import before selecting it. (2) the source relocation path in
`src/launcher/src/ui/windows/properties_tabs.rs` was never used after storage cleanup; disprove by locating a valid
extracted `main` under the configured path. (3) legacy migration retained `builtSource=dump` after the actual source
changed; inspect `services/legacy` and the Qt backup to establish provenance. (4) the adjacent NSP may not be the
same base/update/DLC selection as this entry; disprove by package metadata comparison before any source change.
Agent 2 first action: use the launcher Properties/normal source-selection flow to point this existing entry at the
verified matching NSP, then rebuild; do not hand-edit JSON.

Current correction/rerun: the launcher now supports validated `--source <package>` for a selected entry, composable
with `--build`; integration test, all launcher tests, formatting and strict clippy pass. The shipped CLI selected the
verified 4.0.1 NSP and rebuilt the existing real-library entry without JSON editing. Evidence
`nxrs-agent2/runs/20261122-220420-mk8d-401-race` reaches a readable responsive race at 58.0 FPS with non-silent audio,
0 gameplay underruns and no device loss/missing shaders. Acceptance remains FAIL on 7 normalized resource categories,
75 stutters and short-scope memory growth.

### Pokemon Legends Arceus (1G+1U)

Build checklist: **PASS** source reachable; **FAIL** CLI build (exit 1, 17.0 s); **FAIL** current metadata
(`buildIncomplete=true`); **FAIL** artifact set; **PASS** production save unchanged (empty tree SHA-256
`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`). Runtime: all 13 items **BLOCKED**.
Evidence: `...Arceus 1G1U [nxrs]/Pokemon Legends - Arceus 1G1U.build.log:31-34`.

`ARCEUS-B01` (high, shared translator/profile patch path): AOT generation rejects matched cut at
`main+0x16e3828` because the original is `b`. Ranked hunches: (1) `translator/aot/emit.rs:482-505` incorrectly
restricts patch-site originals to `Op|Bl`, while this profile deliberately replaces an unconditional branch;
disprove with a focused B-original patch fixture. (2) `profiles/games/01001F5010DFA000/patches.toml:18` encodes a
five-cut runtime set that requires CFG-aware branch preservation the emitter never implemented; disprove by emitting
all five cuts and validating labels/targets. (3) `cli/profile.rs:515-533` tests matching/text only and misses AOT
compatibility; disprove with profile-to-generated-C coverage. (4) this cut may be safe as an always-on build cut,
avoiding runtime preservation of the original B; disprove by checking the profile's intended runtime toggle and
nxnative behavior. Agent 2 first action: fix the same branch-original patch contract exposed by `OOT-B01`, add both
regressions, then rebuild both titles.

Agent 2 correction/rerun (2026-11-22): **FIXED / PASS.** `translator/aot/emit.rs` now supports only the proven
one-word branch-to-branch runtime patch contract: original `B`/`Bcond` (including TBZ) and replacement `B`/`Bcond`.
Each on/off alternative retains its own condition and target; replacement targets are discovered and labelled, flag
reads are retained, multiword/arbitrary branch-original replacements remain rejected, and replacement words decode at
their actual PCs. `aot/patches.rs` seeds direct replacement-B targets. Exact fixtures cover OoT `0x360041e8`, Arceus
`0x17fffefe`, Arceus `Bcond -> Bcond`, labels, targets, runtime toggles and rejection. Translator/profile tests passed
(70/2 ignored and 18/0 ignored, plus corpus/synthetic suites).

Production rebuild applied all five Arceus cuts and produced 12,851 valid shaders, 0 failed/invalid. Combined-entry
evidence `nxrs-agent2/runs/20261122-154931-arceus-combined-long`: first control reached, readable beach/field, movement
replay visibly changed camera/avatar position, native 30 FPS in the 300-460 s window, no red diagnostics/device loss,
non-silent audio with 0 underruns. Base-entry evidence `nxrs-agent2/runs/20261122-155906-arceus-base-long`: same
independent first control and visible movement, no red diagnostics/device loss, non-silent audio with 0 underruns;
capture stalls contaminate aggregate pacing but gameplay intervals are native 30 FPS. Production save trees remained
empty/unchanged (`e3b0c442...b855`); all runs used external `NXOS_SAVE`.

### Pokemon Violet

Build: **PASS**, production CLI exit 0 in 302.0 s; current nxrs metadata and all required artifacts/marker confirmed.
Production save: **PASS**, empty and unchanged. Artifact SHA-256 exe/DLL/INI/marker:
`d9dcfda5fb92d6d33b6cc537573d09030945473a545cf552a7d5f9fa2ae7e9a0`,
`344c7eac15df2a19a8f14d0c6aff2dc55aa54cb65a1f0e90bfc2393589356175`,
`6313be29bd23c6fc7a8d0a332133cc4059837687f922d24f9d393a315f9453ad`,
`c07f00294b294a22d8114c0d258af0eda785731d70ee26cc73286a529b7e5ea3`. Runtime: see completion addendum.

### Pokemon Legends Arceus base

Build checklist: **PASS** source reachable; **FAIL** CLI build (exit 1, 17.0 s); **FAIL** current metadata;
**FAIL** artifacts; **PASS** production save unchanged (empty). Runtime: all 13 items **BLOCKED**. Evidence:
`D:/Projects/Switch2RECOMP-main/build/evidence/updates/packages/SwitchRecomp Builds/Pokemon Legends Arceus base
[nxrs]/Pokemon Legends Arceus base.build.log:31-34`.

`ARCEUSBASE-B01` (high, shared): identical `main+0x16e3828 patched instruction is a b` failure as `ARCEUS-B01`.
Ranked hunches: (1) `translator/aot/emit.rs:482-505` rejects original `Kind::B`; disprove with a focused B-original
fixture. (2) `profiles/.../patches.toml` emits the same five-cut runtime set for this package and needs CFG-aware
branch preservation; emit all cuts and validate targets. (3) `cli/profile.rs` only tests site text, not generated C;
add profile-to-emitter coverage. (4) the cut may safely be build-time-only; disprove by checking its intended runtime
toggle semantics. The second entry disproves output-folder/stale-artifact causes. First action: share the emitter fix
with `ARCEUS-B01`, then rebuild this entry independently.

Agent 2 result: **FIXED / PASS** by the shared constrained branch-emission correction above. This separate production
entry rebuilt all 493 C units, linked, validated all 12,851 shaders, and independently reached readable controllable
field gameplay with visible scripted movement (`nxrs-agent2/runs/20261122-155906-arceus-base-long`).

### Anime Girls Sun of a Beach

Build: **PASS**, production CLI exit 0 in 366.0 s; current nxrs metadata and all artifacts/marker confirmed.
Production save: **PASS**, absent before/after. Artifact SHA-256 exe/DLL/INI/marker:
`3de1849091d2d1747b517f4e04f3fa2486d50115c636594b93f5703536680dd3`,
`69ecbd664c82d8d9d4234d1d3377c6f96ae4063c5197c5bf3426161e29ceb974`,
`63f2b6eed6c6c99d14bd9846edeafa1d987950d1d55a1259da0173df0fa27fae`,
`15a83d125e2624b40b1e4ffaf2484bc9ae20b43024bf30fe1255be32bcfb40d4`. Runtime: see completion addendum.

### Beach Bounce Remastered

Build: **PASS**, production CLI exit 0 in 68.0 s; current nxrs metadata and all artifacts/marker confirmed.
Production save: **PASS**, absent before/after. Artifact SHA-256 exe/DLL/INI/marker:
`69ec8357d6de11b9de3f6a8d90d7dca10ca54cae2bfcda9c61e11e1eb0b8ba88`,
`d108796be9129a908009c4eb9cd9a8273295e211ea593216883aa35cb79309eb`,
`16976ac0cc867eb0b54a0d6d7438801e3c2aeeef76503c60525ce3b1e7d500a1`,
`b3e82d4725b9a7e89d9611a33f3f8b82827375afa59d0dbc61b5a5dbce987c68`. Runtime: see completion addendum.

### SONIC X SHADOW GENERATIONS

Build: **PASS**, production CLI exit 0 in 1153.0 s; current nxrs metadata and all artifacts/marker confirmed.
Production save: **PASS**, absent before/after. Artifact SHA-256 exe/DLL/INI/marker:
`2f075a8aaf670b4329f64176d4559a5bba3ceb4acb26d57c9c13f5c02d3d1f49`,
`466a421d5a8ce67c9b141b0acddea6dd94a4f13681b3aac0d90bbc5f7072e611`,
`96ea4c1ebf733db591f65843d2fb3926f116b405996e3ede0ba55e5a007e8b9d`,
`25441d55c74dac2b7b103c5ab4c21b7d7e9a8c38e524b2813097ab59e552f44e`. Runtime: see completion addendum for both
programs.

### Super Mario Party Jamboree

Build: **PASS**, production CLI exit 0 in 353.0 s; current nxrs metadata and all artifacts/marker confirmed.
Production save: **PASS**, absent before/after. Artifact SHA-256 exe/DLL/INI/marker:
`9af1d5186e3f5823cf23c8901f7a9308e5be2905f035901eb7e604da088c09d9`,
`54203fd659d6441a017504cb0f93ddd992d48f3debf76514601dcbb654d77c73`,
`fe7b7267cac53315f88c3c6c156227a34ea46a97a4cd8178c5537e9fba28d009`,
`fe95ee06f0dff96846bce55b1c03ce8471a828a401b5bd47b021986ce6c4cfc3`. Runtime: see completion addendum.

### Super Smash Bros Ultimate

Build: **PASS**, production CLI exit 0 in 2516.0 s; current nxrs metadata and all artifacts/marker confirmed.
Production save: **PASS**, absent before/after. Artifact SHA-256 exe/DLL/INI/marker:
`ab51d33bb7e90f061712d8b632a24e7ca83f644b00251c1df1f313c1afe1c1f1`,
`85269aa94b5707398a7dca3dfed85272789813da0f31240dc2ad71f0c4396098`,
`17422971f10fd00f52a55e621980641c1c811ec452854fddfec470c4a67ecf70`,
`c41880ffd1851f0928c95730acfa2fca2cb00dfb22d2132f9d04f41b2fc0ca05`. Runtime: see completion addendum.

### Pokemon Legends Z-A

Build: **PASS**, production CLI exit 0 in 286.0 s; current nxrs metadata and all required artifacts/marker confirmed.
Production save: **PASS**, unchanged tree SHA-256 `5d08e6eae5a3f38e6f2429516e1b9ddf9dc0192bbb966e58038c7f8d5e3e58c0`.
Artifact SHA-256 exe/DLL/INI/marker: `f8e71bf8b86118f9522e38393c6ff88ef6983f68ebfed7ae1fd5972e0813c928`,
`ff8c9c533c8eb7ceb55c0b50f6f9c1999cc7989b6b72ffe30c178d1de5217a67`,
`3c7d4a777d2c453b4337162b3e53c13110fd9d94167f19d01cb309281d57a8b3`,
`c1c72101a1b3d521be83f5aeb0e39502c9ea3929311b61e64cc44b94eadf4046`. Runtime: see completion addendum.

### Super Mario Bros Wonders

Build: **PASS**, production CLI exit 0 in 189.0 s; current nxrs metadata and all required artifacts/marker confirmed.
Production save: **PASS**, absent before/after build. Artifact SHA-256 exe/DLL/INI/marker:
`5a3990bd0f7f920ecd19f11d2d0f823c1e52861d73bfb2dc3fef60e3976ce7b5`,
`53d72940705e3edea94d9d84e801759f9c0ea21421e6ad755fbecd217954035b`,
`bf14a2fa297b5e731dbe1dfb9f4385c465e96de6d1d34dbd664311ebf63a9056`,
`3a384d16afb9cc2c9fc3e8cec9e16229e7a96f63852a94cceda4778aa20c5b07`. Runtime: see completion addendum.

### The Legend of Zelda Ocarina of Time

Build checklist: **PASS** source reachable; **FAIL** production CLI build (exit 1, 4.0 s); **FAIL** current build
metadata (`engine=nxrs` but `buildIncomplete=true`, empty `builtSource`); **FAIL** complete artifact set; **PASS**
production save unchanged, tree SHA-256 `068b2f7dae25f746b787629d8e4905d24f5906a7fcdf6c36fe9ae170332b23e4`.
Runtime checklist: all 13 runtime items **BLOCKED** because no current executable was produced. Evidence:
`...Ocarina of Time [nxrs]/The Legend of Zelda Ocarina of Time.build.log:17-20`.

`OOT-B01` (high, title-specific): generation fails at `main+0x805bd0`: `patched instruction is a bcond`, before
compile/link. Ranked hunches: (1) `src/translator/src/aot/emit.rs:482-505` only permits original `Op|Bl` at a
switchable patch site, while the known OoT full-resolution original is TBZ; disprove by decoding `0x360041e8` and a
focused patch-emission test. (2) `src/profiles/src/tests.rs:105-113` asserts patch text but never runs AOT emission,
so profile coverage misses this illegal patch kind; disprove by adding an end-to-end profile-to-emitter fixture.
(3) the patch should be a non-switchable build-time replacement rather than `rt_patch_on`, because preserving a TBZ
original requires branch CFG handling; disprove by proving the emitter can safely retain both branch targets and
labels. (4) the profile's replacement address/word may be stale despite matching build ID; disprove by reading the
source word at `0x805bd0` and validating target `0x80640c` against the current profile/reference. Agent 2 first action:
add the failing OoT emitter regression, then either support TBZ originals with correct CFG/labels or make this exact
full-resolution patch build-time-only; rebuild OoT through the launcher.

Agent 2 build correction (2026-11-22): **build blocker FIXED; gameplay/NDVE acceptance still open.** The constrained
branch patch implementation described under Arceus preserves OoT's TBZ off path and unconditional full-resolution on
path. The real production profile applied, 249,665 main functions emitted, 42,848 shaders validated with 0 failures,
and the title linked successfully. Current lean release SHA-256: launcher `3447ee12...8aad7`, engine
`5bb1e105...2d8c1`; `target/` absent. OoT is not marked PASS until the fail-closed NDVE transaction/parity gates and
first controllable gameplay are completed.

The isolated RTX 5090 Vulkan gate passed: all 24,184 packed words matched the CPU oracle, the retained translated
16-product rounding witness matched, all eight tensors and final RGB10A2 output of the complete 13-dispatch graph
matched, guard bytes stayed intact, and validation added zero errors/warnings. This closes arithmetic/graph parity only.
Current source still has no guest dispatch transaction, texture/image binding/residency/ownership integration or
actual guest-frame shadow comparison; `NXGFX_NDVE=on` therefore correctly remains fail-closed and OoT was not run.

### Mario Kart World v1.8.0

Build: **PASS**, production CLI exit 0 in 229.0 s; current nxrs metadata and all required artifacts/marker confirmed.
Production save: **PASS**, absent before/after build. Artifact SHA-256 exe/DLL/INI/marker:
`6ea95c543ad0daed75c8ce8ca9497a531b3f598e8b725b983880ff4f9ebb2455`,
`e655cc44df265b3b557ad7c22f359c0f899cec600e0acf26e17390fa12dd0633`,
`c8b2c12a881d6e0b2ea21def2f865f12ec723b7f9cd14a379b3af915c535d855`,
`37a152ca008a041a2c4b3364d9410c202f8b3846b64f717d27e9ddc5b31dd438`. Runtime: all 13 items **BLOCKED** by
the no-retry safety latch from `MKW-B01`: this entry rebuilt the same configured base NUP and a race route would
repeat the faulting workload unchanged. It gets its own BLOCKED result rather than a fabricated run. Ranked hunches,
disproof checks and Agent 2 handoff are those of `MKW-B01`; after a reviewed correction this entry must be run
separately because duplicate/versioned library entries remain independent acceptance rows.

## Runtime completion addenda

### Pokemon Legends Z-A runtime

Checklist (`runs/03-plza-gameplay`): lifecycle/title/readability/audio **PASS**; controllable gameplay and gameplay
response **FAIL** (18 paced A presses still left mandatory station dialogue; walk changed framing only); pacing
**PASS** (59.765 FPS, p50/p95/p99/max 16.670/17.481/18.941/71.215 ms, 4 stutters); memory **PASS recorded**
(4972/5935/5935 MB, 892.25 MB/min loading scope); diagnostics **FAIL** (13 red: 11 texture target mismatches,
no-target draw, conservative raster); screenshots reviewed **PASS**; production save unchanged **PASS**; cleanup
**PASS**.

`PLZA-B01` (high, route/title): no controllable state after title and 18 advances. Ranked hunches: (1) the production
save has a longer station dialogue than `SCENARIOS[plza]`; check dialogue-state probes. (2) A edges during transitions
are dropped in `os/input/script.rs`; check consumed-input counters. (3) `NXOS_SAVE` mounted a different slot; compare
logged mounted files with the Turbo run. (4) a service/applet gap suppresses dialogue completion; inspect gaps and
applet state at the final line. First action: replace fixed delays with UI-state probes and prove free movement.

`PLZA-B02` (medium, shared graphics): 11 target-1/target-8 texture mismatches. Ranked hunches: (1) `vk/image.rs`
mis-maps an NVN array/cube target; compare guest metadata. (2) `vk/tables.rs` uses a stale view generation. (3)
`shaders/src/reflect.rs` reports the wrong SPIR-V image type. (4) alias views lose layered-target state. First action:
trace texture ID 3208 through owner/view creation and descriptor binding.

Current observable progression evidence `nxrs-agent2/runs/20261122-231444-plza-observable-long` and
`20261123-002030-plza-final-control` supersedes the dialogue blocker: continuous accepted A edges advance arrival,
starter selection and battle tutorials; the final movement replay visibly runs the avatar through the Battle Zone with
the free-movement objective HUD. No service gap/input stall exists. Final-release control pacing is 57.70 FPS with
14 live 2D->cube mismatches and one no-target operation. Generic cube widening remains
disproven and reverted.

### Super Mario Bros Wonders runtime

Checklist (`runs/04-wonder`): lifecycle/title/gameplay/control/readability/audio **PASS** (coin count 0 to 4;
non-silent, 0 underruns); pacing **PASS** (59.734 FPS, 16.663/16.965/17.162/162.096 ms, 9 stutters); memory
**PASS** (5892/6145/6126 MB, 67.93 MB/min); diagnostics **FAIL** (2,730 missing shader keys/red lines); screenshot
review **PASS**; production save absent/unchanged **PASS**; cleanup **PASS**.

`WONDER-B01` (high, shared shader packaging): 2,730 runtime keys are absent despite a 12,817-shader build. Ranked
hunches: (1) `shaders/src/prebuild.rs` scans the wrong Switch 2 archive; compare missing keys to `manifest.tsv`.
(2) `profiles` selects base-title facts for another build ID. (3) runtime `vk/shader.rs` hashing differs from build
hashing. (4) dynamic shader archives are omitted by the package scanner. First action: locate the first missing key in
the source/manifest and fix the earliest selection or hash divergence.

Agent 2 correction/rerun: **FIXED / PASS.** All 2,730 absent keys were pixel partners from the profile's
`shader_pairs.tsv`; none existed in the installed build's 12,817-key manifest. `cli/pipeline.rs` now reconciles profile
pairs after the real source scan, retaining only pairs whose vertex and pixel keys both exist in the produced manifest;
real draw-time missing diagnostics are unchanged. The build removed 5,557 impossible pair rows, retained 12,817 valid
shaders, and passed all 28 CLI tests. Production rerun `nxrs-agent2/runs/20261122-181817-wonder-pairs-reconciled`:
0 missing/refused shaders, 0 red diagnostics, readable responsive course gameplay, 59.91 FPS, 5 stutters, flat memory,
non-silent audio, no device loss; screenshots visually reviewed and production save unchanged.

### Pokemon Violet runtime

Checklist (`runs/05-violet`): lifecycle/title/readability/audio/pacing **PASS** (30.000 FPS,
33.332/33.834/34.141/45.793 ms, 0 stutters, non-silent/0 underruns); gameplay and gameplay response **FAIL**
(empty production save leaves first-run character creator); memory **PASS recorded** (5386 MB peak, 1102.36 MB/min
loading); diagnostics **FAIL** (1,021 missing shaders/1,023 red); screenshots **PASS**; production save empty/unchanged
**PASS**; cleanup **PASS**.

`VIOLET-B01` (high, route): expected saved house state is absent. Ranked hunches: (1) the save exists under another
legacy output name; inspect only launcher-known save locations. (2) the fixed route incorrectly assumes a populated
save. (3) save import was never run after the Turbo build. (4) the wrong user slot is mounted. First action: use the
normal launcher save flow for an authorized save or add a deterministic first-run route to control.

`VIOLET-B02` (high, shared shader packaging): 1,021 keys missing. Ranked hunches: (1) combined 1G+1U+1D scanning in
`shaders/formats/gamefreak.rs` omits update/DLC archives. (2) profile facts target another update. (3) build/runtime
key hashes differ. (4) live translation is unavailable for these formats. First action: classify keys by archive and
fix the dominant scanner/profile miss.

Agent 2 correction/reruns (2026-11-22): **`VIOLET-B02` FIXED; `VIOLET-B01` still FAIL; new GPU safety blocker.** The
1,021 missing keys had zero intersection with the 35,937 production manifest and came from a v0 profile table installed
for a title-only match against the current v4.0.0 executable. The shared exact-build profile-table gate described under
Smash removed stale prewarm tables; production rebuild retained only `shader_facts.tsv`, 35,937 valid shaders and no
failures. Three deterministic first-run runs then reported **0 missing/refused shaders**, native 30 FPS, flat memory,
non-silent audio, no device loss, and only the pre-existing startup no-target diagnostic. They advanced creator through
enrollment and academy cinematics but did not reach control; `NXOS_SAVE` remained empty, so each run restarted setup.

A fourth route added broad B confirms and was invalid (B backed out to language selection). At 232 s the harness
detected a system display-driver reset (`nxrs-agent2/runs/20261122-173739-violet-first-run-b-confirm`) and created the
external fail-closed `nxrs-agent2/GPU-STOP.json`. The engine itself had no device-loss/fault report and was still
presenting at 30 FPS through frame 6,780 with 1.3-4.2 ms GPU time, so no exact draw/dispatch is identified. No unchanged
retry is authorized. Violet remains FAIL/BLOCKED for control and this new safety fault despite packaging closure.

### Anime Girls runtime

Checklist (`runs/06-anime`): lifecycle/title/gameplay/control/readability/audio/pacing/memory **PASS** (kill count 5
to 8; 59.971 FPS, 16.662/17.088/18.200/39.336 ms, 0 stutters; 3671 MB peak, -4.16 MB/min; non-silent/0
underruns); diagnostics **FAIL** (one conservative-raster red); screenshots **PASS**; production save absent/unchanged
**PASS**; cleanup **PASS**.

`AGS-B01` (medium, shared raster state): conservative raster is ignored. Ranked hunches: (1) `vk/exec.rs` omits
`VkPipelineRasterizationConservativeStateCreateInfoEXT`. (2) the pipeline key omits the enable bit. (3) NVN mode maps
to an unsupported Vulkan mode requiring a proven fallback. (4) only noncritical draws use it; correlate the draw
before reducing severity. First action: implement exact pipeline-state mapping and rerun.

Agent 2 correction/rerun (2026-11-22): **FIXED / PASS.** The NVN enable is now recorded in the command stream and
carried in queue state. On devices exposing `VK_EXT_conservative_rasterization` and extended dynamic state 3, pipelines
declare `CONSERVATIVE_RASTERIZATION_MODE_EXT` dynamic state and draws map off to `DISABLED`, on to exact
`OVERESTIMATE`; unsupported devices retain an explicit one-time red warning. Focused/check and the full graphics suite
passed (127 pass, 3 opt-in ignored). Production rerun `nxrs-agent2/runs/20261122-164641-ags-conservative-raster`:
readable responsive shooter gameplay (kills 5 to 8), 59.97 FPS, 0 gameplay underruns, non-silent audio, no red
diagnostics/device loss, 3,717 MB peak; screenshots visually reviewed. Production save remained absent/unchanged.

### Beach Bounce runtime

Checklist (`runs/07-beach`): lifecycle/title/interactive story/control/readability/audio/pacing/memory **PASS**
(dialogue advances; 59.928 FPS, 16.648/17.842/18.948/62.016 ms, 4 stutters; 1953 MB peak, 4.54 MB/min;
non-silent/0 underruns); diagnostics **FAIL** (16 missing keys); screenshots **PASS**; production save
absent/unchanged **PASS**; cleanup **PASS**.

`BEACH-B01` (medium, title shader scan): 16 startup keys absent. Ranked hunches: (1)
`shaders/formats/unity.rs` misses a variant table. (2) build/runtime hash normalization differs. (3) serialized
variants are generated after the scan. (4) the build log's zero found shaders means the expected scanner never ran.
First action: locate key `0a56e5906042ca31` in source data and add the narrow scanner path.

Correction/rerun **PASS**: complete records in `shaders/live` are durable runtime derivations for Unity-generated
variants. Production rebuild now promotes SPIR-V/reflection and manifest/pair rows into the normal library while
excluding incomplete attempts; focused test passes. Beach promoted all 16 variants. Evidence
`nxrs-agent2/runs/20261122-234826-beach-live-promoted`: readable interactive story, 59.95 FPS, flat ~2 GB memory,
non-silent audio, and zero missing/refused/red diagnostics.

### SONIC X SHADOW GENERATIONS runtime

Checklist: Shadow (`nxrs-agent2/runs/20261122-210700-sxsg-shadow-null-fixed`) lifecycle/title/gameplay/control,
readability/audio/pacing/memory **PASS** (29.99 FPS, p95 36.36 ms, 7,220 MB peak, non-silent/0 gameplay underruns);
diagnostics **FAIL** (218 lines/4 normalized issues). Sonic gameplay **PASS**
(`nxrs-agent2/runs/20261122-193247-sxsg-sonic-successor-fixed`): the harness followed program 1, readable Green Hill
gameplay visibly responded to the spindash route, pacing held about 31.5 FPS, audio was non-silent with 0 underruns,
and program 1 had no red diagnostic or device loss. Screenshots **PASS**; external save/cleanup **PASS**. The whole entry
remains **FAIL** because program 0 retains five blacklisted pipelines and Shadow's resource diagnostics.

`SXSG-B01` (high, virtual memory/image/retained pipeline): 215 requests use byte offsets `1..0x15` with 64/128/256
KiB sizes, violating NVN's documented 4 KiB `NVNmappingRequest` alignment; the ABI layout matches the SDK and the MIT
reference also refuses unaligned requests, so rounding or treating offsets as pages is not source-backed. Texture ID
519 names live 2D placeholder serial 7549 while shaders declare cube/3D targets and receives a safe zero view. One
pipeline remains in the preserved compiler-crash blacklist; no blacklist entry may be removed without proving that
exact corrected pipeline safe. Conservative raster is fixed and no longer part of this blocker.

`SXSG-B02` (fixed 2026-11-22, successor handoff): the bounded harness startup grace correctly retained the child, then
exposed the actual failure: `_p1.exe` inherited program 0's positional `_data` argument and rejected its `0x3e1b000`
main image against program 1's translated `0x5226000` layout. `services/oe.rs` now replaces only the front end's first
positional data argument with the successor's own `_pN_data`, removes inherited `NXOS_DATA`/`NXRS_DATA` overrides and
preserves run options. The focused OS suite passed (4 tests), including the new path regression. Lean production build,
production CLI rebuild of both programs and the Green Hill run above verify the correction end to end.

### Super Mario Party Jamboree runtime

Current production checklist (`nxrs-agent2/runs/20261122-235645-jamboree-clean-final`): lifecycle, opening/title
passage, controllable plaza, visible movement, readability and audio **PASS**. The corrected route waits through the
unskippable opening, holds L+R for three seconds at the title, advances first-run dialogue and moves Mario across the
plaza. Pacing is native-rate overall (59.48 FPS; p95/p99/max 23.11/95.35/98.16 ms, 28 estimated stutters); memory is
flat (12,270 MB peak, -0.98 MB/min); audio is non-silent with 0 underruns; no device loss or service gap occurred.
Typed fallback views and correct timed-wait classification leave zero red diagnostics. Screenshots, external temporary
save and cleanup **PASS**; title acceptance is **PASS**.

`JAM-B01` (fixed 2026-11-22, route/progression): the old route sent skip/title input during the unskippable opening.
The current `library_play.py` route waits 260 seconds, holds L+R at the visible title and reaches responsive plaza
control at native-rate pacing. This closes progression only; it does not close the diagnostics below.

`JAM-B02` (fixed 2026-11-22, typed placeholder): the guest's explicit
null storage descriptors (`address 0`, range `0x130`) already received the correct zero fallback but were incorrectly
reported as missing pools; `vk/exec.rs` now applies the existing uniform-buffer null contract to storage diagnostics.
The focused Vulkan fallback test passed. Tiny single-layer 2D fallback textures now bind zero views keyed by the
shader's image type; ordinary larger/layered live resources still report mismatches. A timed `nvnSyncWait` expiry keeps
its specified timeout result without being mislabeled an engine failure. Production diagnostics fell 73 -> 0.

### Super Smash Bros Ultimate runtime

Current checklist (`nxrs-agent2/runs/20261122-230331-smash-generic-placeholder`): lifecycle, VP9/Vorbis movie passage,
title/menu, responsive match, readability and audio **PASS**. The match completed with visible results; pacing was
58.39 FPS (p95/p99/max 23.70/107.80/516.89 ms, 53 estimated stutters), audio was non-silent with 2 gameplay-window
underruns, and no device loss/service gap occurred. Packaging remains corrected: 27,517 valid shaders, 0
missing/refused. Diagnostics **PASS**: live 16x16 2D fallback serial 25 receives a cube-typed zero view without hiding
ordinary live-resource mismatches. Memory rose 6,146 -> 6,912 -> 6,544 MB in this
loading/match/results scope; a longer repeated-match steady-state slope remains owed. Screenshots/save/cleanup **PASS**.

`SMASH-B01` (critical, shader packaging/performance): almost the entire runtime set is absent, with washout/stalls.
Ranked hunches: (1) `shaders/formats/smash.rs` profile table is not selected for this v0 build. (2) runtime/build key
hashing differs globally. (3) scan reads only one archive/program. (4) live translation is disabled, turning misses
into skipped work. First action: compare the first 100 keys to `manifest.tsv` and profile facts, fix the selection/hash
divergence, rebuild, and rerun the same match.

Agent 2 correction/rerun (2026-11-22): **shader-packaging bug FIXED; title acceptance remains FAIL on residual
pacing/audio/raster/movie issues.** Classification proved 28,273 runtime misses had zero intersection with the 27,517
production manifest, while the missing keys came from the profile's `shader_pairs.tsv`. The v0 build log already said
the selected profile tables were for v13.0.5, but `cli/pipeline.rs` still installed those build-specific prewarm tables.
It now installs `pso_manifest.tsv`, `pso_pairs.tsv` and `shader_pairs.tsv` only for exact build matches, removes stale
copies on a title-only match, and retains only key-addressed `shader_facts*.tsv`. Focused invariant and all 27 CLI tests
passed. Production rebuild left only `shader_facts.tsv`, retained 27,517 valid current-source shaders, and completed in
65 s. Rerun `nxrs-agent2/runs/20261122-162600-smash-profile-gate` reached the same readable responsive match with
**0 missing/refused shaders** and no device loss. Remaining: 56.7 FPS/stalls, 3 gameplay underruns, one VP9 decode
error and the shared conservative-raster warning; these keep the row FAIL but are no longer `SMASH-B01` packaging.

## Bug register

| ID | Severity / scope | Primary symptom | Agent 2 first action |
|---|---|---|---|
| `DKB-B01` | Fixed visual / residual pacing-audio | Visible tunnel restored by Omap + NDSR path | Profile audio underruns and short-window pacing |
| `MKW-B01` | Critical / GPU safety | Device loss/reset at direct mesh draw #5211 | Analyze exact task/mesh pair; fix range/payload/lifetime before retry |
| `OOT-B01` | High / translator | AOT rejects TBZ profile patch original | Add emitter regression and support branch original or build-time cut |
| `MK8D-B01` | Fixed / library source | Configured `dump` source missing | `--source` product CLI selected/rebuilt registered 4.0.1 NSP |
| `ARCEUS-B01` | High / translator | AOT rejects matched unconditional-branch cut | Fix branch-original patch contract and rebuild |
| `ARCEUSBASE-B01` | High / translator | Same failure in separate entry | Verify shared fix independently on base entry |
| `PLZA-B01` | Fixed / route progression | Station dialogue appeared stalled | Observable extended route reaches free movement |
| `PLZA-B02` | Medium / image views | 11 texture-target mismatches | Trace texture ID 3208 |
| `WONDER-B01` | High / shader packaging | 2,730 missing keys | Reconcile runtime keys with build manifest |
| `VIOLET-B01` | High / route/save | Empty save leaves first-run creator | Use normal save flow or deterministic first-run route |
| `VIOLET-B02` | High / shader packaging | 1,021 missing keys | Classify by archive and fix scanner/profile miss |
| `AGS-B01` | Medium / raster state | Conservative raster ignored | Implement exact Vulkan pipeline mapping |
| `BEACH-B01` | Fixed / generated variants | 16 runtime-derived Unity keys | Promote complete live derivations during rebuild |
| `SXSG-B01` | High / virtual memory/image | Unaligned vpool requests and target mismatch | Verify first request's NVN page semantics |
| `SXSG-B02` | Fixed / successor handoff | p1 inherited p0's `_data` path | Successor now selects its own `_pN_data`; production Green Hill pass |
| `JAM-B01` | Fixed / route/progression | Title input landed during opening | Late three-second L+R hold reaches controllable plaza |
| `JAM-B02` | Fixed / typed placeholder | 2 target mismatches on fallback slot 272 | Shader-typed zero views; live mismatches retained |
| `SMASH-B01` | Fixed packaging / residual pacing | 28,273 missing keys closed; match stutters remain | Longer steady match pacing/memory scope |

Each ID's ranked 3-4 hunches, evidence and disproof checks are in its title section above. Shared ordering for Agent
2: `MKW-B01` safety first; `OOT-B01` plus both Arceus branch-patch failures; shader-packaging divergence beginning
with Smash/Wonder/Violet; DKB resource output; then progression and residual descriptor/raster issues.

Disproof checks for the compact runtime hunches, in rank order:

| Bug | Checks corresponding to ranked hunches |
|---|---|
| `PLZA-B01` | Probe actual dialogue state/count; verify consumed A edges; compare mounted slot files; show zero service/applet gaps |
| `PLZA-B02` | Decode guest target; compare pool generation; inspect SPIR-V image type; trace alias provenance |
| `WONDER-B01` | Find key in manifest; verify profile build ID; hash identical shader bytes both ways; locate key in omitted archive |
| `VIOLET-B01` | Find launcher-known save tree; show route works with populated copy; inspect save-copy history; verify mounted user slot |
| `VIOLET-B02` | Locate key by container; match profile update; hash same bytes both ways; prove live translation can consume format |
| `AGS-B01` | Inspect pipeline pNext; compare cache keys; test mapped Vulkan mode; identify affected draw output |
| `BEACH-B01` | Locate key in Unity table; hash same bytes both ways; prove runtime generation; force expected scanner and compare count |
| `SXSG-B01` | Validate page-rounded map; decode offset units; compare texture view generation; retest after `AGS-B01` |
| `SXSG-B02` | Closed: startup grace exposed p0 data inheritance; focused path test and production p1 gameplay pass |
| `JAM-B01` | Probe skip UI; remove fence expiry with correct sync; restore first buffer in a focused run; measure after resources resolve |
| `JAM-B02` | Closed descriptor branch: explicit nulls are valid; residual: trace serial 363 destruction, slot target and zero-view selection |
| `SMASH-B01` | Verify profile selection; hash same shader both paths; locate keys in unscanned program; enable one-key live translation discriminator |

## Final counts and handoff

Current build results: **15 PASS** across all 15 actual entries. Gameplay/acceptance results: **6 PASS, 6 FAIL,
3 BLOCKED**. Wonder, both Arceus entries, Anime Girls, Beach Bounce and Jamboree are current production PASS. DKB,
MKW, PLZA, MK8D, SXSG and Smash have current playable evidence but retain named pacing/resource/safety failures. OoT,
MKW v1.8.0 and Violet remain gameplay-blocked by their documented architecture, safety and title-specific reset gates.

Real library final state: **15/15 `engine=nxrs`; 15 current builds; 0 `buildIncomplete=true`; 0 empty
`builtSource`; 0 other engines**. Metadata was changed only through production
launcher CLI flows. No JSON field was hand-edited.

Production saves are unchanged: every existing tree hash matched before/after its build and run; absent trees stayed
absent. Temporary validation saves are retained only under the external evidence root. Historical global GPU-stop
bytes were copied byte-for-byte to `historical/`; the repository markers were not modified. `campaign-GPU-STOP.json`
retains the MKW reset and per-title latches are under `faults/`.

Evidence: `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-library-validation`. Deliverable:
`D:\Projects\RecompLauncherEngine\docs\34-LIBRARY-VALIDATION.md`. Cleanup verified: no game claim, nxrs process,
PresentMon, compiler, or owned game process remains; GPU reports healthy and no reset occurred after MKW.

Exact Agent 2 handoff: start with `runs/02-mkw` shader pair `fea1d6ca04512de1`/`ddc034e8e009ebfc`, draw #5211 and
invalid read `0xd59256000`; do not retry MKW until corrected. In parallel CPU work, add failing AOT patch fixtures for
OoT `main+0x805bd0` and Arceus `main+0x16e3828`, then correct `translator/aot/emit.rs`. After those safety/build
blockers, compare Smash's first 100 missing runtime keys to its production `manifest.tsv` to locate the shared
shader-selection/hash divergence.

## Agent 2 outcome update - 2026-11-22

This pass additionally closed DKB's black-world deployment, MK8D's supported source-selection product gap, generated
Unity variant promotion, typed fallback views, Jamboree's route/diagnostics, and PLZA progression to free movement.
The current inventory is **15 buildable**, with **6 production gameplay PASS, 6 current FAIL and 3 BLOCKED** as counted
above. The remaining blocked work is OoT's atomic guest NDVE transaction, MKW v1.8's safety dependency, and Violet's
retained display-reset latch; failures retain concrete current gameplay evidence and named residuals.

Final lean release hashes are in the production identity table; repository `target/` is absent. External evidence and
the active fail-closed latch are under `C:\Users\RISKYB~1\AppData\Local\Temp\opencode\nxrs-agent2`. The Violet fault
run is `runs/20261122-173739-violet-first-run-b-confirm`; no unchanged retry was made. All inspected production save
trees for affected entries are empty and unchanged (`e3b0c442...b855`); gameplay used external `NXOS_SAVE`. Cleanup:
no owned game, launcher, compiler, PresentMon client or claim remains; only the Windows PresentMon service is present.
