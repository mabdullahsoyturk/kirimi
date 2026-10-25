import io

import multipart
import pytest

import fast_multipart as fm

BOUNDARY = "boundary"


def form(*fields):
    out = b""
    for name, filename, data in fields:
        out += b"--boundary\r\n"
        out += b'Content-Disposition: form-data; name="%s"' % name.encode()
        if filename:
            out += b'; filename="%s"' % filename.encode()
        out += b"\r\n\r\n" + data + b"\r\n"
    return out + b"--boundary--\r\n"


def summarize(parts):
    return [
        (p.name, p.filename, p.content_type, p.charset, p.size, p.is_buffered(), p.raw, p.headerlist)
        for p in parts
    ]


@pytest.mark.parametrize("buffer_size", [7, 1024, 65536])
def test_multipart_parser_matches_reference(buffer_size):
    data = form(
        ("a", None, b"short"),
        ("big", "big.bin", b"x" * 200_000),
        ("b", None, "\xe4€".encode()),
    )
    kwargs = dict(buffer_size=buffer_size, spool_limit=100_000)
    ours = fm.MultipartParser(io.BytesIO(data), BOUNDARY, **kwargs)
    theirs = multipart.MultipartParser(io.BytesIO(data), BOUNDARY, **kwargs)
    assert summarize(ours) == summarize(theirs)
    assert not ours.get("big").is_buffered()
    assert ours.get("b").value == "\xe4€"
    assert [p.name for p in ours.get_all("a")] == ["a"]


def test_memory_and_disk_limits():
    data = form(("a", None, b"x" * 1000), ("b", None, b"y" * 1000))
    with pytest.raises(fm.ParserLimitReached, match="Memory limit"):
        list(fm.MultipartParser(io.BytesIO(data), BOUNDARY, memory_limit=1500))
    with pytest.raises(fm.ParserLimitReached, match="Disk limit"):
        list(fm.MultipartParser(io.BytesIO(data), BOUNDARY, spool_limit=10, disk_limit=1500))
    with pytest.raises(fm.ParserLimitReached, match="segment count"):
        list(fm.MultipartParser(io.BytesIO(data), BOUNDARY, part_limit=1))


def test_save_as_and_close(tmp_path):
    data = form(("f", "f.txt", b"content"))
    part = fm.MultipartParser(io.BytesIO(data), BOUNDARY).get("f")
    assert part.save_as(tmp_path / "out") == 7
    assert (tmp_path / "out").read_bytes() == b"content"
    assert part.headers["Content-Disposition"] == 'form-data; name="f"; filename="f.txt"'
    assert part.disposition == 'form-data; name="f"; filename="f.txt"'
    assert part.content_type == "application/octet-stream"
    part.close()
    with pytest.raises(fm.MultipartError):
        part.raw


def environ(content_type, body):
    return {
        "CONTENT_TYPE": content_type,
        "CONTENT_LENGTH": str(len(body)),
        "wsgi.input": io.BytesIO(body),
    }


@pytest.mark.parametrize(
    "content_type, body",
    [
        ("multipart/form-data; boundary=boundary", form(("a", None, b"1"), ("f", "x.txt", b"2"))),
        ('multipart/form-data; boundary="boundary"; charset=latin1', form(("a", None, b"\xe4"))),
        ("application/x-www-form-urlencoded", b"a=1&b=%C3%A4&a=+3&&=x"),
        ("multipart/form-data; boundary=boundary", b"broken"),
        ("text/plain", b"ignored"),
    ],
)
def test_parse_form_data_matches_reference(content_type, body):
    forms, files = fm.parse_form_data(environ(content_type, body))
    ref_forms, ref_files = multipart.parse_form_data(environ(content_type, body))
    assert forms.dict == ref_forms.dict
    assert {k: [(p.filename, p.raw) for p in v] for k, v in files.dict.items()} == {
        k: [(p.filename, p.raw) for p in v] for k, v in ref_files.dict.items()
    }


def test_parse_form_data_strict_raises():
    with pytest.raises(fm.ParserError):
        fm.parse_form_data(
            environ("multipart/form-data; boundary=boundary", b"broken"), strict=True
        )
    with pytest.raises(fm.StrictParserError):
        fm.parse_form_data(environ("text/plain", b""), strict=True)


def test_is_form_request():
    assert fm.is_form_request({"CONTENT_TYPE": "Multipart/Form-Data; boundary=x"})
    assert not fm.is_form_request({"CONTENT_TYPE": "text/plain"})


