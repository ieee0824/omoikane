//! The GUI page owner. Boa and DOM handles are created and used on this thread.
use std::cell::Cell;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use omoikane::cdp::{CdpSession, RenderDemand};
use omoikane::css::MediaEnvironment;
use omoikane::error_reporting::{ErrorReporter, ExecutionSurface};
use omoikane::frame::{BrowserFrame, BrowserFrameCache};
use omoikane::js::{FindInPageResult, FullscreenTransition, PointerLockTransition};
use omoikane::platform_input::{
    InputModifiers, PlatformImeEvent, PlatformInput, PlatformKeyEvent, PlatformMouseButton,
    PlatformTouchPhase,
};
use serde_json::json;
use winit::event_loop::EventLoopProxy;

/// Native-loop notification from the page owner or cookie jar.
pub(super) enum AsyncEvent {
    PageUpdate,
    CookieChanged,
}

struct CookieWake(Arc<EventLoopProxy<AsyncEvent>>);

impl std::task::Wake for CookieWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let _ = self.0.send_event(AsyncEvent::CookieChanged);
    }
}

/// Values crossing from the native event loop to the page owner.
pub(super) enum PageCommand {
    Navigate(String),
    Viewport(u32, u32),
    Visibility(bool),
    MediaEnvironment(MediaEnvironment),
    Cursor(f64, f64),
    CursorLeft,
    Focus(bool),
    Mouse(PlatformMouseButton, bool),
    Wheel(f64, f64),
    NativeWheel(NativeWheel),
    Touch(u64, PlatformTouchPhase, f64, f64),
    Key(PlatformKeyEvent),
    Ime(PlatformImeEvent),
    Modifiers(InputModifiers),
    RelativeMotion(f64, f64),
    Find {
        action: &'static str,
        query: String,
    },
    PointerLockReply {
        request_id: u64,
        accepted: bool,
    },
    ReleasePointerLock,
    FullscreenExited,
    /// A rendering opportunity measured on the native monotonic clock.
    Frame {
        at: Instant,
        run_tasks: bool,
    },
    Stop,
}

/// Owned native receipt metadata. Pixel wheels remain unmerged: winit does not
/// expose a separate momentum identity on macOS.
#[derive(Clone)]
pub(super) struct NativeWheel {
    pub(super) delta: (f64, f64),
    pub(super) device: winit::event::DeviceId,
    pub(super) cursor: (f64, f64),
    pub(super) modifiers: InputModifiers,
    pub(super) pixel_delta: bool,
    pub(super) phase: winit::event::TouchPhase,
    pub(super) document: u64,
    pub(super) received: Instant,
}

impl NativeWheel {
    fn merge(&mut self, next: &Self) -> bool {
        use winit::event::TouchPhase;
        let gap = next.received.checked_duration_since(self.received);
        if self.pixel_delta
            || next.pixel_delta
            || self.phase != TouchPhase::Moved
            || next.phase != TouchPhase::Moved
            || self.device != next.device
            || self.cursor != next.cursor
            || self.modifiers != next.modifiers
            || self.document != next.document
            || !gap.is_some_and(|gap| gap <= std::time::Duration::from_millis(50))
        {
            return false;
        }
        fn same_direction(a: f64, b: f64) -> bool {
            a.is_finite()
                && b.is_finite()
                && ((a == 0.0 && b == 0.0) || (a != 0.0 && b != 0.0 && a.signum() == b.signum()))
        }
        let sum = (self.delta.0 + next.delta.0, self.delta.1 + next.delta.1);
        if !same_direction(self.delta.0, next.delta.0)
            || !same_direction(self.delta.1, next.delta.1)
            || !sum.0.is_finite()
            || !sum.1.is_finite()
        {
            return false;
        }
        self.delta = sum;
        self.received = next.received;
        true
    }
}

/// The latest page image and native state, with no DOM or Boa references.
pub(super) struct PageUpdate {
    pub(super) frame_completed: bool,
    pub(super) navigation_completed: bool,
    pub(super) deferred_task_turn: bool,
    pub(super) frame: Option<BrowserFrame>,
    pub(super) paint_trace: Option<PagePaintTrace>,
    pub(super) demand: RenderDemand,
    pub(super) url: String,
    pub(super) title: String,
    pub(super) document_generation: u64,
    pub(super) fullscreen: Option<FullscreenTransition>,
    pub(super) pointer_lock: Option<PointerLockTransition>,
    pub(super) find: Option<FindInPageResult>,
    pub(super) error: Option<String>,
}

