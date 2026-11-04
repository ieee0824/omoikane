//! Validation and data conversion for owned host input commands.

use super::host_input_types::*;
use super::*;
use serde_json::{Value, json};

pub(super) struct PreparedInput {
    pub kind: HostInputKind,
    pub arguments: Vec<Value>,
    pub related_target: Option<(usize, usize)>,
}

fn invalid(message: &'static str) -> JsError {
    JsNativeError::typ().with_message(message).into()
}

fn finite(values: impl IntoIterator<Item = f64>) -> JsResult<()> {
    if values.into_iter().all(f64::is_finite) {
        Ok(())
    } else {
        Err(invalid("host input numbers must be finite"))
    }
}

fn node_id(state: &HostState, node: &NodeHandle) -> JsResult<usize> {
    let known = state
        .get_node(node.identity())
        .is_some_and(|known| known == *node);
    let active = if node.node_type() == NodeType::Document {
        state.document_is_active(node.identity())
    } else {
        owner_document_for_node(node)
            .is_some_and(|document| state.document_is_active(document.identity()))
    };
    if known && active {
        Ok(node.identity())
    } else {
        Err(invalid(
            "host input node is foreign or its Document is inactive",
        ))
    }
}

fn modifiers(value: &mut Value, keys: HostInputModifiers) {
    value["altKey"] = json!(keys.alt);
    value["ctrlKey"] = json!(keys.control);
    value["metaKey"] = json!(keys.meta);
    value["shiftKey"] = json!(keys.shift);
}

fn mouse(state: &HostState, input: &HostMouseInput) -> JsResult<(Value, Option<usize>)> {
    finite(
        input
            .client
            .into_iter()
            .chain(input.page)
            .chain(input.screen)
            .chain(input.movement),
    )?;
    let related = input
        .related_target
        .as_ref()
        .map(|node| node_id(state, node))
        .transpose()?;
    let mut value = json!({
        "clientX": input.client[0], "clientY": input.client[1],
        "pageX": input.page[0], "pageY": input.page[1],
        "screenX": input.screen[0], "screenY": input.screen[1],
        "movementX": input.movement[0], "movementY": input.movement[1],
        "button": input.button, "buttons": input.buttons, "relatedTarget": null,
    });
    if let Some(pointer) = input.pointer_id {
        value["pointerId"] = json!(pointer);
    }
    modifiers(&mut value, input.modifiers);
    Ok((value, related))
}

fn touch_point(state: &HostState, point: &HostTouchPoint) -> JsResult<Value> {
    finite(
        point
            .client
            .into_iter()
            .chain(point.page)
            .chain(point.screen)
            .chain([
                point.radius[0] as f64,
                point.radius[1] as f64,
                point.rotation_angle as f64,
                point.force as f64,
                point.altitude_angle,
                point.azimuth_angle,
            ]),
    )?;
    let target = point
        .target
        .as_ref()
        .map(|node| node_id(state, node))
        .transpose()?;
    let mut value = json!({
        "target": target, "clientX": point.client[0], "clientY": point.client[1],
        "pageX": point.page[0], "pageY": point.page[1],
        "screenX": point.screen[0], "screenY": point.screen[1],
        "radiusX": point.radius[0], "radiusY": point.radius[1],
        "rotationAngle": point.rotation_angle, "force": point.force,
        "altitudeAngle": point.altitude_angle, "azimuthAngle": point.azimuth_angle,
        "touchType": match point.touch_type { HostTouchType::Direct => "direct", HostTouchType::Stylus => "stylus" },
    });
    if let Some(identifier) = point.identifier {
        value["identifier"] = json!(identifier);
    }
    Ok(value)
}

fn touch(state: &HostState, input: &HostTouchInput) -> JsResult<Value> {
    finite(input.delta)?;
    let touches = input
        .touches
        .iter()
        .map(|p| touch_point(state, p))
        .collect::<JsResult<Vec<_>>>()?;
    let changed = input
        .changed_touches
        .iter()
        .map(|p| touch_point(state, p))
        .collect::<JsResult<Vec<_>>>()?;
    let mut value = json!({"touches": touches, "changedTouches": changed,
        "deltaX": input.delta[0], "deltaY": input.delta[1], "defaultAllowed": !input.suppress_default});
    modifiers(&mut value, input.modifiers);
    Ok(value)
}

