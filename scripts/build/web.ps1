# Builds nxrs's Rust crates for the web target (docs/35): wasm64 with Emscripten's libc and pthreads, through the
# custom target scripts/build/wasm64-unknown-emscripten.json (Rust ships no 64-bit Emscripten target), with nightly
# Rust and the standard library built from source. The static libraries land in target/wasm64-unknown-emscripten/.
#
#   pwsh scripts/build/web.ps1                  # the runtime crate
#   pwsh scripts/build/web.ps1 -Crates runtime,os,graphics
param(
    [string[]]$Crates = @('runtime'),
    [string]$Emsdk = (Join-Path $PSScriptRoot '..\..\build\tools\emsdk')
)
$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$target = Join-Path $PSScriptRoot 'wasm64-unknown-emscripten.json'

# The pinned nightly (the one the W1 probe and crates were proven with); a different one is refused, not used.
$pinned = 'rustc 1.102.0-nightly (6c709eb42 2026-11-25)'
$have = (& rustc +nightly --version) 2>$null
if ($have -ne $pinned) {
    throw "nightly Rust is '$have', the web build is pinned to '$pinned' (rustup toolchain install nightly --component rust-src)"
}

# The libc crate's Emscripten definitions are 32-bit: on a 64-bit memory nlink_t, blksize_t, struct statvfs and
# struct msghdr differ from Emscripten's C (std::fs::metadata read every file as 0 bytes). A copy of the libc version
# std uses, with scripts/build/libc-wasm64.patch applied, replaces it for the standard library built here (the
# [patch.crates-io] of the nightly toolchain's library workspace; a workspace [patch] does not reach std).
$libcVersion = '0.2.191'
$libcSrc = Get-ChildItem (Join-Path $env:USERPROFILE '.cargo\registry\src') -Directory |
    ForEach-Object { Join-Path $_.FullName "libc-$libcVersion" } | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $libcSrc) { throw "libc $libcVersion is not in the cargo registry (build std once for the web target, or cargo fetch)" }
# A worktree's build\tools is a junction into the main checkout: name the copy by its real folder, so every worktree
# writes the same line into the shared toolchain (a different spelling makes cargo build std again for the others).
$tools = Get-Item (Join-Path $repo 'build\tools')
$toolsDir = if ($tools.LinkType -eq 'Junction') { $tools.Target } else { $tools.FullName }
$libcWeb = Join-Path $toolsDir 'libc-wasm64'
$patch = Join-Path $PSScriptRoot 'libc-wasm64.patch'
$stamp = Join-Path $libcWeb '.nxrs-patch'
$want = (Get-FileHash $patch).Hash
if (-not (Test-Path $stamp) -or (Get-Content $stamp) -ne $want) {
    if (Test-Path $libcWeb) { Remove-Item -Recurse -Force $libcWeb }
    Copy-Item -Recurse $libcSrc $libcWeb
    # from the repository's root with --directory: git apply run inside a subfolder of a repository silently skips
    # the paths it takes as outside that folder
    & git -C $repo apply -p0 --whitespace=nowarn --directory=build/tools/libc-wasm64 $patch
    $mod = Get-Content (Join-Path $libcWeb 'src\unix\linux_like\emscripten\mod.rs') -Raw
    if ($LASTEXITCODE -ne 0 -or $mod -notmatch 'pub type nlink_t = u64') {
        throw "applying $patch to libc $libcVersion failed"
    }
    Set-Content $stamp $want
    # cargo does not see the standard library's libc change: build everything for the web target again
    $webTarget = Join-Path $repo 'target\wasm64-unknown-emscripten'
    if (Test-Path $webTarget) { Remove-Item -Recurse -Force $webTarget }
}
$libraryToml = Join-Path (& rustc +nightly --print sysroot) 'lib\rustlib\src\rust\library\Cargo.toml'
$patchLine = "libc = { path = '$($libcWeb -replace '\\', '/')' }  # nxrs web target (scripts/build/web.ps1)"
$toml = Get-Content $libraryToml -Raw
if ($toml -notmatch [regex]::Escape($patchLine)) {
    $toml = ($toml -split "`n" | Where-Object { $_ -notmatch 'nxrs web target' }) -join "`n"
    $toml = $toml -replace "(\[patch\.crates-io\]\r?\n)", "`$1$patchLine`n"
    [System.IO.File]::WriteAllText($libraryToml, $toml)
}

# C code inside crates (zstd-sys) is compiled by emcc, run by emsdk's own Python and config.
$emsdk = Resolve-Path $Emsdk
$python = Get-ChildItem (Join-Path $emsdk 'python') -Directory | ForEach-Object { Join-Path $_.FullName 'python.exe' } |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $python) { throw "no Python in $emsdk\python (install the Emscripten SDK into build\tools\emsdk)" }
$env:EM_CONFIG = Join-Path $emsdk '.emscripten'
$env:PATH = "$(Split-Path $python);$emsdk\upstream\bin;$emsdk\upstream\emscripten;$env:PATH"
$env:CC_wasm64_unknown_emscripten = Join-Path $emsdk 'upstream\emscripten\emcc.exe'
$env:AR_wasm64_unknown_emscripten = Join-Path $emsdk 'upstream\emscripten\emar.exe'
$env:CFLAGS_wasm64_unknown_emscripten = '-sMEMORY64=1 -pthread'

Push-Location $repo
try {
    # libraries only: the crates' development binaries (nxrt-dev, nxos-dev) are native tools
    $args = @('+nightly', 'build', '--release', '--lib', '-Zjson-target-spec', '-Zbuild-std=std,panic_abort', '--target', $target)
    foreach ($c in $Crates) { $args += @('-p', $c) }
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "cargo build for the web target failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}
