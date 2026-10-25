//! Sans-IO streaming parser for `multipart/form-data` (RFC 7578).
//!
//! The parser does no IO on its own. Feed it chunks of input with
//! [`Parser::feed`] and it reports events to a [`Sink`]: one [`Segment`] per
//! part (headers and metadata), zero or more non-empty body chunks, and an
//! end-of-segment marker. Body chunks borrow from the input chunk whenever
//! possible, so large uploads are passed through without copying.
//!
//! Behavior intentionally follows the `multipart` Python library
//! (<https://github.com/defnull/multipart>) closely, including limits and
//! error messages, so the two can be tested against each other.

mod error;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
mod headers;
mod parser;
mod search;

pub use error::{Error, ErrorKind};
pub use headers::{
    Segment, content_disposition_unquote, header_unquote, parse_content_disposition,
    parse_options_header,
};
pub use parser::{Config, HeaderCharset, Parser, Sink};