fn mouse_phase(phase: HostMousePhase) -> &'static str {
    match phase {
        HostMousePhase::Down => "mousedown",
        HostMousePhase::Up => "mouseup",
        HostMousePhase::Move => "mousemove",
        HostMousePhase::Over => "mouseover",
        HostMousePhase::Out => "mouseout",
        HostMousePhase::Enter => "mouseenter",
        HostMousePhase::Leave => "mouseleave",
        HostMousePhase::Click => "click",
        HostMousePhase::AuxiliaryClick => "auxclick",
        HostMousePhase::DoubleClick => "dblclick",
        HostMousePhase::ContextMenu => "contextmenu",
        HostMousePhase::PointerDown => "pointerdown",
        HostMousePhase::PointerUp => "pointerup",
    }
}

impl PreparedInput {
    fn new(kind: HostInputKind, arguments: Vec<Value>) -> Self {
        Self {
            kind,
            arguments,
            related_target: None,
        }
    }

    fn with_mouse(mut self, index: usize, related: Option<usize>) -> Self {
        self.related_target = related.map(|id| (index, id));
        self
    }
}

pub(super) fn prepare(state: &HostState, command: &HostInputCommand) -> JsResult<PreparedInput> {
    use HostInputCommand as Command;
    use HostInputKind as Kind;
    Ok(match command {
        Command::Keyboard { phase, input } => {
            let phase = match phase {
                HostKeyPhase::Down => "keydown",
                HostKeyPhase::Press => "keypress",
                HostKeyPhase::Up => "keyup",
            };
            let mut value = json!({"key": input.key, "code": input.code, "text": input.text,
                "keyCode": input.key_code, "charCode": input.char_code,
                "repeat": input.repeat, "isComposing": input.is_composing});
            modifiers(&mut value, input.modifiers);
            PreparedInput::new(Kind::Keyboard, vec![json!(phase), value])
        }
        Command::Mouse {
            target,
            phase,
            input,
            focus_target,
        } => {
            let id = node_id(state, target)?;
            let (value, related) = mouse(state, input)?;
            PreparedInput::new(
                Kind::Mouse,
                vec![
                    json!(id),
                    json!(mouse_phase(*phase)),
                    value,
                    json!(focus_target),
                ],
            )
            .with_mouse(2, related)
        }
        Command::Wheel {
            target,
            input,
            delta,
        } => {
            let id = node_id(state, target)?;
            finite(*delta)?;
            let (mut value, related) = mouse(state, input)?;
            value["deltaX"] = json!(delta[0]);
            value["deltaY"] = json!(delta[1]);
            value["deltaZ"] = json!(delta[2]);
            PreparedInput::new(Kind::Wheel, vec![json!(id), value]).with_mouse(1, related)
        }
        Command::Touch {
            target,
            phase,
            input,
        } => {
            let id = node_id(state, target)?;
            let phase = match phase {
                HostTouchPhase::Start => "touchstart",
                HostTouchPhase::Move => "touchmove",
                HostTouchPhase::End => "touchend",
                HostTouchPhase::Cancel => "touchcancel",
            };
            PreparedInput::new(
                Kind::Touch,
                vec![json!(id), json!(phase), touch(state, input)?],
            )
        }
        Command::SetComposition { text, selection } => PreparedInput::new(
            Kind::Composition,
            vec![
                json!("set"),
                json!(text),
                json!(selection[0]),
                json!(selection[1]),
            ],
        ),
        Command::CommitComposition { text } => {
            PreparedInput::new(Kind::Composition, vec![json!("commit"), json!(text)])
        }
        Command::ReleasePointerCapture { pointer_id } => {
            PreparedInput::new(Kind::ReleasePointerCapture, vec![json!(pointer_id)])
        }
        _ => prepare_drag(state, command)?,
    })
}

fn prepare_drag(state: &HostState, command: &HostInputCommand) -> JsResult<PreparedInput> {
    use HostInputCommand as Command;
    let (kind, target, phase, input) = match command {
        Command::PrepareDrag { target, input } => {
            (HostInputKind::PrepareDrag, Some(target), None, input)
        }
        Command::MoveDrag { target, input } => {
            (HostInputKind::Drag, Some(target), Some("move"), input)
        }
        Command::EndDrag { target, input } => {
            (HostInputKind::Drag, Some(target), Some("end"), input)
        }
        Command::CancelDrag { input } => (HostInputKind::Drag, None, Some("cancel"), input),
        _ => return Err(invalid("expected a drag command")),
    };
    let id = target.map(|node| node_id(state, node)).transpose()?;
    let (value, _) = mouse(state, input)?;
    // DragEvent constructs its relatedTarget from the drag state, not input.
    let mut arguments = vec![json!(id)];
    if let Some(phase) = phase {
        arguments.push(json!(phase));
    }
    arguments.push(value);
    Ok(PreparedInput::new(kind, arguments))
}
