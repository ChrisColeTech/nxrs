# LAUNCH-ART-WEBIMPORT - artwork owned by the launcher; importing an existing web build

Wave branch `launch-art` (worktree `.claude/worktrees/agent-a8e0b5ef41b06b708`), base `gameplay-harness` 3e0c449.
Launcher only (`src/launcher`). No titles, no GPU for games except one short Chrome run of the GTA V web port on GPU 1.

## Plan

1. **Artwork belongs to the launcher.** Every path that sets a cover or background (Add game, Properties' Apply,
   Import build) copies the image into `<data folder>\artwork\<entry id>-<cover|background>.<ext>` and stores that
   path. Replacing art replaces the file; removing an entry removes its copies. At startup an idempotent step copies
   every entry's external artwork in and repoints it (art already the entry's own copy is left; a missing source is left
   as it is and logged, never blanked); `library.json` is copied to `library.pre-artwork.json` before that step's first
   write (the `library.pre-layout.json` convention).
2. **Import build takes a web build.** The one Import build action (menu, `--import-build`, drag and drop) also takes:
   an nxrs web build (its folder, `build.json` or page: merged into the library entry with the same title id, which
   then offers Native and Web, else a new entry), and a foreign web game (a page, or a folder with `index.html` at most
   three levels down). The entry keeps the build's path under a new game key `webBuild` (written only when set).
   Play serves it from the launcher's own web server and opens it the way nxrs web builds are opened. A foreign game is
   served at the root of its own origin, `http://<slug>.localhost:<port>/` (its pages use root-absolute paths such as
   `/b/8b0b5899ed/loader.js`), with the same COOP/COEP, ranges and ETags.

## Built

**Artwork** (`services/artwork_store.rs`, `flows/artwork.rs`; `constants/engine.rs` `ARTWORK_FOLDER`, `ART_KINDS`,
`PRE_ARTWORK_BACKUP`). Cause: `cover`/`background` stored the path the image was picked from. Now Add game
(`add_game`), Properties' Apply (`apply_properties`) and Import build (`import_build`, the cover read from the
build's data folder) call `own_artwork`. It copies the image to `<folder of library.json>\artwork\<id>-<kind>.<ext>`
(atomic write; the extension kept; the entry's other copies of that kind removed) and repoints the entry. A
cleared kind loses its copy. An image that can't be copied keeps its path, with a console line. Remove from library
deletes the entry's copies (`flows/saves.rs`). At the window's start (`cli/window.rs`), `own_all_artwork` does the
same for every entry. It is idempotent: the entry's own copy is left, and missing originals are left as they are
and logged to the console and the startup log, never blanked. Before the first repoint, library.json is copied to
`library.pre-artwork.json` (the `library.pre-layout.json` convention). Without that backup nothing is repointed.

**Web import** (`services/web_import.rs`, `flows/web_import.rs`, `services/web_route.rs`, `model/web_build.rs`
`imported_page`/`page_parts`/`site_url`, `services/web_serve.rs` `can_serve`/`open_site`/`mount_site`). The one
Import build action takes the new paths (no new menu entry); so do `--import-build` and drag and drop. Its picker is
titled "Import build" and filters `exe, html, htm, json`.
- **nxrs web build** (a folder with `build.json` + `game.wasm`, picked as the folder, its `build.json` or its page).
  It joins the entry whose native data has the same title id. That entry then has both builds, so Play shows the
  existing Native/Web Choices box. With no matching entry, a new entry is titled from `build.json`.
- **Foreign web game** (a page, or a folder with `index.html` at most 3 levels down, the shallowest level only).
  It becomes a new entry. The new game key `webBuild` is written only when set, so other entries keep the Qt format.
  - **What starts GTA:** `mirror/playgta5.com/index.html` sends every visit without `?mode=story|sandbox` to
    `launcher.html`, the port's own settings menu. `game.html` does the same.
  - **Direct start rule:** `constants/web.rs` `DIRECT_STARTS` holds one rule, written for this port's `index.html`:
    if `index.html` contains the marker `mode === 'story'` and the folder has `game.html`, the entry's page becomes
    `game.html?mode=story`. Importing the folder again moves an older entry of the same site to that page.
  - **Serving:** Play serves the game at the root of its own origin, `http://<slug>.localhost:8780/` plus the
    query. GTA's pages load root-absolute paths (`/b/8b0b5899ed/loader.js`, `/data/…`), so it can't live under
    `/<slug>/`. Chrome resolves `*.localhost` to this computer as a secure context, so the game can be
    cross-origin isolated. The server picks the mount by the `Host` header. It uses the same COOP/COEP/CORP headers,
    ranges and ETags as nxrs builds. More content types: webp, mp4, m4a, woff, svg and others.
- **Card and status:** a game whose only build is a web build now reads "Ready to play in the browser" with no
  attention badge, format "WEB BUILD". Before, it showed the Needs-build pencil and "Build needed · NATIVE BUILD".
  Its Build is disabled with the existing "no source" reason.
- **Properties:** Apply no longer requires a build folder for an entry that has only an imported web build.

**GTA's own server (`launch.py`) is not needed to play.** It isn't in `P:\Games\Web\GTA5`
(`grandv-assets.json` lists no .py or .exe). Its endpoints all have fallbacks in GTA's own code:
- `POST /data/batch` and `GET data/batchc/*.bin`: boot-set prefetch only; a failure is logged and the files are
  read on demand (`io_worker.js` `prefetch`).
- `/userdata/<profile>` (GET/PUT/DELETE): the disk save mirror. Saves commit if IndexedDB or the disk works
  (`userdata_persistence.js` `restore`/`commit`), so they live in the browser, per origin.
- `/relay` (multiplayer, a "future release"), `/__local/port` (only with a `launch.py` token), `POST /log`
  (answered 204).

## Tests

All run on the launcher's suite (`cargo test` in `src/launcher`: every binary ok); `cargo clippy --all-targets -- -D
warnings` clean; `cargo fmt` applied. New behaviour, so each failed before this wave:
- `tests/artwork.rs`:
  - The start step copies outside art into `artwork/`, writes `library.pre-artwork.json` with the old paths and
    saves the repoint. A missing image stays as it was and is reported. Deleting the original leaves the copy. A
    second run copies nothing.
  - Properties Apply copies cover and background. Replacing with another format removes the old copy. Choosing the
    entry's own copy leaves it. Remove from library deletes the copies.
  - Import build copies the build's `icon.png` in.
- `tests/web_import.rs`:
  - A GTA-shaped folder (site two levels down) imports as "GTA5". Play can only go Web; Build can't. The card shows
    Play with no attention badge, the status reads "Ready to play in the browser", the format "WEB BUILD".
    Re-import selects the same entry; `webBuild` is saved.
  - On the server: the page at `http://gta5.localhost:<port>/` has COOP/COEP; `/b/x.js?v=1` is served; a range on
    `/data/f.bin` answers an exact 206. `..` and a request on another host get 404.
  - The direct game link: folder and `index.html` both find `game.html?mode=story`, and an older `index.html` entry
    is moved to it. The address is `/?mode=story`, which serves the game page; `launcher.html` still serves for
    GTA's own Return to launcher.
  - An nxrs web build, picked by its page, joins its game's entry by title id, and Play opens the Native/Web
    Choices box.
  - An empty folder is refused, a dropped site folder imports, and a folder with two pages at one depth is refused.

## Verified

Evidence in `build/LAUNCH-ART/` (worktree). The launcher ran from this worktree's `target/debug/nxrs.exe`, with
`NXRS_DATA_DIR` pointed at a scratch copy of the owner's `%LOCALAPPDATA%\nxrs\library.json` (18 games). It was
captured with PrintWindow and closed by PID.
- **Start step:** "Artwork: 32 images copied into …\data\artwork" (15 entries with cover and background, 2 with an
  `icon_AmericanEnglish.dat` cover, 1 with none), and `library.pre-artwork.json` was written.
  - `shot-1-library.png`: every card and the canvas background load from the artwork folder.
  - A second start logged no artwork line (idempotent).
- **GTA imported:** `--import-build P:\Games\Web\GTA5` added "GTA5" (19 games).
  - `shot-3-gta-selected.png`: the card shows Play with no badge, and the status reads "GTA5 · Ready to play in the
    browser · WEB BUILD".
  - `shot-5-status.png`: "Opened in browser.cmd: http://gta5.localhost:8780/". `browser.cmd` is the
    `NXRS_BROWSER` wrapper that starts Chrome under the run rules below.
- **GTA from the launcher's Play:** Chrome 158 with `--use-adapter-luid=0,113326` and its own profile. Before each
  run there was no other Chrome with `--use-adapter-luid` or `--remote-debugging-port`. Screenshots were at least
  20 s apart, and Chrome was closed by PID.
  - Run 1, the page was still `index.html`: it reached GTA's own launcher (`run1-1-gta-own-launcher.png`,
    cross-origin isolated). Pressing GTA's Play went to `game.html?mode=story` and in-game story mode
    (`run1-5-ingame.png`: Michael in the bank, minimap, about 29 FPS on the counter).
  - Run 2, after the direct-start fix: Play opened `http://gta5.localhost:8780/?mode=story`. That loaded GTA's
    intro screen at 22 s (`run2-1-intro.png`) and in-game story mode by 100 s (`run2-4-ingame.png`), with no click
    in the page.
