use omoikane::css::{matches_selector, parse_selector_list};
use omoikane::dom::{Node, NodeType};
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;
use std::io::Write;

#[path = "support/http_fixture.rs"]
mod http_fixture;

use http_fixture::{
    ACCEPT_TIMEOUT, FixtureWorker, READ_TIMEOUT, accept_with_timeout, bind_loopback,
    read_request_headers,
};

const HTML: &str = r#"<!doctype html><html><head>
<style>:target { color: rgb(13, 42, 71) }</style></head><body>
<div id="first">first</div>
<a id="legacy-anchor" name="legacy">legacy</a>
<div id="legacy">id wins over name</div>
<a id="only-name" name="older">name fallback</a>
<div id="encoded%20value">raw fragment wins</div>
<div id="encoded value">decoded fragment</div>
<div id="only decoded">decoded fallback</div>
<div id="あ">utf8 fragment</div>
<div id="duplicate" data-order="first">first duplicate</div>
<div id="duplicate" data-order="second">second duplicate</div>
</body></html>"#;

#[test]
fn initial_fragment_selects_one_document_target_for_css_and_dom_queries() {
    let target_selector = parse_selector_list(":target").unwrap().remove(0);
    for (fragment, expected_id, expected_order) in [
        ("#first", Some("first"), None),
        ("#legacy", Some("legacy"), None),
        ("#older", Some("only-name"), None),
        ("#encoded%20value", Some("encoded%20value"), None),
        ("#only%20decoded", Some("only decoded"), None),
        ("#%E3%81%82", Some("あ"), None),
        ("#duplicate", Some("duplicate"), Some("first")),
        ("#", None, None),
        ("#missing", None, None),
        ("", None, None),
    ] {
        let document = TreeBuilder::parse(HTML).document();
        let mut runtime = JsRuntime::with_document_and_url(
            document.clone(),
            &format!("https://example.test/page{fragment}"),
        )
        .unwrap();
        let mut pending = document.child_nodes();
        let mut selected = None;
        while let Some(node) = pending.pop() {
            if node.node_type() == NodeType::Element && matches_selector(&node, &target_selector) {
                assert!(selected.is_none(), "multiple targets for {fragment}");
                selected = Some(node.clone());
            }
            pending.extend(node.child_nodes());
        }
        assert_eq!(
            selected.as_ref().and_then(|node| node.get_attribute("id")),
            expected_id.map(str::to_string),
            "fragment {fragment}"
        );
        assert_eq!(
            selected
                .as_ref()
                .and_then(|node| node.get_attribute("data-order")),
            expected_order.map(str::to_string),
            "fragment {fragment}"
        );
        let queried_id = runtime
            .eval("document.querySelector(':target')?.id ?? null")
            .unwrap();
        assert_eq!(
            queried_id.as_string().map(|id| id.to_std_string_escaped()),
            expected_id.map(str::to_string),
            "DOM query for {fragment}"
        );
        let result = runtime
            .eval(
                r#"(() => {
                    const target = document.querySelector(':target');
                    const targets = document.querySelectorAll(':target');
                    if (targets.length !== (target ? 1 : 0)) return false;
                    if (!target) return true;
                    return target.matches(':target') &&
                        getComputedStyle(target).color === 'rgb(13, 42, 71)';
                })()"#,
            )
            .unwrap();
        assert_eq!(result.as_boolean(), Some(true), "fragment {fragment}");
    }
}

#[test]
fn reusing_document_does_not_preserve_a_previous_runtime_target() {
    let document = TreeBuilder::parse(HTML).document();
    {
        let mut runtime =
            JsRuntime::with_document_and_url(document.clone(), "https://example.test/page#first")
                .unwrap();
        assert_eq!(
            runtime
                .eval("document.querySelector(':target')?.id === 'first'")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/page").unwrap();
    assert_eq!(
        runtime
            .eval("document.querySelector(':target') === null")
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

#[test]
fn iframe_fragment_navigation_preserves_its_document_and_targets_only_the_child() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><body><div id="section"></div>
           <iframe id="child" srcdoc="<style>:target { color: rgb(13, 42, 71) }</style><div id='section'></div><div id='other'></div>"></iframe></body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "http://127.0.0.1:1/page").unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const frame = document.getElementById('child');
                const child = frame.contentDocument;
                const changes = [];
                frame.contentWindow.addEventListener('hashchange', event => changes.push([event.oldURL, event.newURL]));
                frame.contentWindow.location.hash = '#section';
                const first = [frame.contentDocument === child,
                    child.querySelector(':target')?.id,
                    getComputedStyle(child.getElementById('section')).color,
                    frame.contentWindow.location.hash];
                frame.contentWindow.location.hash = '#other';
                const second = [frame.contentDocument === child,
                    child.querySelector(':target')?.id,
                    frame.contentWindow.location.hash];
                frame.contentWindow.history.back();
                const back = [frame.contentDocument === child,
                    child.querySelector(':target')?.id,
                    frame.contentWindow.location.hash];
                return JSON.stringify({ first, second, back, changes,
                    parentTarget: document.querySelector(':target')?.id ?? null });
            })()"#,
        )
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap();
    assert_eq!(
        report["first"],
        serde_json::json!([true, "section", "rgb(13, 42, 71)", "#section"])
    );
    assert_eq!(
        report["second"],
        serde_json::json!([true, "other", "#other"])
    );
    assert_eq!(
        report["back"],
        serde_json::json!([true, "section", "#section"])
    );
    assert_eq!(report["changes"].as_array().unwrap().len(), 3);
    assert!(report["parentTarget"].is_null());
}

