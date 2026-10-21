//! The GUI page owner. Boa and DOM handles are created and used on this thread.
use std::cell::Cell;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use omoikane::cdp::{CdpSession, RenderDemand};
use omoikane::css::MediaEnvironment;
use omoikane::error_reporting::{ErrorReporter, ExecutionSurface};
use omoikane::frame::{BrowserFrame, BrowserFrameCache, RetainedScrollScene};
use omoikane::js::native_scroll::{
    AcceptedScroll, Bounds, Fallback, NativeScrollHandle, Offset, OwnerSnapshot, PublicationGate,
    PublicationSequence, Scene, TargetKey,
};
use omoikane::js::{FindInPageResult, FullscreenTransition, PointerLockTransition};
use omoikane::platform_input::{
    InputModifiers, NativeScrollTarget, PlatformImeEvent, PlatformInput, PlatformKeyEvent,
    PlatformMouseButton, PlatformTouchPhase,
};
use serde_json::json;
use winit::event_loop::EventLoopProxy;

const DEFAULT_GUI_RESOURCE_WORKERS: usize = 4;
const MAX_GUI_RESOURCE_WORKERS: usize = 64;
const MAX_NATIVE_SCROLL_DROP_DIAGNOSTICS: u64 = 16;

/// Shared visited-link revocation is provided by the v50 storage observer.
/// It revokes every registered live handle before a new visited key becomes
/// visible, so the native-only publication token covers visited-style reads.
fn shared_visited_native_scroll_ready() -> bool {
    true
}

fn configured_resource_worker_count(value: Option<&str>) -> NonZeroUsize {
    value
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|workers| (1..=MAX_GUI_RESOURCE_WORKERS).contains(workers))
        .and_then(NonZeroUsize::new)
        .unwrap_or(NonZeroUsize::new(DEFAULT_GUI_RESOURCE_WORKERS).unwrap())
}

/// Native scrolling is enabled for the normal GUI unless the emergency
/// opt-out is the exact value `0`.
fn configured_native_scroll(value: Option<&std::ffi::OsStr>) -> bool {
    value != Some(std::ffi::OsStr::new("0"))
}

fn gui_resource_worker_count() -> NonZeroUsize {
    configured_resource_worker_count(std::env::var("OMOIKANE_RESOURCE_WORKERS").ok().as_deref())
}

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
    NativeScrollAccepted(NativeScrollAccepted),
    NativeScrollFallback(NativeScrollFallback),
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

/// Owned native receipt metadata. GUI-accepted wheels remain unmerged: winit
/// does not expose a separate momentum identity on macOS.
#[derive(Clone)]
pub(super) struct NativeWheel {
    pub(super) gui_sequence: u64,
    pub(super) gui_sequences: Vec<u64>,
    pub(super) gui_sequence_last: u64,
    pub(super) gui_source_count: u64,
    pub(super) gui_sequences_truncated: bool,
    pub(super) delta: (f64, f64),
    pub(super) device: winit::event::DeviceId,
    pub(super) cursor: (f64, f64),
    pub(super) modifiers: InputModifiers,
    pub(super) buttons: u8,
    pub(super) pixel_delta: bool,
    pub(super) phase: winit::event::TouchPhase,
    pub(super) document: u64,
    pub(super) received: Instant,
}

/// One GUI-accepted default and its exact native receipt metadata.
pub(super) struct NativeScrollAccepted {
    pub(super) input: AcceptedScroll,
    pub(super) pre_default_offset: Offset,
    pub(super) target: NativeScrollTarget,
    pub(super) wheel: NativeWheel,
}

/// One ordered canonical fallback and its exact native receipt metadata.
pub(super) struct NativeScrollFallback {
    pub(super) fallback: Fallback,
    pub(super) wheel: NativeWheel,
}

/// Trusted authority published with one owned retained scene.
#[derive(Clone)]
pub(super) struct NativeScrollPublication {
    pub(super) publication_sequence: u64,
    pub(super) owner_default_sequence: u64,
    /// Diagnostic notification progress; it grants no scene authority.
    pub(super) owner_notified_sequence: u64,
    pub(super) handle: NativeScrollHandle,
    pub(super) key: TargetKey,
    pub(super) target: NativeScrollTarget,
    pub(super) policy_epoch: u64,
    pub(super) acceptance_epoch: u64,
    pub(super) document: u64,
    pub(super) paint_revision: u64,
    pub(super) pending_scene: Scene,
}

/// Numeric scene inputs retained by the page owner after pixels move to GUI.
///
/// This contains no raster, DOM, Realm, resolver, or borrowed state. It permits
/// trusted target policy to be revalidated against an unchanged frame without
/// cloning the retained pixel payload back from the GUI.
#[derive(Clone, Copy)]
struct NativeScrollSceneState {
    document: u64,
    paint_revision: u64,
    owner: OwnerSnapshot,
    offset: Offset,
    extent: Bounds,
    coverage: Bounds,
}

impl NativeScrollSceneState {
    fn from_scene(scene: &RetainedScrollScene, owner: OwnerSnapshot) -> Self {
        let coverage = scene.coverage();
        Self {
            document: scene.document_generation(),
            paint_revision: scene.paint_revision(),
            owner,
            offset: Offset {
                x: scene.current_scroll().0,
                y: scene.current_scroll().1,
            },
            extent: Bounds {
                min: Offset::default(),
                max: Offset {
                    x: scene.extent().0,
                    y: scene.extent().1,
                },
            },
            coverage: Bounds {
                min: Offset {
                    x: coverage.0.0,
                    y: coverage.0.1,
                },
                max: Offset {
                    x: coverage.1.0,
                    y: coverage.1.1,
                },
            },
        }
    }
}

impl NativeWheel {
    pub(super) fn is_phase_barrier(&self) -> bool {
        self.delta == (0.0, 0.0)
    }

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
            || self.buttons != next.buttons
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
        // next_command bounds each canonical batch to 128 receipts. Keep
        // every trace identity without changing the existing merge decision.
        let available = 128usize.saturating_sub(self.gui_sequences.len());
        self.gui_sequences
            .extend(next.gui_sequences.iter().take(available).copied());
        self.gui_source_count = self.gui_source_count.saturating_add(next.gui_source_count);
        self.gui_sequence_last = next.gui_sequence_last;
        self.gui_sequences_truncated |=
            next.gui_sequences_truncated || next.gui_sequences.len() > available;
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
    pub(super) retained_scene: Option<RetainedScrollScene>,
    pub(super) native_scroll: Option<NativeScrollPublication>,
    pub(super) paint_trace: Option<PagePaintTrace>,
    pub(super) demand: RenderDemand,
    pub(super) url: String,
    pub(super) title: String,
    pub(super) document_generation: u64,
    pub(super) viewport: (u32, u32),
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
    native_scroll_outcome: Option<String>,
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
            Self::NativeScrollAccepted(accepted) if accepted.wheel.delta == (0.0, 0.0) => {
                Some("native_scroll_phase")
            }
            Self::NativeScrollFallback(fallback) if fallback.wheel.delta == (0.0, 0.0) => {
                Some("native_scroll_fallback_phase")
            }
            Self::NativeScrollAccepted(_) => Some("native_scroll_accepted"),
            Self::NativeScrollFallback(_) => Some("native_scroll_fallback"),
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

    fn input_received_at(&self) -> Instant {
        match self {
            Self::NativeWheel(wheel) => wheel.received,
            Self::NativeScrollAccepted(accepted) => accepted.wheel.received,
            Self::NativeScrollFallback(fallback) => fallback.wheel.received,
            _ => Instant::now(),
        }
    }

