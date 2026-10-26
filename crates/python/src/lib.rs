//! Python bindings for `kirimi-core`.
//!
//! The API mirrors `multipart.PushMultipartParser` and
//! `multipart.MultipartSegment` from the `multipart` library, so this module
//! can be used as a drop-in replacement for the non-blocking parser.

mod parts;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use kirimi_core as mp;
use pyo3::create_exception;
use pyo3::exceptions::{PyKeyError, PyLookupError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyByteArray, PyBytes, PyFloat, PyList, PyString};

create_exception!(
    kirimi,
    MultipartError,
    PyValueError,
    "Base class for all parser errors or warnings"
);
create_exception!(
    kirimi,
    ParserError,
    MultipartError,
    "Detected invalid input"
);
create_exception!(
    kirimi,
    StrictParserError,
    ParserError,
    "Detected unusual input while parsing in strict mode"
);
create_exception!(
    kirimi,
    ParserLimitReached,
    MultipartError,
    "Parser reached one of the configured limits"
);
create_exception!(
    kirimi,
    ParserStateError,
    MultipartError,
    "Parser is used incorrectly (e.g. use after close)"
);

fn to_pyerr(err: &mp::Error) -> PyErr {
    let message = err.message;
    match err.kind {
        mp::ErrorKind::Parser => ParserError::new_err(message),
        mp::ErrorKind::Strict => StrictParserError::new_err(message),
        mp::ErrorKind::LimitReached => ParserLimitReached::new_err(message),
        mp::ErrorKind::State => ParserStateError::new_err(message),
    }
}

/// A size or count limit: a non-negative int, or `None` / `inf` for unlimited.
#[derive(Clone, Copy)]
struct Limit(Option<u64>);

impl<'a, 'py> FromPyObject<'a, 'py> for Limit {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        if obj.is_none() {
            return Ok(Limit(None));
        }
        if let Ok(float) = obj.cast::<PyFloat>()
            && float.value() == f64::INFINITY
        {
            return Ok(Limit(None));
        }
        obj.extract::<u64>()
            .map(|v| Limit(Some(v)))
            .map_err(|_| PyValueError::new_err("Limits must be non-negative integers, None or inf"))
    }
}

impl Limit {
    fn to_object(self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        Ok(match self.0 {
            Some(v) => v.into_pyobject(py)?.into_any().unbind(),
            None => PyFloat::new(py, f64::INFINITY).into_any().unbind(),
        })
    }
}

/// Title-case an ASCII header name like Python's `str.title()`.
fn ascii_title(name: &str) -> String {
    let mut prev_cased = false;
    name.chars()
        .map(|c| {
            let out = if !c.is_ascii_alphabetic() {
                c
            } else if prev_cased {
                c.to_ascii_lowercase()
            } else {
                c.to_ascii_uppercase()
            };
            prev_cased = c.is_ascii_alphabetic();
            out
        })
        .collect()
}

/// The header section of a single multipart part, with convenient access to
/// headers and metadata such as `name` and `filename`.
///
/// Segments do not store any payload data, but the parser updates
/// `bytes_received` while parsing and makes the final `size` available once
/// the segment is `complete`.
#[pyclass(module = "kirimi", name = "MultipartSegment", frozen)]
pub struct Segment {
    headers: Vec<(String, String)>,
    headerlist: PyOnceLock<Py<PyList>>,
    disposition: Option<String>,
    name: Option<String>,
    filename: Option<String>,
    content_type: Option<String>,
    charset: Option<String>,
    bytes_received: AtomicU64,
    complete: AtomicBool,
}

impl From<mp::Segment> for Segment {
    fn from(segment: mp::Segment) -> Self {
        Segment {
            headers: segment.headers,
            headerlist: PyOnceLock::new(),
            disposition: segment.disposition,
            name: segment.name,
            filename: segment.filename,
            content_type: segment.content_type,
            charset: segment.charset,
            bytes_received: AtomicU64::new(0),
            complete: AtomicBool::new(false),
        }
    }
}

#[pymethods]
impl Segment {
    /// Ordered list of headers as `(name, value)` pairs. Header names are
    /// normalized (Title-Case) and values are stripped of leading whitespace.
    #[getter]
    fn headerlist(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = self.headerlist.get_or_try_init(py, || {
            PyList::new(
                py,
                self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            )
            .map(Bound::unbind)
        })?;
        Ok(list.clone_ref(py))
    }

    /// The lower-cased `Content-Disposition` header without options.
    #[getter]
    fn disposition(&self) -> Option<&str> {
        self.disposition.as_deref()
    }

