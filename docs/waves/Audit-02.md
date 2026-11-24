# Audit-02 — Independent DV-01 implementation review

## Built

### File table first

| Action | Absolute path | Purpose | Pre-existing state |
|---|---|---|---|
| New | `D:\Projects\RecompLauncherEngine\docs\waves\Audit-02.md` | Independent source/evidence audit, findings and disposition | Confirmed absent before creation; sole writable file |

**Disposition: accept DV-01 for the CPU/reviewed scope. No blocking introduced P0–P3 finding identified.**
This is an independent implementation review combined with the lead-supplied CPU/doc results, not an independent
test execution or hardware certification. Session ID is unavailable. Review date: 2026-11-21.

All production code, tests, evidence, shared documents and GPU-stop latches remained read-only.
Baseline HEAD independently confirmed: `7688e33ef5ef47e64322c7aed0f84c471e9c8b99`; extensive dirty tree preserved.
Every shell command used cwd `D:\Projects\RecompLauncherEngine`. Compiler/test/GPU lease: NONE.

### Runtime boundary and ABI

- `D:\Projects\RecompLauncherEngine\src\runtime\src\rt.rs:106-155`: the change makes the existing
  unconditional raw-pointer dereference an explicit unsafe caller obligation. Non-nullness, alignment, initialized
  lifetime and no concurrent mutation are required even on a sampled-out hit. Diagnostic guest reads have their
  own lifetime/alignment/no-write obligations; readable-page checks are correctly not presented as synchronization.
- `#[no_mangle]`, `extern "C"`, `*mut Cpu` and unit return remain intact at lines 125-126. The body does not gain
  CPU writes or pointer retention. `D:\Projects\RecompLauncherEngine\src\runtime\src\cpu.rs:9-43,65-78`
  supplies the matching aligned C layout, `GuestFn` signature and initialized boxed positive-test value.
- `D:\Projects\RecompLauncherEngine\src\translator\runtime\cpu.h:7-38` still declares `void rt_probe(Cpu*)`.
  `D:\Projects\RecompLauncherEngine\src\translator\src\aot\emit.rs:477-479` spills state and calls
  synchronously before the instruction. Rust unsafety does not change this C ABI. Actual generated-code
  guest-memory synchronization is not newly established by a declaration and is not claimed here.

### Readback ownership and integration

Production sites: `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:3208-3299,3303-3380`.
The following traces were checked against the actual caller, helpers and queue, not just the wave report:

| Result path | Ownership and externally visible result |
|---|---|
| Staging allocation returns Err | Returns false before submission or destination access; no returned Buf to release. Existing allocator-internal cleanup limits are separately noted below. |
| `ensure_image` returns false | Recording closure returns before extracting/using the staging handle; state remains unrecorded and has no Post callback. Finalization skips waiting, takes/releases staging once, returns failure. Other preparation work can still be submitted, but has no reference to this allocation. |
| Scaled bounce allocation fails | Private recorder returns false before any command references the staging buffer. Same immediate-unused-buffer release and unchanged destination as above. |
| Recorded copy, completion, successful wait | Callback sets completed and leaves the wanted buffer in state. Caller takes sole ownership, copies pitched rows, then frees once and returns true. Queue recycling cannot free this buffer: it was never placed in `sub.transient` or `sub.stage`. |
| Wait fails before callback | Caller marks abandoned, leaves buffer in state and returns false without reading/copying/freeing it. The already-attached callback retains its Arc after the caller drops its Arc. |
| Callback arrives after failed return | Callback takes the abandoned buffer under the mutex and frees after unlocking. It cannot touch the former destination because neither the destination nor the copy closure was captured. |
| Completion races with failed wait | Completion-first leaves the buffer for finalization to take/free without copying; abandonment-first lets the callback take/free. Mutex plus `Option::take` gives exactly one owner. A false wait stays failure even if completion is now known. |
| Wait says true without matching completion | Defensive failure retains pending staging for the callback; no CPU read or premature release. Normal queue ordering excludes this state. |
| Submission never completes | Pending submission/callback retains staging indefinitely. This intentional retention is preferable to freeing GPU-referenced memory and is explicitly outside recovery certification. |

