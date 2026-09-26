use std::collections::HashSet;
use std::error::Error;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use omoikane::cdp::CdpSession;
use omoikane::dom::NodeHandle;
use omoikane::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent,
    ReporterConfig, RetentionPolicy, install_panic_reporter,
};
use omoikane::frame::{PlatformFrameScheduler, render_browser_frame};
use omoikane::js::{FindInPageResult, FullscreenTransition, PointerLockTransition};
use omoikane::platform_input::{
    InputModifiers, PlatformImeEvent, PlatformInput, PlatformKeyEvent, PlatformMouseButton,
    PlatformTouchPhase,
};
use serde_json::json;
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, Ime, MouseButton, MouseScrollDelta, TouchPhase,
    WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Fullscreen as WindowFullscreen, Window, WindowId};

#[path = "omoikane/pointer_lock_host.rs"]
mod pointer_lock_host;

#[cfg(test)]
#[path = "omoikane/keyboard_tests.rs"]
mod keyboard_tests;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const DEFAULT_WINDOW_TITLE: &str = "Omoikane";

#[derive(Clone, Copy)]
enum GuiFailure {
    Window,
    DisplayContext,
    SurfaceCreate,
    SurfaceOperation,
    Input,
    Frame,
}

impl GuiFailure {
    fn event(self) -> omoikane::error_reporting::SafeEvent {
        let (code, severity, operation) = match self {
            Self::Window => (
                "GUI_WINDOW_CREATE_FAILED",
                ErrorSeverity::Critical,
                "connect",
            ),
            Self::DisplayContext => (
                "GUI_DISPLAY_CONTEXT_FAILED",
                ErrorSeverity::Critical,
                "connect",
            ),
            Self::SurfaceCreate => (
                "GUI_SURFACE_CREATE_FAILED",
                ErrorSeverity::Critical,
                "connect",
            ),
            Self::SurfaceOperation => ("GUI_SURFACE_IO_FAILED", ErrorSeverity::Error, "render"),
            Self::Input => ("GUI_INPUT_FAILED", ErrorSeverity::Error, "execute"),
            Self::Frame => ("GUI_FRAME_FAILED", ErrorSeverity::Error, "render"),
        };
        RawEvent::new(
            ErrorCategory::Gui,
            severity,
            ErrorCode::new(code).expect("static GUI error code"),
            ExecutionSurface::Gui,
            "GUI operation failed",
            &[("operation", operation)],
        )
        .sanitize()
    }
}

fn report_gui_failure(
    reporter: Option<&ErrorReporter>,
    failure: GuiFailure,
    _details: &dyn std::fmt::Display,
) {
    if let Some(reporter) = reporter {
        reporter.report(failure.event());
    }
}

fn configured_error_reporter() -> Result<Option<Arc<ErrorReporter>>, Box<dyn Error>> {
    let config = ReporterConfig::from_env()?;
    if !config.records_locally() {
        return Ok(None);
    }
    let path = if let Some(path) = std::env::var_os("OMOIKANE_ERROR_REPORT_DB") {
        PathBuf::from(path)
    } else {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME is required for error-report storage",
            )
        })?;
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let home = PathBuf::from(home);
                if cfg!(target_os = "macos") {
                    home.join("Library/Application Support")
                } else {
                    home.join(".local/state")
                }
            });
        state.join("omoikane/error-reports.sqlite")
    };
    Ok(Some(Arc::new(ErrorReporter::new(
        &config,
        path,
        RetentionPolicy::default(),
    )?)))
}

struct FindUi {
    query: String,
    result: FindInPageResult,
    document: NodeHandle,
}

struct BrowserApp {
    session: CdpSession,
    error_reporter: Option<Arc<ErrorReporter>>,
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    frame_scheduler: PlatformFrameScheduler,
    input: PlatformInput,
    window_title: String,
    modifiers: InputModifiers,
    find_ui: Option<FindUi>,
    find_key_releases: HashSet<String>,
    window_occluded: bool,
    window_minimized: bool,
    native_fullscreen: bool,
    native_pointer_lock: bool,
    native_pointer_lock_raw_buttons: bool,
    pointer_restore: (f64, f64),
    trace_input: bool,
    input_trace_sequence: u64,
}

