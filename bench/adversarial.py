"""Throughput on adversarial bodies: the delimiter with one byte changed,
repeated to 1MB, for every position of the changed byte.

Reports raw parser throughput (no stream overhead) on 64KB chunks, so the
numbers are higher than in quick.py.

    python bench/adversarial.py
"""

import time
from types import ModuleType

import multipart
from quick import BOUNDARY, CHUNK_SIZE, Payload

import fast_multipart


def throughput(module: ModuleType, data: bytes, rounds: int = 5) -> float:
    chunks = [data[i : i + CHUNK_SIZE] for i in range(0, len(data), CHUNK_SIZE)]
    best = float("inf")
    for _ in range(rounds):
        start = time.perf_counter()
        parser = module.PushMultipartParser(BOUNDARY)
        for chunk in chunks:
            for _ in parser.parse(chunk):
                pass
        best = min(best, time.perf_counter() - start)
    return len(data) / best / 2**20


def main() -> None:
    delimiter = b"\r\n--" + BOUNDARY
    worst = {}
    for i in range(len(delimiter) + 1):
        if i < len(delimiter):
            pattern = bytearray(delimiter)
            pattern[i] = ord("X") if pattern[i] != ord("X") else ord("Y")
            label = f"byte {i} changed"
        else:
            pattern = delimiter[:-1]
            label = "last byte missing"
        data = Payload().field("f", "f.bin").pattern(bytes(pattern), 1 << 20).end().buf.getvalue()
        for module in (multipart, fast_multipart):
            mbs = throughput(module, data)
            if mbs < worst.get(module.__name__, (float("inf"),))[0]:
                worst[module.__name__] = (mbs, label)
    for name, (mbs, label) in worst.items():
        print(f"{name:16} worst {mbs:8.0f} MB/s ({label})")


if __name__ == "__main__":
    main()
