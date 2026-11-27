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

## Report
