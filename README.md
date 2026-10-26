# kirimi

A fast, streaming `multipart/form-data` parser for Python, written in Rust.
The name is Japanese for a slice cut from a whole fish (切り身), which is what
it does to request bodies.

kirimi is a drop-in replacement for
[multipart](https://github.com/defnull/multipart): it provides the same public
API with the same signatures and semantics, and is faster on every input,
including adversarial ones.

- **Fast:** faster than `multipart` in every benchmark scenario, up to 8×; see
  [Performance](#performance).
- **Streaming:** a sans-IO push parser, plus blocking, async and WSGI helpers.
- **Robust:** worst-case linear time, fuzzed, and tested against `multipart`
  with property-based differential tests.
- **Modern:** Python 3.12+, including free-threaded builds; fully typed.

## Installation

kirimi is not yet published on PyPI. Install it from source (requires a
Rust toolchain):

```sh
pip install git+https://github.com/mabdullahsoyturk/fast-multipart
```

## Usage

Parse a WSGI request into form fields and file uploads:

```python
from kirimi import parse_form_data

forms, files = parse_form_data(environ)
name = forms.get("name")                # str
upload = files.get("upload")            # MultipartPart
if upload is not None:
    upload.save_as("/srv/uploads/upload.bin")
```

Parse a stream part by part. Small parts stay in memory, large ones are spooled
to temporary files:

```python
from kirimi import MultipartParser

for part in MultipartParser(stream, boundary, content_length):
    print(part.name, part.filename, part.size, part.is_buffered())
```

Or drive the non-blocking parser yourself, for example in an async server:

```python
from kirimi import MultipartSegment, PushMultipartParser

with PushMultipartParser(boundary) as parser:
    while not parser.closed:
        for event in parser.parse(read(65536)):
            if isinstance(event, MultipartSegment):
                ...  # start of a part: event.name, event.filename, event.headerlist
            elif event:
                ...  # chunk of the part body (bytes)
            else:
                ...  # end of the part
```

`PushMultipartParser.parse_blocking()` and `parse_async()` wrap this loop for
blocking and async `read` functions. See the
[multipart documentation](https://multipart.readthedocs.io/) for the full API.

## Performance

Throughput relative to `multipart` 2.0.1 in
[multipart_bench](https://github.com/defnull/multipart_bench) (higher is
better):

| Scenario | Description | Non-blocking | Blocking |
|---|---|---:|---:|
| simple | Two small text fields | 4.96× | 7.94× |
| large | 100 small text fields | 5.50× | 7.05× |
| upload | One 32 MB file | 3.31× | 1.13× |
| mixed | Text fields and two files (1 MB, 2 MB) | 3.27× | 1.29× |
| worstcase_crlf | 1 MB of `\r\n` | 2.92× | 1.33× |
| worstcase_lf | 1 MB of `\n` | 3.36× | 1.36× |
| worstcase_bchar | 1 MB of near-boundaries | 2.49× | 1.47× |
| worstcase_junk | 1 MB of junk around the form | 4.36× | 4.82× |

The blocking results for large files are bounded by writes to temporary files.
Absolute numbers, the other parsers and the test setup are in
[bench/RESULTS.md](bench/RESULTS.md).

## Differences from `multipart`

These make the result independent of how the input is split into chunks:

- A bare LF in a part header line is always an error. `multipart` accepts it if
  a CRLF follows in the same chunk.
- A first boundary that does not follow a CRLF is always an error. `multipart`
  misses the byte in front of it if the boundary straddles two chunks.
- A header line of exactly `max_header_size` bytes is accepted. `multipart`
  rejects it if a chunk ends between its CR and LF.

Other differences:

- Boundaries containing CR are rejected (RFC 2046 does not allow them).
- `header_charset` supports UTF-8, Latin-1 and ASCII only.
- Non-ASCII header names are rejected.
- `parse()` returns a list of events instead of a generator, and raises errors
  before returning any events of the failing chunk.
- `MultipartParser` and `MultipartPart` are implemented in Rust. They have the
  same public attributes and methods and can be subclassed, but lack the
  private ones (`_segment`, `_done`, `_part_iter`, `_iterparse`). An unlimited
  `MultipartPart.memfile_limit` reads back as the largest 64-bit integer
  instead of `inf`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup, project
layout and design notes.

## License

MIT
