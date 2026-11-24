# Audit-03 — Independent DG-02 diagnostic review

## Built

### File table first

| Action | Absolute path | Purpose / pre-existing state |
|---|---|---|
| New | `D:\Projects\RecompLauncherEngine\docs\waves\Audit-03.md` | Sole writable file; independent findings and disposition. Confirmed absent before creation. |

**Disposition: return A03-1 (P2) to the original DG-02 agent. The diagnostic is not yet technically ready for
the proposed bounded discriminator.** One introduced diagnostic-correctness blocker was identified; no introduced
P0/P1 copy/lifetime/budget defect was found in the reviewed paths. This disposition concerns the fidelity of the
evidence, not a demonstrated DKB rendering defect or permission to run a GPU workload.

Review date: 2026-11-21. Session ID unavailable. All source, tests, artifacts and gate/latch files remained read-only.
Every shell command used explicit cwd `D:\Projects\RecompLauncherEngine`. HEAD independently confirmed as
`7688e33ef5ef47e64322c7aed0f84c471e9c8b99`; extensive dirty work preserved.

### A03-1 / P2 — Separate backing format from actual view format, or reject format reinterpretation

**Introduced sites:** `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:611-649`, particularly
629-635 and 647; `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs:31-40,117-126`.
The resolver recognizes a view handle, but its predicate checks only the image shape and view's base mip/layer.
It does not inspect the view's effective format. It then uses **`im.format`**, not the backing owner's format,
as both the raw-format label and association key. Those values are not interchangeable in this engine.

**Source counterexample 1 (same backing falsely fails the join / raw-format field changes by alias):**

1. Let owner serial 7 hold a mutable, single-sample, single-level/layer 1440x810
   `R8G8B8A8_UNORM` image H. Let serial 8 alias H as `B8G8R8A8_UNORM`, with its own valid attachment view V8.
   This is a production-supported state: `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:1155-1174`
   permits same-size uncompressed colour formats, and 1186-1211 sets `image=o.image`, `format=tf.vk`,
   `alias_of=Some(owner)`. Mutable colour-image creation is at 891-898.
