//! Native window owner. Page execution and painting live in `PageWorker`.
use super::chrome_layout::WindowRegion;
use super::page_worker::{
    AsyncEvent, NativeScrollAccepted, NativeScrollFallback, NativeScrollPublication, NativeWheel,
    PageCommand as Command, PagePaintTrace, PageUpdate, PageWorker,
};
use super::toolbar_paint::url_field;
use super::url_bar::{UrlBarKey, UrlBarOutcome, navigation_target};
use super::*;
use omoikane::frame::{BrowserFrame, RetainedScrollScene};
use omoikane::js::native_scroll::{
    AcceptedPresentation, NativeScrollHandle, PublicationReconciliation, PublicationSequence,
    Scene, TargetKey,
};
use omoikane::platform_input::{NativeScrollTarget, PlatformTouchPhase};
use winit::event::TouchPhase;

const NATIVE_WHEEL_TRANSACTION_IDLE: Duration = Duration::from_millis(200);

struct FindBar {
    query: String,
    result: FindInPageResult,
    document: u64,
}

struct NativeComposedFrame {
    gui_sequence: u64,
    frame: BrowserFrame,
    handle: NativeScrollHandle,
    accepted: AcceptedPresentation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativePresentationInstall {
    Installed,
    Superseded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeWheelTransactionId(u64);

impl NativeWheelTransactionId {
    fn matches(self, expected: Self) -> bool {
        self == expected
    }
}

#[derive(Default)]
struct NativeScrollRecovery {
    pending: bool,
}

impl NativeScrollRecovery {
    fn request(&mut self) {
        self.pending = true;
    }

    fn clear(&mut self) {
        self.pending = false;
    }

    fn take_if_ready(&mut self, ready: bool) -> bool {
        if self.pending && ready {
            self.pending = false;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeWheelTransactionTiming {
    Phased { deadline: Instant },
    PhaseLess { deadline: Instant },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeWheelTransactionStartRejection {
    OrphanTerminal,
}

impl NativeWheelTransactionTiming {
    fn start(
        phase: TouchPhase,
        now: Instant,
    ) -> Result<Self, NativeWheelTransactionStartRejection> {
        match phase {
            TouchPhase::Started => Ok(Self::Phased {
                deadline: now + NATIVE_WHEEL_TRANSACTION_IDLE,
            }),
            TouchPhase::Moved => Ok(Self::PhaseLess {
                deadline: now + NATIVE_WHEEL_TRANSACTION_IDLE,
            }),
            TouchPhase::Ended | TouchPhase::Cancelled => {
                Err(NativeWheelTransactionStartRejection::OrphanTerminal)
            }
        }
    }

    fn accepts_followup(self, phase: TouchPhase) -> bool {
        !matches!(phase, TouchPhase::Started)
    }

    fn note_followup(&mut self, phase: TouchPhase, now: Instant) {
        if phase != TouchPhase::Moved {
            return;
        }
        let next = now + NATIVE_WHEEL_TRANSACTION_IDLE;
        match self {
            Self::Phased { deadline } | Self::PhaseLess { deadline } => *deadline = next,
        }
    }

    fn closes_after(self, phase: TouchPhase) -> bool {
        matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled)
    }

    fn deadline(self) -> Option<Instant> {
        match self {
            Self::Phased { deadline } | Self::PhaseLess { deadline } => Some(deadline),
        }
    }
}

struct NativeWheelTransaction {
    id: NativeWheelTransactionId,
    device: DeviceId,
    handle: NativeScrollHandle,
    key: TargetKey,
    policy_epoch: u64,
    acceptance_epoch: u64,
    document: u64,
    target: NativeScrollTarget,
    timing: NativeWheelTransactionTiming,
}

impl NativeWheelTransaction {
    fn matches(&self, wheel: &NativeWheel, publication: &NativeScrollPublication) -> bool {
        self.device == wheel.device
            && self.key == publication.key
            && self.policy_epoch == publication.policy_epoch
            && self.acceptance_epoch == publication.acceptance_epoch
            && self.document == wheel.document
            && self.document == publication.document
    }

    fn matches_publication(&self, publication: &NativeScrollPublication) -> bool {
        self.key == publication.key
            && self.policy_epoch == publication.policy_epoch
            && self.acceptance_epoch == publication.acceptance_epoch
            && self.document == publication.document
    }
}

fn native_scroll_event_target(
    mut latched: NativeScrollTarget,
    cursor: (f64, f64),
    pre_default: omoikane::js::native_scroll::Offset,
) -> NativeScrollTarget {
    latched.cursor = cursor;
    latched.client = cursor;
    latched.page = (
        latched.client.0 + pre_default.x,
        latched.client.1 + pre_default.y,
    );
    latched
}

fn expire_native_scroll_transaction_state(
    transaction: &mut Option<NativeWheelTransaction>,
    requires_fresh_scene: &mut bool,
    recovery: &mut NativeScrollRecovery,
    now: Instant,
) {
    let expired = transaction
        .as_ref()
        .and_then(|transaction| transaction.timing.deadline())
        .is_some_and(|deadline| now >= deadline);
    if expired {
        let transaction = transaction.take().unwrap();
        transaction.handle.close_acceptance();
        *requires_fresh_scene = true;
        recovery.request();
    }
}

/// Retires an ambiguous phase restart before it can select another retained target.
///
/// `MouseWheel` has no portable gesture identifier and some winit backends do
/// not guarantee a terminal phase. An overlapping `Started` therefore leaves
/// target continuity unknowable and must recover through the canonical FIFO.
fn retire_overlapping_native_scroll_start(
    phase: TouchPhase,
    transaction: &mut Option<NativeWheelTransaction>,
    requires_fresh_scene: &mut bool,
    recovery: &mut NativeScrollRecovery,
) -> bool {
    if phase != TouchPhase::Started {
        return false;
    }
    let Some(transaction) = transaction.take() else {
        return false;
    };
    transaction.handle.close_acceptance();
    *requires_fresh_scene = true;
    recovery.request();
    true
}

fn reject_unmatched_native_scroll_barrier(
    phase: TouchPhase,
    transaction: &mut Option<NativeWheelTransaction>,
    publication_handle: Option<&NativeScrollHandle>,
    requires_fresh_scene: &mut bool,
    recovery: &mut NativeScrollRecovery,
) {
    let terminal = matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled);
    if transaction.is_none() && !terminal {
        return;
    }
    let transaction_handle = transaction.take().map(|transaction| transaction.handle);
    let had_authority = transaction_handle.is_some() || publication_handle.is_some();
    if let Some(handle) = publication_handle {
        handle.close_acceptance();
    }
    if let Some(handle) = transaction_handle
        && publication_handle.is_none_or(|publication| !handle.is_same_handle(publication))
    {
        handle.close_acceptance();
    }
    if had_authority {
        *requires_fresh_scene = true;
        recovery.request();
    }
}

fn pending_scene_matches_retained_geometry(
    pending: Scene,
    current: (f64, f64),
    extent: (f64, f64),
    coverage: ((f64, f64), (f64, f64)),
) -> bool {
    use omoikane::js::native_scroll::{Bounds, Offset};

    pending.offset
        == (Offset {
            x: current.0,
            y: current.1,
        })
        && pending.extent
            == (Bounds {
                min: Offset::default(),
                max: Offset {
                    x: extent.0,
                    y: extent.1,
                },
            })
        && pending.coverage
            == (Bounds {
                min: Offset {
                    x: coverage.0.0,
                    y: coverage.0.1,
                },
                max: Offset {
                    x: coverage.1.0,
                    y: coverage.1.1,
                },
            })
}

fn retained_scene_matches_publication(
    scene: &RetainedScrollScene,
    publication: &NativeScrollPublication,
    document: u64,
    viewport_matches: bool,
) -> bool {
    // Raster scale is validated by RetainedScrollScene's compositor boundary;
    // Scene has no raster-scale field, so it is deliberately not duplicated.
    viewport_matches
        && publication.document == document
        && scene.matches_owner(publication.document, publication.paint_revision)
        && pending_scene_matches_retained_geometry(
            publication.pending_scene,
            scene.current_scroll(),
            scene.extent(),
            scene.coverage(),
        )
}

fn commit_native_scroll_publication<T>(
    publication: &NativeScrollPublication,
    retained_association_matches: bool,
    native_frame: &mut Option<T>,
    requires_fresh_scene: &mut bool,
    recovery: &mut NativeScrollRecovery,
) -> PublicationReconciliation {
    let disposition = publication.handle.commit_scene_publication(
        publication.key,
        publication.policy_epoch,
        publication.acceptance_epoch,
        PublicationSequence::new(publication.publication_sequence),
        publication.owner_default_sequence,
        publication.pending_scene,
        retained_association_matches,
    );
    match disposition {
        PublicationReconciliation::Current => recovery.clear(),
        PublicationReconciliation::StaleUpdate => {}
        PublicationReconciliation::StaleAcceptance => {
            *requires_fresh_scene = true;
            recovery.request();
        }
        PublicationReconciliation::InvalidCurrentAuthority => *native_frame = None,
    }
    disposition
}

fn apply_native_scroll_publication<P, T>(
    disposition: PublicationReconciliation,
    publication: P,
    current: &mut Option<P>,
    transaction: &mut Option<T>,
    requires_fresh_scene: &mut bool,
    transaction_matches: impl FnOnce(&T, &P) -> bool,
    publication_requires_fresh_scene: impl FnOnce(&P) -> bool,
) -> bool {
    match disposition {
        PublicationReconciliation::StaleUpdate => return false,
        PublicationReconciliation::StaleAcceptance => {
            *transaction = None;
            *requires_fresh_scene = true;
            return false;
        }
        PublicationReconciliation::Current | PublicationReconciliation::InvalidCurrentAuthority => {
        }
    }
    let next = (disposition == PublicationReconciliation::Current).then_some(publication);
    let transaction_stays_current = transaction.as_ref().is_none_or(|transaction| {
        next.as_ref()
            .is_some_and(|publication| transaction_matches(transaction, publication))
    });
    if !transaction_stays_current {
        *transaction = None;
    }
    *requires_fresh_scene = next.as_ref().is_none_or(publication_requires_fresh_scene);
    let installed = next.is_some();
    *current = next;
    installed
}

fn replace_canonical_frame<C, N>(
    canonical_frame: &mut Option<C>,
    frame: C,
    native_frame: &mut Option<N>,
    preserve_native_frame: bool,
) {
    *canonical_frame = Some(frame);
    if !preserve_native_frame {
        *native_frame = None;
    }
}

fn stage_visual_bundle<C, R, T>(
    canonical_frame: &mut Option<C>,
    retained_scene: &mut Option<R>,
    paint_trace: &mut Option<T>,
    pending: (C, Option<R>, Option<T>),
) -> (Option<C>, Option<R>, Option<T>) {
    let (frame, retained, trace) = pending;
    (
        canonical_frame.replace(frame),
        std::mem::replace(retained_scene, retained),
        std::mem::replace(paint_trace, trace),
    )
}

fn resolve_staged_visual_bundle<C, R, T>(
    disposition: PublicationReconciliation,
    canonical_frame: &mut Option<C>,
    retained_scene: &mut Option<R>,
    paint_trace: &mut Option<T>,
    previous: Option<(Option<C>, Option<R>, Option<T>)>,
) -> bool {
    match disposition {
        PublicationReconciliation::Current => true,
        PublicationReconciliation::StaleUpdate | PublicationReconciliation::StaleAcceptance => {
            if let Some((frame, retained, trace)) = previous {
                *canonical_frame = frame;
                *retained_scene = retained;
                *paint_trace = trace;
            }
            false
        }
        PublicationReconciliation::InvalidCurrentAuthority => {
            // The normal frame remains canonical, but its untrusted retained
            // association cannot back native composition or cached authority.
            *retained_scene = None;
            false
        }
    }
}

fn publication_acknowledges_presentation(
    publication: &NativeScrollPublication,
    presentation: AcceptedPresentation,
) -> bool {
    publication.owner_default_sequence >= presentation.input().sequence
}

fn reconcile_native_scroll_composition_failure<T>(
    publication: &NativeScrollPublication,
    native_frame: &mut Option<T>,
    requires_fresh_scene: &mut bool,
    recovery: &mut NativeScrollRecovery,
) -> PublicationReconciliation {
    let disposition = publication.handle.reconcile_publication(
        publication.key,
        publication.policy_epoch,
        publication.acceptance_epoch,
        PublicationSequence::new(publication.publication_sequence),
        false,
    );
    if disposition == PublicationReconciliation::InvalidCurrentAuthority {
        *native_frame = None;
        *requires_fresh_scene = true;
        recovery.request();
    }
    debug_assert_ne!(disposition, PublicationReconciliation::Current);
    disposition
}

/// Only native resources and owned presentation snapshots remain here.
pub(super) struct AsyncBrowserApp {
    worker: PageWorker,
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    frame: Option<BrowserFrame>,
    native_frame: Option<NativeComposedFrame>,
    retained_scene: Option<RetainedScrollScene>,
    native_scroll: Option<NativeScrollPublication>,
    native_scroll_transaction: Option<NativeWheelTransaction>,
    next_native_scroll_transaction: Option<u64>,
    native_scroll_requires_fresh_scene: bool,
    native_scroll_recovery: NativeScrollRecovery,
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
            native_frame: None,
            retained_scene: None,
            native_scroll: None,
            native_scroll_transaction: None,
            next_native_scroll_transaction: Some(1),
            native_scroll_requires_fresh_scene: true,
            native_scroll_recovery: NativeScrollRecovery::default(),
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
                "{}",
                omoikane::diagnostics::json_record(
                    "OMOIKANE_STARTUP",
                    &json!({"phase":phase,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
                )
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

    fn queue_native_scroll_recovery(&mut self) {
        let ready = self.initial_url.is_none() && !self.page_busy;
        if self.native_scroll_recovery.take_if_ready(ready) {
            // v54 republishes a policy-only revocation from the retained
            // numeric scene even when this frame produces no new pixels.
            self.queue_frame(false);
        }
    }

    fn request_native_scroll_recovery(&mut self) {
        self.native_scroll_recovery.request();
        self.queue_native_scroll_recovery();
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
        let retire_native_scroll = update.error.is_some() || update.pointer_lock.is_some();
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
            self.revoke_native_scroll_before_page_work();
            self.retained_scene = None;
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
        let viewport_matches = update.viewport == self.layout().page_viewport();
        let retained_association_matches = update.native_scroll.as_ref().map(|publication| {
            if update.frame.is_some() {
                update.retained_scene.as_ref().is_some_and(|scene| {
                    retained_scene_matches_publication(
                        scene,
                        publication,
                        self.document,
                        viewport_matches,
                    )
                })
            } else {
                self.retained_scene.as_ref().is_some_and(|scene| {
                    retained_scene_matches_publication(scene, publication, self.document, true)
                })
            }
        });
        let native_scroll = update.native_scroll;
        let pending_visual_bundle = update.frame.map(|frame| {
            let retained_scene = update
                .retained_scene
                .filter(|scene| viewport_matches && scene.document_generation() == self.document);
            (frame, retained_scene, update.paint_trace)
        });
        if let Some(publication) = native_scroll {
            // Stage the owned pixels before making the shared Scene eligible.
            // A rejected or superseded publication restores the whole prior
            // bundle before this non-reentrant GUI turn can present it.
            let previous_visual_bundle = pending_visual_bundle.map(|pending| {
                stage_visual_bundle(
                    &mut self.frame,
                    &mut self.retained_scene,
                    &mut self.pending_paint_trace,
                    pending,
                )
            });
            let mut publication_disposition = commit_native_scroll_publication(
                &publication,
                retained_association_matches.expect("publication association"),
                &mut self.native_frame,
                &mut self.native_scroll_requires_fresh_scene,
                &mut self.native_scroll_recovery,
            );
            let visual_bundle_is_current = resolve_staged_visual_bundle(
                publication_disposition,
                &mut self.frame,
                &mut self.retained_scene,
                &mut self.pending_paint_trace,
                previous_visual_bundle,
            );
            if visual_bundle_is_current {
                publication_disposition =
                    self.recompose_native_frame_after_scene_commit(&publication);
            }
            let installed = apply_native_scroll_publication(
                publication_disposition,
                publication,
                &mut self.native_scroll,
                &mut self.native_scroll_transaction,
                &mut self.native_scroll_requires_fresh_scene,
                |transaction, publication| transaction.matches_publication(publication),
                |publication| {
                    publication
                        .handle
                        .notification_backlog(publication.key)
                        .map_or(true, |pending| pending != 0)
                },
            );
            if self.trace_input && installed {
                if let Some(publication) = self.native_scroll.as_ref() {
                    omoikane::diagnostics::input_probe(json!({
                        "event":"publication","eligible":true,
                        "document":publication.document,"publication_sequence":publication.publication_sequence,
                        "key":format!("{:?}",publication.key),
                        "policy_epoch":publication.policy_epoch,"acceptance_epoch":publication.acceptance_epoch,
                        "paint_revision":publication.paint_revision,
                        "owner_default":publication.owner_default_sequence,
                        "owner_notified":publication.owner_notified_sequence,
                    }));
                }
            }
        } else {
            if let Some((frame, retained_scene, paint_trace)) = pending_visual_bundle {
                replace_canonical_frame(&mut self.frame, frame, &mut self.native_frame, true);
                self.retained_scene = retained_scene;
                self.pending_paint_trace = paint_trace;
            }
            if !self.native_scroll_authority_is_current() {
                let preserve_native_frame = self
                    .native_frame
                    .as_ref()
                    .is_some_and(|native| native.handle.validate_for_presentation(native.accepted));
                self.clear_native_scroll_acceptance();
                if !preserve_native_frame {
                    self.native_frame = None;
                }
            }
        }
        if retire_native_scroll {
            self.revoke_native_scroll_before_page_work();
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
        self.next_frame = if update.deferred_task_turn {
            // A wheel paint can leave scroll events and timers unprocessed even
            // when the render demand is Idle. Schedule the owed task turn once
            // pointer input has been quiet long enough.
            self.last_pointer_input_at
                .map_or(now, |last| now.max(last + POINTER_TASK_QUIET_PERIOD))
        } else if update.demand.needs_paint {
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
        self.queue_native_scroll_recovery();
    }

    fn native_scroll_authority_is_current(&self) -> bool {
        self.native_scroll
            .as_ref()
            .is_some_and(|publication| self.native_scroll_publication_is_current(publication))
    }

    fn native_scroll_publication_is_current(&self, publication: &NativeScrollPublication) -> bool {
        let scene_matches = self.retained_scene.as_ref().is_some_and(|scene| {
            scene.matches_owner(publication.document, publication.paint_revision)
        });
        publication.document == self.document
            && scene_matches
            && publication.handle.publication_is_current(
                publication.key,
                publication.policy_epoch,
                publication.acceptance_epoch,
                PublicationSequence::new(publication.publication_sequence),
            )
    }

    fn recompose_native_frame_after_scene_commit(
        &mut self,
        publication: &NativeScrollPublication,
    ) -> PublicationReconciliation {
        let Some(mut native) = self.native_frame.take() else {
            return PublicationReconciliation::Current;
        };
        if !native.handle.is_same_handle(&publication.handle) {
            return PublicationReconciliation::Current;
        }
        if publication_acknowledges_presentation(publication, native.accepted) {
            // P2 canonical pixels already contain this default. The immutable
            // input remains queued independently for exactly-once notification.
            return PublicationReconciliation::Current;
        }
        let Some(rebound) = publication.handle.rebind_presentation_to_publication(
            native.accepted,
            PublicationSequence::new(publication.publication_sequence),
        ) else {
            // The old raster belonged to a superseded policy/publication. Its
            // absence does not invalidate the newly committed P2 authority.
            return PublicationReconciliation::Current;
        };
        let visual_offset = rebound.visual_offset();
        let composed = self
            .retained_scene
            .as_ref()
            .and_then(|scene| scene.compose((visual_offset.x, visual_offset.y)).ok());
        let Some(frame) = composed else {
            return reconcile_native_scroll_composition_failure(
                publication,
                &mut self.native_frame,
                &mut self.native_scroll_requires_fresh_scene,
                &mut self.native_scroll_recovery,
            );
        };
        if !native.handle.validate_for_presentation(rebound) {
            self.native_scroll_requires_fresh_scene = true;
            self.native_scroll_recovery.request();
            return PublicationReconciliation::InvalidCurrentAuthority;
        }
        native.frame = frame;
        native.accepted = rebound;
        self.native_frame = Some(native);
        PublicationReconciliation::Current
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
                self.send_observable_page_command(Command::PointerLockReply {
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
        let rows = page.y as usize * page.width as usize
            ..(page.y + page.height) as usize * page.width as usize;
        let visible_frame = self
            .native_frame
            .as_ref()
            .map(|native| &native.frame)
            .or(self.frame.as_ref());
        if let Some(frame) = visible_frame.filter(|_| page.height > 0) {
            blit_scaled(
                frame.pixels(),
                (frame.width(), frame.height()),
                &mut target[rows.clone()],
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
        // The shared owner can revoke or replace this token while the native
        // buffer is being built. Validate at the actual presentation boundary,
        // and replace rejected composed pixels with the last canonical frame.
        let native_valid = self
            .native_frame
            .as_ref()
            .is_none_or(|native| native.handle.validate_for_presentation(native.accepted));
        if !native_valid {
            self.native_frame = None;
            target[rows.clone()].fill(0x00ff_ffff);
            if let Some(frame) = self.frame.as_ref().filter(|_| page.height > 0) {
                blit_scaled(
                    frame.pixels(),
                    (frame.width(), frame.height()),
                    &mut target[rows],
                    (page.width, page.height),
                    layout.scale(),
                );
            }
        }
        if let Err(error) = target.present() {
            eprintln!("surface present failed: {error}");
            return;
        }
        if self.trace_input {
            if let Some(native) = self.native_frame.as_ref() {
                let input = native.accepted.input();
                let visual_offset = native.accepted.visual_offset();
                omoikane::diagnostics::input_probe(json!({
                    "event":"present","gui_sequence":native.gui_sequence,
                    "input_sequence":input.sequence,"success":native_valid,
                    "visual_offset":[visual_offset.x,visual_offset.y],
                }));
            }
        }
        if let (Some(started), Some(trace)) = (started, self.pending_paint_trace.take()) {
            self.paint_sequence += 1;
            eprintln!(
                "{}",
                omoikane::diagnostics::json_record(
                    "OMOIKANE_PAINT",
                    &json!({
                        "sequence":self.paint_sequence,
                        "elapsed_ms":self.trace_paint_started.map_or(0,|origin|origin.elapsed().as_millis()),
                        "frame_ms":trace.frame.as_secs_f64()*1000.0,
                        "adjusted_layout_ms":trace.timings.layout.as_secs_f64()*1000.0,
                        "paint_ms":trace.timings.paint.as_secs_f64()*1000.0,
                        "present_ms":started.elapsed().as_secs_f64()*1000.0,
                        "scale":layout.scale().factor(),
                        "viewport":layout.page_viewport(),
                    })
                )
            );
        }
        if self.frame.is_some() && !self.initial_navigation_pending && self.first_page_redraw {
            self.trace_startup("first_page_redraw_attempted");
            self.first_page_redraw = false;
        }
    }

    fn send_viewport(&mut self) {
        self.revoke_native_scroll_before_page_work();
        self.retained_scene = None;
        let (width, height) = self.layout().page_viewport();
        self.worker.send(Command::Viewport(width, height));
    }

    fn revoke_native_scroll_before_page_work(&mut self) {
        let handle = self
            .native_scroll
            .as_ref()
            .map(|publication| publication.handle.clone())
            .or_else(|| {
                self.native_scroll_transaction
                    .as_ref()
                    .map(|transaction| transaction.handle.clone())
            })
            .or_else(|| {
                self.native_frame
                    .as_ref()
                    .map(|native| native.handle.clone())
            });
        if let Some(handle) = handle {
            handle.revoke_policy();
        }
        self.clear_native_scroll_acceptance();
        self.native_frame = None;
    }

    fn clear_native_scroll_acceptance(&mut self) {
        self.native_scroll = None;
        self.native_scroll_transaction = None;
        self.native_scroll_requires_fresh_scene = true;
    }

    fn send_observable_page_command(&mut self, command: Command) {
        self.revoke_native_scroll_before_page_work();
        self.worker.send(command);
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
        self.send_observable_page_command(Command::MediaEnvironment(environment));
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
        let unchanged = self.cursor == (x, y) && self.cursor_in_page;
        self.cursor = (x, y);
        let inside = self.pointer_targets_page();
        if inside {
            let (x, y) = self.layout().page_point(x, y);
            if unchanged {
                self.worker.send(Command::Cursor(x, y));
            } else {
                self.send_observable_page_command(Command::Cursor(x, y));
            }
        } else if self.cursor_in_page {
            self.send_observable_page_command(Command::CursorLeft);
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
                self.send_observable_page_command(Command::Mouse(button, false));
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
            self.send_observable_page_command(Command::Mouse(button, true));
        }
    }

    fn focus_url(&mut self) {
        if self.find.take().is_some() {
            self.send_observable_page_command(Command::Find {
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
        self.send_observable_page_command(Command::Key(PlatformKeyEvent {
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
            Ok(url) => {
                self.revoke_native_scroll_before_page_work();
                self.retained_scene = None;
                self.worker.send(Command::Navigate(url.to_owned()));
            }
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
        self.send_observable_page_command(Command::Find { action, query });
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
                self.send_observable_page_command(Command::CursorLeft);
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    // PlatformInput clears its state on blur as well.
                    self.modifiers = InputModifiers::default();
                    self.chrome_key_releases.clear();
                    self.page_buttons.clear();
                    self.unlock_pointer();
                }
                self.send_observable_page_command(Command::Focus(focused));
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
                self.send_observable_page_command(Command::Find {
                    action: "start",
                    query: self.find.as_ref().unwrap().query.clone(),
                });
            }
            WindowEvent::Ime(event) if self.find.is_none() => {
                self.send_observable_page_command(Command::Ime(platform_ime_event(event)))
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
                let page_cursor = self.layout().page_point(self.cursor.0, self.cursor.1);
                let gui_sequence = if self.trace_input {
                    self.input_sequence = self.input_sequence.saturating_add(1);
                    self.input_sequence
                } else {
                    0
                };
                let wheel = NativeWheel {
                    gui_sequence,
                    gui_sequences: if self.trace_input {
                        vec![gui_sequence]
                    } else {
                        Vec::new()
                    },
                    gui_sequence_last: gui_sequence,
                    gui_source_count: u64::from(self.trace_input),
                    gui_sequences_truncated: false,
                    delta: (x, y),
                    device: device_id,
                    cursor: page_cursor,
                    modifiers: self.modifiers,
                    buttons: self.page_button_mask(),
                    pixel_delta: matches!(delta, winit::event::MouseScrollDelta::PixelDelta(_)),
                    phase,
                    document: self.document,
                    received: now,
                };
                if self.trace_input {
                    omoikane::diagnostics::input_probe(json!({
                        "event":"receipt", "gui_sequence":wheel.gui_sequence,
                        "nonzero":!wheel.is_phase_barrier(), "document":wheel.document,
                        "delta":[wheel.delta.0,wheel.delta.1],
                    }));
                }
                if wheel.is_phase_barrier() {
                    self.handle_native_scroll_phase_barrier(&wheel, now);
                    self.worker.send(Command::NativeWheel(wheel));
                    self.queue_native_scroll_recovery();
                    return;
                }
                if self.try_native_scroll(&wheel) {
                    return;
                }
                if self.trace_input {
                    omoikane::diagnostics::input_probe(json!({
                        "event":"decision", "gui_sequence":wheel.gui_sequence,
                        "nonzero":true, "decision":"canonical", "reason":"no_publication",
                    }));
                }
                self.worker.send(Command::NativeWheel(wheel));
            }
        }
    }

    fn next_native_scroll_transaction_id(&mut self) -> Option<NativeWheelTransactionId> {
        let sequence = self.next_native_scroll_transaction?;
        self.next_native_scroll_transaction = sequence.checked_add(1);
        Some(NativeWheelTransactionId(sequence))
    }

    fn expire_native_scroll_transaction(&mut self, now: Instant) {
        expire_native_scroll_transaction_state(
            &mut self.native_scroll_transaction,
            &mut self.native_scroll_requires_fresh_scene,
            &mut self.native_scroll_recovery,
            now,
        );
    }

    fn close_native_scroll_transaction(&mut self, expected: NativeWheelTransactionId) -> bool {
        let matches = self
            .native_scroll_transaction
            .as_ref()
            .is_some_and(|transaction| transaction.id.matches(expected));
        if matches {
            self.native_scroll_transaction = None;
        }
        matches
    }

    fn latch_native_scroll_transaction(
        &mut self,
        wheel: &NativeWheel,
        publication: &NativeScrollPublication,
        now: Instant,
    ) -> Option<NativeWheelTransactionId> {
        if retire_overlapping_native_scroll_start(
            wheel.phase,
            &mut self.native_scroll_transaction,
            &mut self.native_scroll_requires_fresh_scene,
            &mut self.native_scroll_recovery,
        ) {
            return None;
        }
        if self.native_scroll_transaction.is_some() {
            let continues = self
                .native_scroll_transaction
                .as_ref()
                .is_some_and(|transaction| {
                    transaction.matches(wheel, publication)
                        && transaction.timing.accepts_followup(wheel.phase)
                });
            if !continues {
                self.native_scroll_transaction = None;
                self.native_scroll_requires_fresh_scene = true;
                return None;
            }
            let transaction = self.native_scroll_transaction.as_mut().unwrap();
            transaction.timing.note_followup(wheel.phase, now);
            return Some(transaction.id);
        }
        if self.native_scroll_requires_fresh_scene || wheel.cursor != publication.target.cursor {
            return None;
        }
        let Ok(timing) = NativeWheelTransactionTiming::start(wheel.phase, now) else {
            self.native_scroll_requires_fresh_scene = true;
            return None;
        };
        let Some(id) = self.next_native_scroll_transaction_id() else {
            self.native_scroll_requires_fresh_scene = true;
            return None;
        };
        self.native_scroll_transaction = Some(NativeWheelTransaction {
            id,
            device: wheel.device,
            handle: publication.handle.clone(),
            key: publication.key,
            policy_epoch: publication.policy_epoch,
            acceptance_epoch: publication.acceptance_epoch,
            document: publication.document,
            target: publication.target,
            timing,
        });
        Some(id)
    }

    fn handle_native_scroll_phase_barrier(&mut self, wheel: &NativeWheel, now: Instant) {
        self.expire_native_scroll_transaction(now);
        let Some(publication) = self.native_scroll.clone() else {
            reject_unmatched_native_scroll_barrier(
                wheel.phase,
                &mut self.native_scroll_transaction,
                None,
                &mut self.native_scroll_requires_fresh_scene,
                &mut self.native_scroll_recovery,
            );
            self.native_scroll_requires_fresh_scene = true;
            return;
        };
        if wheel.phase == TouchPhase::Started {
            if wheel.document != publication.document
                || !self.native_scroll_publication_is_current(&publication)
                || self
                    .latch_native_scroll_transaction(wheel, &publication, now)
                    .is_none()
            {
                publication.handle.close_acceptance();
                self.native_scroll_recovery.request();
            }
            return;
        }
        let matches = self
            .native_scroll_transaction
            .as_ref()
            .is_some_and(|transaction| transaction.matches(wheel, &publication))
            && self.native_scroll_publication_is_current(&publication);
        if !matches {
            reject_unmatched_native_scroll_barrier(
                wheel.phase,
                &mut self.native_scroll_transaction,
                Some(&publication.handle),
                &mut self.native_scroll_requires_fresh_scene,
                &mut self.native_scroll_recovery,
            );
            return;
        }
        let transaction = self.native_scroll_transaction.as_mut().unwrap();
        transaction.timing.note_followup(wheel.phase, now);
        let id = transaction.id;
        let closes = transaction.timing.closes_after(wheel.phase);
        if closes {
            if self.close_native_scroll_transaction(id) {
                publication.handle.close_acceptance();
                self.native_scroll_requires_fresh_scene = true;
                self.native_scroll_recovery.request();
            }
        }
    }

    fn accepted_native_scroll_target(
        &self,
        id: NativeWheelTransactionId,
        wheel: &NativeWheel,
        pre_default: omoikane::js::native_scroll::Offset,
    ) -> Option<NativeScrollTarget> {
        let target = self
            .native_scroll_transaction
            .as_ref()
            .filter(|transaction| transaction.id == id)
            .map(|transaction| transaction.target)?;
        Some(native_scroll_event_target(
            target,
            wheel.cursor,
            pre_default,
        ))
    }

    fn install_native_scroll_presentation(
        &mut self,
        publication: &NativeScrollPublication,
        presentation: AcceptedPresentation,
        gui_sequence: u64,
    ) -> NativePresentationInstall {
        let visual_offset = presentation.visual_offset();
        let composed = self
            .retained_scene
            .as_ref()
            .and_then(|scene| scene.compose((visual_offset.x, visual_offset.y)).ok());
        let Some(frame) = composed else {
            return match reconcile_native_scroll_composition_failure(
                publication,
                &mut self.native_frame,
                &mut self.native_scroll_requires_fresh_scene,
                &mut self.native_scroll_recovery,
            ) {
                PublicationReconciliation::StaleUpdate
                | PublicationReconciliation::StaleAcceptance => {
                    NativePresentationInstall::Superseded
                }
                PublicationReconciliation::Current
                | PublicationReconciliation::InvalidCurrentAuthority => {
                    NativePresentationInstall::Failed
                }
            };
        };
        // Composition owns no protocol lock. Revalidate after it and again at
        // the actual surface-present boundary inside `present`.
        if !publication.handle.validate_for_presentation(presentation) {
            return NativePresentationInstall::Failed;
        }
        self.native_frame = Some(NativeComposedFrame {
            gui_sequence,
            frame,
            handle: publication.handle.clone(),
            accepted: presentation,
        });
        self.present();
        if self.native_frame.is_some() {
            NativePresentationInstall::Installed
        } else {
            NativePresentationInstall::Failed
        }
    }

    fn try_native_scroll(&mut self, wheel: &NativeWheel) -> bool {
        let Some(publication) = self.native_scroll.clone() else {
            return false;
        };
        if wheel.document != publication.document {
            return false;
        }
        let now = Instant::now();
        self.expire_native_scroll_transaction(now);
        let current = self.native_scroll_publication_is_current(&publication);
        let requires_fresh = self.native_scroll_requires_fresh_scene;
        let transaction = current
            .then(|| self.latch_native_scroll_transaction(wheel, &publication, now))
            .flatten();
        self.trace_native_scroll_precheck(
            wheel,
            &publication,
            current,
            requires_fresh,
            transaction.is_some(),
        );
        if transaction.is_none() {
            self.native_scroll_transaction = None;
            self.native_scroll_requires_fresh_scene = true;
            publication.handle.close_acceptance();
        }
        let delta = omoikane::js::native_scroll::Offset {
            x: wheel.delta.0,
            y: wheel.delta.1,
        };
        let decision = if transaction.is_some() {
            let Some(decision) = publication.handle.scroll_for_publication(
                publication.key,
                publication.policy_epoch,
                publication.acceptance_epoch,
                PublicationSequence::new(publication.publication_sequence),
                delta,
            ) else {
                self.native_scroll_transaction = None;
                self.native_scroll_requires_fresh_scene = true;
                self.native_scroll_recovery.request();
                return false;
            };
            decision
        } else {
            publication.handle.scroll(delta)
        };
        match decision {
            omoikane::js::native_scroll::ScrollDecision::Accepted(presentation) => {
                let input = presentation.input();
                let pre_default_offset = presentation.pre_default_offset();
                let target = transaction
                    .and_then(|id| {
                        self.accepted_native_scroll_target(id, wheel, pre_default_offset)
                    })
                    .unwrap_or_else(|| {
                        native_scroll_event_target(
                            publication.target,
                            wheel.cursor,
                            pre_default_offset,
                        )
                    });
                self.native_scroll_requires_fresh_scene = true;
                if self.trace_input {
                    self.trace_native_decision(
                        wheel,
                        &publication,
                        "accepted",
                        Some(input.sequence),
                        Some(input.visual_offset),
                        None,
                    );
                }
                self.worker
                    .send(Command::NativeScrollAccepted(NativeScrollAccepted {
                        input,
                        pre_default_offset,
                        target,
                        wheel: wheel.clone(),
                    }));
                let installation = self.install_native_scroll_presentation(
                    &publication,
                    presentation,
                    wheel.gui_sequence,
                );
                if installation != NativePresentationInstall::Installed
                    && let Some(id) = transaction
                {
                    self.close_native_scroll_transaction(id);
                }
                if installation == NativePresentationInstall::Failed {
                    self.request_native_scroll_recovery();
                }
                if let Some(id) = transaction {
                    let closes = self
                        .native_scroll_transaction
                        .as_ref()
                        .filter(|current| current.id == id)
                        .is_some_and(|current| current.timing.closes_after(wheel.phase));
                    if closes {
                        if self.close_native_scroll_transaction(id) {
                            publication.handle.close_acceptance();
                            self.native_scroll_requires_fresh_scene = true;
                            self.request_native_scroll_recovery();
                        }
                    }
                }
            }
            omoikane::js::native_scroll::ScrollDecision::Fallback(fallback) => {
                if self.trace_input {
                    self.trace_native_decision(
                        wheel,
                        &publication,
                        "fallback",
                        None,
                        None,
                        Some(fallback),
                    );
                }
                if let Some(id) = transaction {
                    self.close_native_scroll_transaction(id);
                }
                self.native_scroll_requires_fresh_scene = true;
                self.worker
                    .send(Command::NativeScrollFallback(NativeScrollFallback {
                        fallback,
                        wheel: wheel.clone(),
                    }));
                self.request_native_scroll_recovery();
            }
        }
        true
    }

    fn trace_native_scroll_precheck(
        &self,
        wheel: &NativeWheel,
        publication: &NativeScrollPublication,
        current: bool,
        required_fresh_before_latch: bool,
        latched: bool,
    ) {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if std::env::var_os("OMOIKANE_TRACE_NATIVE_POLICY").is_none()
            || COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 256
        {
            return;
        }
        let gate = publication.handle.publication_gate();
        omoikane::diagnostics::input_probe(json!({
            "event":"precheck", "gui_sequence":wheel.gui_sequence,
            "current":current, "latched":latched,
            "required_fresh_before_latch":required_fresh_before_latch,
            "requires_fresh_after_latch":self.native_scroll_requires_fresh_scene,
            "scene_matches":self.retained_scene.as_ref().is_some_and(|scene|
                scene.matches_owner(publication.document, publication.paint_revision)),
            "cursor_matches":wheel.cursor == publication.target.cursor,
            "wheel_phase":format!("{:?}",wheel.phase),
            "gui_document":self.document, "wheel_document":wheel.document,
            "publication_document":publication.document,
            "cached_key":format!("{:?}",publication.key),
            "live_key":format!("{:?}",gate.key),
            "cached_policy_epoch":publication.policy_epoch,
            "live_policy_epoch":gate.policy_epoch,
            "cached_acceptance_epoch":publication.acceptance_epoch,
            "live_acceptance_epoch":gate.acceptance_epoch,
            "live_eligible":gate.eligible,
            "transaction_id":self.native_scroll_transaction.as_ref().map(|transaction|transaction.id.0),
        }));
    }

    fn trace_native_decision(
        &self,
        wheel: &NativeWheel,
        publication: &NativeScrollPublication,
        decision: &str,
        sequence: Option<u64>,
        offset: Option<omoikane::js::native_scroll::Offset>,
        fallback: Option<omoikane::js::native_scroll::Fallback>,
    ) {
        omoikane::diagnostics::input_probe(json!({
            "event":"decision", "gui_sequence":wheel.gui_sequence, "nonzero":true,
            "decision":decision,"input_sequence":sequence,
            "visual_offset":offset.map(|offset| [offset.x,offset.y]),
            "document":publication.document, "publication_sequence":publication.publication_sequence, "key":format!("{:?}",publication.key),
            "policy_epoch":publication.policy_epoch,"acceptance_epoch":publication.acceptance_epoch,"paint_revision":publication.paint_revision,
            "transaction_id":self.native_scroll_transaction.as_ref().map(|transaction|transaction.id.0),
            "reason":fallback.map(|fallback|format!("{:?}",fallback.reason)),
            "ticket":fallback.map(|fallback|format!("{:?}",fallback.order)),
        }));
    }

    fn page_button_mask(&self) -> u8 {
        self.page_buttons.iter().fold(0, |buttons, button| {
            buttons
                | match button {
                    PlatformMouseButton::Left => 1,
                    PlatformMouseButton::Right => 2,
                    PlatformMouseButton::Middle => 4,
                    PlatformMouseButton::Back => 8,
                    PlatformMouseButton::Forward => 16,
                }
        })
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
            self.send_observable_page_command(Command::Touch(touch.id, phase, x, y));
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
        eprintln!(
            "{}",
            omoikane::diagnostics::json_record("OMOIKANE_INPUT", &record)
        );
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
                self.send_observable_page_command(Command::Visibility(
                    self.occluded || self.minimized,
                ));
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
                    self.send_observable_page_command(Command::FullscreenExited);
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
                self.send_observable_page_command(Command::RelativeMotion(delta.0, delta.1))
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
                    self.send_observable_page_command(Command::Mouse(button, pressed));
                } else if pressed {
                    let delta = match button {
                        4 => Some((0.0, -40.0)),
                        5 => Some((0.0, 40.0)),
                        6 => Some((-40.0, 0.0)),
                        7 => Some((40.0, 0.0)),
                        _ => None,
                    };
                    if let Some((x, y)) = delta {
                        self.send_observable_page_command(Command::Wheel(x, y));
                    }
                }
            }
            DeviceEvent::MouseWheel { delta } if self.raw_buttons => {
                let (x, y) = wheel_delta_css_pixels(delta, self.layout().scale().factor());
                self.send_observable_page_command(Command::Wheel(x, y));
            }
            _ => {}
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.send_observable_page_command(Command::Focus(false));
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
        self.expire_native_scroll_transaction(now);
        self.queue_native_scroll_recovery();
        if self
            .last_pointer_input_at
            .is_some_and(|last| now >= last + POINTER_TASK_QUIET_PERIOD)
            && !self.page_busy
        {
            // Keep the quiet deadline while a paint is in flight. Its update
            // may replace next_frame with Idle before deferred scroll events
            // and page tasks get their turn.
            self.last_pointer_input_at = None;
            self.next_frame = now;
        }
        if now >= self.next_frame && !self.page_busy {
            self.queue_frame(true);
            self.next_frame = now + FRAME_INTERVAL;
        }
        let transaction_deadline = self
            .native_scroll_transaction
            .as_ref()
            .and_then(|transaction| transaction.timing.deadline());
        if self.page_busy {
            if let Some(deadline) = transaction_deadline {
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    deadline.max(now + Duration::from_millis(1)),
                ));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
        } else {
            let wake_at = self
                .last_pointer_input_at
                .map(|last| last + POINTER_TASK_QUIET_PERIOD);
            let mut deadline = wake_at.map_or(self.next_frame, |wake| self.next_frame.min(wake));
            if let Some(transaction_deadline) = transaction_deadline {
                deadline = deadline.min(transaction_deadline);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                deadline.max(now + Duration::from_millis(1)),
            ));
        }
    }
}

#[cfg(test)]
mod native_scroll_gui_contract_tests {
    use super::*;

    const SOURCE: &str = include_str!("async_gui.rs");

    fn body(start: &str, end: &str) -> &'static str {
        let start = SOURCE.find(start).expect("start marker");
        let tail = &SOURCE[start..];
        let end = tail.find(end).expect("end marker");
        &tail[..end]
    }

    fn test_scene(
        handle: &NativeScrollHandle,
        key: TargetKey,
        publication: u64,
        acknowledged: u64,
    ) -> Scene {
        use omoikane::js::native_scroll::{Bounds, FallbackTicket, Offset};
        let gate = handle.publication_gate();
        Scene {
            key,
            publication: PublicationSequence::new(publication),
            policy_epoch: gate.policy_epoch,
            acceptance_epoch: gate.acceptance_epoch,
            acknowledged,
            completed_fallback: FallbackTicket::default(),
            offset: Offset::default(),
            extent: Bounds {
                min: Offset::default(),
                max: Offset { x: 0.0, y: 200.0 },
            },
            coverage: Bounds {
                min: Offset::default(),
                max: Offset { x: 0.0, y: 200.0 },
            },
            eligible: true,
        }
    }

    fn install_native_scroll_scene(handle: &NativeScrollHandle, key: TargetKey, publication: u64) {
        let scene = test_scene(handle, key, publication, 0);
        assert!(handle.authorize_policy(key, scene.policy_epoch));
        assert!(handle.install_scene(scene));
    }

    fn reconcile_native_scroll_publication<T>(
        handle: &NativeScrollHandle,
        expected_key: TargetKey,
        expected_policy_epoch: u64,
        expected_acceptance_epoch: u64,
        expected_publication: u64,
        retained_association_matches: bool,
        native_frame: &mut Option<T>,
        requires_fresh_scene: &mut bool,
        recovery: &mut NativeScrollRecovery,
    ) -> PublicationReconciliation {
        let disposition = handle.reconcile_publication(
            expected_key,
            expected_policy_epoch,
            expected_acceptance_epoch,
            PublicationSequence::new(expected_publication),
            retained_association_matches,
        );
        match disposition {
            PublicationReconciliation::Current => recovery.clear(),
            PublicationReconciliation::StaleUpdate => {}
            PublicationReconciliation::StaleAcceptance => {
                *requires_fresh_scene = true;
                recovery.request();
            }
            PublicationReconciliation::InvalidCurrentAuthority => *native_frame = None,
        }
        disposition
    }

    fn ready_native_scroll_handle() -> (NativeScrollHandle, TargetKey) {
        let key = TargetKey {
            document: 11,
            owner_generation: 12,
            viewport_epoch: 13,
            scroll_node: 14,
        };
        let handle = NativeScrollHandle::new(key);
        install_native_scroll_scene(&handle, key, 1);
        (handle, key)
    }

    fn test_publication(
        handle: &NativeScrollHandle,
        key: TargetKey,
        publication_sequence: u64,
        pending_scene: Scene,
    ) -> NativeScrollPublication {
        NativeScrollPublication {
            publication_sequence,
            owner_default_sequence: pending_scene.acknowledged,
            owner_notified_sequence: 0,
            handle: handle.clone(),
            key,
            target: NativeScrollTarget {
                document: 1,
                node: 2,
                cursor: (0.0, 0.0),
                client: (0.0, 0.0),
                page: (0.0, 0.0),
            },
            policy_epoch: pending_scene.policy_epoch,
            acceptance_epoch: pending_scene.acceptance_epoch,
            document: key.document,
            paint_revision: 41,
            pending_scene,
        }
    }

    fn assert_stale_update_preserves_cached_state(disposition: PublicationReconciliation) {
        let mut cached_publication = Some(20_u8);
        let mut cached_transaction = Some(30_u8);
        let mut requires_fresh_scene = false;
        assert!(!apply_native_scroll_publication(
            disposition,
            10_u8,
            &mut cached_publication,
            &mut cached_transaction,
            &mut requires_fresh_scene,
            |_, _| panic!("stale update must not inspect the live transaction"),
            |_| panic!("stale update must not derive replacement freshness"),
        ));
        assert_eq!(cached_publication, Some(20));
        assert_eq!(cached_transaction, Some(30));
        assert!(!requires_fresh_scene);
    }

    #[test]
    fn phase_barriers_stay_raw_while_normalized_line_wheels_remain_eligible() {
        let native_wheel = body("    fn native_wheel(", "    fn try_native_scroll(");
        assert!(
            native_wheel.find("wheel.is_phase_barrier()").unwrap()
                < native_wheel
                    .find("handle_native_scroll_phase_barrier")
                    .unwrap()
        );
        assert!(
            native_wheel
                .find("handle_native_scroll_phase_barrier")
                .unwrap()
                < native_wheel.find("self.try_native_scroll(&wheel)").unwrap()
        );
        let attempt = body("    fn try_native_scroll(", "    fn page_button_mask(");
        assert!(!attempt.contains("!wheel.pixel_delta"));
        assert!(
            attempt.find("latch_native_scroll_transaction").unwrap()
                < attempt.find(".scroll_for_publication(").unwrap()
        );
        assert!(attempt.contains("presentation.pre_default_offset()"));
        assert!(attempt.contains("accepted_native_scroll_target"));
        let update = body("    fn apply_update(", "    fn update_title(");
        assert!(update.contains("notification_backlog(publication.key)"));
        assert!(update.contains("transaction.matches_publication(publication)"));
        let production = SOURCE.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("native_scroll_inflight"));
    }

    #[test]
    fn phased_timing_processes_terminal_delta_before_closing() {
        let now = Instant::now();
        let mut timing = NativeWheelTransactionTiming::start(TouchPhase::Started, now).unwrap();
        assert_eq!(timing.deadline(), Some(now + NATIVE_WHEEL_TRANSACTION_IDLE));
        assert!(timing.accepts_followup(TouchPhase::Moved));
        let moved = now + Duration::from_millis(50);
        timing.note_followup(TouchPhase::Moved, moved);
        assert_eq!(
            timing.deadline(),
            Some(moved + NATIVE_WHEEL_TRANSACTION_IDLE)
        );
        assert!(!timing.closes_after(TouchPhase::Moved));
        assert!(timing.accepts_followup(TouchPhase::Ended));
        assert!(timing.closes_after(TouchPhase::Ended));
        assert_eq!(
            NativeWheelTransactionTiming::start(TouchPhase::Cancelled, now),
            Err(NativeWheelTransactionStartRejection::OrphanTerminal)
        );
        assert_eq!(
            NativeWheelTransactionTiming::start(TouchPhase::Ended, now),
            Err(NativeWheelTransactionStartRejection::OrphanTerminal)
        );
    }

    #[test]
    fn overlapping_started_retires_old_target_before_retargeting() {
        use omoikane::js::native_scroll::{FallbackReason, Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let input = accepted.input();
        let notification = handle
            .claim_notification_for(
                key,
                input,
                Offset::default(),
                omoikane::js::native_scroll::Bounds {
                    min: Offset::default(),
                    max: Offset { x: 0.0, y: 200.0 },
                },
            )
            .unwrap();
        assert_eq!(notification.offset, input.visual_offset);
        assert_eq!(handle.notification_backlog(key), Ok(0));

        // This models a pre-acceptance publication applied after the owner has
        // claimed the notification. Its live backlog is empty, but its target
        // still predates the accepted visual offset.
        let before = handle.publication_gate();
        let mut transaction = Some(NativeWheelTransaction {
            id: NativeWheelTransactionId(7),
            device: winit::event::DeviceId::dummy(),
            handle: handle.clone(),
            key,
            policy_epoch: before.policy_epoch,
            acceptance_epoch: before.acceptance_epoch,
            document: key.document,
            target: NativeScrollTarget {
                document: usize::try_from(key.document).unwrap(),
                node: 22,
                cursor: (4.0, 5.0),
                client: (4.0, 5.0),
                page: (4.0, 5.0),
            },
            timing: NativeWheelTransactionTiming::Phased {
                deadline: Instant::now() + NATIVE_WHEEL_TRANSACTION_IDLE,
            },
        });
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();

        assert!(retire_overlapping_native_scroll_start(
            TouchPhase::Started,
            &mut transaction,
            &mut requires_fresh_scene,
            &mut recovery,
        ));
        assert!(transaction.is_none());
        assert!(requires_fresh_scene);
        assert!(recovery.pending);
        let closed = handle.publication_gate();
        assert_eq!(closed.policy_epoch, before.policy_epoch);
        assert_eq!(closed.acceptance_epoch, before.acceptance_epoch + 1);
        assert!(!closed.eligible);
        assert!(handle.validate_for_presentation(accepted));
        assert!(matches!(
            handle.scroll(Offset { x: 0.0, y: 10.0 }),
            ScrollDecision::Fallback(fallback)
                if fallback.reason == FallbackReason::Ineligible
        ));

        let latch = body(
            "    fn latch_native_scroll_transaction(",
            "    fn handle_native_scroll_phase_barrier(",
        );
        let retire = latch
            .find("retire_overlapping_native_scroll_start")
            .unwrap();
        let reject = latch[retire..].find("return None").unwrap() + retire;
        let retarget = latch.find("target: publication.target").unwrap();
        assert!(retire < reject && reject < retarget);
    }

    #[test]
    fn orphan_terminal_cannot_start_or_consume_a_transaction_id() {
        let now = Instant::now();
        for phase in [TouchPhase::Ended, TouchPhase::Cancelled] {
            assert_eq!(
                NativeWheelTransactionTiming::start(phase, now),
                Err(NativeWheelTransactionStartRejection::OrphanTerminal)
            );
        }
        let latch = body(
            "    fn latch_native_scroll_transaction(",
            "    fn handle_native_scroll_phase_barrier(",
        );
        assert!(
            latch.find("NativeWheelTransactionTiming::start").unwrap()
                < latch.find("next_native_scroll_transaction_id").unwrap()
        );
        let attempt = body("    fn try_native_scroll(", "    fn page_button_mask(");
        let rejected = attempt.find("if transaction.is_none()").unwrap();
        let close = attempt[rejected..]
            .find("publication.handle.close_acceptance()")
            .unwrap()
            + rejected;
        let scroll = attempt.find("publication.handle.scroll(delta)").unwrap();
        assert!(rejected < close && close < scroll);
    }

    #[test]
    fn orphan_zero_terminal_closes_input_but_preserves_latest_pixels() {
        use omoikane::js::native_scroll::{FallbackReason, Offset, ScrollDecision};

        for phase in [TouchPhase::Ended, TouchPhase::Cancelled] {
            let (handle, _) = ready_native_scroll_handle();
            let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
                ScrollDecision::Accepted(accepted) => accepted,
                decision => panic!("unexpected {decision:?}"),
            };
            let before = handle.publication_gate();
            let mut transaction = None;
            let mut requires_fresh_scene = false;
            let mut recovery = NativeScrollRecovery::default();
            reject_unmatched_native_scroll_barrier(
                phase,
                &mut transaction,
                Some(&handle),
                &mut requires_fresh_scene,
                &mut recovery,
            );
            assert!(transaction.is_none());
            assert!(requires_fresh_scene);
            assert!(recovery.pending);
            let after = handle.publication_gate();
            assert_eq!(
                (after.key, after.policy_epoch),
                (before.key, before.policy_epoch)
            );
            assert_eq!(after.acceptance_epoch, before.acceptance_epoch + 1);
            assert!(!after.eligible);
            assert!(handle.validate_for_presentation(accepted));
            assert!(matches!(
                handle.scroll(Offset { x: 0.0, y: 10.0 }),
                ScrollDecision::Fallback(fallback)
                    if fallback.reason == FallbackReason::Ineligible
            ));
            assert!(handle.validate_for_presentation(accepted));
        }
        let barrier = body(
            "    fn handle_native_scroll_phase_barrier(",
            "    fn accepted_native_scroll_target(",
        );
        assert!(barrier.contains("reject_unmatched_native_scroll_barrier"));
        assert!(barrier.contains("self.close_native_scroll_transaction(id)"));
    }

    #[test]
    fn mismatched_terminal_closes_distinct_transaction_and_publication_handles() {
        let (transaction_handle, key) = ready_native_scroll_handle();
        let (publication_handle, publication_key) = ready_native_scroll_handle();
        assert_eq!(key, publication_key);
        let transaction_pixels = match transaction_handle
            .scroll(omoikane::js::native_scroll::Offset { x: 0.0, y: 10.0 })
        {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let publication_pixels = match publication_handle
            .scroll(omoikane::js::native_scroll::Offset { x: 0.0, y: 10.0 })
        {
            omoikane::js::native_scroll::ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let transaction_epoch = transaction_handle.publication_gate().policy_epoch;
        let publication_epoch = publication_handle.publication_gate().policy_epoch;
        let transaction_acceptance = transaction_handle.publication_gate().acceptance_epoch;
        let publication_acceptance = publication_handle.publication_gate().acceptance_epoch;
        let mut transaction = Some(NativeWheelTransaction {
            id: NativeWheelTransactionId(7),
            device: winit::event::DeviceId::dummy(),
            handle: transaction_handle.clone(),
            key,
            policy_epoch: transaction_handle.publication_gate().policy_epoch,
            acceptance_epoch: transaction_handle.publication_gate().acceptance_epoch,
            document: key.document,
            target: NativeScrollTarget {
                document: usize::try_from(key.document).unwrap(),
                node: 22,
                cursor: (4.0, 5.0),
                client: (4.0, 5.0),
                page: (4.0, 5.0),
            },
            timing: NativeWheelTransactionTiming::Phased {
                deadline: Instant::now() + NATIVE_WHEEL_TRANSACTION_IDLE,
            },
        });
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();

        reject_unmatched_native_scroll_barrier(
            TouchPhase::Ended,
            &mut transaction,
            Some(&publication_handle),
            &mut requires_fresh_scene,
            &mut recovery,
        );

        assert!(transaction.is_none());
        assert!(requires_fresh_scene);
        assert!(recovery.pending);
        assert!(!transaction_handle.publication_gate().eligible);
        assert!(!publication_handle.publication_gate().eligible);
        assert_eq!(
            transaction_handle.publication_gate().policy_epoch,
            transaction_epoch
        );
        assert_eq!(
            publication_handle.publication_gate().policy_epoch,
            publication_epoch
        );
        assert_eq!(
            transaction_handle.publication_gate().acceptance_epoch,
            transaction_acceptance + 1
        );
        assert_eq!(
            publication_handle.publication_gate().acceptance_epoch,
            publication_acceptance + 1
        );
        assert!(transaction_handle.validate_for_presentation(transaction_pixels));
        assert!(publication_handle.validate_for_presentation(publication_pixels));
    }

    #[test]
    fn unmatched_terminal_closes_one_shared_handle_generation_once() {
        let (handle, key) = ready_native_scroll_handle();
        let before = handle.publication_gate().acceptance_epoch;
        let mut transaction = Some(NativeWheelTransaction {
            id: NativeWheelTransactionId(7),
            device: winit::event::DeviceId::dummy(),
            handle: handle.clone(),
            key,
            policy_epoch: handle.publication_gate().policy_epoch,
            acceptance_epoch: before,
            document: key.document,
            target: NativeScrollTarget {
                document: usize::try_from(key.document).unwrap(),
                node: 22,
                cursor: (4.0, 5.0),
                client: (4.0, 5.0),
                page: (4.0, 5.0),
            },
            timing: NativeWheelTransactionTiming::Phased {
                deadline: Instant::now() + NATIVE_WHEEL_TRANSACTION_IDLE,
            },
        });
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();

        reject_unmatched_native_scroll_barrier(
            TouchPhase::Ended,
            &mut transaction,
            Some(&handle),
            &mut requires_fresh_scene,
            &mut recovery,
        );

        assert_eq!(handle.publication_gate().acceptance_epoch, before + 1);
        assert!(!handle.publication_gate().eligible);
        assert!(requires_fresh_scene);
        assert!(recovery.pending);
    }

    #[test]
    fn phase_less_timing_extends_and_matching_terminal_closes_it() {
        let now = Instant::now();
        let mut timing = NativeWheelTransactionTiming::start(TouchPhase::Moved, now).unwrap();
        assert_eq!(timing.deadline(), Some(now + NATIVE_WHEEL_TRANSACTION_IDLE));
        let moved = now + Duration::from_millis(150);
        timing.note_followup(TouchPhase::Moved, moved);
        assert_eq!(
            timing.deadline(),
            Some(moved + NATIVE_WHEEL_TRANSACTION_IDLE)
        );
        assert!(timing.accepts_followup(TouchPhase::Ended));
        assert!(timing.closes_after(TouchPhase::Ended));
        assert!(!timing.accepts_followup(TouchPhase::Started));
    }

    #[test]
    fn transaction_key_and_close_are_identity_checked() {
        let matching = body(
            "impl NativeWheelTransaction {",
            "/// Only native resources and owned presentation snapshots remain here.",
        );
        for field in [
            "self.device == wheel.device",
            "self.key == publication.key",
            "self.policy_epoch == publication.policy_epoch",
            "self.acceptance_epoch == publication.acceptance_epoch",
            "self.document == wheel.document",
        ] {
            assert!(matching.contains(field));
        }
        assert!(!matching.contains("self.target.cursor == wheel.cursor"));
        let close = body(
            "    fn close_native_scroll_transaction(",
            "    fn latch_native_scroll_transaction(",
        );
        assert!(close.contains("transaction.id.matches(expected)"));
        assert!(
            close.find("transaction.id.matches(expected)").unwrap()
                < close.find("self.native_scroll_transaction = None").unwrap()
        );
        let old = NativeWheelTransactionId(7);
        let current = NativeWheelTransactionId(8);
        assert!(!current.matches(old));
        assert!(current.matches(current));
    }

    #[test]
    fn latched_identity_uses_each_receipt_cursor_and_pre_default_page_position() {
        let latched = NativeScrollTarget {
            document: 11,
            node: 22,
            cursor: (1.0, 2.0),
            client: (1.0, 2.0),
            page: (1.0, 2.0),
        };
        let first = native_scroll_event_target(
            latched,
            (4.0, 5.0),
            omoikane::js::native_scroll::Offset { x: 0.0, y: 10.0 },
        );
        let second = native_scroll_event_target(
            latched,
            (8.0, 9.0),
            omoikane::js::native_scroll::Offset { x: 0.0, y: 50.0 },
        );
        assert_eq!((first.document, first.node), (11, 22));
        assert_eq!((second.document, second.node), (11, 22));
        assert_eq!(
            (first.cursor, first.client, first.page),
            ((4.0, 5.0), (4.0, 5.0), (4.0, 15.0))
        );
        assert_eq!(
            (second.cursor, second.client, second.page),
            ((8.0, 9.0), (8.0, 9.0), (8.0, 59.0))
        );
    }

    #[test]
    fn every_transaction_timeout_preserves_pixels_until_recovery() {
        use omoikane::js::native_scroll::{FallbackReason, Offset, ScrollDecision};

        let now = Instant::now();
        let deadline = now + NATIVE_WHEEL_TRANSACTION_IDLE;
        for timing in [
            NativeWheelTransactionTiming::Phased { deadline },
            NativeWheelTransactionTiming::PhaseLess { deadline },
        ] {
            let (handle, key) = ready_native_scroll_handle();
            let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
                ScrollDecision::Accepted(accepted) => accepted,
                decision => panic!("unexpected {decision:?}"),
            };
            let epoch = handle.publication_gate().policy_epoch;
            let acceptance_epoch = handle.publication_gate().acceptance_epoch;
            let mut transaction = Some(NativeWheelTransaction {
                id: NativeWheelTransactionId(7),
                device: winit::event::DeviceId::dummy(),
                handle: handle.clone(),
                key,
                policy_epoch: handle.publication_gate().policy_epoch,
                acceptance_epoch: handle.publication_gate().acceptance_epoch,
                document: key.document,
                target: NativeScrollTarget {
                    document: usize::try_from(key.document).unwrap(),
                    node: 22,
                    cursor: (4.0, 5.0),
                    client: (4.0, 5.0),
                    page: (4.0, 5.0),
                },
                timing,
            });
            let mut requires_fresh_scene = false;
            let mut recovery = NativeScrollRecovery::default();

            expire_native_scroll_transaction_state(
                &mut transaction,
                &mut requires_fresh_scene,
                &mut recovery,
                deadline,
            );
            assert!(transaction.is_none());
            assert!(requires_fresh_scene);
            let closed = handle.publication_gate();
            assert_eq!(closed.policy_epoch, epoch);
            assert_eq!(closed.acceptance_epoch, acceptance_epoch + 1);
            assert!(!closed.eligible);
            assert!(handle.validate_for_presentation(accepted));
            assert!(matches!(
                handle.scroll(Offset { x: 0.0, y: 10.0 }),
                ScrollDecision::Fallback(fallback)
                    if fallback.reason == FallbackReason::Ineligible
            ));
            assert!(handle.validate_for_presentation(accepted));
            assert!(!recovery.take_if_ready(false));
            assert!(recovery.pending);
            assert!(recovery.take_if_ready(true));
            assert!(!recovery.pending);
            assert!(!recovery.take_if_ready(true));
        }
    }

    #[test]
    fn timeout_close_is_connected_to_paintless_republication() {
        let expire = body(
            "fn expire_native_scroll_transaction_state(",
            "fn reject_unmatched_native_scroll_barrier(",
        );
        let close = expire
            .find("transaction.handle.close_acceptance()")
            .unwrap();
        let request = expire.find("recovery.request()").unwrap();
        assert!(close < request);
        let recovery = body(
            "    fn queue_native_scroll_recovery(",
            "    fn request_native_scroll_recovery(",
        );
        assert!(recovery.contains("take_if_ready(ready)"));
        assert!(recovery.contains("self.queue_frame(false)"));
        let update = body(
            "    fn apply_update(",
            "    fn native_scroll_authority_is_current(",
        );
        let owner_idle = update.find("if update.frame_completed").unwrap();
        let queue_recovery = update.rfind("self.queue_native_scroll_recovery()").unwrap();
        assert!(owner_idle < queue_recovery);
        let no_publication = update
            .find("if !self.native_scroll_authority_is_current()")
            .unwrap();
        assert!(no_publication < queue_recovery);
        let revoke = body(
            "    fn revoke_native_scroll_before_page_work(",
            "    fn send_observable_page_command(",
        );
        assert!(revoke.contains("self.native_scroll = None"));
        assert!(!revoke.contains("self.retained_scene = None"));
        assert!(!revoke.contains("native_scroll_recovery.clear"));
        let clear = body(
            "    fn clear_native_scroll_acceptance(",
            "    fn send_observable_page_command(",
        );
        assert!(!clear.contains("revoke_policy"));
        assert!(!clear.contains("self.native_frame = None"));
    }

    #[test]
    fn busy_event_loop_keeps_the_transaction_timeout_wake() {
        let wait = body("    fn about_to_wait(", "}\n\n#[cfg(test)]");
        let deadline = wait.find("transaction.timing.deadline()").unwrap();
        let busy = wait.find("if self.page_busy").unwrap();
        let wake = wait[busy..].find("ControlFlow::WaitUntil").unwrap() + busy;
        assert!(deadline < busy && busy < wake);
    }

    #[test]
    fn every_native_composed_redraw_revalidates_at_the_surface_boundary() {
        let present = body("    fn present(&mut self)", "    fn send_viewport(");
        let toolbar = present.find("paint_toolbar(").unwrap();
        let validate = present
            .find(".validate_for_presentation(native.accepted)")
            .unwrap();
        let surface = present.find("target.present()").unwrap();
        assert!(toolbar < validate && validate < surface);
        assert!(present.contains("self.native_frame = None"));
        assert!(present.contains("self.frame.as_ref()"));
    }

    #[test]
    fn failed_presentation_distinguishes_current_failure_from_supersession() {
        let install_body = body(
            "    fn install_native_scroll_presentation(",
            "    fn try_native_scroll(",
        );
        assert!(install_body.contains("validate_for_presentation(presentation)"));
        assert!(install_body.contains("reconcile_native_scroll_composition_failure("));
        assert!(install_body.contains("NativePresentationInstall::Superseded"));
        assert!(!install_body.contains("revoke_policy()"));
        let attempt = body("    fn try_native_scroll(", "    fn page_button_mask(");
        let install = attempt
            .find("self.install_native_scroll_presentation")
            .unwrap();
        let rejected = attempt[install..]
            .find("installation != NativePresentationInstall::Installed")
            .unwrap()
            + install;
        let recovery = attempt[rejected..]
            .find("installation == NativePresentationInstall::Failed")
            .unwrap()
            + rejected;
        assert!(install < rejected && rejected < recovery);
        let recompose = body(
            "    fn recompose_native_frame_after_scene_commit(",
            "    fn update_title(",
        );
        assert!(recompose.contains("reconcile_native_scroll_composition_failure("));
        assert!(!recompose.contains("revoke_policy()"));
    }

    #[test]
    fn exact_current_composition_failure_hard_revokes_and_requests_recovery() {
        let (handle, key) = ready_native_scroll_handle();
        let before = handle.publication_gate();
        let publication = test_publication(&handle, key, 1, test_scene(&handle, key, 1, 0));
        let mut native_pixels = Some(7_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();

        let disposition = reconcile_native_scroll_composition_failure(
            &publication,
            &mut native_pixels,
            &mut requires_fresh_scene,
            &mut recovery,
        );

        assert_eq!(
            disposition,
            PublicationReconciliation::InvalidCurrentAuthority
        );
        assert_eq!(native_pixels, None);
        assert!(requires_fresh_scene);
        assert!(recovery.pending);
        assert!(handle.publication_gate().policy_epoch > before.policy_epoch);
    }

    #[test]
    fn old_composition_failure_is_drop_only_after_same_epoch_newer_scene_install() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let old_gate = handle.publication_gate();
        let old = test_publication(&handle, key, 1, test_scene(&handle, key, 1, 0));
        install_native_scroll_scene(&handle, key, 2);
        let live = handle.publication_gate();
        assert_eq!(live.key, old_gate.key);
        assert_eq!(live.policy_epoch, old_gate.policy_epoch);
        assert_eq!(live.acceptance_epoch, old_gate.acceptance_epoch);
        let mut newer_pixels = Some(11_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();

        let disposition = reconcile_native_scroll_composition_failure(
            &old,
            &mut newer_pixels,
            &mut requires_fresh_scene,
            &mut recovery,
        );

        assert_eq!(disposition, PublicationReconciliation::StaleUpdate);
        assert_eq!(newer_pixels, Some(11));
        assert_eq!(handle.publication_gate(), live);
        assert!(!requires_fresh_scene);
        assert!(!recovery.pending);
        assert!(handle.publication_is_current(
            key,
            live.policy_epoch,
            live.acceptance_epoch,
            PublicationSequence::new(2),
        ));
        assert!(matches!(
            handle.scroll_for_publication(
                key,
                live.policy_epoch,
                live.acceptance_epoch,
                PublicationSequence::new(2),
                Offset { x: 0.0, y: 5.0 },
            ),
            Some(ScrollDecision::Accepted(_))
        ));
    }

    #[test]
    fn old_composition_failure_is_drop_only_after_newer_epoch_scene_install() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let old = test_publication(&handle, key, 1, test_scene(&handle, key, 1, 0));
        handle.revoke_policy();
        install_native_scroll_scene(&handle, key, 2);
        let live = handle.publication_gate();
        let mut newer_pixels = Some(9_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();

        let disposition = reconcile_native_scroll_composition_failure(
            &old,
            &mut newer_pixels,
            &mut requires_fresh_scene,
            &mut recovery,
        );

        assert_eq!(disposition, PublicationReconciliation::StaleUpdate);
        assert_eq!(newer_pixels, Some(9));
        assert_eq!(handle.publication_gate(), live);
        assert!(!requires_fresh_scene);
        assert!(!recovery.pending);
        assert!(handle.publication_is_current(
            key,
            live.policy_epoch,
            live.acceptance_epoch,
            PublicationSequence::new(2),
        ));
        assert!(matches!(
            handle.scroll_for_publication(
                key,
                live.policy_epoch,
                live.acceptance_epoch,
                PublicationSequence::new(2),
                Offset { x: 0.0, y: 5.0 },
            ),
            Some(ScrollDecision::Accepted(_))
        ));
    }

    #[test]
    fn pending_commit_replays_an_accepted_default_without_advancing_early() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let mut pending = test_scene(&handle, key, 2, 0);
        pending.offset.y = 40.0;
        assert!(handle.publication_is_current(
            key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            PublicationSequence::new(1),
        ));
        assert!(!handle.publication_is_current(
            key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            PublicationSequence::new(2),
        ));
        let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        assert!(handle.validate_for_presentation(accepted));
        let owner = handle.owner_sequences(key).unwrap();
        let backlog = handle.notification_backlog(key).unwrap();
        let mut canonical_pixels = Some(1_u8);
        let mut retained_pixels = Some(11_u8);
        let mut paint_trace = Some(21_u8);
        let previous_visual_bundle = Some(stage_visual_bundle(
            &mut canonical_pixels,
            &mut retained_pixels,
            &mut paint_trace,
            (2, Some(12), Some(22)),
        ));
        let disposition = handle.commit_scene_publication(
            key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            PublicationSequence::new(2),
            0,
            pending,
            true,
        );
        assert_eq!(disposition, PublicationReconciliation::Current);
        assert!(resolve_staged_visual_bundle(
            disposition,
            &mut canonical_pixels,
            &mut retained_pixels,
            &mut paint_trace,
            previous_visual_bundle,
        ));
        assert_eq!(canonical_pixels, Some(2));
        assert_eq!(retained_pixels, Some(12));
        assert_eq!(paint_trace, Some(22));
        assert_eq!(handle.visual_offset(), Offset { x: 0.0, y: 50.0 });
        let rebound = handle
            .rebind_presentation_to_publication(accepted, PublicationSequence::new(2))
            .expect("latest accepted frame can be rebound to P2");
        assert_eq!(accepted.input().visual_offset, Offset { x: 0.0, y: 10.0 });
        assert_eq!(rebound.visual_offset(), Offset { x: 0.0, y: 50.0 });
        assert!(handle.validate_for_presentation(rebound));
        let current = handle.publication_gate();
        assert!(!handle.validate_for_presentation(accepted));
        assert_eq!(handle.publication_gate(), current);
        assert_eq!(handle.owner_sequences(key), Ok(owner));
        assert_eq!(handle.notification_backlog(key), Ok(backlog));
        assert!(handle.publication_is_current(
            key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            PublicationSequence::new(2),
        ));
    }

    #[test]
    fn hard_gate_race_drops_pending_commit_but_keeps_new_authority_and_pixels() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let pending_scene = test_scene(&handle, key, 2, 0);
        let pending = test_publication(&handle, key, 2, pending_scene);
        handle.revoke_policy();
        install_native_scroll_scene(&handle, key, 3);
        let replacement_gate = handle.publication_gate();
        let replacement_scene = test_scene(&handle, key, 3, 0);
        let replacement = test_publication(&handle, key, 3, replacement_scene);
        let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let mut canonical_pixels = Some(1_u8);
        let mut retained_pixels = Some(11_u8);
        let mut paint_trace = Some(21_u8);
        let previous_visual_bundle = Some(stage_visual_bundle(
            &mut canonical_pixels,
            &mut retained_pixels,
            &mut paint_trace,
            (2, Some(12), Some(22)),
        ));
        let mut native_pixels = Some(7_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = commit_native_scroll_publication(
            &pending,
            false,
            &mut native_pixels,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(disposition, PublicationReconciliation::StaleUpdate);
        assert!(!resolve_staged_visual_bundle(
            disposition,
            &mut canonical_pixels,
            &mut retained_pixels,
            &mut paint_trace,
            previous_visual_bundle,
        ));
        assert_eq!(canonical_pixels, Some(1));
        assert_eq!(retained_pixels, Some(11));
        assert_eq!(paint_trace, Some(21));
        assert_eq!(native_pixels, Some(7));
        assert_eq!(handle.publication_gate(), replacement_gate);
        assert!(handle.validate_for_presentation(accepted));

        let mut cached_publication = Some(replacement);
        let mut transaction = Some(9_u8);
        assert!(!apply_native_scroll_publication(
            disposition,
            pending,
            &mut cached_publication,
            &mut transaction,
            &mut requires_fresh_scene,
            |_, _| panic!("stale pending update must not inspect the transaction"),
            |_| panic!("stale pending update must not replace freshness"),
        ));
        assert_eq!(canonical_pixels, Some(1));
        assert_eq!(native_pixels, Some(7));
        assert_eq!(cached_publication.unwrap().publication_sequence, 3);
        assert_eq!(transaction, Some(9));
        assert!(!requires_fresh_scene);
        assert!(!recovery.pending);
    }

    #[test]
    fn hard_policy_p1_raster_drops_without_discarding_committed_p2_authority() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let p1 = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        handle.revoke_policy();
        let p2_gate = handle.publication_gate();
        let p2 = test_publication(&handle, key, 2, test_scene(&handle, key, 2, 0));
        let mut native_pixels = Some(7_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = commit_native_scroll_publication(
            &p2,
            true,
            &mut native_pixels,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(disposition, PublicationReconciliation::Current);
        assert!(
            p2.handle
                .rebind_presentation_to_publication(
                    p1,
                    PublicationSequence::new(p2.publication_sequence),
                )
                .is_none()
        );
        native_pixels = None;
        let mut cached_publication = None;
        let mut transaction: Option<u8> = None;
        assert!(apply_native_scroll_publication(
            disposition,
            p2,
            &mut cached_publication,
            &mut transaction,
            &mut requires_fresh_scene,
            |_, _| false,
            |_| false,
        ));
        let cached = cached_publication.expect("P2 remains cached");
        assert_eq!(native_pixels, None);
        assert!(cached.handle.publication_is_current(
            key,
            p2_gate.policy_epoch,
            p2_gate.acceptance_epoch,
            PublicationSequence::new(2),
        ));
        assert!(matches!(
            cached.handle.scroll_for_publication(
                key,
                p2_gate.policy_epoch,
                p2_gate.acceptance_epoch,
                PublicationSequence::new(2),
                Offset { x: 0.0, y: 5.0 },
            ),
            Some(ScrollDecision::Accepted(_))
        ));
    }

    #[test]
    fn acknowledged_p1_raster_retires_without_discarding_p2_or_its_notification() {
        use omoikane::js::native_scroll::{Bounds, Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let p1 = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let extent = Bounds {
            min: Offset::default(),
            max: Offset { x: 0.0, y: 200.0 },
        };
        let materialized = handle
            .materialize_pending(key, Offset::default(), extent)
            .expect("owner applies P1 once");
        let mut p2_scene = test_scene(&handle, key, 2, 1);
        p2_scene.offset = materialized.offset;
        let p2 = test_publication(&handle, key, 2, p2_scene);
        let mut native_pixels = Some(7_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = commit_native_scroll_publication(
            &p2,
            true,
            &mut native_pixels,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(disposition, PublicationReconciliation::Current);
        assert!(publication_acknowledges_presentation(&p2, p1));
        assert_eq!(native_pixels.take(), Some(7));

        let mut cached_publication = None;
        let mut transaction: Option<u8> = None;
        assert!(apply_native_scroll_publication(
            disposition,
            p2,
            &mut cached_publication,
            &mut transaction,
            &mut requires_fresh_scene,
            |_, _| false,
            |_| false,
        ));
        let cached = cached_publication.expect("P2 remains cached");
        assert!(cached.handle.publication_is_current(
            key,
            cached.policy_epoch,
            cached.acceptance_epoch,
            PublicationSequence::new(2),
        ));
        assert_eq!(cached.handle.owner_sequences(key), Ok((1, 0)));
        assert_eq!(cached.handle.notification_backlog(key), Ok(1));
        let notification = cached
            .handle
            .claim_notification(key, materialized.offset, extent)
            .unwrap()
            .expect("P1 notification remains pending");
        assert_eq!(notification.input, p1.input());
        assert!(
            cached
                .handle
                .claim_notification(key, notification.offset, extent)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn retained_geometry_mismatch_keeps_canonical_pixels_but_drops_native_association() {
        let (handle, key) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let mut pending_scene = test_scene(&handle, key, 2, 0);
        let current = (pending_scene.offset.x, pending_scene.offset.y);
        let extent = (pending_scene.extent.max.x, pending_scene.extent.max.y);
        let coverage = (
            (pending_scene.coverage.min.x, pending_scene.coverage.min.y),
            (pending_scene.coverage.max.x, pending_scene.coverage.max.y),
        );
        assert!(pending_scene_matches_retained_geometry(
            pending_scene,
            current,
            extent,
            coverage,
        ));
        let mut extent_mismatch = pending_scene;
        extent_mismatch.extent.max.y = f64::from_bits(extent_mismatch.extent.max.y.to_bits() + 1);
        assert!(!pending_scene_matches_retained_geometry(
            extent_mismatch,
            current,
            extent,
            coverage,
        ));
        let mut coverage_mismatch = pending_scene;
        coverage_mismatch.coverage.max.y =
            f64::from_bits(coverage_mismatch.coverage.max.y.to_bits() + 1);
        assert!(!pending_scene_matches_retained_geometry(
            coverage_mismatch,
            current,
            extent,
            coverage,
        ));
        pending_scene.offset.y = f64::from_bits(pending_scene.offset.y.to_bits() + 1);
        assert!(!pending_scene_matches_retained_geometry(
            pending_scene,
            current,
            extent,
            coverage,
        ));
        let publication = test_publication(&handle, key, 2, pending_scene);
        let mut canonical_pixels = Some(1_u8);
        let mut retained_pixels = Some(11_u8);
        let mut paint_trace = Some(21_u8);
        let previous_visual_bundle = Some(stage_visual_bundle(
            &mut canonical_pixels,
            &mut retained_pixels,
            &mut paint_trace,
            (2, Some(12), Some(22)),
        ));
        let mut native_pixels = Some(7_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = commit_native_scroll_publication(
            &publication,
            false,
            &mut native_pixels,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(
            disposition,
            PublicationReconciliation::InvalidCurrentAuthority
        );
        assert!(!resolve_staged_visual_bundle(
            disposition,
            &mut canonical_pixels,
            &mut retained_pixels,
            &mut paint_trace,
            previous_visual_bundle,
        ));
        assert_eq!(canonical_pixels, Some(2));
        assert_eq!(retained_pixels, None);
        assert_eq!(paint_trace, Some(22));
        assert_eq!(native_pixels, None);
        assert!(handle.publication_gate().policy_epoch > candidate.policy_epoch);
    }

    #[test]
    fn outer_acknowledgement_mismatch_hard_rejects_exact_current_candidate() {
        let (handle, key) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let pending_scene = test_scene(&handle, key, 2, 0);
        assert_eq!(
            handle.commit_scene_publication(
                key,
                candidate.policy_epoch,
                candidate.acceptance_epoch,
                PublicationSequence::new(2),
                1,
                pending_scene,
                true,
            ),
            PublicationReconciliation::InvalidCurrentAuthority
        );
        assert!(handle.publication_gate().policy_epoch > candidate.policy_epoch);
    }

    #[test]
    fn policy_advance_between_candidate_and_reconcile_is_drop_only() {
        use omoikane::js::native_scroll::{Bounds, FallbackTicket, Offset, Scene, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let old_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        handle.revoke_policy();
        assert!(!handle.validate_for_presentation(old_accepted));
        let extent = Bounds {
            min: Offset::default(),
            max: Offset { x: 0.0, y: 200.0 },
        };
        let materialized = handle
            .materialize_pending(key, Offset::default(), extent)
            .expect("materialize invalidated accepted default");
        let replacement = handle.publication_gate();
        assert!(handle.authorize_policy(key, replacement.policy_epoch));
        assert!(handle.install_scene(Scene {
            key,
            publication: PublicationSequence::new(2),
            policy_epoch: replacement.policy_epoch,
            acceptance_epoch: replacement.acceptance_epoch,
            acknowledged: materialized.default_applied_sequence,
            completed_fallback: FallbackTicket::default(),
            offset: materialized.offset,
            extent,
            coverage: extent,
            eligible: true,
        }));
        let new_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let live_gate = handle.publication_gate();
        let mut frame = Some(9_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = reconcile_native_scroll_publication(
            &handle,
            candidate.key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            1,
            false,
            &mut frame,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(disposition, PublicationReconciliation::StaleUpdate);
        assert_eq!(frame, Some(9));
        assert_eq!(handle.publication_gate(), live_gate);
        assert!(handle.validate_for_presentation(new_accepted));
        assert!(!requires_fresh_scene);
        assert!(!recovery.pending);
        assert_stale_update_preserves_cached_state(disposition);
        assert_eq!(handle.publication_gate(), live_gate);
    }

    #[test]
    fn key_replacement_between_candidate_and_reconcile_is_drop_only() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, old_key) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let old_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let replacement_key = TargetKey {
            owner_generation: old_key.owner_generation + 1,
            ..old_key
        };
        assert!(handle.activate(replacement_key));
        install_native_scroll_scene(&handle, replacement_key, 2);
        let new_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let live_gate = handle.publication_gate();
        let mut frame = Some(9_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = reconcile_native_scroll_publication(
            &handle,
            candidate.key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            1,
            false,
            &mut frame,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(disposition, PublicationReconciliation::StaleUpdate);
        assert_eq!(frame, Some(9));
        assert_eq!(handle.publication_gate(), live_gate);
        assert!(!handle.validate_for_presentation(old_accepted));
        assert!(handle.validate_for_presentation(new_accepted));
        assert!(!requires_fresh_scene);
        assert!(!recovery.pending);
        assert_stale_update_preserves_cached_state(disposition);
    }

    #[test]
    fn acceptance_reopen_between_candidate_and_reconcile_is_drop_only() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let old_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        handle.close_acceptance();
        install_native_scroll_scene(&handle, key, 2);
        let new_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let live_gate = handle.publication_gate();
        assert!(live_gate.acceptance_epoch > candidate.acceptance_epoch);
        let mut frame = Some(9_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = reconcile_native_scroll_publication(
            &handle,
            candidate.key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            1,
            false,
            &mut frame,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(disposition, PublicationReconciliation::StaleUpdate);
        assert_eq!(frame, Some(9));
        assert_eq!(handle.publication_gate(), live_gate);
        assert!(!handle.validate_for_presentation(old_accepted));
        assert!(handle.validate_for_presentation(new_accepted));
        assert!(!requires_fresh_scene);
        assert!(!recovery.pending);
        assert_stale_update_preserves_cached_state(disposition);
    }

    #[test]
    fn same_epoch_newer_scene_between_candidate_and_reconcile_is_drop_only() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, key) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let old_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        install_native_scroll_scene(&handle, key, 2);
        let new_accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let live_gate = handle.publication_gate();
        assert_eq!(live_gate, candidate);
        let mut frame = Some(9_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = reconcile_native_scroll_publication(
            &handle,
            candidate.key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            1,
            false,
            &mut frame,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(disposition, PublicationReconciliation::StaleUpdate);
        assert_eq!(frame, Some(9));
        assert_eq!(handle.publication_gate(), live_gate);
        assert!(!handle.validate_for_presentation(old_accepted));
        assert!(handle.validate_for_presentation(new_accepted));
        assert!(!requires_fresh_scene);
        assert!(!recovery.pending);
        assert_stale_update_preserves_cached_state(disposition);
    }

    #[test]
    fn exact_current_invalid_retained_association_hard_revokes() {
        use omoikane::js::native_scroll::{Offset, ScrollDecision};

        let (handle, _) = ready_native_scroll_handle();
        let candidate = handle.publication_gate();
        let accepted = match handle.scroll(Offset { x: 0.0, y: 10.0 }) {
            ScrollDecision::Accepted(accepted) => accepted,
            decision => panic!("unexpected {decision:?}"),
        };
        let mut frame = Some(9_u8);
        let mut requires_fresh_scene = false;
        let mut recovery = NativeScrollRecovery::default();
        let disposition = reconcile_native_scroll_publication(
            &handle,
            candidate.key,
            candidate.policy_epoch,
            candidate.acceptance_epoch,
            1,
            false,
            &mut frame,
            &mut requires_fresh_scene,
            &mut recovery,
        );
        assert_eq!(
            disposition,
            PublicationReconciliation::InvalidCurrentAuthority
        );
        assert_eq!(frame, None);
        assert!(handle.publication_gate().policy_epoch > candidate.policy_epoch);
        assert!(!handle.validate_for_presentation(accepted));

        let mut cached_publication = Some(20_u8);
        let mut cached_transaction = Some(30_u8);
        assert!(!apply_native_scroll_publication(
            disposition,
            10_u8,
            &mut cached_publication,
            &mut cached_transaction,
            &mut requires_fresh_scene,
            |_, _| panic!("invalid publication cannot match a transaction"),
            |_| panic!("invalid publication cannot derive freshness"),
        ));
        assert_eq!(cached_publication, None);
        assert_eq!(cached_transaction, None);
        assert!(requires_fresh_scene);
    }

    #[test]
    fn canonical_frame_update_keeps_only_a_revalidated_native_frame() {
        let mut canonical = Some(1_u8);
        let mut native = Some(7_u8);
        replace_canonical_frame(&mut canonical, 2, &mut native, true);
        assert_eq!(canonical, Some(2));
        assert_eq!(native, Some(7));

        replace_canonical_frame(&mut canonical, 3, &mut native, false);
        assert_eq!(canonical, Some(3));
        assert_eq!(native, None);
    }

    #[test]
    fn observable_page_work_revokes_before_it_enters_the_owner_fifo() {
        let send = body(
            "    fn send_observable_page_command(",
            "    fn send_media_environment(",
        );
        assert!(
            send.find("revoke_native_scroll_before_page_work").unwrap()
                < send.find("self.worker.send(command)").unwrap()
        );
        let frames = body("    fn queue_frame(", "    fn pointer_input_is_recent(");
        assert!(!frames.contains("revoke_native_scroll_before_page_work"));
    }

    #[test]
    fn no_frame_owner_turn_revalidates_instead_of_unconditionally_retiring_authority() {
        let update = body("    fn apply_update(", "    fn update_title(");
        assert!(update.contains("native_scroll_authority_is_current"));
        assert!(SOURCE.contains("publication_is_current("));
        assert!(update.contains("if retire_native_scroll"));
        assert!(!update.contains("if update.frame.is_none()"));
    }

    #[test]
    fn publication_stages_pixels_before_commit_and_restores_stale_bundles() {
        let update = body("    fn apply_update(", "    fn update_title(");
        let classify = update.find("let retained_association_matches").unwrap();
        let take_publication = update
            .find("let native_scroll = update.native_scroll")
            .unwrap();
        let pending_pixels = update.find("let pending_visual_bundle").unwrap();
        let install_publication = update
            .find("if let Some(publication) = native_scroll")
            .unwrap();
        let stage_pixels = update[install_publication..]
            .find("stage_visual_bundle(")
            .unwrap()
            + install_publication;
        let commit = update[stage_pixels..]
            .find("commit_native_scroll_publication(")
            .unwrap()
            + stage_pixels;
        let resolve = update[commit..]
            .find("resolve_staged_visual_bundle(")
            .unwrap()
            + commit;
        assert!(classify < take_publication);
        assert!(take_publication < pending_pixels && pending_pixels < install_publication);
        assert!(install_publication < stage_pixels && stage_pixels < commit && commit < resolve);
        assert!(update.contains("replace_canonical_frame("));
        assert!(update[resolve..].contains("recompose_native_frame_after_scene_commit"));
        assert!(update[classify..take_publication].contains("self.retained_scene.as_ref()"));
        let commit = body(
            "fn commit_native_scroll_publication<T>(",
            "fn apply_native_scroll_publication<P, T>(",
        );
        assert!(commit.contains("commit_scene_publication("));
        assert!(commit.contains("publication.owner_default_sequence"));
        assert!(SOURCE.contains("pending_scene_matches_retained_geometry("));
    }
}
