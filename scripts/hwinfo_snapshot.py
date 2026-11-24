"""Read-only bounded file I/O, executed in a short-lived supervised CPU helper.

Never follows the entire growing log. CSV records containing embedded newlines are
unsupported (and rejected by the parser), rather than guessed from a tail fragment.
"""
import base64
import json
import os
from pathlib import Path
import sys

HEADER_BYTES = 64 * 1024
TAIL_BYTES = 256 * 1024
ANCHOR_BYTES = 256


def snapshot(path, offset=0):
    with Path(path).open('rb') as stream:
        stat = os.fstat(stream.fileno())
        size = stat.st_size  # a fixed prefix; later appends belong to the next poll
        header = stream.readline(min(size, HEADER_BYTES + 1))
        if len(header) > HEADER_BYTES or not header.endswith(b'\n'):
            raise ValueError('missing/oversized/incomplete CSV header')
        reset = offset > size
        start = offset if offset and not reset else max(len(header), size - TAIL_BYTES)
        if size - start > TAIL_BYTES:
            raise ValueError('CSV observation backlog exceeds 256 KiB')
        stream.seek(start)
        data = stream.read(size - start)
        if len(data) != size - start:
            raise ValueError('CSV changed/truncated during snapshot')
        if start > len(header) and (not offset or reset):
            # Initial tail may begin inside a record. Never parse that fragment.
            skipped, separator, data = data.partition(b'\n')
            if not separator:
                raise ValueError('no complete record within tail budget')
            start += len(skipped) + 1
        complete = data.rfind(b'\n') + 1
        end = start + complete
        stream.seek(max(0, offset - ANCHOR_BYTES))
        previous = stream.read(min(offset, ANCHOR_BYTES))
        stream.seek(max(0, end - ANCHOR_BYTES))
        anchor = stream.read(min(end, ANCHOR_BYTES))
        after = os.fstat(stream.fileno())
        if after.st_size < size:
            raise ValueError('CSV truncated during snapshot')
        current = Path(path).stat()
        if (current.st_dev, current.st_ino) != (stat.st_dev, stat.st_ino):
            raise ValueError('CSV rotated during snapshot')
    encode = lambda value: base64.b64encode(value).decode('ascii')
    return dict(identity=[stat.st_dev, stat.st_ino], size=size, start=start, end=end,
                header=encode(header), data=encode(data[:complete]), partial=len(data)-complete,
                previous=encode(previous), anchor=encode(anchor), reset=reset)


if __name__ == '__main__':
    print(json.dumps(snapshot(sys.argv[1], int(sys.argv[2]))))
