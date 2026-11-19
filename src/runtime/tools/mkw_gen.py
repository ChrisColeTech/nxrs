"""Runs mkw-recomp's generator (recomp/gen.py, read-only) with one addition: every function that holds a C++
exception landing pad is made a resumable "owner", like a setjmp caller.

This is a prototype of the change the Rust translator needs for C++ exceptions (README, "Requests to the translator"):
an unwinder's resume jumps to a landing pad in the middle of a lifted function, which only an owner can take. Owners
are found from .eh_frame: each FDE with an LSDA, each call-site entry with a landing pad; the owner is the lifted
function containing the pad.

Usage: python mkw_gen.py --exefs <dir> --out <dir> [--jobs N]   (needs numpy; MKW_RECOMP names the recomp/ folder)
gen.py's workers are spawned processes that import this file again, so the patch below applies in them too.
"""
import os
import struct
import sys

import numpy as np

sys.path.insert(0, os.environ.get("MKW_RECOMP", r"D:\Projects\mkw-recomp-main\recomp"))
import gen  # noqa: E402


def _uleb(b, o):
    v = s = 0
    while True:
        x = b[o]
        o += 1
        v |= (x & 0x7F) << s
        s += 7
        if not x & 0x80:
            return v, o


def _sleb(b, o):
    v = s = 0
    while True:
        x = b[o]
        o += 1
        v |= (x & 0x7F) << s
        s += 7
        if not x & 0x80:
            if x & 0x40:
                v -= 1 << s
            return v, o


def _enc(b, o, enc):
    """(value, next offset) of a DW_EH_PE-encoded value at image offset o; pc-relative values are image offsets."""
    fmt, app = enc & 0x0F, enc & 0x70
    if fmt == 0x01:
        v, n = _uleb(b, o)
    elif fmt == 0x09:
        v, n = _sleb(b, o)
    else:
        size, sign = {0x00: (8, 0), 0x02: (2, 0), 0x03: (4, 0), 0x04: (8, 0), 0x0A: (2, 1), 0x0B: (4, 1), 0x0C: (8, 1)}[fmt]
        v = int.from_bytes(b[o:o + size], "little", signed=bool(sign))
        n = o + size
    if app == 0x10 and v:
        v += o
    return v, n


def landing_pads(img, eh_hdr):
    """Image offsets of every landing pad named by the LSDAs of the FDEs in .eh_frame_hdr's table."""
    if not eh_hdr or img[eh_hdr] != 1:
        return []
    o = eh_hdr + 4
    _, o = _enc(img, o, img[eh_hdr + 1])
    count, o = _enc(img, o, img[eh_hdr + 2])
    pads = []
    for i in range(count):
        fde = eh_hdr + struct.unpack_from("<i", img, o + 8 * i + 4)[0]
        p = fde + 4
        cie = p - struct.unpack_from("<I", img, p)[0]
        q = cie + 9
        aug = img[q:img.index(0, q)].decode()
        q += len(aug) + 1
        _, q = _uleb(img, q)
        _, q = _sleb(img, q)
        q += 1  # return address register (version 1)
        fde_enc, lsda_enc = 0, 0xFF
        if aug.startswith("z"):
            _, q = _uleb(img, q)
            for ch in aug[1:]:
                if ch == "R":
                    fde_enc = img[q]
                    q += 1
                elif ch == "L":
                    lsda_enc = img[q]
                    q += 1
                elif ch == "P":
                    pe = img[q]
                    _, q = _enc(img, q + 1, pe & 0x7F)
        start, p = _enc(img, p + 4, fde_enc)
        _, p = _enc(img, p, fde_enc & 0x0F)
        if lsda_enc == 0xFF or not aug.startswith("z"):
            continue
        alen, p = _uleb(img, p)
        if not alen:
            continue
        lsda, _ = _enc(img, p, lsda_enc)
        if not lsda:
            continue
        # LSDA header
        lp_enc = img[lsda]
        q = lsda + 1
        lpstart = start
        if lp_enc != 0xFF:
            lpstart, q = _enc(img, q, lp_enc)
        tt_enc = img[q]
        q += 1
        if tt_enc != 0xFF:
            _, q = _uleb(img, q)
        cs_enc = img[q]
        cs_len, q = _uleb(img, q + 1)
        end = q + cs_len
        while q < end:
            _, q = _enc(img, q, cs_enc & 0x0F)
            _, q = _enc(img, q, cs_enc & 0x0F)
            lp, q = _enc(img, q, cs_enc & 0x0F)
            _, q = _uleb(img, q)
            if lp:
                pads.append(lpstart + lp)
    return pads


_setjmp_owners = gen.setjmp_owners


def owners(mod, starts):
    found = _setjmp_owners(mod, starts)
    pads = [mod.base + p for p in landing_pads(mod.image, mod.nso.eh_start)]
    if pads:
        k = np.searchsorted(starts, np.array(pads, dtype=np.int64), side="right") - 1
        found |= {int(starts[i]) for i in k if i >= 0}
    return found


gen.setjmp_owners = owners

if __name__ == "__main__":
    # Homebrew is no MKW version: an empty patch table spares gen.py its version detection.
    if "--patches" not in sys.argv and "--out" in sys.argv:
        out = sys.argv[sys.argv.index("--out") + 1]
        os.makedirs(out, exist_ok=True)
        empty = os.path.join(out, "no-patches.txt")
        open(empty, "w").close()
        sys.argv += ["--patches", empty]
    gen.main()
