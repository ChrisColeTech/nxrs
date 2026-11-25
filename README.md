# nxrs

[![CI](https://github.com/ChrisColeTech/nxrs/actions/workflows/ci.yml/badge.svg)](https://github.com/ChrisColeTech/nxrs/actions/workflows/ci.yml)
[![Launcher](https://github.com/ChrisColeTech/nxrs/actions/workflows/launcher.yml/badge.svg)](https://github.com/ChrisColeTech/nxrs/actions/workflows/launcher.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A Rust static recompiler and launcher for Switch and Switch 2 games. Add your NSP, NUP or game dump, build, play.

> [!IMPORTANT]
> No games, keys or firmware are included. Use your own legally obtained dumps and console keys.

![The nxrs library](docs/images/readme/library.png)

## Install

Download from the [latest release](https://github.com/ChrisColeTech/nxrs/releases/latest). You need a 64-bit CPU with
AVX2 and a graphics card with Vulkan. **Settings > Engine** checks your PC and lists anything missing.

**Windows 10 / 11:** unzip `nxrs-windows-x64.zip` anywhere and run `nxrs.exe`. Install
[LLVM](https://github.com/llvm/llvm-project/releases) and the
[Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) ("Desktop development with C++");
nxrs uses them to build each game.

**Linux:** extract `nxrs-linux-x64.tar.gz` anywhere and run `nxrs/nxrs`. Install clang and lld, a Vulkan driver, and the
ALSA, Opus, libvpx, dav1d and FFmpeg (libavcodec) libraries from your distribution.

## Build from source

With [Rust](https://rustup.rs/), the [Vulkan SDK](https://vulkan.lunarg.com/), LLVM and the Visual Studio Build Tools:

```powershell
git clone https://github.com/ChrisColeTech/nxrs
cd nxrs
scripts/build/build.ps1 -Run
```

The app is built into `dist/nxrs` and started.

## Getting started

1. **Add game** and choose an NSP, NUP, XCI or extracted game folder.
2. For NSP games, install your keys once: **Keys > Install keys** and pick your `prod.keys`.
3. Press **Build**. The first build of a game takes a few minutes.
4. Press **Play**.

Updates and DLC are added from the game's menu; choose which version to build there too.

## Games

FPS is the average during gameplay, with the slowest 5% in brackets.

| | | |
|---|---|---|
| ![Super Mario Bros. Wonder](docs/images/readme/games/wonder.jpg)<br>**Super Mario Bros. Wonder**<br>60.1 FPS (59.4) | ![Donkey Kong Bananza](docs/images/readme/games/dkb.jpg)<br>**Donkey Kong Bananza**<br>59.4 FPS (53.4) | ![Mario Kart 8 Deluxe](docs/images/readme/games/mk8d.jpg)<br>**Mario Kart 8 Deluxe**<br>59.8 FPS (58.0) |
| ![Pokémon Legends: Z-A](docs/images/readme/games/plza.jpg)<br>**Pokémon Legends: Z-A**<br>59.6 FPS (57.9) | ![Super Mario Party Jamboree](docs/images/readme/games/jamboree.jpg)<br>**Super Mario Party Jamboree**<br>56.0 FPS (36.5) | ![Mario Kart World](docs/images/readme/games/mkw.jpg)<br>**Mario Kart World**<br>28.8 FPS (25.0) |
| ![Pokémon Legends: Arceus](docs/images/readme/games/arceus.jpg)<br>**Pokémon Legends: Arceus**<br>30.0 FPS (29.5), the game's own cap | ![Anime Girls: Sun of a Beach](docs/images/readme/games/anime-girls.jpg)<br>**Anime Girls: Sun of a Beach**<br>60.0 FPS (59.8) | ![Beach Bounce Remastered](docs/images/readme/games/beach-bounce.jpg)<br>**Beach Bounce Remastered**<br>60.0 FPS (59.1) |

**While playing:** Xbox, DualSense, DualShock 4, Switch Pro and Joy-Con controllers, with rumble and motion.
**F1** controls · **F3** screenshot · **F4** FPS limit · **F6** cheats · **F7** record video · **F11** fullscreen.

## Credits

Built on mkw-recomp, LAS Recomp, totk-pc-recomp, nxnative, Ryujinx and others; see [LICENSES](LICENSES/). HD rumble
uses Three-Dola's RenHDrum 0.5.0 (MIT OR Apache-2.0, used under MIT; `src/os/third_party/renhdrum/`). nxrs is MIT
licensed and not affiliated with Nintendo.
