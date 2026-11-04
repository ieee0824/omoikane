//! An input capability whose lifetime is one synchronous automation execution.
use super::*;

#[derive(Default)]
struct Lifetime {
    open: Cell<bool>,
    busy: Cell<bool>,
}

#[derive(Trace, Finalize)]
struct Capture {
    callback: JsValue,
    #[unsafe_ignore_trace]
    owner: Weak<RefCell<HostState>>,
    #[unsafe_ignore_trace]
    document: usize,
    #[unsafe_ignore_trace]
    lifetime: Rc<Lifetime>,
}

/// Owned by call_test_driver, never by page JavaScript. Drop revokes retained copies.
pub(super) struct InputScope {
    function: Rooted<JsObject>,
    lifetime: Rc<Lifetime>,
}

impl Drop for InputScope {
    fn drop(&mut self) {
        self.lifetime.open.set(false);
    }
}

struct Busy(Rc<Lifetime>);
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.set(false);
    }
}

fn invalid(message: &'static str) -> JsError {
    JsNativeError::typ().with_message(message).into()
}

impl InputScope {
    pub(super) fn new(runtime: &mut JsRuntime, document: usize) -> JsResult<Self> {
        let callback = {
            let state = runtime.host_state.borrow();
            if !state.input_bridge.automation.enabled() || !state.document_is_active(document) {
                return Err(invalid("Automation input Document is inactive"));
            }
            state
                .input_bridge
                .automation_input_callback(document)
                .ok_or_else(|| invalid("Automation input callback is unavailable"))?
        };
        let callback = Rooted::new(callback);
        let lifetime = Rc::new(Lifetime::default());
        lifetime.open.set(true);
        let capture = Capture {
            callback: (*callback).clone(),
            owner: Rc::downgrade(&runtime.host_state),
            document,
            lifetime: Rc::clone(&lifetime),
        };
        let function = FunctionObjectBuilder::new(
            runtime.context.realm(),
            NativeFunction::from_copy_closure_with_captures(invoke, capture),
        )
        .name(js_string!("automationInput"))
        .length(2)
        .build();
        Ok(Self {
            function: Rooted::new(function.into()),
            lifetime,
        })
    }

    pub(super) fn value(&self) -> JsValue {
        (*self.function).clone().into()
    }
}

fn validate_owner(capture: &Capture, context: &Context) -> JsResult<Rc<RefCell<HostState>>> {
    let owner = capture
        .owner
        .upgrade()
        .ok_or_else(|| invalid("Automation input owner is gone"))?;
    with_host_state(|active| {
        if !Rc::ptr_eq(active, &owner) {
            return Err(invalid("Automation input runtime mismatch"));
        }
        let state = active.borrow();
        if !state.input_bridge.automation.enabled() || !state.document_is_active(capture.document) {
            return Err(invalid("Automation input Document is inactive"));
        }
        Ok(())
    })?;
    let author = author_script_realm(context).and_then(|realm| {
        realm
            .host_defined()
            .get::<ModuleDocumentId>()
            .map(|id| id.0)
    });
    if author != Some(capture.document) {
        return Err(invalid("Automation input author Document mismatch"));
    }
    Ok(owner)
}

fn invoke(
    _: &JsValue,
    args: &[JsValue],
    capture: &Capture,
    context: &mut Context,
) -> JsResult<JsValue> {
    if !capture.lifetime.open.get() || capture.lifetime.busy.get() {
        return Err(invalid(
            "Automation input capability is expired or reentrant",
        ));
    }
    capture.lifetime.busy.set(true);
    let _busy = Busy(Rc::clone(&capture.lifetime));
    let owner = validate_owner(capture, context)?;
    if args.len() != 2 {
        return Err(invalid("Automation input needs kind and data"));
    }
    let kind = args[0]
        .as_string()
        .ok_or_else(|| invalid("Automation input kind must be a string"))?
        .to_std_string_escaped();
    if !matches!(kind.as_str(), "keyboard" | "mouse") {
        return Err(invalid("Unsupported automation input kind"));
    }
    // Conversion may invoke author accessors. No HostState borrow is held, and
    // the capability is busy so accessors/listeners cannot recursively dispatch.
    let arguments = Rooted::new(read_payload(&kind, &args[1], context)?);
    validate_owner(capture, context)?;
    if kind == "mouse" {
        let state = owner.borrow();
        validate_target(&state, capture.document, &arguments[0])?;
    }
    let callback = Rooted::new(capture.callback.clone());
    let arguments = JsArray::from_iter(arguments.iter().cloned(), context);
    callback.as_callable().unwrap().call(
        &JsValue::undefined(),
        &[
            js_string!(kind).into(),
            arguments.into(),
            JsValue::null(),
            JsValue::null(),
        ],
        context,
    )
}

