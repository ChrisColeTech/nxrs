#!/usr/bin/env python3
"""Cross-check of the two Switch 2 shader translators on the same programs: dmsh_xlate (Ryujinx's translator, the
engine's primary, as nxshaders built it) against mkw-recomp's sm86 (SM86 -> GLSL -> glslc).

  uv run --no-project --python 3.14 crosscheck_sm86.py --mkw <mkw-recomp checkout> --spec <instructions.json>
      --sample <nxshaders export dir> --build <nxshaders prebuild dir> --work <dir> [--jobs 8]

For every sampled program both translations are checked with spirv-val, and where both are valid their interfaces are
compared: the stage's input and output locations, the constant banks, storage buffers, textures and images it reads,
and a compute shader's workgroup size. Prints a summary and writes <work>/crosscheck.tsv (one row per program).
"""
import argparse
import collections
import json
import os
import struct
import subprocess
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

VK = Path(os.environ.get("VULKAN_SDK", r"C:\VulkanSDK\1.3.268.0")) / "Bin"
GLSLC_STAGE = {"ps": "frag", "vs": "vert", "cs": "comp"}
_dec = None


def spirv_io(path):
    """{'in': {locations}, 'out': {locations}, 'local': [x, y, z] or None} of a SPIR-V module."""
    data = Path(path).read_bytes()
    w = struct.unpack(f"<{len(data) // 4}I", data)
    loc, var, local = {}, {}, None
    i = 5
    while i < len(w):
        n, op = w[i] >> 16, w[i] & 0xFFFF
        if op == 71 and w[i + 2] == 30:  # OpDecorate Location
            loc[w[i + 1]] = w[i + 3]
        elif op == 59:  # OpVariable
            var[w[i + 2]] = w[i + 3]
        elif op == 16 and w[i + 2] == 17:  # OpExecutionMode LocalSize
            local = list(w[i + 3:i + 6])
        i += n
    out = {"in": set(), "out": set(), "local": local}
    for v, sc in var.items():
        if v in loc and sc in (1, 3):
            out["in" if sc == 1 else "out"].add(loc[v])
    return out


def run(cmd):
    r = subprocess.run(cmd, capture_output=True, text=True)
    return r.returncode, (r.stderr or r.stdout).strip()


def init(mkw, spec):
    global _dec
    sys.path[:0] = [str(Path(mkw, "sm86/translate")), str(Path(mkw, "sm86/translate/frontend")), str(Path(mkw, "tools/shaders"))]
    from program import Decoder
    _dec = Decoder(Path(spec))


def mkw_translate(key, stage, ctl, code, work):
    """(verdict, spv path or None, reflection)"""
    import glsl
    from extract import constants, trim
    c1 = constants(ctl, code)
    sph, insns = _dec.program(trim(code, stage), stage)
    try:
        src, refl = glsl.translate(sph, insns, stage, glsl.compute_shape(ctl) if stage == "cs" else None, c1)
    except glsl.Unsupported as e:
        return f"unsupported: {e}"[:120], None, {}
    except Exception as e:
        return f"error: {type(e).__name__} {e}"[:120], None, {}
    gl, spv = Path(work, f"{key}.glsl"), Path(work, f"{key}.mkw.spv")
    gl.write_text(src)
    rc, err = run([str(VK / "glslc.exe"), "--target-env=vulkan1.3", f"-fshader-stage={GLSLC_STAGE[stage]}", "-O0", str(gl), "-o", str(spv)])
    if rc:
        line = next((ln for ln in err.splitlines() if "error" in ln), err[:120])
        return f"glslc: {line.split(': ', 2)[-1][:100]}", None, refl
    rc, err = run([str(VK / "spirv-val.exe"), "--target-env", "vulkan1.3", str(spv)])
    if rc:
        return f"spirv-val: {err.splitlines()[0][:100] if err else rc}", spv, refl
    return "ok", spv, refl


def resources(refl, ours):
    """Comparable resource sets: constant bank slots, storage buffers (bank, word offset), texture and image handles"""
    def tex(lst):
        return {(t.get("cbuf_slot", -1) if t.get("cbuf_slot", -1) not in (-1, 2) else 2, t.get("handle")) for t in lst}
    if ours:
        cb = {b["resource"]["slot"] for b in refl["bindings"] if b["resource"].get("kind") == "cbuf"}
        sb = {(b["resource"].get("cb_slot"), b["resource"].get("cb_offset")) for b in refl["bindings"] if b["resource"].get("kind") == "sbuf"}
        tx = tex([b["resource"] for b in refl["bindings"] if b["resource"].get("kind") == "texture"])
        im = tex([b["resource"] for b in refl["bindings"] if b["resource"].get("kind") == "image"])
    else:
        cb = {c["slot"] for c in refl.get("cbufs", [])}
        sb = {(s.get("cb_slot"), s.get("cb_offset")) for s in refl.get("sbufs", [])}
        tx, im = tex(refl.get("textures", [])), tex(refl.get("images", []))
    return {"cbufs": cb, "sbufs": sb, "textures": tx, "images": im}


