# Owner decisions: research (2026-11-20)

Ten open design questions, each answered from the code of two reference projects, then compared with Turbo and with
nxrs as they are today, and ended with a recommendation under the owner's rules:

- nxrs is a **native PC app, not a Switch emulator**: host behaviour is a player setting with a sensible PC default.
- **Turbo parity**: what Turbo does, nxrs does, unless the owner decides otherwise.
- **Licences cover code, not designs**: Eglantine (GPL) and MZ Labs' parts of Turbo (AGPL) are reimplemented, never
  copied; nxnative (MIT) may be ported or reimplemented.

Status of every recommendation: **Awaiting owner** (PORT_PLAN 5A.7).

## Sources and versions

| Name | Where | Version read | What it is |
|---|---|---|---|
| Eglantine | `D:\Projects\Eglantine-nx2` | `cf6cfe9caa`, VERSION `0.1.0-rc4` | Eden / yuzu fork, Switch 2 emulator (C++, GPL-3). JIT (dynarmic) or NCE, Vulkan / OpenGL, Qt and Android front ends |
| nxnative | `D:\Projects\RecompLauncherEngine\build\Q\nxnative-up` | **`7ebb76e`** (2026-11-20, shallow clone) | Native ARM execution on Apple silicon (MIT): game modules rewritten offline, NVN on Metal, Mac and iOS only |
| nxnative (old) | `D:\Projects\nxnative` | `e4b0d9b` (131 commits **older** than 7ebb76e) | Spot-checked only (frame generation); agrees with 7ebb76e |
| Turbo | `D:\Projects\Switch2RECOMP-main`, branch `turbo` | `11e966f` | The owner's fork of SwitchRecomp (C runtime + Python recompiler) |
| nxrs | this repository; launcher `D:\Projects\RecompLauncher` | engine working tree; launcher `1bb8cde` | — |

Note on the brief: there is no `Switch2RECOMP-main\nxnative` folder. The 7ebb76e import point is the shallow clone at
`build\Q\nxnative-up` (AUDIT.md:8 also names 7ebb76e); the full checkout at `D:\Projects\nxnative` is the older
e4b0d9b. Every nxnative path below is relative to `build\Q\nxnative-up` and means **7ebb76e**.

---

## 1. Frame generation default

**Summary:** Eglantine ships frame generation **off**, per game switchable; nxnative has **none** (an unmerged branch
is mentioned). nxrs's built-in mode is on for every game, which neither reference does. Recommend **off by default,
on per game only where a profile says it was checked clean**, plus a frame-level skip.

**Eglantine.**
- `src/common/settings.h:441-442`: `frame_gen` is a `SwitchableSetting<bool>` with default **false** (switchable =
  has a per-game override). `frame_gen_type` defaults to FSR3-style (`settings.h:444-451`), LSFG optional
  (`settings_enums.h:168`); multiplier 2..4, target rate, flow scale, queue target follow (`settings.h:453-505`).
- Per-game: the per-game properties dialog has its own "Frame Gen." tab (`src/yuzu/configuration/configure_per_game.cpp:126,142`).
  The per-game recommended presets (`src/common/game_presets.h:46-71`, MKW and OoT) do **not** turn it on.
- Artifact handling (`src/video_core/renderer_vulkan/present/`):
  - no UI mask and no scene-cut detection; it works on the final presented image (`renderer_vulkan.cpp:238-253`);
  - warm-up: nothing is generated until 2 frames of history and 2 consecutive warm frames (`frame_gen.cpp:26-27,242-244`);
    any change of size, format or flow scale rebuilds the chain and starts the warm-up again (`frame_gen.cpp:233-236,310-326`);
  - duplicate-frame detection skips the optical flow when the frame did not change (`fsr3_frame_gen.h:91-93`);
  - the interpolation biases toward a 50/50 blend where the two warps disagree, and blends with the un-warped pixel
    where motion is small, so still text and HUD are not dragged (`host_shaders/fsr3_frame_interpolation.comp:91-104`);
  - a pacer generates only when the display has spare refresh capacity and the game is below target
    (`frame_gen_pacer.h`, `vk_present_manager.cpp:511-512`).

