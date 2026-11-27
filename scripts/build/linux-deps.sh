#!/usr/bin/env bash
# linux-deps.sh: what the Linux release bundles so that it builds games with nothing installed but a Vulkan driver
# (docs/waves/RELEASE-SELFCONTAINED.md). Run by .github/workflows/release.yml; runs the same on any x86-64 Debian or
# Ubuntu machine.
#
#   linux-deps.sh codecs <prefix>     static, position-independent opus, libvpx (VP8/VP9 decoders), dav1d and FFmpeg
#                                     (H.264 and AAC decoders) into <prefix>. Build the engine with NXRS_CODECS=<prefix>:
#                                     src/os/build.rs links them into libnxfront.a and nxrs-build links games with
#                                     -lasound alone.
#   linux-deps.sh toolchain <dir>     LLVM's clang and lld, the libraries they load, and a sysroot (glibc's headers and
#                                     link files, GCC's start files and libgcc, ALSA's libasound.so.2 to link against),
#                                     into <dir> (the release's nxrs/toolchain/clang, where nxrs-build looks).
#   linux-deps.sh check <nxrs>        with nothing but the unpacked release <nxrs>: compile and run a program, and link
#                                     the engine's entry with everything it pulls (what every game link does). Run it in
#                                     a clean container (release.yml: ubuntu:24.04, no compiler, no -dev packages).
#
# Build-time needs (codecs, toolchain): curl, git, xz-utils, zstd, nasm, meson, ninja-build, make, gcc, and the build
# machine's libc6-dev, linux-libc-dev, libgcc-*-dev and libasound2-dev (the sysroot is copied from them, so games link
# against the glibc the launcher and the engine were built with).
set -euo pipefail

OPUS=1.5.2      # the versions of src/os/third_party (Windows): libopus 1.5.2, libvpx 1.14.0
VPX=v1.14.0
DAV1D=1.5.1
FFMPEG=7.1.1
LLVM=23.1.5     # the LLVM of llvm-mingw 20261104, the Windows release's compiler
MA=x86_64-linux-gnu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
JOBS=$(nproc)

