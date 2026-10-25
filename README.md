# fast-multipart

A streaming `multipart/form-data` parser for Python with a Rust core. The API
is compatible with [multipart](https://github.com/defnull/multipart), and the
goal is to be faster than it on every input, including the worst cases.

```python
from fast_multipart import PushMultipartParser, MultipartSegment

with PushMultipartParser(boundary) as parser:
    while not parser.closed:
        for event in parser.parse(read(65536)):
            if isinstance(event, MultipartSegment):
                ...  # new part: event.name, event.filename, event.headerlist
            elif event:
                ...  # body chunk (bytes)
            else:
                ...  # end of part
```

`MultipartParser`, `MultipartPart`, `parse_form_data`, `parse_options_header`
and the other public names of `multipart` are available with the same
signatures.

## Layout

| Path | Contents |
|---|---|
| `crates/core` | Sans-IO parser in Rust (`Parser`, `Sink`), header helpers, SIMD delimiter search |
| `crates/python` | PyO3 bindings: `PushMultipartParser`, `MultipartSegment`, exceptions; `parts.rs` has `MultipartParser` and `MultipartPart` |
| `python/fast_multipart` | `parse_blocking`/`parse_async`, `parse_form_data`, header helpers |
| `tests` | API tests and Hypothesis differential tests against `multipart` |
| `fuzz` | cargo-fuzz targets (`parse`, `search`, `headers`), dictionary, seed corpus script |
| `bench` | `quick.py` (multipart_bench scenarios), `blocking.py` (blocking API, interleaved medians), `adversarial.py` (near-miss delimiters), `multipart_bench/` (runs the upstream benchmark with this parser added) |

## Development

```sh
uv venv --python 3.12 .venv
uv pip install --python .venv maturin pytest hypothesis multipart==2.0.1
VIRTUAL_ENV=$PWD/.venv .venv/Scripts/maturin develop --release --uv

cargo test -p fast-multipart-core --features fuzzing
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
.venv/Scripts/python -m pytest
```

Clippy runs with the `pedantic` group enabled (see `Cargo.toml`), and the core
crate forbids `unsafe` code.

### Fuzzing

The fuzz targets are thin wrappers around property checks in
`crates/core/src/fuzzing.rs`:

- `parse`: no panics, the same result whether the input arrives in one piece,
  in arbitrary chunks or byte by byte, and body data never contains the
  delimiter.
- `search`: the SIMD delimiter search agrees with a naive search.
- `headers`: the header helpers do not panic on arbitrary text.

`cargo test --features fuzzing` runs the same checks on generated inputs with
stable Rust. For real fuzzing (nightly Rust and `cargo install cargo-fuzz`):

```sh
cd fuzz
python make_corpus.py
cargo +nightly fuzz run parse -- -dict=multipart.dict -max_total_time=600
```

### multipart_bench

`bench/multipart_bench/run.py` clones
[multipart_bench](https://github.com/defnull/multipart_bench) at a pinned
commit, applies `fast-multipart.patch` (adapters and a README entry, ready to
submit upstream), installs the other parsers plus this package into a separate
venv, runs the benchmark and prints the result tables.

```sh
python bench/multipart_bench/run.py --profile fast            # all scenarios
python bench/multipart_bench/run.py "upload-*" "simple-*"      # a subset
```

The upstream Makefile pins the benchmark to one CPU core with real-time
priority on Linux. The runner does not, so results are noisier.

### Quick benchmarks

```sh
cd bench
../.venv/Scripts/python quick.py        # non-blocking and blocking, all scenarios
../.venv/Scripts/python blocking.py     # blocking only, interleaved medians
../.venv/Scripts/python adversarial.py  # near-miss delimiter worst cases
```

## Design notes

- **Batched events.** `parse()` returns a list of all events for a chunk
  instead of a generator, so each chunk crosses the Python/Rust boundary once.
- **Zero-copy bodies.** If a whole input chunk is body data, the input `bytes`
  object itself is returned. Other body chunks are copied once.
- **Linear-time delimiter search.** The body search scans for the delimiter's
  leading CR and its last byte with SIMD (memchr's `packedpair`), verifies
  candidates inline, and never falls back to a scalar search. Long
  verifications cannot overlap because the delimiter has a single CR, so the
  worst case stays linear. See `crates/core/src/search.rs`.
- **Cheap setup.** For small forms, creating the parser costs as much as
  parsing. Construction avoids precomputed search tables (the boundary
  finder for preambles is only built when the stream does not start with the
  boundary), and common charsets skip the `codecs` lookup. Creating a
  `PushMultipartParser` takes about 0.35 µs, a `MultipartParser` about
  0.45 µs.
- **Alignment.** memchr's SIMD finders need 32-byte alignment, but Python
  objects are 16-byte aligned, so the finders are boxed. Embedding them
  directly in the PyO3 class crashed about half of all runs.
- **Blocking parser in Rust.** `MultipartParser` and `MultipartPart` are
  Rust classes. The parser reads, parses and spools in one loop. In-memory parts keep ranges of the
  input chunks instead of copies, and only become a `BytesIO` when `.file` is
  accessed; `.raw` and `.value` are served from memory. Parts over the spool
  limit are written to a temporary file without further copies (partial
  chunks go out as `memoryview` slices). For spooled parts, temporary file
  creation and writes dominate, so the gain over `multipart` there is small.

## Differences from `multipart`

Intentional, so that results do not depend on how the input is split into
chunks:

- A bare LF in a segment header line is always an error. `multipart` accepts
  it if a CRLF follows in the same chunk.
- A first boundary that does not follow a CRLF is always an error.
  `multipart` misses the byte in front of it if the boundary straddles a chunk
  border.
- A header line exactly at `max_header_size` is accepted. `multipart` rejects
  it if the chunk ends between the line's CR and LF.

Other differences:

- Boundaries containing CR are rejected (RFC 2046 does not allow them, and
  they break partial-delimiter detection in `multipart`).
- `header_charset` supports UTF-8, Latin-1 and ASCII only.
- `parse()` raises errors before returning any events of the failing chunk.
- Non-ASCII header names are rejected. Python's `str.title()` maps a few
  exotic characters (such as `ß`) to ASCII, which is not replicated.
- `MultipartParser` and `MultipartPart` are Rust classes. They have the same
  public attributes and methods (and can be subclassed), but not the private
  ones (`_segment`, `_done`, `_part_iter`, `_iterparse`). An unlimited
  `MultipartPart.memfile_limit` reads back as the largest 64-bit integer
  instead of `inf`.
