<#
.SYNOPSIS
  Optimised release build into dist/nxrs, then keeps target/ small.
.DESCRIPTION
  Builds the release profile of the engine (nxrs-build with the MSVC engine libraries nxfront.lib and
  nxrt.lib) and of the launcher (src/launcher, its own workspace, building into the same target/), and
  assembles dist/nxrs as a release is laid out:
    nxrs.exe                 the launcher
    nxrs-engine/             nxrs-build.exe, engine.json (its contract), nxfront.lib + nxrt.lib, tools/
                             (dmsh_xlate, spirv-val), LICENSES/; gnu/ too when the gnullvm engine is built
    toolchain/llvm-mingw/    with the gnullvm engine: the compiler a machine without Visual Studio needs
    legal/LICENSE.txt
  The launcher finds nxrs-engine/ beside itself, so dist/nxrs runs on its own.
  Small is the DEFAULT: after copying to dist/ it deletes cargo's release cache (deps/, build/,
  .fingerprint/) and prunes target/debug, so the next build is a full rebuild (a few minutes).
  -KeepCache keeps the cache for fast incremental builds. The lane folders other work builds into
  (target/L2, target/K, ...: CARGO_TARGET_DIR) are never touched.
.EXAMPLE
  scripts/build/build.ps1 -Run          # build, assemble dist/nxrs, prune, start dist/nxrs/nxrs.exe
  scripts/build/build.ps1 -KeepCache    # fast incremental builds; target/release stays large
#>
[CmdletBinding()]
param(
    [double]$BudgetGB = 5,
    [switch]$Run,
    [switch]$NoPrune,
    [switch]$KeepCache
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$target = Join-Path $root 'target'   # both workspaces build here (src/launcher/.cargo/config.toml)
$release = Join-Path $target 'release'
$dist = Join-Path $root 'dist\nxrs'
$engine = Join-Path $dist 'nxrs-engine'

if (Get-Process nxrs, nxrs-build -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dist*" }) {
    throw 'an nxrs from dist/ is running; close it first.'
}

Push-Location $root
try {
    $env:CARGO_INCREMENTAL = '0'   # release never uses it; a user config must not turn it on
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue   # never a lane folder
    cargo build --release --locked --jobs 4 -p nxrs-cli -p front -p runtime
    if ($LASTEXITCODE -ne 0) { throw "engine build failed ($LASTEXITCODE)" }
    cargo build --release --locked --jobs 4 --manifest-path src\launcher\Cargo.toml
    if ($LASTEXITCODE -ne 0) { throw "launcher build failed ($LASTEXITCODE)" }
} finally { Pop-Location }

# the tools a build runs: the shader translator and the SPIR-V validator
$xlate = Join-Path $root 'build\tools\dmsh_xlate\dmsh_xlate.exe'
$spirv = if ($env:VULKAN_SDK) { Join-Path $env:VULKAN_SDK 'Bin\spirv-val.exe' } else { '' }
if (-not (Test-Path $xlate)) { throw "missing $xlate (the shader translator a build runs)" }
if (-not $spirv -or -not (Test-Path $spirv)) { throw 'missing spirv-val: install the Vulkan SDK (VULKAN_SDK)' }

if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Force (Join-Path $engine 'tools') | Out-Null
Copy-Item (Join-Path $release 'nxrs.exe') $dist
foreach ($file in 'nxrs-build.exe', 'nxfront.lib', 'nxrt.lib') { Copy-Item (Join-Path $release $file) $engine }
Copy-Item $xlate (Join-Path $engine 'tools')
Copy-Item $spirv (Join-Path $engine 'tools')
Copy-Item (Join-Path $root 'LICENSES') (Join-Path $engine 'LICENSES') -Recurse

# the gnullvm engine, when it has been built: it links with the bundled llvm-mingw (no Visual Studio)
$gnu = Join-Path $target 'x86_64-pc-windows-gnullvm\release'
$llvm = Join-Path $root 'build\tools\llvm-mingw'
if ((Test-Path (Join-Path $gnu 'libnxfront.a')) -and (Test-Path (Join-Path $llvm 'bin\clang.exe'))) {
    New-Item -ItemType Directory -Force (Join-Path $engine 'gnu') | Out-Null
    foreach ($lib in 'libnxfront.a', 'libnxrt.a') { Copy-Item (Join-Path $gnu $lib) (Join-Path $engine 'gnu') }
    Copy-Item (Join-Path $root 'build\tools\gnu-libs\libwindows.0.52.0.a') (Join-Path $engine 'gnu')
    $skip = 'aarch64-w64-mingw32', 'armv7-w64-mingw32', 'i686-w64-mingw32', 'python'
    robocopy $llvm (Join-Path $dist 'toolchain\llvm-mingw') /MIR /XD @skip /XF lldb* liblldb* clangd* clang-tidy* libpython* *.pdb /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy llvm-mingw failed ($LASTEXITCODE)" }
    $global:LASTEXITCODE = 0
}

# engine.json: the contract the launcher reads without starting nxrs-build
$answer = & (Join-Path $engine 'nxrs-build.exe') contract
if ("$answer" -notmatch 'nxrs-contract (\d+)') { throw "nxrs-build does not answer 'contract': $answer" }
$commit = (git -C $root rev-parse --short=12 HEAD 2>$null)
$info = [ordered]@{ contract = [int]$matches[1]; engine = 'nxrs'; version = "$commit"; platform = 'windows-x64' }
$info | ConvertTo-Json | Set-Content (Join-Path $engine 'engine.json') -Encoding utf8
New-Item -ItemType Directory -Force (Join-Path $dist 'legal') | Out-Null
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $dist 'legal\LICENSE.txt')

$gb = (Get-ChildItem $dist -Recurse -File | Measure-Object Length -Sum).Sum / 1GB
Write-Host ("dist/nxrs: {0:N2} GB  ({1})" -f $gb, $dist)

$Lean = -not $KeepCache   # small by default; -KeepCache trades disk for incremental speed
if ($Lean) {
    foreach ($d in 'deps', 'build', '.fingerprint', 'incremental', 'examples') {
        $p = Join-Path $release $d
        if (Test-Path $p) { Remove-Item $p -Recurse -Force }
    }
    Write-Host 'lean: release cache removed (next build is a full rebuild)'
}
# small mode: unless a budget is given, prune the default profiles down (target/debug goes)
if ($Lean -and -not $PSBoundParameters.ContainsKey('BudgetGB')) { $BudgetGB = 0.5 }
if (-not $NoPrune) { & (Join-Path $PSScriptRoot 'prune.ps1') -BudgetGB $BudgetGB }

if ($Run) { Start-Process (Join-Path $dist 'nxrs.exe') }
