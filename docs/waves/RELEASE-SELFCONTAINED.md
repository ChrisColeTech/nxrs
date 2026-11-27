# RELEASE-SELFCONTAINED - the release builds games with nothing else installed

**Status: in progress (2026-11-27).** Branch `release-selfcontained`, base `gameplay-harness` `c6940f4`.

Owner: the release must work out of the box (download, unpack, run, build a game) with nothing else to install: no
LLVM, no Visual Studio Build Tools, no distro packages. The README's "Also install" column goes.

## Plan

### Windows

The engine already builds games without Visual Studio: the bundled llvm-mingw (`toolchain\llvm-mingw`) and the engine
built for `x86_64-pc-windows-gnullvm` (`nxrs-engine\gnu\`). `cc::use_gnu()` picks it when `msvc_ready()` is false
(no Visual Studio, no clang, or no `nxfront.lib`), and `llvm_mingw_clang()` finds `..\toolchain\llvm-mingw` beside
`nxrs-build.exe`. What was missing is the release: `release.yml` passed `-SkipGnu`, and `build.ps1` only copied a
gnullvm engine somebody had built by hand.

- `scripts/build/build.ps1`: builds the gnullvm engine itself (`cargo build --target x86_64-pc-windows-gnullvm -p front
  -p runtime`, C parts with llvm-mingw's clang), takes `libwindows.0.52.0.a` from the cargo registry
  (`windows_x86_64_gnullvm-0.52.6`, byte-identical to `build\tools\gnu-libs`), copies the trimmed llvm-mingw, and
  hash-checks the gnu libraries like the MSVC ones. A missing llvm-mingw is an error (no silent skip); `-SkipGnu`
  stays for a machine that does not want it.
- `release.yml`: the gnullvm Rust target, llvm-mingw 20261104 (the version `ci.yml` pins) unpacked into
  `build\tools\llvm-mingw`, no `-SkipGnu`, and a check that the zip's own llvm-mingw links the gnu engine.
- `cc.rs` `version_of`: the object key leaves out `Configuration file:` as it does `InstalledDir:` (llvm-mingw prints
  its path there, so moving the folder recompiled every object).

### Linux

- Codecs statically: `scripts/build/linux-deps.sh codecs <prefix>` builds opus 1.5.2, libvpx 1.14.0 (VP8/VP9
  decoders), dav1d 1.5.1 and FFmpeg 7.1.1 (H.264 and AAC decoders only) as position-independent static libraries.
  With `NXRS_CODECS=<prefix>`, `src/os/build.rs` links them `static=` (rustc bundles them into `libnxfront.a`), and
  nxrs-build (built in the same cargo run) links games with `-lasound` alone.
- The compiler: `scripts/build/linux-deps.sh toolchain <dir>` puts LLVM 23.1.5's clang and lld (the official release,
  the same LLVM as llvm-mingw 20261104) into `nxrs/toolchain/clang` with the libstdc++ and zlib they load
  (`RUNPATH $ORIGIN/../lib`), a sysroot (glibc's headers, start files and link libraries, GCC's start files and
  libgcc, ALSA's `libasound.so.2` to link against) and `x86_64-unknown-linux-gnu.cfg` / `clang.cfg` pointing clang at
  it. nxrs-build already looks for `../toolchain/clang/bin/clang`.
- `release.yml` (linux job): builds the codecs before the engine, bundles the toolchain, and in a clean `ubuntu:24.04`
  container (no compiler, no -dev packages) uses only the tarball to compile and link a shared object that pulls
  `nxfront_main` out of `libnxfront.a` - what every game link does.
- `nxrs-build doctor`: with the codecs built in, the codec library checks go (only `libvulkan.so.1` and
  `libasound.so.2`, which a desktop has, remain); `package_engine.py`'s `requires` says the same.

### README

The Install section: download for Windows or Linux, unpack anywhere and run nxrs; a 64-bit PC with a Vulkan graphics
card. The "Also install" column goes. The rest stays.

## Verification

- Windows: `build.ps1` on this machine (gnullvm engine + llvm-mingw in `dist\nxrs`); a small title built with
  `NXRS_TOOLCHAIN=gnu` through the normal CLI into its library build folder, run with `scripts/play.py`
  (`NXGFX_GPU=1`), then the MSVC build of the same title, same scenario; FPS compared. The gnu game's imports (no
  `vcruntime140.dll`). A machine without Visual Studio: Windows Sandbox if present.
- Linux: WSL with a clean distro if present.
- Zip size before and after.
