//! Buffered blocking parser: `MultipartPart` and the `_PartReader` iterator
//! behind `MultipartParser`.
//!
//! The reader runs the whole read → parse → spool loop in Rust. Part bodies
//! stay in memory as ranges of the input chunks (small pieces are copied)
//! until they exceed the spool limit, then they are written to a temporary
//! file without further copies. In-memory parts only become a `BytesIO` when
//! their `file` attribute is accessed, so `raw` and `value` are served from
//! memory directly.

use std::collections::VecDeque;

use kirimi_core as mp;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyByteArray, PyBytes, PyList, PyMemoryView, PyModule, PySlice, PyString};

use crate::{
    Limit, MultipartError, ParserLimitReached, ParserStateError, Segment, lookup_charset, to_pyerr,
};

/// Content of a part.
enum Body {
    /// Buffered in memory; not exposed as a file object yet.
    Memory(Memory),
    /// A file object: a materialized `BytesIO`, or a temporary file.
    File(Py<PyAny>),
    /// The part was closed.
    Closed,
}

/// Body data that lives in an immutable input chunk, so it can be kept or
/// written without copying.
#[derive(Clone, Copy)]
struct Source<'a, 'py> {
    chunk: &'a Bound<'py, PyBytes>,
    start: usize,
    end: usize,
}

impl<'py> Source<'_, 'py> {
    /// The data as a Python object: the chunk itself, or a memoryview of it.
    fn object(&self) -> PyResult<Bound<'py, PyAny>> {
        shared_object(self.chunk, self.start, self.end)
    }
}

fn shared_object<'py>(
    chunk: &Bound<'py, PyBytes>,
    start: usize,
    end: usize,
) -> PyResult<Bound<'py, PyAny>> {
    let py = chunk.py();
    if start == 0 && end == chunk.as_bytes().len() {
        return Ok(chunk.clone().into_any());
    }
    let view = PyMemoryView::from(chunk.as_any())?;
    // Slices of allocations always fit into isize.
    let index = |i: usize| isize::try_from(i).expect("slice index fits into isize");
    view.get_item(PySlice::new(py, index(start), index(end), 1))
}

/// In-memory content, in order. Large pieces are kept as ranges of the input
/// chunks they arrived in, so they are never copied: not while buffering,
/// and not when spooling to disk. Small pieces are merged into buffers.
#[derive(Default)]
struct Memory {
    pieces: Vec<Piece>,
    len: usize,
}

enum Piece {
    Shared {
        chunk: Py<PyBytes>,
        start: usize,
        end: usize,
    },
    Owned(Vec<u8>),
}

/// Pieces at least this large are shared instead of copied.
const SHARE_MIN: usize = 4096;

impl Memory {
    fn push(&mut self, data: &[u8], source: Option<Source<'_, '_>>) {
        self.len += data.len();
        match (source, self.pieces.last_mut()) {
            (Some(src), _) if data.len() >= SHARE_MIN => self.pieces.push(Piece::Shared {
                chunk: src.chunk.clone().unbind(),
                start: src.start,
                end: src.end,
            }),
            (_, Some(Piece::Owned(buf))) => buf.extend_from_slice(data),
            _ => self.pieces.push(Piece::Owned(data.to_vec())),
        }
    }

    /// Return the content as one `bytes` object, joining pieces once.
    fn bytes(&mut self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        if let [
            Piece::Shared {
                chunk,
                start: 0,
                end,
            },
        ] = self.pieces.as_slice()
            && *end == chunk.as_bytes(py).len()
        {
            return Ok(chunk.clone_ref(py));
        }
        let joined = PyBytes::new_with(py, self.len, |out| {
            let mut offset = 0;
            for piece in &self.pieces {
                let data = piece.as_bytes(py);
                out[offset..offset + data.len()].copy_from_slice(data);
                offset += data.len();
            }
            Ok(())
        })?
        .unbind();
        self.pieces = vec![Piece::Shared {
            chunk: joined.clone_ref(py),
            start: 0,
            end: self.len,
        }];
        Ok(joined)
    }

    /// Write all pieces to a file object.
    fn write_to(&self, py: Python<'_>, file: &Bound<'_, PyAny>) -> PyResult<()> {
        for piece in &self.pieces {
            let obj = match piece {
                Piece::Shared { chunk, start, end } => shared_object(chunk.bind(py), *start, *end)?,
                Piece::Owned(buf) => PyBytes::new(py, buf).into_any(),
            };
            file.call_method1(intern!(py, "write"), (obj,))?;
        }
        Ok(())
    }
}