impl BrowserApp {
    fn new(url: &str) -> Result<Self, Box<dyn Error>> {
        let mut session = CdpSession::new().map_err(std::io::Error::other)?;
        session.set_pointer_lock_deferred(true);
        session.dispatch("Page.navigate", json!({ "url": url }))?;
        let started_at = Instant::now();
        Ok(Self {
            session,
            error_reporter: None,
            window: None,
            context: None,
            surface: None,
            frame_scheduler: PlatformFrameScheduler::new(started_at, FRAME_INTERVAL),
            input: PlatformInput::new(),
            window_title: DEFAULT_WINDOW_TITLE.to_string(),
            modifiers: InputModifiers::default(),
            find_ui: None,
            find_key_releases: HashSet::new(),
            window_occluded: false,
            window_minimized: false,
            native_fullscreen: false,
            native_pointer_lock: false,
            native_pointer_lock_raw_buttons: false,
            pointer_restore: (0.0, 0.0),
            trace_input: std::env::var_os("OMOIKANE_TRACE_INPUT").is_some(),
            input_trace_sequence: 0,
        })
    }

    fn set_error_reporter(&mut self, reporter: Arc<ErrorReporter>) {
        self.session
            .set_error_reporter(Arc::clone(&reporter), ExecutionSurface::Gui);
        self.error_reporter = Some(reporter);
    }

    fn sync_fullscreen(&mut self) {
        let Some(window) = &self.window else { return };
        if let Some(transition) = self.session.take_fullscreen_transition() {
            match transition {
                FullscreenTransition::Enter => {
                    window.set_fullscreen(Some(WindowFullscreen::Borderless(None)));
                    self.native_fullscreen = true;
                }
                FullscreenTransition::Exit => {
                    window.set_fullscreen(None);
                    self.native_fullscreen = false;
                }
            }
        }
    }

    fn unlock_native_pointer(&mut self) {
        if let Some(window) = &self.window {
            if let Err(error) = pointer_lock_host::set_grab(window, false) {
                eprintln!("cursor release failed: {error}");
            }
            window.set_cursor_visible(true);
            if self.native_pointer_lock {
                let _ = window.set_cursor_position(LogicalPosition::new(
                    self.pointer_restore.0,
                    self.pointer_restore.1,
                ));
            }
        }
        self.native_pointer_lock = false;
        self.native_pointer_lock_raw_buttons = false;
        self.session.reset_pointer_movement();
    }

