//! Native window owner. Page execution and painting live in `PageWorker`.
use super::chrome_layout::WindowRegion;
use super::page_worker::{
    AsyncEvent, NativeWheel, PageCommand as Command, PagePaintTrace, PageUpdate, PageWorker,
};
use super::toolbar_paint::url_field;
use super::url_bar::{UrlBarKey, UrlBarOutcome, navigation_target};
use super::*;
use omoikane::frame::BrowserFrame;
use omoikane::platform_input::PlatformTouchPhase;
use winit::event::TouchPhase;

struct FindBar {
    query: String,
    result: FindInPageResult,
    document: u64,
}

/// Only native resources and owned presentation snapshots remain here.
pub(super) struct AsyncBrowserApp {
    worker: PageWorker,
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    frame: Option<BrowserFrame>,
    pending_paint_trace: Option<PagePaintTrace>,
    trace_paint_started: Option<Instant>,
    paint_sequence: u64,
    trace_input: bool,
    input_sequence: u64,
    page_busy: bool,
    cookie_dirty: bool,
    last_pointer_input_at: Option<Instant>,
    wheel_frame_deadline: Instant,
    needs_paint: bool,
    next_frame: Instant,
    next_rendering: omoikane::cdp::NextRendering,
    initial_url: Option<String>,
    initial_navigation_pending: bool,
    pub(super) initial_error: Option<String>,
    startup_trace: Option<Instant>,
    first_window_redraw: bool,
    first_page_redraw: bool,
    reporter: Option<Arc<ErrorReporter>>,
    url_bar: UrlBar,
    page_title: String,
    find: Option<FindBar>,
    document: u64,
    chrome_key_releases: HashSet<String>,
    modifiers: InputModifiers,
    cursor: (f64, f64),
    cursor_in_page: bool,
    page_buttons: Vec<PlatformMouseButton>,
    page_touches: HashSet<u64>,
    native_fullscreen: bool,
    pointer_locked: bool,
    raw_buttons: bool,
    pointer_restore: (f64, f64),
    occluded: bool,
    minimized: bool,
    media_preferences: omoikane::platform_media::PlatformMediaPreferences,
    pointer_devices: omoikane::platform_media::PointerDeviceTracker<DeviceId>,
    monitor: Option<omoikane::platform_media::MediaPreferenceMonitor>,
    toolbar_font: OnceCell<Option<Font>>,
}

impl AsyncBrowserApp {
    pub(super) fn new(
        url: String,
        wake: winit::event_loop::EventLoopProxy<AsyncEvent>,
        reporter: Option<Arc<ErrorReporter>>,
        startup_trace: Option<Instant>,
    ) -> std::io::Result<Self> {
        let media_preferences = omoikane::platform_media::capture_media_preferences();
        Ok(Self {
            worker: PageWorker::start(wake, reporter.clone())?,
            window: None,
            context: None,
            surface: None,
            frame: None,
            pending_paint_trace: None,
            trace_paint_started: std::env::var_os("OMOIKANE_TRACE_PAINT").map(|_| Instant::now()),
            paint_sequence: 0,
            trace_input: std::env::var_os("OMOIKANE_TRACE_INPUT").is_some(),
            input_sequence: 0,
            page_busy: false,
            cookie_dirty: false,
            last_pointer_input_at: None,
            wheel_frame_deadline: Instant::now(),
            needs_paint: true,
            next_frame: Instant::now(),
            next_rendering: omoikane::cdp::NextRendering::EveryFrame,
            initial_url: Some(url.clone()),
            initial_navigation_pending: true,
            initial_error: None,
            startup_trace,
            first_window_redraw: true,
            first_page_redraw: true,
            reporter,
            url_bar: UrlBar::new(url),
            page_title: DEFAULT_WINDOW_TITLE.into(),
            find: None,
            document: 0,
            chrome_key_releases: HashSet::new(),
            modifiers: InputModifiers::default(),
            cursor: (0.0, 0.0),
            cursor_in_page: false,
            page_buttons: Vec::new(),
            page_touches: HashSet::new(),
            native_fullscreen: false,
            pointer_locked: false,
            raw_buttons: false,
            pointer_restore: (0.0, 0.0),
            occluded: false,
            minimized: false,
            pointer_devices: omoikane::platform_media::PointerDeviceTracker::new(
                media_preferences.pointers,
            ),
            media_preferences,
            monitor: None,
            toolbar_font: OnceCell::new(),
        })
    }

