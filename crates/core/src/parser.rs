use memchr::memmem::Finder;
use memchr::{memchr, memrchr};

use crate::error::Error;
use crate::headers::{Segment, normalize_header_name};
use crate::search::DelimiterFinder;

/// Charset used to decode segment header lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeaderCharset {
    #[default]
    Utf8,
    Latin1,
    Ascii,
}

/// Parser limits and options. `None` means unlimited.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum length of a single header line (name and value).
    pub max_header_size: usize,
    /// Maximum number of headers per segment.
    pub max_header_count: usize,
    /// Maximum size of a single segment body.
    pub max_segment_size: Option<u64>,
    /// Maximum number of segments.
    pub max_segment_count: Option<u64>,
    /// Expected total input size in bytes, if known.
    pub content_length: Option<u64>,
    pub header_charset: HeaderCharset,
    /// Enables additional format and sanity checks.
    pub strict: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_header_size: 4096 + 128,
            max_header_count: 8,
            max_segment_size: None,
            max_segment_count: None,
            content_length: None,
            header_charset: HeaderCharset::Utf8,
            strict: false,
        }
    }
}

/// Receives parser events.
///
/// For each segment the parser calls [`Sink::segment`] once, then
/// [`Sink::data`] zero or more times with non-empty body chunks, then
/// [`Sink::segment_end`] once. Body chunks borrow from the input chunk passed
/// to [`Parser::feed`] whenever possible, but may also come from a small
/// internal buffer when a chunk ended with something that looked like the
/// start of a boundary.
pub trait Sink {
    fn segment(&mut self, segment: Segment);
    fn data(&mut self, data: &[u8]);
    fn segment_end(&mut self, size: u64);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Skipping data in front of the first boundary.
    Preamble,
    /// Reading segment header lines.
    Header,
    /// Reading segment body data.
    Body,
    /// A delimiter was found in the body; waiting for `CRLF` or `--`.
    BoundaryTail,
    /// The final delimiter was found. Anything after it is ignored.
    Complete,
}

/// Incremental (push) parser for `multipart/form-data` streams.
pub struct Parser {
    config: Config,
    /// `CRLF--boundary`
    delimiter: Box<[u8]>,
    // The finders are boxed because `Finder` contains SIMD vectors and needs
    // 32-byte alignment, which containers such as Python objects (16-byte
    // aligned) do not guarantee. Unboxed, the parser crashes when embedded in
    // a PyO3 class.
    /// Finds `CRLF--boundary` in segment bodies.
    delimiter_finder: Box<DelimiterFinder>,
    /// Finds `--boundary` in the preamble, where the leading CRLF is optional.
    /// Built on first use: most streams start with the boundary right away.
    boundary_finder: Option<Box<Finder<'static>>>,
    state: State,
    /// Bytes carried over from the previous chunk. Depending on the state this
    /// is a proper prefix of the delimiter, a partial header line, a partial
    /// boundary tail, or the end of the preamble.
    buf: Vec<u8>,
    received: u64,
    closed: bool,
    error: Option<Error>,
    segment_count: u64,
    headers: Vec<(String, String)>,
    segment_size: u64,
    segment_limit: Option<u64>,
}

impl Parser {
    /// Create a parser for the given boundary (as found in the Content-Type
    /// header, without leading dashes).
    ///
    /// # Errors
    ///
    /// Fails if the boundary is empty or contains CR or LF, or (in strict
    /// mode) is longer than 1024 bytes.
    pub fn new(boundary: &[u8], config: Config) -> Result<Self, Error> {
        if boundary.is_empty() {
            return Err(Error::parser("Empty boundary"));
        }
        // A CR in the boundary would break partial delimiter detection, and
        // neither CR nor LF are allowed by RFC 2046 anyway.
        if boundary.contains(&b'\n') || boundary.contains(&b'\r') {
            return Err(Error::parser("Invalid characters in boundary"));
        }
        if config.strict && boundary.len() > 1024 {
            return Err(Error::strict("Boundary too long"));
        }

        let mut delimiter = Vec::with_capacity(boundary.len() + 4);
        delimiter.extend_from_slice(b"\r\n--");
        delimiter.extend_from_slice(boundary);

        Ok(Self {
            delimiter_finder: Box::new(DelimiterFinder::new(&delimiter)),
            boundary_finder: None,
            delimiter: delimiter.into_boxed_slice(),
            config,
            state: State::Preamble,
            buf: Vec::new(),
            received: 0,
            closed: false,
            error: None,
            segment_count: 0,
            headers: Vec::new(),
            segment_size: 0,
            segment_limit: None,
        })
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// True if the parser was closed, either explicitly or due to an error.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// True if the end of the multipart stream was reached.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.state == State::Complete
    }

