//! Header parsing helpers.
//!
//! These mirror the corresponding functions of the `multipart` Python library,
//! including its legacy quirks (IE6 full-path filenames, WHATWG percent-encoding
//! in `Content-Disposition`), so both libraries agree on names and filenames.

use std::borrow::Cow;

/// Header names that are known to be valid and already normalized.
pub(crate) const KNOWN_HEADERS: [&str; 4] = [
    "Content-Disposition",
    "Content-Type",
    "Content-Length",
    "Content-Range",
];

/// Header metadata of a single multipart segment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Segment {
    /// Header `(name, value)` pairs in input order. Names are Title-Case,
    /// values have leading whitespace removed.
    pub headers: Vec<(String, String)>,
    /// Lower-cased `Content-Disposition` value without options.
    pub disposition: Option<String>,
    /// The `name` option of the `Content-Disposition` header.
    pub name: Option<String>,
    /// The `filename` option of the `Content-Disposition` header.
    pub filename: Option<String>,
    /// Lower-cased `Content-Type` value without options.
    pub content_type: Option<String>,
    /// The `charset` option of the `Content-Type` header.
    pub charset: Option<String>,
}

impl Segment {
    pub(crate) fn from_headers(headers: Vec<(String, String)>) -> Self {
        let mut disposition = None;
        let mut name = None;
        let mut filename = None;
        let mut content_type = None;
        let mut charset = None;

        // Later headers override earlier ones, same as in `multipart`.
        for (header, value) in &headers {
            match header.as_str() {
                "Content-Disposition" => {
                    let parsed = parse_content_disposition(value);
                    disposition = Some(parsed.0);
                    name = parsed.1;
                    filename = parsed.2;
                }
                "Content-Type" => {
                    let (value, options) = parse_options_header(value, header_unquote);
                    content_type = Some(value);
                    charset = options.and_then(|options| last_option(options, "charset"));
                }
                _ => {}
            }
        }

        Segment {
            headers,
            disposition,
            name,
            filename,
            content_type,
            charset,
        }
    }
}

/// Return the last value for `key`, which is what a Python dict built from
/// the same pairs would hold.
fn last_option(options: Vec<(String, String)>, key: &str) -> Option<String> {
    options
        .into_iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// Whitespace as understood by Python's `str.strip()`.
pub(crate) fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Normalize a header name the same way as `name.strip().title()` and
/// validate it against `[a-zA-Z0-9-_]+`.
///
/// Non-ASCII names are rejected. (Python's `title()` maps a handful of exotic
/// characters such as `ß` to ASCII, which we do not replicate.)
pub(crate) fn normalize_header_name(name: &str) -> Option<String> {
    if KNOWN_HEADERS.contains(&name) {
        return Some(name.to_owned());
    }
    let name = name.trim_matches(is_py_space);
    if name.is_empty() || !name.bytes().all(is_hname_byte) {
        return None;
    }
    let mut out = String::with_capacity(name.len());
    let mut prev_cased = false;
    for b in name.bytes() {
        if b.is_ascii_alphabetic() {
            out.push(if prev_cased {
                b.to_ascii_lowercase()
            } else {
                b.to_ascii_uppercase()
            } as char);
            prev_cased = true;
        } else {
            out.push(b as char);
            prev_cased = false;
        }
    }
    Some(out)
}

fn is_hname_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-!#$%&'*+.^_`|~".contains(&b)
}

/// Strip one pair of surrounding double quotes, if present.
fn strip_quotes(val: &str) -> Option<&str> {
    if val.starts_with('"') && val.ends_with('"') {
        Some(if val.len() == 1 {
            ""
        } else {
            &val[1..val.len() - 1]
        })
    } else {
        None
    }
}

/// IE6 and some Windows clients send a full path instead of a filename.
fn is_windows_path(val: &str) -> bool {
    let mut chars = val.chars().skip(1);
    (chars.next() == Some(':') && chars.next() == Some('\\')) || val.starts_with("\\\\")
}

/// Limited percent-decoding as specified by WHATWG for form-data names.
fn percent_unquote(val: &str) -> String {
    val.replace("%0D", "\r")
        .replace("%0A", "\n")
        .replace("%22", "\"")
}

