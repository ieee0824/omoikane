//! Link activation should schedule a navigation only after an uncanceled click.

use omoikane::html::TreeBuilder;
use omoikane::js::{JsRuntime, NavigationRequest};

#[test]
fn link_and_area_clicks_follow_their_href() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><head><base href="https://example.test/base/"></head><body>
        <a id="link" href="next"><span id="child">next</span></a>
        <map><area id="area" href="/map"></map>
        </body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/start").unwrap();

    runtime
        .eval("document.getElementById('child').click()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.take_navigation_requests(),
        vec![NavigationRequest::Navigate {
            url: "https://example.test/base/next".into(),
            replace: false,
        }]
    );

    runtime
        .eval("document.getElementById('area').click()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime.take_navigation_requests(),
        vec![NavigationRequest::Navigate {
            url: "https://example.test/map".into(),
            replace: false,
        }]
    );
}

#[test]
fn canceled_missing_and_detached_links_do_not_navigate() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><body>
        <a id="link" href="/next"><span id="child">next</span></a>
        </body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/start").unwrap();

    runtime
        .eval(
            r#"const link = document.getElementById('link');
               link.addEventListener('click', event => event.preventDefault());
               document.getElementById('child').click();"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_navigation_requests().is_empty());

    runtime
        .eval("link.removeAttribute('href'); link.click()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_navigation_requests().is_empty());

    runtime
        .eval("link.setAttribute('href', '/next'); link.remove(); link.click()")
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn child_realm_link_navigates_its_own_frame() {
    let document = TreeBuilder::parse("<!doctype html><html><body></body></html>").document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/top").unwrap();
    let src = runtime
        .eval(
            r#"globalThis.frame = document.createElement('iframe');
               frame.srcdoc = '<a id="inside" href="https://destination.test/next">next</a>';
               document.body.appendChild(frame);
               frame.contentWindow.eval('document.getElementById("inside").click()');
               frame.getAttribute('src')"#,
        )
        .unwrap();
    assert_eq!(
        src.as_string().map(|value| value.to_std_string_escaped()),
        Some("https://destination.test/next".to_string())
    );
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn named_iframe_target_uses_the_existing_frame() {
    let document = TreeBuilder::parse(
        "<!doctype html><html><body><iframe name='results' srcdoc='<p>initial</p>'></iframe><a id='link' href='https://destination.test/next' target='results'>next</a></body></html>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/top").unwrap();
    let result = runtime
        .eval(
            r#"const frame = document.querySelector('iframe');
               const originalWindow = frame.contentWindow;
               document.getElementById('link').click();
               frame.contentWindow === originalWindow &&
                 frame.getAttribute('src') === 'https://destination.test/next'"#,
        )
        .unwrap();
    assert!(result.to_boolean());
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn named_cross_origin_iframe_target_uses_location_write() {
    let document = TreeBuilder::parse(
        "<!doctype html><html><body><iframe name='results' src='data:text/html,initial'></iframe><a id='link' href='https://destination.test/next' target='results'>next</a></body></html>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/top").unwrap();
    let result = runtime
        .eval(
            r#"const frame = document.querySelector('iframe');
               const originalWindow = frame.contentWindow;
               document.getElementById('link').click();
               frame.contentWindow === originalWindow &&
                 frame.getAttribute('src') === 'https://destination.test/next'"#,
        )
        .unwrap();
    assert!(result.to_boolean());
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn child_link_can_target_a_named_sibling_frame_in_its_parent() {
    let document = TreeBuilder::parse(
        r#"<!doctype html><html><body>
        <iframe id="source" srcdoc='<a href="https://destination.test/next" target="results">next</a>'></iframe>
        <iframe id="result" name="results" srcdoc='<p>initial</p>'></iframe>
        </body></html>"#,
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/top").unwrap();
    let result = runtime
        .eval(
            r#"const source = document.getElementById('source');
               const target = document.getElementById('result');
               const originalWindow = target.contentWindow;
               source.contentWindow.eval('document.querySelector("a").click()');
               target.contentWindow === originalWindow &&
                 target.getAttribute('src') === 'https://destination.test/next'"#,
        )
        .unwrap();
    assert!(result.to_boolean());
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn cross_origin_child_link_targets_named_parent_frame_without_parent_dom_access() {
    let document = TreeBuilder::parse(
        "<!doctype html><html><body><iframe id='source'></iframe><iframe id='result' name='results' srcdoc='<p>initial</p>'></iframe></body></html>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/top").unwrap();
    runtime
        .eval(
            r#"const source = document.getElementById('source');
               source.src = "data:text/html,<a id='link' href='data:text/html,done' target='results'>next</a><script>document.getElementById('link').click()</script>";"#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert!(runtime
        .eval(
            "source.contentDocument === null && document.getElementById('result').getAttribute('src') === 'data:text/html,done'",
        )
        .unwrap()
        .to_boolean());
    assert!(runtime.take_navigation_requests().is_empty());
}

#[test]
fn named_target_uses_iframe_departure_and_history_controller() {
    let document = TreeBuilder::parse(
        "<!doctype html><html><body><iframe name='results' srcdoc='<p>initial</p>'></iframe><a href='about:blank' target='results'>next</a></body></html>",
    )
    .document();
    let mut runtime =
        JsRuntime::with_document_and_url(document, "https://example.test/top").unwrap();
    let departed = runtime
        .eval(
            r#"const frame = document.querySelector('iframe');
               const proxy = frame.contentWindow;
               globalThis.departure = [];
               proxy.addEventListener('pagehide', () => departure.push('pagehide'));
               document.querySelector('a').click();
               frame.contentWindow === proxy && departure.join(',') === 'pagehide'"#,
        )
        .unwrap();
    assert!(departed.to_boolean());
    runtime.run_until_idle().unwrap();
    assert!(runtime
        .eval("frame.contentWindow.history.length === 2 && frame.getAttribute('src') === 'about:blank'")
        .unwrap()
        .to_boolean());
}
