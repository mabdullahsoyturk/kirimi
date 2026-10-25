use std::fmt;

/// Error categories, mirroring the exception hierarchy of the Python API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Input is not a valid multipart stream.
    Parser,
    /// Unusual input detected in strict mode (a subtype of `Parser`).
    Strict,
    /// One of the configured limits was exceeded.
    LimitReached,
    /// The parser was used incorrectly (e.g. use after close).
    State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: &'static str,
}

impl Error {
    pub(crate) const fn parser(message: &'static str) -> Self {
        Self {
            kind: ErrorKind::Parser,
            message,
        }
    }

    pub(crate) const fn strict(message: &'static str) -> Self {
        Self {
            kind: ErrorKind::Strict,
            message,
        }
    }

    pub(crate) const fn limit(message: &'static str) -> Self {
        Self {
            kind: ErrorKind::LimitReached,
            message,
        }
    }

    pub(crate) const fn state(message: &'static str) -> Self {
        Self {
            kind: ErrorKind::State,
            message,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for Error {}
