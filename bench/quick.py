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

import multipart

import fast_multipart

BOUNDARY = b"------------------------WqclBHaXe8KIsoSum4zfZ6"
CHUNK_SIZE = 2**16
SPOOL_LIMIT = 1024 * 500


class Payload:
    def __init__(self):
        self.buf = io.BytesIO()

    def write(self, data):
        self.buf.write(data.encode() if isinstance(data, str) else data)

    def field(self, name, filename=None):
        if self.buf.tell():
            self.write(b"\r\n")
        self.write(b"--%s\r\n" % BOUNDARY)
        disposition = f'form-data; name="{name}"'
        if filename:
            disposition += f'; filename="{filename}"'
        self.write(f"Content-Disposition: {disposition}\r\n\r\n")
        return self

    def pattern(self, pattern, size):
        pattern = pattern.encode() if isinstance(pattern, str) else pattern
        self.write(pattern * (size // len(pattern)) + pattern[: size % len(pattern)])
        return self

    def end(self):
        if self.buf.tell():
            self.write(b"\r\n")
        self.write(b"--%s--\r\n" % BOUNDARY)
        return self


def scenarios():
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


def sansio(module):
    def run(stream):
        read = stream.read
        with module.PushMultipartParser(BOUNDARY) as parser:
            parse = parser.parse
            while not parser.closed:
                for _ in parse(read(CHUNK_SIZE)):
                    pass

    return run


def blocking(module):
    def run(stream):
        list(
            module.MultipartParser(
                stream, boundary=BOUNDARY, buffer_size=CHUNK_SIZE, spool_limit=SPOOL_LIMIT
            )
        )

    return run


def baseline(stream):
    read = stream.read
    for _ in iter(lambda: read(CHUNK_SIZE), b""):
        pass


def measure(func, data, min_time=0.5, rounds=5):
    """Best time per call, with the stream reading overhead subtracted."""

    def best(f):
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


def main(patterns):
    parsers = [
        ("multipart", "non-blocking", sansio(multipart)),
        ("fast_multipart", "non-blocking", sansio(fast_multipart)),
        ("multipart", "blocking", blocking(multipart)),
        ("fast_multipart", "blocking", blocking(fast_multipart)),
    ]
    print(f"{'scenario':<16} {'mode':<13} {'multipart':>14} {'fast_multipart':>16} {'speedup':>8}")
    for name, payload in scenarios():
        if patterns and not any(fnmatch.fnmatch(name, p) for p in patterns):
            continue
        data = payload.buf.getvalue()
        for mode in ("non-blocking", "blocking"):
            results = {}
            for parser_name, parser_mode, func in parsers:
                if parser_mode == mode:
                    results[parser_name] = len(data) / measure(func, data) / 2**20
            ref, ours = results["multipart"], results["fast_multipart"]
            print(
                f"{name:<16} {mode:<13} {ref:>9.1f} MB/s {ours:>11.1f} MB/s {ours / ref:>7.2f}x",
                flush=True,
            )


if __name__ == "__main__":
    main(sys.argv[1:])