/// Worker-only page work; native surface presentation is measured separately.
pub(super) struct PagePaintTrace {
    pub(super) frame: std::time::Duration,
    pub(super) timings: omoikane::paint::RenderTimings,
}

struct QueuedCommand {
    command: PageCommand,
    input: Option<QueuedInput>,
}

struct QueuedInput {
    sequence: u64,
    last_sequence: u64,
    source_count: u64,
    kind: &'static str,
    queued_at: Instant,
}

/// Pull only immediately queued native wheels; retain the first barrier verbatim.
fn next_command(
    commands: &Receiver<QueuedCommand>,
    pending: &mut Option<QueuedCommand>,
    document: u64,
) -> Option<QueuedCommand> {
    let mut current = pending.take().or_else(|| commands.recv().ok())?;
    if !matches!(&current.command, PageCommand::NativeWheel(wheel) if wheel.document == document) {
        return Some(current);
    }
    // Bound work independently of tracing, even while a producer keeps sending.
    let mut source_count = 1;
    while source_count < 128 {
        let Ok(next) = commands.try_recv() else { break };
        let merged = match (&mut current.command, &next.command) {
            (PageCommand::NativeWheel(wheel), PageCommand::NativeWheel(next)) => wheel.merge(next),
            _ => false,
        };
        if !merged {
            *pending = Some(next);
            break;
        }
        source_count += 1;
        if let (Some(current), Some(next)) = (&mut current.input, next.input) {
            current.last_sequence = next.last_sequence;
            current.source_count += next.source_count;
        }
    }
    Some(current)
}

impl PageCommand {
    fn input_kind(&self) -> Option<&'static str> {
        match self {
            Self::NativeWheel(wheel) if wheel.delta == (0.0, 0.0) => Some("wheel_phase"),
            Self::Wheel(..) | Self::NativeWheel(..) => Some("wheel"),
            Self::Cursor(..) | Self::CursorLeft => Some("cursor"),
            Self::Mouse(..) => Some("mouse"),
            Self::Touch(..) => Some("touch"),
            Self::Key(..) => Some("key"),
            Self::Ime(..) => Some("ime"),
            Self::RelativeMotion(..) => Some("relative_motion"),
            _ => None,
        }
    }
}

pub(super) struct PageWorker {
    sender: Sender<QueuedCommand>,
    trace_input: bool,
    input_sequence: Cell<u64>,
    updates: Receiver<PageUpdate>,
}

