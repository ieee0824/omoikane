//! HTTP response parsing.

use flate2::read::GzDecoder;
use std::fmt;
use std::io::{self, BufRead, Read};

use super::request::Method;
use super::url::Url;

/// Default cap for buffered HTTP response bodies, before and after decoding.
pub(crate) const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn response_body_limit() -> usize {
    std::env::var("OMOIKANE_MAX_HTTP_BODY_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_MAX_RESPONSE_BODY_BYTES)
}

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
    effective_url: Option<Url>,
    redirect_count: usize,
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
            effective_url: None,
            redirect_count: 0,
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

    /// Returns the final URL after redirects when the response came from [`Client`](super::Client).
    pub fn effective_url(&self) -> Option<&Url> {
        self.effective_url.as_ref()
    }

    /// Returns the number of HTTP redirects followed to obtain this response.
    /// Responses parsed directly from a stream have a count of zero.
    pub fn redirect_count(&self) -> usize {
        self.redirect_count
    }

    pub(crate) fn set_redirect_count(&mut self, count: usize) {
        self.redirect_count = count;
    }

    pub(crate) fn set_effective_url(&mut self, url: Url) {
        self.effective_url = Some(url);
    }

    /// Parses an HTTP/1.1 response from a readable stream.
    ///
    /// Supports two body framing mechanisms:
    /// - `Content-Length`: reads exactly the specified number of bytes.
    /// - `Transfer-Encoding: chunked`: reassembles chunked-encoded body.
    ///
    /// If neither is present, reads until the stream is closed (EOF).
    pub fn parse(reader: &mut impl Read) -> Result<Self, HttpParseError> {
        Self::parse_with_limit(reader, response_body_limit())
    }

    /// Parses a GET response while limiting buffered and decoded body size.
    pub fn parse_with_limit(
        reader: &mut impl Read,
        max_body_bytes: usize,
    ) -> Result<Self, HttpParseError> {
        Self::parse_for_method_with_limit(reader, Method::Get, max_body_bytes)
    }

    pub(crate) fn parse_for_method(
        reader: &mut impl Read,
        method: Method,
    ) -> Result<Self, HttpParseError> {
        Self::parse_for_method_with_limit(reader, method, response_body_limit())
    }

    fn parse_for_method_with_limit(
        reader: &mut impl Read,
        method: Method,
        max_body_bytes: usize,
    ) -> Result<Self, HttpParseError> {
        let mut buf_reader = io::BufReader::new(reader);
        for _ in 0..16 {
            let mut status_line = String::new();
            buf_reader
                .read_line(&mut status_line)
                .map_err(HttpParseError::Io)?;
            let (status_code, reason) =
                parse_status_line(status_line.trim_end_matches(['\r', '\n']))?;

            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                if buf_reader
                    .read_line(&mut line)
                    .map_err(HttpParseError::Io)?
                    == 0
                {
                    return Err(HttpParseError::InvalidHeader);
                }
                let trimmed = line.trim_end_matches(['\r', '\n']);
                if trimmed.is_empty() {
                    break;
                }
                let (name, value) = parse_header_line(trimmed)?;
                headers.push((name, value));
            }

            // 101 is a terminal protocol switch; other informational responses
            // precede a final response on the same connection.
            if (100..200).contains(&status_code) && status_code != 101 {
                continue;
            }
            let body = if method == Method::Head || matches!(status_code, 101 | 204 | 304) {
                Vec::new()
            } else {
                let body = read_body(&headers, &mut buf_reader, max_body_bytes)?;
                decode_content_encoding(&headers, body, max_body_bytes)?
            };
            return Ok(HttpResponse::new(status_code, reason, headers, body));
        }
        Err(HttpParseError::InvalidStatusLine)
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
    /// The response body exceeds the configured buffer limit.
    BodyTooLarge,
    /// Too many redirects were followed without reaching a final response.
    TooManyRedirects,
    /// A redirect response is missing the `Location` header.
    MissingLocation,
}

impl HttpParseError {
    /// Returns whether the transport stopped waiting because its configured
    /// socket timeout elapsed.  Higher-level consumers such as XHR need to
    /// distinguish this terminal condition from an ordinary network error.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Io(error) if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock))
    }
}

