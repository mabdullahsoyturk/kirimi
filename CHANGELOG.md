# Changelog

All notable changes to kirimi are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/).

Entries are generated from the fragments in `changelog.d/` with
[towncrier](https://towncrier.readthedocs.io/); see
[CONTRIBUTING.md](CONTRIBUTING.md#changelog).

<!-- towncrier release notes start -->

## [0.1.0](https://github.com/mabdullahsoyturk/kirimi/releases/tag/v0.1.0) - 2026-10-26

### Added

- First release: a drop-in replacement for [multipart](https://github.com/defnull/multipart) 2.0 with a Rust core. It provides `PushMultipartParser`, `MultipartParser`, `parse_form_data` and the header helpers with the same API, and wheels for CPython 3.12–3.15, including free-threaded builds.
