<#
.SYNOPSIS
  Optimised release build into dist/nxrs, then keeps target/ small.
.DESCRIPTION
  Builds the release profile of the engine (nxrs-build with the MSVC engine libraries nxfront.lib and
  nxrt.lib) and of the launcher (src/launcher, its own workspace, building into the same target/), and
  assembles dist/nxrs as a release is laid out:
    nxrs.exe                 the launcher
    nxrs-engine/             nxrs-build.exe, engine.json (its contract), nxfront.lib + nxrt.lib, tools/
                             (dmsh_xlate, spirv-val); gnu/ too when the gnullvm engine is built
    toolchain/llvm-mingw/    with the gnullvm engine: the compiler a machine without Visual Studio needs
  The launcher finds nxrs-engine/ beside itself, so dist/nxrs runs on its own.
  Lean is the DEFAULT: after dist/nxrs is assembled, ALL of target/ is removed (debug, release and
  every lane folder), so dist/nxrs is what remains and the next build is a full rebuild (a few
  minutes). -KeepCache keeps target/ for fast incremental builds. Refuses to start while cargo or
  rustc is running.
.EXAMPLE
  scripts/build/build.ps1 -Run          # build, assemble dist/nxrs, remove target/, start dist/nxrs/nxrs.exe
  scripts/build/build.ps1 -KeepCache    # fast incremental builds; target/ stays
#>
[CmdletBinding()]
param(
    [switch]$Run,
    [switch]$KeepCache,
    [ValidateRange(1, 64)][int]$Jobs = [Environment]::ProcessorCount,
    [switch]$SkipGnu,
    [switch]$RetainPrevious,
    [string]$PublicationJournal
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$target = Join-Path $root 'target'   # both workspaces build here (src/launcher/.cargo/config.toml)
$release = Join-Path $target 'release'
$dist = Join-Path $root 'dist\nxrs'
$live = $dist
$transaction = [guid]::NewGuid().ToString('N')
$stage = Join-Path $root "dist\nxrs-stage-$transaction"
$previous = Join-Path $root "dist\nxrs-previous-$transaction"

# Validate packaging prerequisites before doing compiler work.
$xlate = Join-Path $root 'build\tools\dmsh_xlate\dmsh_xlate.exe'
$spirv = if ($env:VULKAN_SDK) { Join-Path $env:VULKAN_SDK 'Bin\spirv-val.exe' } else { '' }
if (-not (Test-Path $xlate)) { throw "missing $xlate (the shader translator a build runs)" }
if (-not $spirv -or -not (Test-Path $spirv)) { throw 'missing spirv-val: install the Vulkan SDK (VULKAN_SDK)' }
if ($PublicationJournal -and (Test-Path $PublicationJournal)) { throw 'publication journal already exists' }

if (Get-Process nxrs, nxrs-build -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dist*" }) {
    throw 'an nxrs from dist/ is running; close it first.'
}
if (Get-Process cargo, rustc -ErrorAction SilentlyContinue) {
    throw 'cargo or rustc is already running: one compiler-heavy command at a time.'
}

Push-Location $root
try {
    $env:CARGO_INCREMENTAL = '0'   # release never uses it; a user config must not turn it on
    Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue   # never a lane folder
    cargo build --release --locked --jobs $Jobs --target-dir $target -p nxrs-cli -p front -p runtime
    if ($LASTEXITCODE -ne 0) { throw "engine build failed ($LASTEXITCODE)" }
    cargo build --release --locked --jobs $Jobs --manifest-path src\launcher\Cargo.toml --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw "launcher build failed ($LASTEXITCODE)" }
} finally { Pop-Location }

# the tools a build runs: the shader translator and the SPIR-V validator
$xlate = Join-Path $root 'build\tools\dmsh_xlate\dmsh_xlate.exe'
$spirv = if ($env:VULKAN_SDK) { Join-Path $env:VULKAN_SDK 'Bin\spirv-val.exe' } else { '' }
if (-not (Test-Path $xlate)) { throw "missing $xlate (the shader translator a build runs)" }
if (-not $spirv -or -not (Test-Path $spirv)) { throw 'missing spirv-val: install the Vulkan SDK (VULKAN_SDK)' }