codecs() {
    local prefix work
    prefix=$(realpath -m "$1")
    work=$(mktemp -d)
    mkdir -p "$prefix"
    cd "$work"
    curl -fsSL "https://downloads.xiph.org/releases/opus/opus-$OPUS.tar.gz" | tar -xz
    (cd "opus-$OPUS" && ./configure --prefix="$prefix" --libdir="$prefix/lib" --enable-static --disable-shared \
        --with-pic --disable-doc --disable-extra-programs && make -j"$JOBS" && make install)
    git clone -q --depth 1 --branch "$VPX" https://chromium.googlesource.com/webm/libvpx
    (cd libvpx && ./configure --prefix="$prefix" --libdir="$prefix/lib" --enable-static --disable-shared --enable-pic \
        --disable-vp8-encoder --disable-vp9-encoder --disable-examples --disable-tools --disable-docs \
        --disable-unit-tests && make -j"$JOBS" && make install)
    curl -fsSL "https://code.videolan.org/videolan/dav1d/-/archive/$DAV1D/dav1d-$DAV1D.tar.gz" | tar -xz
    (cd "dav1d-$DAV1D" && meson setup build --prefix="$prefix" --libdir=lib --buildtype=release \
        --default-library=static -Db_staticpic=true -Denable_tools=false -Denable_tests=false \
        && ninja -C build install)
    curl -fsSL "https://ffmpeg.org/releases/ffmpeg-$FFMPEG.tar.xz" | tar -xJ
    (cd "ffmpeg-$FFMPEG" && ./configure --prefix="$prefix" --libdir="$prefix/lib" --enable-static --disable-shared \
        --enable-pic --disable-programs --disable-doc --disable-autodetect --disable-everything --disable-avdevice \
        --disable-avformat --disable-swscale --disable-swresample --disable-avfilter --disable-network \
        --enable-decoder=h264,aac --enable-parser=h264,aac && make -j"$JOBS" && make install)
    cd /
    rm -rf "$work"
    for name in opus vpx dav1d avcodec avutil; do
        test -s "$prefix/lib/lib$name.a" || { echo "missing $prefix/lib/lib$name.a"; exit 1; }
    done
    ls -l "$prefix"/lib/*.a
}

toolchain() {
    local dir work name major sys gcc f
    dir=$(realpath -m "$1")
    work=$(mktemp -d)
    name=LLVM-$LLVM-Linux-X64
    major=${LLVM%%.*}
    # the official release, only clang, lld and clang's own headers (the rest is 4 GB unpacked)
    curl -fsSL "https://github.com/llvm/llvm-project/releases/download/llvmorg-$LLVM/$name.tar.zst" -o "$work/llvm.tar.zst"
    zstd -dc --long=31 "$work/llvm.tar.zst" | tar -x -C "$work" "$name/bin/clang-$major" "$name/bin/lld" \
        "$name/lib/clang/$major/include"
    rm -f "$work/llvm.tar.zst"
    rm -rf "$dir"
    mkdir -p "$dir/bin" "$dir/lib/clang/$major" "$dir/legal"
    cp "$work/$name/bin/clang-$major" "$work/$name/bin/lld" "$dir/bin/"
    ln -s "clang-$major" "$dir/bin/clang"
    ln -s lld "$dir/bin/ld.lld"
    cp -r "$work/$name/lib/clang/$major/include" "$dir/lib/clang/$major/"
    rm -rf "$work"
    # what clang and lld load beyond glibc (their RUNPATH is $ORIGIN/../lib)
    for f in libstdc++.so.6 libgcc_s.so.1 libz.so.1; do cp -L "/usr/lib/$MA/$f" "$dir/lib/"; done

    # the sysroot: the build machine's glibc (headers, start files, link libraries), GCC's start files and libgcc,
    # ALSA's library to link against (the player's own libasound.so.2 runs)
    sys=$dir/sysroot
    mkdir -p "$sys/usr/include" "$sys/usr/lib/$MA" "$sys/usr/lib64"
    ln -s usr/lib "$sys/lib"
    ln -s usr/lib64 "$sys/lib64"
    for p in libc6-dev linux-libc-dev; do
        dpkg -L "$p" | grep '^/usr/include/' | while read -r f; do
            if [ -f "$f" ]; then install -D -m644 "$f" "$sys$f"; fi
        done
    done
    # libc.so and libm.so are linker scripts naming /lib/$MA/... and /lib64/ld-linux-x86-64.so.2: lld reads those
    # inside the sysroot, through the lib and lib64 links above
    for p in libc6-dev libc6; do
        dpkg -L "$p" | grep -E "/lib(64)?/($MA/)?[^/]+\.(o|a|so)(\.[0-9]+)*$" | while read -r f; do
            if [ -e "$f" ]; then cp -L "$f" "$sys/usr/lib/$MA/$(basename "$f")"; fi
        done
    done
    cp -L "/usr/lib64/ld-linux-x86-64.so.2" "$sys/usr/lib64/" 2>/dev/null \
        || cp -L "/lib64/ld-linux-x86-64.so.2" "$sys/usr/lib64/"
    gcc=$(dirname "$(gcc -print-libgcc-file-name)")   # /usr/lib/gcc/x86_64-linux-gnu/<version>: clang finds it there
    mkdir -p "$sys$gcc"
    cp -L "$gcc"/crt*.o "$gcc/libgcc.a" "$gcc/libgcc_eh.a" "$gcc/libgcc_s.so" "$sys$gcc/"
    cp -L "/usr/lib/$MA/libgcc_s.so.1" "/usr/lib/$MA/libasound.so.2" "$sys/usr/lib/$MA/"
    ln -sf libasound.so.2 "$sys/usr/lib/$MA/libasound.so"

    # clang reads <triple>.cfg (cc.rs passes --target) or clang.cfg beside itself: the sysroot above
    printf '%s\n' '# nxrs: the bundled sysroot, so games build with nothing installed (scripts/build/linux-deps.sh)' \
        '--sysroot=<CFGDIR>/../sysroot' > "$dir/bin/x86_64-unknown-linux-gnu.cfg"
    cp "$dir/bin/x86_64-unknown-linux-gnu.cfg" "$dir/bin/clang.cfg"

    # licenses: LLVM (Apache-2.0 with LLVM exceptions), and the Debian copyright files of what the sysroot and lib/ hold
    cp "$ROOT/LICENSES/llvm-mingw.txt" "$dir/legal/LLVM.txt"
    for p in libc6 libc6-dev linux-libc-dev libgcc-s1 libstdc++6 zlib1g libasound2t64 libasound2; do
        if [ -f "/usr/share/doc/$p/copyright" ]; then cp "/usr/share/doc/$p/copyright" "$dir/legal/$p.txt"; fi
    done
    "$dir/bin/clang" --version
    du -sh "$dir"
}

check() {
    local nxrs work cc
    nxrs=$(realpath "$1")
    work=$(mktemp -d)
    cc=("$nxrs/toolchain/clang/bin/clang" --target=x86_64-unknown-linux-gnu)
    "${cc[@]}" --version
    # a program like a game's loader (cc.rs STUB_C_LINUX's headers and libraries)
    cat > "$work/t.c" <<'EOF'
#define _GNU_SOURCE
#include <dlfcn.h>
#include <limits.h>
#include <math.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
int main(int argc, char **argv) { printf("bundled clang ok: %.3f %d\n", sqrt(2.0 + argc), (int)strlen(argv[0]) > 0); return 0; }
EOF
    "${cc[@]}" -O2 -fuse-ld=lld -o "$work/t" "$work/t.c" -ldl -lm
    "$work/t"
    # a game's code: the engine's entry and everything it pulls in, linked as cc.rs link_so does. What the generated
    # code itself defines is reported as undefined here; nothing of the codecs, ALSA or the C library may be.
    echo 'int nxrs_check(void) { return 0; }' > "$work/g.c"
    "${cc[@]}" -O1 -fPIC -c -o "$work/g.o" "$work/g.c"
    "${cc[@]}" -shared -fuse-ld=lld -o "$work/g.so" "$work/g.o" "$nxrs/nxrs-engine/libnxfront.a" -Wl,-u,nxfront_main \
        -Wl,-Bsymbolic -Wl,--no-undefined -Wl,--warn-unresolved-symbols -Wl,-z,noexecstack -lasound -lgcc_s -lutil \
        -lrt -lpthread -lm -ldl -lc 2> "$work/link.txt" || { cat "$work/link.txt"; exit 1; }
    grep -o 'undefined symbol: [^ ]*' "$work/link.txt" | sort -u > "$work/undefined.txt" || true
    echo "undefined after the engine link (the generated code defines these): $(wc -l < "$work/undefined.txt")"
    head -20 "$work/undefined.txt"
    if grep -E 'undefined symbol: (opus_|vpx_|dav1d_|av_|avcodec_|snd_|pthread_|dl|__libc|_Unwind_)' "$work/undefined.txt"; then
        echo 'the release does not link on its own'
        exit 1
    fi
    ls -l "$work/g.so"
    rm -rf "$work"
}

case "${1:-}" in
    codecs) codecs "${2:?prefix}" ;;
    toolchain) toolchain "${2:?dir}" ;;
    check) check "${2:?nxrs folder}" ;;
    *) sed -n '2,20p' "$0"; exit 2 ;;
esac
