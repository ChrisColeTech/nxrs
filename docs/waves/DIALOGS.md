# DIALOGS - one dialog scaffold, dismissal only in the title bar

Branch `dialogs` (worktree `D:\Projects\RecompLauncherEngine-dialogs`), base `gameplay-harness` 4b9e524. Launcher only.

## Why

The owner rejected BUILD-LAYOUT send-back 2 (3f7e772), verbatim: "why do we have so many redundant close and cancel
buttons in the footers. we already have a close button in the title bar. and second the icons are still not
consistent on the dialogs. some of them only have the icon and the text, missing headers. and some of these images
arent even of dialogs, some are just popups or menus." Mid-wave, on the first after captures: "why do the icons have
outlines. the red ion and green outline looks bad." Then: "some of the dialogs are mising primary buttons. you
removed cancel/close but didnt add ok."

## Rules (written into `docs/ARCHITECTURE-AND-STYLE-GUIDE.md` §5)

1. **Dismissal is the header's X and Escape only.** No footer has Close, Cancel, No, Yes or Done. The X closes a
   window, discards a Draft (Settings restores the previewed appearance, as before), and answers a question No.
   **Every dialog ends with exactly one filled primary**, last: Apply, Add game, Add DLC…, Add update…, a
   question's verb, a message's OK.
2. **One body header.** Every dialog body starts with `page::heading`: the 32 pt icon tile, the bold 16 pt heading,
   the info mark where an explanation exists. Group headings in rail windows, DLC and Updates already used it;
   messages and questions now do too (tiles `i`, `!`, `?`). Tile glyphs are bare line glyphs, no ring; an error tile
   is the error red throughout (glyph, fill, edge), never red inside the green-grey edge.
3. **Messages:** window title = the task or place the message belongs to, never the app's name (send-back 1
   below); heading = the message itself; text under it; footer = one filled OK; OK, Enter, X and Escape close. **Questions:** window title names the action; heading = the question; text and the tick box
   under it; one filled button named with its verb (Remove; Stop build and close). Enter answers the default as
   before.
4. **Dialogs vs popovers.** Dialogs are the child windows. Search & Filter and Build tools are popovers (Live,
   in-window), menus are menus, the build output is a panel: their own grammar, not counted as dialogs. Their
   captures moved to `docs/images/popovers/`.

## Inventory (before = 3f7e772, after = this wave)

| Dialog | Footer before | Footer after | Header before | Header after |
|---|---|---|---|---|
| About | ghost Close | filled **OK** | ringed `i` + text, no heading | `i` tile + heading "nxrs"; window "About nxrs" |
| Error message (e.g. Cannot add game) and every other `App::message` (load/save failed, cannot start/import/install, busy, no folder/file, keys installed...) | ghost Close | filled **OK** | ringed `!` (red) / `i` + text | red `!` tile (red edge) / `i` tile + the message as heading; window titled by its task (table below) |
| Remove from library (not built / built with tick box) | ghost No, filled Yes | filled **Remove** | ringed `?` + "Remove “Kart” from the library?" as body text | `?` tile + that question as heading; "The game's package is never deleted." and the tick box under it |
| Close during a build | ghost No, filled Yes | filled **Stop build and close** | ringed `?` + text | `?` tile + "Cancel the build and close the launcher?" heading |
| DLC | ghost Remove (lead), ghost Close, filled Add DLC… | ghost Remove (lead), filled Add DLC… | DLC tile + heading + info mark | unchanged |
| Updates | ghost Remove, ghost Build now (lead), ghost Close, filled Add update… | ghost Remove, ghost Build now (lead), filled Add update… | Updates tile + heading + info mark | unchanged |
| Properties (6 tabs) | ghost Cancel, filled Apply | filled Apply | rail + Group tiles/headings | unchanged |
| Add game (4 tabs) | ghost Cancel, filled Add game | filled Add game | rail + Group tiles/headings | unchanged |
| Settings (5 tabs) | ghost Cancel, filled Apply | filled Apply | rail + Group tiles/headings | unchanged |
| Install keys | native file picker, then a message | (message: as above) | - | - |

Not dialogs (unchanged, captures moved): Search & Filter popover (ghost Clear filters, X), Build tools popover
(ghost Open Settings, X), the title-bar menus, the build output panel.

## Send-back 1 (owner via lead): message windows named by their task

"Message boxes must not all be titled nxrs." `App::message(context, title, text, error)`: `context` (from
`constants/words_contexts.rs`) titles the window, `title` is the heading. Every message box, window title → heading:

| Where | Window title | Heading |
|---|---|---|
| Help > About | About nxrs | nxrs |
| Add a package / folder (`import_source`, `add_game`) | Add game | Cannot add game · Already in library |
| Add game window's Add with missing details | Add game | Missing details |
| File > Import build | Import build | Cannot import build · Game is busy |
| Build start | Build | Cannot start build |
| Play | Play | Source unavailable · Cannot save game settings · Could not launch |
| Stop | Stop | Could not close game |
| Settings' Apply | Settings | Build in progress · Engine not found · Cannot save game settings (layout / play settings) |
| Properties' Apply | Properties | Missing details · Cannot save game settings |
| Keys > Install keys | Keys | Keys installed · Invalid key file · Cannot install keys |
| DLC (outside the DLC window) | DLC | Cannot install DLC · Game is busy · DLC already installed · DLC added · Cannot remove DLC · DLC not installed · DLC removed · DLC could not be installed · DLC installed |
| Updates (outside the Updates window) | Updates | Cannot install the update · Game is busy · Update already installed · Update added · Update could not be added · Cannot remove the update · Update not installed · Update removed · Version chosen |
| Library load / save | Library | Library could not be loaded · Could not save library |
| Remove from library, deleting the build | Remove from library | Cannot delete the build |
| Game > Open build folder / save folder | Build folder / Save folder | Folder not available |
| Game > View game log | Game log | File not available |

