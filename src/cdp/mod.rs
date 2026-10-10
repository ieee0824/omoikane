//! CDP transport primitives: WebSocket upgrade, frame handling, and JSON-RPC routing.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use boa_engine::JsValue;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::accessibility::{
    AccessibilityNode, AccessibilityProperty, AccessibilityTree, AccessibilityValue,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent,
};
use crate::html::{DecodedHtml, TreeBuilder, decode_html_response};
use crate::http::{Client, HttpRequest, Method};
use crate::http::{HttpParseError, url::UrlParseError};
#[cfg(test)]
use crate::js::PageTaskSource;
use crate::js::{
    CompletedPageTask, FindInPageAction, FormStateRestoreMode, FormStateSnapshot,
    FullscreenTransition, JavaScriptDialog, JavaScriptDialogController, JavaScriptDialogError,
    JavaScriptDialogKind, JsRuntime, NavigationRequest, OwnedPageTask, PageTaskError,
    PointerLockTransition, StorageManager, VisitSource,
};

mod render_demand;
pub use render_demand::{NextRendering, PaintStateKey, RenderDemand};

mod accessibility;
mod browser_session;
mod dom;
mod emulation;
mod input;
mod page;
mod runtime;
mod server;
mod target;
mod transport;
pub use browser_session::BrowserSession;
use browser_session::take_page_task_script_error_lines;
use dom::dom_text_content;
pub use server::{
    CdpMethodResult, CdpServer, DeferredResponseToken, JsonRpcRequest, JsonRpcResponse,
};
pub use transport::{
    WebSocketFrame, WebSocketOpcode, WebSocketUpgrade, WebSocketUpgradeRequest,
    parse_upgrade_request, websocket_accept_key,
};

/// Errors produced while upgrading, parsing frames, or dispatching JSON-RPC requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpError {
    InvalidHttpRequest(&'static str),
    InvalidWebSocketFrame(&'static str),
    InvalidJsonRpc(&'static str),
    UnknownClient(u64),
    UnknownDeferredRequest(u64),
    DeferredTokenExhausted,
    MethodNotFound(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHttpRequest(message) => write!(f, "invalid HTTP request: {message}"),
            Self::InvalidWebSocketFrame(message) => {
                write!(f, "invalid WebSocket frame: {message}")
            }
            Self::InvalidJsonRpc(message) => write!(f, "invalid JSON-RPC message: {message}"),
            Self::UnknownClient(id) => write!(f, "unknown client: {id}"),
            Self::UnknownDeferredRequest(id) => write!(f, "unknown deferred request: {id}"),
            Self::DeferredTokenExhausted => write!(f, "deferred response token space exhausted"),
            Self::MethodNotFound(method) => write!(f, "method not found: {method}"),
        }
    }
}

impl std::error::Error for CdpError {}

/// Errors produced while constructing or navigating a CDP page session.
#[derive(Debug)]
pub enum CdpSessionError {
    /// JavaScript runtime setup or document installation failed.
    JavaScript(boa_engine::JsError),
    /// The requested navigation URL was invalid.
    Url(UrlParseError),
    /// Loading the requested document failed.
    Http(HttpParseError),
    /// A page startup task completed for a retired document generation.
    StalePageTaskCompletion,
    /// A page startup task was cancelled.
    PageTaskCancelled,
    /// A page startup task exceeded its wall-clock limit.
    PageTaskTimedOut,
    /// The browser session no longer has an active page.
    Unavailable,
}

impl std::fmt::Display for CdpSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JavaScript(error) => write!(f, "{error}"),
            Self::Url(error) => write!(f, "{error}"),
            Self::Http(error) => write!(f, "{error}"),
            Self::StalePageTaskCompletion => write!(f, "stale page startup task completion"),
            Self::PageTaskCancelled => write!(f, "page startup task was cancelled"),
            Self::PageTaskTimedOut => {
                write!(f, "page startup task exceeded its wall-clock timeout")
            }
            Self::Unavailable => write!(f, "Browser session is unavailable"),
        }
    }
}

impl std::error::Error for CdpSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::JavaScript(error) => Some(error),
            Self::Url(error) => Some(error),
            Self::Http(error) => Some(error),
            _ => None,
        }
    }
}

impl From<boa_engine::JsError> for CdpSessionError {
    fn from(error: boa_engine::JsError) -> Self {
        Self::JavaScript(error)
    }
}

impl From<UrlParseError> for CdpSessionError {
    fn from(error: UrlParseError) -> Self {
        Self::Url(error)
    }
}

