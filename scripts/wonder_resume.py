"""One explicitly reviewed Wonder resumption. Original GPU-stop checks always run.

The durable intent itself blocks every other process/thread and survives process
death/power loss. Only this live lease-held scope can pass that additional gate;
it never overrides an existing stop, pending fault or in-process poisoned latch.
"""
import hashlib
import json
import os
from pathlib import Path
import threading
import uuid

import gpu_stop
from build_supervisor import write_json

REVIEWED = ('22072e11589fb20bfaac1b44bf9c4514b9f3dec8cb811b5aa096cb8f2c79fbc4',
            '3b32f776abd1710a7f47d395a07e11c75f6172aefe6b8c90977226eb72d0d9d7')
_active = None


def sha(data):
    return hashlib.sha256(data).hexdigest()


def exclusive_bytes(path, data):
    with path.open('xb') as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    if path.read_bytes() != data:
        raise OSError('history/intent verification failed')


def authorized_here():
    if _active is None or _active['pid'] != os.getpid() or _active['thread'] != threading.get_ident():
        return False
    try:
        with _active['lease'].borrow():
            return sha(gpu_stop.RESUMPTION_INTENT.read_bytes()) == _active['intent_sha256']
    except (OSError, RuntimeError):
        return False


class WonderResumption:
    def __init__(self, lease, folder):
        self.lease, self.folder = lease, Path(folder)
        self.stop = gpu_stop.SHARED_STOP
        self.pending = gpu_stop.pending_path(self.stop)
        self.outcome = 'interrupted/unknown'

    def __enter__(self):
        global _active
        with self.lease.borrow():
            if _active or gpu_stop.RESUMPTION_INTENT.exists():
                raise RuntimeError('previous resumption requires review')
            originals = [self.stop.read_bytes(), self.pending.read_bytes()]
            if tuple(map(sha, originals)) != REVIEWED:
                raise RuntimeError('shared fault history changed since lead review; refusal')
            history = self.folder / 'history'
            history.mkdir()
            for name, data in zip(('shared-before.json', 'pending-before.json'), originals):
                exclusive_bytes(history / name, data)
            intent = dict(schema='nxrs.wonder-resumption.v1', title='5afdfd1c-3c79-4576-ab73-761b7b1852b8',
                          runner_pid=os.getpid(), nonce=uuid.uuid4().hex, max_process_seconds=330,
                          history=str(history), original_sha256=list(REVIEWED),
                          recovery='Blocking recovery intent. Never kill recorded PIDs or auto-resume.',
                          decision='THERMAL-INCIDENT.md: one Wonder rebuild and validation authorized')
            raw = json.dumps(intent, indent=2).encode('utf-8')
            exclusive_bytes(gpu_stop.RESUMPTION_INTENT, raw)
            exclusive_bytes(self.folder / 'resumption-intent.json', raw)
            _active = dict(pid=os.getpid(), thread=threading.get_ident(), lease=self.lease,
                           intent_sha256=sha(raw))
            try:
                # Recheck exact bytes after preservation, immediately before clearing this gate.
                if [self.stop.read_bytes(), self.pending.read_bytes()] != originals:
                    raise RuntimeError('fault history changed while archiving')
                self.pending.unlink()
                self.stop.unlink()
            except BaseException:
                try:
                    self.restore()
                finally:
                    _active = None
                raise
        return self

    def restore(self):
        with self.lease.borrow():
            history = self.folder / 'history'
            original = json.loads((history / 'shared-before.json').read_bytes())
            current = gpu_stop._read(self.stop)
            old = original['faults']
            fresh = current['faults'] if current else []
            combined = fresh if fresh[:len(old)] == old else old + fresh
            if self.pending.exists():
                pending = self.pending.read_bytes()
                saved = history / 'pending-after.json'
                exclusive_bytes(saved, pending)
                combined.append(dict(kind='wonder-pending-fault-retained', sha256=sha(pending), path=str(saved)))
            combined.append(dict(kind='wonder-authorized-attempt-closed', outcome=self.outcome,
                                 run=str(self.folder), history=str(history), pending_before_sha256=REVIEWED[1]))
            write_json(self.stop, dict(schema=gpu_stop.SCHEMA, stopped=True, faults=combined))
            # Never replace a newly created fault journal with the historical journal.
            if not self.pending.exists():
                exclusive_bytes(self.pending, (history / 'pending-before.json').read_bytes())
            write_json(self.folder / 'stop-restoration.json', dict(blocked=True,
                       stop_sha256=sha(self.stop.read_bytes()), pending_sha256=sha(self.pending.read_bytes()),
                       old_faults=len(old), final_faults=len(combined)))
            gpu_stop.RESUMPTION_INTENT.unlink()  # only after the shared blocking files are durable

    def __exit__(self, kind, value, traceback):
        global _active
        if value is not None:
            self.outcome = '%s: %s' % (kind.__name__, value)
        try:
            self.restore()
        finally:
            _active = None  # failed restoration leaves the durable intent blocking all callers
        return False