2. A selected pass ending through serial 8 is accepted by every condition at `exec.rs:623-635`.
   It yields owner 7, image H, bpp 4, **format 44 (BGRA8)**. Its raw filename also receives format 44 at
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:701-710`; the copy at 690-695 names H,
   whose creation format is **37 (RGBA8)**. No view conversion is requested by that image-to-buffer command.
3. With no intervening write, sampling H through serial 7 yields the same owner/image/aspect/extent/write sequence
   but **format 37**. `Resource::same_image` rejects the match at `render_chain.rs:40`; the consumer is `Unknown`.
   Thus the field documented as the raw image format changes with the registered alias, despite identical backing.
   The bytes may be intentionally interpreted through a BGRA view, but the record does not distinguish that
   interpretation from the physical image format. This is not evidence that the copy overruns its buffer.

**Source counterexample 2 (different actual view interpretation disappears):** on that same owner image, put a
valid `VW_FORMAT` BGRA8 view and a default RGBA8 view in `im.views`. `image.rs:1239-1253,1268-1275,1295,1389-1409`
uses the override when creating the real Vulkan view. Both pass `exec.rs:629-635`, yet both new records report
format 37 and can return `Link::Observation` with no effective-view-format field. A further production case is
the sRGB-to-UNORM storage-view fallback at `image.rs:1337-1349`: the actual storage view differs from `im.format`
even without a registered format override. `exec.rs:2740-2765` can also substitute the shader's storage format.
The older binding text at `exec.rs:3485-3511` reads the **registered** view, so it does not reliably recover those
effective storage-view changes. Keeping the numeric view handle alone does not record their interpretation.

**Impact / required correction:** the new actual-view discriminator can lose the very alias edge it is meant to
explain, and cannot distinguish physical raw bytes from attachment/sample/storage interpretation. This is a
blocking evidence-integrity defect for the requested format-aware diagnostic, not a claim of GPU corruption or
of the cause of DKB's black world. Preserve a canonical backing format plus the resolved view interpretation
(including relevant subresource/aspect), or explicitly refuse cases not verified by this deliberately narrow
diagnostic. Same-image association may legitimately cross view formats, but it must retain the difference and
must not present an alias's format as the canonical backing format. Do not expand rendering behavior to fix it.

### Reviewed controls and integration — no additional introduced blocker found

- **Actual handles / fallbacks:** attachment handles are copied from the actual `RenderingAttachmentInfo` array
  and depth tuple at `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:2207-2214`. Consumer
  observation follows `tex_view` at 3423-3443; that same handle goes into the storage/sampled descriptor at
  3526-3545. Stand-ins absent from `im.views`, null handles, unresolved owners, or different current backings cannot
  silently become a known registered image. Alias traversal is finite (603-610); an unfinished chain is rejected
  by 624. Format interpretation remains the exception in A03-1.
- **Subresources / extents:** 625-634 restrict the image to 2D, one mip, one layer, one sample, and base mip/layer
  zero. Copies therefore use the full single subresource; full image extent is correct for raw pixels even when
  the rendering area is smaller. Image views are created/clamped in
  `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:1280-1336,1394-1400`. This does not certify
  invalid Vulkan views that an underlying driver might accept. Multimip/layer/MSAA images are intentionally
  unsupported, including a single selected subview of a larger image; no request to broaden that support.
- **Raw bytes / depth:** the explicit map at `exec.rs:611-622` retains all bytes of its supported formats: R8 (1),
  R16 float/RG8 (2), R32 float/RGBA8/BGRA8/packed B10G11R11 (4), and D32 float (4, DEPTH aspect). The copy has
  default zero pitches/offsets/mip/base layer and layer count one (`exec.rs:675-685`). There is no PNG conversion,
  HDR clipping, alpha removal or stencil-plane ambiguity on these supported paths. Other formats have bpp/aspect
  zero and cannot reserve a raw allocation; this does not certify arbitrary HDR/depth formats.
- **Outside-pass copy / synchronization:** `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:1734-1744`
  ends rendering before `passdump::pass_end`. Copies are bracketed by full barriers at 686 and 698, in GENERAL
  layout. `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\queue.rs:472-483` supplies the all-command
  write-to-read/write memory dependency; 677-685 adds the submission's host-read dependency. Images are created
  with `TRANSFER_SRC | TRANSFER_DST` at `image.rs:855-917`; same-image aliases inherit usage at 1197. This path
  relies on successful existing image creation/capability validation, not on a new diagnostic usage upgrade.
- **Lifetime:** `exec.rs:699-730` puts each returned buffer in that exact `Sub::transient` and attaches its callback
  to the same Sub. `queue.rs:803-821` requires timeline completion; 855-887 runs callbacks before transient frees
  at 897-899. The callback has no guest destination and reads only its retained mapped buffer. Output failure is
  logged; allocation failure installs neither copy nor callback. Device loss/noncompletion does not manufacture
  a successful raw file. This preserves the accepted DV-01 paths rather than using their synchronous API.
- **Bounds before allocation:** `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs:6-9,43-50,87-115,177-197`
  enforces checked payload multiplication/addition, 16 reserved attempts and 64 MiB aggregate requested payload,
  one admitted nonzero frame, 16,384 events and 256 FIFO observations. `exec.rs:664-673` reserves before calling
  the allocator. Failed allocations are not refunded; another frame cannot reset the counters while callbacks
  remain pending. These are requested-payload limits, not a promise of 64 MiB total process/allocator footprint.
- **First-frame lock:** the first admission latches a frame, but production callers cannot admit a boot frame
  before the dump trigger: `exec.rs:906-924` checks active dump frame before producers; 3068 and 3426 gate consumers
  on `passdump::active()` (767-770). The trigger sets FRAME only when due (775-793). Starting with the chain env
  enabled does not itself arm frame 1. The first eligible active dump wins; a deliberately earlier dump trigger
  would consume that opportunity. The proposed run must have exactly its intended one-frame trigger.
- **Default off / caches:** `render_chain.rs:136-169` caches opt-in configuration; without `NXGFX_RENDER_CHAIN=1`
  there is no ledger allocation, image lookup or raw-copy work from this feature. Added persistent QState payload
  is nine view handles (`exec.rs:57,454`), with small conditional checks. Active pass dumps disable the outer set
  cache (177-192) and texture-part cache (3068-3092); lower descriptor memoization happens after observation
  (3616-3618). A memoized `tex_view` still supplies the actual handle. No benchmark was performed.
- **Content association:** `render_chain.rs:117-126` returns the latest retained matching attachment observation;
  a write-sequence difference gives `ChangedSince`, not a current-pixel claim. For an ordinary attachment -> compute
  storage overwrite -> sampled-consumer sequence, `exec.rs:5104` records the old capture before storage ownership
  is marked at 5125-5126/2612-2615; `image.rs:2365-2388` advances the owner/group sequence. The later sample cannot
  get an unchanged observation for that old raw capture. Storage binding itself may conservatively report
  `ChangedSince` before an actual store; it is not a completed-write node. This is correct uncertainty, not
  evidence of a bad consumer. No concrete wrong-capture content-sequence counterexample was found in this path.
- **Partial graph / unknowns:** missing metadata, evictions, unsupported formats/views, budget refusal and incomplete
  files remain unknown. Uploads/copies/compute/pool producers are not all modeled, and equal `gpu_seq` does not
  prove an absence of untracked writes. `Recorded` is only scheduled; the separate completion/write-success log
  is necessary. This is an attachment-observation graph, not full last-writer causality.

### Bounded DKB proposal

The proposal in `D:\Projects\RecompLauncherEngine\docs\waves\DG-02.md:194-206` is a useful discriminator after
A03-1 is corrected and CPU-reviewed. Its required controls are `NXGFX_RENDER_CHAIN=1`,
`NXGFX_CHAIN_EXTENTS=1440x810,2880x1620`, `NXGFX_CHAIN_PASSES=18,27`, **`NXGFX_DUMP_PNG=0`**, and
**`NXGFX_DUMP_TEX` unset**, with one gameplay-frame trigger. The latter two controls explicitly disable independent
legacy readback allocations (`exec.rs:807-846,952-977`); the new budget does not bound those older paths.
Historical pass IDs are only candidates. Current stage/attachment metadata must establish their meaning, and
the selected depth must actually be supported D32. Missing captures do not justify an automatic retry.

Even corrected raw depth/colour observations would only narrow the geometry/coverage versus material/attachment
boundary. They would not establish a causal game fix, correct rendering, or input-responsive gameplay.

## Tests written

**None written, compiled or executed by Audit-03.** The lead-supplied full graphics result remains **119 passed /
3 ignored**, including the three DG-02 CPU tests. Prior DV-01 supplied acceptance remains runtime **45 + 1 doc**
and graphics **2 readback docs**, with Audit-02's accepted scope; nothing was rerun here.

The three existing tests at `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs:254-386`
exercise the real ledger, checked reservation, association and FIFO helpers. They do not exercise `chain_resource`:
their `Resource` values are already constructed, so they cannot catch A03-1. The first-frame test also exercises
the ledger rather than the caller's trigger gate; that gate was reviewed directly above.

**Exact CPU regression proposal for A03-1:** factor the production resource-resolution/eligibility decision into
a CPU-callable helper used by `chain_resource` (no second test-only reimplementation), and add
`chain_resource_preserves_backing_and_effective_view_formats` in the implementing agent's leased source:

1. Construct the owner-7/RGBA8, alias-8/BGRA8, same-H, 1440x810, one-level/layer/sample witness above using dummy
   numeric Vulkan handles and no device. Resolve the attachment alias and owner sampled view through that helper.
   If supported, assert both canonical raw formats are RGBA8, each effective view format is preserved, and the
   same-frame/same-write association identifies the same backing without asserting identical interpretation.
   If the narrow implementation declines reinterpretation, assert an explicit unsupported result, zero reserved
   bytes and no copy request for the alias instead. The current implementation accepts it with a BGRA8 raw label.
2. On one owner record, compare default RGBA8 and overridden BGRA8 views. Assert the resolved view difference is
   retained or explicitly refused; handle membership alone must not establish format equivalence. Include the
   effective UNORM storage view of an sRGB backing, using the production resolved-view metadata path.
3. Positive controls: two different same-format views of one backing, and a same-format alias, must still associate;
   a changed backing and a dummy view must be unknown. Keep the existing changed-write-sequence negative control.
   Supported canonical D32 must still request exactly `width * height * 4` bytes of DEPTH, while an unsupported
   format requests no allocation. This protects the useful discriminator while closing the actual resolver gap.

Lead owns execution after the original agent's correction. No new broad-test run is warranted on this unchanged
candidate merely because an audit was performed.

## Gates

Audit commands, all with cwd `D:\Projects\RecompLauncherEngine`; no target directory or compiler/GPU slot used:

- `git rev-parse HEAD` and `git status --short`: confirmed HEAD and preserved dirty baseline.
- `git hash-object --no-filters -- src/graphics/src/vk/exec.rs src/graphics/src/vk/render_chain.rs src/graphics/src/vk/image.rs src/graphics/src/vk/queue.rs src/graphics/src/vk/tables.rs build/GPU-STOP.json build/HARNESS3/GPU-STOP.json`:
  identities below; dependencies/latches match the retained manifests.
- `git diff ecb2590bd1f446ca429a53881b8e212dada074fc -- src/graphics/src/vk/exec.rs`:
  failed with `fatal: bad object`. DG-02 recorded a raw pre-wave hash, but that blob is not available as a Git object.
  No files were written by this attempt. Its evidence directory contains no separate implementation/baseline diff.
- `git diff --unified=2 -- src/graphics/src/vk/exec.rs`: inspected diagnostic hunks and integration against current
  source. This is the extensive **HEAD-to-worktree** diff, not an exact DG-02-only delta; unrelated old hunks were
  not attributed to DG-02. The new module and clearly delimited chain helpers/call sites anchor this finding.
- Report-only `git diff --no-index --check -- NUL docs/waves/Audit-03.md`: no whitespace diagnostics;
  exit 1 for the new-file comparison, with the configured LF/CRLF warning. Exit captured from `$LASTEXITCODE`.
- Final repeat of the seven-file raw-hash command: all source/dependency/latch identities unchanged.
  `git status --short -- docs/waves/Audit-03.md` reports only this new report in the requested report scope.

Retained agent gates reviewed, **not executed by this audit**: `cargo check -p graphics --all-targets --jobs 4`,
`cargo clippy -p graphics --lib --jobs 4`, and `cargo test -p graphics --lib --no-run --jobs 4`. The logs show
completed profiles; the no-run log shows a linked test executable, not test execution. Check/no-run retain the
existing `spvfix.rs:593` unused-mut warning. Clippy reports graphics 99 (1 duplicate), shaders 57, translator 35,
runtime 7, config 1. No diagnostic-module warning was identified. These baseline warnings are not send-backs.

No Cargo, tests, formatter, GPU, replay, game, release/relink, agents, staging or commit command was executed.

## Evidence

Read all six required briefing documents and the source/dependency sites cited above. Evidence root:
`D:\Projects\RecompLauncherEngine\build\DELEGATION\DG-02\`.

| Viewed artifact | Establishes / limitation |
|---|---|
| `result.json:7-27`, `preserved-identities.log:1-6` | Implementer's diagnostic/budget claims, no causal fix, and retained dependency/latch identities; independently matched below. |
| `check.log:1-14`, `test-no-run.log:1-14`, `clippy.log` completion/summary lines | Compile/lint completion with existing warnings. These are not independent audit or lead test transcripts. |
| `inspect_retained.py:10-30,34-57`, selected `retained-inventory.json` records | Sequential size/pixel-capped converted RGB analysis; no renderer invocation. Scripts were read, not run. |
| `stage_facts.py:19-56`, `stage-facts.json` selected records and 875-1102 | Historical converted-RGB comparisons, logged logical serial joins, shader keys and candidate pass/attachment shapes. They do not contain current raw HDR/depth or effective-view identity. |
| `process-release.log:1` | Historical implementer process-release evidence; not a fresh global process scan. |

The retained summary records 425 + 14 PNGs, but this audit did not decode all 439 images or repeat the previous
game-cause investigation. No fresh visual/GPU evidence was produced or certified. The source counterexamples
above, rather than an interpretation of old black screenshots, support A03-1.

Current raw Git blob identities (not SHA-256):

| Absolute path | Identity |
|---|---|
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | `1c8a464715e2308a46d32775755aa0a79147faa7` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs` | `0da35f4df8b9508831cf9c7098b64a016b3e5897` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | `f2616c2a5ac463c0feb5674898f19b9479239d3f` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\queue.rs` | `c98c74145c85e25e1435efd4cc53b171a73f7373` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\tables.rs` | `d7d0b9952988ba35a0ec6e400deab42fffcb67e1` |
| `D:\Projects\RecompLauncherEngine\build\GPU-STOP.json` | `b123fe50e51f58a97d4d89f4214a1163081d194a` |
| `D:\Projects\RecompLauncherEngine\build\HARNESS3\GPU-STOP.json` | `058e8120c9d28fc1026ba162b57eab4c766d2895` |

