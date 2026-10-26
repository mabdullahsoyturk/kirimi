"""Differential tests against the pure-Python ``multipart`` library.

Both parsers get the same input, split into the same chunks, and must agree on
segments, headers, body data and errors. Documented deviations are listed in
``KNOWN_DEVIATIONS``.
"""

import io
from collections.abc import Iterator
from types import ModuleType

import multipart
import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

import fast_multipart as fm

BOUNDARY = b"----b0undary"

# Errors where fast_multipart is stricter than multipart on purpose, because
# multipart's result depends on how the input is split into chunks:
# - Bare LF in a segment header line is always rejected. multipart accepts it
#   in the middle of a line if a CRLF follows in the same chunk.
# - A first boundary that does not follow a CRLF is always rejected. multipart
#   misses the byte in front of it if the boundary straddles a chunk border.
KNOWN_DEVIATIONS = {
    "Invalid line break in segment header",
    "Unexpected byte in front of first boundary",
}

# Errors that multipart raises depending on how the input is split, where
# fast_multipart does not:
# - multipart counts the CR of a header line's CRLF towards the length limit
#   while the LF has not arrived yet.
# For these, multipart is re-run on the input in one piece, which must then
# agree with fast_multipart.
CHUNK_DEPENDENT_IN_MULTIPART = {"Maximum segment header length exceeded"}


def is_error(result: object) -> bool:
    return isinstance(result, tuple) and result[0] == "error"


header_text = st.text(
    alphabet=st.sampled_from(list('abcXYZ019 ;="\\%-_/:.\t\xe4\u20ac')), max_size=30
)
body_bytes = st.lists(
    st.sampled_from(
        [b"x", b"\r", b"\n", b"\r\n", b"-", b"--", b"\r\n--", BOUNDARY, BOUNDARY[:5], b"\x00\xff"]
    ),
    max_size=30,
).map(b"".join)


@st.composite
def header_lines(draw: st.DrawFn) -> bytes:
    name = draw(st.text(alphabet="abcxyz019-_", min_size=1, max_size=8))
    kind = draw(st.sampled_from(["disposition", "type", "length", "custom", "continuation"]))
    if kind == "disposition":
        value = "form-data" + draw(header_text)
    elif kind == "type":
        value = draw(st.sampled_from(["text/plain", "Application/X"])) + draw(header_text)
    elif kind == "length":
        value = str(draw(st.integers(0, 40)))
        return b"Content-Length: " + value.encode()
    elif kind == "continuation":
        return b" " + draw(header_text).encode()
    else:
        value = draw(header_text)
    header = {"disposition": "Content-Disposition", "type": "content-type"}.get(kind, name)
    return (header + ": " + value).encode("utf8")


@st.composite
def multipart_bodies(draw: st.DrawFn) -> bytes:
    out = draw(st.sampled_from([b"", b"preamble\r\n", b"\r\n", b"x"]))
    for _ in range(draw(st.integers(0, 4))):
        out += b"--" + BOUNDARY + b"\r\n"
        name = draw(header_text).encode("utf8")
        disposition = b'Content-Disposition: form-data; name="' + name + b'"'
        if draw(st.booleans()):
            disposition += b'; filename="' + draw(header_text).encode("utf8") + b'"'
        lines = [disposition] + draw(st.lists(header_lines(), max_size=3))
        if draw(st.booleans()):
            draw(st.randoms()).shuffle(lines)
        out += b"".join(line + b"\r\n" for line in lines) + b"\r\n"
        out += draw(body_bytes) + b"\r\n"
    out += b"--" + BOUNDARY + b"--"
    out += draw(st.sampled_from([b"", b"\r\n", b"\r\nepilogue"]))

    # Random mutations to produce invalid or unusual input.
    for _ in range(draw(st.integers(0, 2))):
        pos = draw(st.integers(0, len(out)))
        action = draw(st.sampled_from(["insert", "delete", "truncate"]))
        if action == "insert":
            out = out[:pos] + draw(st.sampled_from([b"\r", b"\n", b"-", b"x", b" "])) + out[pos:]
        elif action == "delete":
            out = out[:pos] + out[pos + 1 :]
        else:
            out = out[:pos]
    return out


def chunked(data: bytes, sizes: list[int]) -> Iterator[bytes]:
    pos, i = 0, 0
    while pos < len(data):
        size = sizes[i % len(sizes)] if sizes else len(data)
        yield data[pos : pos + size]
        pos += size
        i += 1


def run(module: ModuleType, data: bytes, sizes: list[int], **kwargs: object) -> tuple[object, ...]:
    """Parse and return a normalized result or the error."""
    parser = module.PushMultipartParser(BOUNDARY, **kwargs)
    parts = []
    try:
        with parser:
            for chunk in chunked(data, sizes):
                for event in parser.parse(chunk):
                    if event is None:
                        parts[-1][-1] = True
                    elif isinstance(event, (bytes, bytearray)):
                        parts[-1][-2] += event
                    else:
                        parts.append(
                            [
                                event.headerlist,
                                event.disposition,
                                event.name,
                                event.filename,
                                event.content_type,
                                event.charset,
                                b"",
                                False,
                            ]
                        )
    except multipart.MultipartError as err:
        return ("error", type(err).__name__, err.args[0])
    except fm.MultipartError as err:
        return ("error", type(err).__name__, err.args[0])
    return ("ok", parts)


