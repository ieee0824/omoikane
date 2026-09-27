use super::*;
use crate::html::TreeBuilder;

#[test]
fn paint_snapshot_resolves_links_against_first_base_and_current_href() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head>
        <base href="https://example.test/base/">
        <base href="https://ignored.test/">
        </head><body>
        <a id="first" href="next">next</a>
        <a id="second" href="other">other</a>
        </body></html>"#,
    )
    .document();
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = JsRuntime::with_document_url_and_storage(
        document.clone(),
        "https://example.test/start",
        storage.clone(),
        session,
    )
    .unwrap();
    let source = VisitSource::new(
        StorageOrigin::from_url("https://example.test/start").unwrap(),
        "https://example.test/start",
    )
    .unwrap();
    storage.record_page_navigation(
        "https://example.test/base/next",
        "https://example.test/base/next",
        source,
    );

    let initial = runtime
        .host_state
        .borrow()
        .visited_link_ids_for_paint(&document);
    assert_eq!(initial.len(), 1);
    runtime
        .eval("document.getElementById('first').setAttribute('href', 'other')")
        .unwrap();
    assert!(
        runtime
            .host_state
            .borrow()
            .visited_link_ids_for_paint(&document)
            .is_empty()
    );
    runtime
        .eval("document.getElementById('second').setAttribute('href', 'next')")
        .unwrap();
    let changed = runtime
        .host_state
        .borrow()
        .visited_link_ids_for_paint(&document);
    assert_eq!(changed.len(), 1);
    assert_ne!(initial, changed);
}

#[test]
fn committed_visit_changes_pixels_but_not_cssom_or_selector_apis() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
        body { margin: 0 }
        a { display: block; width: 40px; height: 40px; background-color: #ff0000 }
        a:visited { background-color: #00ff00 }
        </style></head><body><a id="link" href="/destination"></a></body></html>"#,
    )
    .document();
    let storage = StorageManager::new();
    let session = storage.create_session();
    let mut runtime = JsRuntime::with_document_url_and_storage(
        document,
        "https://example.test/start",
        storage.clone(),
        session,
    )
    .unwrap();
    let before = runtime.paint_current_document().unwrap();
    assert_eq!(
        before.pixel(20, 20),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );

    let destination = "https://example.test/destination";
    let source = VisitSource::new(
        StorageOrigin::from_url("https://example.test/start").unwrap(),
        "https://example.test/start",
    )
    .unwrap();
    storage.record_page_navigation(destination, destination, source);
    let after = runtime.paint_current_document().unwrap();
    assert_eq!(
        after.pixel(20, 20),
        Some(crate::paint::Color::rgb(0, 255, 0))
    );
    let script = runtime
        .eval(
            "(() => { const a = document.getElementById('link'); return [getComputedStyle(a).backgroundColor, a.matches(':visited'), a.matches(':link')].join('|') })()",
        )
        .unwrap();
    let result = script.as_string().unwrap().to_std_string_escaped();
    assert_eq!(result, "rgb(255, 0, 0)|false|true");

    runtime
        .eval("document.getElementById('link').setAttribute('href', '/other')")
        .unwrap();
    let changed = runtime.paint_current_document().unwrap();
    assert_eq!(
        changed.pixel(20, 20),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );
}
