use omoikane::html::TreeBuilder;
use omoikane::js::JsRuntime;

fn runtime() -> JsRuntime {
    let document = TreeBuilder::parse("<html><body></body></html>").document();
    JsRuntime::with_document_and_url(document, "https://example.test/").unwrap()
}

fn check(runtime: &mut JsRuntime, source: &str) {
    let value = runtime.eval(source).unwrap();
    assert_eq!(value.as_boolean(), Some(true), "{value:?}");
}

#[test]
fn initial_blank_is_accessible_before_cross_origin_data_navigation() {
    let mut runtime = runtime();
    check(
        &mut runtime,
        r#"(() => {
            globalThis.frame = document.createElement('iframe');
            frame.src = 'data:text/html,<p>opaque</p>';
            document.body.append(frame);
            return frame.contentDocument?.URL === 'about:blank' &&
                frame.contentWindow.location.href === 'about:blank';
        })()"#,
    );
    runtime.run_timers(10, 1, 100);
    check(&mut runtime, "frame.contentDocument === null");
}