impl fmt::Display for HttpParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::InvalidStatusLine => write!(f, "invalid HTTP status line"),
            Self::InvalidStatusCode => write!(f, "invalid HTTP status code"),
            Self::InvalidHeader => write!(f, "invalid HTTP header"),
            Self::InvalidChunkSize => write!(f, "invalid chunk size in chunked encoding"),
            Self::BodyTooLarge => write!(f, "HTTP response body exceeds configured limit"),
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
    max_body_bytes: usize,
) -> Result<Vec<u8>, HttpParseError> {
    let mut transfer_codings = Vec::new();
    for (_, value) in headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
    {
        for coding in value.split(',') {
            let coding = coding.trim();
            if coding.is_empty() {
                return Err(HttpParseError::InvalidHeader);
            }
            transfer_codings.push(coding);
        }
    }
    if !transfer_codings.is_empty() {
        if !transfer_codings
            .last()
            .unwrap()
            .eq_ignore_ascii_case("chunked")
        {
            return Err(HttpParseError::InvalidHeader);
        }
        let mut body = read_chunked_body(reader, max_body_bytes)?;
        for coding in transfer_codings[..transfer_codings.len() - 1].iter().rev() {
            if !coding.eq_ignore_ascii_case("gzip") {
                return Err(HttpParseError::InvalidHeader);
            }
            body = decode_gzip(&body, max_body_bytes)?;
        }
        return Ok(body);
    }

    let mut content_length = None;
    for (_, value) in headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
    {
        for length in value.split(',') {
            let length = length.trim();
            if length.is_empty() || !length.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(HttpParseError::InvalidHeader);
            }
            let length = length
                .parse::<usize>()
                .map_err(|_| HttpParseError::InvalidHeader)?;
            if content_length.is_some_and(|previous| previous != length) {
                return Err(HttpParseError::InvalidHeader);
            }
            content_length = Some(length);
        }
    }
    if let Some(length) = content_length {
        if length > max_body_bytes {
            return Err(HttpParseError::BodyTooLarge);
        }
        let mut body = Vec::new();
        reader
            .take(length as u64)
            .read_to_end(&mut body)
            .map_err(HttpParseError::Io)?;
        if body.len() != length {
            return Err(HttpParseError::Io(io::Error::from(
                io::ErrorKind::UnexpectedEof,
            )));
        }
        Ok(body)
    } else {
        read_bounded_to_end(reader, max_body_bytes)
    }
}

fn read_chunked_body(
    reader: &mut impl BufRead,
    max_body_bytes: usize,
) -> Result<Vec<u8>, HttpParseError> {
    let mut body = Vec::new();

    loop {
        let mut size_line = String::new();
        reader
            .read_line(&mut size_line)
            .map_err(HttpParseError::Io)?;
        let line = size_line
            .strip_suffix("\r\n")
            .ok_or(HttpParseError::InvalidChunkSize)?;
        let size_str = line.split(';').next().unwrap_or("").trim();
        if size_str.is_empty() || !size_str.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(HttpParseError::InvalidChunkSize);
        }
        let chunk_size =
            usize::from_str_radix(size_str, 16).map_err(|_| HttpParseError::InvalidChunkSize)?;

        if chunk_size == 0 {
            loop {
                let mut trailing = String::new();
                if reader
                    .read_line(&mut trailing)
                    .map_err(HttpParseError::Io)?
                    == 0
                {
                    return Err(HttpParseError::InvalidHeader);
                }
                if trailing == "\r\n" {
                    break;
                }
                let line = trailing
                    .strip_suffix("\r\n")
                    .ok_or(HttpParseError::InvalidHeader)?;
                parse_header_line(line)?;
            }
            break;
        }

        if chunk_size > max_body_bytes.saturating_sub(body.len()) {
            return Err(HttpParseError::BodyTooLarge);
        }
        let mut remaining = chunk_size;
        let mut buffer = [0u8; 8192];
        while remaining > 0 {
            let count = remaining.min(buffer.len());
            reader
                .read_exact(&mut buffer[..count])
                .map_err(HttpParseError::Io)?;
            body.extend_from_slice(&buffer[..count]);
            remaining -= count;
        }

        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(HttpParseError::Io)?;
        if crlf != *b"\r\n" {
            return Err(HttpParseError::InvalidChunkSize);
        }
    }

    Ok(body)
}

fn read_bounded_to_end(
    reader: &mut impl Read,
    max_body_bytes: usize,
) -> Result<Vec<u8>, HttpParseError> {
    let mut body = Vec::new();
    reader
        .take((max_body_bytes as u64).saturating_add(1))
        .read_to_end(&mut body)
        .map_err(HttpParseError::Io)?;
    if body.len() > max_body_bytes {
        Err(HttpParseError::BodyTooLarge)
    } else {
        Ok(body)
    }
}

fn decode_gzip(body: &[u8], max_body_bytes: usize) -> Result<Vec<u8>, HttpParseError> {
    read_bounded_to_end(&mut GzDecoder::new(body), max_body_bytes)
}

fn decode_content_encoding(
    headers: &[(String, String)],
    body: Vec<u8>,
    max_body_bytes: usize,
) -> Result<Vec<u8>, HttpParseError> {
    let Some(encoding) = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
        .map(|(_, v)| v.as_str())
    else {
        return Ok(body);
    };

    if !encoding
        .split(',')
        .any(|value| value.trim().eq_ignore_ascii_case("gzip"))
    {
        return Ok(body);
    }

    decode_gzip(&body, max_body_bytes)
}