    /// The `name` option of the `Content-Disposition` header.
    #[getter]
    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The optional `filename` option of the `Content-Disposition` header.
    #[getter]
    fn filename(&self) -> Option<&str> {
        self.filename.as_deref()
    }

    /// The lower-cased `Content-Type` header without options.
    #[getter]
    fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// The optional `charset` option of the `Content-Type` header.
    #[getter]
    fn charset(&self) -> Option<&str> {
        self.charset.as_deref()
    }

    /// Body bytes received so far.
    #[getter]
    fn bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    /// True if the parser detected the end of the segment and `size` is final.
    #[getter]
    fn complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    /// Final segment body size in bytes. Only available once `complete`.
    #[getter]
    fn size(&self) -> PyResult<u64> {
        if !self.complete() {
            return Err(ParserStateError::new_err("Segment size not known yet"));
        }
        Ok(self.bytes_received())
    }

    /// Return the value of a header if present, or a default value.
    #[pyo3(signature = (name, default=None))]
    fn header<'py>(
        &self,
        name: &Bound<'py, PyString>,
        default: Option<Bound<'py, PyAny>>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let py = name.py();
        match self.find_header(name)? {
            Some(value) => Ok(Some(PyString::new(py, value).into_any())),
            None => Ok(default),
        }
    }

    /// Return a header value if present, or raise `KeyError`.
    fn __getitem__(&self, name: &Bound<'_, PyString>) -> PyResult<String> {
        match self.find_header(name)? {
            Some(value) => Ok(value.to_owned()),
            None => Err(PyKeyError::new_err(name.clone().unbind())),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "<MultipartSegment name={:?} filename={:?}>",
            self.name.as_deref().unwrap_or_default(),
            self.filename
        )
    }
}

impl Segment {
    fn find_header(&self, name: &Bound<'_, PyString>) -> PyResult<Option<&str>> {
        let compare: String = match name.to_str() {
            Ok(text) if text.is_ascii() => ascii_title(text),
            _ => name.call_method0("title")?.extract()?,
        };
        Ok(self
            .headers
            .iter()
            .find(|(k, _)| *k == compare)
            .map(|(_, v)| v.as_str()))
    }
}

/// Collects parser events as Python objects.
struct EventSink<'py> {
    py: Python<'py>,
    /// The input chunk, if body data covering all of it may be returned as-is.
    input: Option<(Bound<'py, PyBytes>, *const u8, usize)>,
    events: Vec<Bound<'py, PyAny>>,
    current: Option<Py<Segment>>,
    error: Option<PyErr>,
}

impl mp::Sink for EventSink<'_> {
    fn segment(&mut self, segment: mp::Segment) {
        match Bound::new(self.py, Segment::from(segment)) {
            Ok(obj) => {
                self.current = Some(obj.clone().unbind());
                self.events.push(obj.into_any());
            }
            Err(err) => {
                self.error.get_or_insert(err);
            }
        }
    }

    fn data(&mut self, data: &[u8]) {
        let obj = match &self.input {
            // Zero-copy: the whole input chunk is body data.
            Some((bytes, ptr, len)) if data.as_ptr() == *ptr && data.len() == *len => {
                bytes.clone().into_any()
            }
            _ => PyBytes::new(self.py, data).into_any(),
        };
        self.events.push(obj);
    }

    fn segment_end(&mut self, size: u64) {
        if let Some(segment) = self.current.take() {
            let segment = segment.get();
            segment.bytes_received.store(size, Ordering::Relaxed);
            segment.complete.store(true, Ordering::Release);
        }
        self.events.push(self.py.None().into_bound(self.py));
    }
}

/// An incremental non-blocking parser for multipart/form-data.
///
/// Call `parse()` with chunks of input. It returns a list of events: one
/// `MultipartSegment` per part, followed by zero or more non-empty `bytes`
/// chunks of the part body, followed by `None` at the end of the part.
/// Parse an empty chunk or call `close()` to signal the end of input.
#[pyclass(module = "kirimi", name = "PushMultipartParser", subclass)]
pub struct PushParser {
    inner: mp::Parser,
    boundary: Py<PyBytes>,
    content_length: i64,
    header_charset: String,
    max_header_size: Limit,
    max_header_count: Limit,
    max_segment_size: Limit,
    max_segment_count: Limit,
    current: Option<Py<Segment>>,
    error: Option<Py<PyAny>>,
}

