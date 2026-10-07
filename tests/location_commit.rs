//! Location requests preserve committed document URLs until navigation commits.
use omoikane::html::TreeBuilder;
use omoikane::js::{JsRuntime, NavigationRequest};

#[test]
fn cross_document_location_setters_keep_committed_url() {
    let initial = "http://example.test/page?q=old#section";
    for (script, destination) in [
        ("location.assign('/next')", "http://example.test/next"),
        ("location.replace('/next')", "http://example.test/next"),
        ("location.href = '/next'", "http://example.test/next"),
        (
            "location.pathname = '/next'",
            "http://example.test/next?q=old#section",
        ),
        (
            "location.search = '?new'",
            "http://example.test/page?new#section",
        ),
        (
            "location.hostname = 'other.test'",
            "http://other.test/page?q=old#section",
        ),
        (
            "location.port = '8080'",
            "http://example.test:8080/page?q=old#section",
        ),
        (
            "location.protocol = 'https:'",
            "https://example.test/page?q=old#section",
        ),
        (
            "location.host = 'other.test:8080'",
            "http://other.test:8080/page?q=old#section",
        ),
    ] {
        let mut runtime = JsRuntime::with_document_and_url(
            TreeBuilder::parse("<body></body>").document(),
            initial,
        )
        .unwrap();
        runtime.eval(script).unwrap();
        assert_eq!(runtime.eval(
            "location.href === 'http://example.test/page?q=old#section' && document.URL === location.href && location.pathname === '/page' && location.search === '?q=old'"
        ).unwrap().as_boolean(), Some(true), "{script}");
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.take_navigation_requests(),
            [NavigationRequest::Navigate {
                url: destination.to_owned(),
                replace: script.starts_with("location.replace"),
            }],
            "{script}"
        );
    }
}

#[test]
fn empty_fragment_updates_location_and_document_synchronously() {
    let mut runtime = JsRuntime::with_document_and_url(
        TreeBuilder::parse("<body></body>").document(),
        "http://example.test/page",
    )
    .unwrap();
    assert_eq!(runtime.eval(
        "location.hash = '#'; location.href === 'http://example.test/page#' && document.URL === location.href && location.hash === ''"
    ).unwrap().as_boolean(), Some(true));
    assert_eq!(runtime.eval(
        "location.hash = ''; location.href === 'http://example.test/page' && document.URL === location.href"
    ).unwrap().as_boolean(), Some(true));
}
