//! Promise handler and microtask behavior through Omoikane's public runtime.

use omoikane::js::JsRuntime;

#[test]
fn handler_attached_before_rejection_receives_the_reason() {
    let mut runtime = JsRuntime::new().expect("runtime initialization");
    runtime
        .eval(
            "globalThis.reasons = [];
             let rejectLater;
             const promise = new Promise((_, reject) => { rejectLater = reject; });
             promise.catch(reason => reasons.push(reason));
             Promise.resolve().then(() => rejectLater('later'));",
        )
        .expect("valid JavaScript");
    runtime.run_jobs().expect("promise jobs");

    assert_eq!(
        runtime
            .eval("reasons.length === 1 && reasons[0] === 'later'")
            .expect("valid JavaScript")
            .as_boolean(),
        Some(true)
    );
}
