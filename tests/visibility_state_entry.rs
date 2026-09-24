use omoikane::{cdp::CdpSession, js::JsRuntime};
use serde_json::{Value, json};

fn evaluate(session: &mut CdpSession, expression: &str) -> Value {
    session
        .dispatch("Runtime.evaluate", json!({ "expression": expression }))
        .unwrap()["result"]["value"]
        .clone()
}

#[test]
fn initial_visibility_entry_is_visible_at_time_zero() {
    let mut runtime = JsRuntime::new().unwrap();
    assert!(
        runtime
            .eval(
                "typeof VisibilityStateEntry === 'function' && \
             PerformanceObserver.supportedEntryTypes.includes('visibility-state') && \
             (() => { const entries = performance.getEntriesByType('visibility-state'); \
               return entries.length === 1 && entries[0] instanceof VisibilityStateEntry && \
                 entries[0] instanceof PerformanceEntry && entries[0].name === 'visible' && \
                 entries[0].startTime === 0 && entries[0].duration === 0; })()",
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn visibility_transitions_record_entries_and_deliver_buffered_observer_results() {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval(
            "globalThis.delivered = []; \
             const observer = new PerformanceObserver(list => \
               delivered.push(...list.getEntries().map(entry => entry.name))); \
             observer.observe({ type: 'visibility-state', buffered: true });",
        )
        .unwrap();
    runtime.run_jobs().unwrap();
    runtime.set_page_visibility(true);
    runtime.set_page_visibility(true);
    runtime.set_page_visibility(false);
    runtime.run_jobs().unwrap();
    assert!(
        runtime
            .eval(
                "performance.getEntriesByType('visibility-state').map(e => e.name).join(',') === \
             'visible,hidden,visible' && delivered.join(',') === 'visible,hidden,visible'",
            )
            .unwrap()
            .to_boolean()
    );
}

#[test]
fn new_document_and_iframe_start_with_hidden_entry_when_host_is_hidden() {
    let mut session = CdpSession::new().unwrap();
    session.set_host_visibility(true);
    session
        .dispatch(
            "Page.navigate",
            json!({ "url": "data:text/html,<p>hidden</p>" }),
        )
        .unwrap();
    assert_eq!(
        evaluate(
            &mut session,
            "(() => { const parent = performance.getEntriesByType('visibility-state'); \
             const frame = document.createElement('iframe'); document.body.appendChild(frame); \
             const child = frame.contentWindow.performance.getEntriesByType('visibility-state'); \
             return parent.length === 1 && parent[0].name === 'hidden' && \
               parent[0].startTime === 0 && child.length === 1 && child[0].name === 'hidden' && \
               child[0].startTime === 0; })()"
        ),
        true
    );
}