#[test]
fn cross_origin_iframe_fragment_navigation_keeps_document_inaccessible() {
    let url = "data:text/html,%3Cdiv%20id%3D%22section%22%3E%3C%2Fdiv%3E";
    let document = TreeBuilder::parse(&format!(
        "<html><body><iframe id='child' src='{url}'></iframe></body></html>"
    ))
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/page").unwrap();
    let result = runtime
        .eval(&format!(
            r#"(() => {{
                const frame = document.getElementById('child');
                if (frame.contentDocument !== null) return 'unexpected access';
                try {{ frame.contentWindow.location.href = '{url}#section'; }}
                catch (error) {{ return `${{error.name}}: ${{error.message}}`; }}
                return frame.contentDocument === null ? 'ok' : 'unexpected access';
            }})()"#
        ))
        .unwrap();
    assert_eq!(
        result
            .as_string()
            .map(|value| value.to_std_string_escaped()),
        Some("ok".to_string())
    );
}

#[test]
fn iframe_initial_url_fragment_selects_its_own_target() {
    let listener = bind_loopback().unwrap();
    let address = listener.local_addr().unwrap();
    let server = FixtureWorker::spawn(move || {
        for _ in 0..2 {
            let mut stream = accept_with_timeout(&listener, ACCEPT_TIMEOUT).unwrap();
            read_request_headers(&mut stream, READ_TIMEOUT).unwrap();
            let body = "<style>:target { color: rgb(13, 42, 71) }</style><div id='section'></div>";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let child_url = format!("http://{address}/child#section");
    let document = TreeBuilder::parse(&format!(
        "<html><body><iframe id='absolute' src='{child_url}'></iframe><iframe id='relative' src='/relative#section'></iframe></body></html>"
    ))
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, &format!("http://{address}/page")).unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const summarize = id => {
                    const child = document.getElementById(id).contentDocument;
                    const target = child.querySelector(':target');
                    return [child.URL, target?.id ?? null,
                        target ? getComputedStyle(target).color : null];
                };
                return JSON.stringify([summarize('absolute'), summarize('relative'),
                    document.querySelector(':target')?.id ?? null]);
            })()"#,
        )
        .unwrap();
    server.join();
    let report: serde_json::Value =
        serde_json::from_str(&result.as_string().unwrap().to_std_string_escaped()).unwrap();
    assert_eq!(
        report,
        serde_json::json!([
            [child_url, "section", "rgb(13, 42, 71)"],
            [
                format!("http://{address}/relative#section"),
                "section",
                "rgb(13, 42, 71)"
            ],
            null
        ])
    );
}

#[test]
fn target_remains_selected_after_id_and_tree_mutations() {
    let document = TreeBuilder::parse(
        r#"<html><head><style>:target { color: rgb(13, 42, 71) }</style></head>
           <body><div id="fragment"></div><a name="fragment"></a></body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/page#fragment").unwrap();
    let result = runtime
        .eval(
            r#"(() => {
                const first = document.querySelector('div');
                const fallback = document.querySelector('a');
                if (document.querySelector(':target') !== first) return 'initial';
                first.id = 'renamed';
                if (document.querySelector(':target') !== first ||
                    getComputedStyle(first).color !== 'rgb(13, 42, 71)' ||
                    fallback.matches(':target')) return 'renamed';
                const replacement = document.createElement('div');
                replacement.id = 'fragment';
                document.body.appendChild(replacement);
                if (document.querySelector(':target') !== first ||
                    replacement.matches(':target')) return 'inserted';
                first.remove();
                if (document.querySelector(':target') !== null ||
                    !first.matches(':target')) return 'removed';
                document.body.appendChild(first);
                if (document.querySelector(':target') !== first) return 'reinserted';
                return 'ok';
            })()"#,
        )
        .unwrap();
    assert_eq!(
        result
            .as_string()
            .map(|value| value.to_std_string_escaped()),
        Some("ok".to_string())
    );
}
