use super::{OperationType, Promise};
use crate::{
    Context, JsValue, NativeFunction, Source, TestAction, context::HostHooks, js_string,
    object::JsObject, run_test_actions,
};
use indoc::indoc;
use std::{cell::RefCell, rc::Rc};

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