impl PageWorker {
    /// Starts a page owner without transferring a non-Send session between threads.
    pub(super) fn start(
        wake: EventLoopProxy<AsyncEvent>,
        reporter: Option<Arc<ErrorReporter>>,
    ) -> std::io::Result<Self> {
        let (sender, commands) = mpsc::channel();
        let (updates, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("omoikane-page".into())
            .spawn(move || {
                let wake = Arc::new(wake);
                let cookie_waker = std::task::Waker::from(Arc::new(CookieWake(Arc::clone(&wake))));
                let mut owner = match PageOwner::new(reporter, cookie_waker) {
                    Ok(owner) => owner,
                    Err(error) => {
                        eprintln!("page worker startup failed: {error}");
                        let _ = updates.send(PageUpdate {
                            frame_completed: false,
                            navigation_completed: true,
                            deferred_task_turn: false,
                            frame: None,
                            paint_trace: None,
                            demand: RenderDemand {
                                needs_paint: false,
                                next: omoikane::cdp::NextRendering::Idle,
                            },
                            url: String::new(),
                            title: super::DEFAULT_WINDOW_TITLE.into(),
                            document_generation: 0,
                            fullscreen: None,
                            pointer_lock: None,
                            find: None,
                            error: Some(error.to_string()),
                        });
                        let _ = wake.send_event(AsyncEvent::PageUpdate);
                        return;
                    }
                };
                let mut pending = None;
                while let Some(QueuedCommand { command, input }) = next_command(&commands, &mut pending, owner.session.document_generation()) {
                    let queue_ms = input
                        .as_ref()
                        .map(|input| input.queued_at.elapsed().as_secs_f64() * 1000.0);
                    let started = input.as_ref().map(|_| Instant::now());
                    if matches!(command, PageCommand::Stop) {
                        break;
                    }
                    let frame_completed = matches!(command, PageCommand::Frame { .. });
                    let navigation_completed = matches!(command, PageCommand::Navigate(_));
                    let result = owner.apply(command);
                    if let (Some(input), Some(queue_ms), Some(started)) = (input, queue_ms, started)
                    {
                        eprintln!(
                            "OMOIKANE_PAGE_INPUT {}",
                            json!({
                                "sequence": input.sequence, "first_sequence": input.sequence,
                                "last_sequence": input.last_sequence, "source_count": input.source_count,
                                "kind": input.kind,
                                "queue_ms": queue_ms,
                                "execute_ms": started.elapsed().as_secs_f64() * 1000.0,
                                "success": result.is_ok(),
                            })
                        );
                    }
                    let update = owner.snapshot(
                        frame_completed,
                        navigation_completed,
                        result.err().map(|error| error.to_string()),
                    );
                    if updates.send(update).is_err() {
                        break;
                    }
                    let _ = wake.send_event(AsyncEvent::PageUpdate);
                }
            })?;
        Ok(Self {
            sender,
            trace_input: std::env::var_os("OMOIKANE_TRACE_INPUT").is_some(),
            input_sequence: Cell::new(0),
            updates: receiver,
        })
    }

    pub(super) fn send(&self, command: PageCommand) {
        let input = if self.trace_input {
            command.input_kind().map(|kind| {
                let sequence = self.input_sequence.get().saturating_add(1);
                self.input_sequence.set(sequence);
                QueuedInput {
                    sequence,
                    last_sequence: sequence,
                    source_count: 1,
                    kind,
                    queued_at: match &command {
                        PageCommand::NativeWheel(wheel) => wheel.received,
                        _ => Instant::now(),
                    },
                }
            })
        } else {
            None
        };
        if self.sender.send(QueuedCommand { command, input }).is_err() {
            eprintln!("page worker exited");
        }
    }

    pub(super) fn try_recv(&self) -> Result<PageUpdate, mpsc::TryRecvError> {
        self.updates.try_recv()
    }
}

impl Drop for PageWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(QueuedCommand {
            command: PageCommand::Stop,
            input: None,
        });
    }
}

struct PageOwner {
    session: CdpSession,
    input: PlatformInput,
    cache: BrowserFrameCache,
    viewport: (u32, u32),
    clock_at: Instant,
    clock_remainder: std::time::Duration,
    clock_document: u64,
    last_frame: Option<BrowserFrame>,
    last_paint_trace: Option<PagePaintTrace>,
    trace_paint: bool,
    find: Option<FindInPageResult>,
    find_document: u64,
    cookie_changes: omoikane::http::CookieChangeSubscription,
    /// A wheel's Document must get a paint attempt before another task turn.
    /// This tracks worker painting, not native surface presentation.
    pending_wheel_paint: Option<u64>,
    deferred_task_turn: bool,
}

impl PageOwner {
    fn new(
        reporter: Option<Arc<ErrorReporter>>,
        cookie_waker: std::task::Waker,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut session = CdpSession::new()?;
        session.set_task_pump_limit(std::num::NonZeroUsize::new(1));
        session.set_pointer_lock_deferred(true);
        session.set_deferred_document_startup(true);
        session.set_nonblocking_image_loading(true);
        session.set_nonblocking_font_loading(true);
        if let Some(reporter) = reporter {
            session.set_error_reporter(reporter, ExecutionSurface::Gui);
        }
        let clock_document = session.document_generation();
        let cookie_changes = session.subscribe_cookie_changes(cookie_waker);
        Ok(Self {
            session,
            input: PlatformInput::new(),
            cache: BrowserFrameCache::default(),
            viewport: (0, 0),
            clock_at: Instant::now(),
            clock_remainder: std::time::Duration::ZERO,
            clock_document,
            last_frame: None,
            last_paint_trace: None,
            trace_paint: std::env::var_os("OMOIKANE_TRACE_PAINT").is_some(),
            find: None,
            find_document: clock_document,
            cookie_changes,
            pending_wheel_paint: None,
            deferred_task_turn: false,
        })
    }

