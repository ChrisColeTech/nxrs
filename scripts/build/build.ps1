<#
.SYNOPSIS
  Optimised release build into dist/nxrs, then keeps target/ small.
.DESCRIPTION
  Builds the release profile of the engine (nxrs-build with the MSVC engine libraries nxfront.lib and
  nxrt.lib, and the same engine for x86_64-pc-windows-gnullvm with the bundled llvm-mingw) and of the
  launcher (src/launcher, its own workspace, building into the same target/), and assembles dist/nxrs as
  a release is laid out:
    nxrs.exe                 the launcher
    nxrs-engine/             nxrs-build.exe, engine.json (its contract), nxfront.lib + nxrt.lib, tools/
                             (dmsh_xlate, spirv-val), gnu/ (libnxfront.a, libnxrt.a, libwindows.0.52.0.a)
    toolchain/llvm-mingw/    the compiler and linker games are built with on a machine without Visual
                             Studio (x86_64 only: the other targets, lldb, clangd and Python are left out)
  So the release builds games with nothing else installed. -LlvmMingw names the llvm-mingw folder
  (default build\tools\llvm-mingw: llvm-mingw 20261104, ucrt-x86_64, as ci.yml and release.yml pin it).
  -SkipGnu leaves out gnu/ and toolchain/ (a machine with Visual Studio and LLVM only).
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
    [string]$LlvmMingw,
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
$llvm = if ($LlvmMingw) { $LlvmMingw } else { Join-Path $root 'build\tools\llvm-mingw' }
if (-not $SkipGnu -and -not (Test-Path (Join-Path $llvm 'bin\clang.exe'))) {
    throw "missing $llvm\bin\clang.exe: unpack llvm-mingw-20261104-ucrt-x86_64.zip (github.com/mstorsjo/llvm-mingw) there, or pass -SkipGnu"
}

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
    if (-not $SkipGnu) {
        # the gnullvm engine: the same front end and runtime as static libraries llvm-mingw links (C parts built by
        # llvm-mingw's clang)
        if ((rustup target list --installed) -notcontains 'x86_64-pc-windows-gnullvm') {
            rustup target add x86_64-pc-windows-gnullvm
            if ($LASTEXITCODE -ne 0) { throw "rustup target add x86_64-pc-windows-gnullvm failed ($LASTEXITCODE)" }
        }
        $path = $env:PATH
        try {
            $env:PATH = "$(Join-Path $llvm 'bin');$path"
            $env:CC_x86_64_pc_windows_gnullvm = 'x86_64-w64-mingw32-clang'
            cargo build --release --locked --jobs $Jobs --target-dir $target --target x86_64-pc-windows-gnullvm -p front -p runtime
            if ($LASTEXITCODE -ne 0) { throw "gnullvm engine build failed ($LASTEXITCODE)" }
        } finally {
            $env:PATH = $path
            Remove-Item Env:CC_x86_64_pc_windows_gnullvm -ErrorAction SilentlyContinue
        }
    }
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

# the gnullvm engine and the bundled llvm-mingw it links with: games build without Visual Studio or LLVM
$gnu = Join-Path $target 'x86_64-pc-windows-gnullvm\release'
if (-not $SkipGnu) {
    New-Item -ItemType Directory -Force (Join-Path $engine 'gnu') | Out-Null
    foreach ($lib in 'libnxfront.a', 'libnxrt.a') { Copy-Item (Join-Path $gnu $lib) (Join-Path $engine 'gnu') }
    # the windows crate's import library the gnullvm engine links: windows_x86_64_gnullvm's lib/ (Cargo.lock: 0.52.6)
    $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
    $winlib = Get-ChildItem (Join-Path $cargoHome 'registry\src\*\windows_x86_64_gnullvm-0.52.6\lib\libwindows.0.52.0.a') -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $winlib) { throw 'missing libwindows.0.52.0.a (windows_x86_64_gnullvm-0.52.6 in the cargo registry)' }
    Copy-Item $winlib.FullName (Join-Path $engine 'gnu')
    # x86_64 Windows only: no other targets, no Linux compiler-rt, no sanitizers, debugger, clangd or Python
    $skip = @('aarch64-w64-mingw32', 'armv7-w64-mingw32', 'i686-w64-mingw32', 'python')
    foreach ($d in Get-Item (Join-Path $llvm 'lib\clang\*\lib\linux') -ErrorAction SilentlyContinue) { $skip += $d.FullName }
    $skipFiles = 'lldb*', 'liblldb*', 'clangd*', 'clang-tidy*', 'libpython*', '*.pdb', 'libclang_rt.*san*', 'libclang_rt.fuzzer*',
        'libclang_rt.memprof*', 'libclang_rt.*-i386.*', 'libclang_rt.*-aarch64.*', 'libclang_rt.*-arm*.*'
    robocopy $llvm (Join-Path $dist 'toolchain\llvm-mingw') /MIR /XD @skip /XF @skipFiles /NFL /NDL /NJH /NJS /NP | Out-Null
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
$files = @('nxrs.exe', 'nxrs-build.exe', 'nxfront.lib', 'nxrt.lib')
if (-not $SkipGnu) { $files += 'gnu\libnxfront.a', 'gnu\libnxrt.a' }
foreach ($file in $files) {
    $built = if ($file -like 'gnu\*') { Join-Path $gnu (Split-Path $file -Leaf) } else { Join-Path $release $file }
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
