"""Blocking, async and WSGI helpers built on top of the Rust push parser.

Semantics and defaults follow the ``multipart`` library, so this module can be
used as a drop-in replacement.
"""

import codecs
import re
import tempfile
from collections.abc import MutableMapping
from io import BytesIO
from typing import AsyncGenerator, Awaitable, Callable, Generator, Union
from urllib.parse import unquote_plus

from ._fast_multipart import (
    MultipartError,
    MultipartParser,
    MultipartPart,
    MultipartSegment,
    ParserError,
    ParserLimitReached,
    StrictParserError,
    _parse_options_header,
    content_disposition_unquote,
    header_unquote,
)
from ._fast_multipart import PushMultipartParser as _PushMultipartParser

ParserEvent = Union[MultipartSegment, bytes, None]
BlockingReader = Callable[[int], bytes]
AsyncReader = Callable[[int], Awaitable[bytes]]

_re_token = re.compile(r"[a-zA-Z0-9-!#$%&'*+.^_`|~]+", re.ASCII)
_KNOWN_CHARSETS = frozenset(
    ("utf8", "utf-8", "ascii", "us-ascii", "latin1", "latin-1", "iso-8859-1")
)


def _is_valid_charset(charset) -> bool:
    if charset in _KNOWN_CHARSETS:
        return True
    try:
        codecs.lookup(charset)
        return True
    except (LookupError, TypeError):
        return False


##
### Header helpers
##


def header_quote(val: str) -> str:
    """Quote header option values if necessary.

    Note: This is NOT the way modern browsers quote field names or filenames
    in Content-Disposition headers. See :func:`content_disposition_quote`.
    """
    if _re_token.fullmatch(val):
        return val
    return '"' + val.replace("\\", "\\\\").replace('"', '\\"') + '"'


def content_disposition_quote(val: str) -> str:
    """Quote field names or filenames for Content-Disposition headers the same
    way modern browsers do it (see WHATWG HTML5 specification)."""
    val = val.replace("\r", "%0D").replace("\n", "%0A").replace('"', "%22")
    return '"' + val + '"'


def parse_options_header(header: str, options=None, unquote=header_unquote):
    """Parse Content-Type (or similar) headers into a primary value and an
    options-dict.

    For Content-Disposition headers, pass :func:`content_disposition_unquote`
    as ``unquote``.
    """
    if unquote is header_unquote:
        mode = 1
    elif unquote is content_disposition_unquote:
        mode = 2
    else:
        mode = 0

    value, pairs = _parse_options_header(header, mode)
    if pairs is None:
        return value, {}

    options = options or {}
    if mode:
        options.update(pairs)
    else:
        for key, val in pairs:
            options[key] = unquote(val, key == "filename")
    return value, options


class MultiDict(MutableMapping):
    """A dict that stores multiple values per key. Most dict methods return the
    last value by default. There are special methods to get all values.
    """

    def __init__(self, *args, **kwargs):
        self.dict = {}
        for arg in args:
            items = arg.items() if hasattr(arg, "items") else arg
            for key, value in items:
                self[key] = value
        for key, value in kwargs.items():
            self[key] = value

    def __len__(self):
        return len(self.dict)

    def __iter__(self):
        return iter(self.dict)

    def __contains__(self, key):
        return key in self.dict

    def __delitem__(self, key):
        del self.dict[key]

    def __str__(self):
        return str(self.dict)

    def __repr__(self):
        return repr(self.dict)

    def keys(self):
        return self.dict.keys()

    def __getitem__(self, key):
        return self.dict[key][-1]

    def __setitem__(self, key, value):
        self.append(key, value)

    def append(self, key, value):
        """Add an additional value to a key."""
        self.dict.setdefault(key, []).append(value)

    def replace(self, key, value):
        """Replace all values for a key with a single value."""
        self.dict[key] = [value]

    def getall(self, key):
        """Return a list with all values for a key. The list may be empty."""
        return self.dict.get(key) or []

    def get(self, key, default=None, index=-1):
        try:
            return self.dict[key][index]
        except (KeyError, IndexError):
            return default

    def iterallitems(self):
        """Yield (key, value) pairs with repeating keys for each value."""
        for key, values in self.dict.items():
            for value in values:
                yield key, value


##
### Push parser with IO helpers
##


class PushMultipartParser(_PushMultipartParser):
    """An incremental non-blocking parser for multipart/form-data.

    :meth:`parse` takes a chunk of input and returns a list of parser events:
    for each part a :class:`MultipartSegment`, followed by zero or more
    non-empty :class:`bytes` chunks of the part body, followed by ``None``.
    Parsing an empty chunk, calling :meth:`close` or leaving the ``with``
    context signals the end of input.

    Unlike ``multipart``, :meth:`parse` returns a list instead of a generator,
    so all events of a chunk are produced at once.
    """

    __slots__ = ()

    def parse_blocking(
        self, read: BlockingReader, chunk_size: int = 1024 * 64
    ) -> Generator[ParserEvent, None, None]:
        """Parse the entire multipart stream by reading chunks from a blocking
        ``read(size)`` function and yield parser events.

        The parser will not read more than :attr:`content_length` bytes, if
        known, and stops reading once the parser is closed.
        """
        assert chunk_size > 0
        parse = self.parse
        with self:
            readlimit = self.content_length
            while not self.closed:
                if readlimit >= 0:
                    chunk = read(min(chunk_size, readlimit))
                    readlimit -= len(chunk)
                else:
                    chunk = read(chunk_size)
                yield from parse(chunk)

    async def parse_async(
        self, read: AsyncReader, chunk_size: int = 1024 * 64
    ) -> AsyncGenerator[ParserEvent, None]:
        """Same as :meth:`parse_blocking`, but for an async ``read(size)``
        function such as :meth:`asyncio.StreamReader.read`."""
        assert chunk_size > 0
        parse = self.parse
        with self:
            readlimit = self.content_length
            while not self.closed:
                if readlimit >= 0:
                    chunk = await read(min(chunk_size, readlimit))
                    readlimit -= len(chunk)
                else:
                    chunk = await read(chunk_size)
                for event in parse(chunk):
                    yield event