    /// The first error encountered, if any.
    #[must_use]
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    /// Body bytes received so far for the current (or last) segment.
    #[must_use]
    pub fn segment_size(&self) -> u64 {
        self.segment_size
    }

    /// Parse a chunk of input and report all events that can be derived from
    /// it to `sink`. Empty chunks are ignored; use [`Parser::close`] to signal
    /// the end of input.
    ///
    /// # Errors
    ///
    /// Fails if the input is not a valid multipart stream, a limit is
    /// exceeded, or the parser is already closed. On error the parser is
    /// closed and the error is remembered.
    pub fn feed<S: Sink>(&mut self, chunk: &[u8], sink: &mut S) -> Result<(), Error> {
        let result = self.feed_inner(chunk, sink);
        if let Err(err) = &result {
            if self.error.is_none() {
                self.error = Some(err.clone());
            }
            self.closed = true;
            self.buf = Vec::new();
        }
        result
    }

    /// Close the parser.
    ///
    /// # Errors
    ///
    /// With `check_complete`, fails if the end of the multipart stream was
    /// not reached yet.
    pub fn close(&mut self, check_complete: bool) -> Result<(), Error> {
        self.closed = true;
        self.buf = Vec::new();
        if check_complete && self.state != State::Complete {
            let err = Error::parser("Unexpected end of multipart stream (parser closed)");
            if self.error.is_none() {
                self.error = Some(err.clone());
            }
            return Err(err);
        }
        Ok(())
    }

    fn feed_inner<S: Sink>(&mut self, chunk: &[u8], sink: &mut S) -> Result<(), Error> {
        if self.closed {
            return Err(Error::state("Parser closed"));
        }
        if chunk.is_empty() {
            return Ok(());
        }
        if let Some(limit) = self.config.content_length
            && self.received.saturating_add(chunk.len() as u64) > limit
        {
            return Err(Error::parser("Content-Length limit exceeded"));
        }
        if self.state == State::Complete {
            if self.config.strict {
                return Err(Error::strict(
                    "Unexpected data after end of multipart stream",
                ));
            }
            return Ok(());
        }
        self.received += chunk.len() as u64;
        self.run(chunk, sink)
    }

    fn run<S: Sink>(&mut self, data: &[u8], sink: &mut S) -> Result<(), Error> {
        let mut pos = 0;
        while pos < data.len() {
            let rest = &data[pos..];
            // Each step either consumes some bytes and changes state (`Some`),
            // or consumes or buffers everything and asks for more data (`None`).
            let consumed = match self.state {
                State::Preamble => {
                    if !self.buf.is_empty() {
                        // Preamble data is never emitted, so stitching costs
                        // only a copy, and only until the first boundary.
                        let mut joined = std::mem::take(&mut self.buf);
                        joined.extend_from_slice(rest);
                        return self.run(&joined, sink);
                    }
                    self.preamble(rest)?
                }
                State::Header => self.header(rest, sink)?,
                State::Body => self.body(rest, sink)?,
                State::BoundaryTail => self.boundary_tail(rest, sink)?,
                State::Complete => return Ok(()),
            };
            match consumed {
                Some(n) => pos += n,
                None => return Ok(()),
            }
        }
        Ok(())
    }

