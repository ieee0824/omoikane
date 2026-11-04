//! Owned commands for synchronous input from the browser host.

use crate::dom::NodeHandle;

/// Modifier keys sampled by the host for one input operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostInputModifiers {
    /// Alt key is pressed.
    pub alt: bool,
    /// Control key is pressed.
    pub control: bool,
    /// Meta key is pressed.
    pub meta: bool,
    /// Shift key is pressed.
    pub shift: bool,
}

/// Keyboard event phase.
#[derive(Clone, Copy, Debug)]
pub enum HostKeyPhase {
    /// Press a key.
    Down,
    /// Deliver the legacy keypress event.
    Press,
    /// Release a key.
    Up,
}

/// Keyboard data; strings and modifier state are owned by the command.
#[derive(Clone, Debug, Default)]
pub struct HostKeyInput {
    /// Logical key value.
    pub key: String,
    /// Physical key code.
    pub code: String,
    /// Text used by the text-control default action.
    pub text: String,
    /// Legacy key code.
    pub key_code: u32,
    /// Legacy character code.
    pub char_code: u32,
    /// Whether the key repeats.
    pub repeat: bool,
    /// Whether composition is active.
    pub is_composing: bool,
    /// Sampled modifiers.
    pub modifiers: HostInputModifiers,
}

/// Mouse operation accepted by the browser input dispatcher.
#[derive(Clone, Copy, Debug)]
pub enum HostMousePhase {
    /// Mouse button press.
    Down,
    /// Mouse button release.
    Up,
    /// Mouse movement.
    Move,
    /// Pointer enters a target, with bubbling.
    Over,
    /// Pointer leaves a target, with bubbling.
    Out,
    /// Pointer enters a target.
    Enter,
    /// Pointer leaves a target.
    Leave,
    /// Primary activation click.
    Click,
    /// Auxiliary activation click.
    AuxiliaryClick,
    /// Double click.
    DoubleClick,
    /// Context menu request.
    ContextMenu,
    /// Pointer press delivered through the existing mouse dispatcher.
    PointerDown,
    /// Pointer release delivered through the existing mouse dispatcher.
    PointerUp,
}

/// Mouse data shared by mouse, wheel and drag commands.
///
/// Coordinates and movement must be finite. `pointer_id` selects capture;
/// it does not change the dispatched event's interface to PointerEvent.
#[derive(Clone, Default)]
pub struct HostMouseInput {
    /// Client coordinates in CSS pixels.
    pub client: [f64; 2],
    /// Page coordinates in CSS pixels.
    pub page: [f64; 2],
    /// Screen coordinates in CSS pixels.
    pub screen: [f64; 2],
    /// Relative pointer movement in CSS pixels.
    pub movement: [f64; 2],
    /// Changed button, including -1 when no button changed.
    pub button: i16,
    /// Pressed-button bit mask.
    pub buttons: u16,
    /// Capture selector; None uses the dispatcher's default pointer ID.
    pub pointer_id: Option<i32>,
    /// Related node, retained until dispatch completes.
    pub related_target: Option<NodeHandle>,
    /// Sampled modifiers.
    pub modifiers: HostInputModifiers,
}

/// Touch input phase.
#[derive(Clone, Copy, Debug)]
pub enum HostTouchPhase {
    /// A contact begins.
    Start,
    /// A contact moves.
    Move,
    /// A contact ends.
    End,
    /// Contacts are cancelled.
    Cancel,
}

/// Source of a touch contact.
#[derive(Clone, Copy, Debug, Default)]
pub enum HostTouchType {
    /// Direct contact.
    #[default]
    Direct,
    /// Stylus contact.
    Stylus,
}

/// Owned touch contact data. All floating-point values must be finite.
#[derive(Clone, Default)]
pub struct HostTouchPoint {
    /// Contact identifier; None uses its index within the supplied list.
    pub identifier: Option<i32>,
    /// Contact target; None uses the command's target.
    pub target: Option<NodeHandle>,
    /// Client coordinates in CSS pixels.
    pub client: [f64; 2],
    /// Page coordinates in CSS pixels.
    pub page: [f64; 2],
    /// Screen coordinates in CSS pixels.
    pub screen: [f64; 2],
    /// Contact radii in CSS pixels.
    pub radius: [f32; 2],
    /// Contact rotation angle.
    pub rotation_angle: f32,
    /// Contact force.
    pub force: f32,
    /// Stylus altitude angle.
    pub altitude_angle: f64,
    /// Stylus azimuth angle.
    pub azimuth_angle: f64,
    /// Direct or stylus contact.
    pub touch_type: HostTouchType,
}

/// Touch lists and scroll-default input for one touch operation.
#[derive(Clone, Default)]
pub struct HostTouchInput {
    /// Currently active contacts.
    pub touches: Vec<HostTouchPoint>,
    /// Contacts changed by this operation.
    pub changed_touches: Vec<HostTouchPoint>,
    /// Scroll-default delta in CSS pixels; both components must be finite.
    pub delta: [f64; 2],
    /// Suppress the scroll default even when the event is not cancelled.
    pub suppress_default: bool,
    /// Sampled modifiers.
    pub modifiers: HostInputModifiers,
}

/// One synchronous input primitive, with owned data and node handles.
///
/// Entry Document selection is separate from the target, preserving child
/// forwarding. The host owns job draining, navigation and Storage Access scope.
#[derive(Clone)]
pub enum HostInputCommand {
    /// Deliver a keyboard event and its synchronous default action.
    Keyboard {
        /// Event phase.
        phase: HostKeyPhase,
        /// Keyboard data.
        input: HostKeyInput,
    },
    /// Deliver a mouse event and its synchronous default action.
    Mouse {
        /// Hit-tested target.
        target: NodeHandle,
        /// Event phase.
        phase: HostMousePhase,
        /// Mouse data.
        input: HostMouseInput,
        /// Focus the target if the uncancelled operation permits it.
        focus_target: bool,
    },
    /// Deliver a wheel event with pixel deltas (deltaMode is always zero).
    Wheel {
        /// Hit-tested target.
        target: NodeHandle,
        /// Mouse data.
        input: HostMouseInput,
        /// Finite x, y and z deltas in CSS pixels.
        delta: [f64; 3],
    },
    /// Deliver a touch event and its synchronous default action.
    Touch {
        /// Hit-tested target.
        target: NodeHandle,
        /// Event phase.
        phase: HostTouchPhase,
        /// Touch data.
        input: HostTouchInput,
    },
    /// Update composition text and the selection within it.
    SetComposition {
        /// Replacement text.
        text: String,
        /// Selection offsets in UTF-16 code units, clamped by the control.
        selection: [u32; 2],
    },
    /// Commit text, completing any active composition.
    CommitComposition {
        /// Text to commit.
        text: String,
    },
    /// Select a candidate draggable ancestor.
    PrepareDrag {
        /// Hit-tested target.
        target: NodeHandle,
        /// Mouse data.
        input: HostMouseInput,
    },
    /// Start or move the current drag.
    MoveDrag {
        /// Hit-tested target.
        target: NodeHandle,
        /// Mouse data.
        input: HostMouseInput,
    },
    /// Finish the current drag.
    EndDrag {
        /// Hit-tested target.
        target: NodeHandle,
        /// Mouse data.
        input: HostMouseInput,
    },
    /// Cancel the current drag without requiring a target.
    CancelDrag {
        /// Mouse data for the dragend event, if any.
        input: HostMouseInput,
    },
    /// Release pointer capture; distinct from delivering lostpointercapture.
    ReleasePointerCapture {
        /// Pointer whose capture should be released.
        pointer_id: i32,
    },
}
