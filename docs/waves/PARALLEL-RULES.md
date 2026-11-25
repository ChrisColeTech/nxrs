# Parallel waves — shared rules (2026-11-25)

Several waves run at once in the same working tree (owner: builds and games may run concurrently). Every wave brief
that links here follows these rules on top of `AGENTS.md` and `docs/31-DELEGATION-PROTOCOL.md`.

**Running at the same time:** MKW-PERF5 (graphics replay, Mario Kart World, GPU 0), SMASH-POLISH (audio + prewarm,
Smash, GPU 1), ROUTES-SS (scenarios only, Shadow/Sonic, GPU 1), VIOLET (Violet, GPU 1), OOT-PIPE (OoT, GPU 1).

1. **Your titles only.** Run, build and relink only the titles your brief names. Other agents edit engine files at
   the same time: change only the lines your fix needs (Edit tool, never whole-file rewrites), re-read a file right
   before editing it, never revert or "tidy" code you did not write.
2. **One engine build at a time.** `dist/nxrs` is shared. Before `scripts/build/build.ps1 -KeepCache -SkipGnu`, take
   the lock: `New-Item -ItemType File build/locks/dist.lock -Value <WAVE-ID>` (it fails if the file exists). Hold it
   through the build and the relink of your titles (`dist\nxrs\nxrs.exe --select-title "<title>" --build`), then
   `Remove-Item build/locks/dist.lock`. If it is held, poll every 30 s in a foreground loop (under 10 minutes per
   command) and retry. Never delete another wave's lock unless its owner's process is gone for > 30 minutes.
3. **A build error in code you did not touch** is another wave mid-edit: wait a few minutes and retry; do not fix it.
   A game regression you cannot tie to your change: re-run once, then record it in your report (with the run) for
   the lead instead of chasing it.
4. **GPU:** use the GPU your brief names (`--env NXGFX_GPU=<n>` on every `play.py` run).
5. **Perf measurement** (a before/after FPS or timing pair) needs the machine alone: create
   `build/locks/machine.lock` (value: your wave ID), wait until no other `play.py` run is in progress (the other
   waves check the lock before starting each run and hold off while it exists), measure, delete the lock. Keep it
   under 20 minutes. Everyone else: check `build/locks/machine.lock` before starting each game run; while it
   exists, do source/analysis work instead.
6. Tests: run the tests of the crates you changed (`cargo test -p <crate>`); a failure in a test you did not touch
   may be another wave's: re-run once, then report it.
7. **Never run two copies of one title at once** (e.g. `shadow` and `sonic`): they share the build folder's
   `pipeline-*.bin`, and the second process blacklists the first's in-flight compiles as crashes (ROUTES-SS).
8. No commits (the lead commits accepted work), no edits to `docs/32-REMAINING-WORK-PLAN.md` (the lead keeps it), never `--real-save`, kill by PID.
