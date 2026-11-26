# UPSYNC2 — the second audit of the upload-sync code (commit 2f29f99)

Owner: one delegated agent. Lead: main session. Opened 2026-11-26, worktree `D:\Projects\RecompLauncherEngine-upsync2`,
branch `upsync2` (base `ade9a6c` on `gameplay-harness`). Protocol `docs/31-DELEGATION-PROTOCOL.md`. Title: Ocarina of
Time only; GPU 1. Deadline 3 h. Background: `docs/waves/UPLOAD-SYNC.md` (the wave, its audit, "## Audit fixes").

## Plan (agent, before coding)

Files expected: `vk/upskip.rs` (most), `vk/lazy.rs`, `vk/mem.rs` (`release`, `register`, `apply_writebacks`),
`vk/queue.rs` (`Post::Early` gets the submission's timeline value), `vk/gmem.rs` (the raw log passes it on).

1. **HIGH-1, a copy's source sync mid-recording keeps a hash the GPU no longer holds.** `upload_changed` step 3: a
   page stamped by the recording in progress (`stamp == EPOCH`, noted by a draw already recorded) is still uploaded, but
   keeps its stamp with hash and parked hash 0 instead of `COPYING`; step 5 then keeps no hash for it, and its landing
   re-arms it with the hash of the GPU's bytes (parked hash 0: never "unchanged"). Applies to partial pages too (they
   lost the stamp before, harmless but it also broke `gpu_writes`' same-range filter's assumption). Test: the audit's
   interleaving: `gpu_writes(P)`, `upload_changed(P)` with U, `landed(src=G, before=U)` -> hash h(G); the CPU writes U
   again -> uploaded.
2. **MEDIUM-2, brought-back records cleared too eagerly.** `read_page` waits for `last_fence()` = V before its copy,
   so the bytes include every submission with timeline value <= V. `Pg::brought_at = V`; `raw_written` (the `Post::Early`
   callback now receives its submission's value) and `landed` (value passed from `apply_writebacks`) clear the record
   only when their submission's value > `brought_at`. `gpu_writes` still clears at note time (its submission is unsent,
   its value > any V read before). Test: a record survives a landing/raw log of a value <= V, and goes with one > V.
3. **LOW-3, the bring-back's gap.** After `protect(p, true)` other threads see the old guest bytes until the copy fills
   the page; a game store there is overwritten. A second writable view needs file-backed guest memory (it is plain
   `VirtualAlloc`), so not feasible here: mitigation in `upload_changed` step 5: a page of the batch brought back while
   it was copied (record set since step 3) has its upload dropped (the copy may hold the old bytes) and, if whole,
   hash = the brought bytes' hash; the comment at `lazy.rs:416` corrected. Runs are built after step 5.
4. **LOW-4.** `COPYING` per call: high bit plus a sequence number.
5. **LOW-5.** `release`: lazy pages brought back first (`touch`), then the entry dropped only if its end matches the
   released pool's; raw logs inside it kept where another entry still overlaps them. `register`'s pre-insert drop goes
   (the replaced pool's `release` does it, after its touch).
6. **LOW-6.** Read whether `take_lazy` can hold a page whose CPU bytes are queued in an unsent submission; fix if cheap,
   else report.
7. **Perf-only.** `landed` no longer clears another pool's stamp (its own landing of the same epoch re-arms it); a
   brought-back skip sets hash = brought on an unstamped page.

Verification: `cargo test --release -p graphics`; OoT on GPU 1: `oot-house --faults` until >= 60 dense shots (two
runs), one `oot`; every sheet and report.md read.