**Private recording result is truthful.** In
`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:3533-3628`, the direct branch returns true
after its copy command. The scaled branch returns false at 3574 on failed bounce acquisition; the first
staging-buffer copy in that branch is later, at 3593 or 3625. The preceding checkpoint only records a numeric
marker and a formatted handle description, not buffer access
(`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\lost.rs:53-72`). The public void wrapper at
`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:3521-3530` preserves unrelated callers;
their failure handling is not silently counted as fixed.

**Queue order is sufficient for this protocol.**
`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\queue.rs:1922-1944` executes the closure inline,
with the images lock, before `submit_sub` and before `oneoff` returns. Thus finalization cannot see an
intermediate `recorded=false` while its copy is being recorded. Callback registration precedes submission.
Lines 677-685 record the existing host-read barrier; 700-779 associate the Sub with its physical timeline value
and move it into Inflight. Lines 803-821 wait for that value; timeout loops and non-timeout error handling do not
fall through to callbacks. Lines 855-887 execute Post callbacks, then 949-955 publish done. The physical-fence
wait at 1779-1804 checks that publication and caps the wait at one hour. The new callback does not use the
potentially racy late `Queue::defer` registration path.

**Lock/lifecycle review:** the new state mutex is per call; recording releases it before submission. Finalization
releases its first guard before waiting, and both helpers take buffer ownership before unlocking and freeing.
The callback acquires neither the submission nor images lock. `free_buffer` takes the allocator lock, but no
new state-lock/allocator-lock cycle exists because the state guard has already gone. The caller waits only after
`oneoff` has returned and released its submission/images guards. Queue Inflight is popped outside its list lock
before callback execution. `Gpu` is process-lifetime storage
(`D:\Projects\RecompLauncherEngine\src\graphics\src\vk\mod.rs:89-109`), not a temporary receiver being
dropped during a late callback. No introduced ordinary-path race, deadlock, duplicate release or leak found.
Existing panic/device-loss recovery and global queue lifecycle are not certified by this review.

### Destination contract, pitches and screenshot failure

- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:3137-3156,3203-3207,3271-3282`:
  the documented block dimensions, packed source size and pitched destination span match the loop. The span
  includes padding/depth rather than assuming packed size. Sequential destination-row overlap is allowed;
  overlap with the staging source or conflicting references/access remains forbidden. Arithmetic limits and
  pointer validity are unsafe caller obligations; zero pitches are literal here. Success uses the original loop.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\tex.rs:42-46,191-226`: packed geometry and
  strided zero defaults remain unchanged (`row=r0`, `img=i0.max(effective_row)`). Both forwarding entries remain
  void and already unsafe through the macro at
  `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\entry.rs:128-150`. The internal bool is consumed
  without changing bridge returns. Destination lifetime/conflict obligations extend through the blocking call;
  copied metadata does not make an arbitrary guest destination valid.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\mod.rs:51-62` and
  `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\mod.rs:97-109` invoke the forwarding closures
  synchronously. Only the later Post closure escapes, and that closure captures shared staging state alone.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shot.rs:104-161`: allocation lengths use checked
  usize arithmetic and the isize allocation limit for both source and RGBA output. Positive height guarantees
  row-byte multiplication is bounded by the already-checked full allocation. Depth is one and blocks are 1x1.
  A false readback exits before conversion and returns None, so the PNG branch at 89-100 is not entered.
  Valid sizing, conversion and vertical flip remain equivalent. The old 65,536-by-65,537 arithmetic truncation
  is removed without needing to allocate such an image.

### Baseline observations, not introduced findings or DV-01 send-backs