def one(job):
    key, stage, sample, build, work = job
    ctl, code = Path(sample, f"{key}.ctl").read_bytes(), Path(sample, f"{key}.code").read_bytes()
    row = {"key": key, "stage": stage}
    ours = json.loads(Path(build, "spv", f"{key}.json").read_text())
    row["dmsh"] = ours.get("status") if ours.get("status") != "failed" else "failed: " + ours.get("why", "")[:100]
    v, spv, refl = mkw_translate(key, stage, ctl, code, work)
    row["mkw"] = v
    if v == "ok" and ours.get("status") == "ok":
        a, b = spirv_io(Path(build, "spv", f"{key}.spv")), spirv_io(spv)
        diff = []
        for k in ("in", "out"):
            if a[k] != b[k]:
                diff.append(f"{k} {sorted(a[k])} vs {sorted(b[k])}")
        if stage == "cs" and a["local"] != b["local"]:
            diff.append(f"local {a['local']} vs {b['local']}")
        ra, rb = resources(ours, True), resources(refl, False)
        for k in ("cbufs", "sbufs", "textures", "images"):
            # c0 is read by both for storage buffer addresses; compare the game's banks only
            x, y = (ra[k] - {0}, rb[k] - {0}) if k == "cbufs" else (ra[k], rb[k])
            if x != y:
                diff.append(f"{k} {sorted(x, key=str)[:6]} vs {sorted(y, key=str)[:6]}")
        row["agree"] = "yes" if not diff else "no"
        row["diff"] = "; ".join(diff)
    return row


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mkw", required=True)
    ap.add_argument("--spec", required=True)
    ap.add_argument("--sample", required=True)
    ap.add_argument("--build", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--jobs", type=int, default=8)
    a = ap.parse_args()
    os.makedirs(a.work, exist_ok=True)
    jobs = []
    for ln in Path(a.sample, "index.tsv").read_text().splitlines()[1:]:
        key, stage, _ = ln.split("\t", 2)
        if stage in GLSLC_STAGE:
            jobs.append((key, stage, a.sample, a.build, a.work))
    with ProcessPoolExecutor(a.jobs, initializer=init, initargs=(a.mkw, a.spec)) as ex:
        rows = list(ex.map(one, jobs, chunksize=4))
    with open(Path(a.work, "crosscheck.tsv"), "w") as f:
        f.write("key\tstage\tdmsh_xlate\tmkw_sm86\tinterface_agrees\tdifferences\n")
        for r in rows:
            f.write(f"{r['key']}\t{r['stage']}\t{r['dmsh']}\t{r['mkw']}\t{r.get('agree', '-')}\t{r.get('diff', '')}\n")
    n = len(rows)
    ok_d = sum(r["dmsh"] == "ok" for r in rows)
    ok_m = sum(r["mkw"] == "ok" for r in rows)
    both = [r for r in rows if r["dmsh"] == "ok" and r["mkw"] == "ok"]
    agree = sum(r.get("agree") == "yes" for r in both)
    print(f"programs {n} (by stage {dict(collections.Counter(r['stage'] for r in rows))})")
    print(f"dmsh_xlate valid {ok_d}/{n}; mkw sm86 valid {ok_m}/{n}; both valid {len(both)}; interfaces agree {agree}/{len(both)}")
    print("mkw verdicts:", collections.Counter(r["mkw"].split(":")[0] for r in rows).most_common())
    print("dmsh verdicts:", collections.Counter(r["dmsh"].split(":")[0] for r in rows).most_common())
    kinds = collections.Counter(d.split(" ")[0] for r in both for d in r.get("diff", "").split("; ") if d)
    print("interface differences by kind:", kinds.most_common())
    only_m = [r for r in rows if r["mkw"] == "ok" and r["dmsh"] != "ok"]
    only_d = [r for r in rows if r["dmsh"] == "ok" and r["mkw"] != "ok"]
    print(f"valid only with mkw sm86: {len(only_m)}; valid only with dmsh_xlate: {len(only_d)}")
    for r in only_m[:5]:
        print("  mkw-only", r["key"], r["stage"], r["dmsh"])
    for r in only_d[:8]:
        print("  dmsh-only", r["key"], r["stage"], r["mkw"])


if __name__ == "__main__":
    main()