/// Unquote header option values (RFC 2616 style backslash escaping).
#[must_use]
pub fn header_unquote(val: &str, filename: bool) -> String {
    match strip_quotes(val) {
        Some(mut inner) => {
            if filename && is_windows_path(inner) {
                inner = inner.rsplit('\\').next().unwrap_or(inner);
            }
            inner.replace("\\\\", "\\").replace("\\\"", "\"")
        }
        None => val.to_owned(),
    }
}

/// Unquote field names or filenames from `Content-Disposition` headers.
///
/// Modern browsers follow the WHATWG HTML5 specification (limited
/// percent-encoding, no backslash escaping). Legacy backslash escaping is
/// detected and supported as well.
#[must_use]
pub fn content_disposition_unquote(val: &str, filename: bool) -> String {
    let mut val: Cow<'_, str> = match strip_quotes(val) {
        Some(inner) if inner.contains("\\\"") => {
            Cow::Owned(inner.replace("\\\\", "\\").replace("\\\"", "\""))
        }
        Some(inner) if inner.contains('%') => Cow::Owned(percent_unquote(inner)),
        Some(inner) => Cow::Borrowed(inner),
        None if val.contains('%') => Cow::Owned(percent_unquote(val)),
        None => Cow::Borrowed(val),
    };
    if filename && is_windows_path(&val) {
        let base = val.rsplit('\\').next().unwrap_or_default().to_owned();
        val = Cow::Owned(base);
    }
    val.into_owned()
}

/// Lower-case and strip a header value the same way as Python's
/// `value.lower().strip()`.
fn py_lower_strip(val: &str) -> String {
    val.to_lowercase().trim_matches(is_py_space).to_owned()
}

/// Parse a `Content-Type` (or similar) header into a primary value and a list
/// of `(key, value)` options in input order. Keys are lower-cased; duplicate
/// keys are kept (the last one wins when building a dict).
///
/// Returns `None` for the options if the header has no `;` at all.
pub fn parse_options_header(
    header: &str,
    unquote: fn(&str, bool) -> String,
) -> (String, Option<Vec<(String, String)>>) {
    let Some(i) = header.find(';') else {
        return (py_lower_strip(header), None);
    };
    let options = scan_options(header, i)
        .map(|(key, val)| {
            let key = key.to_ascii_lowercase();
            let val = unquote(val, key == "filename");
            (key, val)
        })
        .collect();
    (py_lower_strip(&header[..i]), Some(options))
}

/// Specialized parser for `Content-Disposition` headers. Returns
/// `(disposition, name, filename)`.
pub fn parse_content_disposition(value: &str) -> (String, Option<String>, Option<String>) {
    // Fast path for the strict format emitted by modern browsers.
    let split: Vec<&str> = value.splitn(5, '"').collect();
    if split[0] == "form-data; name=" && split.last() == Some(&"") {
        let unquote_name = |name: &str| {
            if name.contains('%') {
                content_disposition_unquote(name, false)
            } else {
                name.to_owned()
            }
        };
        if split.len() == 3 {
            return ("form-data".to_owned(), Some(unquote_name(split[1])), None);
        }
        if split.len() == 5 && split[2] == "; filename=" {
            let filename = if split[3].contains(['%', '\\']) {
                content_disposition_unquote(split[3], true)
            } else {
                split[3].to_owned()
            };
            return (
                "form-data".to_owned(),
                Some(unquote_name(split[1])),
                Some(filename),
            );
        }
    }
    // Slow path for legacy browsers or non-browser clients.
    let (disposition, options) = parse_options_header(value, content_disposition_unquote);
    let options = options.unwrap_or_default();
    let mut name = None;
    let mut filename = None;
    for (key, val) in options {
        match key.as_str() {
            "name" => name = Some(val),
            "filename" => filename = Some(val),
            _ => {}
        }
    }
    (disposition, name, filename)
}

