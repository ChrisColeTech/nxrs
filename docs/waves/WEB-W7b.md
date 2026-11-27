# WEB-W7b - Run-time shaders translated by the page's server

Wave of `docs/35-WEB-TARGET-PLAN.md` W7 (follows `WEB-W7`). Base `26ad1c9`, branch `web-w7b`, worktree
`D:\Projects\RecompLauncherEngine-web-w7b`. Title: Beach Bounce Remastered (01004060145C0000), web build
`<source folder>\nxrs-web\Beach Bounce Remastered` (`-o "Beach Bounce Remastered"`). GPU 0, port 8775. Protocol:
`docs/31-DELEGATION-PROTOCOL.md`. Parallel wave WEB-W6b owns the shader packs, the hot/cold split, the cache headers
and `web_reply.rs` / `web_file.rs`; this wave's edits there are kept to a dispatch line.

## Goal

A web build whose library lacks a shader the game makes at run time no longer skips its draws: the page asks its
server, the launcher's server translates it with the engine's own live translator, writes it into the build's
library the way a native run does, and answers with the SPIR-V. The web build stops depending on an earlier native
run (`--shaders-from`, WEB-W7) for titles such as Ren'Py's.

## Design (checked against the source)

The native live path (`graphics::vk::live::worker`) does not call `dmsh_xlate` bare: it calls
`shaders::prebuild::translate_one(ctl, code, <data>/shaders/live)`, which checks the stage, runs `dmsh_xlate`
through the build's fixes and validation (`spirv-val`), reads the profile's facts from `<data>/shaders/profile`, and
writes `<data>/shaders/live/<key>/` (`spv/<key>.spv`, `spv/<key>.json`, `manifest.tsv`, `pairs.tsv`). The next build
promotes those records into the library's `spv/` (`pipeline::promote_live_shaders`). The launcher reaches the engine
only through `nxrs-build` as a child process (AGENTS.md), so the server runs that same function through a new
`nxrs-build` subcommand, not `dmsh_xlate` on its own (that would skip the fixes, validation and reflection json).

1. **`nxrs-build shader-xlate <shaders folder>`** (contract v2, new): reads one shader from stdin
   (`u32 LE ctl length | ctl | code`, at most 4 MiB), runs `translate_one` into `<shaders folder>\live`, prints the
   key on success (exit 0), the reason on stderr (exit 1). Fixed arguments; nothing else runs.
