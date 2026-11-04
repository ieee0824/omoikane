//! Exceptions must survive JavaScript run while clearing currentScript.
use super::*;

#[derive(boa_gc::Trace, boa_engine::JsData)]
struct CleanupExceptionMarker {
    #[unsafe_ignore_trace]
    finalized: Rc<Cell<bool>>,
}

impl boa_gc::Finalize for CleanupExceptionMarker {
    fn finalize(&self) {
        self.finalized.set(true);
    }
}

#[derive(boa_gc::Trace, boa_gc::Finalize)]
struct CleanupCollectionCounter {
    #[unsafe_ignore_trace]
    count: Rc<Cell<usize>>,
}

#[derive(boa_gc::Trace, boa_gc::Finalize)]
struct CleanupThrowCapture {
    #[unsafe_ignore_trace]
    finalized: Rc<Cell<bool>>,
    native_cause: bool,
}

fn check_exception_survives_cleanup(native_cause: bool) {
    let mut runtime = JsRuntime::new().unwrap();
    runtime
        .eval("const frame = document.createElement('iframe'); frame.srcdoc = '<body></body>'; document.body.appendChild(frame)")
        .unwrap();
    runtime.run_until_idle().unwrap();
    let (iframe, document) = {
        let state = runtime.host_state.borrow();
        let (iframe, entry) = state.iframe_documents.iter().next().unwrap();
        (*iframe, entry.document.identity())
    };
    let realm = runtime.ensure_iframe_realm(iframe, document).unwrap();
    let finalized = Rc::new(Cell::new(false));
    let collections = Rc::new(Cell::new(0usize));
    let old_realm = runtime.context.enter_realm(realm);
    let script = runtime
        .with_active_host(|context| {
            context.register_global_builtin_callable(
                js_string!("forceCleanupCollection"),
                0,
                NativeFunction::from_copy_closure_with_captures(
                    |_, _, counter, context| {
                        counter.count.set(counter.count.get() + 1);
                        #[cfg(feature = "gc-profile")]
                        let marker_on_native_global = context
                            .global_object()
                            .has_own_property(js_string!("cleanupThrown"), context)?;
                        context.clear_kept_objects();
                        #[cfg(feature = "gc-profile")]
                        let major_before = boa_gc::profile().major.collections;
                        boa_gc::force_collect();
                        #[cfg(feature = "gc-profile")]
                        eprintln!(
                            "CLEANUP_GC_OBSERVATION stage=hook major_before={} major_after={} native_global_has_marker={}",
                            major_before,
                            boa_gc::profile().major.collections,
                            marker_on_native_global,
                        );
                        Ok(JsValue::undefined())
                    },
                    CleanupCollectionCounter {
                        count: Rc::clone(&collections),
                    },
                ),
            )?;
            context.register_global_builtin_callable(
                js_string!("throwCleanupOwnedMarker"),
                0,
                NativeFunction::from_copy_closure_with_captures(
                    |_, _, capture, _| {
                        let marker = JsObject::from_proto_and_data(
                            None,
                            CleanupExceptionMarker {
                                finalized: Rc::clone(&capture.finalized),
                            },
                        );
                        let thrown = JsError::from_opaque(marker.into());
                        if capture.native_cause {
                            Err(JsNativeError::error()
                                .with_message("native cleanup cause")
                                .with_cause(thrown)
                                .into())
                        } else {
                            Err(thrown)
                        }
                    },
                    CleanupThrowCapture {
                        finalized: Rc::clone(&finalized),
                        native_cause,
                    },
                ),
            )?;
            context.eval(Source::from_bytes(
                "globalThis.__omoikane_set_current_script = ((original) => function(id) {\
                original(id); if (id === null) forceCleanupCollection();\
             })(globalThis.__omoikane_set_current_script);\
             const inert = document.createElement('script');\
             inert.type = 'application/json'; document.body.appendChild(inert); inert.__id;",
            ))
        })
        .unwrap()
        .as_number()
        .unwrap() as usize;
    runtime.context.enter_realm(old_realm);
    assert!(!finalized.get(), "setup must precede marker creation");
    assert_eq!(
        collections.get(),
        0,
        "setup must not run cleanup collection"
    );
    let error = runtime
        .eval_iframe_script(iframe, document, script, "throwCleanupOwnedMarker()")
        .unwrap_err();
    // Check the finalizer before any assertion can unwind through this handle.
    if finalized.get() {
        // A failing implementation may already have invalidated this handle.
        // Do not inspect or drop it while reporting the lifetime violation.
        std::mem::forget(error);
        assert_eq!(collections.get(), 1, "unexpected cleanup collection count");
        panic!("cleanup collected the returned exception");
    }
    let error = Rooted::new(error);
    assert_eq!(collections.get(), 1, "cleanup must collect exactly once");
    assert_cleanup_exception_identity(&error, native_cause, &finalized, &mut runtime.context);
    drop(error);
    #[cfg(feature = "gc-profile")]
    let major_before = boa_gc::profile().major.collections;
    boa_gc::force_collect();
    #[cfg(feature = "gc-profile")]
    eprintln!(
        "CLEANUP_GC_OBSERVATION stage=post_release native_cause={} major_before={} major_after={} marker_finalized={}",
        native_cause,
        major_before,
        boa_gc::profile().major.collections,
        finalized.get(),
    );
    assert!(
        finalized.get(),
        "cleanup must release its temporary exception root"
    );
}

fn assert_cleanup_exception_identity(
    error: &JsError,
    native_cause: bool,
    expected: &Rc<Cell<bool>>,
    context: &mut Context,
) {
    let thrown = if native_cause {
        // Accept either retained native representation or its VM opaque form.
        let native = error
            .try_native(context)
            .expect("native Error must survive");
        assert!(matches!(native.kind, boa_engine::JsNativeErrorKind::Error));
        assert_eq!(native.message(), "native cleanup cause");
        native
            .cause()
            .expect("native cause must survive")
            .as_opaque()
            .expect("cause must retain the thrown object")
            .clone()
    } else {
        error
            .as_opaque()
            .expect("opaque throw must survive")
            .clone()
    };
    let object = thrown
        .as_object()
        .expect("thrown marker must remain an object");
    let marker = object
        .downcast_ref::<CleanupExceptionMarker>()
        .expect("thrown marker must retain its native data");
    assert!(
        Rc::ptr_eq(&marker.finalized, expected),
        "exception identity changed"
    );
}

#[test]
fn iframe_opaque_exception_survives_current_script_cleanup_collection() {
    check_exception_survives_cleanup(false);
}

#[test]
fn iframe_native_cause_survives_current_script_cleanup_collection() {
    check_exception_survives_cleanup(true);
}
