import asyncio
import io
import math

import pytest

import fast_multipart as fm


def form(*fields, boundary=b"boundary"):
    """Build a multipart body from (name, filename, data) tuples."""
    out = b""
    for name, filename, data in fields:
        out += b"--" + boundary + b"\r\n"
        out += b'Content-Disposition: form-data; name="%s"' % name.encode()
        if filename:
            out += b'; filename="%s"' % filename.encode()
        out += b"\r\n\r\n" + data + b"\r\n"
    return out + b"--" + boundary + b"--\r\n"


def parse_events(data, chunk_size=None, **kwargs):
    parser = fm.PushMultipartParser(b"boundary", **kwargs)
    chunk_size = chunk_size or max(1, len(data))
    events = []
    with parser:
        for i in range(0, len(data), chunk_size):
            events.extend(parser.parse(data[i : i + chunk_size]))
    return events


def test_events():
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
    assert (a.name, a.filename, a.size, a.complete) == ("a", None, 5, True)
    assert (b.name, b.filename, b.size) == ("b", "b.txt", 5)
    assert events[1] == b"hello"
    assert events[4] == b"world"


def test_segment_api():
    data = (
        b"--boundary\r\n"
        b'Content-Disposition: form-data; name="a"\r\n'
        b"Content-Type: Text/Plain; charset=latin1\r\n"
        b"x-custom: value\r\n"
        b"\r\nxyz\r\n--boundary--"
    )
    segment = parse_events(data)[0]
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


def test_size_while_incomplete():
    parser = fm.PushMultipartParser(b"boundary")
    segment, data = parser.parse(
        b'--boundary\r\nContent-Disposition: form-data; name="a"\r\n\r\nxyz'
    )
    assert data == b"xyz"
    assert segment.bytes_received == 3
    assert not segment.complete
    with pytest.raises(fm.ParserStateError):
        segment.size


def test_zero_copy_for_whole_chunks():
    parser = fm.PushMultipartParser(b"boundary")
    parser.parse(b'--boundary\r\nContent-Disposition: form-data; name="a"\r\n\r\n')
    chunk = b"x" * 100_000
    assert parser.parse(chunk)[0] is chunk


def test_bytearray_input():
    events = fm.PushMultipartParser(b"boundary").parse(bytearray(form(("a", None, b"x"))))
    assert events[1] == b"x"
    assert type(events[1]) is bytes


def test_invalid_chunk_type():
    parser = fm.PushMultipartParser(b"boundary")
    with pytest.raises(fm.ParserStateError):
        parser.parse("text")
    assert parser.closed
    assert isinstance(parser.error, fm.ParserStateError)


def test_close_and_reuse():
    parser = fm.PushMultipartParser(b"boundary")
    parser.parse(form(("a", None, b"x")))
    assert not parser.closed
    assert parser.parse(b"") == []
    assert parser.closed
    with pytest.raises(fm.ParserStateError):
        parser.parse(b"more")


def test_incomplete_stream():
    with pytest.raises(fm.ParserError, match="Unexpected end"):
        parse_events(form(("a", None, b"x"))[:-5])


def test_context_manager_skips_check_on_exception():
    with pytest.raises(KeyError):
        with fm.PushMultipartParser(b"boundary") as parser:
            parser.parse(b"--boundary\r\n")
            raise KeyError()
    assert parser.closed
    assert parser.error is None


def test_exception_hierarchy():
    assert issubclass(fm.MultipartError, ValueError)
    assert issubclass(fm.StrictParserError, fm.ParserError)
    assert fm.ParserLimitReached.http_status == 413
    assert fm.ParserError.__module__ == "fast_multipart"


def test_limits():
    data = form(("a", None, b"0123456789"), ("b", None, b"x"))
    with pytest.raises(fm.ParserLimitReached, match="segment size"):
        parse_events(data, max_segment_size=9)
    with pytest.raises(fm.ParserLimitReached, match="segment count"):
        parse_events(data, max_segment_count=1)
    with pytest.raises(fm.ParserLimitReached, match="header length"):
        parse_events(data, max_header_size=10)
    with pytest.raises(fm.ParserLimitReached, match="header count"):
        parse_events(data, max_header_count=0)
    with pytest.raises(fm.ParserError, match="Content-Length limit"):
        parse_events(data, content_length=len(data) - 1)
    assert len(parse_events(data, max_segment_size=math.inf, max_segment_count=None)) == 6


def test_limit_attributes():
    parser = fm.PushMultipartParser("boundary", max_segment_size=10)
    assert parser.boundary == b"boundary"
    assert parser.max_segment_size == 10
    assert parser.max_segment_count == math.inf
    assert parser.max_header_size == 4096 + 128


def test_strict_mode():
    with pytest.raises(fm.StrictParserError):
        fm.PushMultipartParser(b"x" * 1025, strict=True)
    # A preamble is only rejected if the first chunk has no boundary.
    preamble = b"preamble\r\n" * 10
    assert len(parse_events(preamble + form(("a", None, b"x")), strict=True)) == 3
    with pytest.raises(fm.StrictParserError):
        parse_events(preamble + form(("a", None, b"x")), 20, strict=True)
    parser = fm.PushMultipartParser(b"boundary", strict=True)
    parser.parse(form(("a", None, b"x")))
    with pytest.raises(fm.StrictParserError):
        parser.parse(b"epilogue")


def test_charsets():
    data = form(("\xe4", None, b"x")).replace(b"\xc3\xa4", b"\xe4")
    assert parse_events(data, header_charset="latin1")[0].name == "\xe4"
    with pytest.raises(fm.ParserError, match="decode"):
        parse_events(data)
    with pytest.raises(fm.ParserError, match="Invalid charset"):
        fm.PushMultipartParser(b"boundary", header_charset="no-such-charset")


def test_parse_blocking():
    data = form(("a", None, b"x" * 100_000), ("b", None, b"y"))
    parser = fm.PushMultipartParser(b"boundary", content_length=len(data))
    events = list(parser.parse_blocking(io.BytesIO(data + b"ignored").read, 1000))
    assert b"".join(e for e in events if type(e) is bytes) == b"x" * 100_000 + b"y"
    assert parser.closed


def test_parse_async():
    data = form(("a", None, b"x" * 100_000))

    async def run():
        stream = io.BytesIO(data)

        async def read(size):
            return stream.read(size)

        parser = fm.PushMultipartParser(b"boundary")
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
def test_worst_case_payloads(payload, chunk_size):
    if chunk_size == 1:
        payload = payload[:5000]
    events = parse_events(form(("f", "f.bin", payload)), chunk_size)
    assert b"".join(e for e in events if type(e) is bytes) == payload


def test_junk_around_boundaries():
    junk = bytes(range(32, 127)) * 10_000
    data = junk + b"\r\n" + form(("f", None, b"Content")) + junk
    events = parse_events(data, 4096)
    assert events[1] == b"Content"
