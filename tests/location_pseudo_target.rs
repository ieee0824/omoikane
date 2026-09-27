use omoikane::css::{matches_selector, parse_selector_list};
use omoikane::dom::{Node, NodeType};
use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

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