impl Piece {
    fn as_bytes<'a>(&'a self, py: Python<'_>) -> &'a [u8] {
        match self {
            Piece::Shared { chunk, start, end } => &chunk.as_bytes(py)[*start..*end],
            Piece::Owned(buf) => buf,
        }
    }
}

/// A fully parsed multipart part with access to its headers, metadata and
/// memory- or disk-buffered content.
#[pyclass(module = "kirimi", name = "MultipartPart", subclass, dict)]
pub struct Part {
    /// Part name.
    #[pyo3(get, set)]
    name: Py<PyAny>,
    /// Part filename (if defined).
    #[pyo3(get, set)]
    filename: Py<PyAny>,
    /// Charset as defined in the part header, or the parser default charset.
    #[pyo3(get, set)]
    charset: Py<PyAny>,
    /// Part size in bytes.
    #[pyo3(get, set)]
    size: u64,
    #[pyo3(get, set)]
    memfile_limit: u64,
    #[pyo3(get, set)]
    buffer_size: usize,
    headers: Vec<(String, String)>,
    content_type: Option<String>,
    headerlist: Option<Py<PyAny>>,
    headers_obj: Option<Py<PyAny>>,
    body: Body,
}

impl Part {
    fn from_segment(
        py: Python<'_>,
        segment: mp::Segment,
        default_charset: &Py<PyString>,
        memfile_limit: u64,
        buffer_size: usize,
    ) -> Self {
        let name = segment.name.unwrap_or_default();
        Part {
            name: PyString::new(py, &name).into_any().unbind(),
            filename: segment
                .filename
                .map_or_else(|| py.None(), |f| PyString::new(py, &f).into_any().unbind()),
            charset: segment.charset.map_or_else(
                || default_charset.clone_ref(py).into_any(),
                |c| PyString::new(py, &c).into_any().unbind(),
            ),
            size: 0,
            memfile_limit,
            buffer_size,
            headers: segment.headers,
            content_type: segment.content_type,
            headerlist: None,
            headers_obj: None,
            body: Body::Memory(Memory::default()),
        }
    }

    /// Append body data. `source`, if given, is where `data` lives in an
    /// immutable input chunk.
    fn write(
        &mut self,
        py: Python<'_>,
        data: &[u8],
        source: Option<Source<'_, '_>>,
        tempfile: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.size += data.len() as u64;
        match &mut self.body {
            Body::Memory(memory) => {
                memory.push(data, source);
                if self.size > self.memfile_limit {
                    let file = tempfile.call0()?;
                    memory.write_to(py, &file)?;
                    self.body = Body::File(file.unbind());
                }
            }
            Body::File(file) => {
                let obj = match source {
                    Some(src) => src.object()?,
                    None => PyBytes::new(py, data).into_any(),
                };
                file.bind(py).call_method1(intern!(py, "write"), (obj,))?;
            }
            Body::Closed => {
                return Err(MultipartError::new_err(
                    "Cannot write to closed MultipartPart",
                ));
            }
        }
        Ok(())
    }

    fn mark_complete(&self, py: Python<'_>) -> PyResult<()> {
        if let Body::File(file) = &self.body {
            file.bind(py).call_method1(intern!(py, "seek"), (0,))?;
        }
        Ok(())
    }

    fn in_memory(&self) -> bool {
        matches!(self.body, Body::Memory(_))
    }

    fn find_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn bytes_io_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static BYTES_IO: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    BYTES_IO.import(py, "io", "BytesIO")
}

fn closed_error() -> PyErr {
    MultipartError::new_err("Cannot read from closed MultipartPart")
}

#[pymethods]
impl Part {
    #[new]
    #[pyo3(signature = (segment, buffer_size=1 << 16, memfile_limit=Limit(Some(1 << 18)), charset="utf8"))]
    fn new(
        py: Python<'_>,
        segment: &Bound<'_, Segment>,
        buffer_size: usize,
        memfile_limit: Limit,
        charset: &str,
    ) -> Self {
        let segment = segment.get();
        let core_segment = mp::Segment {
            headers: segment.headers.clone(),
            disposition: segment.disposition.clone(),
            name: segment.name.clone(),
            filename: segment.filename.clone(),
            content_type: segment.content_type.clone(),
            charset: segment.charset.clone(),
        };
        Part::from_segment(
            py,
            core_segment,
            &PyString::new(py, charset).unbind(),
            memfile_limit.0.unwrap_or(u64::MAX),
            buffer_size,
        )
    }

