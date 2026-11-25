# Builds the shader translator tool, dmsh_xlate.exe (a self-contained NativeAOT executable), from nxnative's source:
# its dm/gfx/tools/dmsh_xlate (Program.cs + csproj) and dm/third_party/ryujinx (Ryujinx's shader translator with
# nxnative's changes), copied out of the nxnative checkout (which is not modified) and patched with dmsh_xlate.patch
# (the Vulkan target: no Metal-only bounds flags, and each stage's resources in a descriptor set of its own).
#
#   pwsh src/shaders/tools/build_dmsh_xlate.ps1 [-Nxnative build\Q\nxnative-up] [-Out <candidate folder>]
#
# Needs the .NET 10 SDK and the MSVC linker (Visual Studio Build Tools); vswhere.exe must be reachable.
param(
    [string]$Nxnative = (Join-Path $PSScriptRoot "..\..\..\build\Q\nxnative-up"),
    [string]$Out = (Join-Path $PSScriptRoot "..\..\..\build\tools\dmsh_xlate"),
    [string]$Work = (Join-Path $PSScriptRoot "..\..\..\build\DELEGATION\R1A\dmsh_xlate_src"),
    # the release workflow passes CI's pin (NXNATIVE_XLATE, checked out with core.autocrlf true), which gives the same
    # --source-sha as the shipped tool
    [string]$Commit = "7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f"
)
$ErrorActionPreference = "Stop"
$Nxnative = [System.IO.Path]::GetFullPath($Nxnative)
$Out = [System.IO.Path]::GetFullPath($Out)
$Work = [System.IO.Path]::GetFullPath($Work)
$requiredCommit = $Commit
$trackedSource = @("dm/third_party/ryujinx", "dm/gfx/tools/dmsh_xlate")
if (-not (Test-Path -LiteralPath (Join-Path $Nxnative ".git"))) {
    throw "Pinned nxnative checkout is absent: $Nxnative"
}
$actualCommit = (& git -C $Nxnative rev-parse HEAD).Trim()
if ($LASTEXITCODE -or $actualCommit -ne $requiredCommit) {
    throw "Pinned nxnative checkout must be at $requiredCommit; found '$actualCommit' in $Nxnative"
}
$dirty = & git -C $Nxnative status --porcelain -- @trackedSource
if ($LASTEXITCODE) { throw "Could not verify pinned nxnative source state: $Nxnative" }
if ($dirty) {
    throw "Pinned nxnative translator source is dirty; clean these paths before building:`n$($dirty -join "`n")"
}
$src = $Work
if (Test-Path $src) { Remove-Item -Recurse -Force $src }
New-Item -ItemType Directory -Force (Join-Path $src "dmsh_xlate") | Out-Null
Copy-Item -Recurse (Join-Path $Nxnative "dm\third_party\ryujinx") (Join-Path $src "ryujinx")
Copy-Item (Join-Path $Nxnative "dm\gfx\tools\dmsh_xlate\Program.cs"), (Join-Path $Nxnative "dm\gfx\tools\dmsh_xlate\dmsh_xlate.csproj") (Join-Path $src "dmsh_xlate")
$actualCommit | Set-Content (Join-Path $src "NXNATIVE_COMMIT")
git -C $src apply -p0 (Join-Path $PSScriptRoot "dmsh_xlate.patch")
if ($LASTEXITCODE) { throw "dmsh_xlate.patch does not apply to this nxnative checkout" }
$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\Installer;" + $env:PATH
Push-Location (Join-Path $src "dmsh_xlate")
try {
    dotnet publish dmsh_xlate.csproj -c Release -r win-x64 -p:RyuSrc=..\ryujinx -o $Out
    if ($LASTEXITCODE) { throw "dotnet publish failed" }
} finally { Pop-Location }
& (Join-Path $Out "dmsh_xlate.exe") --source-sha
