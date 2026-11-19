#!/bin/sh
# Compile generated chunks with clang (mkw-recomp's -O1 flag set) to show the C is valid. Objects go to build/cc.
# Usage: scripts/cc_check.sh <generated dir> <chunk.c>...
gen=$1; shift
CL=${CLANG:-"/c/Program Files/LLVM/bin/clang.exe"}
for f in "$@"; do
  o=build/cc/$(echo "$f" | tr '/' '_').o
  if "$CL" -std=gnu11 -O1 -fno-strict-aliasing -Wno-return-type -Wno-unused-label -march=x86-64-v3 \
       -fno-vectorize -fno-slp-vectorize -I"$gen/include" -c "$f" -o "$o" 2> "$o.log"; then echo "ok $f"
  else echo "FAIL $f"; fi
done
