"""Persistent GPU fault gate. Presence is a stop; only a recorded lead decision can resume.

The shared path is repository-relative, never selected by a caller/environment override.
Writes retain an exclusive .pending journal until the complete JSON history is installed.
An interrupted/failed write therefore also blocks launches. No stale-journal auto-recovery.
"""
import datetime
import json
import os
from pathlib import Path
import tempfile

SHARED_STOP = Path(__file__).resolve().parents[1] / 'build' / 'GPU-STOP.json'
RESUMPTION_INTENT = SHARED_STOP.with_name('WONDER-RESUMPTION.json')
SCHEMA = 'nxrs.gpu-stop.v1'
_FAILED_WRITES = {}


class PersistenceError(OSError):
    """A fault could not be durably recorded; the caller must still stop its child."""


def paths(env=None):
    """Ambient and per-run optional paths are additive, including with an empty env."""
    candidates = [SHARED_STOP, os.environ.get('NXRS_GPU_STOP_PATH')]
    if env is not None:
        candidates.append(env.get('NXRS_GPU_STOP_PATH'))
    out = []
    for value in candidates:
        if value:
            path = Path(os.path.abspath(value))
            if path not in out:
                out.append(path)
    return tuple(out)


def pending_path(path):
    return path.with_name(path.name + '.pending')


def _read(path):
    try:
        with path.open(encoding='utf-8') as stream:
            value = json.load(stream)
    except FileNotFoundError:
        # A dangling symlink is an invalid marker, not permission to resume.
        if path.is_symlink():
            raise ValueError('dangling latch symlink')
        return None
    if (not isinstance(value, dict) or value.get('schema') != SCHEMA
            or value.get('stopped') is not True or not isinstance(value.get('faults'), list)
            or not value['faults'] or any(not isinstance(f, dict) or not f.get('kind')
                                         for f in value['faults'])):
        raise ValueError('invalid GPU-stop schema/history')
    return value


def require_clear(latches):
    """Refuse before claims/launches. Even valid JSON with stopped=false is no override."""
    if RESUMPTION_INTENT.exists() or RESUMPTION_INTENT.is_symlink():
        import wonder_resume
        if not wonder_resume.authorized_here():
            raise SystemExit('GPU STOP: Wonder resumption/recovery intent requires review: %s' %
                             RESUMPTION_INTENT)
    for path in latches:
        try:
            if path in _FAILED_WRITES:
                raise ValueError('earlier persistence failure: ' + _FAILED_WRITES[path])
            try:
                pending_path(path).lstat()
            except FileNotFoundError:
                pass
            else:
                raise ValueError('pending fault journal requires review')
            marker = _read(path)
        except (OSError, ValueError) as exc:
            raise SystemExit('GPU STOP: unreadable/invalid latch %s: %s; '
                             'recorded corrective/discriminator decision required' % (path, exc)) from exc
        if marker is not None:
            first = marker['faults'][0]
            raise SystemExit('GPU STOP: %s; first fault %s (%s); '
                             'recorded corrective/discriminator decision required' %
                             (path, first['kind'], first.get('run', 'run unknown')))


def _append(path, event):
    path.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation prevents competing writers from replacing first-fault history.
    # On any failure keep the journal (even if partial) as a fail-closed marker.
    pending = pending_path(path)
    with pending.open('x', encoding='utf-8') as stream:
        json.dump(event, stream, indent=2, allow_nan=False)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    marker = _read(path)
    if marker is None:
        marker = dict(schema=SCHEMA, stopped=True, faults=[])
    marker['faults'].append(event)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=path.parent,
                                         prefix=path.name + '.', suffix='.tmp', delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(marker, stream, indent=2, allow_nan=False)
            stream.write('\n')
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        pending.unlink()
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


def record_fault(latches, event):
    """Attempt every latch, shared first. Preserve first fault and append subsequent evidence.

    A busy journal refuses rather than retrying: parallel GPU writers are outside the
    exclusive workload protocol. Completely unwritable storage cannot guarantee a durable
    stop across interpreter restarts; report it and also poison this interpreter's gate.
    """
    event = dict(event, recorded_at=datetime.datetime.now(datetime.timezone.utc).isoformat())
    errors = []
    for path in latches:
        try:
            _append(path, event)
        except (OSError, ValueError) as exc:
            _FAILED_WRITES[path] = str(exc)
            errors.append('%s: %s' % (path, exc))
    if errors:
        raise PersistenceError('GPU STOP persistence failed: ' + '; '.join(errors))
