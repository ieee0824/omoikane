//! Display-server-independent translation of platform input into browser input.

use serde_json::json;

use crate::cdp::{CdpSession, JsonRpcError};

/// Modifier keys active for a platform input event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputModifiers {
    pub alt: bool,
    pub control: bool,
    pub meta: bool,
    pub shift: bool,
}

impl InputModifiers {
    fn cdp_bits(self) -> u8 {
        u8::from(self.alt)
            | (u8::from(self.control) << 1)
            | (u8::from(self.meta) << 2)
            | (u8::from(self.shift) << 3)
    }
}

/// A mouse button understood by the browser input domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformMouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

impl PlatformMouseButton {
    fn cdp_name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
            Self::Back => "back",
            Self::Forward => "forward",
        }
    }

    fn buttons_bit(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Right => 2,
            Self::Middle => 4,
            Self::Back => 8,
            Self::Forward => 16,
        }
    }
}

/// Platform-neutral keyboard event fields used by native frontends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformKeyEvent {
    pub pressed: bool,
    pub key: String,
    pub code: String,
    pub text: Option<String>,
    pub repeat: bool,
}

/// Stateful input bridge for a single native browser surface.
#[derive(Debug, Default)]
pub struct PlatformInput {
    cursor: (f64, f64),
    modifiers: InputModifiers,
    buttons: u8,
}

impl PlatformInput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_modifiers(&mut self, modifiers: InputModifiers) {
        self.modifiers = modifiers;
    }

    /// Updates the cursor position and dispatches a DOM `mousemove`.
    pub fn cursor_moved(
        &mut self,
        session: &mut CdpSession,
        x: f64,
        y: f64,
    ) -> Result<(), JsonRpcError> {
        self.cursor = (x, y);
        session.dispatch(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseMoved",
                "x": x,
                "y": y,
                "button": "none",
                "buttons": self.buttons,
                "modifiers": self.modifiers.cdp_bits(),
            }),
        )?;
        Ok(())
    }

    /// Dispatches a mouse press or release at the last cursor position.
    pub fn mouse_button(
        &mut self,
        session: &mut CdpSession,
        button: PlatformMouseButton,
        pressed: bool,
    ) -> Result<(), JsonRpcError> {
        if pressed {
            self.buttons |= button.buttons_bit();
        } else {
            self.buttons &= !button.buttons_bit();
        }
        session.dispatch(
            "Input.dispatchMouseEvent",
            json!({
                "type": if pressed { "mousePressed" } else { "mouseReleased" },
                "x": self.cursor.0,
                "y": self.cursor.1,
                "button": button.cdp_name(),
                "buttons": self.buttons,
                "modifiers": self.modifiers.cdp_bits(),
                "clickCount": 1,
            }),
        )?;
        Ok(())
    }

    /// Dispatches a key transition, including text for the editing default action.
    pub fn key_event(
        &mut self,
        session: &mut CdpSession,
        event: PlatformKeyEvent,
    ) -> Result<(), JsonRpcError> {
        let text = if event.pressed {
            event.text.as_deref().unwrap_or("")
        } else {
            ""
        };
        session.dispatch(
            "Input.dispatchKeyEvent",
            json!({
                "type": if event.pressed { "keyDown" } else { "keyUp" },
                "key": event.key,
                "code": event.code,
                "text": text,
                "autoRepeat": event.repeat,
                "modifiers": self.modifiers.cdp_bits(),
            }),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::frame::render_browser_frame;

    fn navigate(session: &mut CdpSession, html: &str) {
        let encoded = html
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": format!("data:text/html,{encoded}") }),
            )
            .unwrap();
        render_browser_frame(session, 320, 200, 0).unwrap();
    }

    fn evaluate(session: &mut CdpSession, expression: &str) -> Value {
        session
            .dispatch("Runtime.evaluate", json!({ "expression": expression }))
            .unwrap()["result"]["value"]
            .clone()
    }

    #[test]
    fn mouse_click_is_hit_tested_and_focuses_a_text_control() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<style>body{margin:0}input{display:block;width:120px;height:30px}</style>\
             <input id='field'><script>field.addEventListener('click',()=>field.dataset.clicked='yes')</script>",
        );
        let mut input = PlatformInput::new();

        input.cursor_moved(&mut session, 10.0, 10.0).unwrap();
        input
            .mouse_button(&mut session, PlatformMouseButton::Left, true)
            .unwrap();
        input
            .mouse_button(&mut session, PlatformMouseButton::Left, false)
            .unwrap();

        assert_eq!(
            evaluate(
                &mut session,
                "document.activeElement.id + ':' + field.dataset.clicked"
            ),
            json!("field:yes")
        );
    }

    #[test]
    fn key_transitions_edit_the_focused_control_and_preserve_modifiers() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<style>body{margin:0}input{width:120px;height:30px}</style><input id='field'>\
             <script>globalThis.keys=[];field.addEventListener('keydown',e=>keys.push([e.type,e.key,e.code,e.shiftKey,e.ctrlKey].join(':')));field.addEventListener('keyup',e=>keys.push([e.type,e.key,e.code,e.shiftKey,e.ctrlKey].join(':')))</script>",
        );
        let mut input = PlatformInput::new();
        input.cursor_moved(&mut session, 10.0, 10.0).unwrap();
        input
            .mouse_button(&mut session, PlatformMouseButton::Left, true)
            .unwrap();
        input
            .mouse_button(&mut session, PlatformMouseButton::Left, false)
            .unwrap();
        input.set_modifiers(InputModifiers {
            shift: true,
            control: true,
            ..InputModifiers::default()
        });

        input
            .key_event(
                &mut session,
                PlatformKeyEvent {
                    pressed: true,
                    key: "A".into(),
                    code: "KeyA".into(),
                    text: Some("A".into()),
                    repeat: false,
                },
            )
            .unwrap();
        input
            .key_event(
                &mut session,
                PlatformKeyEvent {
                    pressed: false,
                    key: "A".into(),
                    code: "KeyA".into(),
                    text: None,
                    repeat: false,
                },
            )
            .unwrap();
        input.set_modifiers(InputModifiers::default());
        for pressed in [true, false] {
            input
                .key_event(
                    &mut session,
                    PlatformKeyEvent {
                        pressed,
                        key: "b".into(),
                        code: "KeyB".into(),
                        text: pressed.then(|| "b".into()),
                        repeat: false,
                    },
                )
                .unwrap();
        }

        assert_eq!(evaluate(&mut session, "field.value"), json!("b"));
        assert_eq!(
            evaluate(&mut session, "keys.join('|')"),
            json!(
                "keydown:A:KeyA:true:true|keyup:A:KeyA:true:true|keydown:b:KeyB:false:false|keyup:b:KeyB:false:false"
            )
        );
    }
}
