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

The dialog design changed four times during the wave on the owner's word (tiles + Cancel; tiles + one primary;
two filled buttons; question box with Web as a ghost lead verb); what shipped is the guide's new **Choices** form:

- One Play and one Build in the Game menu, the context menu, the card, Enter/double-click, Ctrl+B and Updates'
  Build now; the W5 web entries are gone. `App::why_not(id, build, web)` (`flows/target.rs`) is the one rule.
- **Play** runs the one version that can play (native: `play`; web: `play_in_browser`) and asks only when both can.
  Pressed before the startup tools check ended, Play finishes the check first (`refresh_tools_now`): before that
  fix, a Play at startup on Anime Girls saw no browser and ran native without asking.
- **Build** always asks; a version that cannot build is disabled, its reason the tooltip (a built game's Native:
  "The native build is up to date.", the existing enablement).
- The box: `Surface::Choices` drawn by `message::choices` — window title Play / Build (`words_contexts`), heading
  "Play <game> natively or on the web?" / "Build <game> natively or for the web?", the text explaining both, the
  footer Native then Web as equal `button::choice` buttons at the trailing end (`authored::choices_at_bottom`),
  none filled; Enter chooses nothing; X / Escape dismiss. `--show-window play|build` opens it for captures.
- Docs: guide §5 Choices bullet + footer row + filled-primary bullet + Buttons table; `LAUNCHER_CONTRACT.md` web
  section; `35-WEB-TARGET-PLAN.md` W5 row; captures `docs/images/dialogs/play.png`, `build.png`.

Evidence (`build/WEB/W5b`): `game-menu.png`, `play-choices-anime.png`, `build-choices-anime.png`,
`chrome-play-web.png` + `chrome-play-web-later.png` (Play > Web for Anime Girls: Chrome at
`127.0.0.1:8781/anime-girls-sun-of-a-beach/`, title menu reached), `play-native-only.txt` (card Play on Beach
Bounce, native only: no dialog, the game started, then stopped by PID).