#[pymethods]
impl PushParser {
    #[new]
    #[pyo3(signature = (
        boundary,
        content_length=-1,
        max_header_size=Limit(Some(4096 + 128)),
        max_header_count=Limit(Some(8)),
        max_segment_size=Limit(None),
        max_segment_count=Limit(None),
        header_charset="utf8",
        strict=false,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        boundary: &Bound<'_, PyAny>,
        content_length: i64,
        max_header_size: Limit,
        max_header_count: Limit,
        max_segment_size: Limit,
        max_segment_count: Limit,
        header_charset: &str,
        strict: bool,
    ) -> PyResult<Self> {
        let boundary = if let Ok(text) = boundary.cast::<PyString>() {
            PyBytes::new(py, text.to_str()?.as_bytes())
        } else if let Ok(bytes) = boundary.cast::<PyBytes>() {
            bytes.clone()
        } else if let Ok(bytes) = boundary.cast::<PyByteArray>() {
            PyBytes::new(py, &bytes.to_vec())
        } else {
            return Err(ParserError::new_err("Invalid boundary type"));
        };

        let config = mp::Config {
            max_header_size: to_usize(max_header_size),
            max_header_count: to_usize(max_header_count),
            max_segment_size: max_segment_size.0,
            max_segment_count: max_segment_count.0,
            content_length: u64::try_from(content_length).ok(),
            header_charset: lookup_charset(py, header_charset)?,
            strict,
        };
        let inner = mp::Parser::new(boundary.as_bytes(), config).map_err(|e| to_pyerr(&e))?;

        Ok(Self {
            inner,
            boundary: boundary.unbind(),
            content_length,
            header_charset: header_charset.to_owned(),
            max_header_size,
            max_header_count,
            max_segment_size,
            max_segment_count,
            current: None,
            error: None,
        })
    }

    /// Parse a chunk of data and return a list of all parser events that
    /// could be derived from it. An empty chunk signals the end of input.
    fn parse<'py>(&mut self, chunk: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let py = chunk.py();
        let result = self.parse_inner(chunk);
        result.map_err(|err| self.remember(py, err))
    }

    /// Close this parser if not already closed. With `check_complete`, raise
    /// `ParserError` if the end of the multipart stream was not reached yet.
    #[pyo3(signature = (check_complete=true))]
    fn close(&mut self, py: Python<'_>, check_complete: bool) -> PyResult<()> {
        self.current = None;
        self.inner
            .close(check_complete)
            .map_err(|e| self.remember(py, to_pyerr(&e)))
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    /// Close the parser. If the context was left due to an exception, the
    /// check for a complete multipart stream is skipped.
    fn __exit__(
        &mut self,
        py: Python<'_>,
        exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        self.close(py, exc_type.is_none())?;
        Ok(false)
    }

    /// True if the parser was closed, either successfully or due to an error.
    #[getter]
    fn closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// A `MultipartError` instance if parsing failed.
    #[getter]
    fn error(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.error.as_ref().map(|e| e.clone_ref(py))
    }

    #[getter]
    fn boundary(&self, py: Python<'_>) -> Py<PyBytes> {
        self.boundary.clone_ref(py)
    }

    #[getter]
    fn content_length(&self) -> i64 {
        self.content_length
    }

    #[getter]
    fn header_charset(&self) -> &str {
        &self.header_charset
    }

    #[getter]
    fn strict(&self) -> bool {
        self.inner.config().strict
    }

    #[getter]
    fn max_header_size(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.max_header_size.to_object(py)
    }

    #[getter]
    fn max_header_count(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.max_header_count.to_object(py)
    }

    #[getter]
    fn max_segment_size(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.max_segment_size.to_object(py)
    }

    #[getter]
    fn max_segment_count(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.max_segment_count.to_object(py)
    }
}

impl PushParser {
    fn parse_inner<'py>(&mut self, chunk: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
        let py = chunk.py();
        if !chunk.is_truthy()? {
            self.close(py, true)?;
            return Ok(PyList::empty(py));
        }
        if self.inner.is_closed() {
            return Err(ParserStateError::new_err("Parser closed"));
        }

        let owned;
        let (data, input) = if let Ok(bytes) = chunk.cast::<PyBytes>() {
            let data = bytes.as_bytes();
            (data, Some((bytes.clone(), data.as_ptr(), data.len())))
        } else if let Ok(bytes) = chunk.cast::<PyByteArray>() {
            // Copy, because the bytearray may change while events are in use.
            owned = bytes.to_vec();
            (&owned[..], None)
        } else {
            return Err(ParserStateError::new_err("Invalid chunk type"));
        };

