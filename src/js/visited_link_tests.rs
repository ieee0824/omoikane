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

#[test]
fn visited_links_remain_unvisited_in_all_dom_selector_apis() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><style>
        body { margin: 0 }
        a { display: block; width: 40px; height: 40px; background-color: #ff0000 }
        a:visited { background-color: #00ff00 }
        </style></head><body>
        <a id="link" href="/destination"></a>
        <map><area id="area" href="/destination"></map>
        </body></html>"#,
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
    let snapshot = |runtime: &mut JsRuntime| {
        let value = runtime
            .eval(
                r#"(() => {
                    const link = document.getElementById('link');
                    const area = document.getElementById('area');
                    return JSON.stringify({
                        visited: link.matches(':visited'),
                        link: link.matches(':link'),
                        anyLink: link.matches(':any-link'),
                        nestedVisited: link.matches(':is(:visited)'),
                        nestedLink: link.matches(':is(:link, :visited)'),
                        negatedVisited: link.matches(':not(:visited)'),
                        closestVisited: link.closest(':visited') !== null,
                        closestUnvisited: link.closest('a:not(:visited)') === link,
                        queryVisited: document.querySelector(':visited') !== null,
                        queryNestedVisited: document.querySelectorAll(':is(:visited)').length,
                        queryLinkList: document.querySelectorAll('a:visited, a:link, area:link').length,
                        hasVisited: document.querySelector('body:has(a:visited)') !== null,
                        hasLink: document.querySelector('body:has(a:link)') === document.body,
                        areaLink: area.matches(':link')
                    });
                })()"#,
            )
            .unwrap();
        serde_json::from_str::<serde_json::Value>(
            &value.as_string().unwrap().to_std_string_escaped(),
        )
        .unwrap()
    };
    let before = snapshot(&mut runtime);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(20, 20),
        Some(crate::paint::Color::rgb(255, 0, 0))
    );

    let destination = "https://example.test/destination";
    let source = VisitSource::new(
        StorageOrigin::from_url("https://example.test/start").unwrap(),
        "https://example.test/start",
    )
    .unwrap();
    storage.record_page_navigation(destination, destination, source);
    assert_eq!(
        runtime.paint_current_document().unwrap().pixel(20, 20),
        Some(crate::paint::Color::rgb(0, 255, 0))
    );
    let after = snapshot(&mut runtime);
    assert_eq!(after, before, "DOM selectors must not reveal the visit");
    assert_eq!(
        after,
        serde_json::json!({
            "visited": false,
            "link": true,
            "anyLink": true,
            "nestedVisited": false,
            "nestedLink": true,
            "negatedVisited": true,
            "closestVisited": false,
            "closestUnvisited": true,
            "queryVisited": false,
            "queryNestedVisited": 0,
            "queryLinkList": 2,
            "hasVisited": false,
            "hasLink": true,
            "areaLink": true
        })
    );
}

fn native_visited_scene(
    handle: &native_scroll::NativeScrollHandle,
    publication: u64,
) -> native_scroll::Scene {
    let gate = handle.publication_gate();
    native_scroll::Scene {
        key: gate.key,
        publication: native_scroll::PublicationSequence::new(publication),
        policy_epoch: gate.policy_epoch,
        acceptance_epoch: gate.acceptance_epoch,
        acknowledged: 0,
        completed_fallback: native_scroll::FallbackTicket::default(),
        offset: native_scroll::Offset::default(),
        extent: native_scroll::Bounds {
            min: native_scroll::Offset::default(),
            max: native_scroll::Offset { x: 0.0, y: 1000.0 },
        },
        coverage: native_scroll::Bounds {
            min: native_scroll::Offset::default(),
            max: native_scroll::Offset { x: 0.0, y: 900.0 },
        },
        eligible: true,
    }
}

fn native_visited_runtime(storage: &StorageManager) -> JsRuntime {
    JsRuntime::with_document_url_and_storage(
        TreeBuilder::parse("<a href='/next'>next</a>").document(),
        "https://example.test/start",
        storage.clone(),
        storage.create_session(),
    )
    .unwrap()
}

