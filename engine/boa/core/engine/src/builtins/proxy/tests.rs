use std::{cell::Cell, rc::Rc};

use crate::{
    Context, JsData, JsNativeError, JsObject, JsValue, NativeFunction, Source, js_string,
    object::FunctionObjectBuilder, property::PropertyDescriptor,
};

#[derive(Debug, boa_gc::Trace, JsData)]
struct TrapResult {
    #[unsafe_ignore_trace]
    finalized: Rc<Cell<bool>>,
}

impl boa_gc::Finalize for TrapResult {
    fn finalize(&self) {
        self.finalized.set(true);
    }
}

#[derive(boa_gc::Trace, boa_gc::Finalize)]
struct TrapResultState {
    #[unsafe_ignore_trace]
    finalized: Rc<Cell<bool>>,
    #[unsafe_ignore_trace]
    checked: Rc<Cell<bool>>,
}

fn install_result_factory(context: &mut Context, state: TrapResultState) {
    context
        .register_global_builtin_callable(
            js_string!("makeResult"),
            0,
            NativeFunction::from_copy_closure_with_captures(
                |_, _, state, context| {
                    let result = JsObject::from_proto_and_data(
                        None,
                        TrapResult {
                            finalized: Rc::clone(&state.finalized),
                        },
                    );
                    let _result_root = result.clone().root();
                    result.create_data_property_or_throw(
                        js_string!("configurable"),
                        true,
                        context,
                    )?;
                    result.create_data_property_or_throw(js_string!("value"), 42, context)?;
                    result.create_data_property_or_throw(
                        js_string!("0"),
                        js_string!("value"),
                        context,
                    )?;

                    let length_getter: JsObject = FunctionObjectBuilder::new(
                        context.realm(),
                        NativeFunction::from_fn_ptr(|_, _, context| {
                            context.eval(Source::from_bytes(
                                "({ valueOf() { assertLive(); return 1; } })",
                            ))
                        }),
                    )
                    .build()
                    .into();
                    let length_getter = length_getter.root();
                    result.define_property_or_throw(
                        js_string!("length"),
                        PropertyDescriptor::builder().get(length_getter.to_edge()),
                        context,
                    )?;
                    Ok(result.into())
                },
                TrapResultState {
                    finalized: Rc::clone(&state.finalized),
                    checked: Rc::clone(&state.checked),
                },
            ),
        )
        .unwrap();

    context
        .register_global_builtin_callable(
            js_string!("assertLive"),
            0,
            NativeFunction::from_copy_closure_with_captures(
                |_, _, state, _| {
                    boa_gc::force_collect();
                    state.checked.set(true);
                    if state.finalized.get() {
                        return Err(JsNativeError::error()
                            .with_message("Proxy trap result collected during invariant check")
                            .into());
                    }
                    Ok(JsValue::undefined())
                },
                state,
            ),
        )
        .unwrap();
}

fn test_state() -> TrapResultState {
    TrapResultState {
        finalized: Rc::new(Cell::new(false)),
        checked: Rc::new(Cell::new(false)),
    }
}

#[test]
fn get_trap_object_result_survives_reentrant_collection() {
    let mut context = Context::default();
    let state = test_state();
    install_result_factory(
        &mut context,
        TrapResultState {
            finalized: Rc::clone(&state.finalized),
            checked: Rc::clone(&state.checked),
        },
    );
    let result = context
        .eval(Source::from_bytes(
            r#"
                const target = new Proxy({}, {
                    getOwnPropertyDescriptor() { assertLive(); return undefined; },
                });
                const proxy = new Proxy(target, {
                    get() { return makeResult(); },
                });
                proxy.value;
            "#,
        ))
        .unwrap();

    assert!(state.checked.get());
    assert!(!state.finalized.get());
    assert!(result.as_object().unwrap().is::<TrapResult>());
}

#[test]
fn get_prototype_of_trap_object_survives_reentrant_collection() {
    let mut context = Context::default();
    let state = test_state();
    install_result_factory(
        &mut context,
        TrapResultState {
            finalized: Rc::clone(&state.finalized),
            checked: Rc::clone(&state.checked),
        },
    );
    let result = context
        .eval(Source::from_bytes(
            r#"
                const target = new Proxy({}, {
                    isExtensible() { assertLive(); return true; },
                });
                const proxy = new Proxy(target, {
                    getPrototypeOf() { return makeResult(); },
                });
                Object.getPrototypeOf(proxy);
            "#,
        ))
        .unwrap();

    assert!(state.checked.get());
    assert!(!state.finalized.get());
    assert!(result.as_object().unwrap().is::<TrapResult>());
}

#[test]
fn get_own_property_trap_object_survives_reentrant_collection() {
    let mut context = Context::default();
    let state = test_state();
    install_result_factory(
        &mut context,
        TrapResultState {
            finalized: Rc::clone(&state.finalized),
            checked: Rc::clone(&state.checked),
        },
    );
    let result = context
        .eval(Source::from_bytes(
            r#"
                const target = new Proxy({}, {
                    getOwnPropertyDescriptor() { assertLive(); return undefined; },
                });
                const proxy = new Proxy(target, {
                    getOwnPropertyDescriptor() { return makeResult(); },
                });
                Object.getOwnPropertyDescriptor(proxy, "value").value;
            "#,
        ))
        .unwrap();

    assert!(state.checked.get());
    assert!(!state.finalized.get());
    assert_eq!(result.as_number(), Some(42.0));
}

#[test]
fn own_property_keys_trap_object_survives_reentrant_collection() {
    let mut context = Context::default();
    let state = test_state();
    install_result_factory(
        &mut context,
        TrapResultState {
            finalized: Rc::clone(&state.finalized),
            checked: Rc::clone(&state.checked),
        },
    );
    let result = context
        .eval(Source::from_bytes(
            r#"
                const proxy = new Proxy({}, {
                    ownKeys() { return makeResult(); },
                });
                Reflect.ownKeys(proxy).join(",");
            "#,
        ))
        .unwrap();

    assert!(state.checked.get());
    assert!(!state.finalized.get());
    assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "value");
}
