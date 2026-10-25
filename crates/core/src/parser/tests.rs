use super::*;
use crate::ErrorKind;

const BOUNDARY: &[u8] = b"boundary";

#[derive(Debug, Default, PartialEq)]
struct Part {
    segment: Segment,
    data: Vec<u8>,
    size: Option<u64>,
}

#[derive(Default)]
struct Collect {
    parts: Vec<Part>,
    data_events: usize,
}

impl Sink for Collect {
    fn segment(&mut self, segment: Segment) {
        self.parts.push(Part {
            segment,
            ..Default::default()
        });
    }

    fn data(&mut self, data: &[u8]) {
        assert!(!data.is_empty(), "empty data event");
        let part = self.parts.last_mut().expect("data before segment");
        assert!(part.size.is_none(), "data after segment end");
        part.data.extend_from_slice(data);
        self.data_events += 1;
    }

    fn segment_end(&mut self, size: u64) {
        let part = self.parts.last_mut().expect("end before segment");
        assert_eq!(part.data.len() as u64, size);
        part.size = Some(size);
    }
}

fn parse_with(input: &[u8], chunk_size: usize, config: Config) -> Result<Vec<Part>, Error> {
    let mut parser = Parser::new(BOUNDARY, config)?;
    let mut sink = Collect::default();
    for chunk in input.chunks(chunk_size.max(1)) {
        parser.feed(chunk, &mut sink)?;
    }
    parser.close(true)?;
    Ok(sink.parts)
}

fn parse(input: &[u8], chunk_size: usize) -> Result<Vec<Part>, Error> {
    parse_with(input, chunk_size, Config::default())
}

/// Parse with every chunk size from 1 to the input length and check that
/// the result does not depend on how the input was split.
fn parse_all_splits(input: &[u8]) -> Result<Vec<Part>, Error> {
    let expected = parse(input, input.len());
    for size in 1..input.len() {
        assert_eq!(parse(input, size), expected, "chunk size {size}");
    }
    expected
}

fn form(fields: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, filename, data) in fields {
        out.extend_from_slice(b"--boundary\r\n");
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if let Some(filename) = filename {
            out.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
        }
        out.extend_from_slice(b"\r\n\r\n");
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--boundary--\r\n");
    out
}

#[test]
fn simple_form() {
    let input = form(&[("a", None, b"hello"), ("b", Some("x.txt"), b"wor\r\nld")]);
    let parts = parse_all_splits(&input).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].segment.name.as_deref(), Some("a"));
    assert_eq!(parts[0].segment.filename, None);
    assert_eq!(parts[0].data, b"hello");
    assert_eq!(parts[1].segment.name.as_deref(), Some("b"));
    assert_eq!(parts[1].segment.filename.as_deref(), Some("x.txt"));
    assert_eq!(parts[1].data, b"wor\r\nld");
    assert_eq!(parts[1].size, Some(7));
    assert_eq!(
        parts[0].segment.headers,
        vec![(
            "Content-Disposition".to_owned(),
            "form-data; name=\"a\"".to_owned()
        )]
    );
}

#[test]
fn empty_parts_and_empty_form() {
    let parts = parse_all_splits(&form(&[("a", None, b""), ("b", None, b"")])).unwrap();
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|p| p.data.is_empty() && p.size == Some(0)));
    assert_eq!(parse_all_splits(b"--boundary--").unwrap(), vec![]);
}

#[test]
fn headers_are_normalized() {
    let input = b"--boundary\r\n\
        content-disposition: form-data; name=a\r\n\
        x-custom_header:\t value \r\n\
        CONTENT-TYPE: Text/Plain; Charset=\"latin1\"\r\n\
        \r\n\
        data\r\n--boundary--";
    let parts = parse_all_splits(input).unwrap();
    let seg = &parts[0].segment;
    assert_eq!(
        seg.headers,
        vec![
            ("Content-Disposition".into(), "form-data; name=a".into()),
            ("X-Custom_Header".into(), "value ".into()),
            (
                "Content-Type".into(),
                "Text/Plain; Charset=\"latin1\"".into()
            ),
        ]
    );
    assert_eq!(seg.content_type.as_deref(), Some("text/plain"));
    assert_eq!(seg.charset.as_deref(), Some("latin1"));
}