    fn layout(&self) -> ChromeLayout {
        let size = self
            .window
            .as_ref()
            .map_or_else(Default::default, |window| window.inner_size());
        let scale = DeviceScale::new(
            self.window
                .as_ref()
                .map_or(1.0, |window| window.scale_factor()),
        );
        ChromeLayout::new(size.width, size.height, scale, !self.native_fullscreen)
    }

    pub(super) fn trace_startup(&self, phase: &str) {
        if let Some(started) = self.startup_trace {
            eprintln!(
                "OMOIKANE_STARTUP {}",
                json!({"phase":phase,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
            );
        }
    }

    fn queue_frame(&mut self, run_tasks: bool) {
        if self.initial_url.is_some() || self.page_busy {
            return;
        }
        self.page_busy = true;
        self.cookie_dirty = false;
        self.worker.send(Command::Frame {
            at: Instant::now(),
            run_tasks: run_tasks && !self.pointer_input_is_recent(),
        });
    }

    fn pointer_input_is_recent(&self) -> bool {
        self.last_pointer_input_at
            .is_some_and(|last| last.elapsed() < POINTER_TASK_QUIET_PERIOD)
    }

    fn apply_updates(&mut self) {
        while let Ok(update) = self.worker.try_recv() {
            self.apply_update(update);
        }
    }

    fn apply_update(&mut self, update: PageUpdate) {
        if let Some(error) = update.error {
            eprintln!("page worker operation failed: {error}");
            report_gui_failure(self.reporter.as_deref(), GuiFailure::Frame, &error);
            if update.navigation_completed && self.initial_navigation_pending {
                self.initial_error = Some(error);
            }
        }
        if update.navigation_completed {
            self.initial_navigation_pending = false;
            self.trace_startup("navigation_finished");
        }
        if update.frame_completed {
            self.page_busy = false;
        }
        self.needs_paint = update.demand.needs_paint;
        self.next_rendering = update.demand.next;
        self.url_bar.set_page_url(update.url);
        self.page_title = update.title;
        if self.document != update.document_generation {
            self.document = update.document_generation;
            if self
                .find
                .as_ref()
                .is_some_and(|find| find.document != self.document)
            {
                self.find = None;
            }
        }
        if let Some(find) = &mut self.find {
            if let Some(result) = update.find {
                find.result = result;
            }
        }
        if let Some(frame) = update.frame {
            self.frame = Some(frame);
            self.pending_paint_trace = update.paint_trace;
        }
        if let Some(transition) = update.fullscreen {
            self.apply_fullscreen(transition);
        }
        if let Some(transition) = update.pointer_lock {
            self.apply_pointer_lock(transition);
        }
        self.update_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        let now = Instant::now();
        self.next_frame = if update.demand.needs_paint {
            now
        } else {
            match update.demand.next {
                omoikane::cdp::NextRendering::EveryFrame => now + FRAME_INTERVAL,
                omoikane::cdp::NextRendering::After(delay)
                | omoikane::cdp::NextRendering::AfterWall(delay) => now + delay,
                omoikane::cdp::NextRendering::AfterBoth {
                    page_delay,
                    wall_delay,
                } => now + page_delay.min(wall_delay),
                omoikane::cdp::NextRendering::Idle => now + Duration::from_secs(86400),
            }
        };
        if self.cookie_dirty && !self.page_busy {
            self.next_frame = Instant::now();
            self.queue_frame(true);
        }
    }

    fn update_title(&self) {
        if let Some(window) = &self.window {
            let title = match &self.find {
                Some(find) => format!(
                    "Find: {} ({}/{}) — {}",
                    find.query,
                    find.result.active_match_ordinal,
                    find.result.match_count,
                    self.page_title
                ),
                None => self.page_title.clone(),
            };
            window.set_title(&title);
        }
    }

    fn apply_fullscreen(&mut self, transition: FullscreenTransition) {
        let Some(window) = &self.window else {
            return;
        };
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
        self.send_viewport();
        self.send_media_environment();
    }

    fn unlock_pointer(&mut self) {
        if let Some(window) = &self.window {
            let _ = pointer_lock_host::set_grab(window, false);
            window.set_cursor_visible(true);
            if self.pointer_locked {
                let _ = window.set_cursor_position(LogicalPosition::new(
                    self.pointer_restore.0,
                    self.pointer_restore.1,
                ));
            }
        }
        self.pointer_locked = false;
        self.raw_buttons = false;
    }

    fn apply_pointer_lock(&mut self, transition: PointerLockTransition) {
        match transition {
            PointerLockTransition::Release => self.unlock_pointer(),
            PointerLockTransition::Acquire {
                request_id,
                unadjusted_movement,
            } => {
                let accepted = self.window.as_ref().is_some_and(|window| {
                    !unadjusted_movement && pointer_lock_host::set_grab(window, true).is_ok()
                });
                if accepted {
                    let layout = self.layout();
                    let (page_x, page_y) = layout.page_point(self.cursor.0, self.cursor.1);
                    self.pointer_restore = layout.window_logical_point(page_x, page_y);
                    if let Some(window) = &self.window {
                        window.set_cursor_visible(false);
                        self.raw_buttons = pointer_lock_host::uses_raw_buttons(window);
                    }
                    self.pointer_locked = true;
                }
                self.worker.send(Command::PointerLockReply {
                    request_id,
                    accepted,
                });
            }
        }
    }

    fn present(&mut self) {
        let started = self.pending_paint_trace.as_ref().map(|_| Instant::now());
        let layout = self.layout();
        let page = layout.page();
        let toolbar = layout.toolbar();
        let Some(width) = NonZeroU32::new(page.width) else {
            return;
        };
        let Some(height) = NonZeroU32::new(toolbar.height + page.height) else {
            return;
        };
        let Some(surface) = &mut self.surface else {
            return;
        };
        if let Err(error) = surface.resize(width, height) {
            eprintln!("surface resize failed: {error}");
            return;
        }
        let Ok(mut target) = surface.buffer_mut() else {
            return;
        };
        target.fill(0x00ff_ffff);
        if let Some(frame) = self.frame.as_ref().filter(|_| page.height > 0) {
            let rows = page.y as usize * page.width as usize
                ..(page.y + page.height) as usize * page.width as usize;
            blit_scaled(
                frame.pixels(),
                (frame.width(), frame.height()),
                &mut target[rows],
                (page.width, page.height),
                layout.scale(),
            );
        }
        paint_toolbar(
            &mut target,
            page.width,
            toolbar,
            layout.scale(),
            &self.url_bar,
            self.toolbar_font.get_or_init(load_toolbar_font).as_ref(),
        );
        if let Err(error) = target.present() {
            eprintln!("surface present failed: {error}");
            return;
        }
        if let (Some(started), Some(trace)) = (started, self.pending_paint_trace.take()) {
            self.paint_sequence += 1;
            eprintln!(
                "OMOIKANE_PAINT {}",
                json!({
                    "sequence":self.paint_sequence,
                    "elapsed_ms":self.trace_paint_started.map_or(0,|origin|origin.elapsed().as_millis()),
                    "frame_ms":trace.frame.as_secs_f64()*1000.0,
                    "adjusted_layout_ms":trace.timings.layout.as_secs_f64()*1000.0,
                    "paint_ms":trace.timings.paint.as_secs_f64()*1000.0,
                    "present_ms":started.elapsed().as_secs_f64()*1000.0,
                    "scale":layout.scale().factor(),
                    "viewport":layout.page_viewport(),
                })
            );
        }
        if self.frame.is_some() && !self.initial_navigation_pending && self.first_page_redraw {
            self.trace_startup("first_page_redraw_attempted");
            self.first_page_redraw = false;
        }
    }

    fn send_viewport(&self) {
        let (width, height) = self.layout().page_viewport();
        self.worker.send(Command::Viewport(width, height));
    }

    fn send_media_environment(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let mut environment = omoikane::css::MediaEnvironment::default();
        environment.resolution_dppx = window.scale_factor() as f32;
        if let Some(monitor) = window.current_monitor() {
            let size = monitor.size().to_logical::<f64>(monitor.scale_factor());
            environment.device_width = Some(size.width as f32);
            environment.device_height = Some(size.height as f32);
        }
        environment.color_scheme_dark = window.theme() == Some(winit::window::Theme::Dark);
        environment.set_feature("prefers-reduced-motion", "no-preference");
        environment.set_feature("prefers-contrast", "no-preference");
        environment.set_feature("forced-colors", "none");
        environment.set_feature("prefers-reduced-transparency", "no-preference");
        environment.set_feature("inverted-colors", "none");
        self.media_preferences.apply_to(&mut environment);
        self.pointer_devices
            .snapshot(omoikane::platform_media::PointerCapabilities {
                fine: true,
                fine_hover: true,
                ..Default::default()
            })
            .apply_to(&mut environment);
        environment.set_feature(
            "display-mode",
            if self.native_fullscreen {
                "fullscreen"
            } else {
                "browser"
            },
        );
        self.worker.send(Command::MediaEnvironment(environment));
    }

    fn pointer_targets_page(&self) -> bool {
        self.pointer_locked
            || !self.page_buttons.is_empty()
            || matches!(
                self.layout().region_at(self.cursor.0, self.cursor.1),
                WindowRegion::Page { .. }
            )
    }

    fn cursor_moved(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        let inside = self.pointer_targets_page();
        if inside {
            let (x, y) = self.layout().page_point(x, y);
            self.worker.send(Command::Cursor(x, y));
        } else if self.cursor_in_page {
            self.worker.send(Command::CursorLeft);
        }
        self.cursor_in_page = inside;
    }

    fn mouse_button(&mut self, button: PlatformMouseButton, pressed: bool) {
        if self.raw_buttons {
            return;
        }
        if !pressed {
            let prior = self.page_buttons.iter().position(|held| *held == button);
            if let Some(index) = prior {
                self.page_buttons.swap_remove(index);
            }
            if prior.is_some() || self.pointer_locked {
                self.worker.send(Command::Mouse(button, false));
            }
            return;
        }
        let layout = self.layout();
        let to_page = self.pointer_locked
            || match layout.region_at(self.cursor.0, self.cursor.1) {
                WindowRegion::Page { .. } => true,
                WindowRegion::Toolbar { .. } => {
                    if button == PlatformMouseButton::Left
                        && url_field(layout.toolbar(), layout.scale())
                            .contains(self.cursor.0, self.cursor.1)
                    {
                        self.focus_url();
                    }
                    false
                }
                WindowRegion::Outside => false,
            };
        if to_page {
            self.url_bar.cancel();
            self.page_buttons.push(button);
            self.worker.send(Command::Mouse(button, true));
        }
    }

    fn focus_url(&mut self) {
        if self.find.take().is_some() {
            self.worker.send(Command::Find {
                action: "stop",
                query: String::new(),
            });
        }
        self.url_bar.focus();
    }

    fn key(&mut self, event: winit::event::KeyEvent, is_synthetic: bool) {
        if is_synthetic {
            return;
        }
        let pressed = event.state == ElementState::Pressed;
        let key = logical_key_name(&event.logical_key);
        if !pressed && self.chrome_key_releases.remove(&key) {
            return;
        }
        let shortcut = (self.modifiers.control || self.modifiers.meta) && !self.modifiers.alt;
        if shortcut && key.eq_ignore_ascii_case("l") {
            if pressed {
                self.chrome_key_releases.insert(key);
                self.focus_url();
            }
            return;
        }
        if shortcut && key.eq_ignore_ascii_case("f") {
            if pressed {
                self.url_bar.cancel();
                self.find.get_or_insert_with(|| FindBar {
                    query: String::new(),
                    result: FindInPageResult {
                        query: String::new(),
                        match_count: 0,
                        active_match_ordinal: 0,
                    },
                    document: self.document,
                });
                self.chrome_key_releases.insert(key);
            }
            return;
        }
        if self.url_bar.is_editing() && pressed {
            self.chrome_key_releases.insert(key.clone());
            let outcome = self.url_key(&key, event.text.as_deref());
            if let UrlBarOutcome::Navigate(text) = outcome {
                self.navigate(text);
            }
            return;
        }
        if self.find.is_some() {
            if !pressed {
                return;
            }
            self.chrome_key_releases.insert(key.clone());
            self.find_key(&key, event.text.as_deref());
            return;
        }
        let text = event
            .text
            .as_deref()
            .filter(|text| text.chars().all(|character| !character.is_control()))
            .map(ToOwned::to_owned);
        self.worker.send(Command::Key(PlatformKeyEvent {
            pressed,
            key,
            code: physical_key_code(event.physical_key),
            text,
            repeat: event.repeat,
        }));
    }

    fn url_key(&mut self, key: &str, text: Option<&str>) -> UrlBarOutcome {
        if self.modifiers.control || self.modifiers.meta {
            if key.eq_ignore_ascii_case("a") {
                return self.url_bar.select_all();
            }
            return UrlBarOutcome::Ignored;
        }
        let edit = match key {
            "Backspace" => Some(UrlBarKey::Backspace),
            "Delete" => Some(UrlBarKey::Delete),
            "ArrowLeft" => Some(UrlBarKey::Left),
            "ArrowRight" => Some(UrlBarKey::Right),
            "Home" => Some(UrlBarKey::Home),
            "End" => Some(UrlBarKey::End),
            "Enter" => Some(UrlBarKey::Enter),
            "Escape" => Some(UrlBarKey::Escape),
            _ => None,
        };
        if let Some(edit) = edit {
            self.url_bar.key(edit, self.modifiers.shift)
        } else if let Some(text) = text {
            self.url_bar.insert_text(text)
        } else {
            UrlBarOutcome::Ignored
        }
    }

    fn navigate(&mut self, text: String) {
        match navigation_target(&text) {
            Ok(url) => self.worker.send(Command::Navigate(url.to_owned())),
            Err(error) => {
                eprintln!("navigation rejected: {error:?}");
                self.url_bar.edit(text);
            }
        }
    }

    fn find_key(&mut self, key: &str, text: Option<&str>) {
        let find = self.find.as_mut().unwrap();
        let action = match key {
            "Escape" => "stop",
            "Enter" => {
                if self.modifiers.shift {
                    "previous"
                } else {
                    "next"
                }
            }
            "Backspace" => {
                find.query.pop();
                "start"
            }
            _ if !self.modifiers.control && !self.modifiers.meta && !self.modifiers.alt => {
                if let Some(text) = text.filter(|value| !value.chars().any(char::is_control)) {
                    find.query.push_str(text);
                    "start"
                } else {
                    return;
                }
            }
            _ => return,
        };
        let query = find.query.clone();
        if action == "stop" {
            self.find = None;
        }
        self.worker.send(Command::Find { action, query });
    }

    fn input(&mut self, event: WindowEvent) {
        self.trace_input_event(&event);
        match event {
            WindowEvent::CursorMoved {
                position,
                device_id,
                ..
            } => {
                self.last_pointer_input_at = Some(Instant::now());
                if self.pointer_devices.observe(
                    device_id,
                    omoikane::platform_media::PointerCapabilities {
                        fine: true,
                        fine_hover: true,
                        ..Default::default()
                    },
                ) {
                    self.send_media_environment();
                }
                self.cursor_moved(position.x, position.y);
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_in_page = false;
                self.worker.send(Command::CursorLeft);
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    // PlatformInput clears its state on blur as well.
                    self.modifiers = InputModifiers::default();
                    self.chrome_key_releases.clear();
                    self.page_buttons.clear();
                    self.unlock_pointer();
                }
                self.worker.send(Command::Focus(focused));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = platform_mouse_button(button) {
                    self.mouse_button(button, state == ElementState::Pressed);
                }
            }
            WindowEvent::MouseWheel {
                device_id,
                delta,
                phase,
            } => {
                self.native_wheel(device_id, delta, phase);
            }
            WindowEvent::Touch(touch) => self.native_touch(touch),
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                let next = InputModifiers {
                    alt: state.alt_key(),
                    control: state.control_key(),
                    meta: state.super_key(),
                    shift: state.shift_key(),
                };
                // X11 can report button-state changes as ModifiersChanged.
                // Only browser-visible modifier changes belong in the page FIFO.
                if next != self.modifiers {
                    self.modifiers = next;
                    self.worker.send(Command::Modifiers(next));
                }
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => self.key(event, is_synthetic),
            WindowEvent::Ime(Ime::Commit(text)) if self.url_bar.is_editing() => {
                self.url_bar.insert_text(&text);
            }
            WindowEvent::Ime(_) if self.url_bar.is_editing() => {}
            WindowEvent::Ime(Ime::Commit(text)) if self.find.is_some() => {
                self.find.as_mut().unwrap().query.push_str(&text);
                self.worker.send(Command::Find {
                    action: "start",
                    query: self.find.as_ref().unwrap().query.clone(),
                });
            }
            WindowEvent::Ime(event) if self.find.is_none() => {
                self.worker.send(Command::Ime(platform_ime_event(event)))
            }
            _ => {}
        }
        self.next_frame = Instant::now();
        self.queue_frame(true);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn native_wheel(
        &mut self,
        device_id: winit::event::DeviceId,
        delta: winit::event::MouseScrollDelta,
        phase: TouchPhase,
    ) {
        if self.pointer_targets_page() && !self.raw_buttons {
            let (x, y) = wheel_delta_css_pixels(delta, self.layout().scale().factor());
            if x != 0.0 || y != 0.0 || phase != TouchPhase::Moved {
                let now = Instant::now();
                if now >= self.wheel_frame_deadline {
                    self.queue_frame(false);
                    self.wheel_frame_deadline = now + FRAME_INTERVAL;
                }
                self.last_pointer_input_at = Some(now);
                self.worker.send(Command::NativeWheel(NativeWheel {
                    delta: (x, y),
                    device: device_id,
                    cursor: self.cursor,
                    modifiers: self.modifiers,
                    pixel_delta: matches!(delta, winit::event::MouseScrollDelta::PixelDelta(_)),
                    phase,
                    document: self.document,
                    received: now,
                }));
            }
        }
    }

    fn native_touch(&mut self, touch: winit::event::Touch) {
        if self.pointer_devices.observe(
            touch.device_id,
            omoikane::platform_media::PointerCapabilities {
                coarse: true,
                ..Default::default()
            },
        ) {
            self.send_media_environment();
        }
        let phase = match touch.phase {
            TouchPhase::Started => PlatformTouchPhase::Started,
            TouchPhase::Moved => PlatformTouchPhase::Moved,
            TouchPhase::Ended => PlatformTouchPhase::Ended,
            TouchPhase::Cancelled => PlatformTouchPhase::Cancelled,
        };
        let to_page = match phase {
            PlatformTouchPhase::Started => {
                self.page_touches.remove(&touch.id);
                match self.layout().region_at(touch.location.x, touch.location.y) {
                    WindowRegion::Page { .. } => {
                        self.url_bar.cancel();
                        self.page_touches.insert(touch.id);
                        true
                    }
                    WindowRegion::Toolbar { .. } => {
                        if url_field(self.layout().toolbar(), self.layout().scale())
                            .contains(touch.location.x, touch.location.y)
                        {
                            self.focus_url();
                        }
                        false
                    }
                    _ => false,
                }
            }
            PlatformTouchPhase::Moved => self.page_touches.contains(&touch.id),
            _ => self.page_touches.remove(&touch.id),
        };
        if to_page {
            let (x, y) = self.layout().page_point(touch.location.x, touch.location.y);
            self.worker.send(Command::Touch(touch.id, phase, x, y));
        }
    }

    fn trace_input_event(&mut self, event: &WindowEvent) {
        if !self.trace_input {
            return;
        }
        let mut record = match event {
            WindowEvent::Focused(focused) => json!({"kind":"focus","focused":focused}),
            WindowEvent::CursorMoved { position, .. } => json!({
                "kind":"cursor","x":position.x,"y":position.y,
                "previous_x":self.cursor.0,"previous_y":self.cursor.1,
            }),
            WindowEvent::MouseInput { state, button, .. } => json!({
                "kind":"mouse_button","button":format!("{button:?}"),
                "pressed":*state==ElementState::Pressed,
            }),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => json!({
                "kind":"key","synthetic":is_synthetic,
                "pressed":event.state==ElementState::Pressed,
                "key":logical_key_name(&event.logical_key),
                "code":physical_key_code(event.physical_key),"repeat":event.repeat,
            }),
            _ => return,
        };
        self.input_sequence += 1;
        record["sequence"] = json!(self.input_sequence);
        eprintln!("OMOIKANE_INPUT {record}");
    }
}

impl ApplicationHandler<AsyncEvent> for AsyncBrowserApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title(DEFAULT_WINDOW_TITLE)
                .with_inner_size(LogicalSize::new(1280, 720)),
        ) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("window creation failed: {error}");
                event_loop.exit();
                return;
            }
        };
        window.set_ime_allowed(true);
        let context = match Context::new(window.clone()) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("display context failed: {error}");
                event_loop.exit();
                return;
            }
        };
        let surface = match Surface::new(&context, window.clone()) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("surface failed: {error}");
                event_loop.exit();
                return;
            }
        };
        let weak = Arc::downgrade(&window);
        self.monitor = omoikane::platform_media::MediaPreferenceMonitor::start(
            self.media_preferences.clone(),
            Duration::from_secs(2),
            move || {
                if let Some(window) = weak.upgrade() {
                    window.request_redraw();
                }
            },
        )
        .ok();
        self.window = Some(window);
        self.trace_startup("window_created");
        self.context = Some(context);
        self.trace_startup("display_context_ready");
        self.surface = Some(surface);
        self.trace_startup("surface_ready");
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        self.send_media_environment();
        self.send_viewport();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AsyncEvent) {
        match event {
            AsyncEvent::PageUpdate => self.apply_updates(),
            AsyncEvent::CookieChanged => {
                self.cookie_dirty = true;
                self.next_frame = Instant::now();
                self.queue_frame(true);
            }
        }
        if self.initial_error.is_some() {
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        self.apply_updates();
        match event {
            WindowEvent::CloseRequested => {
                self.worker.send(Command::Visibility(true));
                self.worker.send(Command::ReleasePointerLock);
                self.unlock_pointer();
                event_loop.exit();
            }
            WindowEvent::Occluded(value) => {
                self.occluded = value;
                self.worker
                    .send(Command::Visibility(self.occluded || self.minimized));
            }
            WindowEvent::Resized(size) => {
                self.minimized = size.width == 0 || size.height == 0;
                self.worker
                    .send(Command::Visibility(self.occluded || self.minimized));
                if self.native_fullscreen
                    && self
                        .window
                        .as_ref()
                        .is_some_and(|window| window.fullscreen().is_none())
                {
                    self.worker.send(Command::FullscreenExited);
                    self.native_fullscreen = false;
                    self.send_media_environment();
                }
                self.send_viewport();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.send_media_environment();
                self.send_viewport();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ThemeChanged(_) => self.send_media_environment(),
            WindowEvent::RedrawRequested => {
                self.present();
                if self.first_window_redraw {
                    self.trace_startup("first_window_redraw_attempted");
                    self.first_window_redraw = false;
                    if let Some(url) = self.initial_url.take() {
                        self.trace_startup("navigation_started");
                        self.worker.send(Command::Navigate(url));
                        // Present parsed content before deferred scripts run.
                        self.queue_frame(false);
                    }
                }
            }
            event => self.input(event),
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        if matches!(event, DeviceEvent::Added | DeviceEvent::Removed) {
            if matches!(event, DeviceEvent::Removed) {
                self.pointer_devices.remove(&id);
            }
            self.pointer_devices
                .set_native(omoikane::platform_media::capture_pointer_capabilities());
            self.send_media_environment();
        }
        if !self.pointer_locked {
            return;
        }
        match event {
            DeviceEvent::MouseMotion { delta } => {
                self.worker.send(Command::RelativeMotion(delta.0, delta.1))
            }
            DeviceEvent::Button { button, state } if self.raw_buttons => {
                let pressed = state == ElementState::Pressed;
                let mapped = match button {
                    1 => Some(PlatformMouseButton::Left),
                    2 => Some(PlatformMouseButton::Middle),
                    3 => Some(PlatformMouseButton::Right),
                    8 => Some(PlatformMouseButton::Back),
                    9 => Some(PlatformMouseButton::Forward),
                    _ => None,
                };
                if let Some(button) = mapped {
                    self.worker.send(Command::Mouse(button, pressed));
                } else if pressed {
                    let delta = match button {
                        4 => Some((0.0, -40.0)),
                        5 => Some((0.0, 40.0)),
                        6 => Some((-40.0, 0.0)),
                        7 => Some((40.0, 0.0)),
                        _ => None,
                    };
                    if let Some((x, y)) = delta {
                        self.worker.send(Command::Wheel(x, y));
                    }
                }
            }
            DeviceEvent::MouseWheel { delta } if self.raw_buttons => {
                let (x, y) = wheel_delta_css_pixels(delta, self.layout().scale().factor());
                self.worker.send(Command::Wheel(x, y));
            }
            _ => {}
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.worker.send(Command::Focus(false));
        self.unlock_pointer();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.apply_updates();
        if self.initial_url.is_some() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if let Some(monitor) = &self.monitor {
            if let Some(preferences) = monitor.take_latest() {
                self.pointer_devices.set_native(preferences.pointers);
                self.media_preferences = preferences;
                self.send_media_environment();
            }
        }
        let now = Instant::now();
        if self
            .last_pointer_input_at
            .is_some_and(|last| now >= last + POINTER_TASK_QUIET_PERIOD)
        {
            self.last_pointer_input_at = None;
            self.next_frame = now;
        }
        if now >= self.next_frame && !self.page_busy {
            self.queue_frame(true);
            self.next_frame = now + FRAME_INTERVAL;
        }
        if self.page_busy {
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            let wake_at = self
                .last_pointer_input_at
                .map(|last| last + POINTER_TASK_QUIET_PERIOD);
            let deadline = wake_at.map_or(self.next_frame, |wake| self.next_frame.min(wake));
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                deadline.max(now + Duration::from_millis(1)),
            ));
        }
    }
}