    fn preamble(&mut self, rest: &[u8]) -> Result<Option<usize>, Error> {
        let d_len = self.delimiter.len();
        if rest.len() < d_len {
            self.buf.extend_from_slice(rest);
            return Ok(None);
        }

        let boundary = &self.delimiter[2..];
        let found = if rest.starts_with(boundary) {
            Some(0)
        } else {
            self.boundary_finder
                .get_or_insert_with(|| Box::new(Finder::new(boundary).into_owned()))
                .find(rest)
        };
        let Some(index) = found else {
            if self.config.strict {
                // Fail fast instead of consuming a legacy preamble.
                return Err(Error::strict("Boundary not found in first chunk"));
            }
            // Skip the preamble, but keep enough bytes for a boundary that
            // starts in this chunk to be found with the two bytes in front of
            // it, so that the CRLF check does not depend on chunk borders.
            self.buf
                .extend_from_slice(&rest[rest.len() - (d_len - 1)..]);
            return Ok(None);
        };

        // The first boundary must be at the very start, or follow a CRLF.
        if index > 0 && (index < 2 || &rest[index - 2..index] != b"\r\n") {
            return Err(Error::parser("Unexpected byte in front of first boundary"));
        }

        let next = index + d_len;
        let tail = &rest[(next - 2).min(rest.len())..next.min(rest.len())];
        match tail {
            b"\r\n" => {
                self.start_segment()?;
                self.state = State::Header;
                Ok(Some(next))
            }
            b"--" => {
                // First is also last delimiter.
                self.state = State::Complete;
                Ok(Some(rest.len()))
            }
            [b'\n', ..] => Err(Error::parser("Invalid line break after first boundary")),
            _ if next <= rest.len() => Err(Error::parser("Unexpected byte after first boundary")),
            _ => {
                // Tail not complete yet. Keep the boundary and wait.
                self.buf.extend_from_slice(&rest[index.saturating_sub(2)..]);
                Ok(None)
            }
        }
    }

    fn header<S: Sink>(&mut self, rest: &[u8], sink: &mut S) -> Result<Option<usize>, Error> {
        let mut pos = 0;
        loop {
            let avail = &rest[pos..];
            let lf = memchr(b'\n', avail);

            if !self.buf.is_empty() {
                // Complete a header line that started in a previous chunk.
                let Some(i) = lf else {
                    return self.header_wait(avail);
                };
                let mut line = std::mem::take(&mut self.buf);
                if i == 0 {
                    if line.pop() != Some(b'\r') {
                        return Err(Error::parser("Invalid line break in segment header"));
                    }
                } else {
                    if avail[i - 1] != b'\r' {
                        return Err(Error::parser("Invalid line break in segment header"));
                    }
                    line.extend_from_slice(&avail[..i - 1]);
                }
                pos += i + 1;
                if line.is_empty() {
                    self.finish_headers(sink)?;
                    return Ok(Some(pos));
                }
                self.header_line(&line)?;
                continue;
            }

            let Some(i) = lf else {
                return self.header_wait(avail);
            };
            // Bare LF line breaks are rejected, even inside a header line.
            if i == 0 || avail[i - 1] != b'\r' {
                return Err(Error::parser("Invalid line break in segment header"));
            }
            pos += i + 1;
            if i == 1 {
                // Empty line: end of the header section.
                self.finish_headers(sink)?;
                return Ok(Some(pos));
            }
            self.header_line(&avail[..i - 1])?;
        }
    }

    fn header_wait(&mut self, avail: &[u8]) -> Result<Option<usize>, Error> {
        self.buf.extend_from_slice(avail);
        // A trailing CR may be the start of the line break, which does not
        // count towards the limit once the line is complete.
        let len = self.buf.len() - usize::from(self.buf.last() == Some(&b'\r'));
        if len > self.config.max_header_size {
            return Err(Error::limit("Maximum segment header length exceeded"));
        }
        Ok(None)
    }