    /// A file-like buffer holding the part content, or `None` if this part
    /// was closed.
    #[getter]
    fn file(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &mut self.body {
            Body::Memory(memory) => {
                let file = bytes_io_type(py)?.call1((memory.bytes(py)?,))?.unbind();
                self.body = Body::File(file.clone_ref(py));
                Ok(file)
            }
            Body::File(file) => Ok(file.clone_ref(py)),
            Body::Closed => Ok(py.None()),
        }
    }

    #[setter]
    fn set_file(&mut self, file: Option<Py<PyAny>>) {
        self.body = match file {
            Some(file) => Body::File(file),
            None => Body::Closed,
        };
    }

    /// All part headers as a list of `(name, value)` pairs.
    #[getter]
    fn headerlist(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        if self.headerlist.is_none() {
            let list = PyList::new(
                py,
                self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            )?;
            self.headerlist = Some(list.into_any().unbind());
        }
        Ok(self.headerlist.as_ref().expect("set above").clone_ref(py))
    }

    #[setter]
    fn set_headerlist(&mut self, headerlist: Py<PyAny>) {
        self.headerlist = Some(headerlist);
        self.headers_obj = None;
    }

    /// A convenient dict-like holding all part headers.
    #[getter]
    fn headers(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        if self.headers_obj.is_none() {
            static HEADERS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
            let headers = HEADERS
                .import(py, "wsgiref.headers", "Headers")?
                .call1((self.headerlist(py)?,))?;
            self.headers_obj = Some(headers.unbind());
        }
        Ok(self.headers_obj.as_ref().expect("set above").clone_ref(py))
    }

    /// The value of the `Content-Disposition` part header.
    #[getter]
    fn disposition(&self) -> Option<&str> {
        self.find_header("Content-Disposition")
    }

    /// Cleaned up content type provided for this part, or a sensible default
    /// (`application/octet-stream` for files and `text/plain` for text
    /// fields).
    #[getter]
    fn content_type(&self, py: Python<'_>) -> PyResult<String> {
        if let Some(content_type) = &self.content_type {
            return Ok(content_type.clone());
        }
        Ok(if self.filename.bind(py).is_truthy()? {
            "application/octet-stream".to_owned()
        } else {
            "text/plain".to_owned()
        })
    }

    /// Return true if `file` is memory-buffered, or false if the part was
    /// spooled to a temporary file on disk.
    fn is_buffered(&self, py: Python<'_>) -> PyResult<bool> {
        match &self.body {
            Body::Memory(_) => Ok(true),
            Body::File(file) => file.bind(py).is_instance(bytes_io_type(py)?),
            Body::Closed => Ok(false),
        }
    }

    /// The entire payload as a raw byte string.
    #[getter]
    fn raw(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &mut self.body {
            Body::Memory(memory) => Ok(memory.bytes(py)?.into_any()),
            Body::File(file) => {
                let file = file.bind(py);
                let pos = file.call_method0(intern!(py, "tell"))?;
                file.call_method1(intern!(py, "seek"), (0,))?;
                let value = file.call_method0(intern!(py, "read"));
                file.call_method1(intern!(py, "seek"), (pos,))?;
                Ok(value?.unbind())
            }
            Body::Closed => Err(closed_error()),
        }
    }

    /// The entire payload as a decoded text string.
    #[getter]
    fn value(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let charset = self.charset.clone_ref(py);
        Ok(self
            .raw(py)?
            .bind(py)
            .call_method1(intern!(py, "decode"), (charset,))?
            .unbind())
    }

