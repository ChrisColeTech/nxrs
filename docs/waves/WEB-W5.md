# WEB-W5 — One servable web folder; Build for web and Play in browser in the launcher

Owner: one delegated agent. Lead: main session. Opened 2026-11-26. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.
Worktree `D:\Projects\RecompLauncherEngine-web-w5`, branch `web-w5`, base `790bbd8`. Title: **Beach Bounce
Remastered** (web build only, `<source folder>\nxrs-web\Beach Bounce Remastered`). GPU 0 (no native game runs in
this wave). Dev-server port 8773. Plan row: W5 of `docs/35-WEB-TARGET-PLAN.md` (§4.5, §5 W5).

## Goal

`nxrs-build build <nsp> --target web` writes one self-contained folder any COOP/COEP static server can serve; the
launcher builds it ("Build for web") and plays it in Chrome ("Play in browser") from its own in-process server.

## Plan

### 1. Output layout (nxrs-build)

- `src/cli/src/webhost.rs` (new): `HOST_FILES`, the one list of host files (`index.html`, `boot.js`, `input.js`,
  `saves.js`, `io-worker.js`, `audio-worklet.js`), each embedded with `include_str!` from `src/web`, so a release
  needs no `src/` tree. W4 adds `gpu-worker.js` by one line in this list; W6 its cold module the same way.
- `--target web` writes them into the output folder (only when changed), then `build.json`:

  ```json
  {"format": 1, "kind": "nxrs-web",
   "title": "<name>", "titleId": "<16 hex or null>",
   "engine": {"name": "nxrs", "contract": 2, "version": "<engine.json's version, else dev-<crate version>>"},
   "entry": "index.html",
   "data": "<name>_data",
   "files": [{"path": "...", "size": n, "role": "host|loader|module|manifest|bootset"}, ...]}
  ```

  The data folder stays where it is (`<name>_data`, beside the build) and is named, not copied. Roles leave room:
  W4's `gpu-worker.js` is another `host` file and its packs a `shaders` role; W6's cold module a `module` with its
  own path. Readers ignore roles they do not know.

### 2. Serving (launcher)

- `src/launcher/src/model/web_build.rs` (pure): a game's web folder (`<base>\nxrs-web\<name>` beside its
  `<base>\nxrs\<name>`), `build.json` read into `WebBuild`, the URL slug, a request path to a file inside the build
  (never outside it), `Range:` parsing (exact 206 / 416), content types.
- `src/launcher/src/services/web_serve.rs`: an HTTP/1.1 server on `127.0.0.1` in the launcher process (std threads,
  no Python, no new crate): GET/HEAD, keep-alive, COOP `same-origin` + COEP `require-corp` on every answer,
  `Accept-Ranges`, exact `206` + `Content-Range`, `416`, `application/wasm`, `/<slug>/data/<path>` mapped to the
  build's data folder from `build.json`. One server, every web build mounted under its own slug, one fixed port
  (`NXRS_WEB_PORT`, default 8780): the origin stays the same between runs, so Chrome's OPFS data cache and the
  saves (both per origin) survive. Port taken: the next ones (the cache then starts empty, said in the log).
- `dev_server.py` stays for engine developers and reads `build.json` (host files from the build folder; `--live`
  serves them from `src/web` instead, for editing JS without a rebuild).

### 3. Launcher

- `args::build_web`: `build <source> --outdir <web folder> -o <name> --jobs <n> --machine --target web [update]
  [--keys]` (contract v2, documented in `docs/LAUNCHER_CONTRACT.md`).
- Game menu and the game context menu: **Build for web** (enabled under the same conditions as Build, plus a built
  web folder does not block it: it rebuilds) and **Play in browser** (shown only when the web folder has a
  `build.json` and a Chromium browser was found).
- Browser: the default browser when it is Chromium-based (Windows: the `http` UserChoice ProgId — Chrome, Edge,
  Brave, Chromium, Vivaldi, Opera), else Chrome by path (App Paths, then the usual install folders). Linux:
  `google-chrome`, `chromium` on PATH. None found: Play in browser is not shown.
- Headless: `--build-web` (like `--build`), `--serve-web <folder>` (serves a web build, prints the URL, runs until
  closed; the verification path and an alternative to `dev_server.py`).

## Files expected

`src/cli/src/webhost.rs` (new), `src/cli/src/main.rs`, `src/cli/src/pipeline.rs`; `src/web/dev_server.py`;
`src/launcher/src/model/web_build.rs`, `model/http_range.rs` (new), `services/web_serve.rs` (new),
`services/browser.rs` (new), `services/platform/*` (the registry read), `services/toolchain.rs`, `services/build_facts.rs`,
`model/facts.rs`, `model/args.rs`, `flows/web.rs` (new), `flows/runs.rs`, `flows/build_finish.rs`,
`flows/action.rs`, `flows/dispatch.rs`, `flows/state.rs`, `ui/chrome/menu_model.rs`, `constants/` (words, port),
`cli/` (the two flags), `Cargo.toml` (windows-sys registry feature); `docs/LAUNCHER_CONTRACT.md`,
`docs/35-WEB-TARGET-PLAN.md` (W5 row), this document.

