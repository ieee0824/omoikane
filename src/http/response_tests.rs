use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

use super::request::Method;
use super::response::{HttpParseError, HttpResponse};

fn gzip_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

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
    let raw =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
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
fn responses_without_bodies_do_not_wait_for_connection_close() {
    struct OpenConnection<'a>(&'a [u8]);
    impl std::io::Read for OpenConnection<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.0.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "connection remains open",
                ));
            }
            let count = self.0.read(output)?;
            Ok(count)
        }
    }
    for (method, raw, expected_status) in [
        (
            Method::Head,
            &b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n"[..],
            200,
        ),
        (Method::Get, &b"HTTP/1.1 204 No Content\r\n\r\n"[..], 204),
        (Method::Get, &b"HTTP/1.1 304 Not Modified\r\n\r\n"[..], 304),
    ] {
        let response = HttpResponse::parse_for_method(&mut OpenConnection(raw), method).unwrap();
        assert_eq!(response.status_code(), expected_status);
        assert!(response.body().is_empty());
    }
}

#[test]
fn informational_response_is_followed_by_final_response() {
    let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    let response = HttpResponse::parse(&mut &raw[..]).unwrap();
    assert_eq!(response.status_code(), 200);
    assert_eq!(response.body(), b"ok");
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
    let raw =
        b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n";
    let resp = HttpResponse::parse(&mut &raw[..]).unwrap();

    assert_eq!(resp.body(), b"abc");
}

#[test]
fn parse_gzip_encoded_response() {
    let compressed = gzip_bytes(b"hello gzip");
    let raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
        compressed.len()
    )
    .into_bytes();
    let mut response = raw;
    response.extend_from_slice(&compressed);

    let resp = HttpResponse::parse(&mut &response[..]).unwrap();
    assert_eq!(resp.body(), b"hello gzip");
}

#[test]
fn parse_chunked_gzip_response() {
    let compressed = gzip_bytes(b"chunked gzip");
    let chunk = format!("{:X}\r\n", compressed.len()).into_bytes();

    let mut raw =
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    raw.extend_from_slice(&chunk);
    raw.extend_from_slice(&compressed);
    raw.extend_from_slice(b"\r\n0\r\n\r\n");

    let resp = HttpResponse::parse(&mut &raw[..]).unwrap();
    assert_eq!(resp.body(), b"chunked gzip");
}

#[test]
fn chunked_extensions_trailers_and_transfer_coding_are_decoded() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;name=value\r\nhello\r\n0\r\nX-Trailer: one\r\nX-Second: two\r\n\r\n";
    assert_eq!(HttpResponse::parse(&mut &raw[..]).unwrap().body(), b"hello");

    let compressed = gzip_bytes(b"decoded transfer coding");
    let mut raw =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n"
            .to_vec();
    raw.extend_from_slice(format!("{:X};foo=bar\r\n", compressed.len()).as_bytes());
    raw.extend_from_slice(&compressed);
    raw.extend_from_slice(b"\r\n0\r\n\r\n");
    assert_eq!(
        HttpResponse::parse(&mut &raw[..]).unwrap().body(),
        b"decoded transfer coding"
    );
}

#[test]
fn malformed_chunk_terminator_and_content_length_are_rejected() {
    let bad_chunk = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcXX0\r\n\r\n";
    assert!(matches!(
        HttpResponse::parse(&mut &bad_chunk[..]),
        Err(HttpParseError::InvalidChunkSize)
    ));
    let conflicting = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\nabc";
    assert!(matches!(
        HttpResponse::parse(&mut &conflicting[..]),
        Err(HttpParseError::InvalidHeader)
    ));
}

#[test]
fn declared_and_decoded_body_sizes_are_bounded() {
    let large_length = b"HTTP/1.1 200 OK\r\nContent-Length: 1000000000000\r\n\r\n";
    assert!(matches!(
        HttpResponse::parse_with_limit(&mut &large_length[..], 16),
        Err(HttpParseError::BodyTooLarge)
    ));
    let large_chunk = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffff\r\n";
    assert!(matches!(
        HttpResponse::parse_with_limit(&mut &large_chunk[..], 16),
        Err(HttpParseError::BodyTooLarge)
    ));
    let compressed = gzip_bytes(&[b'a'; 128]);
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
        compressed.len()
    )
    .into_bytes();
    raw.extend_from_slice(&compressed);
    assert!(matches!(
        HttpResponse::parse_with_limit(&mut &raw[..], 64),
        Err(HttpParseError::BodyTooLarge)
    ));
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
