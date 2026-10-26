import asyncio
import io
import math
from typing import TypedDict, Unpack

import pytest

import kirimi as km

Event = km.MultipartSegment | bytes | None
Limit = int | float | None


class PushOptions(TypedDict, total=False):
    content_length: int
    max_header_size: Limit
    max_header_count: Limit
    max_segment_size: Limit
    max_segment_count: Limit
    header_charset: str
    strict: bool


def form(*fields: tuple[str, str | None, bytes], boundary: bytes = b"boundary") -> bytes:
    """Build a multipart body from (name, filename, data) tuples."""
    out = b""
    for name, filename, data in fields:
        out += b"--" + boundary + b"\r\n"
        out += b'Content-Disposition: form-data; name="%s"' % name.encode()
        if filename:
            out += b'; filename="%s"' % filename.encode()
        out += b"\r\n\r\n" + data + b"\r\n"
    return out + b"--" + boundary + b"--\r\n"


def parse_events(
    data: bytes, chunk_size: int | None = None, **kwargs: Unpack[PushOptions]
) -> list[Event]:
    parser = km.PushMultipartParser(b"boundary", **kwargs)
    chunk_size = chunk_size or max(1, len(data))
    events = []
    with parser:
        for i in range(0, len(data), chunk_size):
            events.extend(parser.parse(data[i : i + chunk_size]))
    return events


def test_events() -> None:
    events = parse_events(form(("a", None, b"hello"), ("b", "b.txt", b"world")))
    assert [type(e).__name__ for e in events] == [
        "MultipartSegment",
        "bytes",
        "NoneType",
        "MultipartSegment",
        "bytes",
        "NoneType",
    ]
    a, b = events[0], events[3]
    assert isinstance(a, km.MultipartSegment) and isinstance(b, km.MultipartSegment)
    assert (a.name, a.filename, a.size, a.complete) == ("a", None, 5, True)
    assert (b.name, b.filename, b.size) == ("b", "b.txt", 5)
    assert events[1] == b"hello"
    assert events[4] == b"world"


def test_segment_api() -> None:
    data = (
        b"--boundary\r\n"
        b'Content-Disposition: form-data; name="a"\r\n'
        b"Content-Type: Text/Plain; charset=latin1\r\n"
        b"x-custom: value\r\n"
        b"\r\nxyz\r\n--boundary--"
    )
    segment = parse_events(data)[0]
    assert isinstance(segment, km.MultipartSegment)
    assert segment.headerlist == [
        ("Content-Disposition", 'form-data; name="a"'),
        ("Content-Type", "Text/Plain; charset=latin1"),
        ("X-Custom", "value"),
    ]
    assert segment.headerlist is segment.headerlist  # cached
    assert segment.disposition == "form-data"
    assert segment.content_type == "text/plain"
    assert segment.charset == "latin1"
    assert segment.header("X-CUSTOM") == "value"
    assert segment["x-custom"] == "value"
    assert segment.header("missing") is None
    assert segment.header("missing", 5) == 5
    with pytest.raises(KeyError):
        segment["missing"]


def test_size_while_incomplete() -> None:
    parser = km.PushMultipartParser(b"boundary")
    segment, data = parser.parse(
        b'--boundary\r\nContent-Disposition: form-data; name="a"\r\n\r\nxyz'
    )
    assert isinstance(segment, km.MultipartSegment)
    assert data == b"xyz"
    assert segment.bytes_received == 3
    assert not segment.complete
    with pytest.raises(km.ParserStateError):
        segment.size


def test_zero_copy_for_whole_chunks() -> None:
    parser = km.PushMultipartParser(b"boundary")
    parser.parse(b'--boundary\r\nContent-Disposition: form-data; name="a"\r\n\r\n')
    chunk = b"x" * 100_000
    assert parser.parse(chunk)[0] is chunk


def test_bytearray_input() -> None:
    events = km.PushMultipartParser(b"boundary").parse(bytearray(form(("a", None, b"x"))))
    assert events[1] == b"x"
    assert type(events[1]) is bytes


def test_invalid_chunk_type() -> None:
    parser = km.PushMultipartParser(b"boundary")
    with pytest.raises(km.ParserStateError):
        parser.parse("text")  # ty: ignore[invalid-argument-type]
    assert parser.closed
    assert isinstance(parser.error, km.ParserStateError)


def test_close_and_reuse() -> None:
    parser = km.PushMultipartParser(b"boundary")
    parser.parse(form(("a", None, b"x")))
    assert not parser.closed
    assert parser.parse(b"") == []
    assert parser.closed
    with pytest.raises(km.ParserStateError):
        parser.parse(b"more")