    /// Handle a complete, non-empty header line (without the CRLF).
    fn header_line(&mut self, line: &[u8]) -> Result<(), Error> {
        if line[0] != b' ' && line[0] != b'\t' {
            return self.add_header(line);
        }

        // Header continuation (obsolete line folding).
        if self.headers.is_empty() || self.config.strict {
            return Err(Error::strict("Unexpected segment header continuation"));
        }
        if line.iter().all(|b| b" \t\n\r\x0b\x0c".contains(b)) {
            return Err(Error::strict("Empty segment header continuation"));
        }
        let (name, value) = self.headers.pop().expect("checked above");
        let mut joined = Vec::with_capacity(name.len() + value.len() + line.len() + 2);
        self.encode_into(&name, &mut joined);
        joined.extend_from_slice(b": ");
        self.encode_into(&value, &mut joined);
        joined.push(b' ');
        joined.extend_from_slice(&line[1..]);
        self.add_header(&joined)
    }

    fn add_header(&mut self, line: &[u8]) -> Result<(), Error> {
        if line.len() > self.config.max_header_size {
            return Err(Error::limit("Maximum segment header length exceeded"));
        }
        if self.headers.len() >= self.config.max_header_count {
            return Err(Error::limit("Maximum segment header count exceeded"));
        }

        let text = self.decode(line)?;
        let Some((name, value)) = text.split_once(':') else {
            return Err(Error::parser("Malformed segment header"));
        };
        let name =
            normalize_header_name(name).ok_or(Error::parser("Invalid segment header name"))?;
        let value = value.trim_start_matches([' ', '\t']).to_owned();

        if name == "Content-Length" {
            if self.segment_limit.is_some() {
                return Err(Error::parser("Multiple segment Content-Length headers"));
            }
            let length = parse_content_length(&value)
                .ok_or(Error::parser("Invalid segment Content-Length header value"))?;
            if self.config.max_segment_size.is_some_and(|max| length > max) {
                return Err(Error::limit(
                    "Segment Content-Length larger than maximum segment size",
                ));
            }
            self.segment_limit = Some(length);
        }

        self.headers.push((name, value));
        Ok(())
    }

    fn decode(&self, line: &[u8]) -> Result<String, Error> {
        const ERR: Error = Error::parser("Segment header failed to decode");
        match self.config.header_charset {
            HeaderCharset::Utf8 => std::str::from_utf8(line)
                .map(str::to_owned)
                .map_err(|_| ERR),
            HeaderCharset::Ascii if line.is_ascii() => {
                Ok(std::str::from_utf8(line).map_err(|_| ERR)?.to_owned())
            }
            HeaderCharset::Ascii => Err(ERR),
            HeaderCharset::Latin1 => Ok(line.iter().map(|&b| b as char).collect()),
        }
    }

    fn encode_into(&self, text: &str, out: &mut Vec<u8>) {
        match self.config.header_charset {
            HeaderCharset::Utf8 | HeaderCharset::Ascii => out.extend_from_slice(text.as_bytes()),
            // Decoded from Latin-1, so every char fits into one byte.
            HeaderCharset::Latin1 => out.extend(text.chars().map(|c| c as u8)),
        }
    }

    fn finish_headers<S: Sink>(&mut self, sink: &mut S) -> Result<(), Error> {
        let mut segment = Segment::from_headers(std::mem::take(&mut self.headers));
        match segment.disposition.as_deref() {
            Some("form-data") => {}
            None => return Err(Error::parser("Missing Content-Disposition segment header")),
            Some(_) => {
                return Err(Error::parser(
                    "Invalid Content-Disposition segment header: Wrong type",
                ));
            }
        }
        if segment.name.is_none() {
            if self.config.strict {
                return Err(Error::strict(
                    "Invalid Content-Disposition segment header: Missing name option",
                ));
            }
            segment.name = Some(String::new());
        }
        sink.segment(segment);
        self.state = State::Body;
        Ok(())
    }

