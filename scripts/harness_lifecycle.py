"""Positive cleanup proof shared by the runner and its save transaction."""
from dataclasses import dataclass

from harness_job import CleanupError, CleanupTasks

WORKER_JOIN_SECONDS = 3


@dataclass
class RunState:
    launched: bool = False
    workers_confirmed: bool = False
    cleanup_confirmed: bool = False
    uncertain: bool = False

    def resumed(self):
        self.launched = True


def stop_and_join(job, children, workers, state):
    """Attempt every step even when termination, drain, wait, join or close fails."""
    tasks = CleanupTasks()
    tasks.attempt('terminate owned job', job.kill)
    tasks.attempt('confirm job drain', job.drain)
    for child in children:
        tasks.attempt('confirm process %d exit' % child.pid, lambda c=child: c.wait(timeout=2))
    for name, thread in workers:
        if thread is None:
            continue
        def join(thread=thread, name=name):
            thread.join(timeout=WORKER_JOIN_SECONDS)
            if thread.is_alive():
                raise CleanupError('%s worker did not stop; recovery required' % name)
        tasks.attempt('join ' + name, join)
    # Always close all child handles, including if an earlier job operation failed.
    for child in children:
        tasks.attempt('close process %d handle' % child.pid, child.close)
    state.workers_confirmed = not tasks.errors
    state.uncertain |= bool(tasks.errors)
    return tasks


def finish_job(job, state):
    """Successful job.close alone does not prove that Python callbacks joined."""
    state.cleanup_confirmed = False
    try:
        closed = job.close()
        if closed is not True or not state.workers_confirmed or state.uncertain:
            raise CleanupError('positive job-drain and worker-join proof missing; recovery required')
    except BaseException as exc:
        state.uncertain = True
        raise CleanupError('run cleanup unconfirmed: %s' % exc) from exc
    state.cleanup_confirmed = True
