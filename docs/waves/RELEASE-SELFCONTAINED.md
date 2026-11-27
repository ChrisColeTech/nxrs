# RELEASE-SELFCONTAINED - the release builds games with nothing else installed

**Status: built (2026-11-27). Windows verified on this machine (Beach Bounce through the gnu path, same FPS as MSVC);
no Windows Sandbox and no WSL on this machine, so the clean-machine checks are CI steps and local equivalents
(section 4).** Branch `release-selfcontained`, base `gameplay-harness` `c6940f4`.

Owner: the release must work out of the box (download, unpack, run, build a game) with nothing else to install: no
LLVM, no Visual Studio Build Tools, no distro packages. The README's "Also install" column goes.

## 1. Plan (written before the code)

Windows: the engine already built games without Visual Studio (the bundled llvm-mingw in `toolchain\llvm-mingw`, the
engine built for `x86_64-pc-windows-gnullvm` in `nxrs-engine\gnu\`); `release.yml` passed `-SkipGnu` and `build.ps1`
only copied a gnullvm engine somebody had built by hand. Ship them. Linux: bundle a pinned LLVM (clang + lld) with a
sysroot, and link the codecs statically, so a game needs only the GPU driver (and ALSA, which a desktop has).
Settings > Engine (`nxrs-build doctor`) and the README follow.

## 2. Built

### Windows

- `scripts/build/build.ps1` builds the gnullvm engine itself (`cargo build --target x86_64-pc-windows-gnullvm -p front
  -p runtime`, C parts with llvm-mingw's clang, as `ci.yml`'s gnullvm job), **without line tables**
  (`CARGO_PROFILE_RELEASE_DEBUG=0` for that build only: ld.lld kept them as DWARF in every game DLL, while lld-link
  drops the MSVC engine's since no PDB is written; `libnxfront.a` 293 -> 126 MB, Beach Bounce's DLL 275 -> 140 MB, MSVC
  110 MB). `libwindows.0.52.0.a` comes from the cargo registry (`windows_x86_64_gnullvm-0.52.6`, byte-identical to
  `build\tools\gnu-libs`). The gnu libraries are hash-checked like the MSVC ones. A missing llvm-mingw is an error
  (`-LlvmMingw <dir>`, default `build\tools\llvm-mingw`); `-SkipGnu` stays for whoever wants the old layout.
- llvm-mingw trimmed further: besides the other targets, lldb, clangd, Python, also Linux compiler-rt, sanitizer and
  fuzzer runtimes, other architectures' runtimes (`lib\clang\23\lib\windows` keeps builtins, profile, stats, orc).
- **The Visual C++ runtime, app-local.** `nxrs.exe` and `nxrs-build.exe` import `VCRUNTIME140.dll`, `spirv-val.exe`
  also `MSVCP140.dll` and `VCRUNTIME140_1.dll`; Windows has the UCRT but not these, so a PC without the Visual C++
  Redistributable could not even start the launcher. `build.ps1` copies the three from Visual Studio's redist folder
  (`VC\Redist\MSVC\<ver>\x64\Microsoft.VC14x.CRT`, redistributable) beside each of the three programs. The gnu game
  itself imports only system DLLs and the in-box UCRT (`api-ms-win-crt-*`), no `vcruntime140.dll`.
- `.github/workflows/release.yml` (windows): the gnullvm Rust target, llvm-mingw `20261104` (the version `ci.yml`
  pins, `LLVM_MINGW`) unpacked into `build\tools\llvm-mingw`, `build.ps1 -Jobs 4` without `-SkipGnu`, and a step that
  runs the zip's `nxrs-build doctor` with `NXRS_TOOLCHAIN=gnu` and fails unless compiler, runtime library and engine
  library are ok and inside `dist\nxrs`.
- Toolchain choice (`cc::use_gnu`, unchanged, checked): `NXRS_TOOLCHAIN` forces one; else MSVC only when
  `msvc_ready()` (an MSVC `nxfront.lib` beside nxrs-build, a clang found, **and** Visual Studio's VC tools per
  vswhere / `VCToolsInstallDir`), else the bundled llvm-mingw when `llvm_mingw_clang()` finds it, which looks at
  `..\toolchain\llvm-mingw\bin\clang.exe` beside `nxrs-build.exe`: exactly the release layout. On a PC without
  Visual Studio, `have_visual_studio()` is false, so the bundled one is chosen; the doctor then does not check for
  Visual Studio at all (`!gnu` only).
- `cc.rs` `toolchain_id` (was inline in `version_of`): the object key also leaves out `Configuration file:` (llvm-mingw
  and the Linux bundle print their own path there, so a moved or re-unpacked release recompiled every object).

### Linux

- `scripts/build/linux-deps.sh codecs <prefix>`: opus 1.5.2 and libvpx 1.14.0 (the versions in `src/os/third_party`
  for Windows), dav1d 1.5.1, FFmpeg 7.1.1 with only the H.264 and AAC decoders and parsers (what `ffdec.c` opens), all
  static and position-independent. `src/os/build.rs`: with `NXRS_CODECS=<prefix>` they are linked `static=` (rustc
  bundles them into the engine's staticlib, as on Windows) and `ffdec.c` compiles against `<prefix>/include`;
  without it, the old pkg-config path. `cc.rs` `STATIC_CODECS` (`option_env!("NXRS_CODECS")`, the same cargo run
  builds nxrs-build): game links get `-lasound` alone instead of `-lopus -lvpx -ldav1d -lavcodec -lavutil -lasound`.
- `scripts/build/linux-deps.sh toolchain <dir>` -> `nxrs/toolchain/clang` (where `candidates_linux` already looks:
  `../toolchain/clang/bin/clang` beside nxrs-build): LLVM 23.1.5's official `clang-23` and `lld` (the LLVM of
  llvm-mingw 20261104; `ld.lld` beside clang, as `find_lld` wants), clang's headers, the `libstdc++.so.6`,
  `libgcc_s.so.1` and `libz.so.1` they load (checked: their RUNPATH is `$ORIGIN/../lib`; NEEDED libm, libz,
  libstdc++, libgcc_s, libc; GLIBC_2.34, GLIBCXX_3.4.30), and `sysroot/`: libc6-dev's and linux-libc-dev's headers,
  glibc's start files and link libraries (the `libc.so` / `libm.so` linker scripts resolve inside the sysroot through
  `lib -> usr/lib`, `lib64 -> usr/lib64`), GCC's start files and libgcc, and ALSA's `libasound.so.2` to link against
  (the player's own runs). `x86_64-unknown-linux-gnu.cfg` and `clang.cfg` beside clang say
  `--sysroot=<CFGDIR>/../sysroot`; Debian copyright files and LLVM's license in `legal/`.
- `release.yml` (linux): codecs built before the engine (`NXRS_CODECS` in the job's environment), no codec -dev
  packages, the toolchain bundled into `stage/nxrs`, then `linux-deps.sh check` in a clean `ubuntu:24.04` container
  with only the unpacked release mounted: compiles and runs a program with the loader's headers, and links
  `libnxfront.a`'s `nxfront_main` and all it pulls the way `link_so` links a game, failing on any unresolved codec,
  ALSA, C library or unwinder symbol; `nxrs-build doctor` is printed too.
- `nxrs-build doctor` (Settings > Engine): with the codecs built in, the five codec library checks become one line
  "codecs: ... built into the engine"; ALSA and `libvulkan.so.1` stay (a desktop has them). The lld failure no longer
  says "install lld". `package_engine.py`'s `requires` lists only the Vulkan driver and ALSA when `NXRS_CODECS` is set.

### README, docs

README's Install section: download for Windows or Linux, unpack anywhere, run nxrs; nothing else to install; a 64-bit
PC (AVX2) with a Vulkan graphics card; the "Also install" column is gone, links kept, the rest unchanged.
`docs/LAUNCHER_CONTRACT.md` release layout and `ARCHITECTURE-AND-STYLE-GUIDE.md` section 7 say the same.

## 3. Verified (this machine, 2026-11-27)

`build.ps1 -KeepCache` (below-normal priority) built `dist\nxrs` with the gnullvm engine, the trimmed llvm-mingw and
the VC runtime. `NXRS_TOOLCHAIN=gnu nxrs-build doctor`: toolchain "llvm-mingw + the gnullvm engine", compiler
`dist\nxrs\nxrs-engine\..\toolchain\llvm-mingw\bin\clang.exe` (clang 23.1.5), runtime and engine libraries from
`dist\nxrs\nxrs-engine\gnu`: the release workflow's check, run locally, passes.

Beach Bounce Remastered (library build folder backed up, built through the normal CLI relink
`dist\nxrs\nxrs.exe --select-title "Beach Bounce Remastered" --build` with `NXRS_TOOLCHAIN=gnu` / `msvc`, folder
restored afterwards), `py scripts/play.py beach-bounce --env NXGFX_GPU=1`, one game at a time, nothing else running
a game (another agent's MKW run had finished):

| run | toolchain | FPS mean / 5% low / min | frame ms mean / p99 | replay ms | CPU % | anomalies |
|---|---|---|---|---|---|---|
| `build/play/20261127-143219-gnu` | gnu (engine with line tables) | 60.0 / 59.3 / 58 | 16.68 / 18.71 | 1.21 | 45 | none |
| `build/play/20261127-143751-msvc` | MSVC (clang 21.1.8) | 59.9 / 59.0 / 55 | 16.69 / 18.30 | 1.19 | 46 | none |
| `build/play/20261127-144759-gnu-final` | gnu (final engine, no line tables) | 60.0 / 59.7 / 57 | 16.68 / 18.08 | 1.28 | 47 | none |

Window [60, 250] s, 47/47 steps, normal exit, no red lines. All three sheets show the same pictures at the same
moments (title menu at 30 s, Grandma Umi at 88 s, Tomoyo at 147 s, Yuuki at the shop at 205 s). The title runs at its
60 FPS cap either way; frame time, replay time and CPU use match within run-to-run noise. Build times: gnu compile
115 s (clang 23.1.5), MSVC 89 s (the system's clang 21.1.8), same 198 units.

Imports: the gnu `_code.dll` imports only Windows DLLs and the UCRT (advapi32, kernel32, user32, gdi32, ole32,
mfplat, ..., `api-ms-win-crt-*`), the loader exe kernel32, user32 and the UCRT; no `vcruntime140.dll`, no llvm-mingw
DLL. The launcher started from `dist\nxrs` loads `dist\nxrs\VCRUNTIME140.dll` (process modules), not a system copy.

Zip (`Compress-Archive`-equivalent deflate, measured on `dist\nxrs`): **110.8 MB -> 256.2 MB (+145 MB)**: the bundled
llvm-mingw 87.9 MB (339 MB unpacked), the gnullvm engine 55.6 MB (145 MB unpacked), the VC runtime 0.8 MB. (Before the
debug-info and extra trim: 316 MB.) The Linux tarball grows by the toolchain (clang-23 282 MB + lld 213 MB unpacked,
roughly 90-110 MB compressed; not measured, it is built only in CI) and the codecs inside `libnxfront.a`.

Tests: `cargo test --release -p nxrs-cli` 48 passed, including the new
`cc::tests::toolchain_id_leaves_out_where_the_toolchain_is_unpacked` (two `--version` texts from different folders
give one id; the old filter kept the `Configuration file:` line, so it would have failed). `bash -n
scripts/build/linux-deps.sh` passes.

## 4. What could not be run here, and what was checked instead

- **Windows Sandbox:** not on this machine (`C:\Windows\System32\WindowsSandbox.exe` absent; enabling the optional
  feature needs an elevated prompt and a reboot), no Docker or Hyper-V tooling either, and Visual Studio cannot be
  hidden from vswhere. Checked instead: the gnu build forced with `NXRS_TOOLCHAIN=gnu` in an environment with no
  `LIB`, `INCLUDE` or `VCToolsInstallDir`; `clang -v` of the bundled compiler (PATH = System32 only) shows only
  `dist\nxrs\toolchain\llvm-mingw` include and library paths and its own `ld.lld`, and its hello-world runs; the
  game's imports (above); the VC runtime of the three programs ships app-local and is the copy loaded; the selection
  logic read in source (section 2). The one unproven piece is the automatic choice on a real Visual-Studio-free PC.
- **WSL:** not installed (`wsl -l` answers `WSL_E_WSL_OPTIONAL_COMPONENT_REQUIRED`; installing needs elevation and a
  reboot), so nothing Linux ran here. Checked instead: the LLVM 23.1.5 Linux release downloaded and its `clang-23` /
  `lld` inspected with `llvm-readelf` (NEEDED, RUNPATH, symbol versions: what `lib/` must carry); the config-file
  mechanism with the same clang 23 driver (llvm-mingw's `clang-23.exe` with `x86_64-unknown-linux-gnu.cfg` and
  `clang.cfg` beside it: `--target=x86_64-unknown-linux-gnu -###` loads both and passes
  `--sysroot=<bin>/../sysroot` to the link). The codec builds, the sysroot copy and the clean-container check run
  for the first time in `release.yml` on the next push to `main`; its `check` step is the proof (it fails the job if
  the tarball cannot compile and link on a system with no compiler and no -dev packages).
- Linux limits that remain: the tarball is built on ubuntu-24.04, so games (like the launcher already) need glibc
  2.39 or newer; the launcher itself still loads the desktop's GTK 3. A player needs `libasound.so.2` and a Vulkan
  driver, which every desktop distribution has.

## 5. Question for the owner

Ship only the gnullvm engine? The gnu path ran Beach Bounce exactly like the MSVC one; dropping `nxfront.lib` /
`nxrt.lib` would take the zip from 256 MB to about 186 MB and leave one toolchain to support (the code already
handles a gnu-only release). Recommendation: yes, after a heavier title (MK8D or MKW) is compared once on both paths,
since Beach Bounce runs at its cap.
