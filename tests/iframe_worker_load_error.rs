//! Runs the deadline-sensitive iframe/Worker startup regression in its own
//! integration binary, apart from the parallel worker initialization suite.
//! Keep the original sandbox limit and every event-Realm assertion.
use omoikane::js::JsRuntime;

#[test]
fn iframe_worker_load_error_uses_the_owner_event_realm() {
    let document = omoikane::html::TreeBuilder::parse(
        "<html><body><iframe id='frame'></iframe></body></html>",
    )
    .document();
    let mut runtime = JsRuntime::with_document_and_url(document, "https://example.com/").unwrap();
    runtime
        .eval(
            r#"
        globalThis.child = document.getElementById('frame').contentWindow;
        child.eval(`globalThis.loadEvents = [];
            for (const Constructor of [Worker, SharedWorker]) {
                const worker = new Constructor('data:text/plain,not-a-script');
                worker.onerror = event => loadEvents.push([event.constructor === Event,
                    event instanceof Event, event.target === worker, event.isTrusted]);
            }
        `);
    "#,
        )
        .unwrap();
    runtime.run_until_idle().unwrap();
    assert_eq!(
        runtime
            .eval("JSON.stringify(child.loadEvents)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped(),
        "[[true,true,true,true],[true,true,true,true]]"
    );
}
