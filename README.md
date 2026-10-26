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

## Benchmark results

[multipart_bench](https://github.com/defnull/multipart_bench) (pinned commit,
`--profile default`), run with `bench/multipart_bench/run.py`. Throughput in
MB/s, percentages relative to `multipart`:

| Scenario | Non-blocking: multipart | Non-blocking: fast-multipart | Blocking: multipart | Blocking: fast-multipart |
|---|---:|---:|---:|---:|
| simple | 33.08 | 163.99 (496%) | 19.59 | 155.56 (794%) |
| large | 45.93 | 252.73 (550%) | 30.29 | 213.63 (705%) |
| upload | 5462.83 | 18083.24 (331%) | 2090.90 | 2352.74 (113%) |
| mixed | 5192.02 | 16965.95 (327%) | 1846.99 | 2381.70 (129%) |
| worstcase_crlf | 3608.83 | 10538.54 (292%) | 1604.44 | 2136.16 (133%) |
| worstcase_lf | 5174.39 | 17376.41 (336%) | 1684.13 | 2295.61 (136%) |
| worstcase_bchar | 4223.66 | 10514.55 (249%) | 1461.62 | 2147.65 (147%) |
| worstcase_junk | 4147.54 | 18067.19 (436%) | 3899.30 | 18802.76 (482%) |

Setup: Intel Core i7-1260P, Linux 7.0, Python 3.12.12. The benchmark was pinned
to one P-core thread (its sibling idle and both isolated from other processes
with cgroup cpusets) with `nice -n -20` and `chrt -f 99`, as in the upstream
Makefile, after `pyperf system tune` (turbo off, 2.1 GHz). Parser versions:
multipart 2.0.1, python-multipart 0.0.32, werkzeug 3.1.9, django 6.1.2,
streaming-form-data 2.1.0, emmett-core 1.4.6.

<details>
<summary>All parsers</summary>

#### simple

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 33.08 MB/s (100%) | 19.59 MB/s (100%) |
| fast-multipart | 163.99 MB/s (496%) | 155.56 MB/s (794%) |
| python-multipart | 9.33 MB/s (28%) | 5.41 MB/s (28%) |
| werkzeug | 6.45 MB/s (19%) | 4.85 MB/s (25%) |
| django | - | 2.01 MB/s (10%) |
| streaming-form-data | 0.65 MB/s (2%) | 0.62 MB/s (3%) |
| cgi | - | 3.97 MB/s (20%) |
| email | 3.54 MB/s (11%) | 3.02 MB/s (15%) |
| emmett-core | - | 69.89 MB/s (357%) |

#### large

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 45.93 MB/s (100%) | 30.29 MB/s (100%) |
| fast-multipart | 252.73 MB/s (550%) | 213.63 MB/s (705%) |
| python-multipart | 16.30 MB/s (35%) | 9.98 MB/s (33%) |
| werkzeug | 9.45 MB/s (21%) | 7.86 MB/s (26%) |
| django | - | 4.07 MB/s (13%) |
| streaming-form-data | 0.94 MB/s (2%) | 0.91 MB/s (3%) |
| cgi | - | 6.19 MB/s (20%) |
| email | 10.11 MB/s (22%) | 8.71 MB/s (29%) |
| emmett-core | - | 124.60 MB/s (411%) |

#### upload

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 5462.83 MB/s (100%) | 2090.90 MB/s (100%) |
| fast-multipart | 18083.24 MB/s (331%) | 2352.74 MB/s (113%) |
| python-multipart | 4669.09 MB/s (85%) | 1830.61 MB/s (88%) |
| werkzeug | 1492.12 MB/s (27%) | 955.56 MB/s (46%) |
| django | - | 1003.83 MB/s (48%) |
| streaming-form-data | 3585.16 MB/s (66%) | 1508.79 MB/s (72%) |
| cgi | - | 132.51 MB/s (6%) |
| email | 55.37 MB/s (1%) | 48.86 MB/s (2%) |
| emmett-core | - | 2358.91 MB/s (113%) |

