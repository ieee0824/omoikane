use omoikane::{html::TreeBuilder, js::JsRuntime};

#[test]
fn active_view_transition_tracks_the_document_lifecycle_and_invalidates_style() {
    let document = TreeBuilder::parse(
        "<style>:root { color: red } :root:active-view-transition { color: green }</style>",
    )
    .document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime");

    assert_eq!(
        runtime
            .eval(
                "JSON.stringify([\
                   CSS.supports('selector(:active-view-transition)'),\
                   document.documentElement.matches(':active-view-transition'),\
                   getComputedStyle(document.documentElement).color,\
                   document.activeViewTransition])",
            )
            .expect("initial state")
            .as_string()
            .expect("json")
            .to_std_string_escaped(),
        "[true,false,\"rgb(255, 0, 0)\",null]"
    );

    assert!(
        runtime
            .eval(
                "globalThis.transition = document.startViewTransition(() => {\
                   document.documentElement.dataset.updated = 'yes';\
                 });\
                 transition instanceof ViewTransition &&\
                 document.activeViewTransition === transition &&\
                 document.documentElement.matches(':active-view-transition') &&\
                 getComputedStyle(document.documentElement).color === 'rgb(0, 128, 0)'",
            )
            .expect("active state")
            .as_boolean()
            == Some(true)
    );

    runtime.run_jobs().expect("transition jobs");
    assert!(
        runtime
            .eval(
                "document.documentElement.dataset.updated === 'yes' &&\
                 document.activeViewTransition === null &&\
                 !document.documentElement.matches(':active-view-transition') &&\
                 getComputedStyle(document.documentElement).color === 'rgb(255, 0, 0)'",
            )
            .expect("finished state")
            .as_boolean()
            == Some(true)
    );
}

#[test]
fn view_transition_promises_and_skip_preserve_callback_completion() {
    let document = TreeBuilder::parse("<main></main>").document();
    let mut runtime = JsRuntime::with_document(document).expect("runtime");
    assert!(
        runtime
            .eval(
                "ViewTransition.length === 0 && document.startViewTransition.length === 0 &&\
                 !Object.prototype.propertyIsEnumerable.call(globalThis, 'ViewTransition') &&\
                 ['updateCallbackDone','ready','finished','skipTransition'].every(\
                   name => ViewTransition.prototype.propertyIsEnumerable(name)) &&\
                 ['startViewTransition','activeViewTransition'].every(\
                   name => Document.prototype.propertyIsEnumerable(name))",
            )
            .expect("IDL metadata")
            .as_boolean()
            == Some(true)
    );
    runtime
        .eval(
            "globalThis.events = [];\
             globalThis.transition = document.startViewTransition(async () => {\
               events.push('update');\
             });\
             transition.updateCallbackDone.then(() => events.push('update-done'));\
             transition.ready.then(() => events.push('ready'), error => events.push(error.name));\
             transition.finished.then(() => events.push('finished'));\
             transition.skipTransition();",
        )
        .expect("start and skip");
    runtime.run_jobs().expect("transition jobs");
    assert_eq!(
        runtime
            .eval("JSON.stringify(events)")
            .expect("events")
            .as_string()
            .expect("json")
            .to_std_string_escaped(),
        "[\"update\",\"AbortError\",\"update-done\",\"finished\"]"
    );
}
