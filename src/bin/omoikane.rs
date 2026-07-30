use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use omoikane::cdp::CdpSession;
use omoikane::frame::render_browser_frame;
use omoikane::platform_input::{
    InputModifiers, PlatformInput, PlatformKeyEvent, PlatformMouseButton,
};
use serde_json::json;
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

struct BrowserApp {
    session: CdpSession,
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    started_at: Instant,
    input: PlatformInput,
}

impl BrowserApp {
    fn new(url: &str) -> Result<Self, Box<dyn Error>> {
        let mut session = CdpSession::new().map_err(std::io::Error::other)?;
        session.dispatch("Page.navigate", json!({ "url": url }))?;
        Ok(Self {
            session,
            window: None,
            context: None,
            surface: None,
            started_at: Instant::now(),
            input: PlatformInput::new(),
        })
    }

    fn draw(&mut self) -> Result<(), Box<dyn Error>> {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        let elapsed_ms = self
            .started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let frame = render_browser_frame(&mut self.session, size.width, size.height, elapsed_ms)?;

        surface.resize(width, height)?;
        let mut target = surface.buffer_mut()?;
        for (destination, source) in target.iter_mut().zip(frame.pixels().chunks_exact(4)) {
            *destination =
                u32::from(source[0]) << 16 | u32::from(source[1]) << 8 | u32::from(source[2]);
        }
        target.present()?;
        Ok(())
    }

    fn dispatch_input(&mut self, event: &WindowEvent) {
        let result = match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.input
                    .cursor_moved(&mut self.session, position.x, position.y)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(button) = platform_mouse_button(*button) else {
                    return;
                };
                self.input
                    .mouse_button(&mut self.session, button, *state == ElementState::Pressed)
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.input.set_modifiers(InputModifiers {
                    alt: state.alt_key(),
                    control: state.control_key(),
                    meta: state.super_key(),
                    shift: state.shift_key(),
                });
                return;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let text = event
                    .text
                    .as_deref()
                    .filter(|text| text.chars().all(|character| !character.is_control()))
                    .map(ToOwned::to_owned);
                self.input.key_event(
                    &mut self.session,
                    PlatformKeyEvent {
                        pressed: event.state == ElementState::Pressed,
                        key: logical_key_name(&event.logical_key),
                        code: physical_key_code(event.physical_key),
                        text,
                        repeat: event.repeat,
                    },
                )
            }
            _ => return,
        };
        if let Err(error) = result {
            eprintln!("input event failed: {error}");
        }
    }
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
        PhysicalKey::Unidentified(_) => String::new(),
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
                eprintln!("failed to create window: {error}");
                event_loop.exit();
                return;
            }
        };
        let context = match Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("failed to create display context: {error}");
                event_loop.exit();
                return;
            }
        };
        let surface = match Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                eprintln!("failed to create window surface: {error}");
                event_loop.exit();
                return;
            }
        };
        window.request_redraw();
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    eprintln!("frame failed: {error}");
                }
            }
            event => self.dispatch_input(&event),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + FRAME_INTERVAL));
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::keyboard::KeyCode;

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
    }

    #[test]
    fn translates_supported_mouse_buttons_and_ignores_other_buttons() {
        assert_eq!(
            platform_mouse_button(MouseButton::Left),
            Some(PlatformMouseButton::Left)
        );
        assert_eq!(platform_mouse_button(MouseButton::Other(8)), None);
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let url = std::env::args().nth(1).unwrap_or_else(|| {
        "data:text/html,<title>Omoikane</title><style>body{font:32px sans-serif;padding:2rem}</style><h1>Omoikane</h1><p>Pass a URL as the first argument.</p>".to_string()
    });
    let event_loop = EventLoop::new()?;
    let mut app = BrowserApp::new(&url)?;
    event_loop.run_app(&mut app)?;
    Ok(())
}
