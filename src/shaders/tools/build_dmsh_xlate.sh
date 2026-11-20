#!/usr/bin/env bash
# Builds the shader translator tool for Linux (x86-64, or arm64 on an arm64 machine; DMSH_RID=linux-x64|linux-arm64
# overrides), build/tools/dmsh_xlate-linux/dmsh_xlate (a self-contained
# NativeAOT executable), from nxnative's source exactly as build_dmsh_xlate.ps1 does for Windows: its
# dm/gfx/tools/dmsh_xlate (Program.cs + csproj) and dm/third_party/ryujinx copied out of the nxnative checkout (not
# modified) and patched with dmsh_xlate.patch.
#
#   bash src/shaders/tools/build_dmsh_xlate.sh [nxnative checkout] [output folder]
#
# Needs the .NET 10 SDK (dotnet on PATH, or ~/.dotnet), clang and zlib's headers (NativeAOT's link on Linux), git.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
nxnative="${1:-/mnt/d/Projects/nxnative}"
out="${2:-$here/../../../build/tools/dmsh_xlate-linux}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
src="$(dirname "$out")/dmsh_xlate_src_linux"
rm -rf "$src"
mkdir -p "$src/dmsh_xlate"
cp -r "$nxnative/dm/third_party/ryujinx" "$src/ryujinx"
cp "$nxnative/dm/gfx/tools/dmsh_xlate/Program.cs" "$nxnative/dm/gfx/tools/dmsh_xlate/dmsh_xlate.csproj" "$src/dmsh_xlate/"
git -C "$nxnative" rev-parse HEAD > "$src/NXNATIVE_COMMIT"
# a checkout made on Windows has CRLF files and the patch LF lines: the files it changes are patched as LF and given
# their CRLF back, so the embedded source (`--source-sha`) is byte for byte the Windows build's
crlf=()
for f in $(grep '^+++ ' "$here/dmsh_xlate.patch" | awk '{print $2}' | sort -u); do
    if [ -f "$src/$f" ] && grep -q $'\r$' "$src/$f"; then
        crlf+=("$f")
        sed -i 's/\r$//' "$src/$f"
    fi
done
git -C "$src" apply -p0 "$here/dmsh_xlate.patch"
for f in "${crlf[@]}"; do
    sed -i 's/$/\r/' "$src/$f"
done
export PATH="$HOME/.dotnet:$PATH" DOTNET_CLI_TELEMETRY_OPTOUT=1
case "$(uname -m)" in aarch64 | arm64) rid=linux-arm64 ;; *) rid=linux-x64 ;; esac
rid="${DMSH_RID:-$rid}"
(cd "$src/dmsh_xlate" && dotnet publish dmsh_xlate.csproj -c Release -r "$rid" -p:RyuSrc=../ryujinx -o "$out")
"$out/dmsh_xlate" --source-sha
