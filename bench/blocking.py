"""Blocking API benchmark with interleaved runs and median times.

Scenarios that spool parts to temporary files are dominated by file system
noise, so runs of both parsers alternate and the median is reported.

    python bench/blocking.py [scenario-glob ...]
"""

import fnmatch
import gc
import io
import statistics
import sys
import time

import multipart
from quick import blocking, scenarios

import kirimi


def main(patterns: list[str], rounds: int = 31) -> None:
    parsers = {"multipart": blocking(multipart), "kirimi": blocking(kirimi)}
    print(f"{'scenario':<16} {'multipart':>12} {'kirimi':>15} {'speedup':>8}   (median per run)")
    for name, payload in scenarios():
        if patterns and not any(fnmatch.fnmatch(name, p) for p in patterns):
            continue
        data = payload.buf.getvalue()
        stream = io.BytesIO(data)
        repeat = max(1, int(2e6 // len(data)))
        times = {key: [] for key in parsers}
        for _ in range(rounds):
            for key, func in parsers.items():
                gc.collect()
                start = time.perf_counter()
                for _ in range(repeat):
                    stream.seek(0)
                    func(stream)
                times[key].append((time.perf_counter() - start) / repeat)
        ref = statistics.median(times["multipart"])
        ours = statistics.median(times["kirimi"])
        print(
            f"{name:<16} {ref * 1e6:9.1f} us {ours * 1e6:12.1f} us {ref / ours:7.2f}x", flush=True
        )


if __name__ == "__main__":
    main(sys.argv[1:])
