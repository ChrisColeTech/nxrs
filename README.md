# nxrs

**A game launcher and static recompiler for your Nintendo Switch games.** Point it at a dump you own (NSP, NUP, XCI or
an extracted game folder) and nxrs rebuilds the game as a native Windows program, then runs it from a library like
any other PC game.

![The nxrs library](docs/images/readme/library.png)

## Features

**Library**
- Add games from NSP, NUP, XCI or extracted game folders; box art, gallery or list view, search, filter and sort.
- One click to build, one click to play. Build progress and logs in the build-output panel.
- Updates and DLC: install update and DLC packages, choose which version to build.
- Per-game settings: docked or handheld, Switch or Xbox button layout, artwork, compiler workers.
- Built-in checks that your PC has what a build needs (compiler, Vulkan, keys, disk space).

**In game**
- Native x86-64 code (no emulation of the CPU), Vulkan graphics.
- Controllers: Xbox, DualSense / DualShock 4, Switch Pro and Joy-Con, with rumble and motion.
- F1 controls page · F3 screenshot · F4 FPS limit · F6 cheats · F7 record video · F11 fullscreen.
- Game mods and cheats, saves kept per game.

| Per-game settings | Search & filter |
|---|---|
| ![Per-game settings](docs/images/readme/game-settings.png) | ![Search and filter](docs/images/readme/filters.png) |

## Getting started

**You need:** Windows 10/11 x64 with an AVX2 CPU (x86-64-v3), a Vulkan GPU, [LLVM/clang](https://releases.llvm.org/),
the Visual Studio C++ build tools, and your own Nintendo keys (`prod.keys`) and game dumps. nxrs ships no games or keys.

1. Start `nxrs.exe`. **Keys > Install keys** and pick your `prod.keys`.
2. **Add game** and pick an NSP, NUP, XCI or game folder.
3. **Build**: the first build of a game takes a few minutes; later ones reuse what was built.
4. **Play.**

## Building from source

Rust (stable), the Vulkan SDK, LLVM and the Visual Studio C++ build tools:

```powershell
scripts/build/build.ps1 -Run     # builds the engine and launcher into dist/nxrs and starts it
```

`dist/nxrs` is the complete app (`nxrs.exe` plus `nxrs-engine/`). For developers: `AGENTS.md` and
`docs/ARCHITECTURE-AND-STYLE-GUIDE.md` describe the layout and rules.

## Credits

nxrs builds on the work of mkw-recomp, LAS Recomp, totk-pc-recomp, nxnative, Ryujinx, MacNativeGames, deko3d, lsfg-vk,
lztans, dav1d, libvpx, libopus, lewton, cpal and llvm-mingw. Thank you. Their licenses are in [`LICENSES/`](LICENSES/).

nxrs is MIT licensed ([LICENSE](LICENSE)). It is not affiliated with Nintendo.