fn read_payload(kind: &str, value: &JsValue, context: &mut Context) -> JsResult<Vec<JsValue>> {
    let array = value
        .as_object()
        .filter(|object| object.is_array())
        .ok_or_else(|| invalid("Automation input data must be an array"))?;
    let array = Rooted::new(JsArray::from_object(array)?);
    let expected = if kind == "keyboard" { 2 } else { 4 };
    if array.length(context)? != expected {
        return Err(invalid("Invalid automation argument count"));
    }
    if kind == "keyboard" {
        let phase = array.get(0u32, context)?;
        let name = phase
            .as_string()
            .ok_or_else(|| invalid("Key phase must be a string"))?
            .to_std_string_escaped();
        if !matches!(name.as_str(), "keydown" | "keyup") {
            return Err(invalid("Unsupported automation key phase"));
        }
        let input = array.get(1u32, context)?;
        let snapshot = snapshot_dictionary(&input, true, context)?;
        return Ok(vec![phase, snapshot.into()]);
    }
    let id = array.get(0u32, context)?;
    if id.as_number().is_none() {
        return Err(invalid("Automation target identity must be numeric"));
    }
    let phase = array.get(1u32, context)?;
    let name = phase
        .as_string()
        .ok_or_else(|| invalid("Mouse phase must be a string"))?
        .to_std_string_escaped();
    if !matches!(
        name.as_str(),
        "mousemove" | "mousedown" | "mouseup" | "click"
    ) {
        return Err(invalid("Unsupported automation mouse phase"));
    }
    let input = array.get(2u32, context)?;
    let snapshot = Rooted::new(snapshot_dictionary(&input, false, context)?);
    let focus = array.get(3u32, context)?;
    if focus.as_boolean() != Some(name == "mousedown") {
        return Err(invalid("Automation focus must match mouse press"));
    }
    Ok(vec![id, phase, (*snapshot).clone().into(), focus])
}

fn snapshot_dictionary(
    input: &JsValue,
    keyboard: bool,
    context: &mut Context,
) -> JsResult<JsObject> {
    let input = Rooted::new(
        input
            .as_object()
            .ok_or_else(|| invalid("Automation input dictionary required"))?,
    );
    let snapshot = Rooted::new(JsObject::with_object_proto(context.intrinsics()));
    let fields: &[&str] = if keyboard {
        &[
            "key", "text", "repeat", "shiftKey", "ctrlKey", "altKey", "metaKey",
        ]
    } else {
        &[
            "clientX",
            "clientY",
            "pageX",
            "pageY",
            "screenX",
            "screenY",
            "movementX",
            "movementY",
            "buttons",
            "button",
            "pointerId",
        ]
    };
    for &field in fields {
        let value = input.get(js_string!(field), context)?;
        if keyboard {
            if if matches!(field, "key" | "text") {
                !value.is_string()
            } else {
                value.as_boolean().is_none()
            } {
                return Err(invalid("Invalid automation keyboard field"));
            }
        } else {
            let number = value
                .as_number()
                .filter(|number| number.is_finite())
                .ok_or_else(|| invalid("Automation mouse fields must be finite numbers"))?;
            let range = match field {
                "button" => Some((0.0, 4.0)),
                "buttons" => Some((0.0, 31.0)),
                "pointerId" => Some((1.0, i32::MAX as f64)),
                _ => None,
            };
            if range
                .is_some_and(|(low, high)| number.fract() != 0.0 || number < low || number > high)
            {
                return Err(invalid("Invalid automation button or pointer identifier"));
            }
        }
        // Values are validated primitives, preserving UTF-16 key/text exactly.
        // Read each accessor once, then pass only data properties to dispatch.
        snapshot.create_data_property_or_throw(js_string!(field), value, context)?;
    }
    Ok((*snapshot).clone())
}

fn validate_target(state: &HostState, document: usize, value: &JsValue) -> JsResult<()> {
    let id = value
        .as_number()
        .filter(|id| {
            id.is_finite() && id.fract() == 0.0 && *id >= 0.0 && *id <= 9_007_199_254_740_991.0
        })
        .and_then(|id| usize::try_from(id as u64).ok())
        .ok_or_else(|| invalid("Invalid automation target identity"))?;
    let target = state
        .get_node(id)
        .ok_or_else(|| invalid("Unknown automation target"))?;
    if !document_root_for_node(&target).is_some_and(|root| root.identity() == document) {
        return Err(invalid(
            "Automation target is detached or in another Document",
        ));
    }
    Ok(())
}