Supplied historical release identity, not remeasured:
`D:\Projects\RecompLauncherEngine\target\OOTWAIT\release\nxfront.lib`, SHA-256
`6827ce497d42a30b199ae82a659666330ca9390dd03afd8977ba28f43ba83895`; build time **unrecorded**.
There is no supplied current release/GPU build containing DG-02. That library does not certify this source.

## Unproven

Actual Vulkan execution/capabilities on the intended device, mapped-memory visibility, final pixels, completion
under loss, and concurrent image lifecycle are not hardware-tested here. Successful source lifetime review is
not GPU acceptance. Existing allocator-internal failure cleanup and other Audit-02 baseline debt were not
reclassified as introduced DG-02 defects.

Current first incorrect producer/consumer, geometry coverage, raw depth, HDR colour and a causal DKB game fix
remain unproven. Historical pass 18/27 meanings and current depth format must be observed, not assumed. The
partial graph cannot certify all writers or shader execution; lack of a chain record is not a pass.

**Both GPU-stop latches remain active and untouched.** Technical readiness, even after A03-1 closes, does not
lift them. Any future run still requires the lead's explicit bounded resumption decision, run/build provenance,
deadline and fault-stop ownership under the protocol.

## Not built

No production fixes, tests, new artifacts, shared-plan changes, release build, replay or GPU run were made.
Return **A03-1** to the original DG-02 agent for a narrow diagnostic correction plus the production-helper CPU
regression above. Lead owns the correction's acceptance, fresh review of the changed resolver and ledger update.
Do not reopen the retained game-cause investigation as a substitute for fixing diagnostic evidence identity.

