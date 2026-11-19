"""Contract v1 for RecompLauncher: the launcher runs `python -u recomp_cli.py <args>`, which imports this module from
its own folder and returns main(). Here main() runs nxrs.exe with the same arguments, its output straight to the
launcher (merged, unbuffered: nxrs flushes every line), and returns its exit code.

nxrs.exe is NXRS_EXE, else beside this file, else bin/nxrs.exe below it.
"""
import os
import subprocess
import sys


def nxrs_path():
    here = os.path.dirname(os.path.abspath(__file__))
    for p in (os.environ.get('NXRS_EXE'), os.path.join(here, 'nxrs.exe'), os.path.join(here, 'bin', 'nxrs.exe')):
        if p and os.path.isfile(p):
            return p
    raise SystemExit('nxrs.exe is missing beside %s (or set NXRS_EXE)' % here)


def main():
    sys.stdout.flush()
    return subprocess.call([nxrs_path()] + sys.argv[1:])


if __name__ == '__main__':
    sys.exit(main())
