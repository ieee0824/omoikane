use std::{cell::Cell, rc::Rc};

use crate::{Context, JsData, JsObject, JsValue, NativeFunction, Source, js_string};

#[derive(Debug, boa_gc::Trace, JsData)]
struct GetTrapResult {
    #[unsafe_ignore_trace]
    finalized: Rc<Cell<bool>>,
}

impl boa_gc::Finalize for GetTrapResult {
    fn finalize(&self) {
        self.finalized.set(true);
    }
}

#[derive(boa_gc::Trace, boa_gc::Finalize)]
struct GetTrapResultFactory {
    #[unsafe_ignore_trace]
    finalized: Rc<Cell<bool>>,
}

#[test]
fn get_trap_object_result_survives_reentrant_collection() {
    let mut context = Context::default();
    let finalized = Rc::new(Cell::new(false));
    context
        .register_global_builtin_callable(
            js_string!("makeResult"),
            0,
            NativeFunction::from_copy_closure_with_captures(
                |_, _, captures, _| {
                    Ok(JsObject::from_proto_and_data(
                        None,
                        GetTrapResult {
                            finalized: Rc::clone(&captures.finalized),
                        },
                    )
                    .into())
                },
                GetTrapResultFactory {
                    finalized: Rc::clone(&finalized),
                },
            ),
        )
        .unwrap();
    context
        .register_global_builtin_callable(
            js_string!("collect"),
            0,
            NativeFunction::from_fn_ptr(|_, _, _| {
                boa_gc::force_collect();
                Ok(JsValue::undefined())
            }),
        )
        .unwrap();

    let result = context
        .eval(Source::from_bytes(
            r#"
                const target = new Proxy({}, {
                    getOwnPropertyDescriptor() {
                        collect();
                        return undefined;
                    },
                });
                const proxy = new Proxy(target, {
                    get() {
                        return makeResult();
                    },
                });
                proxy.value;
            "#,
        ))
        .unwrap();

    assert!(
        !finalized.get(),
        "Proxy get trap result was collected during invariant checks"
    );
    assert!(result.as_object().unwrap().is::<GetTrapResult>());
}