## Slips

None.

## Questions

None requiring an owner decision. The evidence-format correction is owed implementation; GPU resumption is a
separate lead-recorded gate, not an implied question or authorization in this report.

**Resources released:** report work complete; no compiler/GPU lease acquired and no worker, subagent, helper,
game or background process launched. All synchronous Git commands have returned; no owned process or resource
claim remains. The sole writable deliverable is this report.

## A03-1 recheck — 2026-11-21

### Built

| Action | Absolute path | Purpose / pre-existing state |
|---|---|---|
| Append | `D:\Projects\RecompLauncherEngine\docs\waves\Audit-03.md` | Sole writable file; preserve original findings and append the correction recheck. |

**Recheck disposition: A03-1 remains OPEN / P2, narrowed to backing-format provenance across alias-owner
handoff.** The original fixed-owner examples and effective-view metadata defect are corrected. One concrete
lifecycle counterexample still produces a false raw-format label; therefore the proposed format-aware bounded
discriminator is not yet technically ready. No additional blocking ID is raised for the final-view cache.

Session recorded by the current ledger: `ses_dffd66cafffeXYIGYnCp0WM0HO`. Source, tests, artifacts and both
GPU-stop latches remained read-only. Shell cwd: `D:\Projects\RecompLauncherEngine`.
Compiler/test/GPU/agent leases: NONE. The original report above is preserved as historical review evidence.

#### A03-1 remaining counterexample — the current owner is not necessarily the image's creation owner

Diagnostic sites: `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:603-621` and
`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs:88-98,128-174,60-71`.
`chain_resource` now follows `alias_of` correctly, but `ImageInfo::of(owner)` still gets its supposedly canonical
creation format from `owner.format`. Existing ownership transfer can promote an alias without changing that
alias's format or recreating the Vulkan image.

**Concrete source trace, no concurrency or GPU failure needed:**