**nxnative.** No frame generation on main at 7ebb76e: no code under `dm/host` or `dm/gfx`. `docs/reference/frame-rate.md:674`
mentions "frame generation's unmerged change" (a branch, not shipped); `dm/games/040041E0212AE000/README.md:71` lists
frame generation as untested. Its frame-rate tools instead are per-game tested defaults: `Frame rate` 30/60/120 only
for games whose speed the app controls (`docs/settings.md:24-28`), `Hold frames when late` (`settings.md:195-215`),
`Dynamic resolution` off by default (`settings.md:30-32`), each game's defaults from its profile
(`settings.md:14-20`, `dm/games/settings.tsv`).

**Turbo.** `runtime/main.c:272`: `lsfg=1` is the written default, but it needs the player's own `Lossless.dll`
(`lsfg_dll`, `main.c:277`): without that file nothing is generated, so in practice it is **off until the player opts
in by supplying the DLL**. `fps_target=120` means frame generation x2 (`main.c:261`).

**nxrs today.** `src/config/src/options.rs:83-86`: `lsfg=1`, `frame_gen_type=builtin`. The built-in mode needs no
DLL, so it is **on for every game and every player**. `src/graphics/src/vk/framegen.rs:1-17` and `shaders/fg_interp.comp`
already bias toward zero motion for still pixels and fall back to the nearer real frame where no vector agrees, and
the pacer skips hitches. It still garbled Arceus's village text and hung a Jamboree screenshot (AUDIT.md:74,443); lane W
and the Arceus runs use `lsfg=0` (PORT_PLAN 5B Arceus row).

**Recommendation.**
1. Default `lsfg=0` (written default and an ini migration through `settings_version`, as Turbo moved `docked`,
   `main.c:328-335`, so inis that only carry the old default are switched off once; an explicit player choice
   written after the migration stays). This also matches Turbo's effective behaviour (no DLL, no frames).
2. Per-game opt-in from the profile: a `frame_gen` field in `engine.tsv` (on / off / untested), turned on only after a
   run shows clean text and HUD, exactly as `present_interval` is recorded today (5A.6). The ini wins over the profile.
