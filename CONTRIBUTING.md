# Contributing

## Setup

You need stable Rust and [uv](https://docs.astral.sh/uv/).

```sh
uv sync                # create .venv and build the extension (release mode)
uv run prek install    # run the git hooks on every commit
```

`uv run` rebuilds the extension when Rust sources change (see `cache-keys` in
`pyproject.toml`). Dependencies are locked in `uv.lock` and `Cargo.lock`. The
package version is set in `Cargo.toml` only.

## Checks

```sh
uv run pytest
uv run prek run --all-files        # ruff check, ruff format, ty, detect-secrets
cargo test -p kirimi-core --features fuzzing
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
```

- **Rust:** Clippy runs with the `pedantic` group, and the core crate forbids
  `unsafe` code.
- **Python:** all code is annotated (ruff's `ANN` rules) and type-checked with
  [ty](https://github.com/astral-sh/ty), including the stubs of the Rust
  extension in `python/kirimi/_kirimi.pyi`. Avoid `Any`.
- **Hooks:** configured in `.pre-commit-config.yaml` and run with
  [prek](https://github.com/j178/prek). detect-secrets compares against
  `.secrets.baseline`; after reviewing a new finding, update the baseline with
  `uvx detect-secrets scan --baseline .secrets.baseline`.

## Layout

| Path | Contents |
|---|---|
| `crates/core` | Sans-IO parser (`Parser`, `Sink`), header helpers, SIMD delimiter search |
| `crates/python` | PyO3 bindings; `parts.rs` implements `MultipartParser` and `MultipartPart` |
| `python/kirimi` | Python package: `parse_blocking`/`parse_async`, `parse_form_data`, header helpers, type stubs |
| `tests` | API tests and Hypothesis differential tests against `multipart` |
| `fuzz` | cargo-fuzz targets, dictionary and seed corpus script |
| `bench` | Benchmark scripts and the multipart_bench runner |

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
  parsing. Construction avoids precomputed search tables (the boundary finder
  for preambles is only built when the stream does not start with the
  boundary), and common charsets skip the `codecs` lookup. Creating a
  `PushMultipartParser` takes about 0.35 µs, a `MultipartParser` about 0.45 µs.
- **Alignment.** memchr's SIMD finders need 32-byte alignment, but Python
  objects are 16-byte aligned, so the finders are boxed. Embedding them
  directly in the PyO3 class crashed about half of all runs.
- **Blocking parser in Rust.** `MultipartParser` reads, parses and spools in
  one loop. In-memory parts keep ranges of the input chunks instead of copies
  and only become a `BytesIO` when `.file` is accessed; `.raw` and `.value`
  are served from memory. Parts over the spool limit are written to a
  temporary file without further copies (partial chunks go out as
  `memoryview` slices). For spooled parts, temporary file writes dominate, so
  the gain over `multipart` there is small.

## Fuzzing

The fuzz targets are thin wrappers around property checks in
`crates/core/src/fuzzing.rs`:

- `parse`: no panics; the same result whether the input arrives in one piece,
  in arbitrary chunks or byte by byte; body data never contains the delimiter.
- `search`: the SIMD delimiter search agrees with a naive search.
- `headers`: the header helpers do not panic on arbitrary text.

`cargo test --features fuzzing` runs the same checks on generated inputs with
stable Rust. For real fuzzing, install nightly Rust and `cargo-fuzz`:

```sh
cd fuzz
python make_corpus.py
cargo +nightly fuzz run parse -- -dict=multipart.dict -max_total_time=600
```

## Benchmarks

Quick comparisons against `multipart`:

```sh
uv run bench/quick.py        # all scenarios, non-blocking and blocking
uv run bench/blocking.py     # blocking only, interleaved medians
uv run bench/adversarial.py  # near-miss delimiter worst cases
```

For publishable numbers, `bench/multipart_bench/run.py` clones
[multipart_bench](https://github.com/defnull/multipart_bench) at a pinned
commit, applies `kirimi.patch` (adapters and a README entry, ready to
submit upstream), installs all parsers into a separate venv and prints the
result tables:

```sh
python bench/multipart_bench/run.py --profile default     # all scenarios
python bench/multipart_bench/run.py "upload-*" "simple-*"  # a subset
```

The runner does not pin the benchmark to a CPU core. See
[bench/RESULTS.md](bench/RESULTS.md) for how the published results were
measured.

## CI and releases

Workflows in `.github/workflows`:

- `ci.yml` (pushes and pull requests): Rust formatting, Clippy and tests; the
  prek hooks and a `uv.lock` check; pytest on Linux, macOS and Windows with
  Python 3.12, 3.15 and free-threaded 3.15.
- `fuzz.yml` (weekly, or manually): 10 minutes per fuzz target; crash inputs
  are uploaded as artifacts.
- `release.yml` (`v*` tags, or manually to build only): wheels for Linux
  (manylinux, musllinux), macOS and Windows on x86-64 and ARM64, and the
  sdist. On a tag matching the version, publishes the wheels to PyPI with
  trusted publishing. The sdist is not published, so pip never builds kirimi
  from source.

Actions are pinned to commit hashes. Dependabot keeps them, the Cargo crates
and `uv.lock` up to date.

### Releasing

1. Bump `version` in `Cargo.toml` (the only place it is set) and commit.
2. Optionally run the `Release` workflow manually to check that all wheels
   build; it does not publish.
3. Push a matching tag, for example `git tag v0.2.0 && git push origin v0.2.0`.
   The workflow checks the tag against the version, builds the wheels and
   publishes them after the `pypi` environment's protection rules pass.

Publishing uses PyPI trusted publishing, so no API token is needed. The
publisher is configured on PyPI for owner `mabdullahsoyturk`, repository
`kirimi`, workflow `release.yml` and environment `pypi`.