def test_incomplete_stream() -> None:
    with pytest.raises(km.ParserError, match="Unexpected end"):
        parse_events(form(("a", None, b"x"))[:-5])


def test_context_manager_skips_check_on_exception() -> None:
    with pytest.raises(KeyError):
        with km.PushMultipartParser(b"boundary") as parser:
            parser.parse(b"--boundary\r\n")
            raise KeyError()
    assert parser.closed
    assert parser.error is None


def test_exception_hierarchy() -> None:
    assert issubclass(km.MultipartError, ValueError)
    assert issubclass(km.StrictParserError, km.ParserError)
    assert km.ParserLimitReached.http_status == 413
    assert km.ParserError.__module__ == "kirimi"


def test_limits() -> None:
    data = form(("a", None, b"0123456789"), ("b", None, b"x"))
    with pytest.raises(km.ParserLimitReached, match="segment size"):
        parse_events(data, max_segment_size=9)
    with pytest.raises(km.ParserLimitReached, match="segment count"):
        parse_events(data, max_segment_count=1)
    with pytest.raises(km.ParserLimitReached, match="header length"):
        parse_events(data, max_header_size=10)
    with pytest.raises(km.ParserLimitReached, match="header count"):
        parse_events(data, max_header_count=0)
    with pytest.raises(km.ParserError, match="Content-Length limit"):
        parse_events(data, content_length=len(data) - 1)
    assert len(parse_events(data, max_segment_size=math.inf, max_segment_count=None)) == 6


def test_limit_attributes() -> None:
    parser = km.PushMultipartParser("boundary", max_segment_size=10)
    assert parser.boundary == b"boundary"
    assert parser.max_segment_size == 10
    assert parser.max_segment_count == math.inf
    assert parser.max_header_size == 4096 + 128


def test_strict_mode() -> None:
    with pytest.raises(km.StrictParserError):
        km.PushMultipartParser(b"x" * 1025, strict=True)
    # A preamble is only rejected if the first chunk has no boundary.
    preamble = b"preamble\r\n" * 10
    assert len(parse_events(preamble + form(("a", None, b"x")), strict=True)) == 3
    with pytest.raises(km.StrictParserError):
        parse_events(preamble + form(("a", None, b"x")), 20, strict=True)
    parser = km.PushMultipartParser(b"boundary", strict=True)
    parser.parse(form(("a", None, b"x")))
    with pytest.raises(km.StrictParserError):
        parser.parse(b"epilogue")


def test_charsets() -> None:
    data = form(("\xe4", None, b"x")).replace(b"\xc3\xa4", b"\xe4")
    segment = parse_events(data, header_charset="latin1")[0]
    assert isinstance(segment, km.MultipartSegment) and segment.name == "\xe4"
    with pytest.raises(km.ParserError, match="decode"):
        parse_events(data)
    with pytest.raises(km.ParserError, match="Invalid charset"):
        km.PushMultipartParser(b"boundary", header_charset="no-such-charset")


def test_parse_blocking() -> None:
    data = form(("a", None, b"x" * 100_000), ("b", None, b"y"))
    parser = km.PushMultipartParser(b"boundary", content_length=len(data))
    events = list(parser.parse_blocking(io.BytesIO(data + b"ignored").read, 1000))
    assert b"".join(e for e in events if type(e) is bytes) == b"x" * 100_000 + b"y"
    assert parser.closed


def test_parse_async() -> None:
    data = form(("a", None, b"x" * 100_000))

    async def run() -> list[Event]:
        stream = io.BytesIO(data)

        async def read(size: int) -> bytes:
            return stream.read(size)

        parser = km.PushMultipartParser(b"boundary")
        return [e async for e in parser.parse_async(read, 1000)]

    events = asyncio.run(run())
    assert b"".join(e for e in events if type(e) is bytes) == b"x" * 100_000


@pytest.mark.parametrize(
    "payload",
    [
        b"\r\n" * 50_000,
        b"\n" * 50_000,
        b"\r\n--boundar" * 5_000,
        bytes(range(256)) * 400,
    ],
    ids=["crlf", "lf", "bchar", "binary"],
)
@pytest.mark.parametrize("chunk_size", [1, 13, 4096, 1 << 20])
def test_worst_case_payloads(payload: bytes, chunk_size: int) -> None:
    if chunk_size == 1:
        payload = payload[:5000]
    events = parse_events(form(("f", "f.bin", payload)), chunk_size)
    assert b"".join(e for e in events if type(e) is bytes) == payload


def test_junk_around_boundaries() -> None:
    junk = bytes(range(32, 127)) * 10_000
    data = junk + b"\r\n" + form(("f", None, b"Content")) + junk
    events = parse_events(data, 4096)
    assert events[1] == b"Content"