$dist = $stage  # Existing production stays intact until all staging checks have passed.
$engine = Join-Path $dist 'nxrs-engine'
New-Item -ItemType Directory -Force (Join-Path $engine 'tools') | Out-Null
Copy-Item (Join-Path $release 'nxrs.exe') $dist
foreach ($file in 'nxrs-build.exe', 'nxfront.lib', 'nxrt.lib') { Copy-Item (Join-Path $release $file) $engine }
Copy-Item $xlate (Join-Path $engine 'tools')
Copy-Item $spirv (Join-Path $engine 'tools')

# the gnullvm engine, when it has been built: it links with the bundled llvm-mingw (no Visual Studio)
$gnu = Join-Path $target 'x86_64-pc-windows-gnullvm\release'
$llvm = Join-Path $root 'build\tools\llvm-mingw'
if (-not $SkipGnu -and (Test-Path (Join-Path $gnu 'libnxfront.a')) -and (Test-Path (Join-Path $llvm 'bin\clang.exe'))) {
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
if ($LASTEXITCODE -ne 0 -or "$answer".Trim() -ne 'nxrs-contract 2') {
    throw "nxrs-build does not answer contract 2: $answer (exit $LASTEXITCODE)"
}
$commit = (git -C $root rev-parse --short=12 HEAD 2>$null)
$info = [ordered]@{ contract = 2; engine = 'nxrs'; version = "$commit"; platform = 'windows-x64' }
$info | ConvertTo-Json | Set-Content (Join-Path $engine 'engine.json') -Encoding utf8

# Check copied bytes against this build's target artifacts before any production rename.
$hashes = [ordered]@{}
foreach ($file in 'nxrs.exe', 'nxrs-build.exe', 'nxfront.lib', 'nxrt.lib') {
    $built = Join-Path $release $file
    $packaged = if ($file -eq 'nxrs.exe') { Join-Path $dist $file } else { Join-Path $engine $file }
    $hash = (Get-FileHash -LiteralPath $built -Algorithm SHA256).Hash
    if ($hash -ne (Get-FileHash -LiteralPath $packaged -Algorithm SHA256).Hash) {
        throw "staged artifact differs from target: $file"
    }
    $hashes[$file] = $hash
}
$journal = [ordered]@{ state = 'prepared'; stage = $stage; previous = $previous; live = $live;
    had_previous = (Test-Path $live); hashes = $hashes; contract = "$answer"; jobs = $Jobs;
    skipped_gnu = [bool]$SkipGnu }
function Save-PublicationJournal {
    if ($PublicationJournal) {
        $temporary = "$PublicationJournal.tmp"
        $journal | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $temporary -Encoding utf8
        Move-Item -LiteralPath $temporary -Destination $PublicationJournal -Force
    }
}
Save-PublicationJournal
try {
    if (Test-Path $live) { Move-Item -LiteralPath $live -Destination $previous }
    Move-Item -LiteralPath $stage -Destination $live
    $journal.state = 'promoted'
    Save-PublicationJournal
} catch {
    if ((Test-Path $previous) -and -not (Test-Path $live)) {
        Move-Item -LiteralPath $previous -Destination $live
    }
    throw
}
$dist = $live
$gb = (Get-ChildItem $dist -Recurse -File | Measure-Object Length -Sum).Sum / 1GB
Write-Host ("dist/nxrs: {0:N2} GB  ({1})" -f $gb, $dist)

# lean by default: dist/nxrs is what remains; -KeepCache trades disk for incremental speed
if (-not $KeepCache) {
    Write-Host "lean: removing all of $target (next build is a full rebuild)"
    Remove-Item $target -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path $target) {
        # a file still in use (a running game or tool from target/) keeps its folder; say which
        $left = (Get-ChildItem $target -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object).Count
        Write-Warning "$left file(s) in target/ are in use and were left; close what runs from target/ and remove it"
    }
}

if (-not $RetainPrevious -and (Test-Path $previous)) { Remove-Item $previous -Recurse -Force }
if ($Run) { Start-Process (Join-Path $dist 'nxrs.exe') }