impl From<HttpParseError> for CdpSessionError {
    fn from(error: HttpParseError) -> Self {
        Self::Http(error)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for JsonRpcError {}

/// A queued CDP event emitted by domain operations.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SessionSettleTimings {
    pub timers: Duration,
    pub animation_frames: Duration,
}

/// Minimal stateful CDP session spanning Accessibility, Page, DOM, Network,
/// Runtime, Target, and Input.
#[derive(Debug)]
pub struct CdpSession {
    runtime: JsRuntime,
    media_emulation: emulation::State,
    _storage_lifetime: TabStorageLifetime,
    storage_manager: StorageManager,
    storage_session_id: u64,
    http_client: Client,
    cookie_store: Arc<Mutex<crate::http::CookieJar>>,
    current_url: String,
    last_html: String,
    frame_id: String,
    next_loader_id: u64,
    next_node_id: u64,
    node_to_id: HashMap<usize, u64>,
    id_to_node: HashMap<u64, NodeHandle>,
    next_object_id: u64,
    /// Document generation which owns each live Runtime remote-object handle.
    /// Cross-document commits clear the table; same-document history changes
    /// intentionally preserve it.
    remote_object_generations: HashMap<String, u64>,
    next_browser_context_id: u64,
    browser_context_ids: Vec<String>,
    pending_events: Vec<CdpEvent>,
    last_key_event: Option<Value>,
    last_mouse_event: Option<Value>,
    touch_target: Option<NodeHandle>,
    last_touch_position: Option<(f64, f64)>,
    touch_scroll_allowed: bool,
    mouse_pressed_target: Option<usize>,
    drag_candidate: bool,
    drag_active: bool,
    history_entries: Vec<SessionHistoryEntry>,
    history_index: usize,
    document_generation: u64,
    accessibility_enabled: bool,
    fullscreen_supported: bool,
    fullscreen_transition_allowed: bool,
    /// Host presentation state and CDP lifecycle override are independent.
    host_hidden: bool,
    lifecycle_frozen: bool,
    pending_fullscreen_transition: Option<FullscreenTransition>,
    pointer_lock_deferred: bool,
    pointer_lock_focused: bool,
    pending_pointer_release: bool,
    last_pointer_position: Option<(f64, f64)>,
    previous_input_locked: bool,
}

/// Declared after the runtime so workers and child realms finish dropping
/// before the closed tab's session storage is released.
#[derive(Debug)]
struct TabStorageLifetime {
    manager: StorageManager,
    session_id: u64,
}

impl Drop for TabStorageLifetime {
    fn drop(&mut self) {
        self.manager.remove_session(self.session_id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavigationCommit {
    Push,
    Replace,
    Reload,
    Traverse(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionHistoryEntry {
    url: String,
    state_json: String,
    form_state: Option<FormStateSnapshot>,
    document_generation: u64,
}

/// Document metadata held by the CDP host while its new runtime is owned by a
/// suspendable page-startup task.
pub(crate) struct PendingDocumentCommit {
    url: String,
    html: String,
    generation: u64,
    history_commit: NavigationCommit,
    loader_id: String,
    status: u16,
}

#[derive(Clone, Copy)]
enum CdpFailure {
    Request,
    Navigation,
    PageTask,
}

pub(crate) enum PreparedPageNavigation {
    Complete(Value),
    Pending {
        task: OwnedPageTask,
        commit: PendingDocumentCommit,
        response: Value,
    },
}

impl CdpSession {
    /// Creates a new session with an empty `about:blank` document.
    pub fn new() -> Result<Self, CdpSessionError> {
        Self::with_storage_manager(StorageManager::new())
    }

    /// Creates a tab with its own session storage in a shared browser profile.
    pub fn with_storage_manager(storage_manager: StorageManager) -> Result<Self, CdpSessionError> {
        let storage_session_id = storage_manager.create_session();
        let storage_lifetime = TabStorageLifetime {
            manager: storage_manager.clone(),
            session_id: storage_session_id,
        };
        let cookie_store = Arc::new(Mutex::new(crate::http::CookieJar::new()));
        let mut runtime = JsRuntime::with_document_url_and_storage(
            TreeBuilder::parse("<html><head></head><body></body></html>").document(),
            "about:blank",
            storage_manager.clone(),
            storage_session_id,
        )
        .map_err(CdpSessionError::JavaScript)?;
        Self::configure_runtime_playback(&mut runtime, &cookie_store);
        let mut http_client = Client::new();
        http_client.set_shared_cookie_store(Arc::clone(&cookie_store));
        let mut session = Self {
            runtime,
            _storage_lifetime: storage_lifetime,
            storage_manager,
            storage_session_id,
            http_client,
            cookie_store,
            current_url: "about:blank".to_string(),
            last_html: String::new(),
            frame_id: "frame-0".to_string(),
            next_loader_id: 1,
            next_node_id: 1,
            node_to_id: HashMap::new(),
            id_to_node: HashMap::new(),
            next_object_id: 0,
            remote_object_generations: HashMap::new(),
            next_browser_context_id: 0,
            browser_context_ids: Vec::new(),
            pending_events: Vec::new(),
            last_key_event: None,
            last_mouse_event: None,
            touch_target: None,
            last_touch_position: None,
            touch_scroll_allowed: true,
            mouse_pressed_target: None,
            drag_candidate: false,
            drag_active: false,
            history_entries: vec![SessionHistoryEntry {
                url: "about:blank".to_string(),
                state_json: "null".to_string(),
                form_state: None,
                document_generation: 0,
            }],
            history_index: 0,
            document_generation: 0,
            media_emulation: emulation::State::default(),
            accessibility_enabled: false,
            fullscreen_supported: true,
            fullscreen_transition_allowed: true,
            host_hidden: false,
            lifecycle_frozen: false,
            pending_fullscreen_transition: None,
            pointer_lock_deferred: false,
            pointer_lock_focused: true,
            pending_pointer_release: false,
            last_pointer_position: None,
            previous_input_locked: false,
        };
        session
            .install_runtime_helpers()
            .map_err(CdpSessionError::JavaScript)?;
        session.rebuild_node_index();
        Ok(session)
    }

    /// Sets the shared resource store and live playback for every tab runtime.
    fn configure_runtime_playback(
        runtime: &mut JsRuntime,
        cookie_store: &Arc<Mutex<crate::http::CookieJar>>,
    ) {
        runtime.enable_live_css_animations();
        runtime.set_shared_cookie_store(Arc::clone(cookie_store));
    }

    /// Carries host configuration into a replacement document runtime.
    fn configure_replacement_runtime(
        &self,
        runtime: &mut JsRuntime,
    ) -> Result<(), CdpSessionError> {
        if let Some((reporter, surface)) = self.runtime.error_reporter_destination() {
            runtime.set_error_reporter(reporter, surface);
        }
        Self::configure_runtime_playback(runtime, &self.cookie_store);
        runtime.set_initial_visibility_hidden(self.host_hidden || self.lifecycle_frozen);
        let (width, height) = self.runtime.presentation_viewport();
        runtime.set_viewport(width, height);
        runtime.set_user_agent(self.http_client.user_agent().to_string());
        runtime
            .set_media_environment(self.runtime.media_environment())
            .map_err(CdpSessionError::JavaScript)?;
        runtime.set_fullscreen_supported(self.fullscreen_supported);
        runtime.set_pointer_lock_deferred(self.pointer_lock_deferred);
        runtime
            .set_pointer_lock_focus(self.pointer_lock_focused)
            .map_err(CdpSessionError::JavaScript)?;
        runtime.set_fullscreen_transition_allowed(self.fullscreen_transition_allowed);
        Self::install_runtime_helpers_on(runtime).map_err(CdpSessionError::JavaScript)
    }

    fn runtime_timeout(&self) -> Duration {
        self.runtime.sandbox_timeout()
    }

    /// Dispatches a CDP domain method and returns the result payload.
    pub fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, JsonRpcError> {
        let result = match method {
            "Page.navigate" => self.page_navigate(&params),
            "Page.reload" => self.page_reload(),
            "Page.getFrameTree" => Ok(self.page_get_frame_tree()),
            "Page.setWebLifecycleState" => self.page_set_web_lifecycle_state(&params),
            "DOM.getDocument" => self.dom_get_document(&params),
            "DOM.getAttributes" => self.dom_get_attributes(&params),
            "DOM.querySelector" => self.dom_query_selector(&params),
            "DOM.getOuterHTML" => self.dom_get_outer_html(&params),
            "Accessibility.enable" => {
                self.accessibility_enabled = true;
                Ok(json!({}))
            }
            "Accessibility.disable" => {
                self.accessibility_enabled = false;
                Ok(json!({}))
            }
            "Accessibility.getFullAXTree" => self.accessibility_get_full_tree(&params),
            "Accessibility.getRootAXNode" => self.accessibility_get_root_node(&params),
            "Accessibility.getPartialAXTree" => self.accessibility_get_partial_tree(&params),
            "Accessibility.getAXNodeAndAncestors" => {
                self.accessibility_get_node_and_ancestors(&params)
            }
            "Accessibility.getChildAXNodes" => self.accessibility_get_child_nodes(&params),
            "Accessibility.queryAXTree" => self.accessibility_query_tree(&params),
            "Emulation.setEmulatedMedia" => self.emulation_set_media(&params),
            "Runtime.evaluate" => self.runtime_evaluate(&params),
            "Runtime.callFunctionOn" => self.runtime_call_function_on(&params),
            "Runtime.releaseObject" => self.runtime_release_object(&params),
            "Omoikane.findInPage" => self.omoikane_find_in_page(&params),
            "Target.createBrowserContext" => Ok(self.target_create_browser_context()),
            "Target.getBrowserContexts" => Ok(self.target_get_browser_contexts()),
            "Target.disposeBrowserContext" => self.target_dispose_browser_context(&params),
            "Input.dispatchKeyEvent" => self.input_dispatch_key_event(&params),
            "Input.dispatchMouseEvent" => self.input_dispatch_mouse_event(&params),
            "Input.dispatchTouchEvent" => self.input_dispatch_touch_event(&params),
            "Input.imeSetComposition" => self.input_ime_set_composition(&params),
            "Input.insertText" => self.input_insert_text(&params),
            _ => Err(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {method}"),
            }),
        };
        if result.is_err() {
            self.report_cdp_failure(if matches!(method, "Page.navigate" | "Page.reload") {
                CdpFailure::Navigation
            } else {
                CdpFailure::Request
            });
        }
        result
    }

    /// Returns and clears queued protocol events.
    pub fn drain_events(&mut self) -> Vec<CdpEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Returns the current document.
    pub fn document(&self) -> NodeHandle {
        self.runtime.document()
    }

    fn omoikane_find_in_page(&mut self, params: &Value) -> Result<Value, JsonRpcError> {
        let action = match require_string(params, "action")?.as_str() {
            "start" => FindInPageAction::Start,
            "next" => FindInPageAction::Next,
            "previous" => FindInPageAction::Previous,
            "status" => FindInPageAction::Status,
            "stop" => FindInPageAction::Stop,
            _ => return Err(invalid_params("Invalid find-in-page action".to_string())),
        };
        let query = if action == FindInPageAction::Start {
            require_string(params, "query")?
        } else {
            String::new()
        };
        let result = self
            .runtime
            .find_in_page(action, &query)
            .map_err(|error| JsonRpcError {
                code: -32000,
                message: error.to_string(),
            })?;
        serde_json::to_value(result).map_err(|error| JsonRpcError {
            code: -32000,
            message: error.to_string(),
        })
    }

    /// Returns the URL of the currently loaded document.
    pub fn current_url(&self) -> &str {
        &self.current_url
    }

    /// Renders the current document using its input/CSSOM layout.
    pub(crate) fn paint_current_document(
        &mut self,
    ) -> Result<crate::paint::Canvas, crate::paint::PaintError> {
        self.runtime.paint_current_document()
    }

    /// Attaches the optional error reporter to this session's runtime and HTTP client.
    pub fn set_error_reporter(&mut self, reporter: Arc<ErrorReporter>, surface: ExecutionSurface) {
        self.runtime
            .set_error_reporter(Arc::clone(&reporter), surface);
        self.http_client.set_error_reporter(reporter, surface);
    }

    fn report_cdp_failure(&self, failure: CdpFailure) {
        let Some((reporter, surface)) = self.runtime.error_reporter_destination() else {
            return;
        };
        let (code, operation) = match failure {
            CdpFailure::Request => ("CDP_REQUEST_FAILED", "execute"),
            CdpFailure::Navigation => ("CDP_NAVIGATION_FAILED", "navigate"),
            CdpFailure::PageTask => ("CDP_PAGE_TASK_FAILED", "execute"),
        };
        reporter.report(
            RawEvent::new(
                ErrorCategory::Cdp,
                ErrorSeverity::Error,
                ErrorCode::new(code).expect("static CDP error code"),
                surface,
                "CDP operation failed",
                &[("operation", operation)],
            )
            .sanitize(),
        );
    }

    pub(crate) fn report_paint_failure(&self, code: &'static str) {
        self.runtime.report_paint_failure(code);
    }

    /// Updates the active page's layout and script-visible viewport dimensions.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.runtime.set_viewport(width as f32, height as f32);
    }

    /// Updates the page visibility reported to scripts by its native host.
    /// A frozen CDP lifecycle remains hidden until it is reactivated.
    pub fn set_host_visibility(&mut self, hidden: bool) {
        self.host_hidden = hidden;
        self.runtime
            .set_page_visibility(hidden || self.lifecycle_frozen);
    }

    /// Updates the visible portion of the active page for host-driven zoom or
    /// on-screen-keyboard changes. Values are expressed in CSS pixels.
    pub fn set_visual_viewport(
        &mut self,
        width: f32,
        height: f32,
        offset_left: f32,
        offset_top: f32,
        scale: f32,
    ) {
        self.runtime
            .set_visual_viewport(width, height, offset_left, offset_top, scale);
    }

    /// Advances the active page event loop and commits script navigation.
    /// `elapsed_ms` is a delta in milliseconds since the previous opportunity,
    /// not an absolute timestamp. It advances page, worker and worklet clocks.
    ///
    /// This is the browser-session lifecycle entry point for a future GUI
    /// frame pump: timer tasks and their microtasks run first, then one
    /// animation frame, then any resulting Location/History request installs
    /// the next Document.
    pub fn drive_event_loop(&mut self, elapsed_ms: u64) -> Result<(), JsonRpcError> {
        self.runtime
            .run_animation_frame(elapsed_ms)
            .map_err(js_error)?;
        self.drive_navigation_requests()
    }

    /// Settles deferred page initialization before a static render snapshot.
    pub(crate) fn settle_for_render(&mut self) -> Result<SessionSettleTimings, JsonRpcError> {
        const MAX_NAVIGATION_PASSES: usize = 8;
        const MAX_VIRTUAL_MS: u64 = 10_000;
        const TIMER_STEP_MS: u64 = 10;
        const MAX_TIMER_TASKS: usize = 100_000;
        const MAX_ANIMATION_FRAMES: usize = 8;

        let mut timings = SessionSettleTimings::default();
        for _ in 0..MAX_NAVIGATION_PASSES {
            let previous_generation = self.document_generation;
            let timers_start = Instant::now();
            self.runtime
                .run_timers(MAX_VIRTUAL_MS, TIMER_STEP_MS, MAX_TIMER_TASKS);
            timings.timers += timers_start.elapsed();
            let animation_frames_start = Instant::now();
            self.runtime.run_animation_frames(MAX_ANIMATION_FRAMES, 16);
            timings.animation_frames += animation_frames_start.elapsed();
            self.drive_navigation_requests()?;
            if self.document_generation == previous_generation {
                return Ok(timings);
            }
        }
        Err(JsonRpcError {
            code: -32000,
            message: "render navigation limit exceeded".to_string(),
        })
    }

    /// Sets the HTTP client `User-Agent` used for subsequent navigations.
    pub fn set_user_agent(&mut self, user_agent: impl Into<String>) {
        let user_agent = user_agent.into();
        self.http_client.set_user_agent(user_agent.clone());
        self.runtime.set_user_agent(user_agent);
    }

    /// When `true`, disables TLS certificate verification for all subsequent
    /// navigations. Expired certificates, self-signed certificates, and hostname
    /// mismatches are silently accepted.
    ///
    /// **Security warning**: Only use this in development or testing environments.
    pub fn set_insecure(&mut self, insecure: bool) {
        self.http_client.set_insecure(insecure);
    }

    /// Enables or disables fullscreen support for this tab's presentation host.
    pub fn set_fullscreen_supported(&mut self, supported: bool) {
        self.fullscreen_supported = supported;
        self.runtime.set_fullscreen_supported(supported);
        if let Some(transition) = self.runtime.take_fullscreen_transition() {
            self.pending_fullscreen_transition = Some(transition);
        }
    }

    /// Controls whether the presentation host accepts fullscreen transitions.
    pub fn set_fullscreen_transition_allowed(&mut self, allowed: bool) {
        self.fullscreen_transition_allowed = allowed;
        self.runtime.set_fullscreen_transition_allowed(allowed);
    }

    /// Takes the next native window-state transition requested by the page.
    pub fn take_fullscreen_transition(&mut self) -> Option<FullscreenTransition> {
        self.pending_fullscreen_transition
            .take()
            .or_else(|| self.runtime.take_fullscreen_transition())
    }

    /// Notifies the DOM that the native host left fullscreen independently.
    pub fn fullscreen_exited_by_host(&mut self) -> Result<bool, JsonRpcError> {
        self.runtime.exit_fullscreen_from_host().map_err(js_error)
    }

    /// Configures an external pointer-lock host, including after navigation.
    pub fn set_pointer_lock_deferred(&mut self, deferred: bool) {
        self.pointer_lock_deferred = deferred;
        self.runtime.set_pointer_lock_deferred(deferred);
    }

    /// Takes the next cursor operation requested by the page.
    pub fn take_pointer_lock_transition(&mut self) -> Option<PointerLockTransition> {
        if std::mem::take(&mut self.pending_pointer_release) {
            Some(PointerLockTransition::Release)
        } else {
            self.runtime.take_pointer_lock_transition()
        }
    }

    /// Acknowledges an actual native cursor-grab attempt.
    pub fn complete_pointer_lock_request(
        &mut self,
        id: u64,
        accepted: bool,
    ) -> Result<bool, JsonRpcError> {
        self.runtime
            .complete_pointer_lock_request(id, accepted)
            .map_err(js_error)
    }

    /// Releases pointer lock and pending requests on a host-originated exit.
    pub fn release_pointer_lock_from_host(&mut self) -> Result<(), JsonRpcError> {
        self.last_pointer_position = None;
        self.runtime
            .release_pointer_lock_from_host()
            .map_err(js_error)
    }

    /// Updates the native window focus and releases lock when focus is lost.
    pub fn set_pointer_lock_focus(&mut self, focused: bool) -> Result<(), JsonRpcError> {
        self.pointer_lock_focused = focused;
        self.last_pointer_position = None;
        self.runtime
            .set_pointer_lock_focus(focused)
            .map_err(js_error)
    }

    /// Whether native relative movement should currently be delivered.
    pub fn is_pointer_locked(&self) -> bool {
        self.runtime.pointer_lock_target().is_some()
    }

    /// Resets movement deltas after the cursor leaves the surface.
    pub fn reset_pointer_movement(&mut self) {
        self.last_pointer_position = None;
    }

    /// Clears pointer movement and hover state when the pointer leaves the view.
    pub fn pointer_left_surface(&mut self) {
        self.reset_pointer_movement();
        self.runtime.clear_pointer_hover();
    }

    pub(crate) fn http_client_mut(&mut self) -> &mut Client {
        &mut self.http_client
    }

    async fn evaluate_expression_async(
        &mut self,
        expression: &str,
        return_by_value: bool,
    ) -> Result<Value, JsonRpcError> {
        // Evaluate the requested source directly. Calling JavaScript `eval()`
        // from an async outer script would use Boa's synchronous nested-eval
        // path and reject a native-call suspension.
        let value = self
            .runtime
            .eval_async(expression)
            .await
            .map_err(js_error)?;
        self.runtime.run_until_idle().map_err(js_error)?;
        let result = self.serialize_evaluation_value(value, return_by_value)?;
        self.drive_navigation_requests()?;
        Ok(result)
    }

    fn emit(&mut self, method: &str, params: Value) {
        self.pending_events.push(CdpEvent {
            method: method.to_string(),
            params,
        });
    }
}

fn require_string(params: &Value, key: &'static str) -> Result<String, JsonRpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or(JsonRpcError {
            code: -32602,
            message: format!("Missing or invalid string parameter: {key}"),
        })
}

fn optional_string(params: &Value, key: &'static str) -> Result<Option<String>, JsonRpcError> {
    match params.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| invalid_params(format!("Invalid string parameter: {key}"))),
    }
}

fn require_u64(params: &Value, key: &'static str) -> Result<u64, JsonRpcError> {
    params.get(key).and_then(Value::as_u64).ok_or(JsonRpcError {
        code: -32602,
        message: format!("Missing or invalid numeric parameter: {key}"),
    })
}

fn invalid_params(message: String) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message,
    }
}

fn optional_f64(params: &Value, key: &'static str, default: f64) -> Result<f64, JsonRpcError> {
    let value = params.get(key).map(Value::as_f64).unwrap_or(Some(default));
    value
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid_params(format!("Missing or invalid numeric parameter: {key}")))
}

fn js_error(error: boa_engine::JsError) -> JsonRpcError {
    JsonRpcError {
        code: -32000,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod frameset_tests;

#[cfg(test)]
mod render_demand_tests;