    fn apply(&mut self, command: PageCommand) -> Result<(), Box<dyn std::error::Error>> {
        use PageCommand::*;
        // Native sampling changes delivery count; listener/default processing
        // remains the exact direct-wheel path, including cancelability.
        let command = match command {
            NativeWheel(wheel) if wheel.delta != (0.0, 0.0) => Wheel(wheel.delta.0, wheel.delta.1),
            NativeWheel(_) => return Ok(()), // Phase-only FIFO barrier.
            command => command,
        };
        if !matches!(command, Frame { .. }) {
            self.advance_clock(Instant::now());
        }
        match command {
            Navigate(url) => {
                self.session.dispatch("Page.navigate", json!({"url":url}))?;
                self.pending_wheel_paint = None;
                self.deferred_task_turn = false;
            }
            Viewport(width, height) => {
                self.viewport = (width, height);
                self.session.set_viewport(width, height);
            }
            Visibility(hidden) => self.session.set_host_visibility(hidden),
            MediaEnvironment(environment) => {
                self.session.set_host_media_environment(environment)?
            }
            Cursor(x, y) => self.input.cursor_moved(&mut self.session, x, y)?,
            CursorLeft => self.input.cursor_left(&mut self.session),
            Focus(focused) => self.input.focus_changed(&mut self.session, focused)?,
            Mouse(button, pressed) => {
                self.input
                    .mouse_button(&mut self.session, button, pressed)?
            }
            NativeWheel(_) => unreachable!("native wheel normalized above"),
            Wheel(x, y) => {
                // Keep listener/default-action behavior, including mutations
                // before a listener error. Native receipt time may be stale
                // after this command waits behind a long-running page task.
                self.pending_wheel_paint = Some(self.session.document_generation());
                let started = self.trace_paint.then(Instant::now);
                let result = self.input.wheel(&mut self.session, x, y);
                if let Some(started) = started {
                    eprintln!(
                        "OMOIKANE_WHEEL {}",
                        json!({
                            "dispatch_ms":started.elapsed().as_secs_f64()*1000.0,
                            "delta_x":x,"delta_y":y,"success":result.is_ok(),
                        })
                    );
                }
                result?;
            }
            Touch(id, phase, x, y) => self.input.touch(&mut self.session, id, phase, x, y)?,
            Key(event) => self.input.key_event(&mut self.session, event)?,
            Ime(event) => self.input.ime_event(&mut self.session, event)?,
            Modifiers(modifiers) => self.input.set_modifiers(modifiers),
            RelativeMotion(x, y) => self.input.relative_motion(&mut self.session, x, y)?,
            Find { action, query } => {
                let value = self.session.dispatch(
                    "Omoikane.findInPage",
                    json!({"action":action,"query":query}),
                )?;
                self.find = if action == "stop" {
                    None
                } else {
                    Some(serde_json::from_value(value)?)
                };
                self.find_document = self.session.document_generation();
            }
            PointerLockReply {
                request_id,
                accepted,
            } => {
                self.session
                    .complete_pointer_lock_request(request_id, accepted)?;
            }
            ReleasePointerLock => self.session.release_pointer_lock_from_host()?,
            FullscreenExited => {
                self.session.fullscreen_exited_by_host()?;
            }
            Frame { at, run_tasks } => self.frame(at, run_tasks)?,
            Stop => {}
        }
        Ok(())
    }

    fn frame(&mut self, at: Instant, run_tasks: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.cookie_changes.take_changed();
        if self
            .pending_wheel_paint
            .is_some_and(|document| document != self.session.document_generation())
        {
            self.pending_wheel_paint = None;
        }
        let wheel_paint = self.pending_wheel_paint.is_some();
        let run_tasks = run_tasks && !wheel_paint;
        // The command may have waited behind a long script. Use execution time
        // and never move the page clock back behind an intervening input.
        let at = Instant::now().max(at).max(self.clock_at);
        let startup_pending = self.session.has_pending_document_startup();
        let started = self.trace_paint.then(Instant::now);
        if started.is_some() {
            omoikane::paint::take_last_render_timings();
        }
        let elapsed = self.elapsed_clock_ms(at);
        let painted = if run_tasks {
            self.cache
                .update(&mut self.session, self.viewport.0, self.viewport.1, elapsed)?
        } else {
            self.session.advance_clock_without_tasks(elapsed);
            self.cache.paint_without_advance(&mut self.session)?
        };
        // A canceled wheel can leave pixels unchanged. A successful no-op
        // attempt still releases the task turn; a paint error retains it.
        self.pending_wheel_paint = None;
        if wheel_paint {
            self.deferred_task_turn = true;
        } else if run_tasks {
            self.deferred_task_turn = false;
        }
        if painted {
            self.last_frame = self.cache.frame().cloned();
            if let Some(started) = started {
                self.last_paint_trace = Some(PagePaintTrace {
                    frame: started.elapsed(),
                    timings: omoikane::paint::take_last_render_timings(),
                });
            }
        }
        if startup_pending && !self.session.has_pending_document_startup() {
            self.clock_at = Instant::now();
            self.clock_remainder = std::time::Duration::ZERO;
            self.clock_document = self.session.document_generation();
        }
        Ok(())
    }

