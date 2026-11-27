# WEB-W5b — One Play, one Build: each asks Native or Web

Owner: one delegated agent. Lead: main session. Opened 2026-11-27. Protocol: `docs/31-DELEGATION-PROTOCOL.md`.
Worktree `D:\Projects\RecompLauncherEngine-web-w5b`, branch `web-w5b`, base `404d0a7`. UI only: no engine or
server change. Serving checks on port 8781 (`NXRS_WEB_PORT`). Check title: Anime Girls Sun of a Beach
(`0100C6801E0FA000`, has a web build).

## Goal

Owner, 2026-11-27: "its supposed to be one play button, one build button and when you press it, it displays a
dialog to select either native or web. it should not be two play buttons, two build buttons." W5 added separate
"Play in browser" and "Build for web" entries; they go, and Play and Build each open a Native / Web chooser.

## Plan

- `flows/action.rs`: `Play` and `Build` open the chooser; its answers are `PlayNative`, `PlayWeb`, `BuildNative`,
  `BuildWeb` (the old direct actions: `play`, `play_in_browser`, `start_build`, `start_web_build`, unchanged).
- `flows/surface.rs`: `Surface::Target { game_id, build }`, a modal child window like the message boxes.
- `flows/target.rs` (new): `why_not(id, build, web)`, the one rule for whether each target can run now and the
  one-line reason when it cannot (no native build yet; no web build yet; no Chromium browser found; running;
  building; the native build is up to date; and the existing build checks). `ask_target` opens the chooser.
- `ui/chrome/menu_enabled.rs`: Play enabled when either Play target can run; Build when either Build target can
  (the old Build or Build for web enablement). `menu_model.rs`: one Play, one Build in the Game and context
  menus; the web entries removed.
- Card primary, Enter/double-click, Ctrl+B and the Updates window's Build now: through the chooser. A card whose
  game has no native build but a playable web build offers Play.
- `ui/windows/target.rs` (new): title "Play — <game>" / "Build — <game>", two choice tiles (Native, Web) with a
  one-line description, or the reason when disabled; the footer's ghost Cancel. The family grammar: no filled
  primary (choosing a tile is the commit), card-button tile styling.
- `constants/words_target.rs` (new): the chooser's words; `words_web` loses `BUILD_WEB`/`PLAY_WEB`.
- Docs: `LAUNCHER_CONTRACT.md` web section, the W5 row of `35-WEB-TARGET-PLAN.md`.

## Verification

- `cargo test` and `cargo clippy --all-targets -- -D warnings` in `src/launcher`; tests: one Play and one Build per
  menu, no web entries; Play/Build open the chooser; the chooser's reasons; Web/Native answers run the old paths.
- The worktree's launcher, captured with PrintWindow: Game menu, context menu, card, the Play chooser (Anime Girls:
  Web enabled; a title without a web build: Web disabled with its reason), the Build chooser.
- End to end: Play -> Web for Anime Girls opens Chrome on the launcher's server (port 8781), page past the title.
- Evidence: `build/WEB/W5b`.

## Results

(filled at the end)
