# Builds the shader translator tool, dmsh_xlate.exe (a self-contained NativeAOT executable), from nxnative's source:
# its dm/gfx/tools/dmsh_xlate (Program.cs + csproj) and dm/third_party/ryujinx (Ryujinx's shader translator with
# nxnative's changes), copied out of the nxnative checkout (which is not modified) and patched with dmsh_xlate.patch
# (the Vulkan target: no Metal-only bounds flags, and each stage's resources in a descriptor set of its own).
#
#   pwsh src/shaders/tools/build_dmsh_xlate.ps1 [-Nxnative D:\Projects\nxnative] [-Out build\tools\dmsh_xlate]
#
# Needs the .NET 10 SDK and the MSVC linker (Visual Studio Build Tools); vswhere.exe must be reachable.
param(
    [string]$Nxnative = "D:\Projects\nxnative",
    [string]$Out = (Join-Path $PSScriptRoot "..\..\..\build\tools\dmsh_xlate")
)
$ErrorActionPreference = "Stop"
$src = Join-Path (Split-Path $Out) "dmsh_xlate_src"
if (Test-Path $src) { Remove-Item -Recurse -Force $src }
New-Item -ItemType Directory -Force (Join-Path $src "dmsh_xlate") | Out-Null
Copy-Item -Recurse (Join-Path $Nxnative "dm\third_party\ryujinx") (Join-Path $src "ryujinx")
Copy-Item (Join-Path $Nxnative "dm\gfx\tools\dmsh_xlate\Program.cs"), (Join-Path $Nxnative "dm\gfx\tools\dmsh_xlate\dmsh_xlate.csproj") (Join-Path $src "dmsh_xlate")
git -C $Nxnative rev-parse HEAD | Set-Content (Join-Path $src "NXNATIVE_COMMIT")
git -C $src apply -p0 (Join-Path $PSScriptRoot "dmsh_xlate.patch")
if ($LASTEXITCODE) { throw "dmsh_xlate.patch does not apply to this nxnative checkout" }
$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\Installer;" + $env:PATH
Push-Location (Join-Path $src "dmsh_xlate")
try {
    dotnet publish dmsh_xlate.csproj -c Release -r win-x64 -p:RyuSrc=..\ryujinx -o $Out
    if ($LASTEXITCODE) { throw "dotnet publish failed" }
} finally { Pop-Location }
& (Join-Path $Out "dmsh_xlate.exe") --source-sha
