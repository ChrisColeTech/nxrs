# nxrs launcher in Rust + egui: rewrite plan

Status: plan only, no code. Written 2026-11-20 from the Qt launcher at `D:\Projects\RecompLauncher` (`main`, `622d130`,
release `0.5.7`) and the engine at `D:\Projects\RecompLauncherEngine` (`main`, `88b6f54`).

The Qt launcher's **source** is the spec (owner rule: port from source, not docs). Every row of section 1 names the
file and function it comes from; the port is done when every row is ticked with a test or a screenshot. Where the
source and `docs/LAUNCHER.md` disagree, the source wins, and the few places where the source looks like a bug are
listed in 1.13 for the owner to rule on, not silently "fixed" or silently kept.

Owner decisions this plan needs are marked **[OWNER]** and collected in section 10.

---

## 1. Feature inventory (the parity checklist)

Paths are relative to `D:\Projects\RecompLauncher`. `lib` = `src/launcher/library.cpp`, `win` = `window.cpp`,
`bm` = `buildmanager.cpp`, `art` = `artwork.cpp`, `tb` = `titlebar.cpp`, `pw` = `processwatch.cpp`.

### 1.1 Library data (`library.h`, `lib`)

| ID | Feature | Source |
|---|---|---|
| D-01 | `library.json` at `QStandardPaths::AppLocalDataLocation` with an empty organization: `%LOCALAPPDATA%\nxrs\library.json` (Linux `~/.local/share/nxrs/library.json`); `--library <file>` overrides | `main.cpp` `main` (setOrganizationName(""), setApplicationName("nxrs")) |
| D-02 | One-time copy of the legacy data folders `RecompLauncher/RecompLauncher`, then `RecompLauncher/SwitchRecompLauncher`, into `nxrs` when `nxrs/library.json` does not exist; old folders untouched; skipped with `--library` | `main.cpp` `migrateLegacyData` |
| D-03 | Load: missing file = empty library; unreadable or invalid JSON or `schema != 1` = error message, and the library is marked unwritable (never overwritten) | `Library::load`, `Library::save` (`writable`) |
| D-04 | Top-level keys and clamps: `toolRoot`, `toolRootChosen`, `outputRoot`, `keysPath`, `nxrsBuild`, `jobs` (1..64, 8), `viewMode` (0..1), `coverWidth` (130..240, 180), `showBackground` (true), `colorMode` (dark/light, else dark), `accentColor` (lime), `buttonLayout` (normalized, else `switch`), `pauseOnFocusLoss`, `muteOnFocusLoss` (false), `docked` (true), `askUser` (false), `geometry`, `dockState` (Qt base64 blobs) | `Library::load` / `save` |
| D-05 | Game keys: `id`, `title`, `source`, `buildDir`, `outputName`, `cover`, `background`, `renderer` (default vulkan; anything but vulkan/opengl becomes opengl), `buttonLayout` / `screenMode` (normalized, aliases accepted), `workers` (0..64), `optimization` (0..3, 2), `engine` (written `nxrs`; another value or none loads as nxrs and moves the entry to an nxrs folder of its own, `<base>\nxrs\<name>`, G-08), `builtSource`, `savesOffered` (read and written back unchanged), `hintsBuilt` (>= 0), `buildIncomplete`, `lastPlayed` / `lastBuilt` (Qt ISO local time, `""` when unset), `updates[] {path,titleId,version,display}` and `dlc[]` (deduplicated with `samePath`, empty entries dropped), `updateChoice`, `builtUpdate` | `Game::json`, `Game::fromJson` |
| D-06 | Load-time repairs: `adoptBuiltUpdate` per game (a build whose `romfs.nsp` marker names an update); a library without `screenMode` gets `handheld` for a built game whose INI says `docked=0`; entries without id or title, or with a duplicate id, are skipped; a per-game `pauseOnFocusLoss` (from 382ff33) is ignored and dropped at the next save | `Library::load` |
| D-07 | Atomic save (QSaveFile), creates the folder; keys sorted (QJsonObject order), 4-space indent | `Library::save` |
| D-08 | Add refuses a duplicate source or executable; `importBuild` merges into an unbuilt entry of the same source (keeps title, cover, build options) or appends; repeated import of the same exe is a no-op | `Library::add`, `Library::importBuild` |
| D-09 | `discoverBuilds(folder)`: imports every `*_code.dll/.so` build in a folder and its subfolders (no UI entry; tested API) | `Library::discoverBuilds` |
| D-10 | Installed keys found at startup: `keysPath` empty and `%LOCALAPPDATA%\nxrs\keys\prod.keys` exists -> used | `Window::Window` |
| D-11 | `--root` is accepted for older scripts and has no effect; `toolRoot` / `toolRootChosen` are kept in the file | `resolveToolRoot`, `Toolchain::discoverRoot` |

### 1.2 Games, sources, builds (`lib`)