    /// Save a copy of this part to `path` and return the number of bytes
    /// written.
    fn save_as(&mut self, py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<u64> {
        static OPEN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
        let open = OPEN.import(py, "io", "open")?;
        let source = match &mut self.body {
            Body::Memory(memory) => {
                let bytes = memory.bytes(py)?;
                let fp = open.call1((path, "wb"))?;
                let result = fp.call_method1(intern!(py, "write"), (bytes.bind(py),));
                fp.call_method0(intern!(py, "close"))?;
                result?;
                return Ok(bytes.bind(py).as_bytes().len() as u64);
            }
            Body::File(file) => file.bind(py).clone(),
            Body::Closed => return Err(closed_error()),
        };

        let fp = open.call1((path, "wb"))?;
        let pos = source.call_method0(intern!(py, "tell"))?;
        let copy = || -> PyResult<u64> {
            source.call_method1(intern!(py, "seek"), (0,))?;
            let mut size = 0u64;
            loop {
                let chunk = source.call_method1(intern!(py, "read"), (self.buffer_size,))?;
                let n = chunk.len()?;
                if n == 0 {
                    return Ok(size);
                }
                fp.call_method1(intern!(py, "write"), (chunk,))?;
                size += n as u64;
            }
        };
        let result = copy();
        let restored = source.call_method1(intern!(py, "seek"), (pos,));
        let closed = fp.call_method0(intern!(py, "close"));
        let size = result?;
        restored?;
        closed?;
        Ok(size)
    }

    /// Close `file` and set it to `None` to free up resources.
    fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        if let Body::File(file) = std::mem::replace(&mut self.body, Body::Closed) {
            file.bind(py).call_method0(intern!(py, "close"))?;
        }
        Ok(())
    }

    // Private methods kept for compatibility with `multipart`.

    fn _write(&mut self, py: Python<'_>, chunk: &Bound<'_, PyAny>) -> PyResult<()> {
        let tempfile = &temporary_file_factory(py)?;
        if let Ok(bytes) = chunk.cast::<PyBytes>() {
            let data = bytes.as_bytes();
            let source = Source {
                chunk: bytes,
                start: 0,
                end: data.len(),
            };
            return self.write(py, data, Some(source), tempfile);
        }
        let data: Vec<u8> = chunk.extract()?;
        self.write(py, &data, None, tempfile)
    }

    fn _mark_complete(&self, py: Python<'_>) -> PyResult<()> {
        self.mark_complete(py)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let size = match &self.body {
            Body::Memory(memory) => memory.len as u64,
            _ => self.size,
        };
        Ok(format!(
            "<MultipartPart name={} filename={} size={}>",
            self.name.bind(py).repr()?,
            self.filename.bind(py).repr()?,
            size
        ))
    }
}

/// Spooling state shared by the parser callbacks.
struct Spooler {
    spool_limit: u64,
    memory_limit: Option<u64>,
    disk_limit: Option<u64>,
    charset: Py<PyString>,
    buffer_size: usize,
    tempfile: Py<PyAny>,
    current: Option<Part>,
    ready: VecDeque<Py<Part>>,
    memory_used: u64,
    disk_used: u64,
}

/// Feeds parser events into the spooler. Python errors are kept and stop
/// further processing; the reader raises them after the chunk.
struct SpoolSink<'a, 'py> {
    py: Python<'py>,
    spool: &'a mut Spooler,
    /// The input chunk, if it is an immutable `bytes` object whose data can
    /// be shared instead of copied.
    chunk: Option<&'a Bound<'py, PyBytes>>,
    error: Option<PyErr>,
}

impl SpoolSink<'_, '_> {
    fn data_inner(&mut self, data: &[u8]) -> PyResult<()> {
        let py = self.py;
        let source = self.chunk.and_then(|chunk| {
            let base = chunk.as_bytes().as_ptr_range();
            let range = data.as_ptr_range();
            (base.start <= range.start && range.end <= base.end).then(|| {
                let start = range.start as usize - base.start as usize;
                Source {
                    chunk,
                    start,
                    end: start + data.len(),
                }
            })
        });
        let spool = &mut *self.spool;
        let part = spool.current.as_mut().expect("data outside of a part");
        part.write(py, data, source, spool.tempfile.bind(py))?;
        if part.in_memory() {
            if spool
                .memory_limit
                .is_some_and(|limit| part.size + spool.memory_used > limit)
            {
                return Err(ParserLimitReached::new_err("Memory limit reached"));
            }
        } else if spool
            .disk_limit
            .is_some_and(|limit| part.size + spool.disk_used > limit)
        {
            return Err(ParserLimitReached::new_err("Disk limit reached"));
        }
        Ok(())
    }

    fn end_inner(&mut self) -> PyResult<()> {
        let py = self.py;
        let spool = &mut *self.spool;
        let part = spool.current.take().expect("end outside of a part");
        if part.in_memory() {
            spool.memory_used += part.size;
        } else {
            spool.disk_used += part.size;
        }
        part.mark_complete(py)?;
        spool.ready.push_back(Py::new(py, part)?);
        Ok(())
    }
}