## Disproof first

Chrome refusing cross-origin isolation from the in-process server, or range reads failing through it, would
disprove the design. Checked first with `--serve-web` on the existing Beach Bounce web build (before the launcher UI).

## Notes

- Disproof check, done first: `nxrs --serve-web` (the launcher's server) served the Beach Bounce web build to Chrome
  158 at port 8773. The page was `crossOriginIsolated`. The boot set came cold through 1,115 range requests in 1.39 s,
  nnMain was reached at 4.30 s, and audio ran after a click (`build/WEB/W5/run1-serve-web`). The design holds.
- Midway, W4 was merged in (`055c41d`). `gpu.js` and `gpu-worker.js` joined `HOST_FILES`. The server now answers the
  page's development POSTs (`log`, `shot`) with 204 and does not keep them. Shader files under
  `<name>_data/shaders/` are served on first use like any other data file. dev_server.py's conflict was resolved so
  it keeps both build.json and W4's `--log` and `/shot`.
- The shared `build\tools\emsdk` (a junction) keeps its Emscripten cache keyed by the path it was reached by. One
  launcher build failed with `'bits/alltypes.h' file not found` after `old sanity: …web-w4b… new sanity: …web-w5…
  (config changed, clearing cache)`: two worktrees clear each other's cache. A retry built. This is an environment
  race and is not fixed here.
- `NXRS_BROWSER` (a development override, like `NXRS_BUILD`) let the verification open a separate Chrome profile
  instead of the owner's default browser.

## Report

1. **Built.** (a) `--target web` now writes the host files and `build.json` (`src/cli/src/webhost.rs`). The host
   files are the page, `boot.js`, `input.js`, `saves.js`, `io-worker.js`, `audio-worklet.js`, `gpu.js` and
   `gpu-worker.js`, embedded with `include_str!`. `build.json` lists every file with a role, so the folder is complete
   for any COOP/COEP static server. (b) The launcher has an in-process HTTP/1.1 server (`services/web_serve.rs`,
   `web_reply.rs`, with pure rules in `model/web_build.rs` and `model/http_range.rs`). (c) **Build for web** runs
   contract v2 with `--target web` into `<base>\nxrs-web\<name>` and leaves the native build and the library entry
   untouched. **Play in browser** is shown only with a finished web build and a Chromium browser. It starts the
   server once, mounts the build and opens a tab. Headless: `--build-web` and `--serve-web <folder>`.
   dev_server.py reads build.json (`--live` serves the page from `src/web`).
2. **Tests.** nxrs-cli: `host_files_are_the_pages_own` (the list matches what the page and its scripts load) and
   `build_json_describes_the_folder` (fields, roles, sizes, host files written only when changed): 34 passed.
   Launcher, `model::web_tests` (8): the web folder; `build_web` arguments (`--outdir <web>`, `--target web`, the
   rest as Build); build.json parsing and refusals; slugs; routes that would leave the build; exact ranges and 416;
   content types; Chromium ProgIds. `tests/web.rs` (3): the server's isolation headers, keep-alive, 206/416, HEAD,
   404 for traversal, 405, POST 204 with its body skipped, shader files, 301; `--serve-web` as a process; Build for
   web through the fake engine, then the menu, then Play in browser serving the data. `tests/ui.rs` was updated for
   the new Game menu entry. `cargo test` and `clippy -D warnings` are clean.
3. **Verified.** Beach Bounce was rebuilt with `--target web` into its normal `nxrs-web\Beach Bounce Remastered`.
   The launcher ran on a test library (`NXRS_DATA_DIR`, port 8773). Build for web was clicked and built (log ends
   "Web build complete"). Play in browser was clicked and opened `http://127.0.0.1:8773/beach-bounce-remastered/`,
   served by the launcher's process. Cold start (`?fresh=1`): `crossOriginIsolated` true; boot set 316.1 MiB in
   1.65 s; nnMain at 4.68 s; audio context running after a click; output RMS about -21 dBFS; GPU worker presents at
   60 FPS; no 5xx. Captures: `build/WEB/W5/shot-*.png`, `run2-launcher-play/`. The windows were closed by PID.
4. **Unproven.** (a) Beach Bounce's frames are black: its shaders are translated live (`dmsh_xlate`), and this
   title has no web shader files (W4's area). (b) One windowed run hung after Build for web while a PrintWindow
   capture ran mid-build, and Windows closed it. Later runs did not hang with captures taken before and after the
   build. The cause is not found; it may be PrintWindow with the wgpu surface. (c) Linux browser detection is not
   run here. (d) The server stops when the launcher closes, and so does the page's data streaming.
5. **Slips.** One manual `nxrs-build` run without `-o` wrote `nxrs-web\BeachBounce`. It was removed at once and
   rebuilt into the normal folder. `cargo fmt -p nxrs-cli` reformatted engine files it should not have (`cc.rs`,
   `pipeline.rs`). This was undone before the commit was final (amended).
6. **Questions.** The shared emsdk cache race between worktrees (Notes). Recommendation: give each worktree its own
   `EM_CACHE` in `web.ps1` and `cc.rs`, or one fixed spelling of the emsdk path.
