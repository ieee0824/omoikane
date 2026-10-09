use super::child_document::{FetchedChildResource, LoadedChildDocument, parse_child_document};
use super::form_submission::Submission;
use super::*;
use crate::test_support::http_fixture::{
    FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback, read_request_headers,
};
use std::io::{Read, Write};
use std::time::Duration;

const CSP: &str = "default-src 'none'";

fn text_of(document: &NodeHandle, selector: &str) -> Option<String> {
    document.query_selector(selector).map(|node| {
        node.child_nodes()
            .iter()
            .filter_map(NodeHandle::data)
            .collect()
    })
}

fn is_blank(document: &NodeHandle) -> bool {
    document
        .query_selector("body")
        .is_some_and(|body| body.child_nodes().is_empty())
}

#[test]
fn rendered_child_resources_parse_by_content_type() {
    let html = parse_child_document("text/html; charset=utf-8", b"<p id=x>html</p>").unwrap();
    assert_eq!(text_of(&html, "#x").as_deref(), Some("html"));

    let svg = parse_child_document(
        "image/svg+xml",
        b"<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    assert_eq!(
        svg.child_nodes()
            .iter()
            .find_map(NodeHandle::local_name)
            .as_deref(),
        Some("svg")
    );
    assert_eq!(
        svg.document_content_type().as_deref(),
        Some("image/svg+xml")
    );
    assert!(!svg.is_html_document());

    let text = parse_child_document("Text/Plain; charset=utf-8", b"<b>markup</b>").unwrap();
    assert_eq!(text_of(&text, "pre").as_deref(), Some("<b>markup</b>"));
    assert!(
        text.query_selector("b").is_none(),
        "text/plain never becomes markup"
    );

    let malformed = parse_child_document("application/xml", b"<unclosed").unwrap();
    assert!(
        is_blank(&malformed),
        "malformed XML falls back to an empty document"
    );
    assert!(malformed.is_html_document());

    assert!(parse_child_document("image/png", b"\x89PNG").is_none());
}

#[test]
fn get_resources_drop_csp_only_when_the_content_type_is_not_rendered() {
    let resource = |mime_type: &str| FetchedChildResource {
        mime_type: mime_type.to_owned(),
        body: b"<p id=x>body</p>".to_vec(),
        csp_headers: vec![CSP.to_owned()],
        effective_url: "https://child.example/a#f".to_owned(),
    };
    let html = resource("text/html").into_get_document();
    assert_eq!(text_of(&html.document, "#x").as_deref(), Some("body"));
    assert_eq!(html.csp_headers, [CSP]);
    assert_eq!(html.url.as_deref(), Some("https://child.example/a#f"));

    let image = resource("image/png").into_get_document();
    assert!(is_blank(&image.document));
    assert!(image.csp_headers.is_empty());
    assert_eq!(image.url.as_deref(), Some("https://child.example/a#f"));
}

#[test]
fn documents_without_a_response_have_fixed_urls_and_no_csp() {
    let blank = LoadedChildDocument::about_blank("about:blank#top".to_owned());
    assert!(is_blank(&blank.document));
    assert_eq!(blank.url.as_deref(), Some("about:blank#top"));
    assert!(blank.csp_headers.is_empty());

    let srcdoc = LoadedChildDocument::srcdoc("<p id=x>inline</p>");
    assert_eq!(text_of(&srcdoc.document, "#x").as_deref(), Some("inline"));
    assert_eq!(srcdoc.url.as_deref(), Some("about:srcdoc"));
    assert!(srcdoc.csp_headers.is_empty());

    let failed = LoadedChildDocument::fetch_failed();
    assert!(is_blank(&failed.document));
    assert_eq!(failed.url, None);
    assert!(failed.csp_headers.is_empty());
}

