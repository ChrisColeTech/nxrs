"""Fixed Wonder build adapters. Real launcher flow, real release, one native tool at a time."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import harness_job as hj
from build_supervisor import write_json

ROOT = Path(__file__).resolve().parents[1]
LIBRARY = Path(r'C:\Users\Risky Biz\AppData\Local\nxrs\library.json')
TITLE = '5afdfd1c-3c79-4576-ab73-761b7b1852b8'
RELEASE = ROOT / 'dist/nxrs'
WORKERS = 4  # the machine's normal worker count; native tools share the same bound


def entry():
    library = json.loads(LIBRARY.read_text(encoding='utf-8-sig'))
    matches = [g for g in library['games'] if g['id'] == TITLE]
    if len(matches) != 1:
        raise ValueError('Wonder entry missing/ambiguous')
    return matches[0]


def engine_args(args, game):
    if args == ['contract']:
        return args
    if not args or args[0] != 'build':
        raise ValueError('only Wonder build or contract is authorized')
    if Path(args[1]) != Path(game['source']):
        raise ValueError('wrong source')
    for key, expected in (('--outdir', game['buildDir']), ('-o', game['outputName'])):
        i = args.index(key)
        if (Path(args[i+1]) != Path(expected) if key == '--outdir' else args[i+1] != expected):
            raise ValueError('wrong Wonder output')
    if args.count('--jobs') != 1:
        raise ValueError('expected one normal launcher worker argument')
    result = list(args)
    result[result.index('--jobs')+1] = str(WORKERS)
    return result


class ToolSlot:
    """One of `slots` named mutexes (`name-0` ..): at most `slots` native tools run at once."""

    def __init__(self, name, deadline, slots=1):
        self.name, self.deadline, self.slots, self.handle = name, deadline, slots, None

    def __enter__(self):
        handles = [hj.checked(hj.create_mutex(None, False, '%s-%d' % (self.name, k)))
                   for k in range(self.slots)]
        try:
            while time.monotonic() < self.deadline:
                for k, handle in enumerate(handles):
                    status = hj.wait(handle, 100 if self.slots == 1 else 0)
                    if status == 0:
                        self.handle = handles.pop(k)
                        return self
                    if status != 258:
                        raise RuntimeError('tool slot abandoned/failed; refuse overlap')
                if self.slots > 1:
                    time.sleep(.05)
            raise TimeoutError('tool slot deadline')
        finally:
            for handle in handles:
                hj.close(handle)

    def __exit__(self, *exc):
        try:
            hj.checked(hj.release_mutex(self.handle))
        finally:
            hj.checked(hj.close(self.handle))


def main():
    kind, args = sys.argv[1], sys.argv[2:]
    job = os.environ['NXRS_WONDER_BUILD_JOB']
    if not hj.owns_named(job, os.getpid()):
        raise RuntimeError('adapter requires the supervised Wonder build job')
    folder = Path(os.environ['NXRS_WONDER_EVIDENCE'])
    deadline = float(os.environ['NXRS_WONDER_DEADLINE'])
    if kind == 'engine':
        forwarded = engine_args(args, entry())
        if forwarded[0] == 'build':
            write_json(folder / 'engine-command.json', dict(original=args, forwarded=forwarded,
                       executable=str(RELEASE / 'nxrs-engine/nxrs-build.exe')))
        return subprocess.call([str(RELEASE / 'nxrs-engine/nxrs-build.exe'), *forwarded], cwd=ROOT)
    executable = {'clang': os.environ['NXRS_WONDER_REAL_CLANG'],
                  'xlate': str(RELEASE / 'nxrs-engine/tools/dmsh_xlate.exe'),
                  'spirv': str(RELEASE / 'nxrs-engine/tools/spirv-val.exe')}[kind]
    if kind == 'clang' and '-fuse-ld=lld' in args and '-c' not in args:
        args = [*args, '-Wl,/threads:1']
    slot = os.environ['NXRS_WONDER_TOOL_SLOT']
    with ToolSlot(slot + '-evidence', deadline):
        counts = folder / 'tool-counts.json'
        values = json.loads(counts.read_text()) if counts.exists() else {}
        values[kind] = values.get(kind, 0)+1
        write_json(counts, values)
        if kind == 'clang' and '-c' not in args:
            with (folder / 'link-commands.jsonl').open('a', encoding='utf-8') as stream:
                stream.write(json.dumps([executable, *args])+'\n')
    with ToolSlot(slot, deadline, WORKERS):
        return subprocess.call([executable, *args], cwd=ROOT)


if __name__ == '__main__':
    sys.exit(main())
