"""Dump mkw-recomp simd_c.py output for every distinct FP/SIMD/CRC word in an NSO's .text.

Differential reference for the Rust port (nx-xlate/src/simd.rs). Run from mkw-recomp-main:
  uv run python D:/Projects/RecompLauncherEngine/scripts/simd/dump_py.py <nso> <out.tsv>
Output lines: kind<TAB>word_hex<TAB>pc_hex<TAB>output | UNSUP:msg | ERR:exc
"""
import struct
import sys

sys.path.insert(0, r'D:\Projects\mkw-recomp-main\recomp')
from lifter import Unsupported, bits  # noqa: E402
from nso import Nso  # noqa: E402
from simd_c import SimdC  # noqa: E402


def route(w):
    op0 = bits(w, 25, 4)
    if (op0 & 0b0111) == 0b0111:
        return 'dp'
    if (op0 & 0b0101) == 0b0100 and bits(w, 26, 1):
        return 'ldst'
    if ((op0 & 0b0111) == 0b0101 and bits(w, 28, 1) and bits(w, 21, 4) == 6 and not bits(w, 30, 1)
            and 16 <= bits(w, 10, 6) <= 23):
        return 'crc'
    return None


def main():
    nso = Nso(sys.argv[1])
    off, size = nso.text
    sc = SimdC()
    seen = set()
    with open(sys.argv[2], 'w', newline='\n') as f:
        for o in range(off, off + size, 4):
            w = struct.unpack_from('<I', nso.image, o)[0]
            if w in seen:
                continue
            k = route(w)
            if k is None:
                continue
            seen.add(w)
            pc = 0x7000000000 + o
            try:
                out = sc.dp(w, pc) if k == 'dp' else sc.ldst(w, pc) if k == 'ldst' else sc.crc(w)
            except Unsupported as e:
                out = 'UNSUP:' + str(e)
            except Exception as e:  # noqa: BLE001
                out = 'ERR:' + type(e).__name__ + ':' + str(e)
            f.write(f'{k}\t{w:08x}\t{pc:x}\t{out}\n')
    print(len(seen), 'words')


main()