        let mut sink = EventSink {
            py,
            input,
            events: Vec::new(),
            current: self.current.take(),
            error: None,
        };
        let result = self.inner.feed(data, &mut sink);
        self.current = sink.current;
        if let Some(segment) = &self.current {
            segment
                .get()
                .bytes_received
                .store(self.inner.segment_size(), Ordering::Relaxed);
        }
        if let Some(err) = sink.error {
            return Err(err);
        }
        result.map_err(|e| to_pyerr(&e))?;
        PyList::new(py, sink.events)
    }

    /// Remember the first `MultipartError` raised by this parser.
    fn remember(&mut self, py: Python<'_>, err: PyErr) -> PyErr {
        if self.error.is_none() && err.is_instance_of::<MultipartError>(py) {
            self.error = Some(err.value(py).clone().into_any().unbind());
        }
        if !self.inner.is_closed() {
            let _ = self.inner.close(false);
        }
        err
    }
}

fn to_usize(limit: Limit) -> usize {
    limit
        .0
        .map_or(usize::MAX, |v| usize::try_from(v).unwrap_or(usize::MAX))
}

fn lookup_charset(py: Python<'_>, charset: &str) -> PyResult<mp::HeaderCharset> {
    // Common names first: a codecs lookup costs more than creating a parser.
    match charset.to_ascii_lowercase().as_str() {
        "utf8" | "utf-8" => return Ok(mp::HeaderCharset::Utf8),
        "latin1" | "latin-1" | "iso-8859-1" | "iso8859-1" => return Ok(mp::HeaderCharset::Latin1),
        "ascii" | "us-ascii" => return Ok(mp::HeaderCharset::Ascii),
        _ => {}
    }
    let codecs = py.import("codecs")?;
    let info = codecs.call_method1("lookup", (charset,)).map_err(|err| {
        if err.is_instance_of::<PyLookupError>(py) {
            ParserError::new_err(format!("Invalid charset: {charset:?}"))
        } else {
            err
        }
    })?;
    let name: String = info.getattr("name")?.extract()?;
    match name.as_str() {
        "utf-8" => Ok(mp::HeaderCharset::Utf8),
        "iso8859-1" => Ok(mp::HeaderCharset::Latin1),
        "ascii" => Ok(mp::HeaderCharset::Ascii),
        _ => Err(PyValueError::new_err(format!(
            "Unsupported header charset: {charset:?} (supported: utf8, latin1, ascii)"
        ))),
    }
}

/// Unquote header option values (RFC 2616 style backslash escaping).
#[pyfunction]
#[pyo3(signature = (val, filename=false))]
fn header_unquote(val: &str, filename: bool) -> String {
    mp::header_unquote(val, filename)
}

/// Unquote field names or filenames from `Content-Disposition` headers, the
/// way modern browsers quote them (WHATWG HTML5), with support for legacy
/// backslash escaping.
#[pyfunction]
#[pyo3(signature = (val, filename=false))]
fn content_disposition_unquote(val: &str, filename: bool) -> String {
    mp::content_disposition_unquote(val, filename)
}

/// Parse a `Content-Disposition` header into a `(disposition, name,
/// filename)` tuple.
#[pyfunction]
fn parse_content_disposition(value: &str) -> (String, Option<String>, Option<String>) {
    mp::parse_content_disposition(value)
}

/// Parse an options header. `mode` selects the unquote function:
/// 0 = none (raw values), 1 = `header_unquote`, 2 = `content_disposition_unquote`.
#[pyfunction]
fn _parse_options_header(header: &str, mode: u8) -> (String, Option<Vec<(String, String)>>) {
    fn raw(val: &str, _filename: bool) -> String {
        val.to_owned()
    }
    let unquote = match mode {
        1 => mp::header_unquote,
        2 => mp::content_disposition_unquote,
        _ => raw,
    };
    mp::parse_options_header(header, unquote)
}

#[pymodule]
fn _kirimi(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add_class::<PushParser>()?;
    m.add_class::<Segment>()?;
    m.add_class::<parts::Part>()?;
    m.add_class::<parts::MultipartParser>()?;
    m.add_class::<parts::PartIterator>()?;
    m.add("MultipartError", py.get_type::<MultipartError>())?;
    m.add("ParserError", py.get_type::<ParserError>())?;
    m.add("StrictParserError", py.get_type::<StrictParserError>())?;
    m.add("ParserLimitReached", py.get_type::<ParserLimitReached>())?;
    m.add("ParserStateError", py.get_type::<ParserStateError>())?;
    m.add_function(wrap_pyfunction!(header_unquote, m)?)?;
    m.add_function(wrap_pyfunction!(content_disposition_unquote, m)?)?;
    m.add_function(wrap_pyfunction!(parse_content_disposition, m)?)?;
    m.add_function(wrap_pyfunction!(_parse_options_header, m)?)?;
    Ok(())
}