    fn elapsed_clock_ms(&mut self, at: Instant) -> u64 {
        let at = at.max(self.clock_at);
        let elapsed = at.saturating_duration_since(self.clock_at) + self.clock_remainder;
        self.clock_at = at;
        let generation = self.session.document_generation();
        if self.clock_document != generation {
            self.clock_document = generation;
            self.clock_remainder = std::time::Duration::ZERO;
            return 0;
        }
        let millis = elapsed.as_millis().min(u64::MAX as u128) as u64;
        self.clock_remainder = elapsed.saturating_sub(std::time::Duration::from_millis(millis));
        millis
    }

    fn advance_clock(&mut self, at: Instant) {
        let elapsed = self.elapsed_clock_ms(at);
        self.session.advance_clock_without_tasks(elapsed);
    }

    fn snapshot(
        &mut self,
        frame_completed: bool,
        navigation_completed: bool,
        error: Option<String>,
    ) -> PageUpdate {
        let generation = self.session.document_generation();
        if generation != self.find_document {
            self.find = None;
            self.find_document = generation;
        }
        let title = super::document_window_title(&mut self.session)
            .unwrap_or_else(|_| super::DEFAULT_WINDOW_TITLE.to_string());
        PageUpdate {
            frame_completed,
            navigation_completed,
            deferred_task_turn: self.deferred_task_turn,
            frame: self.last_frame.take(),
            paint_trace: self.last_paint_trace.take(),
            demand: self.cache.demand(&self.session),
            url: self.session.current_url().to_owned(),
            title,
            document_generation: self.session.document_generation(),
            fullscreen: self.session.take_fullscreen_transition(),
            pointer_lock: self.session.take_pointer_lock_transition(),
            find: self.find.clone(),
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoWake;
    impl std::task::Wake for NoWake {
        fn wake(self: Arc<Self>) {}
    }

    fn owner() -> PageOwner {
        PageOwner::new(None, std::task::Waker::from(Arc::new(NoWake))).unwrap()
    }

    fn data_url(html: &str) -> String {
        let encoded = html
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect::<String>();
        format!("data:text/html,{encoded}")
    }

    fn evaluate(page: &mut PageOwner, expression: &str) -> serde_json::Value {
        page.session
            .dispatch("Runtime.evaluate", json!({"expression": expression}))
            .unwrap()["result"]["value"]
            .clone()
    }

    // Runtime.evaluate completes a task turn, so observe via the owned DOM
    // without running JS or timers while checking paint/task ordering.
    fn background_events(page: &PageOwner) -> Option<String> {
        page.session
            .document()
            .query_selector("body")
            .unwrap()
            .get_attribute("data-events")
    }

    fn wheel_page() -> PageOwner {
        let mut page = owner();
        page.session.set_deferred_document_startup(false);
        page.apply(PageCommand::Viewport(32, 32)).unwrap();
        page.apply(PageCommand::Navigate(data_url(
            "<body style='margin:0'><div style='height:40px;background:red'></div>\
             <div style='height:1960px;background:blue'></div></body>",
        )))
        .unwrap();
        page.frame(Instant::now(), true).unwrap();
        let initial = page.last_frame.take().unwrap();
        assert_eq!(&initial.pixels()[..4], &[255, 0, 0, 255]);
        page.apply(PageCommand::Cursor(8.0, 8.0)).unwrap();
        page.frame(Instant::now(), true).unwrap();
        page.last_frame.take();
        page
    }

    #[test]
    fn queued_wheel_paints_before_due_timer_and_animation_frame_then_resumes_tasks() {
        let mut page = wheel_page();
        evaluate(
            &mut page,
            "globalThis.events=[];\
             addEventListener('wheel',()=>events.push('wheel'));\
             setTimeout(()=>{events.push('timer');document.body.setAttribute('data-events',JSON.stringify(events))},100);\
             requestAnimationFrame(()=>{events.push('raf');document.body.setAttribute('data-events',JSON.stringify(events))})",
        );
        // Model a frame command queued after native input's quiet period has
        // expired, without depending on elapsed wall time or a busy callback.
        page.session.advance_clock_without_tasks(500);
        page.apply(PageCommand::Wheel(0.0, 80.0)).unwrap();
        page.frame(Instant::now(), true).unwrap();
        assert_eq!(background_events(&page), None);
        let scrolled = page
            .last_frame
            .take()
            .expect("wheel state must be painted first");
        assert_eq!(&scrolled.pixels()[..4], &[0, 0, 255, 255]);
        assert!(page.pending_wheel_paint.is_none());
        assert!(page.snapshot(true, false, None).deferred_task_turn);

        page.frame(Instant::now(), true).unwrap();
        assert!(!page.deferred_task_turn);
        assert_eq!(
            background_events(&page).as_deref(),
            Some("[\"wheel\",\"timer\",\"raf\"]")
        );

        // A navigation invalidates the old wheel's paint obligation.
        page.apply(PageCommand::Wheel(0.0, 80.0)).unwrap();
        assert!(page.pending_wheel_paint.is_some());
        page.apply(PageCommand::Navigate(data_url(
            "<title>Replacement</title>",
        )))
        .unwrap();
        assert!(page.pending_wheel_paint.is_none());
        assert!(!page.deferred_task_turn);
    }

    #[test]
    fn canceled_wheel_noop_paint_releases_due_timers_on_next_opportunity() {
        let mut page = wheel_page();
        evaluate(
            &mut page,
            "globalThis.events=[];\
             addEventListener('wheel',event=>{events.push('wheel');event.preventDefault()}, {passive:false});\
             setTimeout(()=>{events.push('timer');document.body.setAttribute('data-events',JSON.stringify(events))},100)",
        );
        page.session.advance_clock_without_tasks(500);
        page.apply(PageCommand::Wheel(0.0, 80.0)).unwrap();
        assert!(page.pending_wheel_paint.is_some());
        assert!(!page.cache.demand(&page.session).needs_paint);
        page.frame(Instant::now(), true).unwrap();
        assert!(
            page.last_frame.is_none(),
            "canceled wheel must not invent a paint"
        );
        assert!(page.pending_wheel_paint.is_none());
        assert!(page.deferred_task_turn);
        assert_eq!(background_events(&page), None);
        page.frame(Instant::now(), true).unwrap();
        assert!(!page.deferred_task_turn);
        assert_eq!(
            background_events(&page).as_deref(),
            Some("[\"wheel\",\"timer\"]")
        );
    }

    #[test]
    fn merged_native_wheel_retains_cancelability_and_cancels_total_delta() {
        let mut page = wheel_page();
        evaluate(
            &mut page,
            "addEventListener('wheel',event=>{document.body.setAttribute('data-wheel',JSON.stringify([event.cancelable,event.deltaY]));event.preventDefault()}, {passive:false})",
        );
        let native = NativeWheel {
            delta: (0.0, 40.0),
            device: winit::event::DeviceId::dummy(),
            cursor: (10.0, 10.0),
            modifiers: InputModifiers::default(),
            pixel_delta: false,
            phase: winit::event::TouchPhase::Moved,
            document: page.session.document_generation(),
            received: Instant::now(),
        };
        let mut merged = native.clone();
        assert!(merged.merge(&native));
        page.apply(PageCommand::NativeWheel(merged)).unwrap();
        assert_eq!(
            evaluate(&mut page, "document.body.getAttribute('data-wheel')"),
            json!("[true,80]")
        );
        assert_eq!(evaluate(&mut page, "scrollY"), json!(0));
    }

    #[test]
    fn owned_navigation_input_find_and_frame_share_one_page_owner() {
        let mut page = owner();
        page.apply(PageCommand::Viewport(320, 200)).unwrap();
        page.apply(PageCommand::Navigate(data_url(
            "<title>Worker</title><div style='height:2000px'>needle</div>",
        )))
        .unwrap();
        page.apply(PageCommand::Frame {
            at: Instant::now(),
            run_tasks: true,
        })
        .unwrap();
        let first = page.snapshot(true, true, None);
        assert_eq!(first.title, "Worker");
        assert_eq!(first.frame.as_ref().map(BrowserFrame::width), Some(320));
        page.apply(PageCommand::Cursor(10.0, 10.0)).unwrap();
        page.apply(PageCommand::Wheel(0.0, 80.0)).unwrap();
        page.apply(PageCommand::Find {
            action: "start",
            query: "needle".into(),
        })
        .unwrap();
        let update = page.snapshot(false, false, None);
        assert_eq!(update.find.unwrap().match_count, 1);
        assert!(update.demand.needs_paint);
    }

    #[test]
    fn submillisecond_inputs_preserve_elapsed_page_time() {
        let mut page = owner();
        let start = page.clock_at;
        assert_eq!(
            page.elapsed_clock_ms(start + std::time::Duration::from_micros(500)),
            0
        );
        assert_eq!(
            page.elapsed_clock_ms(start + std::time::Duration::from_micros(1000)),
            1
        );
        assert_eq!(page.elapsed_clock_ms(start), 0);
    }

    #[test]
    fn old_queued_frame_timestamp_never_rewinds_input_clock() {
        let mut page = owner();
        page.apply(PageCommand::Viewport(16, 16)).unwrap();
        let queued_at = page.clock_at;
        page.advance_clock(queued_at + std::time::Duration::from_millis(20));
        let after_input = page.clock_at;
        page.frame(queued_at, false).unwrap();
        assert!(page.clock_at >= after_input);
    }
    #[test]
    fn document_replacement_discards_find_summary_before_the_next_frame() {
        let mut page = owner();
        page.apply(PageCommand::Navigate(data_url("<p>needle</p>")))
            .unwrap();
        page.apply(PageCommand::Find {
            action: "start",
            query: "needle".into(),
        })
        .unwrap();
        assert_eq!(
            page.snapshot(false, true, None).find.unwrap().match_count,
            1
        );
        page.apply(PageCommand::Navigate(data_url("<p>replacement</p>")))
            .unwrap();
        assert!(page.snapshot(false, true, None).find.is_none());
        page.apply(PageCommand::Find {
            action: "start",
            query: "replacement".into(),
        })
        .unwrap();
        assert_eq!(
            page.snapshot(false, false, None).find.unwrap().match_count,
            1
        );
    }
}

#[cfg(test)]
mod native_wheel_tests {
    use super::*;
    use std::time::Duration;
    use winit::event::{DeviceId, TouchPhase};

    fn wheel() -> NativeWheel {
        NativeWheel {
            delta: (0.0, 40.0),
            device: DeviceId::dummy(),
            cursor: (4.0, 5.0),
            modifiers: InputModifiers::default(),
            pixel_delta: false,
            phase: TouchPhase::Moved,
            document: 7,
            received: Instant::now(),
        }
    }

    fn queued(wheel: NativeWheel, sequence: u64) -> QueuedCommand {
        let queued_at = wheel.received;
        QueuedCommand {
            command: PageCommand::NativeWheel(wheel),
            input: Some(QueuedInput {
                sequence,
                last_sequence: sequence,
                source_count: 1,
                kind: "wheel",
                queued_at,
            }),
        }
    }

    #[test]
    fn native_wheel_merge_preserves_trace_and_fifo_barriers() {
        let (tx, rx) = mpsc::channel();
        let first = wheel();
        let oldest = first.received;
        tx.send(queued(first.clone(), 1)).unwrap();
        let mut second = first.clone();
        second.received += Duration::from_millis(15);
        tx.send(queued(second, 2)).unwrap();
        tx.send(QueuedCommand {
            command: PageCommand::Modifiers(InputModifiers::default()),
            input: None,
        })
        .unwrap();
        tx.send(queued(first, 3)).unwrap();
        let mut pending = None;
        let merged = next_command(&rx, &mut pending, 7).unwrap();
        assert!(matches!(
            merged.command,
            PageCommand::NativeWheel(NativeWheel {
                delta: (0.0, 80.0),
                ..
            })
        ));
        let trace = merged.input.unwrap();
        assert_eq!(
            (trace.sequence, trace.last_sequence, trace.source_count),
            (1, 2, 2)
        );
        assert_eq!(trace.queued_at, oldest);
        assert!(matches!(
            next_command(&rx, &mut pending, 7).unwrap().command,
            PageCommand::Modifiers(_)
        ));
        assert_eq!(
            next_command(&rx, &mut pending, 7)
                .unwrap()
                .input
                .unwrap()
                .sequence,
            3
        );
    }

    #[test]
    fn native_wheel_batch_is_bounded_without_tracing_and_preserves_remaining_fifo() {
        let (tx, rx) = mpsc::channel();
        let first = wheel();
        for _ in 0..130 {
            tx.send(QueuedCommand {
                command: PageCommand::NativeWheel(first.clone()),
                input: None,
            })
            .unwrap();
        }
        tx.send(QueuedCommand {
            command: PageCommand::Cursor(11.0, 12.0),
            input: None,
        })
        .unwrap();
        tx.send(queued(first, 131)).unwrap();
        let mut pending = None;
        assert!(matches!(
            next_command(&rx, &mut pending, 7).unwrap().command,
            PageCommand::NativeWheel(NativeWheel {
                delta: (0.0, 5120.0),
                ..
            })
        ));
        assert!(matches!(
            next_command(&rx, &mut pending, 7).unwrap().command,
            PageCommand::NativeWheel(NativeWheel {
                delta: (0.0, 80.0),
                ..
            })
        ));
        assert!(matches!(
            next_command(&rx, &mut pending, 7).unwrap().command,
            PageCommand::Cursor(11.0, 12.0)
        ));
        assert_eq!(
            next_command(&rx, &mut pending, 7)
                .unwrap()
                .input
                .unwrap()
                .sequence,
            131
        );
        let mut phase = wheel();
        phase.delta = (0.0, 0.0);
        phase.phase = TouchPhase::Ended;
        assert_eq!(
            PageCommand::NativeWheel(phase).input_kind(),
            Some("wheel_phase")
        );
    }

    #[test]
    fn native_wheel_rejects_changed_context_phase_sign_and_overflow() {
        let first = wheel();
        for change in 0..12 {
            let mut next = first.clone();
            match change {
                0 => next.delta.1 = -40.0,
                1 => next.delta.0 = 1.0,
                2 => next.phase = TouchPhase::Started,
                3 => next.phase = TouchPhase::Ended,
                4 => next.phase = TouchPhase::Cancelled,
                5 => next.document += 1,
                6 => next.cursor.0 += 1.0,
                7 => next.modifiers.control = true,
                8 => next.pixel_delta = true,
                9 => next.received += Duration::from_millis(51),
                10 => next.delta.1 = f64::INFINITY,
                _ => next.received -= Duration::from_millis(1),
            }
            let mut current = first.clone();
            assert!(!current.merge(&next), "case {change}");
            assert_eq!(current.delta, first.delta);
        }
        let mut huge = first.clone();
        huge.delta.1 = f64::MAX;
        assert!(!huge.clone().merge(&huge));
        for phase in [
            TouchPhase::Started,
            TouchPhase::Ended,
            TouchPhase::Cancelled,
        ] {
            let mut boundary = first.clone();
            boundary.phase = phase;
            assert!(!boundary.merge(&first));
        }
    }

    #[test]
    fn stale_native_and_direct_wheels_are_dispatched_individually() {
        let (tx, rx) = mpsc::channel();
        tx.send(queued(wheel(), 1)).unwrap();
        tx.send(queued(wheel(), 2)).unwrap();
        tx.send(QueuedCommand {
            command: PageCommand::Wheel(0.0, 40.0),
            input: None,
        })
        .unwrap();
        let mut pending = None;
        for sequence in [1, 2] {
            assert_eq!(
                next_command(&rx, &mut pending, 8)
                    .unwrap()
                    .input
                    .unwrap()
                    .sequence,
                sequence
            );
        }
        assert!(matches!(
            next_command(&rx, &mut pending, 8).unwrap().command,
            PageCommand::Wheel(0.0, 40.0)
        ));
    }
}
