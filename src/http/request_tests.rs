use super::request::{HttpRequest, Method, default_user_agent};

#[test]
fn get_request_serialization() {
    let req = HttpRequest::get("http://example.com/path?q=1").unwrap();
    let text = String::from_utf8(req.serialize()).unwrap();
    let default_user_agent = default_user_agent();

    assert!(text.starts_with("GET /path?q=1 HTTP/1.1\r\n"));
    assert!(text.contains("Host: example.com\r\n"));
    assert!(text.contains(&format!("User-Agent: {default_user_agent}\r\n")));
    assert!(text.ends_with("\r\n\r\n"));
}

#[test]
fn post_request_with_body() {
    let body = b"hello=world".to_vec();
    let req = HttpRequest::post("http://example.com/submit", body.clone()).unwrap();
    let bytes = req.serialize();
    let text = String::from_utf8(bytes).unwrap();

    assert!(text.starts_with("POST /submit HTTP/1.1\r\n"));
    assert!(text.contains("Content-Length: 11\r\n"));
    assert!(text.ends_with("\r\n\r\nhello=world"));
}

#[test]
fn host_header_includes_custom_port() {
    let req = HttpRequest::get("http://localhost:9090/api").unwrap();
    let text = String::from_utf8(req.serialize()).unwrap();

    assert!(text.contains("Host: localhost:9090\r\n"));
}

#[test]
fn host_header_omits_default_port() {
    let req = HttpRequest::get("http://example.com:80/").unwrap();
    let text = String::from_utf8(req.serialize()).unwrap();

    assert!(text.contains("Host: example.com\r\n"));
}

#[test]
fn get_request_includes_accept_encoding_gzip() {
    let req = HttpRequest::get("http://example.com").unwrap();
    let text = String::from_utf8(req.serialize()).unwrap();

    assert!(text.contains("Accept-Encoding: gzip\r\n"));
}

#[test]
fn get_request_includes_browser_language_preferences() {
    let req = HttpRequest::get("http://example.com").unwrap();
    let text = String::from_utf8(req.serialize()).unwrap();

    assert!(text.contains("Accept-Language: en-US,en;q=0.5\r\n"));
}

#[test]
fn add_custom_header() {
    let mut req = HttpRequest::get("http://example.com").unwrap();
    req.add_header("Accept", "text/html");
    let text = String::from_utf8(req.serialize()).unwrap();

    assert!(text.contains("Accept: text/html\r\n"));
}

#[test]
fn set_header_replaces_existing_value_case_insensitively() {
    let mut req = HttpRequest::get("http://example.com").unwrap();
    req.set_header("user-agent", "TestAgent/1.0");
    let text = String::from_utf8(req.serialize()).unwrap();

    assert!(text.contains("user-agent: TestAgent/1.0\r\n"));
    assert_eq!(text.matches("User-Agent:").count(), 0);
    assert_eq!(text.matches("user-agent:").count(), 1);
}

#[test]
fn method_display() {
    assert_eq!(Method::Get.to_string(), "GET");
    assert_eq!(Method::Post.to_string(), "POST");
    assert_eq!(Method::Delete.to_string(), "DELETE");
}

#[test]
fn set_body_replaces_content_length() {
    let mut req = HttpRequest::get("http://example.com").unwrap();
    req.set_body(b"abc".to_vec());
    req.set_body(b"abcdef".to_vec());

    let cl_count = req
        .headers()
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .count();
    assert_eq!(cl_count, 1);

    let cl_val = req
        .headers()
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.as_str())
        .unwrap();
    assert_eq!(cl_val, "6");
}