1. Create owner 7 with a single-sample, single-level/layer 1440x810 RGBA8 image H (creation format 37), and alias 8
   over H with BGRA8 registered format 44 and a successfully created BGRA8 view V. This state is allowed by
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:1307-1327`; the alias stores the same H but
   its own `format=tf.vk` at 1339-1364. Keep V's final-create evidence retained; no eviction is involved.
2. Finalize owner 7 with alias 8 as its sole heir. `image.rs:1688-1695` removes the original owner.
   `image.rs:1704-1733`, specifically 1718-1720, changes the heir's `alias_of` to None and transfers the allocation.
   It does **not** copy/preserve a distinct backing-creation format, change H, or replace the heir's own view V.
   Only the removed owner's views are drained and deferred for destruction at 1725-1731.
3. The next selected attachment observation through serial 8 finds `alias_of=None` immediately at
   `exec.rs:606-613`. Both `ImageInfo::of(owner)` and `ImageInfo::of(im)` now carry **44**, although H was created
   with **37**. Current view membership and final-create evidence for V still match H/V at `image.rs:115-124`.
4. `render_chain.rs:138-153` accepts this full 2D image and BGRA8 view, with bpp 4 and COLOR aspect.
   `raw_format` becomes **44** at 161. `exec.rs:661-672` copies the unchanged H and writes a **`-fmt44.bin`** name.
   No Vulkan operation converted or recreated H as format 44. The current alias format has again been mistaken
   for the image's creation format, now after a normal lifetime transition rather than during alias traversal.

The remaining defect is the new diagnostic's provenance assumption, not a request to change the pre-existing
rendering ownership policy. It is the same A03-1 evidence-integrity problem; no claim of buffer overrun, wrong
copy size, DKB occurrence or game causality follows. Changing `owner` serial may conservatively break a historical
join; that separate fail-closed result does not make the newly mislabeled raw capture correct.

**Required narrow correction:** retain actual backing-creation provenance independently of a texture object's
registered/view format across ownership handoff and subsequent reuse, or explicitly make unverified provenance
unknown with no raw reservation. Do not overwrite the heir's rendering/view format just to fix the diagnostic.
The final-view cache is evidence of **view** creation and cannot establish the original image-creation format.

#### Corrected portions accepted by source review

- **Final successful create input:** `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:47-67`
  copies scalar values from `ImageViewCreateInfo`, not from `ViewKey`. Format selection, range/type adjustment,
  sRGB storage fallback and component mapping precede the actual create at 1430-1555. The error branch returns
  without recording; `note_view(view, &ci)` is after successful creation at 1562-1563. Neither the hook nor
  accessor changes the create arguments or queries capabilities to reconstruct an interpretation.
- **Cache hits / unknowns:** `image.rs:1427-1429` returns the renderer's existing handle; it does not fabricate
  metadata on a hit. `resolved_view` at 115-124 requires membership in that Image's current view map, then an
  exact image/view pair in the evidence cache. Missing, evicted or unrecorded data becomes None. The resolver at
  `render_chain.rs:136-173` then produces zero image/aspect/bpp, preventing a raw reservation and known join.
- **Handle reuse / retention:** `image.rs:74-92` removes all prior evidence with the reused view handle before
  inserting the new successful record, even when the image changes. Lookup also checks the image handle and
  excludes null views. Search found the renderer's `im.views.insert` at 1562 paired with this recording hook;
  moved view maps retain the same immutable view objects. Retired entries may remain in the evidence FIFO but
  cannot be recovered through an absent current membership. New recorded creation replaces a reused handle;
  an evicted still-live view remains unknown rather than inferred from registered settings.
- **Bounds / locks:** the added FIFO is capped at 4096 records (`image.rs:74-110`). Production always passes that
  fixed positive limit: retain/lookup scan at most 4096 evidence records; normal insertion at capacity removes
  one oldest record. The `while` loop strictly shrinks the deque. The accessor additionally scans the existing
  per-image view map for membership; that map is not newly bounded by the evidence cap. Cache operations acquire
  only their own mutex and copy plain values; they do not call Vulkan, perform I/O or acquire the images/ledger
  mutex while holding it. Creation and resolution use images -> evidence order; no evidence -> images reverse
  path was found. Alias traversal remains limited to 16 steps and explicitly refuses an unfinished owner chain.
- **Default off / timing:** `image.rs:98-102` caches `NXGFX_RENDER_CHAIN=1` opt-in. Default off adds the hook's
  cached condition on new successful view creation but no evidence allocation/locking. Enabled recording can
  occur before the dump frame so persistent views have evidence; it does not call ledger admission or latch a
  boot frame. The original pass-dump gates and one-frame/16-copy/64-MiB reservation policy remain unchanged.
  Enabled FIFO eviction can reduce diagnostic coverage; no rendering work is added to refresh old evidence.
- **Fixed-owner format correction:** with owner 7 still present, `exec.rs:614-620` supplies owner and registered
  image separately. `render_chain.rs:21-34,138-174` retains raw 37, registered 44 and effective 44 for the original
  alias example. `same_image` at 40-48 compares canonical backing identity/format while preserving interpretations
  in both records. `raw_filename` at 60-71 uses `raw_format`; production calls it at `exec.rs:672`. Default versus
  overridden views and sRGB-backing/UNORM-storage views are now distinguishable. The handoff counterexample above
  is a missing provenance transition, not failure of these fixed-owner assertions.
- **Rendering / accepted readback:** the retained tracked-source delta shows only the diagnostic resolver and
  filename changes in exec, and metadata/cache/test additions plus the successful-create hook in image. Rendering
  format selection/defaults and the accepted DV-01 readback ownership/recording logic are unchanged. Queue,
  tables, screenshot, NVN texture caller and runtime hashes still match their accepted identities. Existing copy
  barriers, `sub.transient` ownership and post-callback retirement remain as reviewed in the original report.

### Tests written

**None written, compiled or executed by this reviewer.** Lead supplies full graphics **121 passed / 3 ignored**,
including both new tests. This is accepted as lead evidence, not presented as independent test execution.

- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs:398-509` exercises the real metadata
  constructor, resolver, association, reservation and production filename helper. It covers both original static
  witnesses, sRGB storage fallback, missing metadata, bad aspect/range, unsupported format and D32 byte count.
  Its `backing` is constructed directly as format 37 and stays fixed; it does not exercise owner finalization.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:130-181` exercises the actual cache and
  constructor with a small injected capacity, eviction, copied format/swizzle/subresource values and reused view
  handle on another image. It does not run Vulkan or the factory/accessor wiring; those paths were source-reviewed.

**Focused remaining CPU proposal:** add `canonical_raw_format_survives_alias_owner_handoff` using the production
provenance/handoff helper used by `tex_release_now` (factor that CPU transition if needed; no duplicated model).
Seed H with creation format 37 and owner 7, create alias/heir 8 with registered/effective format 44 and retained V,
then transfer ownership with the original owner removed. Resolve through the same production path after handoff.
Assert either a known resource with raw 37, registered/effective 44 and `-fmt37.bin`, or explicit unknown with zero
reservation if provenance is deliberately unsupported. The current source returns a known raw 44 resource.
Include a same-format heir positive control and ensure the renderer's registered format stays unchanged; if the
fix uses separate bounded provenance evidence, also test its miss/reuse policy. Lead owns execution after correction.

### Gates

All commands used cwd `D:\Projects\RecompLauncherEngine`; no target directory used by this audit.

- `git rev-parse HEAD`: `7688e33ef5ef47e64322c7aed0f84c471e9c8b99`.
- `git status --short -- src/graphics/src/vk/exec.rs src/graphics/src/vk/render_chain.rs src/graphics/src/vk/image.rs docs/waves/Audit-03.md`:
  confirmed dirty/untracked scoped files; no reset, staging or unrelated change.
- `git hash-object --no-filters -- src/graphics/src/vk/exec.rs src/graphics/src/vk/render_chain.rs src/graphics/src/vk/image.rs src/graphics/src/vk/queue.rs src/graphics/src/vk/tables.rs build/GPU-STOP.json build/HARNESS3/GPU-STOP.json`:
  current identities recorded below; dependencies/latches retain the prior identities.
- `git hash-object --no-filters -- src/graphics/src/vk/shot.rs src/graphics/src/nvn/tex.rs src/runtime/src/rt.rs`:
  all three match the accepted Audit-02 and send-back manifest identities.
- `git diff --no-index --check -- NUL docs/waves/Audit-03.md`: no whitespace diagnostics; exit 1 for the
  new-file comparison, with the configured LF/CRLF warning. Exit captured from `$LASTEXITCODE`.
- Final repeat of the seven-file raw-hash command: all reviewed source/dependency/latch identities unchanged.

Retained send-back check/no-run logs show completed profiles and only the existing `spvfix.rs:593` unused-mut
warning. Clippy completion retains graphics 99 (1 duplicate), shaders 57, translator 35, runtime 7 and config 1.
No Cargo, tests, formatter, GPU, replay, release, agents or commits were executed by this recheck.

### Evidence

Read the protocol, current ledger row, DG-02 send-back at
`D:\Projects\RecompLauncherEngine\docs\waves\DG-02.md:242-378`, and production source cited above.
Artifact root: `D:\Projects\RecompLauncherEngine\build\DELEGATION\DG-02\`.

- `a03-result.json:6-32`: correction/limits/gate claims; lead's subsequent 121/3 result supersedes its pending test
  status. No GPU evidence or causal fix claim is present.
- `a03-delta-review.log:21-103,1296-1465,1799-1815,2248-2279`: isolates resolver/filename and metadata changes
  from inherited HEAD diff; DV-01 readback hunks differ only in shifted hunk positions. Current source matches
  the corrected constructs. The retained pre-/post-diff headers match previous/current exec and image identities.
- `a03-check.log:1-14`, `a03-test-no-run.log:1-14`, `a03-clippy.log` summary/completion lines: agent compile/lint
  evidence, not reviewer test execution.
- `a03-preserved-identities.log:1-7` and fresh raw hashes: accepted dependencies and both stop latches preserved.
- Both stop-latch JSON files were read; shared stop remains `stopped: true`, HARNESS3 retains the MKW reset record.

Current source raw Git blob identities (not SHA-256):

| Absolute path | Identity |
|---|---|
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | `d4b6ab9ce362cc11a0f36c608302f1bd7aa8c2d0` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs` | `287a52ee55b2df2440a61b5ee95c3390649c3d3f` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | `10d3e87ba87416640156d338aa0ad5f53ecfea07` |