2. **Route** `POST /<slug>/shader-xlate/<16 hex key>` on the launcher's server (`services/web_xlate.rs`, new):
   - the body is bounded (`Content-Length` at most 4 MiB, else `413` without reading it); a key that is not 16 hex
     digits, or a body shorter than its own ctl length, is `400`; an unknown slug `404`;
   - the only folder written is that mount's `<data>\shaders\live` (the key names the subfolder; the CLI writes it);
   - when `<live>\<key>\spv\<key>.spv` and `.json` exist already (another tab, a request after a reload) they are
     answered without translating; one translation per key at a time (others wait for it);
   - no translator (the mount has no `nxrs-build`): `503`; translation failed: `422` with the reason;
   - `200`: `u32 LE spv length | spv | json`.
   The mount carries the `nxrs-build` the launcher found (`Toolchain`: `NXRS_BUILD`, Settings, the release's
   `nxrs-engine\`, beside the launcher); `--serve-web` finds it the same way (no Settings: environment, bundled,
   beside).
3. **Web live path** (`vk/live.rs` web arm): a missing shader is queued as natively, on the same translator threads
   (`NXGFX_LIVE_JOBS`); each thread POSTs it (synchronous XHR on its own pthread, `nxgfx_web_post` in
   `src/web/nxgfx.lib.js`) and waits for the answer, as the native thread waits for `dmsh_xlate`. The answer goes
   into memory and the library picks it up from there (`vk::shader` load: a key the live translator was asked for
   is not fetched again from the server while it waits); the draw waits for it through the native `wait_for`
   (3 s per set, 1.5 s per frame). A key not in the library's `spv/` is also looked for in `live/<key>/spv/` on the
   server (what an earlier page translated) before it is asked for: a warm run translates nothing.
4. **Static host** (no route: 404/405, or an empty 204 from an older server): the key is marked failed and the red
   line says the build's server cannot translate shaders and what to do (serve it with the launcher, or
   `--shaders-from`): today's behaviour, the draws skipped and counted.
5. **dev_server.py**: the same route, running `nxrs-build` (`--engine`, else `NXRS_BUILD`, else the repository's
   `target\release\nxrs-build.exe`).

## Disproof first

If the translation in a child `nxrs-build` per shader is too slow for the 3 s draw wait, or `translate_one` needs
something only a game run has, the design fails; measured first with a recorded shader input (the ctl/code of a key
from the native build's live folder is not kept, so the input is recorded from the page's first POST).

## Files expected

| File | Change |
|---|---|
| `src/cli/src/shaderxlate.rs` (new), `src/cli/src/main.rs` | the `shader-xlate` subcommand, with tests |
| `src/graphics/src/vk/live.rs` | web arm: POST through the translator threads, answers kept in memory; body/answer codec with tests |
| `src/graphics/src/vk/shader.rs` | two hooks in `Library::load` (web): the live translator's answers, `live/<key>/spv/` |
| `src/graphics/src/web/mod.rs`, `web/ring.rs`, `src/web/nxgfx.lib.js` | `post`: synchronous POST on the calling thread |
| `src/launcher/src/services/web_xlate.rs` (new), `model/shader_xlate.rs` (new) | the route; its pure rules |
| `src/launcher/src/services/web_serve.rs` | `Mount::translator` |
| `src/launcher/src/services/web_reply.rs` (W6b's: one dispatch) | POSTs to the route go to `web_xlate` |
| `src/launcher/src/model/http_range.rs` | reason phrases 400/413/422/503 |
| `src/launcher/src/flows/web.rs`, `cli/serve.rs` | the mount's translator |
| `src/launcher/src/constants/web.rs`, `words_web.rs` | the route's words and limits |
| `src/launcher/tests/web_xlate.rs` (new) | limits, path safety, end to end with the real `nxrs-build` |
| `src/web/dev_server.py` | the same route |
| `docs/LAUNCHER_CONTRACT.md`, `docs/35-WEB-TARGET-PLAN.md` | `shader-xlate`, the route; the W7 row |

## Verification

- Tests: the route's limits and path safety (`413`, `400`, `404`, `503`, the folder written); end to end: a real
  recorded shader input POSTed to the server running the real `nxrs-build` (opt-in, `NXRS_XLATE_EXE`), SPIR-V back
  and found in `live\<key>\spv`; `cargo test --release -p graphics`, `-p nxrs-cli`, the launcher's `cargo test` and
  clippy.
- Chrome: Beach Bounce built for the web without `--shaders-from` and its library's imported run-time shaders
  moved out (kept and put back after); served by `nxrs --serve-web` on 8775; the 16 shaders translated on request,
  title and story scenes against `build/WEB/W7/run7-validate`; translations counted and timed; a warm run (same
  profile, server restarted) translates nothing. Evidence `D:\Projects\RecompLauncherEngine\build\WEB\W7b\`.
- Native: the fast engine tests; live.rs's native path unchanged (the same `translate_one` call on the same threads).

## Results (2026-11-26)

Evidence in the main checkout's `build/WEB/W7b/` (Chrome 158 over CDP, port 8775; X = A every 4.1 s, shots at the
WEB-W7 run7 steps). The web build was made without `--shaders-from` (`build-web.log`: `shaders: 0 found`, 18.6 s) and
the 16 run-time shaders WEB-W7 had imported were moved out of its `shaders\spv\` first (`imported-spv-backup\`, put
back afterwards and checked by hash).

| Run | Server | Translated | Result |
|---|---|---|---|
| `run1-cold` (new profile) | `nxrs --serve-web` | **16 by the server, 0.12-0.20 s each** (`serve.out`); in the page, each 0.12-0.20 s and 0.1-0.2 s after it was missed (`page.log` at 3.96-4.61 s engine time) | title menu, Grandma Umi (min 1), Tomoyo in the hospital (min 2), Yuuki at the desk (min 3): the same as `build/WEB/W7/run7-validate`; **0 red lines, 0 draws skipped for a shader** (213 replay lines) |
| `run2-warm` (same profile, server restarted) | `nxrs --serve-web` | **0**: no POST (`net.json`), no server line; 16 `live shader ...: an earlier translation found on the page's server` | the same four scenes; 0 red lines, 0 skipped |
| `run3-devserver` (new profile, 25 s, `live\` set aside) | `dev_server.py --log` | 16 (0.41-0.73 s each: dev_server.py translates one at a time), 16 requests kept as `xlate-<key>.bin` | the recorded inputs of the launcher's real-engine test |

- **Same output as native:** all 16 `live\<key>\spv\<key>.spv` and `.json` the server wrote are byte-identical to the
  native build's `live\` records of the same keys (`cmp`).
- **Real-engine test:** `NXRS_XLATE_EXE=...\nxrs-build.exe NXRS_XLATE_INPUT=build\WEB\W7b\run3-devserver\xlate-0a56e5906042ca31.bin
  cargo test --test web_xlate`: 200, SPIR-V magic, the answer equal to the library's file, manifest row; 2.55 s (first
  translation in a fresh folder, `e2e-test.txt`).
- The disproof did not hold: a child `nxrs-build` per shader takes 0.12-0.20 s, well inside the 3 s draw wait.
- CDP saw 12 of the 16 POSTs (worker requests are not all reported to it); the server's lines are the count.

Not verified in Chrome: the static-host red line (a server without the route); the route's 404/405/204 answers are
mapped to it in `live::translate` and the server's 503 is tested.
