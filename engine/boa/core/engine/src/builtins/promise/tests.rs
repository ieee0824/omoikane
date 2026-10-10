use super::{OperationType, Promise};
use crate::{
    Context, JsValue, NativeFunction, Source, TestAction, context::HostHooks, js_string,
    object::JsObject, run_test_actions,
};
use indoc::indoc;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[test]
fn attached_handler_marks_promise_handled_in_every_state() {
    #[derive(Default)]
    struct RejectionHooks(RefCell<Vec<OperationType>>);

    impl HostHooks for RejectionHooks {
        fn promise_rejection_tracker(
            &self,
            _promise: &JsObject,
            operation: OperationType,
            _context: &mut Context,
        ) {
            self.0.borrow_mut().push(operation);
        }
    }

    for (source, expected_events, reject_later) in [
        (
            "var promise = new Promise((_, reject) => { globalThis.rejectPromise = reject; }); promise.catch(() => {});",
            Vec::new(),
            true,
        ),
        (
            "var promise = Promise.resolve(1); promise.then(() => {});",
            Vec::new(),
            false,
        ),
        (
            "var promise = Promise.reject('oops'); promise.catch(() => {});",
            vec![OperationType::Reject, OperationType::Handle],
            false,
        ),
    ] {
        let hooks = Rc::new(RejectionHooks::default());
        let mut context = Context::builder()
            .host_hooks(hooks.clone())
            .build()
            .unwrap();
        context.eval(Source::from_bytes(source)).unwrap();

        let promise = context
            .global_object()
            .get(js_string!("promise"), &mut context)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        assert!(
            promise.downcast_ref::<Promise>().unwrap().handled,
            "{source}"
        );

        if reject_later {
            context
                .eval(Source::from_bytes("rejectPromise('oops')"))
                .unwrap();
        }
        context.run_jobs().unwrap();
        assert_eq!(*hooks.0.borrow(), expected_events, "{source}");
    }
}

