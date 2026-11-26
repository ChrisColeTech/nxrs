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
    $args = @('+nightly', 'build', '--release', '-Zjson-target-spec', '-Zbuild-std=std,panic_abort', '--target', $target)
    foreach ($c in $Crates) { $args += @('-p', $c) }
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "cargo build for the web target failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}