#### mixed

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 5192.02 MB/s (100%) | 1846.99 MB/s (100%) |
| fast-multipart | 16965.95 MB/s (327%) | 2381.70 MB/s (129%) |
| python-multipart | 4276.13 MB/s (82%) | 1451.00 MB/s (79%) |
| werkzeug | 1393.27 MB/s (27%) | 863.74 MB/s (47%) |
| django | - | 859.80 MB/s (47%) |
| streaming-form-data | 1700.68 MB/s (33%) | 957.13 MB/s (52%) |
| cgi | - | 131.64 MB/s (7%) |
| email | 60.56 MB/s (1%) | 57.74 MB/s (3%) |
| emmett-core | - | 2432.05 MB/s (132%) |

#### worstcase_crlf

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 3608.83 MB/s (100%) | 1604.44 MB/s (100%) |
| fast-multipart | 10538.54 MB/s (292%) | 2136.16 MB/s (133%) |
| python-multipart | 3832.52 MB/s (106%) | 1233.42 MB/s (77%) |
| werkzeug | 2309.88 MB/s (64%) | 1112.44 MB/s (69%) |
| django | - | 1086.94 MB/s (68%) |
| streaming-form-data | 28.88 MB/s (1%) | 27.95 MB/s (2%) |
| cgi | - | 3.68 MB/s (0%) |
| email | 3.77 MB/s (0%) | 3.75 MB/s (0%) |
| emmett-core | - | 2404.19 MB/s (150%) |

#### worstcase_lf

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 5174.39 MB/s (100%) | 1684.13 MB/s (100%) |
| fast-multipart | 17376.41 MB/s (336%) | 2295.61 MB/s (136%) |
| python-multipart | 4080.25 MB/s (79%) | 1371.43 MB/s (81%) |
| werkzeug | 2329.04 MB/s (45%) | 1101.05 MB/s (65%) |
| django | - | 1101.48 MB/s (65%) |
| streaming-form-data | 1579.51 MB/s (31%) | 886.25 MB/s (53%) |
| cgi | - | 1.69 MB/s (0%) |
| email | 2.37 MB/s (0%) | 2.35 MB/s (0%) |
| emmett-core | - | 2391.56 MB/s (142%) |

#### worstcase_bchar

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 4223.66 MB/s (100%) | 1461.62 MB/s (100%) |
| fast-multipart | 10514.55 MB/s (249%) | 2147.65 MB/s (147%) |
| python-multipart | 2532.60 MB/s (60%) | 1042.79 MB/s (71%) |
| werkzeug | 1808.36 MB/s (43%) | 992.51 MB/s (68%) |
| django | - | 958.56 MB/s (66%) |
| streaming-form-data | 29.75 MB/s (1%) | 29.18 MB/s (2%) |
| cgi | - | 69.01 MB/s (5%) |
| email | 49.35 MB/s (1%) | 47.76 MB/s (3%) |
| emmett-core | - | 1373.61 MB/s (94%) |

#### worstcase_junk

| Parser | Non-Blocking (MB/s) | Blocking (MB/s) |
|--------|---------------------|-----------------|
| multipart | 4147.54 MB/s (100%) | 3899.30 MB/s (100%) |
| fast-multipart | 18067.19 MB/s (436%) | 18802.76 MB/s (482%) |
| python-multipart | *failed* | *failed* |
| werkzeug | 49.10 MB/s (1%) | 48.94 MB/s (1%) |
| django | - | *failed* |
| streaming-form-data | *failed* | *failed* |
| cgi | - | 1120.13 MB/s (29%) |
| email | 114.57 MB/s (3%) | 114.53 MB/s (3%) |
| emmett-core | - | 19177.70 MB/s (492%) |

</details>

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
