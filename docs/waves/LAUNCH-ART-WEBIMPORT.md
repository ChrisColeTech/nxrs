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

## Expected files

`services/artwork_store.rs` (new), `services/web_import.rs` (new), `services/web_route.rs` (new, the request ->
file routing moved out of `web_reply.rs` plus the host route), `model/game.rs`, `model/game_json.rs`,
`flows/add.rs`, `flows/properties*.rs`, `flows/saves.rs`, `flows/start.rs`, `flows/web.rs`,
`services/web_serve.rs`, `services/web_reply.rs`, `services/build_facts.rs`, `constants/*`, tests under
`src/launcher/tests/`.
