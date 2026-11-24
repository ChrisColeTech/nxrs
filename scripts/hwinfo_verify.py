"""Bounded read-only normal-provider observation. Never launches a workload or clears stops."""
import argparse
import datetime
import json
from pathlib import Path
import time

import gpu_stop
from harness_monitor import Monitor


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', type=Path, required=True)
    args = parser.parse_args()
    monitor = Monitor()
    records = []
    try:
        gpu_stop.require_clear(gpu_stop.paths())
        stop = 'clear (owner hold is separate and remains active)'
    except SystemExit as exc:
        stop = str(exc)
    for index in range(3):
        if index:
            time.sleep(2.1)
        sample = monitor.sample()
        reason = monitor.check(sample)
        record = dict(observed=datetime.datetime.now().astimezone().isoformat(),
                      sample=sample, thermal_refusal=reason, persistent_stop=stop)
        records.append(record)
        cpu = dict(sample['cpu_c'])
        cpu['sensor_count'] = len(cpu.pop('sensors', []))
        print(json.dumps(dict(observation=index+1, cpu=cpu, thermal_refusal=reason,
                              persistent_stop=stop), ensure_ascii=True), flush=True)
    args.evidence.write_text(json.dumps(records, indent=2, allow_nan=False), encoding='utf-8')


if __name__ == '__main__':
    main()
