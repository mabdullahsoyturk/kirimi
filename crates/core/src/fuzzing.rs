//! Property checks used by the fuzz targets in `fuzz/`, and by a smoke test
//! that runs them on stable Rust.
//!
//! Each check takes raw fuzzer input and panics if a property is violated.
//! Only available with the `fuzzing` feature.

use crate::headers::normalize_header_name;
use crate::search::DelimiterFinder;
use crate::{
    Config, Error, Parser, Segment, Sink, content_disposition_unquote, header_unquote,
    parse_content_disposition, parse_options_header,
};

/// Boundaries to choose from. Fuzzers find structure much faster with a
/// fixed set (and a dictionary) than with arbitrary boundaries.
const BOUNDARIES: [&[u8]; 4] = [
    b"b",
    b"boundary",
    b"----WebKitFormBoundary7MA4YWxkTrZu0gW",
    b"------------------------WqclBHaXe8KIsoSum4zfZ6-abcdefghijklmnopqrstuvwxyz",
];

#[derive(Debug, PartialEq)]
struct Part {
    segment: Segment,
    data: Vec<u8>,
    size: Option<u64>,
}

#[derive(Default)]
struct Collect {
    parts: Vec<Part>,
}

impl Sink for Collect {
    fn segment(&mut self, segment: Segment) {
        if let Some(last) = self.parts.last() {
            assert!(
                last.size.is_some(),
                "segment before end of previous segment"
            );
        }
        self.parts.push(Part {
            segment,
            data: Vec::new(),
            size: None,
        });
    }

    fn data(&mut self, data: &[u8]) {
        assert!(!data.is_empty(), "empty data event");
        let part = self.parts.last_mut().expect("data before segment");
        assert!(part.size.is_none(), "data after segment end");
        part.data.extend_from_slice(data);
    }

    fn segment_end(&mut self, size: u64) {
        let part = self.parts.last_mut().expect("end before segment");
        assert!(part.size.is_none(), "segment ended twice");
        assert_eq!(part.data.len() as u64, size, "reported size");
        part.size = Some(size);
    }
}

fn parse(
    boundary: &[u8],
    config: Config,
    body: &[u8],
    sizes: &[usize],
) -> Result<Vec<Part>, Error> {
    let mut parser = Parser::new(boundary, config)?;
    let mut sink = Collect::default();
    let mut rest = body;
    let mut i = 0;
    while !rest.is_empty() {
        let size = sizes
            .get(i % sizes.len().max(1))
            .copied()
            .unwrap_or(rest.len());
        let (chunk, tail) = rest.split_at(size.clamp(1, rest.len()));
        parser.feed(chunk, &mut sink)?;
        rest = tail;
        i += 1;
    }
    parser.close(true)?;
    Ok(sink.parts)
}

/// Parse `data` with a boundary, limits and chunk sizes chosen by its first
/// bytes, and check:
///
/// - The parser does not panic, and reports well-formed events.
/// - Parsing in chunks succeeds exactly when parsing all at once does, with
///   the same result.
/// - Body data never contains the delimiter.
///
/// Errors may differ between chunkings: e.g. an overlong header line with a
/// bare LF hits either the LF check or the length limit first.
///
/// # Panics
///
/// If a property is violated.
pub fn check_parse(data: &[u8]) {
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let (sizes, body) = rest.split_at(rest.len().min(3));
    let sizes: Vec<usize> = sizes.iter().map(|&s| usize::from(s).max(1)).collect();

    let boundary = BOUNDARIES[usize::from(flags & 3)];
    let config = if flags & 4 == 0 {
        Config::default()
    } else {
        Config {
            max_header_size: 40,
            max_header_count: 2,
            max_segment_size: Some(16),
            max_segment_count: Some(3),
            ..Config::default()
        }
    };

    let whole = parse(boundary, config.clone(), body, &[]);
    let chunked = parse(boundary, config.clone(), body, &sizes);
    let one_by_one = parse(boundary, config.clone(), body, &[1]);
    match (&whole, &chunked, &one_by_one) {
        (Ok(a), Ok(b), Ok(c)) => {
            assert_eq!(a, b, "chunked result differs");
            assert_eq!(a, c, "byte-by-byte result differs");
        }
        (Err(_), Err(_), Err(_)) => {}
        _ => panic!(
            "success depends on chunking: whole={:?} chunked={:?} bytes={:?}",
            whole.as_ref().map(Vec::len),
            chunked.as_ref().map(Vec::len),
            one_by_one.as_ref().map(Vec::len)
        ),
    }

    if let Ok(parts) = whole {
        let mut delimiter = b"\r\n--".to_vec();
        delimiter.extend_from_slice(boundary);
        for part in parts {
            assert!(
                !part
                    .data
                    .windows(delimiter.len())
                    .any(|w| w == delimiter.as_slice()),
                "body data contains the delimiter"
            );
            assert_eq!(part.segment.disposition.as_deref(), Some("form-data"));
            assert!(part.segment.name.is_some());
        }
    }

    // Strict mode is chunk dependent by design. It just must not panic.
    let strict = Config {
        strict: true,
        ..config
    };
    let _ = parse(boundary, strict, body, &sizes);
}

