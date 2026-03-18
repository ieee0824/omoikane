//! HTTP response parsing.

use std::fmt;
use std::io::{self, BufRead, Read};

/// A parsed HTTP/1.1 response.
///
/// # Examples
///
/// ```
/// use omoikane::http::HttpResponse;
///
/// let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
/// let resp = HttpResponse::parse(&mut &raw[..]).unwrap();
/// assert_eq!(resp.status_code(), 200);
/// assert_eq!(resp.reason(), "OK");
/// assert_eq!(resp.body(), b"hello");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    status_code: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    pub(crate) fn new(
        status_code: u16,
        reason: impl Into<String>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status_code,
            reason: reason.into(),
            headers,
            body,
        }
    }

    /// Returns the HTTP status code (e.g. `200`, `404`).
    pub fn status_code(&self) -> u16 {
        self.status_code
    }

    /// Returns the reason phrase (e.g. `"OK"`, `"Not Found"`).
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the response headers.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// Returns the first header value matching `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Returns the response body bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Parses an HTTP/1.1 response from a readable stream.
    ///
    /// Supports two body framing mechanisms:
    /// - `Content-Length`: reads exactly the specified number of bytes.
    /// - `Transfer-Encoding: chunked`: reassembles chunked-encoded body.
    ///
    /// If neither is present, reads until the stream is closed (EOF).
    pub fn parse(reader: &mut impl Read) -> Result<Self, HttpParseError> {
        let mut buf_reader = io::BufReader::new(reader);

        // Status line
        let mut status_line = String::new();
        buf_reader
            .read_line(&mut status_line)
            .map_err(HttpParseError::Io)?;
        let status_line = status_line.trim_end_matches(|c| c == '\r' || c == '\n');

        let (status_code, reason) = parse_status_line(status_line)?;

        // Headers
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            buf_reader
                .read_line(&mut line)
                .map_err(HttpParseError::Io)?;
            let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
            if trimmed.is_empty() {
                break;
            }
            let (name, value) = parse_header_line(trimmed)?;
            headers.push((name, value));
        }

        // Body
        let body = read_body(&headers, &mut buf_reader)?;

        Ok(HttpResponse::new(status_code, reason, headers, body))
    }
}

/// Errors that can occur when parsing an HTTP response.
#[derive(Debug)]
pub enum HttpParseError {
    /// An I/O error occurred while reading the stream.
    Io(io::Error),
    /// The status line is malformed.
    InvalidStatusLine,
    /// The status code is not a valid number.
    InvalidStatusCode,
    /// A header line is malformed (missing `:`).
    InvalidHeader,
    /// A chunk size in chunked transfer encoding is malformed.
    InvalidChunkSize,
    /// Too many redirects were followed without reaching a final response.
    TooManyRedirects,
    /// A redirect response is missing the `Location` header.
    MissingLocation,
}

impl fmt::Display for HttpParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::InvalidStatusLine => write!(f, "invalid HTTP status line"),
            Self::InvalidStatusCode => write!(f, "invalid HTTP status code"),
            Self::InvalidHeader => write!(f, "invalid HTTP header"),
            Self::InvalidChunkSize => write!(f, "invalid chunk size in chunked encoding"),
            Self::TooManyRedirects => write!(f, "too many redirects"),
            Self::MissingLocation => write!(f, "redirect response missing Location header"),
        }
    }
}

impl std::error::Error for HttpParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

fn parse_status_line(line: &str) -> Result<(u16, String), HttpParseError> {
    // "HTTP/1.1 200 OK"
    let mut parts = line.splitn(3, ' ');
    let _version = parts.next().ok_or(HttpParseError::InvalidStatusLine)?;
    let code_str = parts.next().ok_or(HttpParseError::InvalidStatusLine)?;
    let reason = parts.next().unwrap_or("").to_string();

    let status_code: u16 = code_str
        .parse()
        .map_err(|_| HttpParseError::InvalidStatusCode)?;

    Ok((status_code, reason))
}