3. A frame-level skip on top of the per-pixel fallback: present no generated frame when the two real frames are
   identical (Eglantine's duplicate check) or when most of the frame disagrees (a scene cut, a menu opening, a dialogue
   box appearing), and re-warm for two frames after such a frame (Eglantine's warm-up). Neither reference has a
   scene-cut detector; it is new work.
4. No UI mask: neither reference has one, and without the game's cooperation the HUD layer is not known.

---

## 2. Interpreter fallback and execution hints

**Summary:** Eglantine never meets "code that was not compiled" (it JITs everything at run time). nxnative runs the
game's own ARM code natively, so the only miss is a **module** with no rewritten copy: the load fails, and the build
script boots the game once to **capture** such modules for the next build. Turbo has an interpreter fallback plus a
`.hints` file fed into the next build. nxrs dies at the first miss. Recommend **Turbo parity, reimplemented**.

**Eglantine.** `src/common/settings.h:256-269` and `settings_enums.h:144`: CPU back end is Dynarmic (JIT) or NCE
(native code execution, default where built with `HAS_NCE`). Every block is translated when first reached, so there is
no "not recompiled" case, no interpreter fallback and no hints file.

**nxnative.**
- All of a game's modules are rewritten offline (`dm/tools/patch_nso.py`, `docs/development.md:9`); on the Mac and
  iPhone the code is the original ARM code, so any branch target inside a rewritten module simply runs. Nothing is
  "missed" per function.
- A module loaded at run time that has no rewritten copy (decoded from memory, as Mario Tennis Fever's boot stub does)
  makes `ro::LoadModule` fail with a red line (`dm/host/hle_ro.c:85`). With `DM_NRO_CAPTURE=<dir>` the host writes
  the image to `<dir>/<sha256>.nro` and ends the run (`hle_ro.c:57-62`); the packing script runs this capture boot
  automatically during the build when the NRR names an NRO no file holds (`scripts/build_ipa_from_nsp.py:1636-1660`)
  and rewrites what it caught like any other NRO.
- Code a game generates itself (nn::jit, Super Mario 64 in 3D All-Stars) runs through a MAP_JIT mapping on the Mac;
  iOS serves it from a capture (`dm/host/hle_jit.c:1-8`).

**Turbo.** `runtime/main.c:26,36-50`: when a jump lands on a function that was not recompiled, `swr_interp_entry`
interprets it and records its address; `save_hints` writes `<exe>.hints` (`main.c:46-48,460,515,531`) and
`load_hints` reads it at start (`main.c:50,490`). `switchrecomp.py:583-600` (`program_hints`) keeps hints per program
version (a stamp file, put aside when the build is of another version: Arceus stopped at start with another
version's hints) and `switchrecomp.py:731-740` feeds them to `codegen.Program` as extra function starts. `--interp-only`
(`switchrecomp.py:643`) is a diagnostic. The interpreter is generated from the same instruction templates as the
recompiled code (`codegen.py:303-352`, `a64.py:1-8`); **both files are MZ Labs' AGPL code**, so the design may be
reimplemented, not the code copied.

**nxrs today.** `src/runtime/src/dispatch.rs:220-229`: a miss is `guest_fatal("rt_dispatch: no function at guest pc …")`.
No hints file, no `discovered.tsv` (PORT_PLAN 3.1 names it; AUDIT.md:115; PORT_PLAN 5A.1 "Execution hints" row, Not
started). `os/src/services/ro.rs:274`: an NRO with no built copy fails to load, with no capture.

**Recommendation.** Turbo parity:
1. An interpreter fallback for a miss **inside a module** (not a function start, not covered by setjmp): generated from
   nxrs's own lifter templates (mkw-recomp lineage, MIT) so the interpreted and recompiled paths share one definition,
   as in Turbo's design. Misaligned addresses and addresses outside every module stay fatal.
2. Each interpreted entry is written to `<data>\discovered.tsv` (the name PORT_PLAN 3.1 already uses), stamped with the
   program's build IDs as `program_hints` does, and read by the next `nxrs-build` as function seeds. The launcher shows
   "N new functions found; rebuild for full speed" after a run that wrote some (rebuild is incremental: only new chunks
   compile).
3. Add nxnative's capture boot for NROs decoded at run time (5A.1 gap row).
4. `--interp-only` only if the interpreter exists anyway; it is a diagnostic, not a player option.

---

## 3. Renderer choice

**Summary:** Eglantine offers Vulkan (default), three OpenGL variants and Null; nxnative is Metal only (guest GL and
Vulkan games are layered onto Metal), with no choice. nxrs is Vulkan only, but the launcher still offers OpenGL and the
engine quietly runs Vulkan. Recommend **Vulkan only, said honestly**: the launcher shows no choice for nxrs games.

**Eglantine.** `src/common/settings_enums.h:140`: `RendererBackend { OpenGL_GLSL, Vulkan, Null, OpenGL_GLASM, OpenGL_SPIRV }`;
`settings.h:349-355`: default Vulkan (OpenGL only on Solaris / managarm). Switchable per game. The game presets pin
Vulkan for MKW and OoT (`game_presets.h:49,62`).

**nxnative.** Metal only. Guest NVN, Vulkan (Hades) and OpenGL (Digimon) all go through layers onto Metal
(`docs/development.md:11`); there is no back-end setting in `docs/settings.md` or `docs/env_switches.tsv`.

**Turbo.** `runtime/main.c:252`: `renderer=opengl` (default), `vulkan` (native NVN layer) or `eden`.

**nxrs today.** `src/config/src/options.rs:57` (`renderer=vulkan`, "the only renderer of this engine") and
`options.rs:214-222`: any other value becomes `vulkan`, said once on stderr. Launcher: Properties > Runtime offers
"Vulkan — native NVN" and "OpenGL — compatibility" for every game (`src/launcher/window.cpp:685`), writes
`renderer=` (`library.cpp:841-843`) and treats unknown values as opengl (`library.cpp:145`). LAUNCHER.md:352-354
records the mismatch. AUDIT.md:332,444: an OpenGL renderer is not a goal of the Vulkan-only plan.

**Recommendation.** Vulkan only. Turbo's OpenGL renderer is its GPFIFO / Maxwell code (`gpu*.c`, `gpu_gl.c`), much
of it MZ Labs' AGPL, and it would be a second renderer to keep at parity; neither reference needs a second back end on
its platform, and Eglantine's own presets pin Vulkan for the Switch 2 games. Launcher: for an nxrs game, show the
graphics back end as fixed text ("Vulkan") instead of the combo, and keep the combo for Turbo games. Engine: keep
accepting old inis but rewrite `renderer=vulkan` with a log line (already), and report "no Vulkan 1.3 device" through
`doctor` and a message box instead of silently falling back.

---

## 4. Docked vs handheld

**Summary:** both references expose it as the **console mode the game is told**, and both keep render resolution as
a separate setting. Eglantine defaults to handheld but its presets set docked for its two showcase games; nxnative
defaults to docked. Turbo and nxrs default to TV mode. Recommend **keep it a console-mode setting, TV by default,
named for what the player gets**, with resolution and performance separate.

**Eglantine.** `src/common/settings.h:834-840`: `use_docked_mode` (`ConsoleMode`, `settings_enums.h:156`), default
**Handheld**, switchable per game; presets set **Docked** for MKW and OoT (`game_presets.h:56,69`). What it changes:
the operation mode the game reads (`core/hle/service/am/service/common_state_getter.cpp:198-200`), the performance mode
(Boost when docked, `core/hle/service/apm/apm_controller.cpp:72`), the display size the game is given
(`core/hle/service/vi/system_display_service.cpp:119`) and the window's default size (`src/yuzu/bootmanager.cpp:770`).
Render resolution is the separate `resolution_setup` multiplier (`settings.h:360`, 1/4x..8x). The controller applet
dialog also lets the player switch modes (`src/yuzu/applets/qt_controller.cpp:675-676`).

**nxnative.** "Console mode" in the settings window (`docs/settings.md:40-42`: docked renders at the Switch's TV
resolution, handheld at the lower one, lighter and faster). `DM_OPMODE` (`docs/env_switches.tsv:297`): default the
profile's `op_mode`, **console (docked) unless the profile says otherwise**; it is runtime state the front end can change,
pushing operation-mode and performance-mode messages 30/31 (`dm/host/hle_oe.c:3-24,60`; iOS drops to handheld when the
phone overheats). Render scale 50-100 % is a separate setting (`settings.md:34-38`), as are frame rate and dynamic
resolution.

**Turbo.** `runtime/main.c:237`: `docked=1` (TV mode) by default; `main.c:328-335` migrated old inis from `docked=0`
because handheld "looked soft on a monitor".

**nxrs today.** `options.rs:25`: `docked=1`; PORT_PLAN 5A.2 "Docked / TV mode by default" Done, a live change notifies
the game. The launcher offers "Screen mode: TV mode — full resolution (recommended) / Handheld mode — the console's own
screen, faster" (`window.cpp:689-692`). `resolution`, `dynamic_resolution`, `game_dynamic_resolution` are separate
options (`options.rs:58-61`). The profile's `op_mode` / `perf_mode` are not read yet (PORT_PLAN 5A.1 "Profile game.tsv
fields" row).

**Recommendation.** Keep it as it is: the console mode the game is told (operation mode + performance mode + display
size, as both references do), TV by default, presented to the player as a picture/performance choice ("TV mode:
sharper / Handheld mode: lighter"), never as "docking". Resolution scale stays its own setting. Read the profile's
`op_mode` as the default for games that need handheld (nxnative's per-game default), with the ini winning.

---

## 5. Controller layout default

**Summary:** both references default to **Switch positions** for Xbox and PlayStation pads (the right button is A),
and nxnative always reads **Nintendo pads by their printed labels**. Turbo and nxrs default to `switch`. Recommend
**Xbox labels as the PC default** (press the button whose letter the game shows), positional as a setting, and
Nintendo pads by label in both modes.

**Eglantine.** `src/input_common/drivers/sdl_driver.cpp:996-1016`: default binding Switch A = `SDL_GAMEPAD_BUTTON_EAST`,
B = SOUTH, X = NORTH, Y = WEST, i.e. **by position** for every SDL pad (on an Xbox pad the printed B is A, on a
PlayStation pad Circle is A). Each player remaps freely in the Qt controls dialog; no Xbox-label preset.

**nxnative.** `dm/host/pad_apple.h:1-19`: "buttons by position, as on an Xbox pad … but a Nintendo pad's by their printed
labels": a Switch Pro Controller or Joy-Con maps name for name; any other pad by position (the right button is A).
No player setting for face buttons (raw HID pads can be remapped with `DM_HID_MAP`, `env_switches.tsv:89`). The Home
quit prompt reads A/B **by label** (`dm/host/home_quit.h:3-4`), the host's own UI follows PC conventions.

**Turbo.** `runtime/main.c:247`: `button_layout=switch` (positional; Circle = A) by default, `xbox` (by label; Cross = A)
as the option, `button_map` per-button overrides (`main.c:248`).

**nxrs today.** `options.rs:42-43`: `button_layout=switch` default, `xbox`, `button_map`; `os/src/input/layout.rs:1-10`
maps a positional mask for **every** pad, Nintendo pads included (`os/src/input/mod.rs:852,877`), so with `xbox` a Pro
Controller's printed B acts as A. Launcher: a global default in Settings > Controller plus a per-game override
(`window.cpp:655-661,686`).

**Recommendation.** For a PC app the sensible default is that the button printed A is A: on-screen prompts show A/B/X/Y
letters, and PC players read them off their pad (Xbox A bottom; PlayStation Cross = A, Square = X). Default
`button_layout=xbox` for new inis (old inis keep their value: nobody's muscle memory changes under them), keep `switch`
as the setting for players who want Nintendo positions. Port nxnative's rule that Nintendo pads (Pro Controller,
Joy-Con 1 and 2, Pro Controller 2) are always read by label, so either setting is right on them (5A.3 gap row). The
engine's own prompts (Home quit, controller prompt) already use A = confirm on the pad as labelled. This departs from
Turbo's default, so it needs the owner's word.

---

## 6. Console-style prompts

**Summary:** both references show a **controller prompt only when the game itself asks** for more players, and
nxnative adds a **hold-Home quit prompt (on by default)**. Profile pickers: Eglantine skips by default, nxnative asks
only when the game asks and more than one choice exists. Error applets: Eglantine runs the real applet, nxnative only
logs. nxrs already has all three switches. Recommend **keep nxrs's defaults**, add the picker as an option off by default.

**Eglantine.**
- Controller applet: HLE by default (`src/common/settings.h:147-148`), i.e. Yuzu's Qt "Controller Applet" dialog when
  the game calls ShowControllerSupport (`src/yuzu/applets/qt_controller.cpp`). Player-select, error, Mii edit and
  net-connect applets default to LLE (the console's own applets from the player's firmware, `settings.h:149-158`).
- Profile picker: `select_user_on_boot` default **false** (`src/qt_common/config/uisettings.h:138-144`); the player-select
  applet (LLE) appears only when the game opens it.
- Quitting: no Home-hold prompt; stopping asks first, `confirm_before_stopping = Ask_Always` (`uisettings.h:108-114`).

**nxnative.**
- Controller prompt: `dm/host/ctrl_prompt.h:1-5`: shown only when the game calls ShowControllerSupport wanting more
  players than it sees; waits with no time limit, a joining controller is a player at once, A finishes, B / Escape
  cancels (`docs/playing.md:58`). "Press a button to join" is a separate option, off by default (`docs/settings.md:117-119`).
- Home quit: hold Home 2 s, then A quits / B returns, **on by default** on the Mac (`docs/playing.md:60`,
  `env_switches.tsv:93`, `dm/host/home_quit.h:1-8`); `DM_HOME_QUIT=0` turns it off. Texts localisable by the front end
  (`env_switches.tsv:32,95`).
- Profile picker: a native dialog only when the game calls ShowUserSelector; a single user is chosen without asking when
  the game allows skipping (`dm/host/hle_account.c:63-90`, `dm/host/user_select_mac.h`).
- Error applet: printed to the log only (`dm/host/hle_misc.c:116-119`).

**Turbo.** The controller applet is answered at once with 1 player, controller 1 (`runtime/kernel.c:715-724`); the
player-select applet returns the console user (`runtime/svc_core.c:300`); no Home-quit or profile prompt.

**nxrs today.** `options.rs:29-50`: `error_notice=1` (a message box beside the game, which goes on), `controller_prompt=1`,
`pc_user=1` (no picker: the game always runs as that local profile), `home_quit=1`, `keyboard_box=1`. PORT_PLAN 5A.3
"Controller-support prompt…" Partial: state machines and windows exist but were never run in a game with pads.

**Recommendation.** Keep: controller prompt on (it appears only when the game asks, as on both references), Home quit
on (nxnative's default; a PC pad has no other way out of a fullscreen game), error notice on (a PC player should learn
why a game stopped; nxnative's log-only is the iPhone's constraint). Profile picker: keep `pc_user` with no dialog by
default, and add an option (`ask_user=0`) that shows nxnative's picker when the game asks and more than one local
profile exists. Prompt texts in the player's language (nxnative's `DM_*_TEXT`) as a small gap.

---

## 7. Build optimisation (-O) and similar build options

**Summary:** neither reference lets players pick compiler flags. Eglantine ships prebuilt binaries (with a PGO variant
in its CI); nxnative compiles no game code and its host is fixed at `-O2`, with packing options that are about content
and platform only. Turbo exposes `-O 0..3` and `--native`. nxrs compiles at one measured flag set and ignores `-O`.
Recommend **no optimisation choice for nxrs games**; the launcher hides it.

**Eglantine.** No game compilation. Its release CI builds plain and clang-PGO variants
(`.github/workflows/build-linux.yml:1-4,34-37`, profile `eglantine.profdata`); CPU accuracy and "unsafe" optimisations
are run-time settings (`src/common/settings.h:270-275`).

**nxnative.** Game code is rewritten, not compiled. The host builds at `-O2` everywhere (`dm/host/Makefile:18-20`:
"Not -O3 (no measured gain over -O2) and not -flto"). The packing script's options (`scripts/build_ipa_from_nsp.py:3103-3150`)
are the package version, language, icon, motion on/off, frame-rate counter, RomFS bundled or separate, which GPUs get
a pipeline archive, and prebake: content and platform, no compiler flags.

**Turbo.** `switchrecomp.py:633,638`: `-O` (default 2) and `--native` (`-march=native`); the C flags are built at
`switchrecomp.py:182`. The launcher passes `-O` from a per-game combo (`buildmanager.cpp:43-45`).

**nxrs today.** `src/cli/src/cc.rs:38-55`: game code always `-O1 -march=x86-64-v3 -fno-vectorize -fno-slp-vectorize`
(measured: fewer instructions than either compiler's defaults); `-O`, `--native`, `--cc` accepted and ignored
(PORT_PLAN 6.1, AUDIT.md:118). The launcher greys the Optimization combo for nxrs games (`window.cpp:674-681`).
`-march=x86-64-v3` means AVX2 is required; `doctor` checks it (PORT_PLAN 6.2).

**Recommendation.** Keep one measured flag set; hide (not grey) the Optimization row for nxrs games and keep accepting
`-O` silently for contract v1. Player-facing build options should be content options only (update, DLC, mods,
language, shaders prebuilt or not), as nxnative's script does. If a CPU without AVX2 must be supported, that is a
second flag set chosen by `doctor`, not a player choice.

---

## 8. Focus / background behaviour

**Summary:** Eglantine **keeps running** in the background by default, with "pause" and "mute" as two separate player
settings. nxnative tells the game it **lost focus** whenever its Mac window is not key (Switch semantics), with no
player setting. nxrs already chose the PC default (keep running) with an option. Recommend **keep it**, add mute.

**Eglantine.** `src/qt_common/config/uisettings.h:118-131`: `pause_when_in_background` default **false**,
`mute_when_in_background` default **false**, both in the UI settings; the guest is not told about focus.

**nxnative.** `dm/host/front_mac.m:1688-1703`: losing key status sends focus state 2 (out of focus) to the game,
minimising sends 3 (background) and suspends audio and GPU, regaining focus sends 1; input held at focus loss is
released (`front_mac.m:327-342`). Games decide what to do (Tears of the Kingdom stops presenting). No player setting;
`DM_FOCUS_KEEP=1` is a test switch for the regression harness (`env_switches.tsv:48`).

**Turbo.** The game is never told; input is dropped while the window is not in front (`runtime/backend_win32.c:588-610`,
`SWR_CONTROLLER_BACKGROUND=1` keeps pads), keys released on `WM_KILLFOCUS` (`backend_win32.c:468`).

**nxrs today.** `options.rs:50`: `pause_on_focus_loss=0` (the game runs on and is never told), `1` = nxnative's
behaviour; PORT_PLAN 5A.2 focus row. The launcher has it as one global setting (launcher `1bb8cde`,
`library.cpp:861,869`). Held pad input is not released on focus loss (PORT_PLAN 5A.3 Optimus row; AUDIT.md:296).

**Recommendation.** Keep `pause_on_focus_loss=0` as the PC default. Add Eglantine's second switch,
`mute_on_focus_loss` (default 0), and release held pad input on focus loss (already a gap). When minimised, skip
presenting (no visible window) without telling the game, unless `pause_on_focus_loss=1`.

---

## 9. Distribution

**Summary:** Eglantine publishes **prebuilt binaries for every platform** on GitHub releases (and is GPL, so source is
public); nxnative has **no releases at all**: players clone and build, front ends pin commits. Turbo's engine ships
inside the launcher's release with its source. Recommend **nxrs engine as binaries inside the public launcher's
release, no engine source**, which the licences allow.

**Eglantine.** `.github/workflows/release.yml:81-117`: Windows zip, Linux AppImage + tar.gz, macOS; daily and
branch releases are rolling, stable and rc are created once (`release.yml:4-5,136-145`); download link in `README.md`
("Download").

**nxnative.** `docs/frontends.md:131`: "nxnative has no releases or tags: a front end pins a commit and moves it
forward". Players build on their own Mac with `scripts/build_ipa_from_nsp.py` (`README.md`, "How to play"); community
front ends (NX2Apple, NX Play, …) wrap the scripts.

**Turbo.** The launcher bundles Turbo's engine (`engine/` with the prebuilt `runtime_win64_win32.exe` /
`runtime_linux_x11`, launcher `docs/RELEASES.md:13-27`) and publishes `nxrs-source.tar.gz` with the pinned engine
appended, because Turbo is AGPL (launcher `.github/workflows/release.yml:245-258`).

**nxrs today.** The engine repository is private; the launcher's Windows bundle includes `nxrs-engine/` only when a
manual run asks for it or `NXRS_ENGINE_IN_RELEASE=true`, using `NXRS_ENGINE_TOKEN` (launcher `release.yml:16-34`,
`docs/LAUNCHER.md:345-350`); Linux bundles never include it (`docs/RELEASES.md:25`). PORT_PLAN 8: nxrs is MIT, built
from MIT / BSD / zlib / Apache sources, nothing GPL or AGPL copied.

**Recommendation.** Ship `nxrs-engine/` (nxrs-build, engine libraries, `engine.json`, `LICENSES/` and the README
credits text) as binaries in the public launcher's releases, Windows now and Linux once M10 is verified on a real GPU.
The licences of every source in PORT_PLAN 8 permit binary-only redistribution with their notices; no engine source
is required. Keep `nxrs-source.tar.gz` limited to the launcher and Turbo (its `git archive` of the engine submodule
already excludes the nxrs engine; check that stays so). Before the first such release: a licence check that no MZ Labs
(AGPL) or Eglantine (GPL) code entered the engine (PORT_MAP.tsv statuses), and the owner's decision to set
`NXRS_ENGINE_IN_RELEASE`.

---

## 10. Linux

**Summary:** Eglantine **builds** Linux x86-64 and aarch64 in CI (AppImage + tar.gz) but runs no tests there; its
Switch 2 work is measured on Windows. nxnative has **no Linux** (Apple only), CI on macOS runners, real-hardware tests
on Macs and iPhones. Recommend **Linux as an experimental target until a run on a real GPU**, then in the release.

**Eglantine.** `.github/workflows/build-linux.yml:54-56,168-169`: `ubuntu-24.04` and `ubuntu-24.04-arm`, built with
`BUILD_TESTING=OFF`, `YUZU_TESTS=OFF`; the job only checks the binaries exist (`build-linux.yml:223-240`). The NX2
notes measure on Windows hardware (`NX2_NOTES.md:33,186`: cdb, a Ryzen 5 2600 + RTX 3050). The user docs, inherited
from Eden, say Linux is supported and tested mostly on Arch and Gentoo, X11 preferred over Wayland
(`docs/content/user/Architectures.mdx:62-68`).

**nxnative.** Apple silicon Macs, iPhone, iPad only (`README.md:3`, "Requirements"). CI: `.github/workflows/tests.yml`
on `xcode-27` macOS runners (host tests, sanitizers, Mac builds, iOS compile; `docs/development.md:159-175`); game
testing on real Macs and iPhones (`scripts/regress.py`, each game's note).

**Turbo.** `switchrecomp.py:636-637`: `--target linux`, `--backend x11`; the launcher's release builds the Linux bundle
on `ubuntu-24.04` and runs a few engine tests there (launcher `release.yml:42-51,73-88`); Turbo's own engine CI is
Windows only (`.github/workflows/engine-ci.yml:21`).

**nxrs today.** PORT_PLAN M10 Partial: the whole engine builds and runs on x86-64 Linux; CI `linux` job on
`ubuntu-latest` (`.github/workflows/ci.yml:93-95`); verified in WSL on llvmpipe only (no real GPU, no sound server).

**Recommendation.** Keep the CI job (build + every test that needs no GPU), and treat Linux as experimental until one
library game runs on a real GPU (a Steam Deck or a Linux PC with Mesa RADV, and one NVIDIA proprietary driver) with
sound. Then bundle `nxrs-engine/` in the Linux release too (gap row in 5A.1). X11 and Wayland both stay supported;
Eglantine's "prefer X11" note is about Qt and its compositor, not Vulkan surfaces.

---

## Summary table

| # | Question | Recommendation | Engine change | Launcher change |
|---|---|---|---|---|
| 1 | Frame generation default | Off by default; on per game only where the profile says checked; frame-level skip (duplicates, scene cuts) | `lsfg` default 0 + one-time ini migration; `engine.tsv` `frame_gen`; skip in `framegen.rs` | Optional per-game "Frame generation" toggle in Properties > Runtime |
| 2 | Interpreter fallback / hints | Turbo parity, reimplemented: interpreter for misses inside modules, `discovered.tsv` fed to the next build; nxnative's NRO capture boot | Interpreter from nxrs's lifter templates; `dispatch.rs` miss → interpreter; `discovered.tsv` writer and reader; capture boot in `nxrs-build` | "New functions found: rebuild" notice after a run |
| 3 | Renderer choice | Vulkan only, stated honestly | Keep `renderer` normalised to vulkan with a log line; no Vulkan → `doctor` + message box | Hide the OpenGL choice for nxrs games (fixed "Vulkan" text) |
| 4 | Docked / handheld | Console mode told to the game, TV default, named as sharper vs lighter; resolution separate | Read the profile's `op_mode` as default | Keep "Screen mode" as is |
| 5 | Controller layout | Xbox labels by default for new inis, `switch` as a setting; Nintendo pads always by label | `button_layout` default `xbox` (new inis only); Nintendo-pad rule in `input/layout.rs` | Default in Settings > Controller becomes Xbox |
| 6 | Console-style prompts | Keep controller prompt, Home quit and error notice on; profile picker as an option, off | `ask_user` option + picker dialog; localised prompt texts | None |
| 7 | Build -O and options | One measured flag set; no compiler options for players | None (keep `-O` accepted and ignored) | Hide the Optimization row for nxrs games |
| 8 | Focus / background | Keep running by default; pause as an option (exists); add mute | `mute_on_focus_loss`; release held pads; skip presenting while minimised | Mute toggle beside the pause setting |
| 9 | Distribution | Engine binaries in the public launcher release, no engine source | Licence check against PORT_MAP before the first release | Set `NXRS_ENGINE_IN_RELEASE`; Linux bundle later |
| 10 | Linux | Experimental until a real-GPU run with sound; then released | Real-GPU verification run | Bundle `nxrs-engine/` in the Linux package after that |
