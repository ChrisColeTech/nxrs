#!/bin/sh
# Compare nxrs's RomFS listing and file hashes with Turbo's (scripts/pkg/turbo_romfs.py output).
# Usage: scripts/pkg/compare.sh <out prefix> <nxrs romfs source args...>
pre=$1; shift
NXRS=${NXRS:-target/release/nxrs-translate}
"$NXRS" romfs ls -r "$@" | sed 's/\r$//' | sort -k2 > "$pre.rs.list"
sort -k2 "$pre.list" > "$pre.py.sorted"
if cmp -s "$pre.py.sorted" "$pre.rs.list"; then echo "listing: identical ($(wc -l < "$pre.rs.list") files)"; else echo "listing: DIFFERS"; diff "$pre.py.sorted" "$pre.rs.list" | head; fi
paths=$(awk '{print $3}' "$pre.sha")
"$NXRS" romfs sha256 "$@" $paths | sed 's/\r$//' > "$pre.rs.sha"
awk '{print $1"  "$2"  "$3}' "$pre.sha" > "$pre.py.sha3"
if cmp -s "$pre.py.sha3" "$pre.rs.sha"; then echo "hashes: $(wc -l < "$pre.rs.sha") identical ($(awk '$4==1' "$pre.sha" | wc -l) through the update patch)"; else echo "hashes: DIFFER"; diff "$pre.py.sha3" "$pre.rs.sha" | head; fi
