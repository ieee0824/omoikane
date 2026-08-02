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

/// Platform-neutral input method event fields used by native frontends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformImeEvent {
    Enabled,
    Preedit {
        text: String,
        selection: Option<(usize, usize)>,
    },
    Commit(String),
    Disabled,
}

/// Stateful input bridge for a single native browser surface.
#[derive(Debug, Default)]
pub struct PlatformInput {
    cursor: (f64, f64),
    modifiers: InputModifiers,
    buttons: u8,
    composition_text: Option<String>,
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

    /// Dispatches a pixel-denominated wheel/touchpad delta at the cursor.
    pub fn wheel(
        &mut self,
        session: &mut CdpSession,
        delta_x: f64,
        delta_y: f64,
    ) -> Result<(), JsonRpcError> {
        session.dispatch(
            "Input.dispatchMouseEvent",
            json!({
                "type": "mouseWheel",
                "x": self.cursor.0,
                "y": self.cursor.1,
                "deltaX": delta_x,
                "deltaY": delta_y,
                "button": "none",
                "buttons": self.buttons,
                "modifiers": self.modifiers.cdp_bits(),
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
        let text = if event.pressed && self.composition_text.is_none() {
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
                "isComposing": self.composition_text.is_some(),
                "autoRepeat": event.repeat,
                "modifiers": self.modifiers.cdp_bits(),
            }),
        )?;
        Ok(())
    }

    /// Dispatches an input method transition to the focused text control.
    ///
    /// Selection offsets are UTF-16 code-unit offsets within the preedit text,
    /// matching CDP and DOM text-control selection indices.
    pub fn ime_event(
        &mut self,
        session: &mut CdpSession,
        event: PlatformImeEvent,
    ) -> Result<(), JsonRpcError> {
        match event {
            PlatformImeEvent::Enabled => {}
            PlatformImeEvent::Preedit { text, selection } => {
                let collapsed = text.encode_utf16().count();
                let (selection_start, selection_end) = selection.unwrap_or((collapsed, collapsed));
                let result = session.dispatch(
                    "Input.imeSetComposition",
                    json!({
                        "text": &text,
                        "selectionStart": selection_start,
                        "selectionEnd": selection_end,
                    }),
                )?;
                self.composition_text =
                    result["handled"].as_bool().unwrap_or(false).then_some(text);
            }
            PlatformImeEvent::Commit(text) => {
                session.dispatch("Input.insertText", json!({ "text": text }))?;
                self.composition_text = None;
            }
            PlatformImeEvent::Disabled => {
                if self.composition_text.take().is_some() {
                    session.dispatch(
                        "Input.imeSetComposition",
                        json!({ "text": "", "selectionStart": 0, "selectionEnd": 0 }),
                    )?;
                    session.dispatch("Input.insertText", json!({ "text": "" }))?;
                }
            }
        }
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

    #[test]
    fn tab_key_navigates_focus_order_and_honors_cancellation() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<button id='ordinary'>ordinary</button>\
             <button id='late' tabindex='2'>late</button>\
             <button id='early' tabindex='1'>early</button>\
             <button id='negative' tabindex='-1'>negative</button>\
             <button id='disabled' disabled>disabled</button>\
             <button id='hidden' style='display:none'>hidden</button>\
             <a id='link' href='#'>link</a>\
             <iframe id='frame' tabindex='-1'></iframe>\
             <script>globalThis.focusLog=[];document.addEventListener('focusin',e=>focusLog.push(e.target.id));\
             ordinary.addEventListener('keydown',e=>{if(e.key==='Tab'&&globalThis.cancelTab)e.preventDefault()})</script>",
        );
        let mut input = PlatformInput::new();
        let tab = |pressed| PlatformKeyEvent {
            pressed,
            key: "Tab".into(),
            code: "Tab".into(),
            text: None,
            repeat: false,
        };

        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(&mut session, "document.activeElement.id"),
            json!("early")
        );
        input.key_event(&mut session, tab(false)).unwrap();
        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(&mut session, "document.activeElement.id"),
            json!("late")
        );
        input.key_event(&mut session, tab(false)).unwrap();
        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(&mut session, "document.activeElement.id"),
            json!("ordinary")
        );
        input.key_event(&mut session, tab(false)).unwrap();

        evaluate(&mut session, "globalThis.cancelTab=true");
        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(&mut session, "document.activeElement.id"),
            json!("ordinary")
        );
        input.key_event(&mut session, tab(false)).unwrap();
        evaluate(&mut session, "globalThis.cancelTab=false");
        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(&mut session, "document.activeElement.id"),
            json!("link")
        );
        input.key_event(&mut session, tab(false)).unwrap();
        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(&mut session, "document.activeElement.id"),
            json!("early")
        );
        input.key_event(&mut session, tab(false)).unwrap();

        input.set_modifiers(InputModifiers {
            shift: true,
            ..InputModifiers::default()
        });
        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(&mut session, "document.activeElement.id"),
            json!("link")
        );
        assert_eq!(
            evaluate(&mut session, "focusLog.join(',')"),
            json!("early,late,ordinary,link,early,link")
        );

        input.set_modifiers(InputModifiers::default());
        evaluate(
            &mut session,
            "globalThis.sub=frame.contentDocument;globalThis.subFirst=sub.createElement('button');\
             globalThis.subSecond=sub.createElement('button');subFirst.id='sub-first';subSecond.id='sub-second';\
             sub.body.appendChild(subFirst);sub.body.appendChild(subSecond);\
             globalThis.subKeyTarget='';subFirst.addEventListener('keydown',e=>subKeyTarget=e.target.id);subFirst.focus()",
        );
        input.key_event(&mut session, tab(true)).unwrap();
        assert_eq!(
            evaluate(
                &mut session,
                "[document.activeElement===frame,sub.activeElement.id,subKeyTarget].join(':')"
            ),
            json!("true:sub-second:sub-first")
        );
    }

    #[test]
    fn escape_key_requests_modal_dialog_cancellation_end_to_end() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<button id='before'>before</button><dialog id='dialog'><button id='inside'>inside</button></dialog>\
             <script>globalThis.dialogLog=[];dialog.addEventListener('cancel',e=>{dialogLog.push('cancel');if(globalThis.blockCancel)e.preventDefault()});\
             dialog.addEventListener('close',()=>dialogLog.push('close'));before.focus();dialog.showModal()</script>",
        );
        let mut input = PlatformInput::new();
        let escape = PlatformKeyEvent {
            pressed: true,
            key: "Escape".into(),
            code: "Escape".into(),
            text: None,
            repeat: false,
        };

        input.key_event(&mut session, escape.clone()).unwrap();
        assert_eq!(
            evaluate(
                &mut session,
                "[dialog.open,document.activeElement.id,dialogLog.join(',')].join(':')"
            ),
            json!("false:before:cancel,close")
        );

        evaluate(
            &mut session,
            "globalThis.blockCancel=true;dialog.showModal()",
        );
        input.key_event(&mut session, escape).unwrap();
        assert_eq!(
            evaluate(
                &mut session,
                "[dialog.open,document.activeElement.id,dialogLog.join(',')].join(':')"
            ),
            json!("true:inside:cancel,close,cancel")
        );
    }

    #[test]
    fn wheel_dispatches_fractional_delta_and_scrolls_nearest_container() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<style>body{margin:0}#scroller{width:100px;height:50px;overflow:auto}#content{height:200px}</style>\
             <div id='scroller'><div id='content'></div></div><script>globalThis.wheelLog='';\
             scroller.addEventListener('wheel',e=>wheelLog=[e.deltaX,e.deltaY,e.deltaMode,e.altKey,e.clientX,e.clientY].join(':'))</script>",
        );
        let mut input = PlatformInput::new();
        input.set_modifiers(InputModifiers {
            alt: true,
            ..InputModifiers::default()
        });
        input.cursor_moved(&mut session, 10.0, 10.0).unwrap();

        input.wheel(&mut session, 1.25, 12.5).unwrap();

        assert_eq!(evaluate(&mut session, "scroller.scrollTop"), json!(12.5));
        assert_eq!(evaluate(&mut session, "scrollY"), json!(0));
        assert_eq!(
            evaluate(&mut session, "wheelLog"),
            json!("1.25:12.5:0:true:10:10")
        );
    }

    #[test]
    fn canceled_wheel_event_prevents_default_window_scroll() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<style>body{margin:0;height:1000px}</style><script>document.addEventListener('wheel',e=>e.preventDefault())</script>",
        );
        let mut input = PlatformInput::new();
        input.cursor_moved(&mut session, 10.0, 10.0).unwrap();

        input.wheel(&mut session, 0.0, 80.0).unwrap();

        assert_eq!(evaluate(&mut session, "scrollY"), json!(0));
    }

    #[test]
    fn wheel_falls_back_to_the_top_level_window() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<style>body{margin:0;height:1000px}</style><div>page</div>",
        );
        let mut input = PlatformInput::new();
        input.cursor_moved(&mut session, 10.0, 10.0).unwrap();

        input.wheel(&mut session, 0.0, 80.0).unwrap();

        assert_eq!(evaluate(&mut session, "scrollY"), json!(80));
    }

    #[test]
    fn ime_preedit_updates_and_commits_the_focused_text_control() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<input id='field' value='A'><script>globalThis.imeLog=[];field.focus();field.setSelectionRange(1,1);\
             for(const type of ['compositionstart','compositionupdate','compositionend','beforeinput','input'])\
             field.addEventListener(type,e=>imeLog.push([type,e.data,e.inputType||'',e.isComposing||false].join(':')))</script>",
        );
        let mut input = PlatformInput::new();

        input
            .ime_event(
                &mut session,
                PlatformImeEvent::Preedit {
                    text: "に".into(),
                    selection: Some((0, 1)),
                },
            )
            .unwrap();
        input
            .key_event(
                &mut session,
                PlatformKeyEvent {
                    pressed: true,
                    key: "x".into(),
                    code: "KeyX".into(),
                    text: Some("x".into()),
                    repeat: false,
                },
            )
            .unwrap();
        input
            .ime_event(
                &mut session,
                PlatformImeEvent::Preedit {
                    text: "日本".into(),
                    selection: Some((1, 2)),
                },
            )
            .unwrap();
        input
            .ime_event(&mut session, PlatformImeEvent::Commit("日本".into()))
            .unwrap();

        assert_eq!(evaluate(&mut session, "field.value"), json!("A日本"));
        assert_eq!(evaluate(&mut session, "field.selectionStart"), json!(3));
        assert_eq!(evaluate(&mut session, "field.selectionEnd"), json!(3));
        assert_eq!(
            evaluate(&mut session, "imeLog.join('|')"),
            json!(
                "compositionstart:::false|compositionupdate:に::false|beforeinput:に:insertCompositionText:true|input:に:insertCompositionText:true|compositionupdate:日本::false|beforeinput:日本:insertCompositionText:true|input:日本:insertCompositionText:true|compositionend:日本::false"
            )
        );
    }

    #[test]
    fn ime_commit_without_preedit_inserts_text_and_disabled_clears_preedit() {
        let mut session = CdpSession::new().unwrap();
        navigate(
            &mut session,
            "<input id='field'><script>globalThis.ends=[];field.focus();field.oncompositionend=e=>ends.push(e.data)</script>",
        );
        let mut input = PlatformInput::new();

        input
            .ime_event(&mut session, PlatformImeEvent::Commit("é".into()))
            .unwrap();
        input
            .ime_event(
                &mut session,
                PlatformImeEvent::Preedit {
                    text: "語".into(),
                    selection: None,
                },
            )
            .unwrap();
        input
            .ime_event(&mut session, PlatformImeEvent::Disabled)
            .unwrap();

        assert_eq!(evaluate(&mut session, "field.value"), json!("é"));
        assert_eq!(
            evaluate(&mut session, "JSON.stringify(ends)"),
            json!(r#"[""]"#)
        );
    }

    #[test]
    fn rejected_ime_preedit_does_not_suppress_later_keyboard_editing() {
        let mut session = CdpSession::new().unwrap();
        navigate(&mut session, "<input id='field'>");
        let mut input = PlatformInput::new();

        input
            .ime_event(
                &mut session,
                PlatformImeEvent::Preedit {
                    text: "ignored".into(),
                    selection: None,
                },
            )
            .unwrap();
        evaluate(&mut session, "field.focus()");
        input
            .key_event(
                &mut session,
                PlatformKeyEvent {
                    pressed: true,
                    key: "x".into(),
                    code: "KeyX".into(),
                    text: Some("x".into()),
                    repeat: false,
                },
            )
            .unwrap();

        assert_eq!(evaluate(&mut session, "field.value"), json!("x"));
    }
}
