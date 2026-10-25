"""Fast streaming parser for ``multipart/form-data``, written in Rust.

The API is compatible with the `multipart <https://github.com/defnull/multipart>`_
library: :class:`PushMultipartParser` is a non-blocking (sans-IO) parser,
:class:`MultipartParser` reads from blocking streams and buffers parts in
memory or on disk, and :func:`parse_form_data` handles WSGI environments.
"""

from ._fast_multipart import (
    MultipartError,
    MultipartSegment,
    ParserError,
    ParserLimitReached,
    ParserStateError,
    StrictParserError,
    content_disposition_unquote,
    header_unquote,
    parse_content_disposition,
)
from ._multipart import (
    MultiDict,
    MultipartParser,
    MultipartPart,
    PushMultipartParser,
    content_disposition_quote,
    header_quote,
    is_form_request,
    parse_form_data,
    parse_options_header,
)

__version__ = "0.1.0"
__all__ = [
    "MultipartError",
    "ParserLimitReached",
    "ParserError",
    "StrictParserError",
    "ParserStateError",
    "is_form_request",
    "parse_form_data",
    "parse_options_header",
    "parse_content_disposition",
    "header_quote",
    "header_unquote",
    "content_disposition_quote",
    "content_disposition_unquote",
    "MultiDict",
    "MultipartParser",
    "MultipartPart",
    "PushMultipartParser",
    "MultipartSegment",
]

#: Suitable HTTP status codes for each exception type
MultipartError.http_status = 500  # Internal Error
ParserError.http_status = 415  # Unsupported Media Type
StrictParserError.http_status = 415  # Unsupported Media Type
ParserLimitReached.http_status = 413  # Request Entity Too Large
ParserStateError.http_status = 500  # Internal Error