/// Iterate over `; key=value` options starting at byte offset `start`.
///
/// This is a hand-written equivalent of `re.findall()` with the pattern
/// `; *([a-zA-Z0-9-_]+) *= *("[^\\"]*"|<token>|"(?:\\.|[^\\"])*")` used by
/// `multipart`, including the way it silently skips over garbage.
fn scan_options(s: &str, start: usize) -> impl Iterator<Item = (&str, &str)> {
    let bytes = s.as_bytes();
    let mut pos = start;
    std::iter::from_fn(move || {
        while pos < bytes.len() {
            if let Some((key, val, end)) = match_option(bytes, pos) {
                pos = end;
                return Some((&s[key.0..key.1], &s[val.0..val.1]));
            }
            pos += 1;
        }
        None
    })
}

type Span = (usize, usize);

fn match_option(b: &[u8], pos: usize) -> Option<(Span, Span, usize)> {
    let n = b.len();
    let skip_spaces = |mut i: usize| {
        while i < n && b[i] == b' ' {
            i += 1;
        }
        i
    };

    if b[pos] != b';' {
        return None;
    }
    let key_start = skip_spaces(pos + 1);
    let mut i = key_start;
    while i < n && is_hname_byte(b[i]) {
        i += 1;
    }
    if i == key_start {
        return None;
    }
    let key = (key_start, i);
    i = skip_spaces(i);
    if i >= n || b[i] != b'=' {
        return None;
    }
    let val_start = skip_spaces(i + 1);
    if val_start >= n {
        return None;
    }

    if b[val_start] == b'"' {
        // Simple quoted string without backslashes.
        let mut j = val_start + 1;
        while j < n && b[j] != b'"' && b[j] != b'\\' {
            j += 1;
        }
        if j < n && b[j] == b'"' {
            return Some((key, (val_start, j + 1), j + 1));
        }
        // Quoted string with backslash escapes. `\\.` does not match a newline.
        let mut j = val_start + 1;
        while j < n {
            match b[j] {
                b'"' => return Some((key, (val_start, j + 1), j + 1)),
                b'\\' => {
                    if j + 1 >= n || b[j + 1] == b'\n' {
                        return None;
                    }
                    j += 1 + utf8_char_len(b[j + 1]);
                }
                _ => j += 1,
            }
        }
        None
    } else if is_token_byte(b[val_start]) {
        let mut j = val_start;
        while j < n && is_token_byte(b[j]) {
            j += 1;
        }
        Some((key, (val_start, j), j))
    } else {
        None
    }
}

fn utf8_char_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_fast_path() {
        assert_eq!(
            parse_content_disposition(r#"form-data; name="field""#),
            ("form-data".into(), Some("field".into()), None)
        );
        assert_eq!(
            parse_content_disposition(r#"form-data; name="file"; filename="a%22b.txt""#),
            (
                "form-data".into(),
                Some("file".into()),
                Some("a\"b.txt".into())
            )
        );
    }

    #[test]
    fn content_disposition_slow_path() {
        assert_eq!(
            parse_content_disposition(r#"Form-Data; filename="C:\\x\\y.txt"; name=foo"#),
            ("form-data".into(), Some("foo".into()), Some("y.txt".into()))
        );
        assert_eq!(
            parse_content_disposition(r#"form-data; name="a\"b""#),
            ("form-data".into(), Some("a\"b".into()), None)
        );
        assert_eq!(
            parse_content_disposition("attachment"),
            ("attachment".into(), None, None)
        );
    }

    #[test]
    fn options_header() {
        let (value, options) =
            parse_options_header("Text/Plain; charset=UTF-8; x = \"y\"", header_unquote);
        assert_eq!(value, "text/plain");
        assert_eq!(
            options.unwrap(),
            vec![
                ("charset".to_owned(), "UTF-8".to_owned()),
                ("x".to_owned(), "y".to_owned())
            ]
        );
        assert_eq!(
            parse_options_header(" A/B ", header_unquote),
            ("a/b".into(), None)
        );
    }

    #[test]
    fn header_names() {
        assert_eq!(
            normalize_header_name("content-type").as_deref(),
            Some("Content-Type")
        );
        assert_eq!(normalize_header_name(" x1y_z ").as_deref(), Some("X1Y_Z"));
        assert_eq!(normalize_header_name("bad name"), None);
        assert_eq!(normalize_header_name(""), None);
    }
}
