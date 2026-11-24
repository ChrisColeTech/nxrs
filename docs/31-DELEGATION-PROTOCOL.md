# 31 — Delegation protocol: nxrs engine, launcher and acceptance

**Binding for delegated work; revised 2026-11-22** after a review found that most of the first version's process
rules were local choices that stopped work rather than real constraints. Adapted from
`D:\Projects\giga-cut-rust\docs\31-DELEGATION-PROTOCOL.md`. `ARCHITECTURE-AND-STYLE-GUIDE.md` governs code; owner
instructions take precedence over both. The execution ledger is `32-REMAINING-WORK-PLAN.md`; the game-outcome
roadmap is `33-REFERENCE-DELTA-ROADMAP.md`; `PORT_PLAN.md` and `INTERFACES.md` stay the status and interface records.

## 1. Roles

| Role | Owns |
|---|---|
| **Owner** | Scope, priorities, product choices, authorizations |
| **Lead** — main session | Briefs, priorities between agents, integration review, the ledger and status docs. May implement, build, test and run games like any agent |
| **Agent** | A bug or feature **end to end**: diagnosis, the fix, its tests, the build, and verification in the game. It keeps the work until the game shows the result or the agent reports a real blocker |
| **Auditor** — fresh session | An independent review, only when §4 calls for one |

Domain specialties (graphics, shaders, runtime/OS, harness, launcher) describe who knows what; they do not limit
what an agent may run for its own work.

## 2. The brief

1. **Goal and baseline:** the deliverable (a fix verified in the game, not a diagnosis), current HEAD and dirty-tree
   state, the build in use.
2. **Read first:** absolute paths to this protocol, `AGENTS.md`, the architecture guide, the ledger row, relevant
   source and evidence.
3. **Scope and hypothesis:** the behaviour, the evidence for the hypothesis, exclusions, decisions already made, and
   what would disprove it. Reference implementations are evidence, not truth.
4. **Files:** what the agent expects to change, recorded in `docs/waves/<id>.md`. Touching more is fine when the fix
   needs it; say so in the report. Only the lead edits the shared ledger and summaries.
5. **Report:** the headings in §5.

## 3. Shared machine, shared tree

These hold for everyone, lead included:

- **Preserve the dirty tree.** Inspect unfamiliar edits before touching them; never reset, revert or reformat other
  work; never stage or commit it. Edit with `apply_patch` or the editor tools; no scripts that rewrite `.rs`,
  `.toml` or `.md`.
- **One compiler-heavy command and one game/GPU workload at a time** on this machine (`--jobs 4`). Check before
  starting one; don't build while a game runs.
- **Use the normal paths.** Build with the normal CLI into `target/release` and the configured build folders; run
  games from the real library (`%LOCALAPPDATA%\nxrs`) and the launcher, which the owner has authorized. Don't make
  a full copy of a game, its data or its shaders per run.
- **Saves:** back them up before a run that may change them, and restore them when the run was an experiment.
- **Device loss or reset:** stop the faulting workload by PID and keep its log. **The fault becomes the top-priority
  bug** for whoever owns that title: diagnose it, fix it, and re-run that path as soon as a candidate fix exists.
  The only thing not to do is re-run the same path unchanged, since a blind retry just resets the driver again.
  Other titles and other work carry on meanwhile.
- **Blacklists** stay unless a specific corrected pipeline is shown safe; then remove that entry, not the list.
- Close only your own processes, by PID, never by image name.
- **Commit or push only when the owner asks.**

## 4. Verification

- **Agents run their own tests** (focused first, the affected package's suite before reporting) and their own game
  runs. `not run` is not a pass.
- **A game claim needs real evidence:** screenshots of the game window and input-response evidence over the declared
  stretch, plus the logs and metrics. A named screenshot, an FPS number or a synthetic test alone is not gameplay.
- **Diagnostics are never milestones.** A diagnosis, a test or an audit is a step inside a task; the task ends with a
  source correction and its game result, or with a reported blocker.
- **Audits are for genuinely dangerous changes** (guest-execution, memory lifetime, descriptor or shader-arithmetic
  changes that could corrupt state or hang the GPU), done by a fresh session **after a fix exists**. Not per
  diagnostic, not as a gate before every GPU run.
- **Evidence stays compact:** keep what proves the result and the last failure; delete duplicated candidates,
  superseded captures and stale target lanes.

## 5. Report (required headings)

1. **Built:** what changed and why it fixes the cause; diagnosis kept apart from correction.
2. **Tests:** what each asserts, whether it failed before the fix, and that it was run.
3. **Verified:** commands, results, and the game evidence looked at (paths).
4. **Unproven:** what is still open and the next step.
5. **Slips:** deviations from this protocol, or `None`.
6. **Questions:** only real product choices left after §6, with sources and a recommendation.

## 6. Resolve questions from sources first

Check, in order: the latest owner instructions; recorded decisions and the ledger; architecture and contracts;
current source and reproducible evidence; documented upstream behaviour and licensed references. Then:

- **Answered:** act.
- **Owed implementation:** do it; never ask whether to finish promised behaviour.
- **Missing evidence:** run the smallest useful experiment yourself.
- **Excluded by an owner decision:** keep the exclusion.
- **Real product choice:** ask, with sources and a recommendation.

Never answer a rendering bug by hiding work, weakening acceptance, faking a service result, forcing a completion
signal or enabling an unverified replacement.

## 7. Handoff

The lead updates the ledger after each report and acceptance. A handoff lists HEAD and the dirty-tree state, the
accepted changes, the build in use, the evidence paths, any open device-loss bug with its owner and evidence, and the next
concrete task. List running processes and active agents as they are; an `In progress` row implies nothing is running.