Queue, tables and latch hashes equal those in the original report. No new release was built or supplied; the
historical nxfront.lib identity/build-time limitation recorded above still applies.

### Unproven

No GPU execution, driver behavior, current raw pixels, DKB causal fix or clean game is certified. Source review
does not establish that the remaining handoff counterexample occurs in DKB. Missing/evicted view evidence remains
unknown; the graph remains partial rather than all-writer causality. The one-frame proposal still requires
`NXGFX_DUMP_PNG=0` and `NXGFX_DUMP_TEX` unset to exclude independent legacy allocations.

**Technical readiness does not authorize GPU use. Both stop latches remain active and untouched.** Even after
A03-1 is fully closed, a separately recorded lead resumption decision and current build provenance are required.

### Not built

No production changes, test execution, new evidence artifacts or shared-status edits. Return A03-1 to the original
DG-02 agent for the narrow creation-provenance/handoff correction and regression above. Final-view capture/cache
work is accepted within this review's source scope; it need not be replaced to resolve the remaining issue.

### Slips

None.

### Questions

None requiring an owner decision. Remaining work is a concrete diagnostic correction and lead-owned acceptance.

**Lease/resources released:** recheck report complete; no compiler/GPU slot acquired, no worker, subagent,
helper, game or background process launched. Synchronous Git operations returned; no owned process or claim remains.

## A03-1 second recheck — 2026-11-21

### Built

| Action | Absolute path | Purpose / pre-existing state |
|---|---|---|
| Append | `D:\Projects\RecompLauncherEngine\docs\waves\Audit-03.md` | Sole writable file; second correction recheck, preserving both prior reviews. |

**Disposition: A03-1 CLOSED for the CPU/reviewed diagnostic scope. No remaining introduced blocking finding
identified in this second recheck.** The owner-promotion counterexample is corrected in the actual production
transition, and physical recreation cannot reuse an older observation merely by recycling a handle and format.
Combined with the supplied lead graphics **123 passed / 3 ignored**, the bounded discriminator is technically
ready within its supported, fail-closed diagnostic scope. This is not GPU authorization or a DKB causal fix.

Original findings and first recheck above remain historical; this disposition supersedes their open status.
Session: `ses_dffd66cafffeXYIGYnCp0WM0HO` (ledger identity from the first recheck). Source, tests, artifacts and
latches remained read-only. Every shell command used cwd `D:\Projects\RecompLauncherEngine`.
No compiler/test/GPU/subagent/commit grant was acquired.

#### Closure evidence and production integration