impl mp::Sink for SpoolSink<'_, '_> {
    fn segment(&mut self, segment: mp::Segment) {
        if self.error.is_none() {
            let spool = &mut *self.spool;
            spool.current = Some(Part::from_segment(
                self.py,
                segment,
                &spool.charset,
                spool.spool_limit,
                spool.buffer_size,
            ));
        }
    }

    fn data(&mut self, data: &[u8]) {
        if self.error.is_none()
            && let Err(err) = self.data_inner(data)
        {
            self.error = Some(err);
        }
    }

    fn segment_end(&mut self, _size: u64) {
        if self.error.is_none()
            && let Err(err) = self.end_inner()
        {
            self.error = Some(err);
        }
    }
}

/// The factory for temporary files, looked up on every use so that it can be
/// replaced (e.g. in tests).
fn temporary_file_factory(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    static MODULE: PyOnceLock<Py<PyModule>> = PyOnceLock::new();
    let module =
        MODULE.get_or_try_init(py, || py.import("kirimi._multipart").map(Bound::unbind))?;
    module.bind(py).getattr(intern!(py, "_temporary_file"))
}

/// Reads from a blocking stream, parses and spools parts.
struct PartReader {
    parser: mp::Parser,
    read: Py<PyAny>,
    chunk_size: usize,
    /// Bytes left to read, or `None` if unknown.
    remaining: Option<u64>,
    spool: Spooler,
    done: bool,
    /// An error to raise once the parts completed before it were returned.
    error: Option<PyErr>,
}

impl PartReader {
    fn new(py: Python<'_>, options: &MultipartParser) -> PyResult<Self> {
        if options.buffer_size == 0 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "buffer_size must be positive",
            ));
        }
        let read = options
            .stream
            .bind(py)
            .getattr(intern!(py, "read"))?
            .unbind();
        let boundary = options.boundary.bind(py);
        let boundary: Vec<u8> = match boundary.cast::<PyString>() {
            Ok(text) => text.to_str()?.as_bytes().to_vec(),
            Err(_) => boundary.extract()?,
        };
        let content_length = u64::try_from(options.content_length).ok();
        let config = mp::Config {
            max_header_size: crate::to_usize(options.headersize_limit),
            max_header_count: crate::to_usize(options.header_limit),
            max_segment_size: options.partsize_limit.0,
            max_segment_count: options.part_limit.0,
            content_length,
            header_charset: lookup_charset(py, &options.charset)?,
            strict: options.strict,
        };
        let parser = mp::Parser::new(&boundary, config).map_err(|e| to_pyerr(&e))?;
        Ok(Self {
            parser,
            read,
            chunk_size: options.buffer_size,
            remaining: content_length,
            spool: Spooler {
                spool_limit: options.spool_limit.0.unwrap_or(u64::MAX),
                memory_limit: options.memory_limit.0,
                disk_limit: options.disk_limit.0,
                charset: PyString::new(py, &options.charset).unbind(),
                buffer_size: options.buffer_size,
                tempfile: temporary_file_factory(py)?.unbind(),
                current: None,
                ready: VecDeque::new(),
                memory_used: 0,
                disk_used: 0,
            },
            done: false,
            error: None,
        })
    }

    /// Return the next complete part, or `None` at the end of the stream.
    fn next_part(&mut self, py: Python<'_>) -> PyResult<Option<Py<Part>>> {
        loop {
            if let Some(part) = self.spool.ready.pop_front() {
                return Ok(Some(part));
            }
            if let Some(err) = self.error.take() {
                return Err(err);
            }
            if self.done {
                return Ok(None);
            }
            if let Err(err) = self.step(py) {
                self.done = true;
                self.spool.current = None;
                let _ = self.parser.close(false);
                self.error = Some(err);
            }
        }
    }

    /// Read and parse one chunk.
    fn step(&mut self, py: Python<'_>) -> PyResult<()> {
        let size = match self.remaining {
            Some(remaining) => {
                usize::try_from(remaining).map_or(self.chunk_size, |r| r.min(self.chunk_size))
            }
            None => self.chunk_size,
        };
        let chunk = self.read.bind(py).call1((size,))?;

        let owned;
        let data: &[u8] = if let Ok(bytes) = chunk.cast::<PyBytes>() {
            bytes.as_bytes()
        } else if let Ok(bytes) = chunk.cast::<PyByteArray>() {
            owned = bytes.to_vec();
            &owned
        } else {
            return Err(ParserStateError::new_err("Invalid chunk type"));
        };

        if data.is_empty() {
            self.done = true;
            return self.parser.close(true).map_err(|e| to_pyerr(&e));
        }
        if let Some(remaining) = &mut self.remaining {
            *remaining = remaining.saturating_sub(data.len() as u64);
        }

        let mut sink = SpoolSink {
            py,
            spool: &mut self.spool,
            chunk: chunk.cast::<PyBytes>().ok(),
            error: None,
        };
        let result = self.parser.feed(data, &mut sink);
        if let Some(err) = sink.error {
            return Err(err);
        }
        result.map_err(|e| to_pyerr(&e))
    }
}