    fn sync_pointer_lock(&mut self) {
        if self.window.is_none() {
            return;
        }
        // Page callbacks run on the event loop, not inside this native handshake.
        while let Some(transition) = self.session.take_pointer_lock_transition() {
            match transition {
                PointerLockTransition::Release => self.unlock_native_pointer(),
                PointerLockTransition::Acquire {
                    request_id,
                    unadjusted_movement,
                } => {
                    let window = self.window.as_ref().unwrap();
                    // Unaccelerated delivery is optional. Do not promise it on
                    // platforms whose device-event capabilities are unverified.
                    let accepted =
                        !unadjusted_movement && pointer_lock_host::set_grab(window, true).is_ok();
                    if accepted {
                        if !self.native_pointer_lock {
                            self.pointer_restore = self.input.cursor_position();
                        }
                        window.set_cursor_visible(false);
                        self.native_pointer_lock = true;
                        self.native_pointer_lock_raw_buttons =
                            pointer_lock_host::uses_raw_buttons(window);
                    }
                    match self
                        .session
                        .complete_pointer_lock_request(request_id, accepted)
                    {
                        Ok(true) => {}
                        Ok(false) if accepted => self.unlock_native_pointer(),
                        Err(error) => {
                            eprintln!("pointer lock completion failed: {error}");
                            let _ = self.session.release_pointer_lock_from_host();
                            self.unlock_native_pointer();
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn draw(&mut self, elapsed_ms: u64) -> Result<(), Box<dyn Error>> {
        let Some(window) = self.window.clone() else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        let frame = render_browser_frame(&mut self.session, size.width, size.height, elapsed_ms)
            .map_err(|error| {
                report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Frame, &error);
                error
            })?;
        self.sync_find_document();
        let title = find_window_title(
            document_window_title(&mut self.session).map_err(|error| {
                report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Frame, &error);
                error
            })?,
            self.find_ui.as_ref(),
        );
        if let Some(title) = changed_window_title(&mut self.window_title, title) {
            window.set_title(&title);
        }

        let Some(surface) = &mut self.surface else {
            return Ok(());
        };
        surface.resize(width, height).map_err(|error| {
            report_gui_failure(
                self.error_reporter.as_deref(),
                GuiFailure::SurfaceOperation,
                &error,
            );
            error
        })?;
        let mut target = surface.buffer_mut().map_err(|error| {
            report_gui_failure(
                self.error_reporter.as_deref(),
                GuiFailure::SurfaceOperation,
                &error,
            );
            error
        })?;
        for (destination, source) in target.iter_mut().zip(frame.pixels().chunks_exact(4)) {
            *destination =
                u32::from(source[0]) << 16 | u32::from(source[1]) << 8 | u32::from(source[2]);
        }
        target.present().map_err(|error| {
            report_gui_failure(
                self.error_reporter.as_deref(),
                GuiFailure::SurfaceOperation,
                &error,
            );
            error
        })?;
        Ok(())
    }

    fn trace_input_event(&mut self, event: &WindowEvent) {
        if !self.trace_input {
            return;
        }
        let mut record = match event {
            WindowEvent::Focused(focused) => json!({ "kind": "focus", "focused": focused }),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => json!({
                "kind": "key", "synthetic": is_synthetic,
                "pressed": event.state == ElementState::Pressed,
                "key": logical_key_name(&event.logical_key),
                "code": physical_key_code(event.physical_key), "repeat": event.repeat,
            }),
            _ => return,
        };
        self.input_trace_sequence += 1;
        record["sequence"] = json!(self.input_trace_sequence);
        eprintln!("OMOIKANE_INPUT {record}");
    }

    fn dispatch_key_input(
        &mut self,
        event: PlatformKeyEvent,
        is_synthetic: bool,
    ) -> Result<(), omoikane::cdp::JsonRpcError> {
        // winit replays held keys on focus changes. They are state snapshots,
        // not new user input: dispatching them would edit text, trigger page
        // shortcuts and grant activation again. Focused(false) independently
        // releases Pointer Lock and clears stale button/modifier state.
        if is_synthetic {
            return Ok(());
        }
        self.input.key_event(&mut self.session, event)
    }

    fn find_action(&mut self, action: &str) -> Result<(), omoikane::cdp::JsonRpcError> {
        let query = self.find_ui.as_ref().map_or("", |ui| ui.query.as_str());
        let result = self.session.dispatch(
            "Omoikane.findInPage",
            json!({ "action": action, "query": query }),
        )?;
        if action == "stop" {
            self.find_ui = None;
        } else if self.find_ui.is_some() {
            let result: FindInPageResult =
                serde_json::from_value(result).map_err(|error| omoikane::cdp::JsonRpcError {
                    code: -32000,
                    message: error.to_string(),
                })?;
            if action == "status"
                && self
                    .find_ui
                    .as_ref()
                    .is_some_and(|ui| ui.query != result.query)
            {
                self.find_ui = None;
            } else if let Some(ui) = &mut self.find_ui {
                ui.result = result;
            }
        }
        Ok(())
    }

    fn sync_find_document(&mut self) {
        if self
            .find_ui
            .as_ref()
            .is_some_and(|ui| ui.document != self.session.document())
        {
            self.find_ui = None;
        }
    }

    fn handle_find_key(&mut self, key: &str, text: Option<&str>, pressed: bool) -> bool {
        if !pressed && self.find_key_releases.remove(key) {
            return true;
        }
        if (self.modifiers.control || self.modifiers.meta)
            && !self.modifiers.alt
            && key.eq_ignore_ascii_case("f")
        {
            if pressed {
                self.find_key_releases.insert(key.to_string());
            }
            if pressed && self.find_ui.is_none() {
                self.find_ui = Some(FindUi {
                    query: String::new(),
                    result: FindInPageResult {
                        query: String::new(),
                        match_count: 0,
                        active_match_ordinal: 0,
                    },
                    document: self.session.document(),
                });
            }
            return true;
        }
        if self.find_ui.is_none() {
            return false;
        }
        if !pressed {
            return true;
        }
        self.find_key_releases.insert(key.to_string());
        let action = match key {
            "Escape" => Some("stop"),
            "Enter" => Some(if self.modifiers.shift {
                "previous"
            } else {
                "next"
            }),
            "Backspace" => {
                self.find_ui.as_mut().unwrap().query.pop();
                Some("start")
            }
            _ if !self.modifiers.control && !self.modifiers.meta && !self.modifiers.alt => {
                if let Some(text) = text.filter(|value| !value.chars().any(char::is_control)) {
                    self.find_ui.as_mut().unwrap().query.push_str(text);
                    Some("start")
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(action) = action {
            if let Err(error) = self.find_action(action) {
                eprintln!("find-in-page failed: {error}");
            }
        }
        true
    }

    fn dispatch_input(&mut self, event: WindowEvent) -> bool {
        self.trace_input_event(&event);
        let scale_factor = self
            .window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor());
        let result = match event {
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = physical_position_css_pixels(position, scale_factor);
                self.input.cursor_moved(&mut self.session, x, y)
            }
            WindowEvent::CursorLeft { .. } => {
                self.input.cursor_left(&mut self.session);
                Ok(())
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.find_key_releases.clear();
                }
                self.input.focus_changed(&mut self.session, focused)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.native_pointer_lock_raw_buttons {
                    return false;
                }
                let Some(button) = platform_mouse_button(button) else {
                    return false;
                };
                self.input
                    .mouse_button(&mut self.session, button, state == ElementState::Pressed)
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.native_pointer_lock_raw_buttons {
                    return false;
                }
                let (delta_x, delta_y) = wheel_delta_css_pixels(delta, scale_factor);
                self.input.wheel(&mut self.session, delta_x, delta_y)
            }
            WindowEvent::Touch(touch) => {
                let (x, y) = physical_position_css_pixels(touch.location, scale_factor);
                let phase = match touch.phase {
                    TouchPhase::Started => PlatformTouchPhase::Started,
                    TouchPhase::Moved => PlatformTouchPhase::Moved,
                    TouchPhase::Ended => PlatformTouchPhase::Ended,
                    TouchPhase::Cancelled => PlatformTouchPhase::Cancelled,
                };
                self.input.touch(&mut self.session, touch.id, phase, x, y)
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.modifiers = InputModifiers {
                    alt: state.alt_key(),
                    control: state.control_key(),
                    meta: state.super_key(),
                    shift: state.shift_key(),
                };
                self.input.set_modifiers(self.modifiers);
                return true;
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                if !is_synthetic
                    && self.handle_find_key(
                        &logical_key_name(&event.logical_key),
                        event.text.as_deref(),
                        event.state == ElementState::Pressed,
                    )
                {
                    return true;
                }
                let text = event
                    .text
                    .as_deref()
                    .filter(|text| text.chars().all(|character| !character.is_control()))
                    .map(ToOwned::to_owned);
                self.dispatch_key_input(
                    PlatformKeyEvent {
                        pressed: event.state == ElementState::Pressed,
                        key: logical_key_name(&event.logical_key),
                        code: physical_key_code(event.physical_key),
                        text,
                        repeat: event.repeat,
                    },
                    is_synthetic,
                )
            }
            WindowEvent::Ime(Ime::Commit(text)) if self.find_ui.is_some() => {
                self.find_ui.as_mut().unwrap().query.push_str(&text);
                self.find_action("start")
            }
            WindowEvent::Ime(event) if self.find_ui.is_some() => {
                let _ = event;
                Ok(())
            }
            WindowEvent::Ime(event) => self
                .input
                .ime_event(&mut self.session, platform_ime_event(event)),
            _ => return false,
        };
        if let Err(error) = result {
            report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Input, &error);
            eprintln!("input event failed: {error}");
        }
        self.sync_fullscreen();
        self.sync_pointer_lock();
        true
    }
}

fn platform_ime_event(event: Ime) -> PlatformImeEvent {
    match event {
        Ime::Enabled => PlatformImeEvent::Enabled,
        Ime::Preedit(text, selection) => PlatformImeEvent::Preedit {
            selection: selection
                .map(|(start, end)| (utf16_offset(&text, start), utf16_offset(&text, end))),
            text,
        },
        Ime::Commit(text) => PlatformImeEvent::Commit(text),
        Ime::Disabled => PlatformImeEvent::Disabled,
    }
}

fn utf16_offset(text: &str, byte_offset: usize) -> usize {
    let mut boundary = byte_offset.min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text[..boundary].encode_utf16().count()
}

fn platform_mouse_button(button: MouseButton) -> Option<PlatformMouseButton> {
    match button {
        MouseButton::Left => Some(PlatformMouseButton::Left),
        MouseButton::Middle => Some(PlatformMouseButton::Middle),
        MouseButton::Right => Some(PlatformMouseButton::Right),
        MouseButton::Back => Some(PlatformMouseButton::Back),
        MouseButton::Forward => Some(PlatformMouseButton::Forward),
        MouseButton::Other(_) => None,
    }
}

fn document_window_title(session: &mut CdpSession) -> Result<String, omoikane::cdp::JsonRpcError> {
    let result = session.dispatch(
        "Runtime.evaluate",
        json!({ "expression": "document.title", "returnByValue": true }),
    )?;
    Ok(result["result"]["value"]
        .as_str()
        .filter(|title| !title.is_empty())
        .unwrap_or(DEFAULT_WINDOW_TITLE)
        .to_string())
}

fn changed_window_title(current: &mut String, next: String) -> Option<String> {
    if *current == next {
        return None;
    }
    *current = next.clone();
    Some(next)
}

fn find_window_title(page_title: String, find_ui: Option<&FindUi>) -> String {
    match find_ui {
        Some(ui) => format!(
            "Find: {} ({}/{}) — {}",
            ui.query, ui.result.active_match_ordinal, ui.result.match_count, page_title
        ),
        None => page_title,
    }
}

fn logical_key_name(key: &Key) -> String {
    match key.as_ref() {
        Key::Character(character) => character.to_string(),
        Key::Named(NamedKey::Space) => " ".to_string(),
        Key::Named(NamedKey::Super) => "Meta".to_string(),
        Key::Named(named) => format!("{named:?}"),
        Key::Dead(_) => "Dead".to_string(),
        Key::Unidentified(_) => "Unidentified".to_string(),
    }
}

fn physical_key_code(key: PhysicalKey) -> String {
    match key {
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperLeft) => "MetaLeft".to_string(),
        PhysicalKey::Code(winit::keyboard::KeyCode::SuperRight) => "MetaRight".to_string(),
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => "Unidentified".to_string(),
    }
}

fn physical_position_css_pixels(position: PhysicalPosition<f64>, scale_factor: f64) -> (f64, f64) {
    let logical = position.to_logical::<f64>(scale_factor);
    (logical.x, logical.y)
}

fn wheel_delta_css_pixels(delta: MouseScrollDelta, scale_factor: f64) -> (f64, f64) {
    const CSS_PIXELS_PER_LINE: f64 = 40.0;
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (
            f64::from(x) * CSS_PIXELS_PER_LINE,
            f64::from(y) * CSS_PIXELS_PER_LINE,
        ),
        MouseScrollDelta::PixelDelta(position) => {
            physical_position_css_pixels(position, scale_factor)
        }
    }
}

impl ApplicationHandler for BrowserApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Omoikane")
            .with_inner_size(LogicalSize::new(1280, 720));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Window, &error);
                eprintln!("failed to create window: {error}");
                event_loop.exit();
                return;
            }
        };
        window.set_ime_allowed(true);
        let context = match Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                report_gui_failure(
                    self.error_reporter.as_deref(),
                    GuiFailure::DisplayContext,
                    &error,
                );
                eprintln!("failed to create display context: {error}");
                event_loop.exit();
                return;
            }
        };
        let surface = match Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                report_gui_failure(
                    self.error_reporter.as_deref(),
                    GuiFailure::SurfaceCreate,
                    &error,
                );
                eprintln!("failed to create window surface: {error}");
                event_loop.exit();
                return;
            }
        };
        self.frame_scheduler
            .request_rendering_opportunity(Instant::now());
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.session.set_host_visibility(true);
                let _ = self.session.release_pointer_lock_from_host();
                self.unlock_native_pointer();
                event_loop.exit();
            }
            WindowEvent::Occluded(occluded) => {
                self.window_occluded = occluded;
                self.session
                    .set_host_visibility(self.window_occluded || self.window_minimized);
            }
            WindowEvent::Resized(size) => {
                self.window_minimized = size.width == 0 || size.height == 0;
                self.session
                    .set_host_visibility(self.window_occluded || self.window_minimized);
                if self.native_fullscreen
                    && self
                        .window
                        .as_ref()
                        .is_some_and(|window| window.fullscreen().is_none())
                {
                    let _ = self.session.fullscreen_exited_by_host();
                    self.native_fullscreen = false;
                    self.sync_fullscreen();
                }
                self.frame_scheduler
                    .request_rendering_opportunity(Instant::now());
            }
            WindowEvent::RedrawRequested => {
                let elapsed_ms = self.frame_scheduler.begin_frame(Instant::now());
                if let Err(error) = self.draw(elapsed_ms) {
                    eprintln!("frame failed: {error}");
                }
            }
            event => {
                if self.dispatch_input(event) {
                    self.frame_scheduler
                        .request_rendering_opportunity(Instant::now());
                }
            }
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if !self.native_pointer_lock {
            return;
        }
        let result = match event {
            DeviceEvent::MouseMotion { delta } => {
                self.input
                    .relative_motion(&mut self.session, delta.0, delta.1)
            }
            DeviceEvent::Button { button, state } if self.native_pointer_lock_raw_buttons => {
                let pressed = state == ElementState::Pressed;
                let button = match button {
                    1 => PlatformMouseButton::Left,
                    2 => PlatformMouseButton::Middle,
                    3 => PlatformMouseButton::Right,
                    8 => PlatformMouseButton::Back,
                    9 => PlatformMouseButton::Forward,
                    4..=7 if pressed => {
                        // X11 wheel buttons encode one 40-CSS-pixel line step.
                        let (dx, dy) = match button {
                            4 => (0.0, -40.0),
                            5 => (0.0, 40.0),
                            6 => (-40.0, 0.0),
                            _ => (40.0, 0.0),
                        };
                        if let Err(error) = self.input.wheel(&mut self.session, dx, dy) {
                            report_gui_failure(
                                self.error_reporter.as_deref(),
                                GuiFailure::Input,
                                &error,
                            );
                            eprintln!("relative wheel input failed: {error}");
                        }
                        self.sync_pointer_lock();
                        self.frame_scheduler
                            .request_rendering_opportunity(Instant::now());
                        return;
                    }
                    _ => return,
                };
                self.input.mouse_button(&mut self.session, button, pressed)
            }
            DeviceEvent::MouseWheel { delta } if self.native_pointer_lock_raw_buttons => {
                let scale = self
                    .window
                    .as_ref()
                    .map_or(1.0, |window| window.scale_factor());
                let (dx, dy) = wheel_delta_css_pixels(delta, scale);
                self.input.wheel(&mut self.session, dx, dy)
            }
            _ => return,
        };
        if let Err(error) = result {
            report_gui_failure(self.error_reporter.as_deref(), GuiFailure::Input, &error);
            eprintln!("relative input failed: {error}");
        }
        self.sync_pointer_lock();
        self.frame_scheduler
            .request_rendering_opportunity(Instant::now());
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        let _ = self.input.focus_changed(&mut self.session, false);
        self.unlock_native_pointer();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.sync_fullscreen();
        self.sync_pointer_lock();
        if let Some(window) = &self.window {
            let now = Instant::now();
            if self.frame_scheduler.queue_redraw_if_due(now) {
                window.request_redraw();
            }
            if self.frame_scheduler.redraw_pending() {
                event_loop.set_control_flow(ControlFlow::Wait);
            } else {
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(self.frame_scheduler.deadline()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::keyboard::{KeyCode, NativeKeyCode};

    use super::*;

    #[test]
    fn translates_winit_keys_to_dom_key_and_code_names() {
        assert_eq!(logical_key_name(&Key::Character("é".into())), "é");
        assert_eq!(
            logical_key_name(&Key::Named(NamedKey::ArrowLeft)),
            "ArrowLeft"
        );
        assert_eq!(logical_key_name(&Key::Named(NamedKey::Space)), " ");
        assert_eq!(logical_key_name(&Key::Named(NamedKey::Super)), "Meta");
        assert_eq!(physical_key_code(PhysicalKey::Code(KeyCode::KeyA)), "KeyA");
        assert_eq!(
            physical_key_code(PhysicalKey::Code(KeyCode::SuperLeft)),
            "MetaLeft"
        );
        assert_eq!(
            physical_key_code(PhysicalKey::Unidentified(NativeKeyCode::Xkb(0))),
            "Unidentified"
        );
    }

    #[test]
    fn translates_supported_mouse_buttons_and_ignores_other_buttons() {
        assert_eq!(
            platform_mouse_button(MouseButton::Left),
            Some(PlatformMouseButton::Left)
        );
        assert_eq!(platform_mouse_button(MouseButton::Other(8)), None);
    }

    #[test]
    fn translates_cursor_and_wheel_positions_to_css_pixels() {
        assert_eq!(
            physical_position_css_pixels(PhysicalPosition::new(12.5, -7.0), 1.0),
            (12.5, -7.0)
        );
        assert_eq!(
            physical_position_css_pixels(PhysicalPosition::new(25.0, -14.0), 2.0),
            (12.5, -7.0)
        );
        assert_eq!(
            wheel_delta_css_pixels(MouseScrollDelta::LineDelta(0.5, -2.0), 2.0),
            (20.0, -80.0)
        );
        assert_eq!(
            wheel_delta_css_pixels(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(2.5, -7.0)),
                2.0
            ),
            (1.25, -3.5)
        );
    }

    #[test]
    fn translates_ime_preedit_byte_ranges_to_dom_utf16_offsets() {
        assert_eq!(utf16_offset("a😀い", 0), 0);
        assert_eq!(utf16_offset("a😀い", 1), 1);
        assert_eq!(utf16_offset("a😀い", 5), 3);
        assert_eq!(utf16_offset("a😀い", usize::MAX), 4);
        assert_eq!(
            platform_ime_event(Ime::Preedit("a😀い".into(), Some((1, 5)))),
            PlatformImeEvent::Preedit {
                text: "a😀い".into(),
                selection: Some((1, 3)),
            }
        );
    }

    #[test]
    fn synchronizes_initial_script_and_timer_document_titles_without_redundant_updates() {
        let mut session = CdpSession::new().unwrap();
        session
            .dispatch(
                "Page.navigate",
                json!({ "url": "data:text/html,<title>Initial</title><body></body>" }),
            )
            .unwrap();
        let mut current = DEFAULT_WINDOW_TITLE.to_string();

        assert_eq!(
            changed_window_title(&mut current, document_window_title(&mut session).unwrap()),
            Some("Initial".to_string())
        );
        assert_eq!(
            changed_window_title(&mut current, document_window_title(&mut session).unwrap()),
            None
        );

        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "document.title='Script';setTimeout(()=>document.title='Timer',0)" }),
            )
            .unwrap();
        assert_eq!(document_window_title(&mut session).unwrap(), "Script");
        render_browser_frame(&mut session, 100, 100, 1).unwrap();
        assert_eq!(document_window_title(&mut session).unwrap(), "Timer");

        session
            .dispatch(
                "Runtime.evaluate",
                json!({ "expression": "document.title=''" }),
            )
            .unwrap();
        assert_eq!(
            document_window_title(&mut session).unwrap(),
            DEFAULT_WINDOW_TITLE
        );
    }

    #[test]
    fn find_shortcut_edits_query_navigates_and_releases_page_keys() {
        let mut app = BrowserApp::new("data:text/html,<p>needle needle</p>").unwrap();
        app.modifiers.control = true;
        assert!(app.handle_find_key("f", Some("f"), true));
        app.modifiers.control = false;
        for character in "needle".chars() {
            assert!(app.handle_find_key(
                &character.to_string(),
                Some(&character.to_string()),
                true
            ));
        }
        let ui = app.find_ui.as_ref().unwrap();
        assert_eq!(
            (ui.result.match_count, ui.result.active_match_ordinal),
            (2, 1)
        );
        assert!(find_window_title("Page".into(), app.find_ui.as_ref()).contains("needle (1/2)"));
        assert!(app.handle_find_key("Enter", None, true));
        assert_eq!(app.find_ui.as_ref().unwrap().result.active_match_ordinal, 2);
        app.modifiers.shift = true;
        assert!(app.handle_find_key("Enter", None, true));
        assert_eq!(app.find_ui.as_ref().unwrap().result.active_match_ordinal, 1);
        assert!(app.handle_find_key("Escape", None, true));
        assert!(app.find_ui.is_none());
        assert!(app.handle_find_key("Escape", None, false));
        assert!(!app.handle_find_key("a", Some("a"), true));
        app.modifiers.control = true;
        assert!(app.handle_find_key("f", Some("f"), true));
        app.modifiers.control = false;
        assert!(app.handle_find_key("n", Some("n"), true));
        app.session
            .dispatch(
                "Page.navigate",
                json!({ "url": "data:text/html,<p>new</p>" }),
            )
            .unwrap();
        app.sync_find_document();
        assert!(app.find_ui.is_none());
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let url = std::env::args().nth(1).unwrap_or_else(|| {
        "data:text/html,<title>Omoikane</title><style>body{font:32px sans-serif;padding:2rem}</style><h1>Omoikane</h1><p>Pass a URL as the first argument.</p>".to_string()
    });
    let reporter = configured_error_reporter()?;
    if let Some(reporter) = reporter.as_ref() {
        install_panic_reporter(reporter, ExecutionSurface::Gui);
    }
    let event_loop = EventLoop::new()?;
    let mut app = BrowserApp::new(&url)?;
    if let Some(reporter) = reporter {
        app.set_error_reporter(reporter);
    }
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod gui_error_reporting_tests {
    use super::*;
    use omoikane::error_reporting::EventStore;

    #[test]
    fn gui_failure_boundaries_record_only_fixed_safe_events() {
        let directory =
            std::env::temp_dir().join(format!("omoikane-gui-errors-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("events.sqlite");
        let config = ReporterConfig::from_values(Some("record-only"), None).unwrap();
        let reporter =
            ErrorReporter::new(&config, database.clone(), RetentionPolicy::default()).unwrap();
        let failures = [
            GuiFailure::Window,
            GuiFailure::DisplayContext,
            GuiFailure::SurfaceCreate,
            GuiFailure::SurfaceOperation,
            GuiFailure::Input,
            GuiFailure::Frame,
        ];
        for failure in failures {
            report_gui_failure(
                Some(&reporter),
                failure,
                &"SECRET_GUI /home/private/page?token=SECRET_GUI",
            );
        }
        reporter.flush().unwrap();
        let store = EventStore::open(&database, RetentionPolicy::default()).unwrap();
        assert_eq!(store.len().unwrap(), failures.len());
        for failure in failures {
            let event = failure.event();
            let stored = store.get(event.fingerprint()).unwrap().unwrap();
            assert_eq!(stored.error_code, event.code().as_str());
            assert_eq!(stored.category, "gui");
            assert_eq!(stored.surface, "gui");
        }
        drop(store);
        drop(reporter);
        for suffix in ["", "-wal", "-shm"] {
            let mut path = database.as_os_str().to_os_string();
            path.push(suffix);
            if let Ok(contents) = std::fs::read(PathBuf::from(path)) {
                for secret in [b"SECRET_GUI".as_slice(), b"/home/private", b"token="] {
                    assert!(!contents.windows(secret.len()).any(|bytes| bytes == secret));
                }
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