def check(data: bytes, sizes: list[int], **kwargs: object) -> None:
    expected = run(multipart, data, sizes, **kwargs)
    actual = run(fm, data, sizes, **kwargs)
    if is_error(actual) and actual[2] in KNOWN_DEVIATIONS:
        return
    if is_error(expected) and expected[2] in CHUNK_DEPENDENT_IN_MULTIPART and actual != expected:
        expected = run(multipart, data, [], **kwargs)
    assert actual == expected


chunk_sizes = st.lists(st.integers(1, 64), max_size=5)


@settings(max_examples=3000, suppress_health_check=[HealthCheck.too_slow], deadline=None)
@given(multipart_bodies(), chunk_sizes, st.booleans())
def test_differential(data: bytes, sizes: list[int], strict: bool) -> None:
    check(data, sizes, strict=strict)


@settings(max_examples=500, deadline=None)
@given(multipart_bodies(), chunk_sizes)
def test_differential_limits(data: bytes, sizes: list[int]) -> None:
    check(data, sizes, max_header_count=2, max_header_size=40, max_segment_size=8)


@settings(max_examples=500, deadline=None)
@given(multipart_bodies())
def test_chunking_does_not_matter(data: bytes) -> None:
    whole = run(fm, data, [])
    for size in (1, 2, 3, 7):
        assert run(fm, data, [size]) == whole


@pytest.mark.parametrize(
    "data",
    [
        b"--" + BOUNDARY + b"--",
        b"--" + BOUNDARY + b"\r\n\r\n\r\n--" + BOUNDARY + b"--",
        b"x--" + BOUNDARY + b"\r\n",
        b"\r\n--" + BOUNDARY + b" \r\n",
        b"--" + BOUNDARY + b"\r\nContent-Disposition: form-data\r\n\r\nx\r\n--" + BOUNDARY + b"--",
        b"--" + BOUNDARY + b"\r\n Content-Disposition: form-data\r\n\r\n",
    ],
)
def test_edge_cases(data: bytes) -> None:
    for strict in (False, True):
        for sizes in ([], [1], [5]):
            check(data, sizes, strict=strict)


# Header helper functions

header_values = st.text(
    alphabet=st.sampled_from(list('aZ09 ;="\\%-_/:.\t\n\xe4\u20ac*!')), max_size=40
)


@settings(max_examples=3000)
@given(header_values)
def test_parse_options_header(value: str) -> None:
    for prefix in ("", "form-data; ", "text/plain;"):
        header = prefix + value
        assert fm.parse_options_header(header) == multipart.parse_options_header(header)
        assert fm.parse_options_header(
            header, unquote=fm.content_disposition_unquote
        ) == multipart.parse_options_header(header, unquote=multipart.content_disposition_unquote)


@settings(max_examples=3000)
@given(header_values, st.booleans())
def test_unquote(value: str, filename: bool) -> None:
    for v in (value, '"' + value + '"'):
        assert fm.header_unquote(v, filename) == multipart.header_unquote(v, filename)
        assert fm.content_disposition_unquote(v, filename) == multipart.content_disposition_unquote(
            v, filename
        )


@settings(max_examples=3000)
@given(header_values)
def test_parse_content_disposition(value: str) -> None:
    for prefix in ("", 'form-data; name="', "form-data; name="):
        header = prefix + value
        assert fm.parse_content_disposition(header) == multipart.parse_content_disposition(header)


# Blocking parser


def run_blocking(
    module: ModuleType, data: bytes, read_size: int, **kwargs: object
) -> list[tuple[object, ...]] | tuple[str, str, str]:
    stream = io.BytesIO(data)

    def read(n: int) -> bytes:
        return stream.read(min(n, read_size))

    reader = type("Reader", (), {"read": staticmethod(read)})()
    try:
        parts = module.MultipartParser(reader, BOUNDARY, **kwargs).parts()
        return [
            (
                p.name,
                p.filename,
                p.charset,
                p.content_type,
                p.size,
                p.is_buffered(),
                p.raw,
                p.headerlist,
            )
            for p in parts
        ]
    except (multipart.MultipartError, fm.MultipartError) as err:
        return ("error", type(err).__name__, err.args[0])


@settings(max_examples=1000, deadline=None)
@given(
    multipart_bodies(),
    st.integers(1, 64),
    st.sampled_from([0, 1, 5, 20, 1000]),
    st.sampled_from([1, 8, 64]),
    st.sampled_from([None, 30, 1000]),
)
def test_differential_blocking(
    data: bytes, read_size: int, spool_limit: int, buffer_size: int, memory_limit: int | None
) -> None:
    kwargs = dict(spool_limit=spool_limit, buffer_size=buffer_size)
    if memory_limit is not None:
        kwargs["memory_limit"] = memory_limit
    expected = run_blocking(multipart, data, read_size, **kwargs)
    actual = run_blocking(fm, data, read_size, **kwargs)
    if is_error(actual) and actual[2] in KNOWN_DEVIATIONS:
        return
    if is_error(expected) and expected[2] in CHUNK_DEPENDENT_IN_MULTIPART and actual != expected:
        whole = dict(kwargs, buffer_size=len(data) + 1)
        expected = run_blocking(multipart, data, len(data) + 1, **whole)
    assert actual == expected