/// `min()` for limits, where `None` means unlimited.
fn min_limit(a: Limit, b: Limit) -> Limit {
    match (a.0, b.0) {
        (Some(a), Some(b)) => Limit(Some(a.min(b))),
        (Some(a), None) => Limit(Some(a)),
        (None, b) => Limit(b),
    }
}

/// `limit or default`: a missing or zero limit means "use the default", as
/// in `multipart`.
fn limit_or(limit: Option<Limit>, default: Limit) -> Limit {
    match limit {
        Some(Limit(Some(0))) | None => default,
        Some(limit) => limit,
    }
}

/// A parser that reads from a `multipart/form-data` encoded byte stream and
/// yields buffered `MultipartPart` instances.
///
/// The parser is a lazy iterator and reads only as much data as needed to
/// return the next part. Results are cached, so iterating again is cheap.
///
/// Parts up to `spool_limit` bytes are buffered in memory and count towards
/// `memory_limit`. Larger parts are spooled to temporary files and count
/// towards `disk_limit`.
#[pyclass(module = "kirimi", name = "MultipartParser", subclass, dict)]
pub struct MultipartParser {
    /// A readable byte stream with a `read(size)` method.
    #[pyo3(get, set)]
    stream: Py<PyAny>,
    /// The multipart boundary from the Content-Type header.
    #[pyo3(get, set)]
    boundary: Py<PyAny>,
    #[pyo3(get, set)]
    content_length: i64,
    /// Default charset for headers and text fields.
    #[pyo3(get, set)]
    charset: String,
    #[pyo3(get, set)]
    strict: bool,
    /// Chunk size when reading from the stream.
    #[pyo3(get, set)]
    buffer_size: usize,
    header_limit: Limit,
    headersize_limit: Limit,
    part_limit: Limit,
    partsize_limit: Limit,
    spool_limit: Limit,
    memory_limit: Limit,
    disk_limit: Limit,
    done: Vec<Py<Part>>,
    /// Created on first iteration, so that errors surface there.
    reader: Option<PartReader>,
}

