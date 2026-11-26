# USE-SYNC — CPU-write tracking by write faults instead of the per-submission write-watch scan

Owner: one delegated agent. Lead: main session. Opened 2026-11-26 ~18:10, worktree
`D:\Projects\RecompLauncherEngine-usesync`, branch `usesync` (base `4b9e524`). Protocol `docs/31-DELEGATION-PROTOCOL.md`.
Title: Mario Kart World only, GPU 0 (OOT-DESC owns OoT on GPU 1). Deadline 4 h. Design source: `REF-ARCH.md` difference #1
and its USE-SYNC wave.

## Plan (written before code)

Step 0: one quiet `mkw` run on HEAD, window [250, 325] s: FPS / 5 % low, replay ms, prep ms, the `[vk] pools:` parts
(lock / scan / pass, sealed count) and the census of why the mirrors not sealed are not. SCAN-COST measured OoT only.

Step 1 (stage A, the cost the scan is): extend `vk/seal.rs` from whole quiet pools to **chunks**. A watched mirror is
armed whether or not the CPU writes it; a store into a protected 64 KiB chunk faults, the handler opens that chunk and
stamps it. The pool stays in the registry: a sync's write-watch scan asks only its **open chunks**, not its whole range.
An open chunk the scans find unwritten for a few frames is closed again (protected, watch taken); a chunk written frame
after frame stays open (hot) and is asked by every scan, as Turbo's hot pages are copied at every submission. The
existing guarantees carry over unchanged: the handler opens a chunk before the store lands, a scan reads the open set
after reading the replay queue's tail, host writes and commits open what they touch (`host_write`), landings unseal.

Step 2 (stage B, only if stage A leaves sync cost that matters): the use-driven part — check only the pages a binding
uses, in the replay thread's pre-pass. Decided after stage A's numbers.

Switch `NXGFX_USE_SYNC` (0 off = today's whole-pool sealing; 1 on; `verify`: at every scanning sync, every armed pool's
**closed** chunks are peeked too and a written page there — a store the new path would miss — is a red line and is
taken). `docs/env_switches.tsv` rows.

Files expected: `vk/seal.rs`, `vk/mem.rs`, `docs/env_switches.tsv`, this doc.

What would disprove it: MKW's scan already small after sealing (then this wave's gain is small, said with numbers), or
faults on chunks that are written often but not every frame cost more than the scan they replace.
