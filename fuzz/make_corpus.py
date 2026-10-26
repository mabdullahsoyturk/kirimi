"""Write seed inputs for the fuzz targets to corpus/<target>/.

    python make_corpus.py

Inputs for the parse target start with a flags byte (bits 0-1: boundary,
bit 2: small limits) and three chunk-size bytes, followed by the body. See
`check_parse` in crates/core/src/fuzzing.rs.
"""

from collections.abc import Iterator
from pathlib import Path

BOUNDARIES = [b"b", b"boundary", b"----WebKitFormBoundary7MA4YWxkTrZu0gW"]


def form(
    boundary: bytes,
    fields: list[tuple[list[bytes], bytes]],
    preamble: bytes = b"",
    epilogue: bytes = b"\r\n",
) -> bytes:
    out = preamble
    for headers, body in fields:
        out += b"--" + boundary + b"\r\n" + b"".join(h + b"\r\n" for h in headers) + b"\r\n"
        out += body + b"\r\n"
    return out + b"--" + boundary + b"--" + epilogue


def parse_seeds() -> Iterator[tuple[str, bytes]]:
    for index, boundary in enumerate(BOUNDARIES):
        delimiter = b"\r\n--" + boundary
        bodies = [
            form(boundary, []),
            form(boundary, [([b'Content-Disposition: form-data; name="a"'], b"value")]),
            form(
                boundary,
                [
                    ([b'Content-Disposition: form-data; name="a"'], b"1"),
                    (
                        [
                            b'Content-Disposition: form-data; name="f"; filename="C:\\x\\f%22.txt"',
                            b"Content-Type: text/plain; charset=latin1",
                            b"Content-Length: 4",
                        ],
                        b"\r\n\r\n",
                    ),
                ],
                preamble=b"preamble\r\n",
                epilogue=b"\r\nepilogue",
            ),
            form(
                boundary,
                [([b"Content-Disposition: form-data;", b' name="folded"'], delimiter[:-1] * 3)],
            ),
            form(boundary, [([b'Content-Disposition: form-data; name="crlf"'], b"\r\n" * 40)]),
        ]
        for limits in (0, 4):
            for body_index, body in enumerate(bodies):
                for sizes in (b"\xff\xff\xff", b"\x01\x02\x03", b"\x07\x0d\x40"):
                    yield (
                        f"b{index}-l{limits}-{body_index}-{sizes.hex()}",
                        (bytes([index | limits]) + sizes + body),
                    )


def search_seeds() -> Iterator[tuple[str, bytes]]:
    yield "plain", b"\x08boundary" + b"x" * 100 + b"\r\n--boundary"
    yield "near_miss", b"\x08boundary" + b"\r\n--boundarX" * 20
    yield "crlf", b"\x01b" + b"\r\n" * 64 + b"\r\n--b"


def header_seeds() -> Iterator[tuple[str, bytes]]:
    yield "cd", b'form-data; name="a"; filename="b.txt"'
    yield "escaped", b'form-data; name="a\\"b"; filename="C:\\x\\y.txt"'
    yield "percent", b'form-data; name="a%22b%0D%0A"'
    yield "options", b'Text/Plain; charset=UTF-8; x = "y"; junk'
    yield "unicode", 'form-data; name="\xe4\u20ac"'.encode()


def main() -> None:
    root = Path(__file__).parent / "corpus"
    for target, seeds in (
        ("parse", parse_seeds),
        ("search", search_seeds),
        ("headers", header_seeds),
    ):
        directory = root / target
        directory.mkdir(parents=True, exist_ok=True)
        count = 0
        for name, data in seeds():
            (directory / f"seed-{name}").write_bytes(data)
            count += 1
        print(f"{target}: {count} seeds in {directory}")


if __name__ == "__main__":
    main()