#[pymethods]
impl MultipartParser {
    #[new]
    #[pyo3(signature = (
        stream,
        boundary,
        content_length=-1,
        charset="utf8".to_owned(),
        strict=false,
        buffer_size=1 << 16,
        header_limit=Limit(Some(8)),
        headersize_limit=Limit(Some(4096 + 128)),
        part_limit=Limit(Some(128)),
        partsize_limit=Limit(None),
        spool_limit=Limit(Some(1 << 16)),
        memory_limit=Limit(Some(1 << 23)),
        disk_limit=Limit(None),
        mem_limit=None,
        memfile_limit=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        stream: Py<PyAny>,
        boundary: Py<PyAny>,
        content_length: i64,
        charset: String,
        strict: bool,
        buffer_size: usize,
        header_limit: Limit,
        headersize_limit: Limit,
        part_limit: Limit,
        partsize_limit: Limit,
        spool_limit: Limit,
        memory_limit: Limit,
        disk_limit: Limit,
        mem_limit: Option<Limit>,
        memfile_limit: Option<Limit>,
    ) -> Self {
        // Same as in `multipart`: the old `mem_limit` and `memfile_limit`
        // names override the new ones unless they are zero.
        let memory_limit = limit_or(mem_limit, memory_limit);
        let spool_limit = min_limit(limit_or(memfile_limit, spool_limit), memory_limit);
        Self {
            stream,
            boundary,
            content_length,
            charset,
            strict,
            buffer_size,
            header_limit,
            headersize_limit,
            part_limit,
            partsize_limit,
            spool_limit,
            memory_limit,
            disk_limit,
            done: Vec::new(),
            reader: None,
        }
    }

    #[getter]
    fn header_limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.header_limit.to_object(py)
    }

    #[setter]
    fn set_header_limit(&mut self, value: Limit) {
        self.header_limit = value;
    }

    #[getter]
    fn headersize_limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.headersize_limit.to_object(py)
    }

    #[setter]
    fn set_headersize_limit(&mut self, value: Limit) {
        self.headersize_limit = value;
    }

    #[getter]
    fn part_limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.part_limit.to_object(py)
    }

    #[setter]
    fn set_part_limit(&mut self, value: Limit) {
        self.part_limit = value;
    }

    #[getter]
    fn partsize_limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.partsize_limit.to_object(py)
    }

    #[setter]
    fn set_partsize_limit(&mut self, value: Limit) {
        self.partsize_limit = value;
    }

    #[getter]
    fn spool_limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.spool_limit.to_object(py)
    }

    #[setter]
    fn set_spool_limit(&mut self, value: Limit) {
        self.spool_limit = value;
    }

    #[getter]
    fn memory_limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_limit.to_object(py)
    }

    #[setter]
    fn set_memory_limit(&mut self, value: Limit) {
        self.memory_limit = value;
    }

    #[getter]
    fn disk_limit(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.disk_limit.to_object(py)
    }

    #[setter]
    fn set_disk_limit(&mut self, value: Limit) {
        self.disk_limit = value;
    }

    /// Parse the multipart stream and yield `MultipartPart` instances as
    /// soon as they are available.
    fn __iter__(slf: Py<Self>) -> PartIterator {
        PartIterator {
            parser: slf,
            index: 0,
        }
    }

    /// Parse the entire multipart stream and return all parts as a list.
    fn parts(&mut self, py: Python<'_>) -> PyResult<Vec<Py<Part>>> {
        while self.fetch(py)?.is_some() {}
        Ok(self.done.iter().map(|part| part.clone_ref(py)).collect())
    }

    /// Return the first part with a given name, or the default value if no
    /// matching part exists.
    #[pyo3(signature = (name, default=None))]
    fn get(
        &mut self,
        py: Python<'_>,
        name: &Bound<'_, PyAny>,
        default: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let mut index = 0;
        loop {
            let part = match self.done.get(index) {
                Some(part) => part.clone_ref(py),
                None => match self.fetch(py)? {
                    Some(part) => part,
                    None => return Ok(default.unwrap_or_else(|| py.None())),
                },
            };
            if name.eq(&part.borrow(py).name)? {
                return Ok(part.into_any());
            }
            index += 1;
        }
    }

    /// Return all parts with the given name.
    fn get_all(&mut self, py: Python<'_>, name: &Bound<'_, PyAny>) -> PyResult<Vec<Py<Part>>> {
        while self.fetch(py)?.is_some() {}
        let mut matches = Vec::new();
        for part in &self.done {
            if part.borrow(py).name.bind(py).eq(name)? {
                matches.push(part.clone_ref(py));
            }
        }
        Ok(matches)
    }
}

impl MultipartParser {
    /// Parse the next part and add it to `done`.
    fn fetch(&mut self, py: Python<'_>) -> PyResult<Option<Py<Part>>> {
        if self.reader.is_none() {
            self.reader = Some(PartReader::new(py, self)?);
        }
        let reader = self.reader.as_mut().expect("created above");
        let part = reader.next_part(py)?;
        if let Some(part) = &part {
            self.done.push(part.clone_ref(py));
        }
        Ok(part)
    }
}

/// Iterator returned by `MultipartParser.__iter__`: first the parts parsed so
/// far, then new ones.
#[pyclass(module = "kirimi", name = "_PartIterator")]
pub struct PartIterator {
    parser: Py<MultipartParser>,
    index: usize,
}

#[pymethods]
impl PartIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<Part>>> {
        let mut parser = self.parser.borrow_mut(py);
        let part = match parser.done.get(self.index) {
            Some(part) => Some(part.clone_ref(py)),
            None => parser.fetch(py)?,
        };
        if part.is_some() {
            self.index += 1;
        }
        Ok(part)
    }
}
