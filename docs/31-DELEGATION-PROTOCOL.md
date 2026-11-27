# 31 - Delegation protocol: how the lead and agents divide work (rewritten 2026-11-26)

**Status: BINDING.** Owner, 2026-11-26: the previous version was not written properly; rewrite it on the pattern of
`D:\Projects\giga-cut-rust\docs\31-DELEGATION-PROTOCOL.md`. Every agent working on this repository reads this document
before it starts, and the lead holds every brief, report, merge and commit to it. Where this document and an older one
disagree about *how work is delegated*, this one wins (it replaces `docs/waves/PARALLEL-RULES.md`, the lock files
`build/locks/*.lock`, and `AGENTS.md`'s summary). `ARCHITECTURE-AND-STYLE-GUIDE.md` still governs the code itself;
the owner's instructions govern both. The ledger is `32-REMAINING-WORK-PLAN.md`; the game-outcome roadmap is
`33-REFERENCE-DELTA-ROADMAP.md`.

## 1. Roles

| Role | Who | Does | Never does |
|---|---|---|---|
| **Owner** | the person | sets scope and order, decides real choices, authorizes | - |
| **Lead** | the main session | writes briefs, opens and removes worktrees, reviews reports, re-runs tests, reads the sheets, takes the accepted perf numbers, merges and commits, keeps the ledger, rebuilds the main `dist` | attributes its own inferences to the owner; lets accepted work sit uncommitted |
| **Wave agent** | a subagent, one per wave | owns a bug or feature **end to end** in its own worktree: diagnosis, the fix, its tests, its build, verification in the game, the wave doc, commits on its own branch | touches another wave's worktree or branch, commits to `gameplay-harness`, pushes, edits the ledger, kills by image name |
| **Audit agent** | a fresh subagent with no stake | compares built work with the spec, the reference and the pictures, writes a findings document; only when the owner asks for one | writes or changes code or tests |

## 2. Parallel waves: one worktree each

Owner, 2026-11-25: builds and games may run concurrently at full parallelism. Waves therefore do not share a tree.

- **Each wave gets its own git worktree and branch**, opened by the lead from a committed `gameplay-harness`:
  `git worktree add D:\Projects\RecompLauncherEngine-<wave> -b <wave> gameplay-harness`. The agent builds there
  (its own `target/` and `dist/`) and relinks its titles from its own `dist`. Nothing is shared but the machine and
  the library, so there are no lock files and no waiting on other agents.
- **Titles belong to one wave at a time.** The brief names them. A title's build folder in the library is shared by
  every worktree, so only the owning wave relinks or runs it. Taking turns is not enough: each wave's relink replaces
  the other's build (2026-11-26, UPSYNC2 and OOT-FLASH both on OoT ran each other's builds). The lead never gives one
  title to two running waves. **Never two copies of one title at once** (they share
  `pipeline-*.bin`; the second blacklists the first's in-flight compiles).
- **A worktree's `build\tools` is a junction** into the main checkout: remove the junction (`rmdir`) before removing
  the worktree, never `rm -rf` it. Check disk (`duf`) before opening one; each `target/` is large.
- **One wave alone** may work in the main tree instead (no worktree); the rules below still hold.
- **GPU:** two GPUs; the brief names one (`--env NXGFX_GPU=<n>` on every `play.py` run). Two waves running games
  at once get different GPUs (the lead checks `run.json` of recent runs; 2026-11-26 every brief said GPU 1 and four
  games ran on it while GPU 0 sat idle).
- **Perf numbers:** agents' numbers are indicative (the machine is shared); an agent's report says what else was
  running during its runs if it knows. The lead's quiet-machine numbers are a regression check after merge, never a
  merge gate (2026-11-27: eight finished waves sat unmerged waiting for a quiet machine that never came).

## 3. The brief (lead to agent)

Every brief has these parts, in this order. A brief missing one is not sent.

1. **Wave id and one-line goal**, the deliverable (a fix verified in the game, never a diagnosis), the base commit,
   the worktree path and branch, the titles it owns, its GPU.
2. **Read first:** this document, `AGENTS.md`, the architecture guide, the ledger rows, the relevant wave docs and
   source, and the evidence (run folders).
3. **Scope and hypothesis:** the behaviour, the evidence, exclusions, decisions already made (cite them), and what
   would disprove the hypothesis. Reference implementations (nxnative) are evidence, not truth.
4. **Wave doc first:** the agent writes `docs/waves/<id>.md` (plan, expected files) before any code.
5. **Verification required:** the scenarios (`py scripts/play.py --list`), the regressions, and the numbers to report
   against which baseline and target.
6. **Deadline** (investigation: 1-2 h hard; implementation: as the brief says) and the **report shape** (§5).

## 4. Rules the agent follows while working

- **Edit only with the editor tools** (Edit, Write), one file at a time. No `sed`, no script that writes a `.rs`,
  `.toml` or `.md`, no multi-file replace; `cargo fmt` is the only bulk rewrite. A slip is reported and the lead checks
  the diff of every file it names.
- **Fix the cause, in source.** Never answer a rendering bug by skipping draws, hiding work, weakening robustness or
  acceptance, faking a service result or forcing a completion signal. Live native NDVE stays off. Blacklists stay
  unless a specific corrected pipeline is shown safe; then remove that entry, not the list.
- **Tests for every behaviour added** where a test can show it, run and passing (`cargo test --release -p <crate>` for
  each crate changed; launcher: `cargo test` and `cargo clippy --all-targets -- -D warnings` in `src/launcher`;
  harness: `py -m unittest scripts.test_play`). Default test runs stay seconds-fast. Fix the product, not the test.
- **Verify in the game** with `scripts/play.py`: read every sheet yourself, and read `report.md`'s anomaly table,
  "Near each screenshot" and route checks. An **OFF-ROUTE** run's FPS is invalid. Use `--faults` when chasing a
  one-frame fault. A game claim needs screenshots over the declared stretch, not an FPS number alone.
- **Measure against the baseline, not against switch-off runs** (owner, 2026-11-27). One baseline run at the start
  of the wave with the diagnostics on (frame time, replay ms, the stage splits); after the work, a normal run compared
  with that baseline and its diagnostics. The diagnostics show what a change did; no run with the new switch turned
  off as a "control", per change or per wave. A change's switch exists to turn it off if it breaks something. A
  change that moves nothing visible (FPS, frame time, the stage it targeted) is taken out, not kept.
- **Diagnostics are steps, not milestones.** The work ends with a source correction and its game result, or a
  reported blocker.
- **Device loss or reset:** stop the workload by PID, keep the log; it becomes the wave's top-priority bug. Never
  re-run the same path unchanged.
- **Use the normal paths (owner-authorized).** Build with the normal CLI into the library entry's configured build
  folder and run games from the real library (`%LOCALAPPDATA%\nxrs`) and the launcher. No throwaway temp builds, no
  per-run copies of a game, its data or its shaders. (This line was dropped in the 2026-11-26 rewrite by mistake and
  restored the same day.)
- **Saves:** back up before a run that may change them; restore after experiments; never `--real-save`. Anything
  moved or swapped is backed up and restored, and the restore direction checked.
- **Processes:** close only your own, by PID, never by image name. A relink (`dist\nxrs\nxrs.exe --select-title
  "<title>" --build`) returns early; wait on its PID before relying on its output.
- **Stay in scope.** Unrelated defects are reported, not fixed, unless they block the wave. Scratch work stays in the
  scratchpad, outside the repo.
- **Commits:** on the wave's own branch only, as work lands (owner, 2026-11-25: work never piles up uncommitted),
  message ending with the attribution line. Never `gameplay-harness`, never push.
- **Python:** `py` with `PYTHONDONTWRITEBYTECODE=1`.

## 5. The report (agent to lead)

Every report has these headings, in this order. A missing heading is a returned report.

1. **Built:** what changed per item of the brief and why it fixes the cause; diagnosis kept apart from correction.
2. **Tests:** each test and what it asserts, whether it failed before the fix, and the run result.
3. **Verified:** the runs (folders), sheets read, anomaly and route-check results, numbers against baseline and target.
4. **Unproven:** what no test or run shows, and the next step.
5. **Slips:** any §4 rule broken, with the files involved. "None" if none.
6. **Questions:** only those that pass §6, each with the sources checked and a recommendation.

## 6. Settling a question (before anyone asks the owner)

In this order; stop at the first that answers: the owner's latest instructions; the ledger and recorded decisions;
the architecture guide and contracts; current source and reproducible evidence; documented upstream behaviour and the
reference implementation. Then:

- answered -> act on it;
- promised behaviour not yet built -> owed work, done, not asked;
- missing evidence -> run the smallest experiment that settles it;
- excluded by an owner decision -> not raised again;
- none of these -> a real question for the owner, stated with the sources checked and a recommendation.

## 7. The lead's acceptance (before any merge)

The lead merges a wave only when every line holds:

- [ ] The report has all §5 headings.
- [ ] Slips, if any, are checked in the diff.
- [ ] The lead re-ran the tests of every changed crate in the wave's worktree: all pass.
- [ ] The lead read the wave's final sheets and `report.md`s (anomalies, route checks), and any fault captures.
- [ ] Perf: the report's numbers against the wave's baseline are recorded in the ledger. The lead's quiet-machine
      run comes after the merge when the machine is free; it never holds a merge. Titles already at their target
      (owner: work the broken titles, don't re-verify passing ones) get one regression run each, by the lead;
      agents spend their runs only on the titles below target.
- [ ] Merge into `gameplay-harness` (`git merge --no-ff <wave>`), message ending with the attribution line; the
      ledger row updated and committed; the main `dist` rebuilt and the wave's titles relinked from it.
- [ ] The lead merges each wave as soon as it reports and these lines hold, never batching finished waves; the
      worktree is removed (junction first) right after its merge. A next wave branches from the merged
      `gameplay-harness`, not from an unmerged wave branch.
- [ ] Push only when the owner asks, and to the feature branch (`git push origin gameplay-harness`), where CI builds
      and tests. `main` changes only by merging a green feature branch (a pull request) when the owner says so; a push
      to `main` is what builds and publishes the release (owner, 2026-11-26).

Any unticked line sends the work back with the exact output and whether the product or the test is at fault.

## 8. Agents: reuse, fresh, size

- **Send-backs** for a wave go to that wave's agent while its context is healthy.
- **Fresh** agent for an audit the owner asks for, and for a new wave once an agent's context is large or it repeats slips; the
  brief then carries the conventions it needs. Fresh agents complete ledger rows, not open-ended game bugs.
- **Split** a large wave when one part can be verified and merged alone.
- **Few agents:** one per real sub-project; never split one doc set or one file's change between agents.

## 9. Enforcement

- `AGENTS.md` points here, and every brief's "Read first" names this document.
- The lead returns a brief, report or merge that breaks §§3-5, and records the return in the ledger row.
- Repeated slips of one rule end that agent's reuse.
- The lead's own breaches (a misattributed instruction, an unmerged accepted wave, a question a source answered) are
  logged in the ledger when the owner points them out, and the rule that was missed is added here.