#[test]
fn promise() {
    run_test_actions([
        TestAction::run(indoc! {r#"
                    let count = 0;
                    const promise = new Promise((resolve, reject) => {
                        count += 1;
                        resolve(undefined);
                    }).then((_) => (count += 1));
                    count += 1;
                "#}),
        TestAction::assert_eq("count", 2),
        TestAction::inspect_context(|ctx| ctx.run_jobs().unwrap()),
        TestAction::assert_eq("count", 3),
    ]);
}

#[test]
fn resolving_promise_survives_collection_in_then_getter() {
    for (then_body, expected) in [
        ("return done => done(42)", "fulfilled:42"),
        ("throw 42", "rejected:42"),
        ("return undefined", "fulfilled:[object Object]"),
    ] {
        let source = format!(
            "let resolve, reject, result, getterCalls = 0;\
             new Promise((yes, no) => {{ resolve = yes; reject = no; }}).then(\
                 value => result = 'fulfilled:' + value,\
                 reason => result = 'rejected:' + reason);\
             resolve({{ get then() {{\
                 getterCalls++;\
                 forceCollect();\
                 resolve(99);\
                 reject(99);\
                 {then_body};\
             }} }});"
        );
        run_test_actions([
            TestAction::inspect_context(|ctx| {
                ctx.register_global_builtin_callable(
                    js_string!("forceCollect"),
                    0,
                    NativeFunction::from_fn_ptr(|_, _, _| {
                        boa_gc::force_collect();
                        Ok(JsValue::undefined())
                    }),
                )
                .unwrap();
            }),
            TestAction::run(source),
            TestAction::inspect_context(|ctx| ctx.run_jobs().unwrap()),
            TestAction::assert_eq("getterCalls", 1),
            TestAction::assert_eq("result", js_string!(expected)),
        ]);
    }
}

/// Bounded garbage used to cross the normal 4 MiB nursery threshold.
/// Its finalizer confirms that a real collection ran during the finally job.
#[derive(Debug)]
struct FinallyNurseryPadding {
    _bytes: [u8; 4096],
    finalized: Rc<Cell<usize>>,
}

impl boa_gc::Finalize for FinallyNurseryPadding {
    fn finalize(&self) {
        self.finalized.set(self.finalized.get() + 1);
    }
}

// SAFETY: The byte array and counter contain no garbage-collected handles.
unsafe impl boa_gc::Trace for FinallyNurseryPadding {
    boa_gc::empty_trace!();
}

#[derive(Debug, boa_gc::Trace, boa_gc::Finalize)]
struct FinallyCollectionCounter {
    // The shared integer holds no garbage-collected values.
    #[unsafe_ignore_trace]
    count: Rc<Cell<usize>>,
}

#[test]
fn finally_intermediate_promise_survives_thunk_allocation_collection() {
    for (settle, expected) in [("resolve", "fulfilled:42"), ("reject", "rejected:42")] {
        let finalized = Rc::new(Cell::new(0));
        let mut context = Context::default();
        context
            .register_global_builtin_callable(
                js_string!("cleanupReturningPromise"),
                0,
                NativeFunction::from_copy_closure_with_captures(
                    |_, _, finalized, context| {
                        // Reset the nursery before creating the promise under test.
                        boa_gc::force_collect();
                        let promise = crate::object::builtins::JsPromise::resolve(
                            JsValue::undefined(),
                            context,
                        );
                        {
                            let _no_gc = boa_gc::NoGcScope::new();
                            // 1025 * 4096 payload bytes alone exceed 4 MiB.
                            // NoGcScope::drop only resumes collection. The default
                            // PromiseResolve identity path performs no GC allocation,
                            // so the next allocation is the finally thunk's closure.
                            for _ in 0..1025 {
                                let _garbage = boa_gc::GcEdge::new(FinallyNurseryPadding {
                                    _bytes: [0; 4096],
                                    finalized: finalized.count.clone(),
                                });
                            }
                        }
                        Ok(promise.into())
                    },
                    FinallyCollectionCounter {
                        count: finalized.clone(),
                    },
                ),
            )
            .unwrap();
        let source = format!(
            r#"
                const original = {{ expected: 42 }};
                let outcome = 'pending', cleanupCalls = 0;
                Promise.{settle}(original).finally(() => {{
                    cleanupCalls++;
                    return cleanupReturningPromise();
                }}).then(
                    value => outcome = value === original ? 'fulfilled:42' : 'bad-value',
                    reason => outcome = reason === original ? 'rejected:42' : 'bad-reason'
                );
            "#
        );
        context.eval(Source::from_bytes(&source)).unwrap();
        context.run_jobs().unwrap();
        assert!(
            finalized.get() >= 1025,
            "the nursery garbage must be collected"
        );
        assert_eq!(
            context.eval(Source::from_bytes("cleanupCalls")).unwrap(),
            JsValue::from(1)
        );
        assert_eq!(
            context.eval(Source::from_bytes("outcome")).unwrap(),
            JsValue::from(js_string!(expected))
        );
    }
}

#[test]
fn finally_thunk_survives_collection_in_then_getter() {
    for (settle, expected) in [("resolve", "fulfilled:42"), ("reject", "rejected:42")] {
        let collections = Rc::new(Cell::new(0));
        let mut context = Context::default();
        context
            .register_global_builtin_callable(
                js_string!("forceCollect"),
                0,
                NativeFunction::from_copy_closure_with_captures(
                    |_, _, collections, _| {
                        boa_gc::force_collect();
                        collections.count.set(collections.count.get() + 1);
                        Ok(JsValue::undefined())
                    },
                    FinallyCollectionCounter {
                        count: collections.clone(),
                    },
                ),
            )
            .unwrap();
        let source = format!(
            r#"
                const original = {{ expected: 42 }}, originalThen = Promise.prototype.then;
                let outcome = 'pending', cleanupCalls = 0, getterCalls = 0;
                Promise.{settle}(original).finally(() => {{
                    cleanupCalls++;
                    const intermediate = Promise.resolve();
                    Object.defineProperty(intermediate, 'then', {{ get() {{
                        getterCalls++;
                        forceCollect();
                        return originalThen;
                    }} }});
                    return intermediate;
                }}).then(
                    value => outcome = value === original ? 'fulfilled:42' : 'bad-value',
                    reason => outcome = reason === original ? 'rejected:42' : 'bad-reason'
                );
            "#
        );
        context.eval(Source::from_bytes(&source)).unwrap();
        context.run_jobs().unwrap();
        assert_eq!(collections.get(), 1, "the custom then getter must collect");
        for name in ["cleanupCalls", "getterCalls"] {
            assert_eq!(
                context.eval(Source::from_bytes(name)).unwrap(),
                JsValue::from(1)
            );
        }
        assert_eq!(
            context.eval(Source::from_bytes("outcome")).unwrap(),
            JsValue::from(js_string!(expected))
        );
    }
}
