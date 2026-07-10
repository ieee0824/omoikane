//! Integration tests for the Acid3 harness.
//!
//! These assertions are engine-independent: they verify that the fixture
//! server serves the vendored resources with the exact HTTP status and
//! Content-Type Acid3 depends on, and that the Acid3 page can be fetched over
//! HTTP and parsed into a DOM containing the expected structural elements.
//! They do NOT assert a particular Acid3 score, so they stay green regardless
//! of the engine's current JS/DOM completeness. Use `cargo run --example acid3`
//! to observe the live baseline score.

#[path = "acid3_common/harness.rs"]
mod harness;

use harness::{DriveMode, FixtureServer, run_acid3};
use omoikane::html::TreeBuilder;
use omoikane::http::Client;

/// The fixture server must replay the exact status code + Content-Type recorded
/// in `manifest.json` for the resources Acid3 relies on. In particular
/// `empty.css` is served as `text/html` (so it is not applied as a stylesheet)
/// and `support-a.png` returns `404`.
#[test]
fn server_serves_fixtures_with_manifest_headers() {
    let server = FixtureServer::start();
    let mut client = Client::new();

    // (path, expected status, expected Content-Type)
    let cases = [
        ("acid3.html", 200u16, "text/html; charset=utf-8"),
        ("empty.css", 200, "text/html; charset=utf-8"),
        ("empty.html", 200, "text/html; charset=utf-8"),
        ("empty.png", 200, "image/png"),
        ("empty.txt", 200, "text/plain; charset=utf-8"),
        ("empty.xml", 200, "application/xml;charset=utf-8"),
        ("reference.html", 200, "text/html; charset=utf-8"),
        ("font.ttf", 200, "application/x-truetype-font"),
        ("svg.xml", 200, "image/svg+xml"),
        ("xhtml.1", 200, "text/xml"),
        ("support-a.png", 404, "image/png"),
        ("support-b.png", 200, "text/html; charset=utf-8"),
        ("support-c.png", 200, "image/png"),
    ];

    for (path, expected_status, expected_ct) in cases {
        let url = format!("{}/{}", server.base_url(), path);
        let resp = client
            .get(&url)
            .unwrap_or_else(|e| panic!("GET {path} failed: {e}"));
        assert_eq!(
            resp.status_code(),
            expected_status,
            "unexpected status for {path}"
        );
        assert_eq!(
            resp.header("content-type"),
            Some(expected_ct),
            "unexpected Content-Type for {path}"
        );
        assert!(
            !resp.body().is_empty(),
            "expected a non-empty body for {path}"
        );
    }
}

/// The root path (`/`) must serve the Acid3 page, matching the canonical origin.
#[test]
fn server_root_serves_acid3_page() {
    let server = FixtureServer::start();
    let mut client = Client::new();
    let resp = client
        .get(&server.base_url())
        .expect("GET / should succeed");
    assert_eq!(resp.status_code(), 200);
    let body = String::from_utf8_lossy(resp.body());
    assert!(
        body.contains("The Acid3 Test"),
        "root did not serve the Acid3 page"
    );
    assert!(
        body.contains("id=\"score\""),
        "Acid3 page is missing the #score element"
    );
}

/// The Acid3 page must be fetchable over HTTP and parseable into a DOM whose
/// scoreboard elements are present. This exercises the fetch + parse portion of
/// the pipeline without depending on JS behaviour.
#[test]
fn acid3_page_parses_to_dom_with_scoreboard() {
    let server = FixtureServer::start();
    let mut client = Client::new();
    let resp = client.get(&server.acid3_url()).expect("fetch acid3.html");
    assert_eq!(resp.status_code(), 200);

    let html = String::from_utf8_lossy(resp.body()).to_string();
    let document = TreeBuilder::parse(&html).document();

    let score = document
        .query_selector("#score")
        .expect("#score element must be present in the parsed DOM");
    assert_eq!(score.tag_name().as_deref(), Some("span"));

    assert!(
        document.query_selector("#result").is_some(),
        "#result element must be present"
    );
    assert!(
        document.query_selector("#instructions").is_some(),
        "#instructions element must be present"
    );
}

/// The runner must complete both drive modes without panicking and return a
/// readable result snapshot. This captures a baseline signal (page loads, score
/// is extractable) without asserting a specific score.
#[test]
fn runner_completes_without_panicking() {
    let server = FixtureServer::start();

    let faithful = run_acid3(&server.base_url(), DriveMode::Faithful);
    assert_eq!(faithful.page_status, 200);
    assert!(faithful.html_bytes > 100_000, "acid3.html should be large");

    let direct = run_acid3(&server.base_url(), DriveMode::DirectDrive);
    assert_eq!(direct.page_status, 200);
}