#[test]
fn header_continuation() {
    let input = b"--boundary\r\n\
        Content-Disposition: form-data;\r\n name=\"a\"\r\n\
        \r\n\
        x\r\n--boundary--";
    let parts = parse_all_splits(input).unwrap();
    assert_eq!(parts[0].segment.name.as_deref(), Some("a"));

    let strict = Config {
        strict: true,
        ..Config::default()
    };
    let err = parse_with(input, input.len(), strict).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Strict);
}

#[test]
fn preamble_and_epilogue_are_ignored() {
    let mut input = b"preamble\r\nmore preamble\r\n".to_vec();
    input.extend(form(&[("a", None, b"x")]));
    input.extend_from_slice(b"epilogue --boundary\r\n garbage");
    let parts = parse_all_splits(&input).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].data, b"x");
}

#[test]
fn boundary_lookalikes_in_body() {
    let data = b"\r\n--boundar\r\n--boundarX\r\n-\r\r\n\r\n--bound\r--boundary\r";
    let parts = parse_all_splits(&form(&[("a", None, data)])).unwrap();
    assert_eq!(parts[0].data, data);
}

#[test]
fn worst_case_inputs() {
    let crlf = b"\r\n".repeat(10_000);
    let lf = b"\n".repeat(10_000);
    let bchar = b"\r\n--boundar".repeat(1_000);
    for data in [&crlf[..], &lf[..], &bchar[..]] {
        for chunk_size in [1, 7, 64, 1000, 65536] {
            let parts = parse(&form(&[("f", Some("f.bin"), data)]), chunk_size).unwrap();
            assert_eq!(parts[0].data, data);
        }
    }

    let junk: Vec<u8> = (0..100_000u32)
        .map(|i| b" \r\n-abcxyz"[(i % 10) as usize])
        .collect();
    let mut input = junk.clone();
    input.extend_from_slice(b"\r\n");
    input.extend(form(&[("f", None, b"Content")]));
    input.extend(&junk);
    for chunk_size in [1, 7, 64, 1000, 65536] {
        let parts = parse(&input, chunk_size).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].data, b"Content");
    }
}

#[test]
fn body_data_is_zero_copy() {
    struct Check<'a> {
        input: &'a [u8],
        copied: usize,
    }
    impl Sink for Check<'_> {
        fn segment(&mut self, _: Segment) {}
        fn segment_end(&mut self, _: u64) {}
        fn data(&mut self, data: &[u8]) {
            let range = self.input.as_ptr_range();
            if !range.contains(&data.as_ptr()) {
                self.copied += data.len();
            }
        }
    }

    let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
    let input = form(&[("f", Some("f.bin"), &data)]);
    let mut parser = Parser::new(BOUNDARY, Config::default()).unwrap();
    let mut sink = Check {
        input: &input,
        copied: 0,
    };
    parser.feed(&input, &mut sink).unwrap();
    assert!(parser.is_complete());
    assert_eq!(sink.copied, 0);
}

#[test]
fn incomplete_input() {
    let input = form(&[("a", None, b"x")]);
    for end in 0..input.len() - 2 {
        let err = parse(&input[..end], 3).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parser, "end {end}");
    }
}