fn parse_header_line(line: &str) -> Result<(String, String), HttpParseError> {
    let (name, value) = line.split_once(':').ok_or(HttpParseError::InvalidHeader)?;
    Ok((name.trim().to_string(), value.trim().to_string()))
}

fn read_body(
    headers: &[(String, String)],
    reader: &mut impl BufRead,
) -> Result<Vec<u8>, HttpParseError> {
    // Check Transfer-Encoding first (takes priority over Content-Length per RFC 7230)
    let is_chunked = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.eq_ignore_ascii_case("chunked")
    });

    if is_chunked {
        return read_chunked_body(reader);
    }

    // Check Content-Length
    let content_length = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok());

    if let Some(len) = content_length {
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).map_err(HttpParseError::Io)?;
        Ok(body)
    } else {
        // Read until EOF
        let mut body = Vec::new();
        reader.read_to_end(&mut body).map_err(HttpParseError::Io)?;
        Ok(body)
    }
}

fn read_chunked_body(reader: &mut impl BufRead) -> Result<Vec<u8>, HttpParseError> {
    let mut body = Vec::new();

    loop {
        let mut size_line = String::new();
        reader
            .read_line(&mut size_line)
            .map_err(HttpParseError::Io)?;
        let size_str = size_line.trim();

        let chunk_size =
            usize::from_str_radix(size_str, 16).map_err(|_| HttpParseError::InvalidChunkSize)?;

        if chunk_size == 0 {
            // Read trailing \r\n after final chunk
            let mut trailing = String::new();
            let _ = reader.read_line(&mut trailing);
            break;
        }

        let mut chunk = vec![0u8; chunk_size];
        reader.read_exact(&mut chunk).map_err(HttpParseError::Io)?;
        body.extend_from_slice(&chunk);

        // Read \r\n after chunk data
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(HttpParseError::Io)?;
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_response() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.reason(), "OK");
        assert_eq!(resp.header("Content-Length"), Some("5"));
        assert_eq!(resp.body(), b"hello");
    }

    #[test]
    fn parse_404_response() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.status_code(), 404);
        assert_eq!(resp.reason(), "Not Found");
        assert_eq!(resp.body(), b"not found");
    }

    #[test]
    fn parse_chunked_response() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.status_code(), 200);
        assert_eq!(resp.body(), b"hello world");
    }

    #[test]
    fn parse_no_content_length_reads_to_eof() {
        let raw = b"HTTP/1.1 200 OK\r\n\r\nsome body content";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.body(), b"some body content");
    }

    #[test]
    fn parse_empty_body() {
        let raw = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.status_code(), 204);
        assert_eq!(resp.reason(), "No Content");
        assert_eq!(resp.body(), b"");
    }

    #[test]
    fn parse_multiple_headers() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 4\r\nX-Custom: value\r\n\r\ntest";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.header("Content-Type"), Some("text/html"));
        assert_eq!(resp.header("X-Custom"), Some("value"));
        assert_eq!(resp.headers().len(), 3);
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nContent-Length: 0\r\n\r\n";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.header("Content-Type"), Some("text/html"));
        assert_eq!(resp.header("CONTENT-TYPE"), Some("text/html"));
    }

    #[test]
    fn chunked_takes_priority_over_content_length() {
        // Per RFC 7230 §3.3.3, Transfer-Encoding takes priority
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
        let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

        assert_eq!(resp.body(), b"abc");
    }

    #[test]
    fn invalid_status_line() {
        let raw = b"INVALID\r\n\r\n";
        let result = HttpResponse::parse(&mut &raw[..]);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_header_no_colon() {
        let raw = b"HTTP/1.1 200 OK\r\nBadHeader\r\n\r\n";
        let result = HttpResponse::parse(&mut &raw[..]);
        assert!(result.is_err());
    }
}