1. **Physical creation evidence has its own source.**
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:117-153` copies image handle, format,
   extent, type, level/layer/sample counts from `ImageCreateInfo` and assigns a fresh atomic creation ID.
   The optional record is private per-Image state at 226-230; `backing_creation` checks its handle at 267-270.
   Ordinary creation uses the same `ci` passed to successful Vulkan creation, after allocation/bind succeeds
   (`image.rs:1330-1402`). The delegated sparse branch explicitly stores None at 1342-1348. Neither the current
   owner format nor final-view format is substituted for missing physical creation evidence.

2. **The exact owner-promotion witness now retains raw 37.**
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:1961-1995` removes the former owner and
   invokes `a.promote_from(&mut im)` at 1992. The helper at 272-279 performs the existing alias/allocation
   transition and transfers the former owner's checked creation record. It leaves the heir's format, H and view
   map unchanged. Thus owner 7 / H / creation-format 37 -> heir 8 / registered BGRA8 44 retains creation-format 37.
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs:603-619` calls `resolve_images`, whose
   backing input comes from retained creation data at
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs:106-154`. The resulting resource
   keeps raw 37, registered 44 and effective 44 separately (203-224). Production filenames use raw format at
   `render_chain.rs:63-74` and `exec.rs:670`. The old `-fmt44.bin` mislabel no longer follows this trace.

3. **Replacement/follower helpers are wired into production, not just tests.**
   `Image::replace_backing` at `image.rs:281-292` returns the old image/provenance pair and installs only new
   provenance matching the supplied handle; missing/mismatched evidence clears the old record.

   | Production path in `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | Checked integration |
   |---|---|
   | Ordinary image creation, 1374-1402 | Successful final creation input records a new physical ID. |
   | Sparse creation, 1342-1348 | None; unsupported provenance cannot schedule diagnostic raw copies. |
   | Alias creation, 1611-1637 | Copies `o.backing_creation()` while retaining its own `format=tf.vk`. |
   | Parked reuse, 1442-1480; live takeover, 1485-1524 | Moves the whole Image; changing texture serial/swizzle does not invent a physical creation. |
   | Owner promotion, 1991-1994 | Calls the tested promotion helper; other aliases retain their shared creation record. |
   | Rescale, 2152-2179,2267-2297 | Records the new successful create input; old Image gets returned old H/provenance for retirement. |
   | Rescale followers, 2308-2323 | Fetches the owner's new record and calls replacement helper after draining obsolete views. |
   | Storage upgrade, 2363-2379,2441-2476 | Installs `new.backing_creation()` from ordinary creation; old Image retains its previous record and allocation/views. |
   | Storage followers, 2490-2505 | Shares the owner's new record through the same helper and drains obsolete views. |

   Constructor/assignment search found all five production Image construction sites in this file and the actual
   backing assignment in the helper. The changed helper calls preserve the previous handle/allocation movements;
   no rendering format/extent/usage selection or GPU command is gated on diagnostic provenance.

4. **Same numeric handle plus same format does not recover an old association.**
   `image.rs:132-143` assigns a new ID for each instrumented physical creation; aliases and promotions copy it,
   while rescale/storage replacement install the newly created one. `render_chain.rs:185-191` requires owner and
   registered records to agree on a nonzero creation ID and backing; `Resource::same_image` at 41-51 also requires
   ID equality. Therefore two creations of H=200, both BGRA8 format 44, with the same current owner and write
   counter still cannot match across their differing physical IDs. Current owner serial remains an additional
   conservative identity check, so a historical link crossing promotion can remain unknown without mislabeling
   the next raw capture. No counter reset or mutable-owner-format inference is used to manufacture continuity.

5. **Final-view evidence and fail-closed behavior remain intact.**
   `image.rs:1815-1837` still records only the successful final `ImageViewCreateInfo`; the 4096-record FIFO,
   reused-view-handle replacement and exact membership/image/view accessor remain at 70-115,155-166.
   Replacement paths drain old views; parked/live moves retain the same objects. No view-cache hit fabricates
   interpretation. `render_chain.rs:119-127` makes absent physical provenance image/ID/raw-format zero, and
   183-224 rejects missing/mismatched view evidence or unsupported range/format. None of those unknown cases
   reserves bytes. Sparse/uninstrumented images are intentionally excluded rather than inferred from a descriptor.

6. **Default behavior, bounds and synchronization remain in the accepted scope.**
   Shared `chain_evidence_on` at `image.rs:98-101` and `note_creation` at 151-153 leave creation-ID assignment and
   evidence recording off by default. The optional per-Image field has fixed storage even when None; there is no
   new global creation map or resource owner. Promotion/replacement add scalar copies/checks under existing image
   ownership; they acquire no new locks and perform no I/O/Vulkan calls. The atomic ID needs uniqueness only, so
   relaxed ordering does not weaken publication under the existing image lock. The view-cache lock order is as
   previously reviewed, and the alias walk remains 16 steps. No new unbounded loop is introduced.

   `render_chain.rs:8-11,260-288,351-370` retains one admitted frame, 16,384 events, 256 observations, 16 reserved
   raw attempts and 64 MiB requested payload. `exec.rs:633-642` still reserves before allocating; 655-689 retains
   barriers, exact-Sub transient ownership and completion callback. More scalar metadata does not change those
   payload limits. The independent legacy dump limits still require the explicit disable controls below.
   The retained delta and unchanged dependency identities show no DV-01 readback/queue retirement change.

### Tests written

**None written, compiled or executed by this reviewer.** Lead's supplied full graphics acceptance is **123 passed /
3 ignored**, including both new production-transition tests. Retained agent logs establish compilation only;
the lead execution result is separately attributed to the user.

- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:377-431`,
  `canonical_raw_format_survives_alias_owner_handoff`: CPU fixtures seed the old owner and alias, remove/drop the
  old owner around the real `promote_from` call, and use production `resolve_images`. Assertions retain H, creation
  ID, raw 37, the heir's registered/effective format and existing view, plus the production `-fmt37.bin` filename.
  Same-format heir is a positive control. Post-handoff provenance is not manually substituted for the helper.
