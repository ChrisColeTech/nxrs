#!/usr/bin/env bash
# Builds the shader translator tool for Linux (x86-64, or arm64 on an arm64 machine; DMSH_RID=linux-x64|linux-arm64
# overrides), build/tools/dmsh_xlate-linux/dmsh_xlate (a self-contained
# NativeAOT executable), from nxnative's source exactly as build_dmsh_xlate.ps1 does for Windows: its
# dm/gfx/tools/dmsh_xlate (Program.cs + csproj) and dm/third_party/ryujinx copied out of the nxnative checkout (not
# modified) and patched with dmsh_xlate.patch.
#
#   bash src/shaders/tools/build_dmsh_xlate.sh [nxnative checkout] [output folder] [disposable source folder]
#
# Needs the .NET 10 SDK (dotnet on PATH, or ~/.dotnet), clang and zlib's headers (NativeAOT's link on Linux), git.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
nxnative="${1:-/mnt/d/Projects/RecompLauncherEngine/build/Q/nxnative-up}"
out="${2:-$here/../../../build/tools/dmsh_xlate-linux}"
src="${3:-$here/../../../build/DELEGATION/R1A/dmsh_xlate_src_linux}"
# CI and the release pass their pin (NXNATIVE_XLATE, checked out with core.autocrlf true: the shipped --source-sha)
required_commit="${DMSH_COMMIT:-7ebb76eda8ec1c532a4fc52170f2c5e13b18e80f}"
tracked_source=(dm/third_party/ryujinx dm/gfx/tools/dmsh_xlate)
if [ ! -e "$nxnative/.git" ]; then
    echo "Pinned nxnative checkout is absent: $nxnative" >&2
    exit 1
fi
actual_commit="$(git -C "$nxnative" rev-parse HEAD)"
if [ "$actual_commit" != "$required_commit" ]; then
    echo "Pinned nxnative checkout must be at $required_commit; found '$actual_commit' in $nxnative" >&2
    exit 1
fi
dirty="$(git -C "$nxnative" status --porcelain -- "${tracked_source[@]}")"
if [ -n "$dirty" ]; then
    printf 'Pinned nxnative translator source is dirty; clean these paths before building:\n%s\n' "$dirty" >&2
    exit 1
fi
mkdir -p "$out"
out="$(cd "$out" && pwd)"
rm -rf "$src"
mkdir -p "$src/dmsh_xlate"
cp -r "$nxnative/dm/third_party/ryujinx" "$src/ryujinx"
cp "$nxnative/dm/gfx/tools/dmsh_xlate/Program.cs" "$nxnative/dm/gfx/tools/dmsh_xlate/dmsh_xlate.csproj" "$src/dmsh_xlate/"
printf '%s\n' "$actual_commit" > "$src/NXNATIVE_COMMIT"
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