def test_multidict():
    md = fm.MultiDict(a=1)
    md["a"] = 2
    md.append("b", 3)
    assert md["a"] == 2
    assert md.getall("a") == [1, 2]
    assert md.get("a", index=0) == 1
    assert list(md.iterallitems()) == [("a", 1), ("a", 2), ("b", 3)]
    md.replace("a", 5)
    assert md.getall("a") == [5]


def test_quote_helpers():
    assert fm.header_quote("token") == "token"
    assert fm.header_quote('a "b"') == '"a \\"b\\""'
    assert fm.content_disposition_quote('a"\r\n') == '"a%22%0D%0A"'


# MultipartPart (implemented in Rust)


class ChunkedReader:
    """A stream that returns at most `size` bytes per read, of a given type."""

    def __init__(self, data, size, kind=bytes):
        self.stream = io.BytesIO(data)
        self.size = size
        self.kind = kind

    def read(self, n):
        return self.kind(self.stream.read(min(n, self.size)))


@pytest.mark.parametrize(
    "payload",
    [b"x" * 300_000, b"\r\n" * 150_000, b"\r\n--boundar" * 30_000, bytes(range(256)) * 1200],
    ids=["plain", "crlf", "bchar", "binary"],
)
@pytest.mark.parametrize("kind", [bytes, bytearray])
@pytest.mark.parametrize("read_size", [1000, 65536])
def test_spooled_content(payload, kind, read_size):
    data = form(("small", None, b"value"), ("big", "big.bin", payload), ("tail", None, b"t"))
    stream = ChunkedReader(data, read_size, kind)
    parts = fm.MultipartParser(stream, BOUNDARY, spool_limit=100_000, buffer_size=65536).parts()
    small, big, tail = parts
    assert small.is_buffered() and small.raw == b"value"
    assert not big.is_buffered()
    assert big.size == len(payload)
    assert big.file.tell() == 0
    assert big.raw == payload
    assert tail.value == "t"


def test_in_memory_part_file_is_lazy():
    part = fm.MultipartParser(io.BytesIO(form(("a", None, b"hello"))), BOUNDARY).get("a")
    assert part.raw == b"hello"
    assert part.raw is part.raw  # served from one cached bytes object
    assert part.is_buffered()
    file = part.file
    assert isinstance(file, io.BytesIO)
    assert part.file is file
    assert file.read() == b"hello"
    assert part.raw == b"hello"  # position is restored
    assert file.tell() == 5
    file.write(b" world")
    assert part.raw == b"hello world"


def test_part_attributes_and_headers():
    data = (
        b"--boundary\r\n"
        b'Content-Disposition: form-data; name="f"; filename="a.txt"\r\n'
        b"Content-Type: text/plain; charset=latin1\r\n\r\n"
        b"\xe4\r\n--boundary--"
    )
    part = fm.MultipartParser(io.BytesIO(data), BOUNDARY).get("f")
    assert (part.name, part.filename, part.charset, part.size) == ("f", "a.txt", "latin1", 1)
    assert part.value == "\xe4"
    assert part.content_type == "text/plain"
    assert part.headers["content-type"] == "text/plain; charset=latin1"
    assert part.headers is part.headers
    assert part.headerlist[0] == ("Content-Disposition", 'form-data; name="f"; filename="a.txt"')
    part.charset = "utf8"
    with pytest.raises(UnicodeDecodeError):
        part.value
    part.custom = 1  # instances have a __dict__
    assert "name='f'" in repr(part)


def test_close_and_save_as(tmp_path):
    data = form(("a", None, b"x" * 10), ("b", "b.bin", b"y" * 1000))
    a, b = fm.MultipartParser(io.BytesIO(data), BOUNDARY, spool_limit=100).parts()
    assert a.save_as(tmp_path / "a") == 10
    assert b.save_as(tmp_path / "b") == 1000
    assert (tmp_path / "a").read_bytes() == b"x" * 10
    assert (tmp_path / "b").read_bytes() == b"y" * 1000
    b.close()
    a.close()
    assert a.file is None and b.file is None
    assert not a.is_buffered()
    for part in (a, b):
        with pytest.raises(fm.MultipartError):
            part.raw
        with pytest.raises(fm.MultipartError):
            part.save_as(tmp_path / "c")
    a.close()  # closing twice is fine


def test_tempfile_only_for_spooled_parts(monkeypatch):
    from fast_multipart import _multipart

    created = []

    def factory():
        created.append(io.BytesIO())
        return created[-1]

    monkeypatch.setattr(_multipart, "_temporary_file", factory)
    data = form(("a", None, b"x" * 10), ("b", None, b"y" * 1000))
    a, b = fm.MultipartParser(io.BytesIO(data), BOUNDARY, spool_limit=100).parts()
    assert len(created) == 1
    assert b.file is created[0]
    assert b.raw == b"y" * 1000