A DLC or Updates outcome with an empty title (shown only in their windows' status lines in practice) takes its
context as the heading, so no box is ever without one.

Captures of the message boxes a flag can open (`build/dialogs/msg`, copied to `docs/images/dialogs/`): `about.png`
(About nxrs / nxrs), `error-add.png` (Add game / Cannot add game, `--show-add <non-game file>`), `error-import.png`
(Import build / Cannot import build, `--import-build <non-game file>`), `error-library.png` (Library / Library could
not be loaded, a library file that is not JSON). All four read. **These four are at 1x**: the machine's display was
at 96 DPI (1352 x 848) during this send-back, not the 3x of the first pass; PrintWindow captured the real windows
at the scale the system gave. "Cannot start build" could not be captured: `--build` runs headless without a window.
The other message boxes are the same `message::message` body and need a game state (a running game, a busy build)
that no flag opens.

## Code

- `ui/popups/authored.rs`: `footer_bar` has no dismissal slot; `footer(ui, p, primary, enabled) -> bool`.
- `ui/widgets/page.rs`: `heading(ui, icon, title, tip, ink)`, the one body header (`group` calls it); wraps long
  headings; an `ink` tints the tile's glyph, fill and edge.
- `ui/widgets/rail.rs`: `Icon::Info`, `Icon::Error`, `Icon::Question` (bare glyphs).
- `ui/windows/message.rs`: rewritten on the heading; a message's footer is its filled OK, a question's its verb.
- `flows/surface.rs`: `Ask { title, heading, text, verb }` in `Surface::Question`; `App::question(ask, ..)`,
  `question_checked(ask, ..)`; `App::ask_close` (shared by the close request and the capture opener).
- `constants`: `REMOVE_HEADING` / `REMOVE_TEXT` / `REMOVE_VERB`, `CLOSE_VERB`; `CANCEL`, `CLOSE`, `YES`, `NO`
  removed from `words_settings` (nothing uses them); `OK` is the message's button. About's heading is the menu's "About nxrs".
- `properties.rs`, `settings.rs`, `list_window.rs`, `dlc.rs`, `updates.rs`: footer dismissal removed.
- CLI: `--show-window close-build` opens the close-during-build question (a capture; before this it could not be
  captured without a running build).

## Captures

PrintWindow at 3x of each dialog's own OS window, debug `nxrs.exe` from this worktree, `NXRS_DATA_DIR` pointed at a
fixture in `build/dialogs/fixture` (three games, Kart with a 1.5 MB fake build; not the owner's library), each
process closed by PID. No `nxrs-build` was configured, so the engine readouts say it is missing.

- Dialogs: `docs/images/dialogs/` - about, error-add, error-import, error-library (these four re-captured in
  send-back 1, at 1x), remove-built, remove-unbuilt, close-build, dlc, updates,
  add-game, props-0..5, settings-0..4.
- Popovers: `docs/images/popovers/` - filters, tool (the main window with the popover open).

## Tests

In `src/launcher`: `cargo fmt --check` clean, `cargo clippy --all-targets -- -D warnings` clean, `cargo test` 71
passed, 0 failed (13 suites; after send-back 1).

- `tests/ui_scaffold.rs` (new or rewritten through the real shell): `a_message_has_the_header_and_one_filled_ok`
  (window "About nxrs", no "Close nxrs"; no Cancel/No/Yes/Done and only the caption's Close; OK closes; the X
  closes); `an_error_is_titled_by_its_task_and_headed_by_itself` (a non-game file added: window "Add game",
  heading "Cannot add game", OK closes);
  `a_question_has_one_verb_and_its_x_answers_no` (heading is the question, "Stop build and close" present, X closes
  without `closing`); `rail_windows_end_with_their_commit_only` (Settings, Properties: Apply, no Cancel);
  `a_list_window_has_the_shared_footer_and_its_empty_state` (DLC: Remove, Add DLC…, no Close); the filter popover
  test unchanged. Not run against 3f7e772; by construction they would fail there (its footers had Close / Cancel /
  No / Yes).
- `tests/ui_dialogs.rs`: Remove from library clicks **Remove** (was Yes), and asserts the heading and that no
  Yes / No exist.
- `tests/ui.rs` `properties_list_and_remove_dlc`: no Cancel; the header's X discards the Draft (was Cancel).

## Slips

- Send-back 1: one `sed -i` on `src/launcher/tests/ui_scaffold.rs` (the module comment's second line) instead of
  the Edit tool; its overlong result was then rewrapped with Edit. Only that comment changed.

## Unproven

- Light mode was not captured (the error tile's red edge is derived from the error token and should hold there).
- The engine-present Settings > Engine page (doctor checks list) was not captured: no `nxrs-build` in the fixture.