##
### Buffered blocking parser
##


def copy_file(stream, target, maxread=-1, buffer_size=2**16):
    """Read from ``stream`` and write to ``target`` until ``maxread`` or EOF."""
    size, read = 0, stream.read
    while True:
        to_read = buffer_size if maxread < 0 else min(buffer_size, maxread - size)
        part = read(to_read)
        if not part:
            return size
        target.write(part)
        size += len(part)


#: Creates the temporary files that large parts are spooled to.
_temporary_file = tempfile.TemporaryFile


##
### WSGI
##


def is_form_request(environ) -> bool:
    """Return True if the environ represents a form request that can be
    parsed with :func:`parse_form_data`."""
    content_type = environ.get("CONTENT_TYPE", "")
    return content_type.split(";", 1)[0].strip().lower() in (
        "multipart/form-data",
        "application/x-www-form-urlencoded",
        "application/x-url-encoded",
    )


def parse_form_data(environ, charset="utf8", strict=False, ignore_errors=None, **kwargs):
    """Parse both types of form data (multipart and url-encoded) from a WSGI
    environment and return two :class:`MultiDict` instances, one for text
    form fields (strings) and one for file uploads (:class:`MultipartPart`
    instances). Text fields that are too big to fit into memory limits are
    treated as file uploads with no filename.

    :param environ: A WSGI environment dictionary. Only ``wsgi.input``,
        ``CONTENT_TYPE`` and ``CONTENT_LENGTH`` are used.
    :param charset: The default charset used to decode headers and text fields.
    :param strict: Enables additional format and sanity checks.
    :param ignore_errors: If True, suppress all exceptions. If False, raise
        them. None (default) raises in strict mode and suppresses otherwise.
    :param kwargs: Additional keyword arguments (e.g. limits) passed to
        :class:`MultipartParser`.
    """
    forms, files = MultiDict(), MultiDict()

    try:
        stream = environ.get("wsgi.input")
        if not stream:
            if strict:
                raise StrictParserError("No 'wsgi.input' in WSGI environment")
            stream = BytesIO()

        content_type = environ.get("CONTENT_TYPE", "")
        if not content_type:
            if strict:
                raise StrictParserError("Missing Content-Type header")
            return forms, files

        try:
            content_length = int(environ.get("CONTENT_LENGTH", -1))
        except ValueError:
            raise ParserError("Invalid Content-Length header")

        content_type, options = parse_options_header(content_type)
        kwargs["charset"] = charset = options.get("charset", charset)

        if not _is_valid_charset(charset):
            raise ParserError(f"Invalid charset: {charset!r}")

        if content_type == "multipart/form-data":
            boundary = options.get("boundary", "")
            if not boundary:
                raise ParserError("Missing boundary for multipart/form-data")

            for part in MultipartParser(
                stream, boundary, content_length, strict=strict, **kwargs
            ):
                if part.filename or not part.is_buffered():
                    files.append(part.name, part)
                else:
                    forms.append(part.name, part.value)
                    part.close()

        elif content_type in (
            "application/x-www-form-urlencoded",
            "application/x-url-encoded",
        ):
            mem_limit = kwargs.get("memory_limit", kwargs.get("mem_limit", 1024 * 64 * 128))
            part_limit = kwargs.get("part_limit", 128)

            if content_length > -1:
                if content_length > mem_limit:
                    raise ParserLimitReached("Memory limit exceeded")
                data = stream.read(min(mem_limit, content_length))
                if len(data) < content_length:
                    raise ParserError("Unexpected end of data stream")
            else:
                data = stream.read(mem_limit + 1)
                if len(data) > mem_limit:
                    raise ParserLimitReached("Memory limit exceeded")

            fields = data.decode(charset).split("&", part_limit)
            if len(fields) > part_limit:
                raise ParserLimitReached("Part limit exceeded")
            for field in fields:
                name, _, value = field.partition("=")
                if not name:
                    continue
                forms.append(
                    unquote_plus(name, encoding=charset, errors="replace"),
                    unquote_plus(value or "", encoding=charset, errors="replace"),
                )

        elif strict:
            raise StrictParserError("Unsupported Content-Type")

    except MultipartError:
        if ignore_errors is None:
            ignore_errors = not strict
        if not ignore_errors:
            for _, part in files.iterallitems():
                part.close()
            raise

    return forms, files