def test_limit_errors_stop_iteration():
    data = form(("a", None, b"x" * 1000), ("b", None, b"y" * 1000))
    parser = fm.MultipartParser(io.BytesIO(data), BOUNDARY, memory_limit=1500)
    seen = []
    with pytest.raises(fm.ParserLimitReached):
        for part in parser:
            seen.append(part.name)
    assert seen == ["a"]
    assert [p.name for p in parser] == ["a"]  # cached parts, no new errors


def test_invalid_chunk_type():
    with pytest.raises(fm.ParserStateError):
        list(fm.MultipartParser(ChunkedReader(form(), 10, str), BOUNDARY))


def test_multipart_part_compat_constructor():
    segment = fm.PushMultipartParser(b"boundary").parse(form(("a", "f.txt", b"")))[0]
    part = fm.MultipartPart(segment, memfile_limit=4, charset="latin1")
    part._write(b"ab")
    part._write(bytearray(b"cd"))
    assert part.is_buffered()
    part._write(b"ef")
    assert not part.is_buffered()
    part._mark_complete()
    assert (part.name, part.filename, part.size, part.raw) == ("a", "f.txt", 6, b"abcdef")


# MultipartParser (implemented in Rust)


def test_parser_is_lazy():
    data = form(("a", None, b"1"), ("b", None, b"2"), ("c", None, b"3"))
    stream = ChunkedReader(data, 60)
    parser = fm.MultipartParser(stream, BOUNDARY, buffer_size=60)
    assert stream.stream.tell() == 0  # nothing read before iterating
    assert parser.get("a").value == "1"
    assert stream.stream.tell() < len(data)  # stopped early
    assert parser.get("missing", 5) == 5
    assert stream.stream.tell() == len(data)


def test_parser_iteration_is_cached_and_shared():
    data = form(("a", None, b"1"), ("b", None, b"2"), ("a", None, b"3"))
    parser = fm.MultipartParser(ChunkedReader(data, 50), BOUNDARY, buffer_size=50)
    first = iter(parser)
    a = next(first)
    second = iter(parser)
    assert next(second) is a  # replays parsed parts
    b = next(second)  # parses a new part
    assert next(first) is b  # visible to the other iterator
    assert [p.value for p in parser] == ["1", "2", "3"]
    assert parser.parts() == list(parser)
    assert [p.value for p in parser.get_all("a")] == ["1", "3"]


@pytest.mark.parametrize("module", [fm, multipart], ids=["fast_multipart", "multipart"])
def test_parser_errors_surface_when_iterating(module):
    parser = module.MultipartParser(io.BytesIO(b""), "")  # invalid boundary
    with pytest.raises(module.MultipartError):
        list(parser)


@pytest.mark.parametrize(
    "kwargs",
    [
        {},
        {"mem_limit": 1000},
        {"memfile_limit": 10, "spool_limit": 100},
        {"spool_limit": 100, "memory_limit": 50},
        {"mem_limit": 0, "memfile_limit": 0},
        {"partsize_limit": None, "disk_limit": float("inf")},
    ],
)
def test_parser_limits_match_reference(kwargs):
    ours = fm.MultipartParser(io.BytesIO(), BOUNDARY, **kwargs)
    ref = multipart.MultipartParser(io.BytesIO(), BOUNDARY, **kwargs)
    for name in ("spool_limit", "memory_limit", "disk_limit", "part_limit", "partsize_limit"):
        expected = getattr(ref, name)
        assert getattr(ours, name) == (float("inf") if expected is None else expected), name


def test_parser_attributes():
    stream = io.BytesIO(form(("a", None, b"x")))
    parser = fm.MultipartParser(stream, BOUNDARY, charset="latin1", buffer_size=7)
    assert (parser.stream, parser.boundary, parser.charset, parser.buffer_size) == (
        stream,
        BOUNDARY,
        "latin1",
        7,
    )
    parser.part_limit = 0
    assert parser.part_limit == 0
    with pytest.raises(fm.ParserLimitReached):
        list(parser)


def test_parser_subclass():
    class Parser(fm.MultipartParser):
        def names(self):
            return [part.name for part in self]

    parser = Parser(io.BytesIO(form(("a", None, b"x"), ("b", None, b"y"))), BOUNDARY)
    parser.extra = 1
    assert parser.names() == ["a", "b"]
