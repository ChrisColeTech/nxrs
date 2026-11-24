# R1A - Pin and refresh the MIT nxnative Ampere translator

## File table

| Action | Absolute path | Purpose | Pre-existing state |
|---|---|---|---|
| Update | `D:\Projects\RecompLauncherEngine\src\shaders\tools\build_dmsh_xlate.ps1` | Pin and verify the exact Windows translator source before disposable-copy build | Tracked, clean at baseline |
| Update | `D:\Projects\RecompLauncherEngine\src\shaders\tools\build_dmsh_xlate.sh` | Pin and verify the exact Linux translator source before disposable-copy build | Tracked, clean at baseline |
| Update if required | `D:\Projects\RecompLauncherEngine\src\shaders\tools\dmsh_xlate.patch` | Minimum Vulkan/descriptor adaptation for the pinned source | Tracked, clean at baseline |
| Update after oracle proof | `D:\Projects\RecompLauncherEngine\src\graphics\tests\ampere_orin.rs` | Remove only `HERE` operations proven to match Orin | Tracked, dirty at baseline from formatting-only changes across the file |
| New | `D:\Projects\RecompLauncherEngine\docs\waves\R1A.md` | Wave file table and eight-heading report | Did not exist at baseline; `docs\waves` was already untracked |
| New | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R1A\` | Immutable old-tool evidence, disposable source/build, logs, and four candidate translations | Did not exist at baseline; ignored build evidence area |
| Replace after gates | `D:\Projects\RecompLauncherEngine\build\tools\dmsh_xlate\dmsh_xlate.exe` | Install the gated pinned translator | Existing pre-refresh release tool; preserve byte-identical copy in R1A evidence first |
| New | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R1B\` | Independent full DKB candidate, full refreshed shader library, relink/run evidence, stop archive, receipt-bound attempt, and optional corrected candidate | Did not exist when the continuation was assigned; DC-02 remains read-only |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\shaders\src\link.rs` | R2 shader interface correction only if first R1B run proves the mapped-output defect | Tracked and heavily dirty before continuation; preserve all existing edits |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\pipe.rs` | R2 pipeline output/attachment correction only if directly demonstrated | Tracked and heavily dirty before continuation; preserve all existing edits |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | R2 render-state mapping correction only if directly demonstrated | Tracked and heavily dirty before continuation; preserve all existing edits |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\spvfix.rs` | R2 SPIR-V output-component correction only if directly demonstrated | Tracked and heavily dirty before continuation; preserve all existing edits |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shader.rs` | R2 shader metadata correction only if directly demonstrated | Tracked and heavily dirty before continuation; preserve all existing edits |

Baseline project HEAD is `7688e33ef5ef47e64322c7aed0f84c471e9c8b99`; the worktree has extensive unrelated tracked and untracked changes, which this wave does not modify. Reference checkout `D:\Projects\RecompLauncherEngine\build\Q\nxnative-up` is at `7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f`, with no relevant changes under `dm\third_party\ryujinx` or `dm\gfx\tools\dmsh_xlate` at baseline.

## Built

- Corrected both builders to default to `D:\Projects\RecompLauncherEngine\build\Q\nxnative-up` (Linux `/mnt/d/Projects/RecompLauncherEngine/build/Q/nxnative-up`) and require commit `7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f`. They now fail before copying/building when the checkout is absent, at another commit, or has tracked, staged, or untracked changes under `dm/third_party/ryujinx` or `dm/gfx/tools/dmsh_xlate`.
- Made the Windows output/work paths absolute before `Push-Location`; this also fixes relative `-Out` being interpreted below the disposable source directory. Both builders use disposable source under `D:\Projects\RecompLauncherEngine\build\DELEGATION\R1A\` by default. The reference checkout remains clean and unchanged.
- Reused `dmsh_xlate.patch` unchanged. It applies to the copied `7ebb76e` source and retains the Vulkan flags plus per-stage descriptor sets; no upstream adaptation was needed.
- Built and installed the pinned Windows tool only after the gates below. Old identity: `--source-sha e11dc43d4ddc5b58`, SHA-256 `1a27dd0c0b1e0b5317ae67d5d01762b1a32648f93d4f23cf05c6a1f5afdb306a`. New identity: `--source-sha e11dc43d4ddc5b58`, SHA-256 `7a28092f4702c5b067e351cbe105ac4b23aed4b9bb1ce5bd09ccfacfb17dc30e`. The source SHA intentionally remains equal because upstream defines it as the embedded `Program.cs + csproj` hash; the changed executable hash captures refreshed Ryujinx source. The installed executable byte-matches `new-tool\dmsh_xlate.exe`.
- Updated `ampere_orin.rs` only by making the GPU test explicitly ignored, as its prescribed command requires, recording the pinned result, and removing 28 `HERE` rows the oracle printed as `matches now`. Seven measured differences remain in `HERE`; `KNOWN`, `APPROX`, comparison logic, and bit tolerances are unchanged.
- Reference provenance: nxnative commit `7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f`, commit time `2026-11-20 14:31:41 +0800`, subject `game: run Skywalker Saga at 60 fps by default`; root `LICENSE` is MIT, copyright 2026 seedds, and `dm/third_party/ryujinx/LICENSE.txt` is MIT, copyright Ryujinx Team and Contributors.

## Tests written

- No new product test or weakened exception was added. The existing oracle remains the behavioral regression test.
- Added the missing `#[ignore = "requires a Vulkan GPU and the external shader toolchain"]` marker so the lead-prescribed exact `--ignored --exact` command executes one test rather than zero.
- Created an evidence-only focused scanner at `build\DELEGATION\R1A\focus-tool\` which invokes the existing scan, job, fix, mesh-stage, reflection, and validation paths after retaining only the requested keys. It does not modify staged game libraries.

## Gates

- Builder syntax: PowerShell parser and `bash -n src/shaders/tools/build_dmsh_xlate.sh`, exit 0. Missing checkout, wrong commit, and dirty relevant-source probes all failed early with their specific messages. The Linux wrong-commit probe likewise failed early. A detached dirty-test worktree was removed; `git worktree list` again contains only the reference checkout.
- Correct-source build: `src\shaders\tools\build_dmsh_xlate.ps1 -Out <R1A\new-tool> -Work <R1A\dmsh_xlate_src>`, exit 0. Build log: `build\DELEGATION\R1A\build.log`. Existing .NET warning: `Microsoft.DotNet.ILCompiler 10.0.12` has advisory `GHSA-p3jp-9v83-76w9`; not changed in this wave.
- Shader package: `cargo check -p shaders --all-targets --jobs 4`, exit 0. `cargo test -p shaders --jobs 4`, exit 0: 72 library tests passed, 1 ignored; 2 corpus tests passed; other targets/doc tests had zero tests.
- Graphics: `cargo check -p graphics --all-targets --features gpu-tests --jobs 4`, exit 0. Exact oracle test compiled again after the `HERE` edit with `cargo test -p graphics --features gpu-tests --test ampere_orin ampere_ops_match_the_orin --no-run --jobs 4`, exit 0. Existing warning at `src/graphics/src/vk/spvfix.rs:593`: unnecessary `mut`; not in writable scope.
- One leased GPU execution only: `cargo test -p graphics --features gpu-tests --test ampere_orin ampere_ops_match_the_orin -- --ignored --exact --nocapture --test-threads=1`, candidate supplied through `NXSHADERS_XLATE`, outer timeout 90 seconds. Exit 0, one passed, 19.80 seconds, validation on, NVIDIA GeForce RTX 5090, no device loss/reset/timeout. Log: `build\DELEGATION\R1A\ampere-orin-new.log`.
- Oracle comparison: accepted old baseline 124/193 exact matches; pinned build 153/193 (`ok` 153, `differs` 20, `translator` 20). It printed 28 former `HERE` operations as matching: `dp2a_hi`, `dp2a_lo`, `dp4a_s32`, `dp4a_su`, `dp4a_u32`, `f2f_tf32`, `f2fp_pack_bf16x2`, `f2fp_pack_f16x2_relu`, `fadd_rm`, `fadd_rp`, `fadd_rz`, `ffma_rm`, `ffma_rp`, `fmul_rp`, `fmul_rz`, `hfma2`, `hfma2_ftz`, `hfma2_relu`, `hfma2_sat`, `i2f_s64`, `iadd64`, `idiv_u32`, `idiv_u32_by_7`, `imul24`, `imul64`, `irem_s32`, `isad`, `isub64`.
- Retained `HERE` evidence: `f2i_s32_rmi` differs by 4 words; `f2i_s32_rpi` 7; `ffma_rz` 1; `fmul_rm` 1; `fsel_cmp` 2; `fsetp` 116; `fsetp_ftz` 45. No formerly unlisted operation failed.
- Focused translation: both DKB keys found and 2/2 valid; both MKW keys found and 2/2 valid. Explicit `spirv-val --target-env vulkan1.3` passed old and new files for all four keys. `spirv-dis` and `spirv-cross --reflect` completed for all eight modules.
- Scoped `git diff --check` passed. PowerShell and shell parsers passed after final edits. `rustfmt --check src\graphics\tests\ampere_orin.rs` recursively checks `tests/common/mod.rs` and reports that pre-existing file as unformatted; no diff was emitted for `ampere_orin.rs`, and no broad formatting was performed.

## Evidence

- Immutable old executable: `build\DELEGATION\R1A\old-tool\dmsh_xlate.exe`; gated new executable: `build\DELEGATION\R1A\new-tool\dmsh_xlate.exe`; installed path: `build\tools\dmsh_xlate\dmsh_xlate.exe`.
- DKB candidate: `build\DELEGATION\R1A\candidates\dkb\`. Vertex `327992a9765e5b2b` is byte-identical, old/new SHA-256 `a015f36fac2e99277f4b84c2b7035fdc11b42cacc8e2c28984b07d950ecc33a5`, 13,560 bytes. Fragment `3d98baee7872e4b7` changes from `d93f93b7d366f6cabd8980d0b2347260784d268782da33e651229df2688c196e` (205,932 bytes) to `964bafb71194c2bc705d5a040eca5813f9634a19b028fe854b3e8f08f786c22e` (281,508 bytes).
- DKB semantic result: engine reflection differs only in chain/job identity, and `spirv-cross --reflect` is byte-identical. Both versions have `frag_out_map` 111 (`0x6f`), output location 0 as `vec4` with component stores 0/1/2/3, output location 1 as `vec4` with stores to components 1/2, and no `FragDepth`/`DepthReplacing`. Therefore this correction changes fragment arithmetic but does not change output location/component or depth semantics. Disassembly: `build\DELEGATION\R1A\disassembly\{old,new}\3d98baee7872e4b7.spvasm`.
- MKW candidate: `build\DELEGATION\R1A\candidates\mkw\`. Task `6098504f5d12ea03` changes from `479433cb629e681ff829d3b9e7874989b94df8c067afdc5ea151dde4acf9bba7` (70,532 bytes) to `7dc2ca785eebf4eb449c45df96131eb4f7552fe18c70f731e3e41be5de456330` (75,476 bytes). Mesh `1de24686294997d8` changes from `6212264ec8fbcaa09bb5abf62b91c828611eb36fe116f9895bf793bff79205d6` (179,892 bytes) to `98cba286d3ea0f1e8938e71c59389fbee306815d6c00ccbfb4091491aac174a2` (189,024 bytes).
- MKW engine reflection differs only in chain/job identity. `spirv-cross` reflection differs only in generated numeric identifier names; descriptor sets/bindings, task/mesh execution modes, local size, output counts/types/locations, payload metadata, and draw record remain unchanged. This is an arithmetic/code-generation change, not proof of fault safety.
- Focus logs: `dkb-focus.log` and `mkw-focus.log`. Raw disassembly and reflection pairs: `disassembly\{old,new}\` and `spirv-cross-reflect\{old,new}\`. Original DKB/MKW libraries, latches, caches, and game data were not overwritten.

## Unproven

- No game was run. The changed DKB fragment makes a guarded DKB rebuild worthwhile and may improve material arithmetic, but it does not prove pass-27 colour becomes nonconstant or the world becomes visible. Because output/component/depth declarations are unchanged, an unchanged guarded result should proceed to R2 output/attachment/component mapping rather than another translator diagnostic.
- Changed MKW task/mesh arithmetic does not prove the retained invalid-read/reset is fixed; native task/mesh safety remains lead-owned and subject to the HARNESS3 stop history.
- Linux NativeAOT publication was not run on this Windows host; Linux script syntax and fail-fast provenance behavior were checked.
- The 29-match total gain versus the accepted baseline is recorded from aggregate old/new results; the oracle directly authorized removal of exactly 28 `HERE` rows. No claim is made about the additional aggregate match beyond that evidence.

## Not built

- No DKB or MKW staged/full shader library was replaced or rebuilt, no release front/engine was built or relinked, and no game/scenario was executed, as required.
- Lead handoff: rebuild DKB shaders into an isolated candidate data folder using the installed translator, produce the release/relink candidate under the lead's build lease, then authorize the existing short saved DKB route. Acceptance discriminator is pass-27 slot 0 finite nonzero HDR coverage plus a visible input-responsive world. If unchanged, retain this candidate evidence and move directly to R2 output mapping/attachment analysis. MKW should not be the first live discriminator because its retained fault latch requires its separate safety gate.

## Slips

- None. One initial candidate command used relative `-Out`/log paths and exposed the pre-existing `Push-Location` path bug; its nested scratch output was not installed or used as evidence. The script was corrected, then rebuilt cleanly to absolute R1A paths. This did not touch the reference checkout or staged libraries.

## Questions

- Lead decision only: whether to rebuild just the isolated DKB library first or include other affected-title libraries in the next release candidate. Recommendation: DKB first because its named fragment changed and current pass-27 evidence gives a precise guarded discriminator; defer broader relink/rebuild until that result identifies whether R1 or R2 owns the black world.

Compiler and GPU resources are released. The MSBuild/C# build servers were shut down; no owned game, compiler, helper, claim, or GPU workload remains.

# R1B continuation - full DKB candidate and guarded result

The conditional R2 lease above is declared before implementation as required. It is an extension authorized by the owner prompt, not permission for speculative edits; only the smallest source path proven by an unchanged safe first result may be touched.

## Built

- Created `D:\Projects\RecompLauncherEngine\build\DELEGATION\R1B\stage` as an independent 45,743-file, 2.472 GiB copy of the restored DC-02 stage. `robocopy /E /COPY:DAT /DCOPY:DAT /XJ` copied no reparse points; every source/copy SHA-256 matched. A second full verification after rebuild/relink proves all 45,743 DC-02 source files remain unchanged. Initial copy manifest: `stage-copy-manifest.csv`, SHA-256 `a28556927ddb2511b8c5a0baa26bb62bb5fc328ce7d6eb0f1a6973697fc1d6b5`.
- Archived exact pre-resumption bytes without altering the live latches: shared stop `4f2ca437320773411a3a7757f4d686de9803280aafadee16c551eb7c90b66c9b`, shared pending `3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7`, immutable HARNESS3 stop `9447653d8d319644fff34614756ca0a3e0d33ce64b3b3e01ebfd8b3c18be4230` under `R1B\stop-archive`.
- Preserved the complete old shader library at `R1B\old-shaders`, copied only its profile facts into a fresh shader output, and ran the repository `nxshaders prebuild-data` path over the original staged ExeFS/RomFS inputs with the installed refreshed translator. This is a complete coherent library, not a focused-file replacement.
- Full shader result: 22,854 found, 22,854 newly translated, zero reused, zero failed/skipped/invalid, 22,854 valid. Old and new libraries both contain 22,854 `ok` rows with no status changes. 20,004 SPIR-V modules changed and 2,850 are byte-identical. Old manifest SHA-256 `3e413b1426408109a383d89f9a3504f8429a65d134d53ac3072828b1374572f6`; new manifest `bfae4116b1e274099bafd5d4bb08b96ed1969a7e7805c1bc98b344e17aa58231`. `problems.txt` remains empty; pairs and profile facts are byte-identical.
- Verified the current `nxfront.lib` remains exactly the DC-02 receipt identity: 180,804,528 bytes, SHA-256 `f05e631ccc1731370e6b407258c731435e38893acede3ad5fdb098487100984a`. The DC-02-bound `exec.rs`, `render_chain.rs`, `image.rs`, and `queue.rs` hashes also remain exact, so no front rebuild was needed for translator-only changes.
- Verified all 275 cached DKB objects and `link.rsp` against the original link manifest, preserved old link artifacts under `R1B\old-link`, and relinked those objects against the verified library. Candidate executable SHA-256 `ae49bc2a17ab7611fe5ccf53643255cad36aea4e9c521f26208afa25ba1da55e`; INI `f61748e357062e59cba852681f951bd1645f7a219f66d9fa2ce98248e311430c`; new DLL `0ac7b7737d0d0a24f228d85cdb7f2692bd64388104b19d90a90deffa684e0276`; link manifest `6090415c04d39b10b63d4ee3a5c2535188f3696d30ad53465fff895ca6e2f6e9`. Old DLL evidence remains `1a4679a6be50c4bc5b395550069c5fdd19233171b86db34cbedf3b5bc9d5744d`.
- Sealed all 45,743 candidate-stage files in `candidate-stage-manifest.csv`, SHA-256 `a9c3b480c6cc3a686499777a60212738aa79eeb77ef11c79f4c86ffaad940ded`. Prepared a one-attempt R1B driver and receipt gate which bind candidate, code, route, source, library, latches, and pending journal identities. The gate atomically replaces the retained pending journal with receipt proof, keeps ordinary entrypoints blocked, and atomically restores the exact original pending bytes after restoring shared stop history.

## Tests written

- Adapted the reviewed DC-02 receipt lifecycle tests under `R1B\test_receipt_gate.py` for the existing retained pending journal. Seventeen CPU-only failure-injection tests cover exact old-stop and old-pending archives, atomic proof replacement/restoration, fresh generic-guard blocking before/during/after archive, callback/termination/fault failures, malformed/new stop preservation, private fault journals, receipt/scope mismatches, and one-attempt consumption.
- No R2 product test was written because no authorized first game result exists to demonstrate an output/component/attachment defect. The five conditional R2 source files remain untouched.

## Gates

- `CARGO_TARGET_DIR=D:\Projects\RecompLauncherEngine\target\OOTWAIT cargo build -p shaders --bin nxshaders --release --jobs 4`: exit 0.
- `target\OOTWAIT\release\nxshaders.exe prebuild-data D:\Projects\RecompLauncherEngine\build\DELEGATION\R1B\stage\dkb_data --jobs 4`: exit 0 in 238.8 seconds; scan read 9,178 MB with zero problems; all 22,854 outputs passed `spirv-val`. Log SHA-256 `5be5b9d8b0f4a7f8144c0b05143b0fcd4dfaf71039bf3db4344a304877547976`.
- Cached link inputs: all 275 object hashes and response hash matched the DC-02 manifest before link. Clang/lld relink exited 0; object, library, command, DLL, executable, and response identities are recorded in `stage\link.json`.
- R1B gate: `PYTHONDONTWRITEBYTECODE=1`, R1B on `PYTHONPATH`, `py -m unittest build\DELEGATION\R1B\test_receipt_gate.py`: 17 passed, exit 0. `py -m py_compile` passed for `prepare.py`, `receipt_gate.py`, `scenario_clock.py`, and the test.
- Source/candidate integrity: DC-02 source rehashed against all 45,743 initial-manifest rows after all R1B work; no difference. Candidate full-stage verification and `prepare.py describe` passed. Pipeline cache, crash blacklist, pipeline-key cache, ASTC index, and ASTC pack remain byte-identical to DC-02.
- Exact approved state: `R1B\approved-state.json`, 7,399 bytes, SHA-256 `a6783026adf71254014d96a249e0069952f4fe69678fb1cecaec90fe0819b246`. Bound driver SHA-256 `8775f4af52e882c46a5f2a6c9b0ee5652ffb56c9b49959e1fdf41aef89a71e7c`; gate `c32a6ca97449fb7becc1ce2f02a514777167a1ef4af87eb5a1ef1a1fac47818a`; clock `2847c65231c8c6da12bf0decd16e824e50cfa286fa43e7e96bc58599911a6a3a`.

## Evidence

- Full candidate: `D:\Projects\RecompLauncherEngine\build\DELEGATION\R1B\stage`; old shader/library evidence: `R1B\old-shaders`; old link evidence: `R1B\old-link`; rebuild log: `R1B\full-shader-rebuild.log`; exact approved state: `R1B\approved-state.json`.
- Preserved cache/blacklist identities: `pipeline-cache.bin` `a8497571a0e5fefc56a1f970fc3f2e48221fe649ded5c49c1337c11767b859b6`; `pipeline-crashed.bin` `6ae8df6e79b63124b6e3c7c39297ef4c6cc8425933ca22c5bfa00ba409738ae9`; `pipeline-keys.bin` `2b242b0a726148e8114a70b2a69b4302345d05876f283a9c712611a035105804`; ASTC index `ad2776163492020f8a83b817930aeb4b03501655d87588965e9a69e2d6c68721`; ASTC pack `5bd36a3c742138803697840bd828b81b2f1b6c49292d1f9d09e5a21ca98ffcd2`.
- The approved scope is DKB only, one attempt, 170 seconds, no LSFG, isolated data/save/app-data, unchanged title/Continue/walk/punch/walk route, captures every 45 seconds, and bounded one-frame render-chain pass 18/27 evidence. It binds the current shared stop and pending hashes and immutable HARNESS3 hash listed above.
- A valid launch requires a new `D:\Projects\RecompLauncherEngine\build\DELEGATION\R1B\lead-authorization.json` with exactly seven top-level keys: `schema` = `nxrs.r1b.authorization.v1`, `author_role` = `lead`, non-placeholder `author` and explicit `decision`, `approved` = true, `attempts` = 1, and `approved_state` exactly equal to `approved-state.json`. Immediate command after lead creation: `py build\DELEGATION\R1B\prepare.py run --receipt build\DELEGATION\R1B\lead-authorization.json` from the project root with no compiler/game overlap.

## Unproven

- Visible gameplay, player/world visibility, input-responsive scene, pass-27 colour, FPS, skipped draws, red diagnostics, audio, memory, screenshots, and post-run save/cache cleanup are unproven because no lead-authored R1B receipt exists. No game or GPU workload was launched.
- Whether R2 is needed remains unproven. The conditional R2 lease cannot be exercised until the safe first result is black with constant pass-27 colour.

## Not built

- The game result and any conditional R2 correction are not built because protocol section 3 and the supplied stop policy require a lead-authored receipt before temporarily archiving the live shared stop/pending state. The owner prompt grants the attempt but explicitly requires returning the prepared exact state rather than self-authoring a lead receipt when that receipt is necessary.
- No screenshot was produced or viewed. No claim is made about visibility or pass-27 behavior.

## Slips

- The first profile-fact copy command used `-LiteralPath` with a wildcard and therefore copied nothing; it did not alter the preserved old library. The command was immediately repeated correctly with `-Path`, and all four profile files were verified before the full rebuild. No lease or safety deviation occurred.

## Questions

- Lead action required, not a product choice: inspect `approved-state.json` and the 17-test gate, then write the exact lead authorization receipt described above. Recommendation: authorize the one prepared R1B attempt; its candidate and source identities are fully sealed, and ordinary entrypoints remain blocked throughout the archive window.

Compiler/build ownership is released. GPU/game ownership was never exercised. No game, compiler, linker, helper, or claim remains active; the shared stop, shared pending, and immutable HARNESS3 stop remain at their original hashes.

# R2 - DKB fragment output mapping correction

## R2 file table

| Action | Absolute path | Purpose | Pre-existing state |
|---|---|---|---|
| Update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | Correct the demonstrated fragment-output-map to physical-attachment/channel-write association and add focused regression coverage in its existing test module or a declared sibling | Tracked and heavily dirty before R2; preserve all DG/DV/render-chain edits |
| Conditional only | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\pipe.rs` | Pipeline format/output association only if `exec.rs` comparison proves insufficient | Tracked and heavily dirty before R2; no speculative edit |
| Conditional only | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shader.rs` | Reflection/output metadata only if source comparison proves it wrong | Tracked and heavily dirty before R2; no speculative edit |
| Conditional only | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\spvfix.rs` | SPIR-V component/type correction only if module declarations are proven wrong | Tracked and heavily dirty before R2; no speculative edit |
| Conditional only | `D:\Projects\RecompLauncherEngine\src\shaders\src\link.rs` | Cross-stage output linking only if the actual fragment interface is proven wrong | Tracked and heavily dirty before R2; no speculative edit |
| New | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R2-DKB\` | Source/reference analysis, tests, isolated corrected game candidate, relink, gate, and exact approved state | Did not exist before R2; R1A/R1B/DC-02 evidence remains read-only |

## Built

- Proved `frag_out_map=0x6f` means RT0 components RGBA and RT1 components G/B, with RT2 absent: nxnative `EmitterContext.cs:524-565` emits scalar output stores for every enabled bit, and the actual refreshed SPIR-V stores location 0 components 0/1/2/3 and location 1 components 1/2. The earlier roadmap shorthand calling RT1 “RG” was not source-accurate.
- Identified the Vulkan integration defect in `src/graphics/src/vk/exec.rs`: nxrs intersected NVN channel state with every raw OmapTargets component nibble at the physical ROP mask even though the Vulkan SPIR-V already contains the component-level stores. That is Metal-specific handling applied twice. nxnative Metal requires the extra mask because Metal writes whole attachment outputs (`nvn_mtl.m:9365-9379`); Turbo Vulkan instead uses reflected output-location presence to gate the attachment and leaves partial components to SPIR-V (`vk_replay.c:1630-1647`).
- Added `fragment_write_masks`: a nonzero Omap nibble enables that output location's NVN channel mask; a zero nibble disables the physical target. Partial components remain selected by shader stores. For DKB, all-channel state plus `0x6f` now yields physical masks `[RGBA, RGBA, none]` rather than `[RGBA, GB, none]`; the shader itself still stores only G/B to location 1. No title/key hardcoding, draw skip, fallback, recorder, or exception was added.
- Changed only `src/graphics/src/vk/exec.rs`; the conditional `pipe.rs`, `shader.rs`, `spvfix.rs`, and `link.rs` paths were proven unnecessary and left untouched by R2.
- Built release `nxfront.lib`, SHA-256 `c214215536c1e56f939aa17fd371f978c72f97ebc135d0ac630d67ab560a788f`, 180,808,148 bytes. Prepared independent `build\DELEGATION\R2-DKB\stage` from the restored consumed R1B candidate; all 45,734 `dkb_data` files hash-identically to R1B and no reparse point exists. The full refreshed 22,854-shader library is retained unchanged.
- Verified all 275 cached objects and the response file, preserved the prior link under `R2-DKB\old-link`, and relinked against the corrected library. Candidate DLL SHA-256 `ac2261a122562853568772e97cf44e4c65229cc26fa27f213c479dbb1c4cc682`, 209,886,208 bytes; executable `ae49bc2a17ab7611fe5ccf53643255cad36aea4e9c521f26208afa25ba1da55e`; INI `f61748e357062e59cba852681f951bd1645f7a219f66d9fa2ce98248e311430c`; link manifest `28a3683682a50fa26787532e73f43e3bdca1f620984179d8cf03b28469f05506`.

## Tests written

- Added `vk::exec::downstream_tests::fragment_output_map_gates_locations_not_channels`. Its pre-fix counterexample is DKB's `chmask=0xffffffff`, `omap=0x6f`, three physical attachments: old code produced `[0xf, 0x6, 0]`; corrected code requires `[0xf, 0xf, 0]` while the module's scalar stores preserve G/B-only location 1 semantics.
- The same test covers independent channel state, sparse output locations, single-component Omap entries, all-output fallback, and multiple values used by title shaders. Attachment formats in the DKB case are all float-compatible classes (`B10G11R11_UFLOAT`, `R8G8B8A8_UNORM`, `R16_SFLOAT`); integer retyping is therefore correctly not engaged for this draw.
- Adapted the reviewed one-attempt pending-preserving gate under `R2-DKB`; 17 CPU failure-injection cases pass. No R1B receipt/attempt is reused.

## Gates

- Focused regression: `cargo test -p graphics vk::exec::downstream_tests::fragment_output_map_gates_locations_not_channels --jobs 4 -- --exact --nocapture`: 1 passed. An initial command used only the leaf filter with `--exact` and selected zero tests; the corrected fully qualified command executed the test.
- Graphics CPU suite: `cargo test -p graphics --jobs 4`: 124 passed, 3 ignored, 0 failed; doc tests 2 passed. The accepted pre-R2 count was 123/3, so the only added pass is the new regression.
- Shader CPU suite: `cargo test -p shaders --jobs 4`: 72 library tests passed/1 ignored plus 2 corpus tests passed; zero failures.
- Offline module gate: `spirv-val --target-env vulkan1.3` passed the actual refreshed DKB vertex `327992a9765e5b2b` (`a015f36fac2e99277f4b84c2b7035fdc11b42cacc8e2c28984b07d950ecc33a5`) and fragment `3d98baee7872e4b7` (`964bafb71194c2bc705d5a040eca5813f9634a19b028fe854b3e8f08f786c22e`). The runtime correction intentionally does not rewrite either module.
- Release: `CARGO_TARGET_DIR=D:\Projects\RecompLauncherEngine\target\OOTWAIT cargo build --release -p front --jobs 4`, exit 0. Existing unrelated warnings remain `spvfix.rs:593 unused_mut` and deprecated `fetch_update` in `os/services/oe.rs:365`; neither was cleaned outside scope.
- Candidate copy/relink: all 45,734 data files source/candidate SHA-256 matched; no reparse points; all 275 object and response hashes matched before clang/lld relink; link exited 0. Caches and blacklists were copied, not deleted.
- R2 receipt gate: 17 tests passed; `py_compile` passed; `prepare.py describe` verified the sealed 45,743-file stage and produced the exact approved state. Scoped `git diff --check` passed.

## Evidence

- R1B discriminator: exit 0, 167.5 seconds, peak 8,818 MB, no reset, save/candidate/code cleanup true. Viewed screenshots remained HUD-only black (`replay_gameplay` black fraction about 0.9448; `shot_end` about 0.9449). Diagnostic performance 100-150 seconds was 15.585 FPS, 429 stutters, dropped work 63.
- Frame 5318 pass sequence: material draws with VS `327992a9765e5b2b`, FS `3d98baee7872e4b7`, depth EQUAL/write off, `chmask=0xffffffff`, `omap=0x6f`, and three targets `B10G11R11_UFLOAT`, `R8G8B8A8_UNORM`, `R16_SFLOAT`. Completed raw hashes were slot0 `70397d2534be8eb8f2a8d32052dc07398ebf0e1f426e8aa18a3cc06da152a4c7`, slot1 `b44652badabfe57ed2777ab43d453b8a5097e249a31acafbab5274bfdf5ad50d`, slot2 `35d4ae982aa5c19c106461abf61f0085d510874fb83b90567a3d5f8c9bc4a553`, depth `a569cb5feccba6febfe391250e10bea2bcd3be04cec0fb1208a04865116598ca`.
- Reference agreement: nxnative exact MIT checkout `7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f` establishes the Omap bits and scalar stores; its root and Ryujinx licenses are MIT as recorded in R1A. Turbo exact commit `11e966f33b1f2aec8b05d0764f795804e807510c` establishes Vulkan location-gating behavior but its root is AGPL-3.0. No Turbo code was copied; the Rust helper/test are an independent implementation from behavior and the MIT shader-interface facts.
- R2 exact approved state: `D:\Projects\RecompLauncherEngine\build\DELEGATION\R2-DKB\approved-state.json`, SHA-256 `f75bab192df942320407112348a9d7e09f4bc82448756da6e0748e7ac1a10497`, 7,448 bytes. Candidate manifest SHA-256 `3aa0cc84c8e3b570a5deb197e59c9365842abbfe3cdf6449d1ee1aeb131f8bf6`; bound driver `fdd86cf1d056bf62336371e29fc82d3ff2a14d6e7b4376287062aa4e5c512be2`; gate `e24aed7cd2fc8241fe8c60694f185cfd1d574571a9ec58b75c436c89f0cf091d`.
- Bound stop identities: shared `7a1071c977a9de1fc19ce9e5d90e0b0af37033a05fa4e11ac3bd7360fee791e5`, pending `3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7`, immutable HARNESS3 `9447653d8d319644fff34614756ca0a3e0d33ce64b3b3e01ebfd8b3c18be4230`; exact bytes archived under `R2-DKB\stop-archive`.

## Unproven

- The corrected DKB picture and pass-27 colour are unproven until the lead-authored R2 receipt is created and the single guarded route runs. No game was run by this implementation phase.
- The correction is intentionally bounded to Vulkan output-location/component association. If the corrected route remains black, its new raw result must decide the next material defect; this receipt permits one run only and no automatic retry.

## Not built

- No additional R2 source path was modified, no shader library was regenerated unnecessarily, and no game run was attempted without the required fresh lead receipt.
- Lead must create `D:\Projects\RecompLauncherEngine\build\DELEGATION\R2-DKB\lead-authorization.json` with exactly: `schema` = `nxrs.r2-dkb.authorization.v1`, `author_role` = `lead`, non-placeholder `author` and explicit `decision`, `approved` = true, `attempts` = 1, and `approved_state` exactly equal to `approved-state.json`.

## Slips

- None. The zero-test focused invocation was immediately corrected and is recorded above; it consumed no GPU/game resource and changed no file.

## Questions

- Lead action required: review the source diff, tests, candidate hashes, and exact approved state, then author the fresh receipt. Immediate command from `D:\Projects\RecompLauncherEngine`: `py build\DELEGATION\R2-DKB\prepare.py run --receipt build\DELEGATION\R2-DKB\lead-authorization.json`.

Compiler/build ownership is released. GPU/game ownership was not exercised for R2. No game, compiler, linker, helper, or claim remains active; retained stop history remains present and bound by the approved state.

# R3 - DKB downstream scene composition correction

## R3 file table

| Action | Absolute path | Purpose | Pre-existing state |
|---|---|---|---|
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | Correct the first demonstrated replay binding/write association defect; preserve R2 Omap fix | Tracked and heavily dirty before R3 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | Correct effective-view/format/swizzle or backing ownership only if the chain proves it first | Tracked and heavily dirty before R3 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\tables.rs` | Correct descriptor snapshot selection only if exact binding evidence proves it | Tracked and heavily dirty before R3 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\upscale.rs` | Correct the exact upscale input/output association only if current frame reaches it incorrectly | Tracked and heavily dirty before R3 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\present.rs` | Correct final presentation composition only if it is the first failed consumer | Tracked and heavily dirty before R3 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shader.rs` | Correct affected shader reflection/resource handling only if demonstrated | Tracked and heavily dirty before R3 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\spvfix.rs` | Correct affected SPIR-V integration only if demonstrated | Tracked and heavily dirty before R3 |
| New | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R3-DKB\` | Bounded frame-chain analysis, tests, isolated corrected candidate, gate, and approved state | Did not exist before R3; R2 artifacts read-only |

## Built

- Decoded R2 pass-27 slot 0 as `B10G11R11_UFLOAT_PACK32`: 10,307 of 1,166,400 pixels are finite nonzero (0.8837%), bounding box `(0,0)..(906,335)`, finite range `0..54`; representative RGB values include `(0.1045,0.0435,0.0154)` through `(0.9297,0.6484,0.1328)`. This proves meaningful HDR material values rather than raw-backing hash noise.
- Reconstructed the existing frame-5670 chain without a new recorder: owner 60574 remains the scene target through pass 707; pass 708 copies it through `e9c6f339b7550c71` into owner 60584; passes 727-729 tone-map/compose into owner 53348; compute `14a8aaf3e0a9edbd` samples owner 53348 and stores owner 4856; pass 748 samples owner 4856 through the final effective view and writes the window target. Effective sampled views are `B10G11R11_UFLOAT` with `[R,G,B,ONE]`; the LDR alias passes use audited `R8G8B8A8_UNORM`; final attachment view is sRGB. Descriptor selection, backing owner, view format and swizzle agree throughout.
- Identified the first remaining production divergence from nxnative’s DKB behavior: `RENDER_COND` treated an unresolved same-submission counter pair as passing. nxnative’s DKB profile records the exact black-frame cause as a conditional clear-only composite rebind executing before the upscaler reads it. Known conditions in R2 are evaluated, but nxrs still had a fail-open `_ => true` path when either counter value was unavailable.
- Added `render_condition` in `exec.rs`: unconditional mode 2 renders, mode 3 does not; equal/not-equal modes use only a complete counter pair; an unresolved pair now fails closed and cannot erase a valid composite. The R2 Omap correction remains intact. No other graphics source file was changed.
- Built release `nxfront.lib` SHA-256 `658ab01f8e3dc71b436d595c3ec59832029b563a453709ac68f179fd7b49b62c` (180,803,788 bytes), independently copied the restored R2 stage into `R3-DKB`, preserved caches/blacklists/full refreshed shader library, verified all cached link objects, and relinked. R3 DLL SHA-256 `e684e885b5c41f99500545b3a8ba252f76eac6da5279652e012db1fea889b49c`; executable `ae49bc2a17ab7611fe5ccf53643255cad36aea4e9c521f26208afa25ba1da55e`; INI `f61748e357062e59cba852681f951bd1645f7a219f66d9fa2ce98248e311430c`; link manifest `0787fecc5a23323f58cf4fa4b80038aa9de82805f46d77c7c2ad4ec008192665`.

## Tests written

- Added `vk::exec::downstream_tests::unresolved_render_condition_cannot_clear_a_composite`. It covers unconditional render/skip, known equal and unequal pairs for both conditional modes, and the demonstrated unresolved-pair counterexample. Before the correction both unresolved assertions rendered; now both are false.
- Reused the existing R2 Omap regression, which continues to pass. No shader source changed, so no shader test was added.
- Adapted the reviewed single-attempt gate under `R3-DKB`; all 17 pending/stop/receipt failure-injection tests pass.

## Gates

- Focused test: `cargo test -p graphics vk::exec::downstream_tests::unresolved_render_condition_cannot_clear_a_composite --jobs 4 -- --exact --nocapture`: 1 passed after correcting a compile-time closure/type error found by the first invocation.
- Graphics CPU suite: 125 passed, 3 ignored, 0 failed; doc tests 2 passed. `cargo check -p graphics --all-targets --jobs 4` passed.
- `cargo clippy -p graphics --all-targets --jobs 4` passed with the existing repository warning set; no new warning points to `render_condition` or its test. Existing warnings were not broadly cleaned.
- Offline affected path: actual NDSR compute `14a8aaf3e0a9edbd` passes `spirv-val --target-env vulkan1.3`; its sampled binding resolves owner 53348 and storage binding owner 4856 in the R2 log. The source correction changes replay condition state, not SPIR-V.
- `cargo build --release -p front --jobs 4` passed. Existing `spvfix.rs:593 unused_mut` and deprecated OS `fetch_update` warnings remain unrelated.
- Candidate: 45,743 stage files sealed, all cached objects verified before relink, no writable junctions introduced. R3 gate tests: 17 passed. `prepare.py describe` passed and produced the exact approved state.

## Evidence

- Bounded analysis: `build\DELEGATION\R3-DKB\analyze_frame.py`, `frame-analysis.json`, NDSR disassembly and GLSL. It uses only R2 evidence and records exact producer/consumer links; it adds no engine recorder.
- R2 result retained: exit 0, 166.6 seconds, no reset, cleanup/save restored; HUD/tutorial-only final image despite populated pass 27. Pass-27 hashes: slot0 `584d74c7b4b7c0785f722400dd5a30ba93235fac962935f5ab1b7c5ef30fb140`, slot1 `a1f5a20ee0d7e20c15c47c8862dfb27c4b6c0d781446d248273f607329af9fd3`, slot2 `75413b1989b7b74ce5071ac397a804f8f066618f1b19905bcb68127cc4d0497c`, depth `8d63a6fa58f28835dc760fe30074272e06b52e90e2d043cf8c1a8bcc77dfbc55`.
- Reference agreement: MIT nxnative commit `7ebb76e`, `dm/games/040074A01BF12000/README.md:21,39-43`, identifies conditional clear-before-upscale as DKB’s black-frame mechanism and states render conditions are evaluated at replay. The implementation is independent Rust using existing nxrs counters; no AGPL Turbo code was copied.
- Exact approved state: `D:\Projects\RecompLauncherEngine\build\DELEGATION\R3-DKB\approved-state.json`, SHA-256 `ef1b7005ae6b44576e8660c4927dac21a8793a5d640844f5863cf1d90f1ce7bf`. Candidate manifest `3a54d199ad67404617637e152434076c8bd2eb049e00c70b1727bb3faa2271a2`; driver `c87828eb25f0afcc7177bc805bb04525ca8abba439f697e075a83f7eaa2471df`; gate `6be46d1debbf44b4f2774dc04c4a856c69a8c5288099234fd6c76123ef877252`.
- Bound stop identities: shared `f33221d3609e3b59f114e7da07c8cfeec2d78c1a9d0c45ffde362efa265514d4`, pending `3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7`, immutable HARNESS3 `9447653d8d319644fff34614756ca0a3e0d33ce64b3b3e01ebfd8b3c18be4230`.

## Unproven

- The corrected final picture remains unproven until the fresh lead receipt authorizes the one R3 route. No R3 game was run by this implementation phase.
- An unresolved condition now preserves rather than clears content. If R3 remains black, the exact R3 pass chain is required for the next correction; this candidate allows no retry.

## Not built

- No new recorder, no shader rebuild, and no edit to `image.rs`, `tables.rs`, `upscale.rs`, `present.rs`, `shader.rs`, or `spvfix.rs`; current frame evidence showed their descriptor/view/format chain was coherent.
- Lead must create `build\DELEGATION\R3-DKB\lead-authorization.json` with schema `nxrs.r3-dkb.authorization.v1`, `author_role: lead`, explicit author/decision, `approved: true`, `attempts: 1`, and `approved_state` byte-for-value equal to `approved-state.json`.

## Slips

- The first focused compile exposed two local implementation errors (`?` in a tuple-returning closure and signed mode type); both were corrected before any successful gate or candidate build. No GPU/game resource was used.

## Questions

- Lead action required: review and author the fresh receipt, then run from project root: `py build\DELEGATION\R3-DKB\prepare.py run --receipt build\DELEGATION\R3-DKB\lead-authorization.json`.

Compiler/build ownership is released. GPU/game ownership was not exercised for R3. No game, compiler, linker, helper, or claim remains active; stop/fault history is preserved.

# R4 - DKB runtime shader-key closure

## R4 file table

| Action | Absolute path | Purpose | Pre-existing state |
|---|---|---|---|
| Conditional update | `D:\Projects\RecompLauncherEngine\src\shaders\src\scan.rs` | Correct demonstrated source discovery omission | Tracked and heavily dirty before R4 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\shaders\src\prebuild.rs` | Correct demonstrated build/library publication or retained-key closure defect | Tracked and heavily dirty before R4 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\shaders\src\job.rs` | Correct demonstrated job/key construction defect | Tracked and heavily dirty before R4 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\shaders\src\nvn.rs` | Correct demonstrated shader-key hashing mismatch | Tracked and heavily dirty before R4 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\shaders\src\xlate.rs` | Correct demonstrated translation publication omission | Tracked and heavily dirty before R4 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\prog.rs` | Correct proven restored program-key mismatch only | Tracked and heavily dirty before R4 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\prog2.rs` | Correct proven NVN2 program-key mismatch only | Tracked and heavily dirty before R4 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shader.rs` | Correct proven runtime base/variant lookup mismatch only | Tracked and heavily dirty before R4 |
| Conditional update/revert | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | Retain or revert only the unsupported R3 conditional hunk after log/reference proof; preserve R2 Omap fix | Tracked and heavily dirty before R4 |
| Lease extension: update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\pipe.rs` | Stop retained pipeline-variant/stale member hashes from being looked up and reported as base-library shader requirements during prewarm | Tracked and heavily dirty before R4; extension declared after retained-key classification |
| New | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R4-DKB\` | Missing-key classification, closure tests/report, rebuilt library, isolated candidate, gate, approved state | Did not exist before R4; earlier artifacts read-only |

## Built

- Classified all 87 R3 “missing” keys against the 22,854-row rebuilt manifest, nxnative shader map/profile, runtime pipeline lines and frame-5670 chain. None is a source shader key in either nxrs or nxnative. Thirty-seven are runtime-generated module/variant identities that later compile and execute in the same run (examples `5b97f25bc32f378a`, `a47505fc6abc10bc`, `7fd0f36ac248e71c`); fifty occur only as retained pipeline members and are never reached. The 173 prewarm misses are pipeline-member occurrences, not 173 source shaders.
- Proved the final-chain source keys are present: `327992a9765e5b2b`, `3d98baee7872e4b7`, `e9c6f339b7550c71`, `0e423e497f1e3355`, `9f6e1f0fb59db67c`, `f9b50f0536b57926`, and `14a8aaf3e0a9edbd` are valid library entries. No missing key is the frame-5670 material, tone-map, composite, NDSR or final-copy shader.
- Corrected the actual coverage defect in `vk/pipe.rs`: persisted pipeline keys are now checked with `Library::has` before prewarm calls `get`. Runtime-generated one-way variant hashes and stale pipeline members are skipped without being looked up/logged as missing base-library shaders; their owning draw still recreates variants after base modules and state are known. Caches/blacklists remain intact.
- Tested and withdrew an NVN2 archive-padding key hypothesis: a complete rebuild produced an identical manifest and closed 0/87, so the `nvn.rs` hunk/test were reverted. The final source contains no speculative key change.
- Reverted only the R3 fail-closed conditional helper/test. R3 gave no counter-value proof and dropped work rose from 22 to 133; known false conditions already worked before R3. R2 Omap remains.
- Rebuilt the full DKB library: 22,854 translated, 22,854 valid, zero failed/skipped/invalid; manifest SHA-256 remains `bfae4116b1e274099bafd5d4bb08b96ed1969a7e7805c1bc98b344e17aa58231`, proving key/source output returned to the accepted full build after hypothesis withdrawal.
- Built and relinked R4. `nxfront.lib` SHA-256 `4451f532779ac05fa2004de55ea376b8405cb2c5836d52b932ff52493ebf3005`; DKB DLL `1e3175e08f797e923e59449da869c1bea60510a46a2c440f6b861fd9896c6667`.

## Tests written

- Added `vk::pipe::pipeline_identity_tests::prewarm_extracts_only_pipeline_shader_members`, covering graphics empty slots and compute membership used by the prewarm availability gate.
- The temporary NVN2 padding regression was removed with its disproved hypothesis. Existing restored-program-key and variant-identity tests remain unchanged and pass.

## Gates

- Focused prewarm regression: 1 passed. Graphics suite: 125 passed, 3 ignored, 0 failed. Shader suite: 72 library tests passed/1 ignored plus 2 corpus tests passed.
- Full rebuild: 22,854 found/newly translated/validated, zero problems, 244.5 seconds. Old/new manifest hashes are identical.
- Retained-key closure: source-available missing keys `0`; runtime-generated/reached variants `37`; stale/unreached retained members `50`. Final-chain unavailable keys `0`.
- Release front build, cached-object verification and relink passed. R4 gate failure-injection suite: 17 passed. Candidate stage sealed with 45,743 files.

## Evidence

- R3 summary `missing_keys` plus run log establish the 37 later-compiled identities; manifest/nxnative-map joins establish 0 source entries. Examples: `5b97...` logs missing at 1.30 s, then compiles at 92.07 s and is bound across captured passes; `7fd0...` follows the same pattern.
- The full shader rebuild log is `build\DELEGATION\R4-DKB\full-shader-rebuild.log`; accepted manifest SHA is above. Prior full library is preserved at `R4-DKB\old-shaders`.
- Exact approved state: `D:\Projects\RecompLauncherEngine\build\DELEGATION\R4-DKB\approved-state.json`, SHA-256 `e7c2c63334d34795f8cabbb010d1f073547ebebbef7458cdf77ba2a6aacfbf75`. Candidate manifest `0bb52eff98228111d5e7f6dc9eeb43a60fe9cf251e98a04e8bc8df83befefcb8`.
- Bound stop identities: shared `564cd806dd9bee73c7a073ce1bd3bf02a2aa542bd08fc9495dfdade1a12bd24d`, pending `3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7`; HARNESS3 remains immutable.

## Unproven

- R4’s visible result is unproven pending a fresh lead receipt. The 87-key warning was not the direct final-chain black cause; R4 corrects its prewarm treatment and removes R3’s harmful unsupported branch.

## Not built

- No game run was attempted. Lead must create `R4-DKB\lead-authorization.json` with schema `nxrs.r4-dkb.authorization.v1`, role `lead`, explicit decision, one attempt, and approved state exactly equal to the prepared JSON.

## Slips

- None. The padding hypothesis was tested end-to-end, disproved by exact closure/manifest evidence, and fully reverted before release/candidate build.

## Questions

- Lead action: authorize and run `py build\DELEGATION\R4-DKB\prepare.py run --receipt build\DELEGATION\R4-DKB\lead-authorization.json` from the project root.

Compiler/build resources are released. No game, compiler, linker, helper or claim remains active; stop/fault history is preserved.

# R5 - DKB scene-chain resource correction

## R5 file table

| Action | Absolute path | Purpose | Pre-existing state |
|---|---|---|---|
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | Correct demonstrated descriptor bank/target/range binding defect | Tracked and heavily dirty; R2 Omap retained, R3 reverted |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | Correct demonstrated target/view/alias interpretation defect | Tracked and heavily dirty before R5 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\tables.rs` | Correct demonstrated texture-table generation/index defect | Tracked and heavily dirty before R5 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shader.rs` | Correct demonstrated reflection/resource-type defect | Tracked and heavily dirty before R5 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\pipe.rs` | Correct narrowly proven blacklist alias/migration defect; preserve R4 prewarm fix | Tracked and heavily dirty before R5 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\tex.rs` | Correct demonstrated registered NVN target/view metadata defect | Tracked and heavily dirty before R5 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\prog.rs` | Correct proven program/reflection key issue only | Tracked and heavily dirty before R5 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\prog2.rs` | Correct proven NVN2 program/reflection issue only | Tracked and heavily dirty before R5 |
| New | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R5-DKB\` | Bounded resource classification, tests, isolated candidate, gate and approved state | Did not exist before R5; prior evidence read-only |

## Built

- Classified the first demonstrated R4 scene-chain resource defect before editing source. At 92.57 s/frame 5120, texture ID 62272's table slot still names serial 45150 while the live-image map no longer contains it; the binding consequently falls back to the zero image and reports target 1 where the fragment resource requires target 2. The later frame-5720 and frame-6320 diagnostics name the same ID and serial, establishing persistence rather than a one-draw transient.
- Corrected the bounded lifetime defect in `vk/image.rs::materialize`. After the existing lazy-registration path, materialization now resolves the table's exact serial, leaves an existing live image unchanged, and otherwise removes that same serial's retained `Image` from the parked set and restores it to `by_serial`. This preserves the image's actual target, format, backing and cached views; it does not coerce a descriptor target, fabricate a view, clear a blacklist, or alter unrelated resources.
- Preserved R2 Omap, the R4 prewarm availability gate, all pipeline caches, and the prior compiler-crash blacklist. No R5 changes were made to `exec.rs`, `tables.rs`, `shader.rs`, `pipe.rs`, `nvn/tex.rs`, `nvn/prog.rs`, or `nvn/prog2.rs`.
- Built release front and relinked the isolated R5 stage. `nxfront.lib` SHA-256 `a8a0d331ac1fbb79b16931cb5a5b6c1f5a47079a67de9ca6361aff30e8d9174e`; R5 DKB DLL `c66c2fbc374aa5812b16b9350e28b582528805fade96337c96e0a3d513be07a5`.

## Tests written

- Added `vk::image::parked_materialize_tests::table_serial_revives_its_exact_parked_target_and_view`. It uses the production parked-serial selector with neighboring serial 45149 and the demonstrated serial 45150, and proves only 45150's target/view tuple is removed and returned while the unrelated parked image remains.

## Gates

- Focused production-path regression: 1 passed. Full graphics suite: 126 passed, 3 ignored, 0 failed; doc tests: 2 passed. `cargo check -p graphics --all-targets --jobs 4`: passed with the pre-existing `spvfix.rs:593` unused-`mut` warning.
- Release front build: passed. R5 receipt-gate failure-injection suite: 17 passed. Candidate stage contains 45,743 files and describes successfully against its own R5 paths and current source identities.
- RED, pre-existing workspace debt: strict `cargo clippy -p graphics --all-targets --jobs 4 -- -D warnings` does not reach an R5-local finding because dependency crates `config` and `translator` fail first on existing warnings (35 translator findings plus `config/options.rs:225`). No files outside the lease were changed to hide these failures.
- RED, pre-existing workspace debt: `cargo fmt --all -- --check`, package fmt check, and direct `rustfmt --check` report broad existing formatting drift, including pre-existing sections of `image.rs`; the new helper/test is rustfmt-shaped. No bulk formatting was applied to the dirty tree.
- SKIPPED: offline validation of `d87ccbede95b9f2c` and `6655320127e098d6`; `spirv-val` is not installed or available on `PATH`. The pair remains blacklisted and no compilation attempt was forced.

## Evidence

- Primary resource evidence: `build\DELEGATION\R4-DKB\run\run.log` lines 1925-1926 report ID 62272 target mismatch and table serial 45150 with no live image; lines 29907 and 30546 repeat the absent serial. Frame-1252 pass dumps preserve ID 62272/serial 45150 and its sampled metadata.
- The five initial zero-address storage-buffer reports (`b141b821bf9d2ee0`, `77fab64c80aafa32`, `644a1075d83095e7`, `a8eac51a9b0a979a`, `6e11972dff0d701f`) are compute/background diagnostics, not the demonstrated ID-62272 lifetime path. They remain unfixed and are not represented as the final scene correction.
- The one retained 79-byte crash key, VS `d87ccbede95b9f2c` plus FS `6655320127e098d6`, remains untouched. Existing R4 logs show other VS combinations with the fragment shader compiling; no evidence justified deleting or bypassing the exact prior crash record.
- Exact approved state: `D:\Projects\RecompLauncherEngine\build\DELEGATION\R5-DKB\approved-state.json`, SHA-256 `e365fdd1cd78f0bc7242019211b9fb065f7aa5071421e3f4666fa6665676e927`. Candidate manifest SHA-256 `7ae1a67d088a5b7a39544ee1b6ecfa7d5c7bc123301d8951f60afe78b8aaea1d`; link manifest `8d6253ee80cbb85c528325bd81131bf82d74a05d7711756524a7c52d90269438`.
- Bound stop identities remain byte-identical to their pre-work archives: shared `8368aff787fafc91387a68e96ae7cdee868b6dd92a3c0a71750a981816e1a24e`, pending `3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7`, HARNESS3 `9447653d8d319644fff34614756ca0a3e0d33ce64b3b3e01ebfd8b3c18be4230`.

## Unproven

- The production game result is unproven. CPU tests prove exact parked-serial selection and metadata preservation, but only one lead-authorized DKB attempt can establish whether ID 62272 is revived in the captured scene and whether the world renders.
- The unavailable offline validator means no new claim is made about the blacklisted shader pair. The zero-address storage-buffer diagnostics are classified but not corrected.

## Not built

- No game/GPU workload was run, no receipt was created or consumed, no stop file was cleared, and no compiler-crash key was retried.
- No speculative target coercion, descriptor-bank change, reflection rewrite, table-generation change, or blacklist migration was built.

## Slips

- Protocol slip: during candidate sealing, an R5 intermediate was mistakenly copied/relinked under the consumed `build\DELEGATION\R4-DKB\stage` path before the required `R5-DKB` directory was created. The consumed R4 `run` evidence and stop histories were not changed, and displaced link assets were retained under R4 backup directories, but the R4 stage root and its manifest/approved-state files are no longer the original read-only bytes. The final candidate was rebuilt, relinked, manifested and sealed under `R5-DKB`; this report does not conceal or call the R4 stage mutation harmless.

## Questions

- Lead action: review this report and create `R5-DKB\lead-authorization.json` only if authorizing the one bounded attempt. The receipt schema is `nxrs.r5-dkb.authorization.v1`, with role `lead`, explicit decision, one attempt, and `approved_state` exactly equal to the prepared JSON.
- Authorized command, from the exact project root: `py build\DELEGATION\R5-DKB\prepare.py run --receipt build\DELEGATION\R5-DKB\lead-authorization.json`.

Compiler/build resources are released. No game, compiler, linker, helper or claim remains active; stop/fault history and the prior blacklist are preserved.

# R6 - DKB downstream NDSR/output ownership correction

## R6 file table

| Action | Absolute path | Purpose | Pre-existing state |
|---|---|---|---|
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | Correct demonstrated NDSR input/output descriptor or write sequence defect | Tracked and heavily dirty before R6; R2 Omap retained, R3 reverted |
| Conditional update/revert | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | Correct demonstrated storage-image ownership/generation/alias defect; retain or revert R5 revival from production proof | Tracked and heavily dirty before R6 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\tables.rs` | Correct demonstrated sampled/storage descriptor association or stale-view defect | Tracked and heavily dirty before R6 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\upscale.rs` | Correct demonstrated NDSR dispatch/input/output association defect | Tracked and heavily dirty before R6 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\queue.rs` | Correct demonstrated queue barrier or ownership ordering defect | Tracked and heavily dirty before R6 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shader.rs` | Correct demonstrated reflection resource-type defect | Tracked and heavily dirty before R6 |
| Conditional update | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\gmem.rs` | Correct proven raw-pointer association only | Tracked and heavily dirty before R6 |
| New | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R6-DKB\` | Existing-evidence analysis, focused regressions, refreshed shader library, isolated candidate, gate and approved state | Did not exist before R6; R5 and earlier evidence read-only |

## Built

- Traced the existing R5 frame without adding diagnostics. Pass 27's meaningful 1440x810 scene attachment is owner 53467, creation 533, Vulkan format 37. In the terminal frame sequence, pass 1042 retains that attachment while producing owner 61281; pass 1044 samples 61281 and writes owner 61274; pass 1045 samples owner 61274 and writes owner 61277; pass 1046 samples owners 61277 and 61281 and writes owner 53355, creation 560, `B10G11R11_UFLOAT_PACK32` 1440x810. NDSR pass 1047 samples that exact owner 53355 and binds owner 4856, creation 561, `B10G11R11_UFLOAT_PACK32` 2880x1620 as its real storage output. Passes 1047-1059 retain owner 4856 for HUD composition; final pass 1062 samples owner 4856 and writes present texture serial 6, owner/creation 6, format 37. No stale/dummy view, wrong backing association, ownership handoff, or later storage upload occurs in this chain.
- Identified the first broken invariant as execution content rather than resource association: translated tensor-network shader `14a8aaf3e0a9edbd` runs with correct descriptors and ordering but leaves the real NDSR output black. Reflection declares a type-3 combined sampled image, type-3 formatless storage image, read-only storage buffer at c0+204, compute push bank c24, local size 128x1x1; the recorded dispatch is 60x27x1 groups, input 1440x810 format 0x3f and output 2880x1620 format 0x3f. None of the zero-address scratch-warning shader keys is this chain.
- Added an exact key-gated terminal NDSR host path in `vk/exec.rs`. It resolves the first reflected sampled texture and storage image from the same c2 handle words used by production descriptors, materializes/prepares their real images, requires 2D single-sample `B10G11R11_UFLOAT_PACK32` with transfer capabilities, records full barrier -> scaled blit -> full barrier in replay order, and marks the destination GPU-owned. Final composition therefore samples the newly written owner 4856; submission-start CPU write-watch processing cannot upload stale guest storage over it.
- This agrees with current MIT nxnative/Turbo behavior in `Switch2RECOMP-main/runtime/nvn/vk/vk_replay.c`: translated NDSR tensor kernels are not trusted; the terminal network operation is replaced with an ordered image blit, preserving the scene while omitting learned anti-aliasing/upscale correction. Its image lifecycle also upgrades images registered without `IMAGE`, retains real storage output ownership, barriers before/after the replacement, and marks destination content current. R5 logs prove nxrs had already upgraded serial 4856 at 93.15 s and bound a real storage view, so no duplicate descriptor/usage fix was added.
- Reverted the R5 parked-serial revival and its test. The R5 production log contains no `revived from parked` event, the mismatch moved from ID 62272 to 62080, and the world remained black. Reviving an object after release solely because a stale table slot names its serial was not source-supported and could change unrelated lifetime behavior.
- Rebuilt all 22,854 DKB shaders from scratch into the isolated R6 stage and relinked release front. `nxfront.lib` SHA-256 `3aae25b0a41552ebfbc4d63157d0e596b4690576db425259721e10288e8fa961`; R6 DKB DLL `65443476285584fc91e53a8f29e8776d86950cb91383290229a7c805837a6aa0`.

## Tests written

- Added `vk::exec::ndsr_output_tests::dkb_terminal_ndsr_copies_exact_sampled_input_to_storage_output`. It reproduces the demonstrated 1440x810 sampled `B10G11R11` input and 2880x1620 storage/transfer destination, accepts only the exact DKB key and ordered-write capabilities, and rejects an unrelated key or a destination that cannot receive the write.
- Descriptor-set and descriptor-buffer backends are not separate branches of this correction: the substitution deliberately occurs before `bind_sets`/descriptor emission and resolves the reflected handles directly from the common driver bank. The same key/handle/image invariant therefore gates both backends; no backend-specific descriptor rewrite exists to test separately.

## Gates

- Focused NDSR regression: 1 passed. Full graphics suite: 126 passed, 3 ignored, 0 failed; graphics doc tests: 2 passed. `cargo check -p graphics --all-targets --jobs 4`: passed with the pre-existing `spvfix.rs:593` unused-`mut` warning.
- Full shader rebuild: 22,854 found, 22,854 translated, 0 reused/failed/skipped/invalid, 22,854 valid, 0 scan problems, 244.4 seconds. This validates affected shader `14a8aaf3e0a9edbd` and every other output with `spirv-val`; manifest SHA-256 `bfae4116b1e274099bafd5d4bb08b96ed1969a7e7805c1bc98b344e17aa58231`.
- Release front build, cached-object relink, candidate manifest verification and R6 describe passed. Receipt-gate failure-injection suite: 17 passed. Scoped `git diff --check` passed apart from Git's line-ending notices.
- RED, unchanged workspace debt: `cargo clippy -p graphics --all-targets --jobs 4 -- -D warnings` stops in dependency crates on the existing `config/options.rs:225` warning and 35 existing `translator` findings. No out-of-lease files were edited to hide them.

## Evidence

- R5 result: exit 0, no device loss, save cleanup true, black world/HUD screenshots, `dropped_work=23`, `missing_unique=0`. Moving target mismatch ID 62080 disproves the serial-specific R5 world hypothesis.
- `R5-DKB/run/run.log:22682-22697` proves NDSR input/output identity and dimensions; lines 22830-22847 prove owner 4856 survives HUD work, is sampled by final shader `e9c6f339b7550c71`, and writes present serial 6. Earlier line 1935 proves serial 4856 was upgraded for storage despite flags 0; no zero stand-in was bound.
- `R5-DKB/stage/dkb_data/shaders/spv/14a8aaf3e0a9edbd.json` proves local size, resource kinds/banks and formatless storage-write translation. `Switch2RECOMP-main/runtime/nvn/vk/vk_replay.c:1907-1946` is the MIT reference for host NDSR substitution; `vk_tex.c:71-101,947-957` is the reference storage-image lifecycle.
- Full shader log: `build\DELEGATION\R6-DKB\full-shader-rebuild.log`, SHA-256 `20a5e7ab2a1443d5fecc3851a1aceb008e7f727ff6968186fde052fd07c40a4b`.
- Exact approved state: `D:\Projects\RecompLauncherEngine\build\DELEGATION\R6-DKB\approved-state.json`, SHA-256 `6bacb0b6b233eb7e597640ab18b413cd6a9b2887ac1fe79d9ec3113cae8c1c2d`. Candidate manifest SHA-256 `03385aa06437030671b5c2e6d176efb882067928ac86fda550ece3e7dd9e3f72`; link manifest `104c87ba428aed31974bd6e4be3aa4c83add41c967199fdcec386cca52775a15`.
- Stop files remain byte-identical to the R6 pre-work archives: shared `acb254b56369538c81cdd4b7c1b40fcc3e63c7bb6368163423a1711073d446bc`, pending `3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7`, HARNESS3 `9447653d8d319644fff34614756ca0a3e0d33ce64b3b3e01ebfd8b3c18be4230`.

## Unproven

- The visible R6 game result is unproven pending a fresh lead receipt. CPU evidence proves exact resource selection, ordered replacement recording and ownership transition, but only the bounded run can establish a visible world.
- The host blit preserves the terminal input picture and scale but intentionally omits the NDSR network's learned reconstruction, matching the reference fallback rather than claiming tensor-network parity.

## Not built

- No game/GPU workload was run, no R6 receipt was created or consumed, no stop file was cleared, and no blacklist/cache was removed.
- No generic moving-ID target coercion, zero-scratch workaround, descriptor rewrite, table-generation change, queue-wide ownership change, or raw-pointer change was built. `tables.rs`, `upscale.rs`, `queue.rs`, `shader.rs`, and `gmem.rs` were not edited because existing evidence proved their association in this chain.

## Slips

- None. The previously disclosed consumed-R4 stage mutation remains historical; R5 and earlier artifacts were read only during R6.

## Questions

- Lead action: review this report and create `R6-DKB\lead-authorization.json` only if authorizing the one bounded attempt. Required schema: `nxrs.r6-dkb.authorization.v1`, role `lead`, explicit decision, one attempt, and `approved_state` exactly equal to the prepared JSON.
- Authorized command from `D:\Projects\RecompLauncherEngine`: `py build\DELEGATION\R6-DKB\prepare.py run --receipt build\DELEGATION\R6-DKB\lead-authorization.json`.

Compiler/build resources are released. No game, compiler, linker, shader worker, helper or claim remains active; saves, caches, blacklists and fault/stop history are preserved.