These constrain the acceptance statement; neither warrants expanding this wave implicitly.

1. **A02-B1 / P2, pre-existing partial-allocation cleanup:**
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\scale.rs:108-122` creates an image, then uses
   `?` on allocation/bind failure before installing cleanup. Concrete witness: image creation succeeds and
   allocation fails; the function returns None without destroying that image. The file is unchanged in DV-01
   (and clean against HEAD). Similarly, buffer creation followed by allocation failure at
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\mod.rs:182-198` does not destroy the created
   buffer. This is separate from the readback staging allocation that DV-01 now correctly releases on no-copy.
   No new failure site or leak was introduced by extracting the result-bearing recorder. Proposed CPU regression
   and separate cleanup scope are below.
2. **A02-B2 / P2, pre-existing crop-end overflow:**
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shot.rs:124-126` still adds signed crop endpoints
   without checking overflow. For a 2x2 source and crop `[i32::MAX, 0, 2, 1]`, `x+w` panics with overflow checks
   or wraps negative and passes the bounds comparison without them, reaching an invalid source offset.
   Destination allocation sizing is nevertheless correct. The exact crop check predates DV-01 and was moved
   unchanged into `capture_with`, as the current Git diff confirms. Checked destination sizing is accepted;
   universal crop/source-region validation is not claimed. This is a separate baseline correction candidate.

## Tests written

**None written, compiled or executed by Audit-02.** Production/tests were read-only.

Reviewed existing DV-01 coverage:

- `D:\Projects\RecompLauncherEngine\src\runtime\src\rt.rs:120-124,332-349`: negative safe-call doctest
  plus initialized positive unit caller through the exact `GuestFn` ABI type. The type assignment alone permits
  safe-to-unsafe coercion and would not prove the boundary; the negative example supplies that check. The
  documented `nxrt` crate and public paths exist. State assertions cover pc/x0, not every Cpu field. Probe test
  execution assumes controlled diagnostic environment, especially unset `NXRT_FATAL_PEEK`.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:3158-3184`: same imports, arguments
  and bool result in the negative and positive no-run examples. Function bodies are type-checked even though
  not called. The positive control prevents an inaccessible API or bad type from masquerading as rejection of
  the unsafe call. Neither initializes a graphics device; the `E0133` label alone is not the basis of acceptance.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs:3383-3517`: all five tests invoke the
  actual production ownership helpers. Sentinel destination checks, release/Drop observations, retained callback
  ownership after caller drop, both completion orderings, wrong-submission isolation and success-before-free
  are meaningful regressions against the former unconditional copy/free behavior. They inject `recorded` and
  completion; they do not execute real command recording, queue scheduling or a simultaneous threaded race.
  The actual integration was therefore reviewed separately above.
- `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shot.rs:365-430`: tests invoke production
  `capture_with`/`capture_len`. Failure rejects zero-filled false success; successful injected pixels plus flip
  reject an always-fail implementation. Extent witnesses are allocation-free and include RGBA output limits.

Focused CPU proposals, **not acceptance blockers or commands run**:

1. For future integration hardening, an injectable production recording boundary in
   `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` could force image-unavailable and
   bounce-unavailable separately, observing no buffer-reference command/callback, no wait/read and one release;
   direct/scaled successes must attach exactly one callback before fake submission. Avoid a second model of the
   boolean state machine. Current simple branches are adequately source-reviewed, not claimed CPU-executed.
2. A deterministic two-thread helper test using barriers and atomic drop/release counters could exercise
   completion-first and abandonment-first on the production helpers, ensuring release closures can re-lock state
   (release is outside the mutex). Existing sequential tests cover the logical orderings, not scheduler stress.
3. **A02-B1:** under a separate implementation lease, inject create/allocate/bind/release operations into the
   production resource-acquisition path. Create-success/allocate-failure must destroy once; bind-failure must
   destroy/free once; success must defer cleanup until retirement. Test the real acquisition path, no Vulkan.
4. **A02-B2:** use production `capture_with` with the 2x2 descriptor and crop witness above. A fake reader should
   assert that any accepted region is in bounds, or that invalid crop rejection avoids the reader. Assert normal
   crops still forward exact offsets/extents. Use a shared checked-end predicate if factoring is needed, rather
   than copying the condition into the test; never submit the invalid region to a device.

## Gates

All audit shell operations were read-only Git commands in `D:\Projects\RecompLauncherEngine`; tool output is
retained in this conversation, with no additional artifact files. Commands used:

- `git rev-parse HEAD` and `git status --short`: confirmed requested HEAD and extensive pre-dirty baseline.
- `git diff -- src/runtime/src/rt.rs src/graphics/src/vk/shot.rs src/graphics/src/nvn/tex.rs`:
  inspected actual changes and distinguished the prior `view_offset` formatting hunk. Image changes were
  reviewed using current source and the retained pre-dirty/delta evidence, not attributed wholesale from HEAD.
- `git hash-object --no-filters -- src/runtime/src/rt.rs src/graphics/src/vk/image.rs src/graphics/src/vk/shot.rs src/graphics/src/nvn/tex.rs src/graphics/src/vk/queue.rs build/GPU-STOP.json build/HARNESS3/GPU-STOP.json`:
  all seven identities match the final DV-01 manifest; queue and latches also match its pre-extension manifest.
- `git diff --no-index --check -- NUL docs/waves/Audit-02.md`: report-only whitespace check; exit 1 for the
  new-file comparison, no whitespace diagnostics, only the configured LF/CRLF warning. Exit was explicitly
  captured from `$LASTEXITCODE`; this is not reported as an exit-0 gate.
- Final repeat of the seven-file raw hash command: all identities still match. `git status --short --
  docs/waves/Audit-02.md src/graphics/src/vk/scale.rs` reports only the new audit report; scale.rs is clean.

**No Cargo, test, rustfmt, GPU, graphics-device initialization, game, release/relink, subagent or commit command
was executed.** No target directory or compiler slot was used by this audit.

Previously supplied lead acceptance (not re-executed here): runtime **45 unit + 1 compile-fail doc**;
graphics **116 passed / 3 ignored + 2 read_texels docs**; current runtime/graphics clippy **exit 0 with existing
warnings**. These counts are explicit user/ledger evidence; the DV-01 artifact directory inspected here contains
agent compile/no-run/lint logs, not independently inspected lead test-execution transcripts. This distinction
does not erase the lead's supplied acceptance evidence.

Retained agent gates inspected: `cargo check -p graphics --all-targets --jobs 4`,
`cargo clippy -p graphics --lib --jobs 4`, `cargo test -p graphics --lib --no-run --jobs 4`, and
`cargo clippy -p runtime --lib --jobs 4`; recorded cwd is the project and target is
`D:\Projects\RecompLauncherEngine\target\OOTWAIT`. Logs show completed profiles, not additional test runs.
Graphics clippy retains 99 warnings (1 duplicate), shaders 57, translator 35, runtime 7 and config 1.
Runtime clippy retains runtime 7/config 1. The old graphics raw-pointer error is present in the earlier log and
absent from the final completed gate. No warning cleanup or suppression was requested or performed.

## Evidence

Read the five required briefing documents and the production/tests/adjacent integration sites cited above.
Evidence root: `D:\Projects\RecompLauncherEngine\build\DELEGATION\DV-01\`.

| Viewed evidence | What it establishes |
|---|---|
| `implementation.diff`, current runtime Git diff | Focused unsafe boundary/comment/test changes; no runtime algorithm change. |
| `extension-callers.diff`, current caller Git diffs | Original extent correction, forwarding contracts, retained pitch defaults, subsequently added failure propagation. |
| Relevant `readback-delta-review.log` hunks and `extension-baseline-image.diff` search results | No-copy/ignored-wait behavior existed before the behavior extension; precise new state/recording-result changes amid extensive older image work. |
| `baseline-identities.log`, `readback-baseline-identities.log`, `readback-final-identities.log` | Candidate provenance and unchanged queue/latches, independently matched with current raw hashes. |
| `readback-check-graphics.log:1-14`, `readback-test-graphics-no-run.log:1-14` | All-target check and library compile/link completed; baseline `spvfix.rs:593` unused_mut warning; no execution implied. |
| `clippy-runtime.log:1-93`; summary/error searches in `clippy-graphics.log`, `extension-clippy-graphics.log`, `readback-clippy-graphics.log` | Runtime gate completion, original graphics blocker and subsequent completed warning-bearing graphics gates. |
| `readback-process-release.log:1` | Implementer's recorded zero remaining compiler/formatter processes; historical evidence, not an audit-wide process scan. |

Current raw Git blob identities (not SHA-256):

| Absolute path | Identity |
|---|---|
| `D:\Projects\RecompLauncherEngine\src\runtime\src\rt.rs` | `caf87cb0186917217e9af241503cd01c36bf6826` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\image.rs` | `f2616c2a5ac463c0feb5674898f19b9479239d3f` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\shot.rs` | `035db87e49f728606cb6d1f58cad4b1b3724ea82` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\nvn\tex.rs` | `a599f3fe9783dd9be1c7ea3f2ef48d6b7b6cbee7` |
| `D:\Projects\RecompLauncherEngine\src\graphics\src\vk\queue.rs` | `c98c74145c85e25e1435efd4cc53b171a73f7373` |
| `D:\Projects\RecompLauncherEngine\build\GPU-STOP.json` | `b123fe50e51f58a97d4d89f4214a1163081d194a` |
| `D:\Projects\RecompLauncherEngine\build\HARNESS3\GPU-STOP.json` | `058e8120c9d28fc1026ba162b57eab4c766d2895` |

