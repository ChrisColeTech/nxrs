"""Like dump_py.py, but over N random words forced into each SimdC route (dp / ldst / crc), to exercise the
Unsupported paths and rare encodings that real code never contains. Deterministic (seeded).
  uv run python D:/Projects/RecompLauncherEngine/scripts/simd/dump_fuzz.py <N> <out.tsv>
"""
import random
import sys

sys.path.insert(0, r'D:\Projects\mkw-recomp-main\recomp')
from lifter import Unsupported  # noqa: E402
from simd_c import SimdC  # noqa: E402

n = int(sys.argv[1])
rng = random.Random(1234)
sc = SimdC()
with open(sys.argv[2], 'w', newline='\n') as f:
    for i in range(n):
        r = rng.getrandbits(32)
        k = ('dp', 'ldst', 'crc')[i % 3]
        if k == 'dp':
            w = r | (0b0111 << 25)
        elif k == 'ldst':
            w = (r & ~(0b0101 << 25)) | (0b0100 << 25) | (1 << 26)
        else:
            w = (r & 0x80FFFFFF & ~(0xF << 21) & ~(0x3F << 10)) | (0b0011010110 << 21) | ((16 + (r >> 24) % 8) << 10)
        pc = 0x7000000000 + rng.getrandbits(24) * 4
        try:
            out = sc.dp(w, pc) if k == 'dp' else sc.ldst(w, pc) if k == 'ldst' else sc.crc(w)
        except Unsupported as e:
            out = 'UNSUP:' + str(e)
        except Exception as e:  # noqa: BLE001
            out = 'ERR:' + type(e).__name__ + ':' + str(e)
        f.write(f'{k}\t{w:08x}\t{pc:x}\t{out}\n')