#[test]
fn iframe_get_loads_handle_blank_data_and_unreachable_references() {
    let document = crate::html::TreeBuilder::parse("<p>top</p>").document();
    let runtime = JsRuntime::with_document(document).unwrap();
    let mut state = runtime.host_state.borrow_mut();

    let empty = state.load_iframe_document("", None);
    assert_eq!(empty.url.as_deref(), Some("about:blank"));
    let blank = state.load_iframe_document("about:blank#x", None);
    assert_eq!(blank.url.as_deref(), Some("about:blank#x"));

    let data = "data:text/html,<p id=x>data</p>";
    let loaded = state.load_iframe_document(data, None);
    assert_eq!(text_of(&loaded.document, "#x").as_deref(), Some("data"));
    assert_eq!(loaded.url.as_deref(), Some(data));
    assert!(loaded.csp_headers.is_empty());

    let closed = bind_loopback().unwrap();
    let unreachable = format!("http://{}/gone", closed.local_addr().unwrap());
    drop(closed);
    let failed = state.load_iframe_document(&unreachable, None);
    assert!(
        is_blank(&failed.document),
        "GET failures fall back to about:blank"
    );
    assert_eq!(failed.url, None);
    assert!(failed.csp_headers.is_empty());

    let submission = Submission {
        url: unreachable,
        method: "POST".to_owned(),
        body: Some(b"q=1".to_vec()),
        content_type: Some("application/x-www-form-urlencoded".to_owned()),
    };
    assert!(
        state.load_iframe_form_submission(&submission).is_err(),
        "POST failures propagate instead of committing a blank document"
    );
}

/// Serves `count` requests: `/error` is a 404 HTML page and `/image` a PNG,
/// both carrying a CSP header. Returns the request lines in arrival order.
fn serve_child_resources(count: usize) -> (String, FixtureWorker<Vec<String>>) {
    let listener = bind_loopback().unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = FixtureWorker::spawn(move || {
        let mut lines = Vec::new();
        while lines.len() < count {
            let mut stream = accept_with_timeout(&listener, Duration::from_secs(5)).unwrap();
            let headers = read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let line = headers.lines().next().unwrap().to_owned();
            let length = headers
                .lines()
                .filter_map(|header| header.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map_or(0, |(_, value)| value.trim().parse().unwrap());
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let (status, mime_type, body): (_, _, &[u8]) = if line.contains(" /error") {
                ("404 Not Found", "text/html", b"<p id=x>error page</p>")
            } else {
                ("200 OK", "image/png", b"\x89PNG")
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: {mime_type}\r\nContent-Security-Policy: {CSP}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
            lines.push(line);
        }
        lines
    });
    (origin, server)
}

#[test]
fn http_child_loads_keep_error_bodies_fragments_and_post_metadata() {
    let (origin, server) = serve_child_resources(3);
    let document = crate::html::TreeBuilder::parse("<p>top</p>").document();
    let runtime = JsRuntime::with_document(document).unwrap();
    let mut state = runtime.host_state.borrow_mut();

    let error = state.load_iframe_document(&format!("{origin}/error#frag"), None);
    assert_eq!(
        text_of(&error.document, "#x").as_deref(),
        Some("error page"),
        "an HTTP error status still renders its body"
    );
    assert_eq!(error.csp_headers, [CSP]);
    assert_eq!(error.url, Some(format!("{origin}/error#frag")));

    let image = state.load_iframe_document(&format!("{origin}/image"), None);
    assert!(is_blank(&image.document));
    assert!(
        image.csp_headers.is_empty(),
        "an unrendered GET drops its CSP"
    );
    assert_eq!(image.url, Some(format!("{origin}/image")));

    let submission = Submission {
        url: format!("{origin}/image"),
        method: "POST".to_owned(),
        body: Some(b"q=1".to_vec()),
        content_type: Some("application/x-www-form-urlencoded".to_owned()),
    };
    let posted = state.load_iframe_form_submission(&submission).unwrap();
    assert!(is_blank(&posted.document));
    assert_eq!(posted.csp_headers, [CSP], "a POST keeps its CSP headers");
    assert_eq!(posted.url, Some(format!("{origin}/image")));
    drop(state);

    let lines = server.join();
    assert!(lines[0].starts_with("GET /error"), "{lines:?}");
    assert!(lines[1].starts_with("GET /image"), "{lines:?}");
    assert!(lines[2].starts_with("POST /image"), "{lines:?}");
}