#[test]
fn invalid_input() {
    let cases: &[(&[u8], &str)] = &[
        (b"x--boundary\r\n", "Unexpected byte in front of first boundary"),
        (b"--boundary\n\r\n", "Invalid line break after first boundary"),
        (b"--boundaryXY", "Unexpected byte after first boundary"),
        (
            b"--boundary\r\nContent-Disposition: form-data; name=a\n\r\n",
            "Invalid line break in segment header",
        ),
        (b"--boundary\r\nfoo\r\n\r\n", "Malformed segment header"),
        (b"--boundary\r\nf o: x\r\n\r\n", "Invalid segment header name"),
        (b"--boundary\r\n\r\n", "Missing Content-Disposition segment header"),
        (
            b"--boundary\r\nContent-Disposition: attachment\r\n\r\n",
            "Invalid Content-Disposition segment header: Wrong type",
        ),
        (
            b"--boundary\r\nContent-Disposition: form-data; name=a\r\n\r\nx\r\n--boundaryXY",
            "Unexpected bytes after boundary",
        ),
        (
            b"--boundary\r\nContent-Disposition: form-data; name=a\r\nContent-Length: 2\r\n\r\nx\r\n--boundary--",
            "Segment size does not match Content-Length header",
        ),
        (
            b"--boundary\r\nContent-Disposition: form-data; name=a\r\nContent-Length: 0\r\n\r\nx\r\n--boundary--",
            "Segment Content-Length exceeded",
        ),
        (
            b"--boundary\r\nContent-Disposition: form-data; name=a\r\nContent-Length: 01\r\n\r\nx\r\n--boundary--",
            "Invalid segment Content-Length header value",
        ),
        (
            b"--boundary\r\nContent-Disposition: form-data; name=a\r\n\xff: x\r\n\r\n",
            "Segment header failed to decode",
        ),
    ];
    for (input, message) in cases {
        for chunk_size in [1, 2, 5, input.len()] {
            let err = parse(input, chunk_size).unwrap_err();
            assert_eq!(err.message, *message, "chunk size {chunk_size}");
        }
    }
}

#[test]
fn limits() {
    let input = form(&[("a", None, b"0123456789"), ("b", None, b"x")]);
    let limited = |config: Config| parse_with(&input, 4, config).unwrap_err();

    let err = limited(Config {
        max_segment_size: Some(9),
        ..Config::default()
    });
    assert_eq!(err.message, "Maximum segment size exceeded");

    let err = limited(Config {
        max_segment_count: Some(1),
        ..Config::default()
    });
    assert_eq!(err.message, "Maximum segment count exceeded");

    let err = limited(Config {
        max_header_size: 20,
        ..Config::default()
    });
    assert_eq!(err.message, "Maximum segment header length exceeded");

    let err = limited(Config {
        max_header_count: 0,
        ..Config::default()
    });
    assert_eq!(err.message, "Maximum segment header count exceeded");

    let err = limited(Config {
        content_length: Some(input.len() as u64 - 1),
        ..Config::default()
    });
    assert_eq!(err.message, "Content-Length limit exceeded");
    assert!(
        parse_with(
            &input,
            4,
            Config {
                content_length: Some(input.len() as u64),
                ..Config::default()
            }
        )
        .is_ok()
    );
}

#[test]
fn header_limit_does_not_depend_on_chunking() {
    let line: &[u8] = b"Content-Disposition: form-data; name=a";
    let config = |max| Config {
        max_header_size: max,
        ..Config::default()
    };
    let mut input = b"--boundary\r\n".to_vec();
    input.extend_from_slice(line);
    input.extend_from_slice(b"\r\n\r\nx\r\n--boundary--");
    for size in 1..=input.len() {
        assert!(
            parse_with(&input, size, config(line.len())).is_ok(),
            "size {size}"
        );
        let err = parse_with(&input, size, config(line.len() - 1)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::LimitReached, "size {size}");
    }
}

#[test]
fn errors_close_the_parser() {
    let mut parser = Parser::new(BOUNDARY, Config::default()).unwrap();
    let mut sink = Collect::default();
    assert!(parser.feed(b"x--boundary\r\n", &mut sink).is_err());
    assert!(parser.is_closed());
    let err = parser.feed(b"more", &mut sink).unwrap_err();
    assert_eq!(err.kind, ErrorKind::State);
    assert_eq!(
        parser.error().unwrap().message,
        "Unexpected byte in front of first boundary"
    );
}

#[test]
fn parser_alignment_fits_python_objects() {
    // Python only guarantees 16-byte alignment for object memory.
    assert!(std::mem::align_of::<Parser>() <= 16);
}

#[test]
fn invalid_boundaries() {
    assert!(Parser::new(b"", Config::default()).is_err());
    assert!(Parser::new(b"a\nb", Config::default()).is_err());
    assert!(Parser::new(b"a\rb", Config::default()).is_err());
}