Historical library identity, supplied rather than remeasured:
`D:\Projects\RecompLauncherEngine\target\OOTWAIT\release\nxfront.lib`, SHA-256
`6827ce497d42a30b199ae82a659666330ca9390dd03afd8977ba28f43ba83895`; build time **unrecorded**.
It is not evidence of a release containing DV-01. No new binary or visual evidence was generated/viewed.

## Unproven

Real GPU copies/pixels, scaled readback, allocation failure, one-hour timeout, late device completion,
device-loss recovery and game behavior remain unverified by this audit. CPU helpers prove only their tested
transitions; source review establishes queue wiring, not driver execution. No hardware/game claim or GPU
resumption authorization is made. Both stop latches remain active and untouched.

Unsafe contracts do not validate arbitrary pointers, synchronize arbitrary guest memory, prove every texture
format/view combination, or fix all other raw-pointer APIs. Permanent pending retention is intentional.
Baseline partial-allocation cleanup, invalid crop handling, panic recovery and other public copy-wrapper callers
remain outside the accepted correction; they are not hidden behind a global soundness or leak-free claim.

## Not built

No DV-01 implementation send-back is required by this review. Lead owns final ledger/status disposition and any
separately scoped baseline follow-up for A02-B1/A02-B2. This audit wrote only the declared report and no tests,
production fixes, artifacts or shared-status updates. No release/relink was built.

## Slips

None.

## Questions

None requiring an owner decision. Sources answer the audited ownership/ABI questions; the optional CPU proposals
and baseline follow-ups are implementation scheduling matters, not requests to widen this audit's lease.

**Resource lease released:** Audit-02's report work is complete; no compiler/GPU slot was acquired, no worker,
subagent, helper, game or background process was launched, and no owned process or resource claim remains.
Lead can record the CPU/reviewed acceptance. GPU gate remains blocked.