- **The owner's library:** it was not written by this wave. At the start, the owner's
  `D:\Projects\RecompLauncherEngine\target\release\nxrs.exe` (PID 31044) was running. It exited during the wave,
  and library.json was rewritten at 11:55 by something other than this wave: it has no `webBuild` key, no
  `/artwork/` paths and no `library.pre-artwork.json`. That binary predates this branch. The copy-in, with its
  backup, happens at the first start of a launcher built from this branch after the merge.

## Unproven

- The real library's start step has not run (above). It is tested on a copy of that library (32 images).
- One launcher process crashed once: egui-wgpu "Failed to create staging buffer for index data"
  (`egui-wgpu-0.37.0\src\renderer.rs:1033`, in `%TEMP%\nxrs-startup.log`). It happened when a mis-scaled synthetic
  mouse click from a DPI-unaware script landed near the window. It is not in code this wave changed, and it did not
  happen again in two more runs. Reported, not fixed.
- The disk save mirror (`/userdata/` PUT/GET) isn't served, so GTA's saves live in the browser's storage for
  `gta5.localhost:8780`. The saves in `P:\Games\Web\GTA5\userdata` (written by GTA's own server at
  `localhost:8008`) are not seen at the new origin. Serving them would mean writing under `P:\`, which this wave
  must not do. That is an owner decision.
- `DIRECT_STARTS` knows one site's routing (story mode). Sandbox mode, or GTA's launcher settings (graphics,
  stored per origin), are reached through the game's "Return to launcher".

## Slips

None.

## Expected files

`services/artwork_store.rs` (new), `services/web_import.rs` (new), `services/web_route.rs` (new, the request ->
file routing moved out of `web_reply.rs` plus the host route), `model/game.rs`, `model/game_json.rs`,
`flows/add.rs`, `flows/properties*.rs`, `flows/saves.rs`, `flows/start.rs`, `flows/web.rs`,
`services/web_serve.rs`, `services/web_reply.rs`, `services/build_facts.rs`, `constants/*`, tests under
`src/launcher/tests/`.
