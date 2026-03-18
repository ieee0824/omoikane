use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;

use super::response::HttpResponse;

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
