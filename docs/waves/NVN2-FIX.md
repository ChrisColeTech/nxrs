# NVN2-FIX — the NVN2-PARITY audit's findings

Owner: one delegated agent. Lead: main session. Opened 2026-11-25. Protocol: `docs/31-DELEGATION-PROTOCOL.md`;
shared rules: `docs/waves/PARALLEL-RULES.md`. **Main tree.** MKW-RECORD works in the worktree
`D:\Projects\RecompLauncherEngine-mkw`: never relink or run Mario Kart World from here, don't edit `vk/queue.rs` or
the replay split, respect `build/locks/machine.lock` (and check it immediately before starting a measured run).

## Brief (lead)

The independent audit of NVN2-PARITY (commit `78c7f82`; "Independent audit of 78c7f82" in
`docs/waves/NVN2-PARITY.md`) found four MEDIUM issues; two of them can bind storage to stale or zero data, which is
what texture pop-in looks like. Fix each as a general correction with a unit test:

1. **Set cache vs inline uniform updates** (`vk/exec.rs:515`): a storage buffer whose address the shader reads from
   a uniform buffer (c3-c18) is keyed by the binding only, so a set built before an `UPDATE_UB` that changed the
   address is reused after it. Key such sets by a uniform-update counter (or the resolved address).
2. **Size-0 pushed / handle-addressed storage buffers in sparse pools** (`exec.rs:4178`): the size runs to the end
   of the physical pool, `resolve` fails at the first unmapped page and the whole binding falls back to the zero
   buffer (nxnative's PRAGMATA / Resident Evil 4 case: a 0xc0000 run mapped at the base of a 1 GB sparse pool).
   Bound the size to the contiguous mapped run.
3. **A decoded handle's size is discarded** (`exec.rs:3194`): use `(v >> 45) * 16` (buffers > 1 MB are cut off
   today; small written buffers are noted GPU-written over up to 1 MB each submission).
4. **Stand-in storage views in the storage-view list** (`vk/tables.rs:985`, walked per draw at `exec.rs:5695`):
   write the stand-in descriptor but keep it out of the list.
5. LOWs, if cheap: on Switch 2 always decode compute c0 rows 0x210-0x310 (a size-0 handle looks like an address);
   reset the global guest clock in the counters test.

**Game checks:** `dkb` and `oot` (sheets read, FPS vs 60 / 59, red lines), `wonder` (Switch 1, unchanged).
`cargo test -p graphics --lib --release`. **Descriptor change: the lead audits the result.** No draws skipped, no
robustness weakened, native NDVE off, no commits, don't edit docs/32. Deadline 3 hours. Report under "## Report".