/// Compare the SIMD delimiter search with a naive search.
///
/// # Panics
///
/// If the results differ.
pub fn check_search(data: &[u8]) {
    let Some((&len, rest)) = data.split_first() else {
        return;
    };
    let len = usize::from(len % 80).max(1).min(rest.len());
    let (boundary, haystack) = rest.split_at(len);
    let mut needle = b"\r\n--".to_vec();
    needle.extend(
        boundary
            .iter()
            .map(|&b| if b == b'\r' || b == b'\n' { b'x' } else { b }),
    );

    let finder = DelimiterFinder::new(&needle);
    let naive = haystack
        .windows(needle.len())
        .position(|w| w == needle.as_slice());
    assert_eq!(finder.find(haystack), naive, "needle {needle:?}");
}

/// Run the header helpers on arbitrary text and check simple invariants.
///
/// # Panics
///
/// If a helper panics or an invariant is violated.
pub fn check_headers(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    for filename in [false, true] {
        let unquoted = header_unquote(&text, filename);
        if !(text.starts_with('"') && text.ends_with('"')) {
            assert_eq!(unquoted, text, "unquoted value changed");
        }
        let _ = content_disposition_unquote(&text, filename);
    }

    let (value, options) = parse_options_header(&text, header_unquote);
    assert_eq!(value, value.to_lowercase(), "value is not lower case");
    for (key, _) in options.iter().flatten() {
        assert!(
            key.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "invalid option key {key:?}"
        );
    }
    let _ = parse_options_header(&text, content_disposition_unquote);
    let _ = parse_content_disposition(&text);

    if let Some(name) = normalize_header_name(&text) {
        assert_ne!(name, "");
        assert_eq!(normalize_header_name(&name).as_deref(), Some(name.as_str()));
    }
}

#[cfg(test)]
mod tests {
    //! Run the checks on generated inputs, so they are exercised on stable
    //! Rust without a fuzzer.

    use super::*;

    /// xorshift64*, good enough to generate test input.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn byte(&mut self, n: u8) -> u8 {
            u8::try_from(self.next() % u64::from(n)).unwrap()
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % n as u64).unwrap()
        }

        fn pick<'a>(&mut self, items: &[&'a [u8]]) -> &'a [u8] {
            items[self.below(items.len())]
        }
    }

    const TOKENS: [&[u8]; 16] = [
        b"\r\n",
        b"\r",
        b"\n",
        b"--",
        b"-",
        b"boundary",
        b"--boundary",
        b"\r\n--boundary",
        b"Content-Disposition: form-data; name=\"a\"",
        b"; filename=\"f\"",
        b"Content-Type: text/plain; charset=utf8",
        b"Content-Length: 3",
        b" folded",
        b"x",
        b"abc",
        b"\xff\x00",
    ];

    fn valid_form(rng: &mut Rng) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..rng.below(4) {
            out.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"a\"\r\n");
            if rng.below(2) == 0 {
                out.extend_from_slice(b"Content-Type: text/plain\r\n");
            }
            out.extend_from_slice(b"\r\n");
            for _ in 0..rng.below(6) {
                out.extend_from_slice(rng.pick(&TOKENS));
            }
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"--boundary--\r\n");
        out
    }

    fn generate(rng: &mut Rng) -> Vec<u8> {
        // flags: boundary "boundary", maybe small limits; then chunk sizes.
        let mut input = vec![1 | rng.byte(2) << 2];
        input.extend((0..3).map(|_| rng.byte(20) + 1));
        let mut body = valid_form(rng);
        for _ in 0..rng.below(4) {
            let pos = rng.below(body.len() + 1);
            match rng.below(3) {
                0 => {
                    let token = rng.pick(&TOKENS);
                    body.splice(pos..pos, token.iter().copied());
                }
                1 if pos < body.len() => {
                    body.remove(pos);
                }
                _ => body.truncate(pos),
            }
        }
        input.extend(body);
        input
    }

    #[test]
    fn parse_properties() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..3000 {
            check_parse(&generate(&mut rng));
        }
    }

    #[test]
    fn search_properties() {
        let mut rng = Rng(42);
        for _ in 0..3000 {
            let mut input = vec![rng.byte(12) + 1];
            input.extend_from_slice(b"boundary");
            for _ in 0..rng.below(40) {
                input.extend_from_slice(rng.pick(&TOKENS));
            }
            check_search(&input);
        }
    }

    #[test]
    fn header_properties() {
        let mut rng = Rng(7);
        let tokens: [&[u8]; 12] = [
            b";",
            b"=",
            b"\"",
            b"\\",
            b"%22",
            b"%0D",
            b" ",
            b"name",
            b"filename",
            b"C:",
            b"\xc3\xa4",
            b"\xe2\x82\xac",
        ];
        for _ in 0..5000 {
            let mut input = Vec::new();
            for _ in 0..rng.below(16) {
                input.extend_from_slice(rng.pick(&tokens));
            }
            check_headers(&input);
        }
    }
}
