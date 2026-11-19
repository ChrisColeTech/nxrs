"""Imports nxnative's per-game shader tables as data (developer side; the engine never runs Python).

    uv run --no-project --python 3.14 src/profiles/tools/export_shaders.py <nxnative checkout> [--check]

Writes games/<id>/shaders.tsv.zst under src/profiles/ for every profile that has any of these tables, each file
byte for byte after a "## <file name>" line, the whole zstd-compressed (level 19):

  pso_manifest.tsv         the prewarm seed: S <key> <stage> (shaders a play-through used, in order), R <pipeline
                           key> (nxnative's Metal pipeline key), C <key> (compute shaders)
  pso_pairs.tsv            vertex / pixel pairs of agl program variations outside shader_pairs.tsv
  shader_pairs.tsv         vertex / pixel pairs within BNSH variations
  shader_facts.tsv         answers to the shader translator's queries (key, fact JSON)
  shader_facts_manual.tsv  hand-written answers, when the profile has them

shader_map.tsv (549 MB: where each key was found, evidence) and the run captures are not imported. --check compares
the decompressed contents with nxnative without writing.
"""
import compression.zstd as zstd
import sys
from pathlib import Path

TABLES = ["pso_manifest.tsv", "pso_pairs.tsv", "shader_pairs.tsv", "shader_facts.tsv", "shader_facts_manual.tsv"]


def bundle(game: Path) -> bytes | None:
    out = bytearray()
    for name in TABLES:
        p = game / name
        if p.is_file():
            data = p.read_bytes()
            out += b"## " + name.encode() + b"\n" + data
            if data and not data.endswith(b"\n"):
                out += b"\n"
    return bytes(out) if out else None


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    src = Path(sys.argv[1]) / "dm" / "games"
    dst = Path(__file__).resolve().parent.parent / "games"
    check = "--check" in sys.argv
    written = differ = 0
    total = 0
    for game in sorted(p for p in src.iterdir() if (p / "game.tsv").is_file()):
        b = bundle(game)
        out = dst / game.name / "shaders.tsv.zst"
        if b is None:
            continue
        if check:
            have = zstd.decompress(out.read_bytes()) if out.is_file() else None
            if have != b:
                differ += 1
                print("differs:", out)
            continue
        if out.is_file() and zstd.decompress(out.read_bytes()) == b:
            total += out.stat().st_size
            continue
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(zstd.compress(b, level=19))
        total += out.stat().st_size
        written += 1
    if check:
        print(f"{differ} differences")
        return 1 if differ else 0
    print(f"{written} written, {total / 1e6:.1f} MB in all")
    return 0


if __name__ == "__main__":
    sys.exit(main())