    fn native_scroll_outcome(&self) -> Option<String> {
        match self {
            Self::NativeScrollAccepted(_) => Some("accepted".into()),
            Self::NativeScrollFallback(fallback) => {
                Some(format!("fallback:{:?}", fallback.fallback.reason))
            }
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
                            retained_scene: None,
                            native_scroll: None,
                            paint_trace: None,
                            demand: RenderDemand {
                                needs_paint: false,
                                next: omoikane::cdp::NextRendering::Idle,
                            },
                            url: String::new(),
                            title: super::DEFAULT_WINDOW_TITLE.into(),
                            document_generation: 0,
                            viewport: (0, 0),
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
                    let probe_gui_sequence = match &command {
                        PageCommand::NativeWheel(wheel) => Some(wheel.gui_sequence),
                        PageCommand::NativeScrollAccepted(accepted) => Some(accepted.wheel.gui_sequence),
                        PageCommand::NativeScrollFallback(fallback) => Some(fallback.wheel.gui_sequence),
                        _ => None,
                    };
                    let probe_gui_sequences = input.as_ref().and_then(|_| {
                        let wheel = match &command {
                            PageCommand::NativeWheel(wheel) => wheel,
                            PageCommand::NativeScrollAccepted(accepted) => &accepted.wheel,
                            PageCommand::NativeScrollFallback(fallback) => &fallback.wheel,
                            _ => return None,
                        };
                        Some(json!({"ids":wheel.gui_sequences,"total_count":wheel.gui_source_count,
                            "first":wheel.gui_sequence,"last":wheel.gui_sequence_last,
                            "truncated":wheel.gui_sequences_truncated}))
                    });
                    let result = owner.apply(command);
                    if let (Some(input), Some(queue_ms), Some(started)) = (input, queue_ms, started)
                    {
                        eprintln!("{}", omoikane::diagnostics::json_record("OMOIKANE_PAGE_INPUT", &json!({
                                "sequence": input.sequence, "gui_sequence":probe_gui_sequence, "gui_sequences":probe_gui_sequences, "first_sequence": input.sequence,
                                "last_sequence": input.last_sequence, "source_count": input.source_count,
                                "kind": input.kind,
                                "native_scroll_outcome": input.native_scroll_outcome,
                                "queue_ms": queue_ms,
                                "execute_ms": started.elapsed().as_secs_f64() * 1000.0,
                                "success": result.is_ok(),
                            })));
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
                    native_scroll_outcome: command.native_scroll_outcome(),
                    queued_at: command.input_received_at(),
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
    last_retained_scene: Option<RetainedScrollScene>,
    retained_native_scene: Option<NativeScrollSceneState>,
    last_native_scroll: Option<NativeScrollPublication>,
    last_paint_trace: Option<PagePaintTrace>,
    trace_paint: bool,
    trace_retained_capture: bool,
    find: Option<FindInPageResult>,
    find_document: u64,
    cookie_changes: omoikane::http::CookieChangeSubscription,
    /// A wheel's Document must get a paint attempt before another task turn.
    /// This tracks worker painting, not native surface presentation.
    pending_wheel_paint: Option<u64>,
    deferred_task_turn: bool,
    trace_native_probe: bool,
    native_publication_diagnostic: Option<(u64, &'static str)>,
    native_scroll_requested: bool,
    native_scroll_shared_visited_ready: bool,
    native_scroll_cursor: Option<(f64, f64)>,
    next_native_scroll_publication: Option<u64>,
    native_scroll_dropped_deliveries: u64,
}

impl PageOwner {
    fn new(
        reporter: Option<Arc<ErrorReporter>>,
        cookie_waker: std::task::Waker,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut session = CdpSession::new()?;
        session.set_resource_worker_count(gui_resource_worker_count());
        session.set_task_pump_limit(std::num::NonZeroUsize::new(1));
        session.set_pointer_lock_deferred(true);
        session.set_deferred_document_startup(true);
        session.set_nonblocking_image_loading(true);
        session.set_nonblocking_font_loading(true);
        session.set_nonblocking_script_loading(true);
        session.set_nonblocking_fetch_loading(true);
        session.set_nonblocking_iframe_loading(true);
        let native_scroll_requested =
            configured_native_scroll(std::env::var_os("OMOIKANE_NATIVE_SCROLL").as_deref());
        let trace_retained_capture = std::env::var_os("OMOIKANE_TRACE_RETAINED_SCROLL").is_some();
        // The normal GUI reaches authority only because the v50 shared visited
        // observer revokes every registered handle before style can change.
        session.set_retained_scroll_capture(native_scroll_requested || trace_retained_capture);
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
            last_retained_scene: None,
            retained_native_scene: None,
            last_native_scroll: None,
            last_paint_trace: None,
            trace_paint: std::env::var_os("OMOIKANE_TRACE_PAINT").is_some(),
            trace_retained_capture,
            find: None,
            find_document: clock_document,
            cookie_changes,
            pending_wheel_paint: None,
            deferred_task_turn: false,
            trace_native_probe: std::env::var_os("OMOIKANE_TRACE_INPUT").is_some(),
            native_publication_diagnostic: None,
            native_scroll_requested,
            native_scroll_shared_visited_ready: shared_visited_native_scroll_ready(),
            native_scroll_cursor: None,
            next_native_scroll_publication: Some(1),
            native_scroll_dropped_deliveries: 0,
        })
    }

    fn apply(&mut self, command: PageCommand) -> Result<(), Box<dyn std::error::Error>> {
        use PageCommand::*;
        // Raw fallback-only sampling retains its existing coalescing behavior.
        // GUI-accepted inputs use distinct variants before they enter this
        // queue, so they can never reach this merge/normalization path.
        let current_document = self.session.document_generation();
        let command = match command {
            NativeWheel(wheel) if wheel.document != current_document => return Ok(()),
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
                self.native_scroll_cursor = None;
                self.last_native_scroll = None;
            }
            Viewport(width, height) => {
                self.viewport = (width, height);
                self.session.set_viewport(width, height);
                self.last_native_scroll = None;
            }
            Visibility(hidden) => self.session.set_host_visibility(hidden),
            MediaEnvironment(environment) => {
                self.session.set_host_media_environment(environment)?
            }
            Cursor(x, y) => {
                let target_changed = self.native_scroll_cursor != Some((x, y));
                if target_changed && self.native_scroll_enabled() {
                    self.session.native_scroll_handle().revoke_policy();
                }
                self.input.cursor_moved(&mut self.session, x, y)?;
                self.native_scroll_cursor = Some((x, y));
                if target_changed {
                    self.last_native_scroll = None;
                }
            }
            CursorLeft => {
                if self.native_scroll_enabled() {
                    self.session.native_scroll_handle().revoke_policy();
                }
                self.input.cursor_left(&mut self.session);
                self.native_scroll_cursor = None;
                self.last_native_scroll = None;
            }
            Focus(focused) => self.input.focus_changed(&mut self.session, focused)?,
            Mouse(button, pressed) => {
                self.input
                    .mouse_button(&mut self.session, button, pressed)?
            }
            NativeWheel(_) => unreachable!("native wheel normalized above"),
            NativeScrollAccepted(accepted) => self.apply_native_scroll_accepted(accepted)?,
            NativeScrollFallback(fallback) => self.apply_native_scroll_fallback(fallback)?,
            Wheel(x, y) => {
                // Keep listener/default-action behavior, including mutations
                // before a listener error. Native receipt time may be stale
                // after this command waits behind a long-running page task.
                self.pending_wheel_paint = Some(self.session.document_generation());
                let started = self.trace_paint.then(Instant::now);
                let result = self.input.wheel(&mut self.session, x, y);
                if let Some(started) = started {
                    eprintln!(
                        "{}",
                        omoikane::diagnostics::json_record(
                            "OMOIKANE_WHEEL",
                            &json!({
                                "dispatch_ms":started.elapsed().as_secs_f64()*1000.0,
                                "delta_x":x,"delta_y":y,"success":result.is_ok(),
                            })
                        )
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

    fn native_scroll_enabled(&self) -> bool {
        self.native_scroll_requested && self.native_scroll_shared_visited_ready
    }

    fn retained_capture_interested(&self) -> bool {
        if self.trace_retained_capture {
            return true;
        }
        if !self.native_scroll_enabled()
            || self.native_scroll_cursor.is_none()
            || self.next_native_scroll_publication.is_none()
        {
            return false;
        }
        let handle = self.session.native_scroll_handle();
        let key = handle.publication_gate().key;
        handle.retained_capture_possible(key)
    }

    fn refresh_retained_capture_interest(&mut self) {
        // Tests and embedders may request owned capture without enabling native
        // input. Only the PageOwner-managed product/trace configurations get a
        // transient interest decision here.
        if !self.native_scroll_requested && !self.trace_retained_capture {
            return;
        }
        let interested = self.retained_capture_interested();
        self.session
            .set_retained_scroll_capture_interest(interested);
        if !interested {
            self.last_retained_scene = None;
            self.retained_native_scene = None;
            self.last_native_scroll = None;
        }
    }

    fn apply_native_scroll_accepted(
        &mut self,
        accepted: NativeScrollAccepted,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let NativeScrollAccepted {
            input,
            pre_default_offset,
            target,
            wheel,
        } = accepted;
        let expected_delta = Offset {
            x: wheel.delta.0,
            y: wheel.delta.1,
        };
        let current_document = self.session.document_generation();
        let current_key = self.session.native_scroll_handle().publication_gate().key;
        if wheel.document != current_document
            || target.document as u64 != current_key.scroll_node
            || input.key != current_key
        {
            self.record_native_scroll_drop("stale_accepted", &wheel);
            return Ok(());
        }
        if wheel.is_phase_barrier()
            || target.cursor != wheel.cursor
            || input.delta != expected_delta
            || !input.matches_pre_default_offset(pre_default_offset)
            || target.page
                != (
                    target.client.0 + pre_default_offset.x,
                    target.client.1 + pre_default_offset.y,
                )
        {
            return Err(native_scroll_protocol_error(
                "accepted input metadata no longer identifies the active target",
            ));
        }
        let started = self.trace_paint.then(Instant::now);
        let probe_before = self
            .trace_native_probe
            .then(|| {
                self.session
                    .native_scroll_handle()
                    .owner_sequences(input.key)
                    .ok()
            })
            .flatten();
        let notification = self.session.claim_native_scroll_notification(input)?;
        let probe_claimed = self
            .trace_native_probe
            .then(|| {
                self.session
                    .native_scroll_handle()
                    .owner_sequences(input.key)
                    .ok()
            })
            .flatten();
        self.pending_wheel_paint = Some(wheel.document);
        let result = self.session.dispatch_native_scroll_notification(
            target,
            (notification.input.delta.x, notification.input.delta.y),
            wheel.modifiers,
            wheel.buttons,
        );
        self.trace_native_scroll_delivery("accepted", &wheel, started, result.is_ok());
        if self.trace_native_probe {
            let after = self
                .session
                .native_scroll_handle()
                .owner_sequences(input.key)
                .ok();
            omoikane::diagnostics::input_probe(json!({
                "event":"delivery","decision":"accepted","gui_sequence":wheel.gui_sequence,
                "input_sequence":input.sequence,"success":result.is_ok(),
                "default_before":probe_before.map(|p|p.0), "notified_before":probe_before.map(|p|p.1),
                "default_after":probe_claimed.map(|p|p.0),"notified_after":probe_claimed.map(|p|p.1),
                "key":format!("{:?}",input.key),
                "default_end":after.map(|p|p.0),"notified_end":after.map(|p|p.1),
            }));
        }
        if result.is_err() {
            self.session.native_scroll_handle().revoke_policy();
            self.record_native_scroll_drop("retired_notification_target", &wheel);
        }
        Ok(())
    }

    fn apply_native_scroll_fallback(
        &mut self,
        ordered: NativeScrollFallback,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let NativeScrollFallback { fallback, wheel } = ordered;
        let expected_delta = Offset {
            x: wheel.delta.0,
            y: wheel.delta.1,
        };
        let current_key = self.session.native_scroll_handle().publication_gate().key;
        if wheel.document != self.session.document_generation() || fallback.key != current_key {
            self.record_native_scroll_drop("stale_fallback", &wheel);
            return Ok(());
        }
        if wheel.is_phase_barrier() || fallback.delta != expected_delta {
            return Err(native_scroll_protocol_error(
                "fallback metadata no longer identifies the active Document",
            ));
        }
        let handle = self.session.native_scroll_handle();
        handle
            .fallback_ready(fallback)
            .map_err(|error| native_scroll_rejection("fallback barrier", error))?;
        self.pending_wheel_paint = Some(wheel.document);
        let started = self.trace_paint.then(Instant::now);
        let result = self.input.wheel_at(
            &mut self.session,
            wheel.cursor,
            wheel.modifiers,
            wheel.buttons,
            wheel.delta.0,
            wheel.delta.1,
        );
        self.trace_native_scroll_delivery("fallback", &wheel, started, result.is_ok());
        if self.trace_native_probe {
            omoikane::diagnostics::input_probe(json!({
                "event":"delivery","decision":"fallback","gui_sequence":wheel.gui_sequence,
                "success":result.is_ok(),"reason":format!("{:?}",fallback.reason),
                "ticket":format!("{:?}",fallback.order),
            }));
        }
        result?;
        handle
            .complete_fallback(fallback)
            .map_err(|error| native_scroll_rejection("fallback completion", error))?;
        Ok(())
    }

    fn record_native_scroll_drop(&mut self, reason: &str, wheel: &NativeWheel) {
        self.native_scroll_dropped_deliveries =
            self.native_scroll_dropped_deliveries.saturating_add(1);
        if self.trace_native_probe {
            omoikane::diagnostics::input_probe(json!({
                "event":"drop","gui_sequence":wheel.gui_sequence,"reason":reason,
            }));
        }
        if self.native_scroll_dropped_deliveries <= MAX_NATIVE_SCROLL_DROP_DIAGNOSTICS {
            eprintln!(
                "{}",
                omoikane::diagnostics::json_record(
                    "OMOIKANE_NATIVE_SCROLL_DROP",
                    &json!({
                        "reason":reason,
                        "document":wheel.document,
                        "delta_x":wheel.delta.0,
                        "delta_y":wheel.delta.1,
                        "count":self.native_scroll_dropped_deliveries,
                    })
                )
            );
        }
    }

    fn trace_native_scroll_delivery(
        &self,
        kind: &str,
        wheel: &NativeWheel,
        started: Option<Instant>,
        success: bool,
    ) {
        let Some(started) = started else {
            return;
        };
        eprintln!(
            "{}",
            omoikane::diagnostics::json_record(
                "OMOIKANE_NATIVE_SCROLL_DELIVERY",
                &json!({
                    "kind":kind,
                    "dispatch_ms":started.elapsed().as_secs_f64()*1000.0,
                    "delta_x":wheel.delta.0,"delta_y":wheel.delta.1,
                    "cursor_x":wheel.cursor.0,"cursor_y":wheel.cursor.1,
                    "device":format!("{:?}",wheel.device),
                    "pixel_delta":wheel.pixel_delta,
                    "phase":format!("{:?}",wheel.phase),
                    "document":wheel.document,"success":success,
                })
            )
        );
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
        if run_tasks {
            self.cache
                .advance(&mut self.session, self.viewport.0, self.viewport.1, elapsed)?;
        } else {
            self.session.advance_clock_without_tasks(elapsed);
        }
        self.refresh_retained_capture_interest();
        // Author tasks may synchronously revoke the old epoch. Sample the gate
        // after that task turn and before style/layout/paint/visited reads so the
        // resulting pixels can authorize this fresh epoch in the same frame.
        let native_scroll_gate = self
            .native_scroll_enabled()
            .then(|| self.session.native_scroll_handle().publication_gate());
        let painted = self.cache.paint_if_needed(&mut self.session)?;
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
            let retained_scene = self.session.take_retained_scroll_scene();
            // Capture owner counters immediately after the retained pixels on
            // this owner thread. No script or geometry materialization runs in
            // this interval. Concurrent GUI accepts do not advance
            // `owner.applied`; the GUI-side atomic commit therefore replays them
            // over this exact captured offset instead of attaching a newer ack
            // to old pixels.
            let native_owner = native_scroll_gate.and_then(|gate| {
                self.session
                    .native_scroll_handle()
                    .owner_snapshot(gate.key)
                    .ok()
            });
            self.retained_native_scene = retained_scene
                .as_ref()
                .zip(native_owner)
                .map(|(scene, owner)| NativeScrollSceneState::from_scene(scene, owner));
            self.last_retained_scene = retained_scene;
            if let Some(started) = started {
                self.last_paint_trace = Some(PagePaintTrace {
                    frame: started.elapsed(),
                    timings: omoikane::paint::take_last_render_timings(),
                });
            }
        }
        let publish = painted || native_scroll_gate.is_some_and(|gate| !gate.eligible);
        self.last_native_scroll = match (publish, self.retained_native_scene, native_scroll_gate) {
            (true, Some(scene), Some(gate)) => self.publish_native_scroll_scene(scene, gate),
            (true, None, Some(_)) => {
                self.trace_native_publication_status("missing_retained_scene");
                None
            }
            (true, _, None) => {
                self.trace_native_publication_status("native_scroll_disabled");
                None
            }
            _ => None,
        };
        if startup_pending && !self.session.has_pending_document_startup() {
            self.clock_at = Instant::now();
            self.clock_remainder = std::time::Duration::ZERO;
            self.clock_document = self.session.document_generation();
        }
        Ok(())
    }

    fn publish_native_scroll_scene(
        &mut self,
        scene: NativeScrollSceneState,
        before: PublicationGate,
    ) -> Option<NativeScrollPublication> {
        if !self.native_scroll_enabled() || scene.document != self.session.document_generation() {
            self.trace_native_publication_status("disabled_or_stale_document");
            return None;
        }
        let Some(cursor) = self.native_scroll_cursor else {
            self.trace_native_publication_status("missing_cursor");
            return None;
        };
        let target = match self.session.native_scroll_target_at(cursor.0, cursor.1) {
            Ok(Some(target)) => target,
            Ok(None) => {
                self.trace_native_publication_status("target_policy_rejected");
                return None;
            }
            Err(_) => {
                self.trace_native_publication_status("target_policy_error");
                return None;
            }
        };
        let handle = self.session.native_scroll_handle();
        let after = handle.publication_gate();
        if after.key != before.key
            || after.policy_epoch != before.policy_epoch
            || after.acceptance_epoch != before.acceptance_epoch
        {
            self.trace_native_publication_status("gate_changed_during_target_resolution");
            return None;
        }
        let Some(publication_value) = self.next_native_scroll_publication else {
            self.trace_native_publication_status("publication_sequence_exhausted");
            return None;
        };
        self.next_native_scroll_publication = publication_value.checked_add(1);
        let pending_scene = Scene {
            key: before.key,
            publication: PublicationSequence::new(publication_value),
            policy_epoch: before.policy_epoch,
            acceptance_epoch: before.acceptance_epoch,
            acknowledged: scene.owner.default_applied_sequence,
            completed_fallback: scene.owner.completed_fallback,
            offset: scene.offset,
            extent: scene.extent,
            coverage: scene.coverage,
            eligible: true,
        };
        let final_gate = handle.publication_gate();
        let current = final_gate.key == before.key
            && final_gate.policy_epoch == before.policy_epoch
            && final_gate.acceptance_epoch == before.acceptance_epoch;
        self.trace_native_publication_status(if current {
            "pending_gui_commit"
        } else {
            "gate_changed_before_enqueue"
        });
        current.then_some(NativeScrollPublication {
            publication_sequence: publication_value,
            owner_default_sequence: scene.owner.default_applied_sequence,
            owner_notified_sequence: scene.owner.notification_delivered_sequence,
            handle,
            key: before.key,
            target,
            policy_epoch: before.policy_epoch,
            acceptance_epoch: before.acceptance_epoch,
            document: scene.document,
            paint_revision: scene.paint_revision,
            pending_scene,
        })
    }

    fn trace_native_publication_status(&mut self, reason: &'static str) {
        if !self.trace_native_probe {
            return;
        }
        let document = self.session.document_generation();
        if self.native_publication_diagnostic == Some((document, reason)) {
            return;
        }
        self.native_publication_diagnostic = Some((document, reason));
        let gate = self.session.native_scroll_handle().publication_gate();
        eprintln!(
            "{}",
            omoikane::diagnostics::json_record(
                "OMOIKANE_NATIVE_SCROLL_PUBLICATION",
                &json!({"reason":reason,"document":document,
                    "policy_epoch":gate.policy_epoch,"acceptance_epoch":gate.acceptance_epoch,
                    "eligible":gate.eligible,"cursor":self.native_scroll_cursor}),
            )
        );
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
            retained_scene: self.last_retained_scene.take(),
            native_scroll: self.last_native_scroll.take(),
            paint_trace: self.last_paint_trace.take(),
            demand: self.cache.demand(&self.session),
            url: self.session.current_url().to_owned(),
            title,
            document_generation: self.session.document_generation(),
            viewport: self.viewport,
            fullscreen: self.session.take_fullscreen_transition(),
            pointer_lock: self.session.take_pointer_lock_transition(),
            find: self.find.clone(),
            error,
        }
    }
}

fn native_scroll_protocol_error(message: &str) -> Box<dyn std::error::Error> {
    std::io::Error::other(message.to_string()).into()
}

fn native_scroll_rejection(
    operation: &str,
    rejection: omoikane::js::native_scroll::OwnerRejection,
) -> Box<dyn std::error::Error> {
    std::io::Error::other(format!("native scroll {operation} rejected: {rejection:?}")).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_worker_count_accepts_only_bounded_nonzero_values() {
        for (value, expected) in [
            (None, 4),
            (Some(""), 4),
            (Some("0"), 4),
            (Some("8"), 8),
            (Some("64"), 64),
            (Some("65"), 4),
            (Some("invalid"), 4),
        ] {
            assert_eq!(configured_resource_worker_count(value).get(), expected);
        }
    }

    #[test]
    fn native_scroll_is_default_on_with_exact_zero_opt_out() {
        for (value, expected) in [
            (None, true),
            (Some(std::ffi::OsStr::new("1")), true),
            (Some(std::ffi::OsStr::new("0")), false),
        ] {
            assert_eq!(configured_native_scroll(value), expected);
        }
    }

    #[test]
    fn retained_capture_interest_keeps_recoverable_epochs_and_skips_permanent_blocks() {
        let mut page = owner();
        page.trace_retained_capture = false;
        page.native_scroll_requested = true;
        page.native_scroll_shared_visited_ready = true;
        page.next_native_scroll_publication = Some(1);
        assert!(!page.retained_capture_interested());

        page.native_scroll_cursor = Some((4.0, 4.0));
        let handle = page.session.native_scroll_handle();
        let key = handle.publication_gate().key;
        assert!(page.retained_capture_interested());
        handle.revoke_policy();
        handle.close_acceptance();
        assert!(page.retained_capture_interested());

        page.next_native_scroll_publication = None;
        assert!(!page.retained_capture_interested());
        page.trace_retained_capture = true;
        assert!(page.retained_capture_interested());

        page.trace_retained_capture = false;
        page.next_native_scroll_publication = Some(1);
        handle.revoke_for_blocking_listener();
        assert!(!page.retained_capture_interested());
        assert!(!handle.activate(key));
        assert!(!page.retained_capture_interested());
        let replacement = TargetKey {
            owner_generation: key.owner_generation + 1,
            ..key
        };
        assert!(handle.activate(replacement));
        assert!(page.retained_capture_interested());

        page.native_scroll_requested = false;
        page.trace_retained_capture = true;
        assert!(page.retained_capture_interested());
        assert!(!page.native_scroll_enabled());
    }

    #[test]
    fn first_cursor_rearms_configured_capture_after_a_cursorless_navigation() {
        let mut page = owner();
        page.trace_retained_capture = false;
        page.native_scroll_requested = true;
        page.native_scroll_shared_visited_ready = true;
        page.session.set_deferred_document_startup(false);
        page.session.set_retained_scroll_capture(true);
        page.apply(PageCommand::Viewport(32, 32)).unwrap();
        page.apply(PageCommand::Navigate(data_url(
            "<body style='margin:0;height:192px;background:red'></body>",
        )))
        .unwrap();

        page.frame(Instant::now(), false).unwrap();
        assert!(page.last_retained_scene.is_none());
        assert!(page.last_native_scroll.is_none());

        page.apply(PageCommand::Cursor(8.0, 8.0)).unwrap();
        page.frame(Instant::now(), false).unwrap();
        assert!(page.last_retained_scene.is_some());
        assert!(page.last_native_scroll.is_some());
    }

    #[test]
    fn trace_only_capture_never_publishes_native_input_authority() {
        let mut page = owner();
        page.native_scroll_requested = false;
        page.trace_retained_capture = true;
        page.session.set_deferred_document_startup(false);
        page.session.set_retained_scroll_capture(true);
        page.apply(PageCommand::Viewport(32, 32)).unwrap();
        page.apply(PageCommand::Navigate(data_url(
            "<body style='margin:0;height:192px;background:red'></body>",
        )))
        .unwrap();
        page.apply(PageCommand::Cursor(8.0, 8.0)).unwrap();
        page.frame(Instant::now(), false).unwrap();

        let update = page.snapshot(true, true, None);
        assert!(update.retained_scene.is_some());
        assert!(update.native_scroll.is_none());
        assert!(!page.native_scroll_enabled());
    }

    struct NoWake;
    impl std::task::Wake for NoWake {
        fn wake(self: Arc<Self>) {}
    }

    fn owner() -> PageOwner {
        let mut page = PageOwner::new(None, std::task::Waker::from(Arc::new(NoWake))).unwrap();
        // Individual native-scroll tests opt in explicitly. Keep unrelated
        // unit tests independent from both the product default and host env.
        page.native_scroll_requested = false;
        page.trace_retained_capture = false;
        page.session.set_retained_scroll_capture(false);
        page
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

    fn native_scroll_page() -> (PageOwner, PageUpdate) {
        native_scroll_page_with_viewport((32, 32))
    }

    fn native_scroll_page_with_viewport(viewport: (u32, u32)) -> (PageOwner, PageUpdate) {
        let (page, update) = pending_native_scroll_page_with_viewport(viewport);
        commit_pending_publication(update.native_scroll.as_ref().unwrap());
        (page, update)
    }

    fn pending_native_scroll_page_with_viewport(viewport: (u32, u32)) -> (PageOwner, PageUpdate) {
        let mut page = owner();
        page.native_scroll_requested = true;
        page.native_scroll_shared_visited_ready = true;
        page.session.set_deferred_document_startup(false);
        page.session.set_retained_scroll_capture(true);
        page.apply(PageCommand::Viewport(viewport.0, viewport.1))
            .unwrap();
        page.apply(PageCommand::Navigate(data_url(
            "<body style='margin:0'><div style='height:40px;background:red'></div>\
             <div style='height:1960px;background:blue'></div></body>",
        )))
        .unwrap();
        page.apply(PageCommand::Cursor(8.0, 8.0)).unwrap();
        page.frame(Instant::now(), false).unwrap();
        let update = page.snapshot(true, true, None);
        assert!(update.retained_scene.is_some());
        assert!(
            update.native_scroll.is_some(),
            "native publication missing: {:?}",
            page.session.native_scroll_handle().publication_gate()
        );
        (page, update)
    }

    fn commit_pending_publication(publication: &NativeScrollPublication) {
        assert_eq!(
            publication.handle.commit_scene_publication(
                publication.key,
                publication.policy_epoch,
                publication.acceptance_epoch,
                PublicationSequence::new(publication.publication_sequence),
                publication.owner_default_sequence,
                publication.pending_scene,
                true,
            ),
            omoikane::js::native_scroll::PublicationReconciliation::Current
        );
    }

    fn native_scroll_policy_page(markup: &str) -> PageOwner {
        let mut page = owner();
        page.native_scroll_requested = true;
        page.native_scroll_shared_visited_ready = true;
        page.session.set_deferred_document_startup(false);
        page.session.set_retained_scroll_capture(true);
        page.apply(PageCommand::Viewport(32, 32)).unwrap();
        page.apply(PageCommand::Navigate(data_url(markup))).unwrap();
        page.apply(PageCommand::Cursor(8.0, 8.0)).unwrap();
        page.frame(Instant::now(), false).unwrap();
        page
    }

    fn native_wheel(page: &PageOwner, delta: (f64, f64)) -> NativeWheel {
        NativeWheel {
            gui_sequence: 0,
            gui_sequences: Vec::new(),
            gui_sequence_last: 0,
            gui_source_count: 0,
            gui_sequences_truncated: false,
            delta,
            device: winit::event::DeviceId::dummy(),
            cursor: (8.0, 8.0),
            modifiers: InputModifiers::default(),
            buttons: 0,
            pixel_delta: true,
            phase: winit::event::TouchPhase::Moved,
            document: page.session.document_generation(),
            received: Instant::now(),
        }
    }

    fn accepted_native_command(
        presentation: omoikane::js::native_scroll::AcceptedPresentation,
        mut target: NativeScrollTarget,
        wheel: NativeWheel,
    ) -> NativeScrollAccepted {
        let pre_default_offset = presentation.pre_default_offset();
        target.cursor = wheel.cursor;
        target.client = wheel.cursor;
        target.page = (
            target.client.0 + pre_default_offset.x,
            target.client.1 + pre_default_offset.y,
        );
        NativeScrollAccepted {
            input: presentation.input(),
            pre_default_offset,
            target,
            wheel,
        }
    }

    #[test]
    fn v50_shared_visited_revocation_gates_default_gui_authority() {
        let mut page = owner();
        page.native_scroll_requested = true;
        assert!(shared_visited_native_scroll_ready());
        page.native_scroll_shared_visited_ready = false;
        assert!(!page.native_scroll_enabled());
        page.native_scroll_shared_visited_ready = true;
        assert!(page.native_scroll_shared_visited_ready);
        assert!(page.native_scroll_enabled());
    }

    #[test]
    fn pending_scene_keeps_the_committed_scene_until_gui_commit_and_replays_fifo() {
        let (mut page, mut first) = native_scroll_page();
        let committed = first.native_scroll.take().unwrap();
        let before = committed.handle.publication_gate();
        let retained = page.retained_native_scene.expect("retained numeric scene");

        let pending = page
            .publish_native_scroll_scene(retained, before)
            .expect("pending GUI publication");
        assert_eq!(committed.handle.publication_gate(), before);
        assert!(committed.handle.publication_is_current(
            committed.key,
            committed.policy_epoch,
            committed.acceptance_epoch,
            PublicationSequence::new(committed.publication_sequence),
        ));
        assert!(!pending.handle.publication_is_current(
            pending.key,
            pending.policy_epoch,
            pending.acceptance_epoch,
            PublicationSequence::new(pending.publication_sequence),
        ));

        let accepted = match pending.handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let owner = pending.handle.owner_sequences(pending.key).unwrap();
        let backlog = pending.handle.notification_backlog(pending.key).unwrap();
        commit_pending_publication(&pending);
        assert_eq!(pending.handle.visual_offset(), Offset { x: 0.0, y: 10.0 });
        let rebound = pending
            .handle
            .rebind_presentation_to_publication(
                accepted,
                PublicationSequence::new(pending.publication_sequence),
            )
            .expect("latest accepted raster can be rebound to the committed scene");
        assert!(pending.handle.validate_for_presentation(rebound));
        let current = pending.handle.publication_gate();
        assert!(!pending.handle.validate_for_presentation(accepted));
        assert_eq!(pending.handle.publication_gate(), current);
        assert_eq!(pending.handle.owner_sequences(pending.key), Ok(owner));
        assert_eq!(
            pending.handle.notification_backlog(pending.key),
            Ok(backlog)
        );
        assert!(pending.handle.publication_is_current(
            pending.key,
            pending.policy_epoch,
            pending.acceptance_epoch,
            PublicationSequence::new(pending.publication_sequence),
        ));
    }

    #[test]
    fn identical_cursor_notification_preserves_published_authority() {
        let (mut page, mut update) = native_scroll_page();
        let publication = update.native_scroll.take().unwrap();
        let before = publication.handle.publication_gate();
        page.apply(PageCommand::Cursor(8.0, 8.0)).unwrap();
        let after = publication.handle.publication_gate();
        assert_eq!(
            (after.key, after.policy_epoch),
            (before.key, before.policy_epoch)
        );
        assert!(after.eligible);
        assert!(matches!(
            publication.handle.scroll(Offset { x: 0.0, y: 10.0 }),
            omoikane::js::native_scroll::ScrollDecision::Accepted(_)
        ));
    }

    #[test]
    fn task_mutation_uses_a_fresh_gate_for_the_same_frame_capture() {
        let (mut page, mut first) = native_scroll_page();
        let old = first.native_scroll.take().unwrap();
        let old_epoch = old.handle.publication_gate().policy_epoch;
        evaluate(
            &mut page,
            "setTimeout(()=>document.body.style.background='rgb(0,128,0)',0)",
        );

        page.frame(Instant::now() + std::time::Duration::from_millis(1), true)
            .unwrap();
        let next = page.snapshot(true, false, None);
        let publication = next.native_scroll.expect("fresh publication");
        commit_pending_publication(&publication);
        assert!(next.frame.is_some());
        assert!(next.retained_scene.is_some());
        assert!(publication.policy_epoch > old_epoch);
        assert!(publication.handle.publication_gate().eligible);
    }

    #[test]
    fn policy_only_revoke_republishes_the_unchanged_cached_scene() {
        let (mut page, mut first) = native_scroll_page();
        let old = first.native_scroll.take().unwrap();
        let old_epoch = old.handle.revoke_policy();

        page.frame(Instant::now(), true).unwrap();
        let next = page.snapshot(true, false, None);
        let publication = next.native_scroll.expect("cached scene reauthorization");
        commit_pending_publication(&publication);
        assert!(next.frame.is_none());
        assert!(next.retained_scene.is_none());
        assert_eq!(publication.document, old.document);
        assert_eq!(publication.paint_revision, old.paint_revision);
        assert_eq!(publication.policy_epoch, old_epoch);
        assert!(publication.handle.publication_gate().eligible);
        assert!(matches!(
            publication.handle.scroll(Offset { x: 0.0, y: 10.0 }),
            omoikane::js::native_scroll::ScrollDecision::Accepted(_)
        ));
    }

    #[test]
    fn input_close_rejects_stale_capture_then_republishes_cached_scene() {
        let (mut page, mut first) = native_scroll_page();
        let old = first.native_scroll.take().unwrap();
        let accepted = match old.handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        // First commit the accepted movement and consume its painted frame.
        // Only closing input authority below must then be paintless.
        assert!(
            page.session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_some()
        );
        assert_eq!(old.handle.owner_sequences(old.key), Ok((1, 0)));
        page.frame(Instant::now(), false).unwrap();
        let committed = page.snapshot(true, false, None);
        assert!(committed.frame.is_some());
        assert!(committed.native_scroll.is_some());
        assert!(old.handle.validate_for_presentation(accepted));
        let before = old.handle.publication_gate();
        old.handle.close_acceptance();
        let closed = old.handle.publication_gate();
        assert_eq!(closed.policy_epoch, before.policy_epoch);
        assert_eq!(closed.acceptance_epoch, before.acceptance_epoch + 1);
        assert!(old.handle.validate_for_presentation(accepted));

        let retained = page.retained_native_scene.expect("retained numeric scene");
        assert!(page.publish_native_scroll_scene(retained, before).is_none());
        assert_eq!(old.handle.publication_gate(), closed);
        assert!(old.handle.validate_for_presentation(accepted));

        page.frame(Instant::now(), true).unwrap();
        let next = page.snapshot(true, false, None);
        let publication = next.native_scroll.expect("fresh input generation");
        commit_pending_publication(&publication);
        assert!(next.frame.is_none());
        assert!(next.retained_scene.is_none());
        assert_eq!(publication.policy_epoch, before.policy_epoch);
        assert_eq!(publication.acceptance_epoch, closed.acceptance_epoch);
        let current = publication.handle.publication_gate();
        assert!(publication.owner_default_sequence >= accepted.input().sequence);
        assert!(
            publication
                .handle
                .rebind_presentation_to_publication(
                    accepted,
                    PublicationSequence::new(publication.publication_sequence),
                )
                .is_none()
        );
        assert!(current.eligible);
        assert!(!old.handle.validate_for_presentation(accepted));
        assert_eq!(publication.handle.publication_gate(), current);
        assert_eq!(
            publication.handle.notification_backlog(publication.key),
            Ok(1)
        );
    }

    #[test]
    fn target_query_cannot_attach_a_new_ack_to_old_scene_pixels() {
        let (mut page, mut first) = native_scroll_page();
        let publication = first.native_scroll.take().unwrap();
        let accepted = match publication.handle.scroll(Offset { x: 0.0, y: 40.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let before = publication.handle.publication_gate();
        let captured = page.retained_native_scene.expect("retained numeric scene");
        assert_eq!(captured.owner.default_applied_sequence, 0);

        let republished = page
            .publish_native_scroll_scene(captured, before)
            .expect("safe old-ack publication");
        commit_pending_publication(&republished);

        assert_eq!(
            publication.handle.owner_sequences(publication.key),
            Ok((1, 0)),
            "target lookup must demonstrate owner-side materialization"
        );
        assert_eq!(publication.handle.accepted_barrier(), Some(1));
        assert_eq!(
            publication.handle.visual_offset(),
            Offset { x: 0.0, y: 40.0 }
        );
        let rebound = publication
            .handle
            .rebind_presentation_to_publication(
                accepted,
                PublicationSequence::new(republished.publication_sequence),
            )
            .expect("accepted pixels can be rebound after safe old-ack replay");
        assert!(publication.handle.validate_for_presentation(rebound));
        let current = publication.handle.publication_gate();
        assert!(!publication.handle.validate_for_presentation(accepted));
        assert_eq!(publication.handle.publication_gate(), current);
        assert_eq!(republished.acceptance_epoch, before.acceptance_epoch);
    }

    #[test]
    fn nonoverflowing_auto_ancestors_can_publish_native_root_scroll() {
        for markup in [
            "<body style='margin:0;overflow-y:auto'><div style='height:1000px;background:red'></div></body>",
            "<body style='margin:0;overflow-x:hidden'><div style='height:1000px;background:red'></div></body>",
            "<body style='margin:0'><div style='overflow:auto'><div style='height:1000px;background:red'></div></div></body>",
        ] {
            let mut page = native_scroll_policy_page(markup);
            page.apply(PageCommand::Wheel(0.0, 5.0)).unwrap();
            assert_eq!(evaluate(&mut page, "scrollY"), json!(5));
            assert_eq!(evaluate(&mut page, "document.body.scrollTop"), json!(0));
            page.frame(Instant::now(), false).unwrap();
            let publication = page
                .snapshot(true, false, None)
                .native_scroll
                .expect("a nonoverflowing auto ancestor must not block root scrolling");
            commit_pending_publication(&publication);
            let accepted = match publication.handle.scroll(Offset { x: 0.0, y: 10.0 }) {
                omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
                other => panic!("root native input rejected: {other:?}"),
            };
            let wheel = native_wheel(&page, (0.0, 10.0));
            page.apply(PageCommand::NativeScrollAccepted(accepted_native_command(
                accepted,
                publication.target,
                wheel,
            )))
            .unwrap();
            assert_eq!(evaluate(&mut page, "scrollY"), json!(15));
            assert_eq!(evaluate(&mut page, "document.body.scrollTop"), json!(0));
        }
    }

    #[test]
    fn native_scroll_range_proof_ignores_author_geometry_and_rejects_new_overflow() {
        let mut page = native_scroll_policy_page(
            "<body style='margin:0'><div id='wrap' style='overflow:auto'>\
             <div style='height:1000px;background:red'></div></div></body>",
        );
        assert_eq!(
            evaluate(
                &mut page,
                "typeof __omoikane_native_scroll_axes_have_no_range"
            ),
            json!("undefined")
        );
        evaluate(
            &mut page,
            "globalThis.geometryReads=0;\
             Object.defineProperty(wrap,'scrollHeight',{get(){geometryReads++;throw Error('author getter')}});\
             Object.defineProperty(wrap,'clientHeight',{get(){geometryReads++;throw Error('author getter')}});\
             globalThis.__omoikane_native_scroll_axes_have_no_range=()=>false",
        );
        assert!(
            page.session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_some()
        );
        assert_eq!(evaluate(&mut page, "geometryReads"), json!(0));
        // No pixels or policy changed: the initial publication is still current,
        // and a paintless frame need not emit a duplicate publication.
        let old = page.snapshot(true, false, None).native_scroll.unwrap();
        commit_pending_publication(&old);
        assert!(old.handle.publication_gate().eligible);
        evaluate(
            &mut page,
            "wrap.style.height='32px';\
             globalThis.__omoikane_native_scroll_axes_have_no_range=()=>true",
        );
        assert!(!old.handle.publication_gate().eligible);
        assert!(
            page.session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_none()
        );
        assert_eq!(evaluate(&mut page, "geometryReads"), json!(0));
    }

    #[test]
    fn nonoverflowing_auto_overscroll_boundary_remains_ineligible() {
        let mut page = native_scroll_policy_page(
            "<body style='margin:0;overflow-y:auto;overscroll-behavior-y:none'>\
             <div style='height:1000px;background:red'></div></body>",
        );
        assert!(
            page.session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_none()
        );
        page.apply(PageCommand::Wheel(0.0, 5.0)).unwrap();
        assert_eq!(evaluate(&mut page, "scrollY"), json!(0));
    }

    #[test]
    fn independent_body_scroller_and_root_scroll_snap_are_ineligible() {
        let mut body = native_scroll_policy_page(
            "<body style='margin:0;height:32px;overflow:auto'>\
             <div style='height:1000px;background:red'></div></body>",
        );
        assert!(
            body.session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_none()
        );

        let mut snapped = native_scroll_policy_page(
            "<html style='scroll-snap-type:y mandatory'><body style='margin:0'>\
             <div style='height:1000px;scroll-snap-align:start;background:red'></div></body></html>",
        );
        assert!(
            snapped
                .session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn shadow_and_slotted_targets_stay_on_the_canonical_owner() {
        let mut shadow = native_scroll_policy_page(
            "<body style='margin:0'><div id='host' style='display:block;width:32px;height:32px'></div>\
             <script>host.attachShadow({mode:'open'}).innerHTML=\
             '<div style=\"width:32px;height:32px;background:red\"></div>'</script>\
             <div style='height:1000px'></div></body>",
        );
        assert!(
            shadow
                .session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_none()
        );

        let mut slotted = native_scroll_policy_page(
            "<body style='margin:0'><div id='host' style='display:block;width:32px;height:32px'>\
             <span slot='content' style='display:block;width:32px;height:32px;background:red'></span></div>\
             <script>host.attachShadow({mode:'open'}).innerHTML='<slot name=content></slot>'</script>\
             <div style='height:1000px'></div></body>",
        );
        assert!(
            slotted
                .session
                .native_scroll_target_at(8.0, 8.0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn accepted_line_delta_commits_once_and_notifies_once_in_pixel_mode() {
        let (mut page, mut update) = native_scroll_page();
        let publication = update.native_scroll.take().unwrap();
        let scene = update.retained_scene.take().unwrap();
        let presentation = match publication.handle.scroll(Offset { x: 0.0, y: 40.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let input = presentation.input();
        let visual_offset = presentation.visual_offset();
        assert!(scene.compose((visual_offset.x, visual_offset.y)).is_ok());

        evaluate(
            &mut page,
            "globalThis.nativeEvents=[];document.body.addEventListener('wheel',event=>{\
             nativeEvents.push([scrollY,event.deltaY,event.deltaMode,event.cancelable]);event.preventDefault();\
             },{passive:false})",
        );
        assert!(!publication.handle.validate_for_presentation(presentation));

        // Winit LineDelta has already been normalized to CSS pixels by the GUI.
        let mut wheel = native_wheel(&page, (0.0, 40.0));
        wheel.pixel_delta = false;
        let command = accepted_native_command(presentation, publication.target, wheel.clone());
        page.apply(PageCommand::NativeScrollAccepted(command))
            .unwrap();
        assert_eq!(evaluate(&mut page, "scrollY"), json!(40));
        assert_eq!(
            evaluate(&mut page, "JSON.stringify(nativeEvents)"),
            json!("[[40,40,0,true]]")
        );
        assert_eq!(
            publication.handle.owner_sequences(publication.key),
            Ok((1, 1))
        );
        assert_eq!(
            publication.handle.notification_backlog(publication.key),
            Ok(0)
        );

        let duplicate = accepted_native_command(presentation, publication.target, wheel);
        page.pending_wheel_paint = None;
        assert!(
            page.apply(PageCommand::NativeScrollAccepted(duplicate))
                .is_err()
        );
        assert!(page.pending_wheel_paint.is_none());
        assert_eq!(evaluate(&mut page, "scrollY"), json!(40));
        assert_eq!(evaluate(&mut page, "nativeEvents.length"), json!(1));
    }

    #[test]
    fn transaction_inputs_commit_and_notify_fifo_with_each_pre_default_page_position() {
        // Both defaults must fit the real retained coverage, including 70px.
        let (mut page, mut update) = native_scroll_page_with_viewport((64, 64));
        let publication = update.native_scroll.take().unwrap();
        let first = match publication.handle.scroll(Offset { x: 0.0, y: 40.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let second = match publication.handle.scroll(Offset { x: 0.0, y: 30.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        assert_eq!(first.pre_default_offset(), Offset::default());
        assert_eq!(second.pre_default_offset(), Offset { x: 0.0, y: 40.0 });

        evaluate(
            &mut page,
            "globalThis.transactionEvents=[];document.body.addEventListener('wheel',event=>{\
             transactionEvents.push([scrollY,event.pageY,event.clientY,event.screenX,event.screenY,\
             event.deltaY,event.deltaMode,event.shiftKey,event.buttons]);},{passive:true})",
        );
        let mut first_wheel = native_wheel(&page, (0.0, 40.0));
        first_wheel.phase = winit::event::TouchPhase::Started;
        let mut corrupt = accepted_native_command(first, publication.target, first_wheel.clone());
        corrupt.pre_default_offset.y = 1.0;
        page.pending_wheel_paint = None;
        assert!(
            page.apply(PageCommand::NativeScrollAccepted(corrupt))
                .is_err()
        );
        assert_eq!(
            publication.handle.owner_sequences(publication.key),
            Ok((0, 0))
        );
        assert!(page.pending_wheel_paint.is_none());

        page.apply(PageCommand::NativeScrollAccepted(accepted_native_command(
            first,
            publication.target,
            first_wheel,
        )))
        .unwrap();
        // A geometry read in the first listener materializes both accepted
        // defaults, but delivers only the first notification.
        assert_eq!(
            publication.handle.owner_sequences(publication.key),
            Ok((2, 1))
        );
        assert_eq!(
            evaluate(&mut page, "JSON.stringify(transactionEvents)"),
            json!("[[70,8,8,8,8,40,0,false,0]]")
        );
        let mut second_wheel = native_wheel(&page, (0.0, 30.0));
        second_wheel.cursor = (12.0, 10.0);
        second_wheel.phase = winit::event::TouchPhase::Ended;
        second_wheel.pixel_delta = false;
        second_wheel.modifiers.shift = true;
        second_wheel.buttons = 2;
        page.apply(PageCommand::NativeScrollAccepted(accepted_native_command(
            second,
            publication.target,
            second_wheel,
        )))
        .unwrap();

        assert_eq!(evaluate(&mut page, "scrollY"), json!(70));
        assert_eq!(
            evaluate(&mut page, "JSON.stringify(transactionEvents)"),
            json!("[[70,8,8,8,8,40,0,false,0],[70,50,10,12,10,30,0,true,2]]")
        );
        assert_eq!(
            publication.handle.owner_sequences(publication.key),
            Ok((2, 2))
        );
        assert_eq!(
            publication.handle.notification_backlog(publication.key),
            Ok(0)
        );
    }

    #[test]
    fn ordered_fallback_waits_for_notification_then_runs_canonical_default() {
        let (mut page, mut update) = native_scroll_page();
        let publication = update.native_scroll.take().unwrap();
        let accepted = match publication.handle.scroll(Offset { x: 0.0, y: 20.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let accepted_wheel = native_wheel(&page, (0.0, 20.0));
        page.apply(PageCommand::NativeScrollAccepted(accepted_native_command(
            accepted,
            publication.target,
            accepted_wheel,
        )))
        .unwrap();
        evaluate(
            &mut page,
            "globalThis.fallbackEvents=[];document.body.addEventListener('wheel',event=>\
             fallbackEvents.push([scrollY,event.deltaY]),{passive:true})",
        );
        publication.handle.revoke_policy();
        let fallback = match publication.handle.scroll(Offset { x: 0.0, y: 30.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Fallback(fallback) => fallback,
            decision => panic!("unexpected {decision:?}"),
        };
        let fallback_wheel = native_wheel(&page, (0.0, 30.0));
        page.apply(PageCommand::NativeScrollFallback(NativeScrollFallback {
            fallback,
            wheel: fallback_wheel,
        }))
        .unwrap();
        assert_eq!(evaluate(&mut page, "scrollY"), json!(50));
        assert_eq!(
            evaluate(&mut page, "JSON.stringify(fallbackEvents)"),
            json!("[[20,30]]")
        );
        let snapshot = publication.handle.owner_snapshot(publication.key).unwrap();
        let omoikane::js::native_scroll::FallbackOrder::Ticket(ticket) = fallback.order else {
            panic!("fallback ticket exhausted");
        };
        assert_eq!(snapshot.completed_fallback, ticket);
    }

    #[test]
    fn retired_notification_target_drops_event_without_rolling_back_default() {
        let (mut page, mut update) = native_scroll_page();
        let publication = update.native_scroll.take().unwrap();
        let accepted = match publication.handle.scroll(Offset { x: 0.0, y: 20.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        evaluate(&mut page, "document.body.firstElementChild.remove()");
        let wheel = native_wheel(&page, (0.0, 20.0));
        page.apply(PageCommand::NativeScrollAccepted(accepted_native_command(
            accepted,
            publication.target,
            wheel,
        )))
        .unwrap();
        assert_eq!(evaluate(&mut page, "scrollY"), json!(20));
        assert_eq!(
            publication.handle.owner_sequences(publication.key),
            Ok((1, 1))
        );
        assert_eq!(
            publication.handle.notification_backlog(publication.key),
            Ok(0)
        );
        assert_eq!(
            page.pending_wheel_paint,
            Some(page.session.document_generation())
        );

        let fallback = match publication.handle.scroll(Offset { x: 0.0, y: 30.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Fallback(fallback) => fallback,
            decision => panic!("unexpected {decision:?}"),
        };
        let wheel = native_wheel(&page, (0.0, 30.0));
        page.apply(PageCommand::NativeScrollFallback(NativeScrollFallback {
            fallback,
            wheel,
        }))
        .unwrap();
        assert_eq!(evaluate(&mut page, "scrollY"), json!(50));
    }

    #[test]
    fn stale_document_native_inputs_never_reach_the_replacement_document() {
        let (mut page, mut update) = native_scroll_page();
        let publication = update.native_scroll.take().unwrap();
        let accepted = match publication.handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        publication.handle.revoke_policy();
        let fallback = match publication.handle.scroll(Offset { x: 0.0, y: 20.0 }) {
            omoikane::js::native_scroll::ScrollDecision::Fallback(fallback) => fallback,
            decision => panic!("unexpected {decision:?}"),
        };
        let accepted_wheel = native_wheel(&page, (0.0, 10.0));
        let fallback_wheel = native_wheel(&page, (0.0, 20.0));
        let mut phase = native_wheel(&page, (0.0, 0.0));
        phase.phase = winit::event::TouchPhase::Ended;
        let mut raw = native_wheel(&page, (0.0, 40.0));
        raw.pixel_delta = false;

        page.apply(PageCommand::Navigate(data_url(
            "<body style='margin:0;height:2000px'>replacement</body>",
        )))
        .unwrap();
        page.apply(PageCommand::NativeWheel(raw)).unwrap();
        page.apply(PageCommand::NativeWheel(phase)).unwrap();
        page.apply(PageCommand::NativeScrollAccepted(accepted_native_command(
            accepted,
            publication.target,
            accepted_wheel,
        )))
        .unwrap();
        page.apply(PageCommand::NativeScrollFallback(NativeScrollFallback {
            fallback,
            wheel: fallback_wheel,
        }))
        .unwrap();

        assert_eq!(evaluate(&mut page, "scrollY"), json!(0));
        assert_eq!(page.native_scroll_dropped_deliveries, 2);
        assert!(page.pending_wheel_paint.is_none());
    }

    #[test]
    fn zero_phase_payload_cannot_enter_the_accepted_owner_path() {
        let (mut page, mut update) = native_scroll_page();
        let publication = update.native_scroll.take().unwrap();
        let accepted = match publication.handle.scroll(Offset::default()) {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let mut wheel = native_wheel(&page, (0.0, 0.0));
        wheel.phase = winit::event::TouchPhase::Ended;
        assert!(
            page.apply(PageCommand::NativeScrollAccepted(accepted_native_command(
                accepted,
                publication.target,
                wheel,
            )))
            .is_err()
        );
        assert!(page.pending_wheel_paint.is_none());
        assert_eq!(
            publication.handle.owner_sequences(publication.key),
            Ok((0, 0))
        );
    }

    #[test]
    fn retained_scene_crosses_the_actual_page_update_as_owned_pixels() {
        fn assert_send<T: Send>() {}
        assert_send::<PageUpdate>();
        let mut page = wheel_page();
        page.session.set_retained_scroll_capture(true);
        page.frame(Instant::now(), false).unwrap();
        let update = page.snapshot(true, false, None);
        let frame = update.frame.unwrap();
        let scene = update
            .retained_scene
            .expect("capture must force a cached frame repaint");
        assert!(scene.matches_owner(update.document_generation, scene.paint_revision()));
        assert_eq!(scene.raster_scale(), 1.0);
        assert_eq!(scene.current_scroll(), (0.0, 0.0));
        assert_eq!(scene.compose((0.0, 0.0)).unwrap().pixels(), frame.pixels());
        let fractional = scene.compose((0.0, 16.5)).unwrap();
        assert_eq!((fractional.width(), fractional.height()), (32, 32));
        assert!(page.snapshot(false, false, None).retained_scene.is_none());
        page.apply(PageCommand::Navigate(data_url("<p>replacement</p>")))
            .unwrap();
        assert_ne!(
            page.session.document_generation(),
            scene.document_generation()
        );
        assert!(!scene.matches_owner(page.session.document_generation(), scene.paint_revision()));
    }

    #[test]
    fn retained_capture_configuration_survives_initial_and_replacement_navigation() {
        let mut page = owner();
        // This exercises the lower-level persistent capture setting without
        // PageOwner's cursor-driven product interest management.
        page.native_scroll_requested = false;
        page.trace_retained_capture = false;
        page.session.set_deferred_document_startup(false);
        page.session.set_retained_scroll_capture(true);
        page.apply(PageCommand::Viewport(32, 32)).unwrap();
        let mut old_generation = None;
        for color in ["red", "lime"] {
            page.apply(PageCommand::Navigate(data_url(&format!(
                "<body style='margin:0;height:192px;background:{color}'></body>"
            ))))
            .unwrap();
            page.frame(Instant::now(), false).unwrap();
            let update = page.snapshot(true, true, None);
            let scene = update
                .retained_scene
                .expect("configured capture must survive navigation");
            assert_eq!(
                scene.compose((0.0, 0.0)).unwrap().pixels(),
                update.frame.unwrap().pixels()
            );
            if let Some(old) = old_generation {
                assert_ne!(old, scene.document_generation());
            }
            old_generation = Some(scene.document_generation());
        }
        page.session.set_retained_scroll_capture(false);
        page.apply(PageCommand::Navigate(data_url(
            "<body>capture disabled</body>",
        )))
        .unwrap();
        page.frame(Instant::now(), false).unwrap();
        assert!(page.snapshot(true, true, None).retained_scene.is_none());
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
            gui_sequence: 0,
            gui_sequences: Vec::new(),
            gui_sequence_last: 0,
            gui_source_count: 0,
            gui_sequences_truncated: false,
            delta: (0.0, 40.0),
            device: winit::event::DeviceId::dummy(),
            cursor: (10.0, 10.0),
            modifiers: InputModifiers::default(),
            buttons: 0,
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
            gui_sequence: 0,
            gui_sequences: Vec::new(),
            gui_sequence_last: 0,
            gui_source_count: 0,
            gui_sequences_truncated: false,
            delta: (0.0, 40.0),
            device: DeviceId::dummy(),
            cursor: (4.0, 5.0),
            modifiers: InputModifiers::default(),
            buttons: 0,
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
                native_scroll_outcome: None,
                queued_at,
            }),
        }
    }

    fn accepted(wheel: NativeWheel, sequence: u64) -> QueuedCommand {
        let delta = Offset {
            x: wheel.delta.0,
            y: wheel.delta.1,
        };
        QueuedCommand {
            command: PageCommand::NativeScrollAccepted(NativeScrollAccepted {
                input: AcceptedScroll {
                    key: TargetKey::default(),
                    sequence,
                    delta,
                    visual_offset: delta,
                },
                pre_default_offset: Offset::default(),
                target: NativeScrollTarget {
                    document: 1,
                    node: 2,
                    cursor: wheel.cursor,
                    client: wheel.cursor,
                    page: wheel.cursor,
                },
                wheel,
            }),
            input: None,
        }
    }

    #[test]
    fn only_zero_delta_is_a_phase_barrier() {
        for phase in [
            TouchPhase::Started,
            TouchPhase::Moved,
            TouchPhase::Ended,
            TouchPhase::Cancelled,
        ] {
            let mut nonzero = wheel();
            nonzero.phase = phase;
            assert!(!nonzero.is_phase_barrier());
        }
        let mut zero_terminal = wheel();
        zero_terminal.delta = (0.0, 0.0);
        zero_terminal.phase = TouchPhase::Ended;
        assert!(zero_terminal.is_phase_barrier());
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
                gui_sequence: 0,
                gui_sequences: ref gui_sequences,
                gui_sequence_last: 0,
                gui_source_count: 0,
                gui_sequences_truncated: false,
                delta: (0.0, 80.0),
                ..
            }) if gui_sequences.is_empty()
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
    fn accepted_inputs_never_merge_and_zero_phase_stays_a_raw_barrier() {
        let (tx, rx) = mpsc::channel();
        let mut first = wheel();
        first.pixel_delta = true;
        first.phase = TouchPhase::Started;
        first.modifiers.alt = true;
        first.buttons = 9;
        let expected = first.clone();
        let mut second = first.clone();
        second.delta = (0.0, 20.0);
        let mut phase = first.clone();
        phase.delta = (0.0, 0.0);
        phase.pixel_delta = true;
        phase.phase = TouchPhase::Ended;
        tx.send(accepted(first, 1)).unwrap();
        tx.send(accepted(second, 2)).unwrap();
        tx.send(queued(phase, 3)).unwrap();
        let mut pending = None;

        let first = next_command(&rx, &mut pending, 7).unwrap();
        let PageCommand::NativeScrollAccepted(first) = first.command else {
            panic!("accepted input changed variant");
        };
        assert_eq!(first.wheel.delta, expected.delta);
        assert_eq!(first.wheel.device, expected.device);
        assert_eq!(first.wheel.cursor, expected.cursor);
        assert_eq!(first.wheel.modifiers, expected.modifiers);
        assert_eq!(first.wheel.buttons, expected.buttons);
        assert_eq!(first.wheel.pixel_delta, expected.pixel_delta);
        assert_eq!(first.wheel.phase, expected.phase);
        assert_eq!(first.wheel.document, expected.document);
        let second = next_command(&rx, &mut pending, 7).unwrap();
        assert!(matches!(
            second.command,
            PageCommand::NativeScrollAccepted(NativeScrollAccepted {
                wheel: NativeWheel {
                    gui_sequence: 0,
                    gui_sequences: ref gui_sequences,
                    gui_sequence_last: 0,
                    gui_source_count: 0,
                    gui_sequences_truncated: false,
                    delta: (0.0, 20.0),
                    ..
                },
                ..
            }) if gui_sequences.is_empty()
        ));
        let phase = next_command(&rx, &mut pending, 7).unwrap();
        assert!(matches!(
            phase.command,
            PageCommand::NativeWheel(NativeWheel {
                gui_sequence: 0,
                gui_sequences: ref gui_sequences,
                gui_sequence_last: 0,
                gui_source_count: 0,
                gui_sequences_truncated: false,
                delta: (0.0, 0.0),
                phase: TouchPhase::Ended,
                ..
            }) if gui_sequences.is_empty()
        ));
    }

    #[test]
    fn accepted_and_fallback_traces_keep_the_original_native_receipt() {
        let wheel = wheel();
        let received = wheel.received;
        let accepted = accepted(wheel.clone(), 1).command;
        assert_eq!(accepted.input_received_at(), received);
        assert_eq!(
            accepted.native_scroll_outcome().as_deref(),
            Some("accepted")
        );

        let fallback = PageCommand::NativeScrollFallback(NativeScrollFallback {
            fallback: Fallback {
                key: TargetKey::default(),
                order: omoikane::js::native_scroll::FallbackOrder::Ticket(
                    omoikane::js::native_scroll::FallbackTicket::new(1),
                ),
                reason: omoikane::js::native_scroll::FallbackReason::Ineligible,
                after_sequence: Some(1),
                delta: Offset {
                    x: wheel.delta.0,
                    y: wheel.delta.1,
                },
            },
            wheel,
        });
        assert_eq!(fallback.input_received_at(), received);
        assert_eq!(
            fallback.native_scroll_outcome().as_deref(),
            Some("fallback:Ineligible")
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
                gui_sequence: 0,
                gui_sequences: ref gui_sequences,
                gui_sequence_last: 0,
                gui_source_count: 0,
                gui_sequences_truncated: false,
                delta: (0.0, 5120.0),
                ..
            }) if gui_sequences.is_empty()
        ));
        assert!(matches!(
            next_command(&rx, &mut pending, 7).unwrap().command,
            PageCommand::NativeWheel(NativeWheel {
                gui_sequence: 0,
                gui_sequences: ref gui_sequences,
                gui_sequence_last: 0,
                gui_source_count: 0,
                gui_sequences_truncated: false,
                delta: (0.0, 80.0),
                ..
            }) if gui_sequences.is_empty()
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
        for change in 0..13 {
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
                8 => next.buttons = 1,
                9 => next.pixel_delta = true,
                10 => next.received += Duration::from_millis(51),
                11 => next.delta.1 = f64::INFINITY,
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
