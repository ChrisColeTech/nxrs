# R3-MKW - Mario Kart World mesh safety

## Built

Baseline HEAD: `7688e33ef5ef47e64322c7aed0f84c471e9c8b99`. The worktree has extensive
pre-existing modifications. This wave preserves unrelated work and does not stage, commit, reset, or revert it.

### File table

| Action | Absolute path | Purpose | Dirty state before R3-MKW |
|---|---|---|---|
| New | `D:\Projects\RecompLauncherEngine\docs\waves\R3-MKW.md` | Wave file table, audit, implementation, gates, and lead handoff | Absent |
| Update if audit proves defect | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | Mesh draw record, descriptor binding, and corrected execution invariant | Modified in baseline |
| Update if audit proves defect | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\queue.rs` | Submission ownership through the exact completion fence | Modified in baseline |
| Update if audit proves defect | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\gmem.rs` | Immutable physical/global-memory range snapshot | Modified in baseline |
| Update if audit proves defect | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\mem.rs` | Submission-owned device allocation or range lifetime | Modified in baseline |
| Update if compute variant required | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shader.rs` | Compute-compatible mesh shader loading/reflection | Modified in baseline |
| Update if compute variant required | `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\pipe.rs` | Explicit profile/capability-selected mesh compute pipelines | Modified in baseline |
| Update if compute variant required | `D:\Projects\RecompLauncherEngine\src\shaders\src\mesh.rs` | Bounded task/mesh compute variant generation | Modified in baseline |
| Update if compute variant required | `D:\Projects\RecompLauncherEngine\src\shaders\src\job.rs` | Translation job metadata for compute variants | Modified in baseline |
| Update if compute variant required | `D:\Projects\RecompLauncherEngine\src\shaders\src\link.rs` | Variant linkage and reflected interface | Modified in baseline |
| New if required | `D:\Projects\RecompLauncherEngine\src\graphics\tests\mkw_mesh_safety.rs` | Focused payload/range/lifetime and backend-selection regressions | Absent |
| Evidence/candidate | `D:\Projects\RecompLauncherEngine\build\DELEGATION\R3-MKW\` | Offline audit, gate outputs, hashes, candidate, and approved-state receipt gate | Absent; build output only |

No source path outside this table is writable for this wave. Reference trees, isolated game data, saves, caches,
blacklists, and retained fault evidence are read-only.

The offline audit established a concrete task-output bound mismatch, so this wave took required path 2 rather than
adding the larger mesh-as-compute backend. `src\shaders\src\mesh.rs` translated NVN2 task output word 1 directly to
`OpEmitMeshTasksEXT`, capped only at Vulkan's 65,535 groups. The MKW behavior reference allocates and compacts exactly
32 mesh records per task (`MESH_CS_PER_TASK`), and its task ABI documents those records inside the bounded task
payload. A corrupted or incorrectly produced count could therefore fan out thousands of native mesh groups, beyond
the payload represented by the task, before descriptor robustness could make their work safe.

The source correction caps every NVN2 task emission at 32 and bumps the shader identity from `mesh 3` to `mesh 4`,
forcing existing libraries to retranslate rather than retaining the unsafe wrapper. Native mesh remains available;
no title/device backend downgrade, draw skip, blacklist removal, descriptor-range widening, or fault-history change
was made. Direct/indirect draw-record handling and nonzero `first` semantics are unchanged.

The refreshed fault pair proves the generated invariant: task `6098504f5d12ea03` now performs
`OpULessThan want %uint_32`, selects `min(want, 32)`, then calls `OpEmitMeshTasksEXT`. Its 256-byte payload, shared
layout `[0,16,272,528]`, zero initialization and four control barriers remain. Mesh `1de24686294997d8` remains 32
threads, triangle output, 256 maximum vertices, 192 maximum primitives, three bounded primitive indices, and the
same descriptor/reflection interface. Neither refreshed module contains physical-storage-buffer addressing; both
use logical storage-buffer descriptors, so no gmem-generation correction was justified.

Prepared candidate: `D:\Projects\RecompLauncherEngine\build\DELEGATION\R3-MKW\candidate`. It is a plain 6.332 GiB
copy of the isolated HARNESS3 MKW state (23,246 files) with saves, ASTC cache, pipeline cache, pipeline keys and
`pipeline-crashed.bin` retained. All 11,609 shaders were freshly translated with refreshed translator executable
SHA-256 `7a28092f4702c5b067e351cbe105ac4b23aed4b9bb1ce5bd09ccfacfb17dc30e`: zero reused,
zero failed/skipped/invalid, 11,609 validated. The original 396 cached objects (283,030,320 bytes, aggregate identity
`b633fbac0e182be4ec2de8aead4a84c6c2493bde2c2fbd622cd76de82fa43209`) were relinked against the release library.

## Tests written

`src\shaders\src\mesh.rs::mesh::tests::task_launch_limit_matches_nvn2_record_capacity` pins the 32-record bound and
the required `mesh 4` cache identity. Before correction the production constant and identity were 65,535 / `mesh 3`.

Offline product regression under `build\DELEGATION\R3-MKW\focused` regenerated exactly the retained task/mesh keys.
Disassembly confirms the task's emitted count uses 32 while payload initialization/barriers remain; the mesh limits,
primitive indices and reflection remain stable. Both pass `spirv-val --target-env vulkan1.3`.

## Gates

All Cargo commands used cwd `D:\Projects\RecompLauncherEngine`, exclusive
`CARGO_TARGET_DIR=D:\Projects\RecompLauncherEngine\target\MKW-R3`, and at most `--jobs 4`.

| Command | Result |
|---|---|
| `cargo test -p shaders task_launch_limit_matches_nvn2_record_capacity --jobs 4` | PASS, 1 focused test |
| `cargo test -p shaders --jobs 4` | PASS, 73 passed / 1 ignored plus 2 corpus tests |
| `cargo test -p graphics --lib --jobs 4` | PASS, baseline preserved: 126 passed / 3 ignored |
| `cargo check -p shaders --all-targets --jobs 4` | PASS |
| `cargo check -p graphics --all-targets --jobs 4` | PASS; existing `spvfix.rs:593 unused_mut` warning |
| `cargo clippy -p shaders --all-targets --jobs 4 -- -D warnings` | BLOCKED by 35 existing translator findings |
| `cargo clippy -p shaders --all-targets --no-deps --jobs 4 -- -D warnings` | BLOCKED by 57 existing shader-package findings |
| release build `-p nxrs-cli -p front -p runtime --jobs 4` | PASS; existing graphics `unused_mut` and OS deprecated `fetch_update` warnings |
| focused/full shader translation and `spirv-val` | PASS: 2/2 focused and 11,609/11,609 full |
| `git diff --check -- src/shaders/src/mesh.rs docs/waves/R3-MKW.md` | PASS; LF/CRLF advisory only |

Targeted `rustfmt --check` reports broad pre-existing formatting differences throughout `mesh.rs`; no blanket format
was applied to the dirty file. No synthetic GPU test ran: it was not needed to establish the generated bound and
would not prove survival of the retained real-title workload. No game ran.

## Evidence

Retained baseline `build\HARNESS3\20261121-135645-mkw-baseline` exits 71 after 33.3 seconds. Checkpoint 94396 is a
direct 1x1x1 mesh draw using task `6098504f5d12ea03` and mesh `1de24686294997d8`; the invalid read is
`0xd434b0000`. The address is inside an 8 MiB staging allocation unbound 2.34 seconds earlier and is owned by zero
recorded replay-held ranges at fault time. Robustness2 is enabled but cannot guard physical-address loads.

DG/K3 history was also audited. The old `stale scratch` lines are not exact-draw proof because address names survive
Vulkan-handle reuse; the faulting submission's task/mesh/fragment descriptor sets were fresh. The refreshed pair has
no physical/global loads, and pool/gmem snapshots are submission-owned and fence-retired. This ruled out a supported
descriptor/gmem lifetime correction and isolated the over-wide native task fanout as the demonstrated invariant.

Candidate identities:

| Resource | SHA-256 |
|---|---|
| `target\MKW-R3\release\nxfront.lib` | `6fd153b849037432445f4c45fbb0e20134205f0f3b3638a7bdf53ae43da949f7` |
| candidate `mkw_code.dll` | `a653adca03b055b2a26a000a8c6d9504efa0b72413bc6ae626fa99a2026b0f1d` |
| full shader manifest | `ff1c677e8cc20547a3f044122357b093bc68f391b67cd6432da88448848b4c76` |
| corrected task SPIR-V | `345320d437b6c243fc95be9adadffbd34211b301f316517c44ccb54a58dbb0ca` |
| refreshed mesh SPIR-V | `6ed0964a9e14e30d51785486d3d00941cef82a2c66e2f1d05ff6616fe30a1111` |
| retained crash blacklist | `246f1e93a74d2c49a461af0de50819bc127bd27e7d5a19eb4404064bb09cc8cc` |
| starting save | `3d519bfae388e560f8b02d64bca378ab898fc89b7f731a14e864f355fc1e1539` |
| approved state | `41dc0227588eb4248ee6bf7c64f5dff97ae482555c322adc961518738aff4f0f` |

Receipt gate: `build\DELEGATION\R3-MKW\launch.py` validates every approved file and all 396 link objects, requires an
exact lead-authored `nxrs.r3-mkw.authorization.v1` receipt, atomically consumes one attempt, preserves/restores the
shared stop/pending history, uses the campaign driver in-process with save restoration, and refuses retries. The
placeholder template was exercised CPU-only and failed before creating `attempt` or `run`; all three latch hashes
remained exact.

## Unproven

The real 35-second result is deliberately unrun pending the lead receipt. Until that run survives, the source defect
is corrected and candidate-ready but causal closure of checkpoint 94396 is unproven. The driver's exact faulting
instruction remains unavailable. A failure with the bounded task count must preserve evidence and proceed to the
independently written mesh-as-compute backend; it does not authorize a retry, draw skip, or blacklist clearing.

Haze/readability, 6-7 FPS/pool cost, missing shaders, audio, and a full race are R6/performance work and are not claimed
fixed by this safety candidate.

## Not built

No mesh-as-compute backend was built because the audit found the concrete output-count invariant first. No synthetic
GPU, game, screenshot review, haze correction, performance correction, commit, staging, or owner-data access occurred.

Lead action: author `D:\Projects\RecompLauncherEngine\build\DELEGATION\R3-MKW\lead-receipt.json` from the template,
with `approved: true`, explicit author/decision, and `approved_state` equal to the exact parsed content of
`approved-state.json`, then run from the project root:

```powershell
$env:PYTHONDONTWRITEBYTECODE = '1'
py build\DELEGATION\R3-MKW\launch.py --receipt build\DELEGATION\R3-MKW\lead-receipt.json
```

The first review criterion is only: 35 seconds without invalid read/device reset, with save/cache/blacklist and stop
history audits intact. Do not infer haze, race readability, or pacing from that result.

## Slips

The first focused offline scan had a 120-second shell timeout before translation; no process survived and the same
CPU-only scan completed once under a 10-minute watchdog. No GPU/game retry or lease overlap occurred.

## Questions

None. Recommendation after the lead run: accept only the bounded safety milestone on a clean survivor; on any loss,
retain the one consumed attempt and implement the roadmap's generic mesh-as-compute path before another MKW launch.

All compiler/translator processes are released. No game, helper, claim, `attempt`, or `run` process/artifact exists.