- `image.rs:433-490`, `new_backing_and_alias_followers_replace_or_clear_creation_provenance`: invokes actual
  replacement/follower helpers, verifies old record return/new record install, follower identity sharing, old-view
  rejection, and changed ID/nonassociation for the same H **and** same format. Unknown/mismatched provenance clears
  raw label/ID/bpp instead of inheriting stale evidence. Tests initialize no Vulkan device or real allocation.
- Existing resolver, view-cache, D32, budget and ledger tests remain. The updated resolver fixture derives its
  physical input from `ImageCreation::from_create_info`, and the FIFO test retains one physical fixture identity.

These tests cover the identified CPU transition, not driver execution or allocation ownership under real device
loss. The production callers and allocation-transfer equivalence were therefore checked separately above.
No further blocking CPU regression proposal is required for A03-1 on this candidate.

### Gates

All audit shell commands used cwd `D:\Projects\RecompLauncherEngine`; no Cargo target or compiler/GPU slot used.

- `git rev-parse HEAD`: unchanged `7688e33ef5ef47e64322c7aed0f84c471e9c8b99`.
- `git status --short -- src/graphics/src/vk/exec.rs src/graphics/src/vk/image.rs src/graphics/src/vk/render_chain.rs docs/waves/Audit-03.md`:
  scoped dirty/untracked state preserved.
- `git hash-object --no-filters -- src/graphics/src/vk/exec.rs src/graphics/src/vk/image.rs src/graphics/src/vk/render_chain.rs src/graphics/src/vk/queue.rs src/graphics/src/vk/tables.rs src/graphics/src/vk/shot.rs src/graphics/src/nvn/tex.rs src/runtime/src/rt.rs build/GPU-STOP.json build/HARNESS3/GPU-STOP.json`:
  current candidate identities below; all seven dependencies/latches match the previous accepted records.
- `git diff --no-index --check -- NUL docs/waves/Audit-03.md`: no whitespace diagnostics; exit 1 denotes
  the new-file comparison, with the configured LF/CRLF warning. Exit captured from `$LASTEXITCODE`.
- Final repeat of the ten-file raw-hash command: all reviewed source/dependency/latch identities unchanged.

Reviewed retained agent check/no-run-final logs and clippy summaries: completed profiles, existing
`spvfix.rs:593` unused-mut warning, unchanged clippy warning counts (graphics 99 with 1 duplicate, shaders 57,
translator 35, runtime 7, config 1). These were not rerun. No Cargo/tests/GPU/replay/formatter/subagents/commits
or release commands were executed by this recheck.

### Evidence

Read the protocol and current DG-02 second send-back at
`D:\Projects\RecompLauncherEngine\docs\waves\DG-02.md:399-542`, plus the production/test sites cited above.
Artifact root: `D:\Projects\RecompLauncherEngine\build\DELEGATION\DG-02\`.

| Inspected evidence | Established scope |
|---|---|
| `a03b-result.json:6-38` | Implementer's provenance/correctness and no-GPU claims; its pending lead-acceptance statement is superseded by supplied 123/3. |
| `a03b-delta-review.log`, especially 1975-2147, and `a03b-final-render-chain.diff` identity-related hunks | Actual promotion/replacement/follower integration and new physical identity requirement amid the preserved dirty baseline. |
| `a03b-check.log:1-14`, `a03b-test-no-run-final.log:1-14`, `a03b-clippy.log` summary/completion lines | Successful agent compile/link/lint profiles with existing warnings; no independent reviewer execution. |
| `a03b-preserved-identities.log:1-7`, fresh Git hashes | Queue, tables, screenshot, NVN caller, runtime and both stop files unchanged from accepted identities. |

Current raw Git blob identities (not SHA-256):

| Absolute path | Identity |
|---|---|
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\exec.rs` | `4426b52b09a026dbc638e0bcd6748d97bdc8e465` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | `5270810bdd67caccf92ef8f4fc78ecc532a27598` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\render_chain.rs` | `e05cafc822463da970b6688c70de6ac3bd47410e` |

Stop-latch identities remain `b123fe50e51f58a97d4d89f4214a1163081d194a` (shared) and
`058e8120c9d28fc1026ba162b57eab4c766d2895` (HARNESS3), matching the active JSON records previously read.
No new release/build identity or visual evidence was generated; the historical library/build-time limitation
in the original report remains.

### Unproven

GPU execution, driver behavior, real raw pixels, readback under device loss and current DKB causality remain
unproven. This closes the diagnostic evidence defect; it does not establish that owner promotion caused DKB's
black world. The graph remains a partial attachment-observation graph. Equal write sequence is not proof of no
untracked writes; metadata misses, sparse/uninstrumented creation and unsupported view/format remain unknown.

Technical readiness applies to the proposed **single intended gameplay dump frame**, selected passes 18/27 only
after their current meaning is verified, configured extents 1440x810/2880x1620, and supported raw/D32 views.
Keep **`NXGFX_DUMP_PNG=0` and `NXGFX_DUMP_TEX` unset** to disable independent legacy allocations. A scheduled
capture still requires its completion/write-success evidence; missing data does not authorize an automatic retry.

**Both GPU-stop latches remain active.** CPU/reviewed technical acceptance does not clear them, authorize a GPU
run, or certify a game fix. A separately recorded lead resumption decision, current binary provenance and bounded
run/fault-stop ownership remain required.

### Not built

No production edits, tests, artifacts, shared-ledger changes or release build. No further A03-1 send-back is
required on the reviewed candidate. Lead owns recording its CPU/reviewed diagnostic acceptance and any separately
gated future discriminator; gameplay and causal-fix work remain open.

### Slips

None.

### Questions

None requiring an owner decision.

**Lease/resources released:** second recheck complete; no compiler/GPU slot acquired, no worker, helper, game,
subagent or background process launched. All synchronous Git operations returned; no owned process or claim remains.
