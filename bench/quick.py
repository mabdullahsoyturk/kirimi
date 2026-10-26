"""Quick benchmark against the ``multipart`` library.

Scenarios, chunk size and spool limit are the same as in
https://github.com/defnull/multipart_bench, but this script has no extra
dependencies and reports the best of several runs instead of confidence
intervals. Use multipart_bench itself for publishable numbers.

    python bench/quick.py [scenario-glob ...]
"""

import fnmatch
import gc
import io
import string
import sys
import time
from collections.abc import Callable, Iterator
from types import ModuleType
from typing import Self

import multipart

import kirimi

BOUNDARY = b"------------------------WqclBHaXe8KIsoSum4zfZ6"
CHUNK_SIZE = 2**16
SPOOL_LIMIT = 1024 * 500

#: Parses a whole payload from a stream.
Runner = Callable[[io.BytesIO], None]


class Payload:
    def __init__(self) -> None:
        self.buf = io.BytesIO()

    def write(self, data: str | bytes) -> None:
        self.buf.write(data.encode() if isinstance(data, str) else data)

    def field(self, name: str, filename: str | None = None) -> Self:
        if self.buf.tell():
            self.write(b"\r\n")
        self.write(b"--%s\r\n" % BOUNDARY)
        disposition = f'form-data; name="{name}"'
        if filename:
            disposition += f'; filename="{filename}"'
        self.write(f"Content-Disposition: {disposition}\r\n\r\n")
        return self

    def pattern(self, pattern: str | bytes, size: int) -> Self:
        pattern = pattern.encode() if isinstance(pattern, str) else pattern
        self.write(pattern * (size // len(pattern)) + pattern[: size % len(pattern)])
        return self

    def end(self) -> Self:
        if self.buf.tell():
            self.write(b"\r\n")
        self.write(b"--%s--\r\n" % BOUNDARY)
        return self


def scenarios() -> Iterator[tuple[str, Payload]]:
    yield "empty", Payload().end()

    p = Payload()
    p.field("email").pattern(string.printable, 24)
    p.field("password").pattern(string.printable, 16)
    yield "simple", p.end()

    p = Payload()
    for i in range(100):
        p.field(f"field{i}").pattern(string.printable, i)
    yield "large", p.end()

    yield "upload", Payload().field("foo", "bar.bin").pattern(string.printable, 32 << 20).end()

    p = Payload()
    p.field("field").pattern(string.printable, 16)
    p.field("file", "file.bin").pattern(string.printable, 1 << 20)
    p.field("field2").pattern(string.printable, 32)
    p.field("file2", "file2.bin").pattern(string.printable, 2 << 20)
    yield "mixed", p.end()

    yield "worstcase_crlf", Payload().field("file", "file.bin").pattern("\r\n", 1 << 20).end()
    yield "worstcase_lf", Payload().field("file", "file.bin").pattern("\n", 1 << 20).end()
    yield (
        "worstcase_bchar",
        Payload().field("file", "file.bin").pattern(b"\r\n--" + BOUNDARY[:-1], 1 << 20).end(),
    )

    p = Payload().pattern(string.printable, 1 << 20)
    p.field("file", "file.bin").write(b"Content\r\n")
    p.end().pattern(string.printable, 1 << 20)
    yield "worstcase_junk", p


def sansio(module: ModuleType) -> Runner:
    def run(stream: io.BytesIO) -> None:
        read = stream.read
        with module.PushMultipartParser(BOUNDARY) as parser:
            parse = parser.parse
            while not parser.closed:
                for _ in parse(read(CHUNK_SIZE)):
                    pass

    return run


def blocking(module: ModuleType) -> Runner:
    def run(stream: io.BytesIO) -> None:
        list(
            module.MultipartParser(
                stream, boundary=BOUNDARY, buffer_size=CHUNK_SIZE, spool_limit=SPOOL_LIMIT
            )
        )

    return run


def baseline(stream: io.BytesIO) -> None:
    read = stream.read
    for _ in iter(lambda: read(CHUNK_SIZE), b""):
        pass


def measure(func: Runner, data: bytes, min_time: float = 0.5, rounds: int = 5) -> float:
    """Best time per call, with the stream reading overhead subtracted."""

    def best(f: Runner) -> float:
        stream = io.BytesIO(data)
        seek = stream.seek
        n, elapsed = 1, 0.0
        while True:  # calibrate
            start = time.perf_counter()
            for _ in range(n):
                seek(0)
                f(stream)
            elapsed = time.perf_counter() - start
            if elapsed > min_time / rounds:
                break
            n *= 2
        times = []
        for _ in range(rounds):
            gc.collect()
            start = time.perf_counter()
            for _ in range(n):
                seek(0)
                f(stream)
            times.append((time.perf_counter() - start) / n)
        return min(times)

    return max(best(func) - best(baseline), 1e-9)


def main(patterns: list[str]) -> None:
    parsers = [
        ("multipart", "non-blocking", sansio(multipart)),
        ("kirimi", "non-blocking", sansio(kirimi)),
        ("multipart", "blocking", blocking(multipart)),
        ("kirimi", "blocking", blocking(kirimi)),
    ]
    print(f"{'scenario':<16} {'mode':<13} {'multipart':>14} {'kirimi':>16} {'speedup':>8}")
    for name, payload in scenarios():
        if patterns and not any(fnmatch.fnmatch(name, p) for p in patterns):
            continue
        data = payload.buf.getvalue()
        for mode in ("non-blocking", "blocking"):
            results = {}
            for parser_name, parser_mode, func in parsers:
                if parser_mode == mode:
                    results[parser_name] = len(data) / measure(func, data) / 2**20
            ref, ours = results["multipart"], results["kirimi"]
            print(
                f"{name:<16} {mode:<13} {ref:>9.1f} MB/s {ours:>11.1f} MB/s {ours / ref:>7.2f}x",
                flush=True,
            )


if __name__ == "__main__":
    main(sys.argv[1:])