| ID | Feature | Source |
|---|---|---|
| G-01 | Accepted sources: folder with `main` / `exefs/main` / `ExeFS/main`; `.nup` (`NUP\1`), `.nsp` (`PFS0`), `.nro` (`NRO0` at 0x10), `.xci` (`HEAD` at 0x100 or 0x1100); exact error texts | `validSource` |
| G-02 | New entry: UUID id, title from the file/folder name with `[...]` removed, ASCII output name (NFKD, letters/digits/`-_ `, else `Game`), build folder `<source folder or outputRoot>\nxrs\<name>` (owner, 2026-11-26; was `SwitchRecomp Builds\<name> [nxrs]`) | `gameFromSource`, `titleForSource`, `outputNameFor` |
| G-03 | Format labels: DIRECT NUP / NSP / XCI, HOMEBREW, EXTRACTED, NATIVE BUILD | `Game::format` |
| G-04 | Display version from `<data>\control.nacp` at 0x3060 (digit-first only) | `Game::version` |
| G-05 | "Has a build": not incomplete, exe + `<name>_code.dll/.so` + `<name>_data\` exist, built source = source (or no source), built update = chosen update; `updatePending` = built but with another update | `Game::hasBuild`, `updatePending`, `builtFiles`, `sameToken` |
| G-06 | Import a native build: needs exe + code module + data; source from `romfs.nup` (line 1) or `romfs.nsp` (`SWRNSP1/2`, line 2); applied update from `SWRNSP2` `update` row; engine from `<name>_build*\gen\nxrs.gen.key`; renderer from INI; cover from `icon.jpg` / `icon.png` / `icon_AmericanEnglish.dat` in data | `gameFromExecutable`, `sourceOfBuild`, `builtWithNxrs`, `adoptBuiltUpdate` |
| G-07 | Path equality: clean absolute, `/` and `\` equal, case-insensitive on Windows | `samePath` |
| G-08 | Build folder: `model::build_layout` (`<base>\nxrs\<name>`); an entry loaded from another engine moves to `nxrs_folder_for` its old folder and reads back what that folder has built; entries in the old layout (`SwitchRecomp Builds\<name> [nxrs]`) are migrated only when asked, by `--migrate-builds` or Settings > Build defaults > Old build folders > Move, never at window start (renamed, `buildDir` updated, running / in-use / taken skipped and reported, `library.pre-layout.json` first; LAUNCHER_CONTRACT.md v2) | `setGameEngine`, `buildFolderFor` |
| G-09 | A package that holds game + DLC: the first build from it puts the package on the game's DLC list (never re-adds after the user removed it) | `adoptPackageDlc`, `BuildManager::reportsPackageDlc` |
| G-10 | "Rebuild for full speed": `<data>\discovered.tsv` non-comment lines minus `hintsBuilt`, nxrs games only | `discoveredCount`, `newDiscoveries` |

### 1.3 Engines, toolchain, contract (`lib`, `bm`)

| ID | Feature | Source |
|---|---|---|
| E-01 | nxrs-build lookup: `NXRS_BUILD` (file or folder), Settings / `--nxrs-build` choice, `<app dir>\nxrs-engine\` | `Toolchain::findNxrs` |
| E-02 | Contract: `engine.json` beside nxrs-build (`contract`, `version`), else `nxrs-build contract` (10 s) parsing `nxrs-contract <n>`; needs 2 | `findNxrs`, `nxrsContractNeeded` |
| E-03 | nxrs problems text (missing, old contract); **on non-Windows nxrs is refused outright** (see 1.13) | `Toolchain::nxrsProblem` |
| E-04 | Python engine toolchain discovery (Python, gcc, runtime exe, dotnet): not ported, the launcher runs no Python engine | `Toolchain::find`, `pythonTool`, `isReleaseEngine`, `recompCli` |
| E-05 | Python engine problems text: not ported | `Toolchain::problem` |
| E-06 | Child environment: the launcher's own plus `NXRS_PROD_KEYS` when keys are installed | `Toolchain::environment` |
| E-07 | .NET 10 check: `dotnet --list-runtimes` async, 10 s, `Microsoft.NETCore.App 10.` | `Window::refreshTools` |
| E-08 | v1 (the legacy command line) arguments: not ported; nxrs-build still answers them (docs/LAUNCHER_CONTRACT.md) | `BuildManager::arguments`, `updateInfoArguments`, `dlcSyncArguments` |
| E-09 | v2 (nxrs) arguments: `build <src> --outdir -o --jobs <game or default, 1..64> --machine [--no-update/--update] [--keys]`; `update-info --game <src> [--keys] <pkg>...`; `dlc sync --datadir <data> [--keys] <pkg>...`; `--keys` only when the file exists | `nxrsBuildArguments`, `nxrsUpdateInfoArguments`, `nxrsDlcSyncArguments` |
| E-10 | Process: program = nxrs-build; working dir = its folder; stdout+stderr merged; `PYTHONUTF8=1`, `PYTHONUNBUFFERED=1`; Linux new session | `BuildManager::start`, `program`, `engineFolder` |
| E-11 | Doctor: `nxrs-build doctor`, `ok ` / `FAIL ` / note lines, `@` lines skipped, error when no checks; 60 s; tick/cross text | `parseNxrsDoctor`, `runNxrsDoctor`, `NxrsDoctor::text` |
| E-12 | Engine badge: ready when nxrs-build can build; its problem text | `refreshTools`, `toolProblem` |
| E-13 | Health text (nxrs-build path + contract + version, Keys), refreshed live in Settings | `refreshTools`, `healthText` |

### 1.4 Build (`bm`, `win`)

| ID | Feature | Source |
|---|---|---|
| B-01 | Preconditions in order: one build at a time; XCI needs nxrs; engine can build; NSP/XCI need prod.keys; source valid; chosen update exists and keys present; build folder creatable | `BuildManager::start` |
| B-02 | Start: output name re-normalized; effective controller layout resolved into the copy; play settings captured; `buildIncomplete = true` saved; console cleared, "Source: / Output:" header; dock shown and titled | `Window::startBuild` |
| B-03 | `<buildDir>\<name>.build.log` truncated, header `nxrs build · <ISO>` / `SwitchRecomp build · <ISO>`, every byte appended, final detail line | `BuildManager::start`, `readOutput`, `complete` |
| B-04 | Line handling: split on `\n`, trimmed, UTF-8; `@` lines not shown; `@progress\t<stage>\t<done>\t<total>` mapped to labels (open, analyze, translate, compile, link, shaders, data); v1 regex `(?:C generated:\|compiled)\s+(\d+)/(\d+)`, `Analyzing`, `Linking`, `RomFS:`; `DLC: this package also holds add-on content` | `parseLine` |
| B-05 | Progress shown on the card (cover strip), in the status bar (phase n/m, bar) and in the card's button tooltip | `Window` progress connection, `GameCard::setBuildProgress` |
| B-06 | Finish: cancelled text, failure "exit N" text, success requires the files (`hasBuild`), then writes the game's INI settings; texts exact | `BuildManager` finished lambda |
| B-07 | After success: adopt package DLC, `builtSource`, `builtUpdate`, `lastBuilt`, `hintsBuilt`; DLC sync when the game has DLC (`--build` waits for it) | `Window` finished connection |
| B-08 | Cancel: Windows `taskkill /PID <pid> /T /F`; Linux SIGTERM to the group, SIGKILL after 4 s; hard kill after 5 s; "Stopping build processes" | `BuildManager::cancel` |
| B-09 | Close during a build: confirm, cancel, close when it ends | `Window::closeEvent` |

### 1.5 Play, stop, watch (`win`, `pw`)

| ID | Feature | Source |
|---|---|---|
| P-01 | Play guards: has build, not running/building; in-place package offline message | `Window::play` |
| P-02 | DLC up to date before Play, else sync first, then Play (`DlcPlay`) | `play`, `dlcInstaller` finished |
| P-03 | One-time save-copy offer after an engine switch: not ported (`savesOffered` kept in the file) | `play` |
| P-04 | Writes renderer, effective button layout and play settings into the INI, then starts the exe detached (cwd = build dir, toolchain env); records pid and `lastPlayed`; "Game launched" | `play`, `writeGameSettings` |
| P-05 | Running detection every 2.5 s by full exe path (Windows Toolhelp + `QueryFullProcessImageNameW`; Linux `/proc/*/exe`), so games started outside the launcher count | `findRunningGames`, poll timer |
| P-06 | On a game's exit with new discoveries: status line "rebuild for full speed (n new functions...)" | poll timer |
| P-07 | Stop: re-checks the pid's exe path, then `WM_CLOSE` to its visible windows (Linux `pidfd_send_signal SIGTERM`); never force-kills | `requestGameClose`, `Window::stop` |
| P-08 | Enter / double-click: Play when built, else Properties > Build; card cover double-click: Play or Properties > General | `activateGame`, `GameCard` |

### 1.6 DLC (`win`, `dlcpanel.cpp`, `lib`)

| ID | Feature | Source |
|---|---|---|
| L-01 | `dlc.manifest` lines `<id16> <ver> [data\|stub] <package>`, comments ignored | `readDlcManifest` |
| L-02 | Package check: nsp/nsz (`PFS0`), xci/xcz (`HEAD`), nca (>= 0xC00); texts | `validDlcPackage` |
| L-03 | `dlcUpToDate`, `adoptInstalledDlc` (installed but unlisted joins the list), descriptions ("— <ids>", "not installed yet", "installs when the game is built", "file not found", "(no data)") | `lib` |
| L-04 | DLC window: list (multi-select), Add DLC… (multi-file picker, start dir), drop files, Remove (button, Delete, context menu "Remove n packages"), Open containing folder, Close; busy bar; status line (error red); locked when running/building | `DlcDialog` |
| L-05 | Install flow: validate, dedupe, add to list, sync when built else "installed when built"; command line all-or-nothing; engine refusals remove the package from the list (data folder is truth); outcome texts; reasons `DlcInstalled/Built/Play/Changed` | `installDlcPackages`, `removeDlcPackages`, `startDlcSync`, `dlcReport`, finished handler |
| L-06 | Properties > DLC page: same list, Remove applied on OK, then sync | `DlcPanel`, `showProperties` |

### 1.7 Updates (`win`, `updatepanel.cpp`, `lib`)

| ID | Feature | Source |
|---|---|---|
| U-01 | Package check: nsp (`PFS0`), xci (`HEAD`), nca; nsz/xcz refused as compressed | `validUpdatePackage` |
| U-02 | Probe through the engine (`UPDATE\t...` / `UPDATE error:`), keys and the game's own package required; found updates merged by path | `UpdateProbe`, `parseUpdateInfo` |
| U-03 | Choice: newest (default), a package, or `none`; removal of the chosen one falls back to newest; labels ("1.1.1 (v262144)", "the game without updates", "the game as its package is") | `Game::activeUpdate`, `updateToken`, `updateTokenLabel`, `updateDescription` |
| U-04 | Updates window: Version to build combo, list (multi-select, word-wrapped), Add update…, drop, Remove, context menu (Build with this update / Remove n / Open containing folder), pending text, Build now (enabled only when a rebuild is pending), Close; busy; status; locked | `UpdateDialog` |
| U-05 | Outcome texts ("Build the game again to get …", "The current build already is …") | `updateOutcome`, `updateReport` |
| U-06 | Properties > Updates page: remove on OK | `UpdatePanel`, `showProperties` |

### 1.8 Saves (`lib`, `win`)

| ID | Feature | Source |
|---|---|---|
| S-01 | Save copy between engines' builds (plan, copy, Game menu item, `--copy-saves`): not ported, there is one engine | `saveCopyPlan`, `copySavesFromOtherEngine`, `Window::copySaves` |

### 1.9 INI writing (`lib`)

| ID | Feature | Source |
|---|---|---|
| I-01 | `writeIniKey`: replaces every `key=` line anywhere, else inserts right after `[section]`, else appends `[section]` + key + blank line; keeps every other line, comment and the file's line ending (CRLF if present); atomic | `writeIniKey` |
| I-02 | Keys: `[Graphics] renderer`, `[Controls] button_layout`, `pause_on_focus_loss`, `mute_on_focus_loss`, `ask_user`, `[Display] docked` (+ `[Other] settings_version=2` when the INI has no `settings_version=`) | `writeRenderer`, `writeButtonLayout`, `writePlaySettings`, `writeDocked` |
| I-03 | Readers with engine defaults (renderer opengl, docked true, the rest false) | `readRenderer`, `readDocked`, … |
| I-04 | Effective values: game override else launcher default (layout, screen mode); written at build success, at Play, and immediately to every built game when Settings or Properties change them | `effectiveButtonLayout`, `effectiveDocked`, `writeGameSettings`, `showSettings`, `showProperties` |

### 1.10 Screens and controls (`win`, `art`, `tb`, `filterpopup.cpp`, `theme.cpp`)

| ID | Feature | Source |
|---|---|---|
| W-01 | Client-drawn title bar (36 px): accent app glyph, menus File/Game/Keys/View/Help in the caption row, themed min/max/close (close red `#c42b1c` / `#b3261e`), double-click maximize, right-click system menu, Windows Snap Layouts on the maximize button (`HTMAXBUTTON`), native resize borders and shadow, maximized top inset; Linux frameless with system move and 6 px edge resize | `TitleBar`, `CaptionButton`, `handleNativeEvent`, `EdgeResizer` |
| W-02 | Menus. File: Add game (Ctrl+O), Add extracted folder…, Import build…, Settings, Exit (Alt+F4). Game: Play, Stop/Cancel build, Build/Rebuild (Ctrl+B), Install DLC…, Install Updates…, Remove from library…. Keys: Install keys…, Open keys folder. View: Gallery, List, Search & Filter (Ctrl+F), Show selected game background, Refresh library (F5), View console (Ctrl+L), Build output toggle. Help: About nxrs | `createMenusAndToolbar`, `createActions` |
| W-03 | Commands strip: Add game split button (Add game / Add extracted folder / Import build), Search & Filter with active count | `createMenusAndToolbar` |
| W-04 | Gallery: wrapping grid, card width 130..240 (slider), card height `w*0.64+78`; cover crop-to-fill, rounded 5; placeholder gamepad + "Add artwork" / "Image unavailable"; attention badge (pencil for Needs build, `!` otherwise); build progress strip; title footer (semibold, selected tint); primary button (Play/Build/Rebuild/Stop/Cancel build with painted icon) + Properties button | `GameCard`, `CoverView`, `localIcon` |
| W-05 | List: columns Name, Status, Format, Renderer, Last played (`yyyy-MM-dd HH:mm` / `—`), Build folder; sortable; alternating rows; 32 px rows; sort preset kept in step with the filter popup | `createLibrary`, `refreshLibrary` |
| W-06 | Statuses: card (`cardState`) and list (`statusFor`: Preview, Building, Running, Source offline, Update changed — build again, Rebuild for full speed, Ready to play, Build needed) | `cardState`, `statusFor` |
| W-07 | Context menu on a game (gallery and list): Play, Build, Properties, Open build folder, Open save folder, View game log, Remove from library… | `showContextMenu` |
| W-08 | Selection: single, kept across refresh, first game when lost; selection line "title · format · vX · renderer" with source tooltip | `selectGame`, `refreshSelection` |
| W-09 | Background canvas: selected game's background (else cover), opacity .32 dark / .20 light, top-to-bottom fade; toggle | `LibraryCanvas` |
| W-10 | Empty states: "Your library is empty …" / "No games match these filters …" | `refreshLibrary` |
| W-11 | Search & Filter popover: Search, Status (All / Ready / Needs build / Running), Source (All / NUP-NSP / NRO / Extracted / Native builds), Sort (Name, Last played, Build status, "Custom table order"), Gallery/List, Card size (disabled in list), "x of y games shown", Clear filters (one change signal); the shared header's X, Esc and Enter close (no Done: a Live popover commits nothing, 2026-11-26); anchored under its button, clamped to the screen | `FilterPopup`, `LibraryFilter::matches`, `activeCount` |
| W-12 | Status bar: game count ("n games" / "x / y games"), progress bar (determinate or busy), selection line, job line, engine badge (click: popup with problem + health + Open Settings, Esc closes), Settings button, size grip | `createStatusBar`, `showToolPopup` |
| W-13 | Build output dock: bottom or right, hide/undock, title follows state, Open log, Clear view, 12000-line cap, placeholder, opens at 190 px | `createBuildDock`, `showBuildOutput` |
| W-14 | Drag & drop onto the window: native builds import, anything else goes through Add | `dropEvent` |
| W-15 | Shortcuts: Ctrl+O, Ctrl+B, Ctrl+F, Ctrl+L, Alt+Enter, Enter (gallery/list), F5 (clear artwork cache + tools + library), Delete (DLC/update lists), Esc (popups) | `createActions`, `Window::Window` |
| W-16 | Window: 1280x800 default, min 920x600; geometry and dock state saved on close | `Window::Window`, `closeEvent` |
| W-17 | Dark native caption via DWM attribute 20 | `Window::Window`, `setAppearance` |
| W-18 | Theme: dark/light x 5 accents (lime, mint, violet, rose, cyan), exact colour tokens; no focus rectangles; live preview, Cancel restores | `Theme::colorsFor`, `stylesheet`, `showSettings` |
| W-19 | App icon (exe resource + window icon), title glyph in the accent | `app.rc`, `make-icon.py`, `Theme::accentGlyph` |
| W-20 | Artwork loading: cache by path+size+mtime, EXIF orientation, downscale to 2048, decode errors shown in tooltip and refused in Properties | `loadArtwork`, `invalidateArtwork`, `editGame` |
| W-21 | Accessibility names: gallery, list, cards ("title, status"), card buttons ("Play <title>", "Properties for <title>"), progress bars, caption buttons (Restore/Maximize), filter fields | throughout |
| W-22 | Demo mode: six fake games, nothing saved, actions disabled, no process scan | `seedDemo`, `--demo` |

No artwork is fetched from the network: covers and backgrounds are local paths only (W-20, G-06). The port adds no
fetching.

### 1.11 Dialogs (`win`)

| ID | Feature | Source |
|---|---|---|
| X-01 | Add game / Properties (670x400): General (Display name, Source + browse + Choose extracted folder…, Format, Version, Status, note), Build (Build folder, Executable name read-only unless adding, Compiler workers 0 = default), Runtime (Graphics backend fixed "Vulkan", Controller layout "Use default — X", Screen mode "Use default — TV mode/Handheld", engine-dependent note, Open game settings file), Artwork (cover, background, note), DLC, Updates; Build and Runtime locked while running/building; OK checks title, folder and image decodability | `editGame` |
| X-02 | Properties apply: field copy, update removal resets a stale choice, screen mode and layout written to a built game's INI now, artwork cache cleared, DLC removal synced | `showProperties` |
| X-03 | Settings (690x440): Engine (nxrs-build, health text, nxrs checks with Check again run on open, Open setup guide in a checkout), Build defaults (Build location, Compiler workers 1..64), Controller (Button layout + note), Appearance (Color mode, Accent with swatches, background toggle, library file path), Play (Screen mode, Pause, Mute, Ask who is playing, note); OK rules (no engine/build changes during a build, empty = bundled, nxrs-build must exist, layout/play changes written to every built game with a failure count) | `showSettings` |
| X-04 | Install keys: picker for `prod.keys`, must contain `header_key`/`header_key_source`, atomic copy to `<data>\keys\prod.keys`, texts | `installKeys` |
| X-05 | Remove from library: confirmation with the tick box *Also delete the build (N GB)*, ticked by default (owner, 2026-11-26). Ticked: saves, SD card, cheats and mods moved to `<data>\saves\<title id>`, then the build folder deleted (refused while running or building); the next build of the game puts them back. Unticked: files untouched. The package is never deleted | `removeGame` |
| X-06 | About text; message boxes (info/warning) and question boxes with exact texts | `createMenusAndToolbar`, `message` |
| X-07 | Open build folder / save folder / game log / build log / INI, with "not available" messages | `openFolder`, `openGameFile`, dock toolbar |

### 1.12 Command line (`main.cpp`)

Each flag keeps its name, value and exit code; scripts (`smoke-release.py`, `test_launcher_workflow.py`, the
engine's docs) depend on them.

| Flag | Behaviour / exit |
|---|---|
| `--help`, `--version` | Qt standard; version from `VERSION` |
| `--root <dir>` | kept for older scripts; no effect |
| `--library <file>` | library file; skips legacy migration |
| `--import-build <exe>` | import after the window opens |
| `--demo` | non-persistent preview library |
| `--list-view`, `--show-console`, `--show-filters`, `--show-appearance`, `--show-controller`, `--show-engine-settings`, `--show-play-settings`, `--show-updates`, `--show-dlc`, `--show-window remove\|about\|build-settings\|tool`, `--show-add <package>` | open that view (the last two: captures of every dialog) |
| `--show-properties` + `--properties-tab <0..5>` | Properties on a tab |
| `--show-menu <File\|Game\|Keys\|View\|Help>` | pop a title-bar menu (after 300 ms) |
| `--preview-theme <mode>`, `--preview-accent <id>` | appearance without saving |
| `--select-title <title>` | select by display title (case-insensitive) |
| `--engine nxrs` | saved engine for the selection (only `nxrs`); exit 2 when invalid or the game is busy |
| `--nxrs-build <exe>` | saved nxrs-build path (empty = bundled) |
| `--build` | build the selection, exit 0/1 after the build (and its DLC sync) |
| `--install-dlc <pkg>` (repeatable), `--remove-dlc <pkg>` | headless DLC actions; `DLC: …` / `DLC error: …` lines; exit 0/1 |
| `--install-update <pkg>` (repeatable), `--remove-update <pkg>`, `--update-choice newest\|none\|<pkg>` | headless update actions; `Updates: …` lines; exit 0/1 |
| `--nxrs-doctor` | prints the checks; exit 0 all ok, else 1 |
| `--smoke-build <input>` + `--build-output <dir>` | import + build a synthetic input; exit 2 without `--build-output` or `--library`; 0/1 |
| `--check-image <file>` | decode test; exit 0/1 |
| `--screenshot <png>` + `--screenshot-delay <ms>` (1100) | grab the window, or the open dialog/popup composited over it; max 1600 px wide; exit 0/1 |

### 1.13 Inconsistencies found in the source (decide, do not guess) [OWNER]

1. **`hintsAtStart` is never set.** `win:874` has `builder.hintsAtStart=discoveredCount(...)` inside the end-of-line
   comment of the previous statement, so every build stores `hintsBuilt = 0` and "Rebuild for full speed" never clears
   after the rebuild. The docs (`LAUNCHER.md` "Rebuild for full speed") state the intent. Recommend: port the intent.
2. **nxrs refused on Linux.** `Toolchain::nxrsProblem` returns "builds Windows games only" on every non-Windows build,
   while releases ship `nxrs-engine/` for Linux and the contract doc says Windows and Linux. Port as is, or allow nxrs
   on Linux (no engine change needed).
3. **"Source offline" differs.** The list status checks `.nup` only (`statusFor`), the card checks every in-place
   package (`cardState`). Recommend: the card's rule for both.
4. **Dead action.** "Build options…" (`buildOptionsAction`) is created and enabled but placed in no menu. Recommend:
   not ported (nothing reaches it).

---

## 2. Architecture in Rust / egui

### 2.1 Crates (versions checked on crates.io 2026-11-20)

| Need | Crate | Notes |
|---|---|---|
| Window + UI | `eframe` 0.37, `egui` 0.37 | MSRV 1.95 (engine CI pins 1.99). Defaults kept (accesskit, wgpu, x11, wayland, fonts); `persistence` off: all state lives in `library.json` |
| Tables | `egui_extras` 0.37 | `TableBuilder` for the list view (resizable columns, virtual rows) |
| Images | `image` 0.25.10, `default-features = false`, `png`, `jpeg`, `webp`, `bmp`, `gif`, `rayon` | format guessed from content: `icon_AmericanEnglish.dat` is a JPEG |
| File dialogs | `rfd` 0.18 | Windows common dialogs; Linux `xdg-portal` (default; no GTK dev packages to build) |
| JSON | `serde` 1, `serde_json` 1 | default `BTreeMap` objects = Qt's sorted keys |
| Dates | `chrono` 0.4.45 | `NaiveDateTime` "yyyy-MM-ddTHH:mm:ss" local, as Qt writes |
| Data folder | `dirs` 7 | `data_local_dir()/nxrs` = Qt's AppLocalDataLocation on both platforms |
| Ids | `uuid` 1 (v4) | lowercase hyphenated, no braces, as `QUuid::WithoutBraces` |
| Names | `unicode-normalization` 0.1 | NFKD for `outputNameFor` (the engine's cli uses the same crate) |
| Small parsers | `regex` 1 | progress, contract, title brackets |
| CLI | `clap` 4 | `ArgAction::Append` for the repeatable flags |
| Win32 | `windows-sys` 0.61 (engine's version) | Toolhelp, `QueryFullProcessImageNameW`, `EnumWindows`/`PostMessageW`, job objects, `SetWindowSubclass`, DWM, `GetDpiForWindow` |
| Linux | `libc` 0.2 | `pidfd_open`/`pidfd_send_signal`, `kill(-pgid)` |
| HWND | `raw-window-handle` 0.6 | for the title-bar subclass |
| Open in shell | `open` 5 | `QDesktopServices::openUrl` |
| Exe icon | `embed-resource` 3 (build dep) | `app.ico` + manifest |
| Tests | `egui_kittest` 0.37, `tempfile` 3 | UI harness (accesskit queries), temp dirs |

Not used, on purpose: `sysinfo` (a direct port of `processwatch.cpp` is ~120 lines and matches the source exactly),
`notify` (the Qt app polls, it does not watch files), an INI crate (`writeIniKey` must keep every byte it does not
own; a port of its 25 lines is the spec), `egui_dock` (one resizable panel suffices), an HTTP client (nothing is
fetched).

### 2.2 Layout

```
src/launcher/
  Cargo.toml           own [workspace] (like src/oracle), package nxrs-launcher, bins `nxrs` and `fake-engine`
  build.rs             Windows: app.ico + manifest; NXRS_VERSION from env or VERSION
  assets/              app.ico, app.png (from the Qt repo's src/launcher/icons)
  src/
    main.rs            argument parsing, headless verbs, eframe start
    cli.rs             clap definitions -> Command values; exit codes
    core/              no UI, no threads: the ported library.cpp
      model.rs         Game, GameUpdate, DlcTitle, PlaySettings, CardState, LibraryFilter
      library.rs       load/save (unknown keys kept), add, import_build, discover_builds, legacy migration
      paths.rs         same_path, suffixes, data dir
      sources.rs       valid_source, game_from_source/executable, markers, nacp version, output/title names
      ini.rs           write_ini_key, readers, write_game_settings / play settings
      engine.rs        engine choice, build folders, v1/v2 arguments, UPDATE parsing, labels
      dlc.rs  saves.rs  doctor.rs  status.rs (card_state, status_for, filter, sort)  keys.rs
      toolchain.rs     nxrs lookup + contract, problems, child environment
    jobs/              one module per engine job, all on worker threads
      runner.rs        spawn, merged pipe, line splitter, log file, cancel (job object / process group)
      build.rs  update_probe.rs  dlc_sync.rs  doctor.rs  tools_check.rs (toolchain + dotnet, off the UI thread)
    process/           mod.rs, windows.rs, linux.rs: find running games, request close, launch detached
    app/
      mod.rs           App state, event drain, the Action enum and its single dispatcher
      theme.rs fonts.rs icons.rs (painted glyphs: commandIcon, localIcon, accentGlyph)
      titlebar.rs titlebar_win.rs gallery.rs card.rs table.rs canvas.rs artwork.rs
      filter.rs statusbar.rs console.rs dnd.rs screenshot.rs
      dialogs/ properties.rs settings.rs dlc.rs updates.rs message.rs
    bin/fake_engine.rs answers contract / doctor / update-info / dlc sync / -u … like tests.cpp main()
  tests/               integration tests (process, CLI, kittest UI)
```

Every user command is one `Action` value (`Play(id)`, `Build(id)`, `ShowProperties(id, tab)`, …) handled by one
dispatcher, as the Qt `QAction`s are: menus, context menus, shortcuts, card buttons and CLI flags all produce
`Action`s. Enabled/label state is computed once per frame from the same function the Qt `refreshSelection` is.

Code conventions: no per-file attribution or licence headers (credits in the engine README and `LICENSES/` only);
user-visible strings copied from the Qt source.

### 2.3 nxrs-build: spawn the CLI (recommended) or link the engine

**Recommendation: spawn `nxrs-build` per contract v2, exactly as the Qt launcher does. Link nothing from the engine.**

- The contract is the tested boundary: arguments, `@progress`/`@result`, `UPDATE`/`DLC error` lines and exit codes
  are verified and both launchers can share it. Linking would bypass it and fork the behaviour.
- nxrs-build is found at run time (`NXRS_BUILD`, Settings, bundle) so a developer points the launcher at any
  `target\<lane>\release\nxrs-build.exe`. Linking pins one engine into the launcher binary and removes that feature.
- A build is long, spawns clang many times and can crash; in a child process a crash or a cancel (kill the tree) never
  takes the UI down. In-process, cancel needs cooperative cancellation through the translator and shaders crates.
- Linking `cli` pulls `translator`, `shaders`, `profiles`, `ash`; the launcher would then build only where the engine
  builds (glslang, LLVM-mingw setup), slowing its CI and coupling every engine commit to it.

Linking would buy typed progress and no output parsing; the parser is 30 lines and already specified.

### 2.4 Long builds without blocking the UI

1. `jobs::runner` builds a `std::process::Command` (program, args, cwd, env from `toolchain`), with
   `std::io::pipe()` (stable since Rust 1.87): the writer is cloned into stdout and stderr, giving Qt's merged channel.
   Windows: `creation_flags(CREATE_NO_WINDOW)` (the launcher is a GUI app; without it every nxrs-build or Python opens
   a console) and the child is put in a **job object** so Cancel kills the whole tree (Qt: `taskkill /PID /T /F`;
   same result, no helper process; never by image name). Linux: `process_group(0)` then `kill(-pgid, SIGTERM)`,
   `SIGKILL` after 4 s (Qt's timings).
2. A reader thread owns the pipe: appends raw bytes to `<name>.build.log`, splits on `\n`, trims, decodes UTF-8
   (lossy), parses each line (`@progress`, v1 regexes, the DLC marker) and sends `JobEvent::{Line, Progress, Dlc}`
   over an `mpsc` channel, then `ctx.request_repaint()`.
3. A waiter thread `wait()`s the child and sends `JobEvent::Exited(code, signal)` after the reader drained, with a
   short grace period in case a grandchild still holds the pipe.
4. `App::update` drains the channels every frame (bounded per frame so a burst of lines never stalls rendering), applies
   the same state changes as the Qt slots, saves the library at the same moments.
5. The console keeps a 12000-line ring buffer and draws only visible rows (`ScrollArea::show_rows`), sticking to the
   bottom unless the user scrolled up.
6. Update probe, DLC sync, doctor, `nxrs-build contract`, Python and `dotnet` probes run the same way. Toolchain
   discovery moves off the UI thread (Qt runs the Python probe and `contract` synchronously on it).

### 2.5 Launching and watching games

- Launch: `Command` on `<buildDir>\<name>.exe`, cwd = build dir, the toolchain environment, the same creation flags as
  Qt's `QProcess::startDetached` (check `qprocess_win.cpp` of Qt 6.8.3 in phase 2; it uses `CREATE_NO_WINDOW` when the
  parent has no console), so the game survives the launcher; Linux `process_group(0)`. The `Child` is dropped without
  waiting; the pid is kept in `running`.
- Watch: a worker snapshots processes every 2.5 s (`CreateToolhelp32Snapshot` filtered by exe name, then
  `QueryFullProcessImageNameW` full-path compare; Linux `/proc/<pid>/exe`) and sends the map; the UI diffs it, reports
  new discoveries for exited games, refreshes. `request_repaint_after(2.5 s)` keeps idle CPU near zero.
- Stop: re-check the pid's exe path, then post `WM_CLOSE` to its visible top-level windows (Linux: `pidfd_open` +
  `pidfd_send_signal(SIGTERM)` after the path check). No force-kill.

---

## 3. Data compatibility and migration

**Goal: the Rust launcher opens the existing `%LOCALAPPDATA%\nxrs\library.json` as is, and the Qt launcher still
opens what the Rust launcher wrote.**

- Same path rules (D-01), same `schema: 1`, same keys, clamps and repairs (D-04..D-06), same refusal to overwrite an
  unreadable file (D-03), same atomic write (temp file + rename in the same folder).
- Formats: dates as Qt writes them (`2026-11-20T15:10:03`, local, no offset; `""` when unset); `version` as a JSON
  number (Qt writes it through a double; integers stay integral); empty strings where Qt writes them; keys sorted;
  4-space indent. A golden-file test compares the Rust output with a Qt-written fixture.
- `geometry` and `dockState` are Qt `saveGeometry`/`saveState` blobs. The Rust launcher keeps them byte-for-byte
  (round-trip, never rewritten) and stores its own placement under new keys (`window`, `consolePanel`).
- Unknown keys (top level and per game) are kept and written back. Note the asymmetry: the Qt launcher rebuilds the
  object and drops keys it does not know, so Rust-only keys must be optional with defaults; going back to Qt for a day
  costs only the Rust window placement.
- INI files, build folders, saves, `keys\prod.keys`, `dlc.manifest`, markers: read and written exactly as today (1.9).
- Legacy folder migration (D-02) is ported unchanged.

Migration story:

1. No user step. A release replaces `nxrs.exe`; the data folder, library, keys and builds stay.
2. Safety copy: on its first start against a library last written by Qt (no Rust-only key present), the Rust launcher
   copies `library.json` to `library.qt-backup.json` once, then proceeds.
3. Coexistence: during the transition both launchers work on the same file (one at a time, as two Qt instances today).
4. Rollback: an older Qt release keeps working because the schema stays 1.
5. Any future incompatible change bumps `schema` to 2; the Qt launcher then refuses the file without overwriting it.

All development and tests use `--library <temp>`; nothing reads or writes the owner's live folder until the owner runs
the acceptance phase (section 8, phase 7).

---

## 4. One engine

The launcher drives only nxrs (owner decision, section 10). Not ported: the Python engine's toolchain discovery and
problems (E-04, E-05), v1 arguments (E-08), its Play checks (P-01), the renderer and optimization rows (X-01), the
engine folder setting (X-03) and the save copy between engines (P-03, S-01). Releases carry no Python, GCC, .NET or
hactool bundle. An entry saved with another engine loads as nxrs in an nxrs folder of its own and shows as needing a build.

---

## 5. Packaging and CI

### 5.1 What changes in the release pipeline (`.github/workflows/release.yml`, `scripts/`)

- Remove: `jurplel/install-qt-action`, the Qt CMake build, `build-launcher.ps1/.sh/.bat`, `windeployqt`, Qt DLLs and
  plugin folders, `qt.conf`, Qt source archives in `legal/sources`, the Qt rows of `required_files` and `check_layout`,
  the Linux Qt/xcb library collection for the launcher.
- Add: Rust 1.99 (already installed by the engine steps), `cargo build --release --locked` of the launcher,
  `cargo test` of the launcher (seconds), a Windows static CRT (`+crt-static`) so `nxrs.exe` is one file with no MSVC
  DLLs, and third-party licence texts for the launcher's crates into `legal/licenses/nxrs-launcher/` (generated with
  `cargo about`, checked in CI).
- `package-release.py`: `launcher_files` takes one executable; `check_layout`/`required_files`/`validate_contents`
  lose Qt; `tests/test_release.py` follows.
- `smoke-release.py`: the same flags (`--library`, `--smoke-build`, `--build-output`, `--import-build`, `--screenshot`)
  keep working (that is why 1.12 is parity). `QT_QPA_PLATFORM=offscreen` goes; the Linux job runs the launcher under
  `xvfb-run` with Mesa (wgpu falls back to llvmpipe/lavapipe) for `--screenshot`. The headless verbs need no display.
- Release notes: also list launcher commits (path `src/launcher/**`) from the engine repository if the source moves.
- Linux runtime: the Rust binary loads X11/Wayland, xkbcommon and the GPU driver from the system at run time (no
  bundled `lib/` needed for the launcher); baseline stays Ubuntu 24.04.

### 5.2 Where the launcher lives [OWNER]

| Option | For | Against |
|---|---|---|
| 1. Stay in ChrisColeTech/nxrs (replace `src/launcher`) | One repo for the product; release.yml already there; no cross-repo pin | Two Rust toolchains/CIs to keep in step; contract changes span two repos |
| **2. Source in the engine workspace (`src/launcher`, its own cargo workspace), releases still published from ChrisColeTech/nxrs** | Contract v3 and the launcher change in one commit; one Rust CI; MIT with the engine; release URL, tags and `VERSION` history stay where users look | release.yml checks out the engine for the launcher too (it already does for nxrs-engine at `NXRS_ENGINE_REF`); release notes read two repos |
| 3. Everything in the engine repo, nxrs repo retired | One place | Loses the release history location |

**Recommendation: 2** (the location the brief names); 3 is possible later.
Licence note: the new crate would be MIT like the workspace. The Qt launcher repo carries AGPL notices for the vendored
engine; the launcher code is the owner's (all commits ChrisColeTech), and a Rust reimplementation copies no C++ code,
only behaviour and UI strings. The owner confirms the crate's licence.

### 5.3 Engine CI impact

None by default: the launcher is not a member of the engine workspace, so `cargo build --workspace` and
`cargo test --workspace` in `ci.yml` never see it. Launcher CI goes in a new workflow file
`.github/workflows/launcher.yml` (Windows + Linux, `paths: src/launcher/**`, own cache key, a few minutes), so engine
pushes do not run it and launcher pushes do not run the 90-minute engine jobs.

---

## 6. UI plan

### 6.1 Screens

1. **Main window** (min 920x600): title-bar row (accent glyph, File/Game/Keys/View/Help, caption buttons) / library
   area (gallery or list over the background canvas) / build output panel (bottom or right, resizable, collapsible,
   pop-out to its own window) / status bar.
2. **Search & Filter** popover under the title-bar row's right side (Ctrl+F).
3. **Properties / Add game**, **Settings**, **DLC**, **Updates**: modal dialogs inside the main window (`egui::Modal`),
   resizable, same tab order and defaults as Qt.
4. **Engine badge popup**, message and question boxes: small modals with the Qt texts.

### 6.2 Owner rules applied

| Rule | Plan |
|---|---|
| One accent colour | Theme tokens ported from `Theme::colorsFor`; the chosen accent is the only accent in use. Semantic reds (close button, error badge `#e06c60`, dialog errors `#ff7a7a`) stay semantic, not accents |
| Uniform elevated buttons, no ragged chips | One button style (raised fill, 1 px border, radius 5, 30 px height) for dialogs, cards and status bar. The Qt status-bar pills ("Engine ready" badge, Settings) become two uniform status-bar buttons; the badge keeps a coloured dot for state |
| One home per control | Qt duplicates, proposed single homes **[OWNER confirms]**: Add game / extracted folder / import build -> File menu (plus drop, plus the library's empty-space context menu) and the Commands strip goes; Search & Filter -> View menu + Ctrl+F (count shown in the status bar's game count, as Qt already does); Settings -> File menu (status-bar button removed, the engine badge still opens Settings from its popup); per-game actions -> Game menu + game context menu (menus are not "controls" duplicated on screen), the card keeps only its primary button and Properties |
| Context menus over toolbars | No toolbar (above); game context menu as W-07; empty-space context menu: Add game…, Add extracted folder…, Import build…, Refresh; console context menu: Open log, Clear view, Dock bottom/right, Pop out (the Qt dock toolbar's two buttons move here) |
| Fixed letterbox players | Not applicable: the launcher plays no media. Cover art stays crop-to-fill as in Qt (cards, not players) |
| Resizable panels everywhere | Console panel with a 1 px sash, height/width saved; list columns resizable; dialogs resizable with lists that grow |
| Native PC app, not an emulator | Wording from Qt kept ("TV mode — sharper", "Handheld — lighter", pause/mute as PC behaviour); About text rewritten for the one engine |
| New player settings global, per-game overrides stay | Existing overrides kept (layout, screen mode, workers); anything new goes to Settings > Play |

### 6.3 What egui does well and badly here

| Area | egui | Handling |
|---|---|---|
| Theming | Good: full control of `Visuals` (fills, strokes, radii, selection); no stylesheet | Map each Qt token (canvas, surface, raised, field, text, muted, accent, accentText, selected, border, hover) to `Visuals`; custom-paint the card, badge and caption buttons; port the contrast test |
| Fonts | Weak: bundled fonts lack CJK and real weights; game titles can be Japanese | Load Segoe UI, Segoe UI Semibold and Cascadia Mono / Consolas from `C:\Windows\Fonts` at start; add a CJK fallback (Yu Gothic / Meiryo; Linux Noto Sans CJK if present) to every family; semibold as its own family for titles and headers |
| High-DPI (devicePixelRatio 3 here) | Good: everything is in logical points; winit reports the scale per monitor | Keep Qt's logical sizes 1:1; draw icons and the app glyph as vector shapes (crisp at 3x) instead of Qt's 128 px pixmaps; size cover textures to `card width x pixels_per_point`; verify all screenshots at dpr 3 |
| Large image grids | Weak: immediate mode re-lays out every frame; texture uploads happen on the UI thread | Virtualized rows (`show_rows`/`show_viewport`), decode + EXIF orientation + downscale on two worker threads, upload a few textures per frame, an LRU texture cache bounded in bytes keyed by path+size+mtime(+target px), placeholder while loading, repaint only on events |
| Accessibility | Good: AccessKit (UIA on Windows, AT-SPI on Linux) is on by default | Custom widgets get explicit labels and roles matching W-21; kittest queries by those labels, so UI tests also check them |
| Title bar | Weak: an undecorated winit window loses Snap Layouts and the native resize/caption behaviour | Windows: subclass the HWND (`SetWindowSubclass`) and answer `WM_NCCALCSIZE`/`WM_NCHITTEST`/`WM_NC*BUTTON*` as `titlebar.cpp` does; Linux: `ViewportCommand::StartDrag` / `BeginResize` with a 6 px edge |
| Native dialogs | None in egui | `rfd` for file/folder pickers; message boxes as egui modals |
| Keyboard | Tab focus only; no grid arrow navigation | Arrow keys move the selection in gallery and list; Enter, Delete, Esc as Qt |
| Screenshots | `ViewportCommand::Screenshot` returns the frame | `--screenshot` and visual verification use it (no OS capture needed) |

---

## 7. Testing

- **Core tests** (`#[test]` beside each `core` module): the 33 cases of `src/launcher/tests.cpp`, one Rust test per
  Qt slot, same fixtures and assertions: pythonPrecedenceAndEnvironment, releaseAndCheckoutLayouts,
  bundledEngineWinsOverRememberedOne,
  discoverNspBuildAndPreserveLibrary, themeTextContrast, searchAndFiltersCombine, rendererPreservesSettings,
  screenModeIni, pauseOnFocusLossIni, askUserAndDiscoveries, screenModePerGameOverDefault, buttonLayoutIni,
  libraryRoundTripAndCorruption, dlcListRoundTrip, dlcManifestAndState, dlcPackagesAreCheckedBeforeTheEngineRuns,
  updatesInTheLibraryAndTheBuild, importedBuildBringsItsUpdate, importChecksAllArtifacts,
  perCardActionTargetsCorrectOperation, pathFormattingDoesNotInvalidateBuild, combinedNspBuild, nspSourceClassification,
  xciSourceIsAnNxrsGame, engineChoicePerGame, nxrsDoctorChecks (parse part), savesCopyBetweenEngines,
  toolEnvironmentCarriesKeys. Plus new: Qt-written `library.json` golden round trip (unknown keys, blobs, dates).
- **Process tests** (`tests/`, using `env!("CARGO_BIN_EXE_fake-engine")`, which answers like the Qt test binary's
  `main()`): dlcInstallerRunsTheEngineSync, updateProbeAsksTheEngine, nxrsEngineAnswersUpdatesAndDlc,
  nxrsDoctorChecks (run part), matchesRunningExecutableByFullPath, plus build streaming/cancel (fake engine prints
  `@progress` and sleeps; cancel kills it and its child).
- **UI tests** (`egui_kittest`): the 8 cases of `gui_tests.cpp`: clearsAsOneUpdate, titleBarHitRegions (the hit-test
  function), titleBarIconFollowsAccent (glyph raster), gameMenuInstallsDlc (menu items and enabled states in demo),
  propertiesListAndRemoveDlc, dlcWindowAddsAndRemovesSeveral, updatesWindowAddsRemovesAndChooses, escapeClosesPopup.
- **CLI tests**: each flag of 1.12 with its exit code against a temp library and the fake engine; the two cases of
  `tests/test_launcher_workflow.py` ported to drive the Rust binary.
- **Opt-in heavy runs** (never in the default run, owner rule): `NXRS_DOCTOR_EXE` (real doctor, as Qt), and
  `NXRS_LAUNCHER_E2E=<nxrs-build>`: a real build of the runtime's `hello` homebrew folder into a temp build folder
  through `--library <temp> --smoke-build`.
- Default `cargo test` stays in seconds.
- **Visual verification** each increment: `--demo --screenshot` at dpr 3 for gallery, list, filters, console,
  each menu, each dialog tab, light and dark, compared side by side with the Qt launcher's own `--screenshot` of the
  same view.

---

## 8. Phases, exit criteria, effort

Effort in agent-days. "A" = a second (Python) engine kept, "B" = nxrs only (the owner's choice, section 10).

| Phase | Work | Exit criteria | A | B |
|---|---|---|---|---|
| 0. Spike + scaffold | Crate as its own workspace; eframe window at dpr 3; theme tokens, fonts with CJK fallback; Windows title-bar subclass spike (snap, Snap Layouts, shadow, maximized inset) and Linux drag/resize; `--screenshot`; kittest running; launcher.yml | Screenshot of the themed empty window with title-bar menus at dpr 3; Snap Layouts flyout shown on the maximize button; engine `cargo build --workspace` untouched | 1.5 | 1.5 |
| 1. Core port | `core/*` with every D, G, E, L, U, S, I row; core tests | All ported core tests pass on Windows and Linux (WSL); golden `library.json` round trip | 3.75 | 3.0 |
| 2. Jobs + processes | runner, build/probe/sync/doctor/tools jobs, job object / process group, process watch, close request, launch; fake engine | Process tests pass; a 10 000-line fake build streams with the UI at full frame rate; cancel leaves no process | 2.25 | 2.0 |
| 3. Main window | Title bar + menus, gallery (virtualized, artwork pipeline), list, canvas, filter popover, status bar + badge popup, console panel, drag & drop, shortcuts, context menus, empty states, demo, window state | Side-by-side screenshots vs Qt for every W row; kittest for filters and the Game menu; 300-game synthetic library scrolls smoothly | 4.0 | 4.0 |
| 4. Dialogs + flows | Add/Properties (6 tabs), Settings (5 tabs + doctor), DLC, Updates, keys, saves, remove, about, message boxes; every flow of 1.4–1.8 wired | Every X/B/P row checked; the 8 UI tests pass; a real nxrs build + Play + Stop of the `hello` homebrew through the UI with a temp library | 4.5 | 4.0 |
| 5. CLI parity | All flags of 1.12, headless verbs, exit codes, workflow tests | Each flag tested; `smoke-release.py` passes against a locally packaged bundle | 1.5 | 1.5 |
| 6. Packaging + CI | release.yml, package-release.py, smoke-release.py, test_release.py, licences, icons | CI green on Windows and Linux; relocated archive smoke passes; draft release artifacts | 2.0 | 2.0 (+0.5 for entries from another engine, counted in 4) |
| 7. Acceptance + cut-over | Owner runs the Rust launcher on the live library (backup made first), works through the parity list; fixes; Qt sources removed from the release; LAUNCHER.md / RELEASES.md / LAUNCHER_CONTRACT.md updated | Owner sign-off | 1.0 | 1.0 |
| **Total** | | | **20.5** | **19.0** |
| Contingency | Title bar, CI display, fonts | | +3 | +3 |

**Riskiest parts**

1. **Windows title bar (W-01).** winit owns the window procedure; Snap Layouts need `HTMAXBUTTON` answers and the
   caption needs `WM_NCCALCSIZE` handling without fighting winit's own undecorated-window code. Phase 0 proves it or
   falls back to native decorations with the menus in a row below **[OWNER, only if the spike fails]**.
2. **Data compatibility.** Byte-level details (dates, numbers, sorted keys, Qt blobs, unknown keys, INI line endings)
   decide whether the owner's library survives; golden tests against Qt-written files before any live run.
3. **Windows process plumbing.** Console windows from a GUI parent, merged output, killing clang trees, pipes kept open
   by grandchildren, paths with spaces/brackets/`&` (no shell is used, as in Qt).
4. **Gallery at dpr 3.** Texture memory and frame time with many large covers; the downscale + cache design above.
5. **CI with a display.** `--screenshot` and kittest snapshots on Linux need xvfb + Mesa; headless verbs avoid it for
   everything else.

---

## 9. Isolation from engine work

**Yes: the launcher can be built without disturbing engine work.**

- **Own crate and workspace**: `src/launcher/Cargo.toml` carries its own `[workspace]` table (as `src/oracle` does), so
  it has its own `Cargo.lock` and is never a member of the engine workspace. The root `Cargo.toml` and `Cargo.lock`
  are not edited.
- **Own target dir**: `src/launcher/target` (its default as a separate workspace), ignored by a `src/launcher/.gitignore`
  (`/target`); the engine's `target\<lane>` folders and their 243 GB are never touched or locked.
- **No edits to other crates**: the launcher spawns `nxrs-build` and has no path dependency on any engine crate.
  Contract v2 already covers everything the Qt launcher does; no engine change is needed.
- **Small touches outside `src/launcher`, all optional**:
  1. root `Cargo.toml`: add `"src/launcher"` to `exclude` beside `src/oracle` (documentation only; cargo does not need it
     because the launcher's manifest is its own workspace root);
  2. new file `.github/workflows/launcher.yml` (no change to `ci.yml`);
  3. `docs/LAUNCHER_CONTRACT.md`: name the new launcher's location at cut-over (phase 7).
- **CI impact**: none on `ci.yml` (`--workspace` does not include the launcher); the new workflow runs only on
  `src/launcher/**` changes and takes minutes.
- Note: the engine working copy currently has an uncommitted `Cargo.lock` change (`flate2`); the launcher work does not
  touch that file.

---

## 10. Owner decisions

1. **Engine** (section 4): nxrs only, or a second engine kept.
2. **Repository** (5.2): recommended option 2 (source in the engine workspace, releases from ChrisColeTech/nxrs);
   and the crate's licence (MIT recommended).
3. **One home per control** (6.2): confirm the proposed removals (Commands strip, status-bar Settings button) and homes.
4. **Source inconsistencies** (1.13): `hintsAtStart` (recommend fix), nxrs on Linux (port or allow), "Source offline"
   rule (recommend unify), dead "Build options…" action (recommend drop).
5. **Title bar fallback**, only if the phase 0 spike fails.

## Owner decisions (2026-11-20)

- Engine: the Rust launcher drives only the nxrs engine. Use the nxrs-only estimates (B, ~19 agent-days + contingency).
- Location: engine repo, src/launcher, its own cargo workspace (MIT); may be renamed later.
- Fix the four Qt problems: hintsBuilt recording (window.cpp:874), nxrs games allowed on Linux, one 'source offline' rule (the cards'), Drop the dead 'Build options...' entry. Correction (owner): keep the Qt layout as is: the top bar (Add game with its drop-down, Search & Filter) and the status-bar Settings button stay; the port replicates the Qt UI, it does not redesign it.