#[test]
fn another_runtime_visit_revokes_an_authorized_scene_and_its_inflight_token() {
    let storage = StorageManager::new();
    let mut first = native_visited_runtime(&storage);
    let second = native_visited_runtime(&storage);
    let key = native_scroll::TargetKey {
        document: 1,
        owner_generation: 1,
        viewport_epoch: 1,
        scroll_node: 1,
    };
    let handle = first.activate_native_scroll(key);
    let initial = native_visited_scene(&handle, 1);
    assert!(handle.authorize_policy(key, initial.policy_epoch));
    assert!(handle.install_scene(initial));
    let token = match handle.scroll(native_scroll::Offset { x: 0.0, y: 10.0 }) {
        native_scroll::ScrollDecision::Accepted(token) => token,
        other => panic!("expected authorized native scroll: {other:?}"),
    };
    second
        .host_state
        .borrow()
        .storage_manager
        .record_page_navigation(
            "https://example.test/next",
            "https://example.test/next",
            VisitSource::new(
                StorageOrigin::from_url("https://example.test/start").unwrap(),
                "https://example.test/start",
            )
            .unwrap(),
        );
    assert_ne!(handle.publication_gate().policy_epoch, initial.policy_epoch);
    assert!(!handle.publication_gate().eligible);
    assert!(!handle.validate_for_presentation(token));
    assert!(!handle.authorize_policy(key, initial.policy_epoch));
    assert_eq!(handle.owner_sequences(key), Ok((0, 0)));
    assert_eq!(handle.notification_backlog(key), Ok(1));
}

#[test]
fn visited_observer_follows_owner_replacement_and_fresh_scene_recovers() {
    let storage = StorageManager::new();
    let mut runtime = native_visited_runtime(&storage);
    let old = native_scroll::TargetKey {
        document: 1,
        owner_generation: 1,
        viewport_epoch: 1,
        scroll_node: 1,
    };
    let handle = runtime.activate_native_scroll(old);
    let initial = native_visited_scene(&handle, 1);
    assert!(handle.authorize_policy(old, initial.policy_epoch));
    assert!(handle.install_scene(initial));
    let token = match handle.scroll(native_scroll::Offset { x: 0.0, y: 5.0 }) {
        native_scroll::ScrollDecision::Accepted(token) => token,
        other => panic!("expected accepted token: {other:?}"),
    };
    let replacement = native_scroll::TargetKey {
        document: 2,
        owner_generation: 2,
        ..old
    };
    runtime.activate_native_scroll(replacement);
    storage.record_page_navigation(
        "https://example.test/next",
        "https://example.test/next",
        VisitSource::new(
            StorageOrigin::from_url("https://example.test/start").unwrap(),
            "https://example.test/start",
        )
        .unwrap(),
    );
    let current = native_visited_scene(&handle, 1);
    assert!(handle.authorize_policy(replacement, current.policy_epoch));
    assert!(handle.install_scene(current));
    assert!(!handle.validate_for_presentation(token));
    assert!(
        handle.publication_gate().eligible,
        "old owner must not poison replacement"
    );
    assert!(matches!(
        handle.scroll(native_scroll::Offset { x: 0.0, y: 3.0 }),
        native_scroll::ScrollDecision::Accepted(_)
    ));
}

#[test]
fn a_fresh_visited_capture_can_reauthorize_the_same_owner() {
    let storage = StorageManager::new();
    let runtime = native_visited_runtime(&storage);
    let handle = runtime.native_scroll_handle();
    let initial = native_visited_scene(&handle, 1);
    assert!(handle.authorize_policy(initial.key, initial.policy_epoch));
    assert!(handle.install_scene(initial));
    storage.record_page_navigation(
        "https://example.test/next",
        "https://example.test/next",
        VisitSource::new(
            StorageOrigin::from_url("https://example.test/start").unwrap(),
            "https://example.test/start",
        )
        .unwrap(),
    );
    let before = handle.publication_gate();
    assert!(!before.eligible);
    assert!(
        !runtime
            .host_state
            .borrow()
            .visited_link_ids_for_paint(&runtime.document())
            .is_empty()
    );
    let after = handle.publication_gate();
    assert_eq!(before.key, after.key);
    assert_eq!(before.policy_epoch, after.policy_epoch);
    assert!(handle.authorize_policy(after.key, after.policy_epoch));
    assert!(handle.install_scene(native_visited_scene(&handle, 2)));
    assert!(matches!(
        handle.scroll(native_scroll::Offset { x: 0.0, y: 1.0 }),
        native_scroll::ScrollDecision::Accepted(_)
    ));
}