    fn body<S: Sink>(&mut self, rest: &[u8], sink: &mut S) -> Result<Option<usize>, Error> {
        let d_len = self.delimiter.len();

        if !self.buf.is_empty() {
            // The last chunk ended with a proper prefix of the delimiter.
            let have = self.buf.len();
            let need = d_len - have;
            let n = need.min(rest.len());
            if rest[..n] == self.delimiter[have..have + n] {
                if n < need {
                    self.buf.extend_from_slice(rest);
                    return Ok(None);
                }
                self.buf.clear();
                self.state = State::BoundaryTail;
                return Ok(Some(n));
            }
            // False alarm, the carried bytes are body data. A delimiter cannot
            // start later within them either: they start with the only CR the
            // delimiter contains.
            let carry = std::mem::take(&mut self.buf);
            self.emit_data(&carry, sink)?;
        }

        if let Some(i) = self.delimiter_finder.find(rest) {
            if i > 0 {
                self.emit_data(&rest[..i], sink)?;
            }
            self.state = State::BoundaryTail;
            return Ok(Some(i + d_len));
        }

        // No delimiter. Emit everything but a possible partial delimiter at
        // the end. Only the last CR can start one, for the same reason as above.
        let window = rest.len().saturating_sub(d_len - 1);
        let keep = match memrchr(b'\r', &rest[window..]) {
            Some(r) if self.delimiter.starts_with(&rest[window + r..]) => window + r,
            _ => rest.len(),
        };
        if keep > 0 {
            self.emit_data(&rest[..keep], sink)?;
        }
        self.buf.extend_from_slice(&rest[keep..]);
        Ok(None)
    }

    fn boundary_tail<S: Sink>(
        &mut self,
        rest: &[u8],
        sink: &mut S,
    ) -> Result<Option<usize>, Error> {
        let n = (2 - self.buf.len()).min(rest.len());
        self.buf.extend_from_slice(&rest[..n]);
        if self.buf.len() < 2 {
            return Ok(None);
        }
        let tail = [self.buf[0], self.buf[1]];
        self.buf.clear();
        match &tail {
            b"\r\n" => {
                self.end_segment(sink)?;
                self.start_segment()?;
                self.state = State::Header;
                Ok(Some(n))
            }
            b"--" => {
                self.end_segment(sink)?;
                self.state = State::Complete;
                Ok(Some(rest.len()))
            }
            _ => Err(Error::parser("Unexpected bytes after boundary")),
        }
    }

    fn start_segment(&mut self) -> Result<(), Error> {
        self.segment_count += 1;
        if self
            .config
            .max_segment_count
            .is_some_and(|max| self.segment_count > max)
        {
            return Err(Error::limit("Maximum segment count exceeded"));
        }
        self.headers.clear();
        self.segment_size = 0;
        self.segment_limit = None;
        Ok(())
    }

    fn emit_data<S: Sink>(&mut self, data: &[u8], sink: &mut S) -> Result<(), Error> {
        self.segment_size += data.len() as u64;
        if self
            .config
            .max_segment_size
            .is_some_and(|max| self.segment_size > max)
        {
            return Err(Error::limit("Maximum segment size exceeded"));
        }
        if self
            .segment_limit
            .is_some_and(|limit| self.segment_size > limit)
        {
            return Err(Error::parser("Segment Content-Length exceeded"));
        }
        sink.data(data);
        Ok(())
    }

    fn end_segment<S: Sink>(&mut self, sink: &mut S) -> Result<(), Error> {
        if self
            .segment_limit
            .is_some_and(|limit| self.segment_size < limit)
        {
            return Err(Error::parser(
                "Segment size does not match Content-Length header",
            ));
        }
        sink.segment_end(self.segment_size);
        Ok(())
    }
}

/// Parse an unsigned decimal in canonical form (no sign, no leading zeros,
/// no whitespace). Values that overflow `u64` saturate.
fn parse_content_length(value: &str) -> Option<u64> {
    let digits = value.as_bytes();
    if digits.is_empty()
        || !digits.iter().all(u8::is_ascii_digit)
        || (digits.len() > 1 && digits[0] == b'0')
    {
        return None;
    }
    Some(digits.iter().fold(0u64, |acc, d| {
        acc.saturating_mul(10).saturating_add(u64::from(d - b'0'))
    }))
}

#[cfg(test)]
mod tests;
