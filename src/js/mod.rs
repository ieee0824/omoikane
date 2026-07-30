//! JavaScript engine embedding and DOM/Web API bindings.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::task::{Context as TaskContext, Poll};
use std::thread;

use base64::Engine as _;
use boa_engine::JsString;
use boa_engine::Module;
use boa_engine::builtins::promise::{OperationType, PromiseState};
use boa_engine::context::HostHooks;
use boa_engine::module::{ModuleLoader, Referrer};
use boa_engine::native_function::{NativeCallSuspension, NativeFunction};
use boa_engine::object::{
    JsObject,
    builtins::{JsArrayBuffer, JsPromise, JsUint8Array},
};
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, Script, Source, js_string};

use crate::css::{
    AffineTransform, ComputedStyle, ComputedValue, Origin, Selector, StyleResolver, matches_selector,
    parse_selector_list,
};
use crate::dom::{Node, NodeHandle, NodeType, ShadowRootMode, is_actually_disabled};
use crate::http::{Client, HttpRequest, Method, default_user_agent};
use crate::http::cors::{
    CredentialsMode, Origin as CorsOrigin, PreflightCache, RedirectMode, RequestMode,
    ResponseType, exposed_response_headers,
};
use crate::layout::{InlineFragmentContent, LayoutBox, Rect, edge_sizes};

mod storage;
mod event_loop;
pub use storage::StorageManager;
use storage::StorageOrigin;
use event_loop::{EventLoop, Task};

/// Most page-script task errors retained per drain. See
/// [`JsRuntime::record_task_error`].
const MAX_TASK_ERRORS: usize = 32;

thread_local! {
    static ACTIVE_HOST_STATE: RefCell<Option<Rc<RefCell<HostState>>>> = const { RefCell::new(None) };
}

struct ActiveHostGuard(Option<Rc<RefCell<HostState>>>);

impl Drop for ActiveHostGuard {
    fn drop(&mut self) {
        ACTIVE_HOST_STATE.with(|slot| {
            slot.replace(self.0.take());
        });
    }
}

fn activate_host_state(host_state: Rc<RefCell<HostState>>) -> ActiveHostGuard {
    let previous = ACTIVE_HOST_STATE.with(|slot| slot.replace(Some(host_state)));
    ActiveHostGuard(previous)
}

struct ActiveHostFuture<F> {
    future: Pin<Box<F>>,
    host_state: Rc<RefCell<HostState>>,
}

impl<F> Drop for ActiveHostFuture<F> {
    fn drop(&mut self) {
        self.host_state.borrow_mut().pending_javascript_dialog = None;
    }
}

impl<F: Future> Future for ActiveHostFuture<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let _guard = activate_host_state(Rc::clone(&self.host_state));
        self.future.as_mut().poll(cx)
    }
}

const DOM_BOOTSTRAP: &str = include_str!("dom_bootstrap.js");

#[derive(Debug)]
struct BrowserHostHooks;

impl HostHooks for BrowserHostHooks {
    fn promise_rejection_tracker(
        &self,
        promise: &JsObject,
        operation: OperationType,
        _context: &mut Context,
    ) {
        if operation != OperationType::Reject || std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_none()
        {
            return;
        }
        let Ok(promise) = JsPromise::from_object(promise.clone()) else {
            return;
        };
        if let PromiseState::Rejected(reason) = promise.state() {
            eprintln!("[omoikane][unhandled-rejection] {}", reason.display());
            for frame in _context.stack_trace() {
                let location = frame.position();
                eprintln!(
                    "[omoikane][unhandled-rejection-frame] function={} path={:?} position={:?}",
                    location.function_name.to_std_string_escaped(),
                    location.path,
                    location.position
                );
            }
            if let Some(error) = reason.as_object()
                && let Ok(stack) = error.get(js_string!("stack"), _context)
                && !stack.is_undefined()
            {
                eprintln!("{}", stack.display());
            }
        }
    }
}

#[derive(Debug, Default)]
struct HttpModuleLoader {
    modules: RefCell<HashMap<String, Module>>,
    client: RefCell<Client>,
}

impl ModuleLoader for HttpModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        specifier: JsString,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        let result = (|| {
            let specifier = specifier.to_std_string_escaped();
            let referrer_url = referrer
                .path()
                .and_then(|path| path.to_str())
                .and_then(|path| path.parse::<crate::http::Url>().ok());
            let resolved = if specifier.starts_with("http://") || specifier.starts_with("https://")
            {
                specifier
                    .parse::<crate::http::Url>()
                    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
            } else {
                let base = referrer_url.as_ref().ok_or_else(|| {
                    JsNativeError::typ()
                        .with_message(format!("cannot resolve module specifier: {specifier}"))
                })?;
                crate::http::url::resolve_url(base, &specifier)
                    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
            };
            let resolved_string = resolved.to_string();

            if let Some(module) = self.modules.borrow().get(&resolved_string) {
                if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
                    eprintln!("[omoikane][module] cache-hit {resolved_string}");
                }
                return Ok(module.clone());
            }

            let public_only = requires_public_fetch(&resolved, referrer_url.as_ref());
            let fetch_start = std::time::Instant::now();
            let response = if public_only {
                self.client.borrow_mut().get_public(&resolved_string)
            } else {
                self.client.borrow_mut().get(&resolved_string)
            }
            .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
            if response.status_code() != 200 {
                return Err(JsNativeError::typ()
                    .with_message(format!(
                        "module request returned HTTP {}",
                        response.status_code()
                    ))
                    .into());
            }
            let fetch_elapsed = fetch_start.elapsed();
            let source_bytes = response.body().len();
            let source = String::from_utf8_lossy(response.body());
            let parse_start = std::time::Instant::now();
            let module = Module::parse(
                Source::from_reader(source.as_bytes(), Some(Path::new(&resolved_string))),
                None,
                &mut context.borrow_mut(),
            )?;
            let parse_elapsed = parse_start.elapsed();
            if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
                eprintln!(
                    "[omoikane][module] loaded {resolved_string} bytes={source_bytes} fetch_ms={:.3} parse_ms={:.3}",
                    fetch_elapsed.as_secs_f64() * 1_000.0,
                    parse_elapsed.as_secs_f64() * 1_000.0,
                );
            }
            self.modules
                .borrow_mut()
                .insert(resolved_string, module.clone());
            Ok(module)
        })();
        async { result }
    }

    fn init_import_meta(
        self: Rc<Self>,
        import_meta: &JsObject,
        module: &Module,
        context: &mut Context,
    ) {
        if let Some(url) = module.path().and_then(|path| path.to_str()) {
            let _ = import_meta.set(js_string!("url"), js_string!(url), false, context);
        }
    }
}

/// What a scheduled timer executes when it fires.
///
/// `setTimeout`/`setInterval` accept either a code string (legal per the HTML
/// spec, evaluated in the global scope) or a function callback. Function
/// callbacks are retained as live `JsValue` handles so their captured closure
/// scope survives until the timer fires, together with any extra arguments
/// passed after the delay (`setTimeout(fn, ms, arg1, arg2, ...)`).
#[derive(Debug, Clone)]
enum TimerPayload {
    /// A code string, evaluated in the global scope when the timer fires.
    Source(String),
    /// A retained function callback plus the extra arguments to invoke it with.
    Callback {
        callback: JsValue,
        args: Vec<JsValue>,
    },
    /// A connected iframe/object resource load, followed by `load` dispatch.
    ResourceLoad { node_id: usize },
}

/// A top-level navigation requested by script in the current browsing context.
///
/// The JavaScript runtime only queues the request. The owning browser session
/// decides when to fetch and install the next Document, keeping networking and
/// browsing-history ownership outside the ECMAScript embedding layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationRequest {
    Navigate { url: String, replace: bool },
    /// A form submission whose encoded payload must be fetched as a document.
    FormSubmit {
        url: String,
        method: String,
        body: Option<Vec<u8>>,
        content_type: Option<String>,
    },
    UpdateHistory {
        url: String,
        replace: bool,
        state_json: String,
    },
    Reload,
    Traverse { delta: i32 },
}

/// Kind of blocking Window modal dialog requested by page script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaScriptDialogKind {
    Alert,
    Confirm,
    Prompt,
}

/// Script-visible metadata for the currently pending Window modal dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaScriptDialog {
    pub id: u64,
    pub kind: JavaScriptDialogKind,
    pub message: String,
    pub default_prompt: Option<String>,
}

/// Opaque identity of the JavaScript runtime that owns a modal dialog.
/// Dialog ids are monotonic only within one runtime, so frontends serving
/// several runtimes must compare both values.
#[derive(Clone)]
pub struct JavaScriptRuntimeIdentity(Rc<()>);

impl std::fmt::Debug for JavaScriptRuntimeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("JavaScriptRuntimeIdentity")
            .field(&Rc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for JavaScriptRuntimeIdentity {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for JavaScriptRuntimeIdentity {}

/// Failure while resolving a pending Window modal dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JavaScriptDialogError {
    NoPendingDialog,
    StaleDialog { expected: u64, actual: u64 },
    EvaluationCancelled,
}

impl std::fmt::Display for JavaScriptDialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPendingDialog => write!(f, "no JavaScript dialog is pending"),
            Self::StaleDialog { expected, actual } => {
                write!(f, "stale JavaScript dialog id {actual}; expected {expected}")
            }
            Self::EvaluationCancelled => write!(f, "JavaScript dialog evaluation was cancelled"),
        }
    }
}

impl std::error::Error for JavaScriptDialogError {}

/// Cloneable control-plane handle for observing and resolving Window dialogs
/// while [`JsRuntime::eval_async`] exclusively borrows the JavaScript runtime.
#[derive(Clone)]
pub struct JavaScriptDialogController {
    host_state: Rc<RefCell<HostState>>,
}

/// A dialog request bound to the runtime that created it.
/// Clones share the same exactly-once resolution state.
#[derive(Clone)]
pub struct JavaScriptDialogRequest {
    dialog: JavaScriptDialog,
    controller: JavaScriptDialogController,
}

impl std::fmt::Debug for JavaScriptDialogRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JavaScriptDialogRequest")
            .field("runtime", &self.runtime_identity())
            .field("dialog", &self.dialog)
            .finish()
    }
}

impl JavaScriptDialogRequest {
    pub fn dialog(&self) -> &JavaScriptDialog {
        &self.dialog
    }

    pub fn runtime_identity(&self) -> JavaScriptRuntimeIdentity {
        self.controller.runtime_identity()
    }

    pub fn resolve(
        &self,
        accept: bool,
        prompt_text: Option<String>,
    ) -> Result<(), JavaScriptDialogError> {
        self.controller.handle(self.dialog.id, accept, prompt_text)
    }

    pub fn dismiss(&self) -> Result<(), JavaScriptDialogError> {
        self.resolve(false, None)
    }

    pub fn is_pending(&self) -> bool {
        self.controller
            .pending()
            .is_some_and(|pending| pending.id == self.dialog.id)
    }

    pub(crate) fn same_request(&self, other: &Self) -> bool {
        self.runtime_identity() == other.runtime_identity() && self.dialog.id == other.dialog.id
    }
}

impl JavaScriptDialogController {
    pub fn runtime_identity(&self) -> JavaScriptRuntimeIdentity {
        JavaScriptRuntimeIdentity(Rc::clone(&self.host_state.borrow().runtime_identity))
    }

    pub fn pending(&self) -> Option<JavaScriptDialog> {
        self.host_state
            .borrow()
            .pending_javascript_dialog
            .as_ref()
            .map(|pending| pending.dialog.clone())
    }

    pub fn pending_request(&self) -> Option<JavaScriptDialogRequest> {
        self.pending().map(|dialog| JavaScriptDialogRequest {
            dialog,
            controller: self.clone(),
        })
    }

    pub fn handle(
        &self,
        dialog_id: u64,
        accept: bool,
        prompt_text: Option<String>,
    ) -> Result<(), JavaScriptDialogError> {
        let pending = {
            let mut state = self.host_state.borrow_mut();
            let Some(pending) = state.pending_javascript_dialog.take() else {
                return Err(JavaScriptDialogError::NoPendingDialog);
            };
            if pending.dialog.id != dialog_id {
                let expected = pending.dialog.id;
                state.pending_javascript_dialog = Some(pending);
                return Err(JavaScriptDialogError::StaleDialog {
                    expected,
                    actual: dialog_id,
                });
            }
            pending
        };

        let value = match pending.dialog.kind {
            JavaScriptDialogKind::Alert => JsValue::undefined(),
            JavaScriptDialogKind::Confirm => JsValue::from(accept),
            JavaScriptDialogKind::Prompt if !accept => JsValue::null(),
            JavaScriptDialogKind::Prompt => JsValue::from(js_string!(
                prompt_text.unwrap_or_else(|| pending.dialog.default_prompt.unwrap_or_default())
            )),
        };
        pending
            .suspension
            .resume(Ok(value))
            .map_err(|_| JavaScriptDialogError::EvaluationCancelled)
    }
}

struct PendingJavaScriptDialog {
    dialog: JavaScriptDialog,
    suspension: NativeCallSuspension,
}

impl TimerPayload {
    fn kind(&self) -> &'static str {
        match self {
            Self::Source(_) => "source",
            Self::Callback { .. } => "callback",
            Self::ResourceLoad { .. } => "resource-load",
        }
    }
}

struct HostState {
    runtime_identity: Rc<()>,
    /// Monotonic clock origin used by `performance.now()` for this global.
    performance_start: std::time::Instant,
    /// Unix epoch milliseconds corresponding to `performance_start`.
    performance_time_origin: f64,
    event_loop: EventLoop,
    document: NodeHandle,
    nodes: HashMap<usize, NodeHandle>,
    console_logs: Vec<String>,
    /// Errors raised by page script while an event-loop task ran.
    ///
    /// A task's failure belongs to the page, not to the embedder's request, so it
    /// is collected here and the loop continues. Draining is the embedder's job
    /// (see [`JsRuntime::take_task_errors`]); leaving them unreported is how the
    /// navigation-aborting bug in issue #303 stayed invisible.
    task_errors: Vec<String>,
    /// How many task errors were dropped once `task_errors` hit its cap.
    suppressed_task_errors: usize,
    location_href: String,
    navigator_user_agent: String,
    http_client: Client,
    websocket_clients: HashMap<u64, WebSocketConnection>,
    next_websocket_id: u64,
    /// Successful CORS preflight results for this environment settings object.
    cors_preflight_cache: PreflightCache,
    /// Viewport used when resolving computed styles and running layout for the
    /// `getComputedStyle` / layout-metrics bindings (issues 016-8 and 044-2).
    viewport: Rect,
    /// Top-level Window scroll offset in document CSS pixels.
    window_scroll: (f32, f32),
    /// Scroll targets waiting for the next rendering opportunity. This is an
    /// ordered set: first-queue order is retained and duplicate ids are skipped.
    pending_scroll_targets: Vec<usize>,
    /// Effective element offsets captured before invalidating layout. Reflow
    /// compares these with the rebuilt scrolling extents to detect clamps.
    scroll_offsets_before_layout: HashMap<usize, (f32, f32)>,
    /// Per-document cached style resolvers, keyed by the identity of each
    /// document's root [`Document`] node (the top-level document and every
    /// `<iframe>` sub-browsing-context document). Each entry is rebuilt on
    /// demand from that document's own inline `<style>` rules when it is marked
    /// dirty, so `getComputedStyle` on a node resolves against the cascade of
    /// the document that node actually lives in — the main document's rules
    /// never leak into a sub-document and vice versa (issue 016-15).
    document_styles: HashMap<usize, DocumentStyleEntry>,
    /// Cached layout tree for the **main** document, matching its entry in
    /// [`HostState::document_styles`]. Rebuilt lazily and only when a layout
    /// metric (not just a computed style) is requested. Sub-documents do not
    /// participate in layout (layout metrics for sub-document nodes report
    /// zero); only computed styles are document-scoped here.
    layout_root: Option<LayoutBox>,
    #[cfg(test)]
    layout_generation: u64,
    #[cfg(test)]
    style_resolver_generation: u64,
    /// Insertion reference for `document.write`.
    ///
    /// Models the HTML tokenizer's "insertion point". While a `<script>` runs,
    /// this holds the node **after which** the next `document.write` fragment is
    /// inserted (initially the script element itself, so written content lands
    /// as the script's following siblings, exactly as a streaming parser would
    /// place it). Each write advances the reference to the last node it
    /// inserted, so consecutive writes stay in order. `None` means no script is
    /// currently executing; a write then falls back to appending to `<body>`.
    write_insertion_ref: Option<NodeHandle>,
    /// Base URL of the top-level document, used to resolve relative resource
    /// references such as `<iframe src="empty.html">`. Populated when the
    /// document's scripts run (see [`JsRuntime::execute_document_scripts`]) or
    /// explicitly via [`JsRuntime::set_base_url`]. `None` means relative
    /// references cannot be resolved.
    base_url: Option<crate::http::Url>,
    /// Sub-browsing-context documents — one per `<iframe>` element whose
    /// `contentDocument` has been accessed. Keyed by the iframe element's node
    /// identity. Each entry records the loaded sub-document root and the `src`
    /// value it was loaded from, so a subsequent `src` change triggers a
    /// reload while an unchanged `src` returns the same document instance.
    iframe_documents: HashMap<usize, IframeDocument>,
    /// Resource elements that already have a queued load task. This prevents a
    /// move within one connected document from producing duplicate events.
    pending_resource_loads: HashSet<usize>,
    navigation_requests: VecDeque<NavigationRequest>,
    pending_javascript_dialog: Option<PendingJavaScriptDialog>,
    next_javascript_dialog_id: u64,
    storage_manager: StorageManager,
    storage_session_id: u64,
    document_origins: HashMap<usize, Option<StorageOrigin>>,
}

#[derive(Debug)]
enum WebSocketReadResult {
    Message(crate::realtime::WebSocketMessage),
    Error(String),
}

#[derive(Debug)]
struct WebSocketConnection {
    client: crate::realtime::WebSocketClient,
    incoming: Receiver<WebSocketReadResult>,
}

/// A loaded sub-browsing-context document owned by an `<iframe>` element.
#[derive(Debug)]
struct IframeDocument {
    /// Root document node of the sub-browsing context.
    document: NodeHandle,
    /// The `src` attribute value this document was loaded from (`""` for an
    /// `about:blank` sub-document with no `src`).
    loaded_src: String,
}

/// A cached [`StyleResolver`] for one document (the top-level document or an
/// iframe sub-document), plus a dirty flag driving lazy rebuilds.
///
/// The resolver is seeded from that document's own inline `<style>` rules only;
/// it is rebuilt on the next computed-style query whenever [`dirty`] is set (or
/// the resolver has never been built), so a DOM mutation in one document does
/// not force every other document's resolver to rebuild.
///
/// [`dirty`]: DocumentStyleEntry::dirty
#[derive(Debug, Default)]
struct DocumentStyleEntry {
    /// Cached resolver seeded with this document's inline `<style>` rules, or
    /// `None` until first built.
    resolver: Option<StyleResolver>,
    /// `true` when this document was mutated since `resolver` was built, so the
    /// next query must rebuild it (a forced synchronous style recompute).
    dirty: bool,
    /// DOM-derived values changed and every element must be sampled once so
    /// transition start/end values are discovered without rebuilding rules.
    needs_full_sample: bool,
}

impl std::fmt::Debug for HostState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostState")
            .field("nodes", &self.nodes.len())
            .field("console_logs", &self.console_logs.len())
            .field("location_href", &self.location_href)
            .field("document_styles", &self.document_styles.len())
            .finish()
    }
}

/// Default viewport dimensions (px) used for computed-style and layout-metric
/// resolution when the embedder has not configured one. Matches the
/// `window.innerWidth` / `window.innerHeight` defaults exposed by the DOM
/// bootstrap so `vw`/`vh` units and metrics agree with `window.inner*`.
const DEFAULT_VIEWPORT_WIDTH: f32 = 1280.0;
const DEFAULT_VIEWPORT_HEIGHT: f32 = 720.0;
const DEFAULT_IFRAME_VIEWPORT_WIDTH: f32 = 300.0;
const DEFAULT_IFRAME_VIEWPORT_HEIGHT: f32 = 150.0;

/// Clamps a caller-supplied viewport dimension to a finite, non-negative pixel
/// value.
///
/// A viewport width/height flows unchecked into `StyleResolver::set_viewport`
/// and `layout_tree`, so a `NaN`, `±∞`, or negative value would produce invalid
/// geometry (e.g. `vw`/`vh` resolving to `NaN`) or overflow a later `as i64`
/// cast. Any non-finite or negative input therefore maps to `0.0`, a safe and
/// well-defined dimension; finite non-negative inputs pass through unchanged.
fn sanitize_viewport_dimension(dim: f32) -> f32 {
    if dim.is_finite() && dim >= 0.0 {
        dim
    } else {
        0.0
    }
}

impl HostState {
    fn new(
        document: NodeHandle,
        location_href: String,
        storage_manager: StorageManager,
        storage_session_id: u64,
    ) -> Self {
        // Seed the main document's style entry immediately so its identity is a
        // known key from the start; iframe sub-document entries are created when
        // their content document is first loaded (see `iframe_content_document`).
        let mut document_styles = HashMap::new();
        document_styles.insert(
            document.identity(),
            DocumentStyleEntry {
                resolver: None,
                dirty: true,
                needs_full_sample: true,
            },
        );
        let performance_start = std::time::Instant::now();
        let performance_time_origin = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1_000.0;
        let mut document_origins = HashMap::new();
        document_origins.insert(document.identity(), StorageOrigin::from_url(&location_href));
        let mut state = Self {
            runtime_identity: Rc::new(()),
            performance_start,
            performance_time_origin,
            event_loop: EventLoop::default(),
            document: document.clone(),
            nodes: HashMap::new(),
            console_logs: Vec::new(),
            task_errors: Vec::new(),
            suppressed_task_errors: 0,
            base_url: location_href.parse::<crate::http::Url>().ok(),
            location_href,
            navigator_user_agent: default_user_agent(),
            http_client: Client::new(),
            websocket_clients: HashMap::new(),
            next_websocket_id: 1,
            cors_preflight_cache: PreflightCache::default(),
            viewport: Rect {
                x: 0.0,
                y: 0.0,
                width: DEFAULT_VIEWPORT_WIDTH,
                height: DEFAULT_VIEWPORT_HEIGHT,
            },
            window_scroll: (0.0, 0.0),
            pending_scroll_targets: Vec::new(),
            scroll_offsets_before_layout: HashMap::new(),
            document_styles,
            layout_root: None,
            #[cfg(test)]
            layout_generation: 0,
            #[cfg(test)]
            style_resolver_generation: 0,
            write_insertion_ref: None,
            iframe_documents: HashMap::new(),
            pending_resource_loads: HashSet::new(),
            navigation_requests: VecDeque::new(),
            pending_javascript_dialog: None,
            next_javascript_dialog_id: 1,
            storage_manager,
            storage_session_id,
            document_origins,
        };
        state.register_tree(&document);
        state
    }

    /// Queue loads for iframe and data-bearing object descendants when a
    /// detached subtree first becomes connected to a document.
    fn schedule_connected_resource_loads(&mut self, root: &NodeHandle, include_scripts: bool) {
        fn visit(state: &mut HostState, node: &NodeHandle, include_scripts: bool) {
            let tag = node.tag_name().unwrap_or_default();
            let is_resource = tag.eq_ignore_ascii_case("iframe")
                || (include_scripts
                    && tag.eq_ignore_ascii_case("script")
                    && node
                        .attributes()
                        .is_some_and(|attrs| attrs.contains_key("src")))
                || (tag.eq_ignore_ascii_case("object")
                    && node
                        .attributes()
                        .is_some_and(|attrs| attrs.contains_key("data")));
            if is_resource && state.pending_resource_loads.insert(node.identity()) {
                state.event_loop.enqueue_timer(TimerPayload::ResourceLoad {
                    node_id: node.identity(),
                });
            }
            for child in node.child_nodes() {
                visit(state, &child, include_scripts);
            }
        }
        visit(self, root, include_scripts);
    }

    /// Queue a fresh navigation for a single resource element whose resource
    /// attribute (`<iframe src>` / `<object data>`) has just changed, dispatching
    /// `load` when the new sub-document finishes loading.
    ///
    /// This is the single-node analogue of
    /// [`schedule_connected_resource_loads`](Self::schedule_connected_resource_loads),
    /// driven by an attribute write rather than a connection. It only queues when:
    ///
    /// - the element is **connected** to a document — a detached element defers
    ///   its load until it is later connected, so `about:blank`/`src` set on a
    ///   freestanding element never fires prematurely;
    /// - the resource actually **changed** — setting the attribute to the value
    ///   already loaded is a no-op navigation.
    ///
    /// The `pending_resource_loads` guard collapses a change that races an
    /// already-queued load (e.g. "set `src`, then append") into a single task.
    fn schedule_resource_load_on_attribute_change(
        &mut self,
        node: &NodeHandle,
        resource_attr: &str,
    ) {
        if document_root_for_node(node).is_none() {
            return;
        }
        let new_src = node
            .attributes()
            .and_then(|attrs| attrs.get(resource_attr).cloned())
            .unwrap_or_default()
            .trim()
            .to_string();
        if self
            .iframe_documents
            .get(&node.identity())
            .is_some_and(|entry| entry.loaded_src == new_src)
        {
            return;
        }
        if self.pending_resource_loads.insert(node.identity()) {
            self.event_loop.enqueue_timer(TimerPayload::ResourceLoad {
                node_id: node.identity(),
            });
        }
    }

    /// Returns the embedded document for an iframe or object, loading it on the
    /// first access and reloading it whenever its resource attribute changes.
    ///
    /// The returned document's whole node tree is registered so it can be
    /// traversed and mutated through the DOM primitives exactly like the
    /// top-level document. Iframes load their `src`, while objects load their
    /// `data` attribute. An empty or `about:blank` resource yields an empty HTML
    /// skeleton (`<html><head></head><body></body></html>`). Other resources are
    /// parsed as HTML or XML (including SVG) according to their content type;
    /// unsupported content types and load failures yield the empty skeleton.
    fn iframe_content_document(&mut self, iframe: &NodeHandle) -> NodeHandle {
        let resource_attribute = if iframe
            .tag_name()
            .is_some_and(|tag| tag.eq_ignore_ascii_case("object"))
        {
            "data"
        } else {
            "src"
        };
        let src = iframe
            .attributes()
            .and_then(|attrs| attrs.get(resource_attribute).cloned())
            .unwrap_or_default()
            .trim()
            .to_string();
        let iframe_id = iframe.identity();

        if let Some(entry) = self.iframe_documents.get(&iframe_id)
            && entry.loaded_src == src
        {
            return entry.document.clone();
        }

        // The `src` changed (or this is the first load). Drop any previously
        // loaded sub-document tree from the node registry before loading the
        // new one, so stale nodes are released and their ids stop resolving
        // instead of leaking across reloads. The top-level sub-document's style
        // cache entry is dropped too, so the reloaded document does not inherit
        // the old document's resolver. Cleanup is not recursive: iframes nested
        // inside the discarded sub-document keep their `iframe_documents` /
        // `document_styles` entries (tracked in issue 049).
        if let Some(previous) = self.iframe_documents.remove(&iframe_id) {
            self.document_styles.remove(&previous.document.identity());
            self.document_origins.remove(&previous.document.identity());
            self.unregister_tree(&previous.document);
        }

        let document = self.load_iframe_document(&src);
        let origin = if src.is_empty() || src.eq_ignore_ascii_case("about:blank") {
            let owner_document = owner_document_for_node(iframe);
            self.document_origins
                .get(&owner_document.as_ref().map_or(self.document.identity(), NodeHandle::identity))
                .cloned()
                .flatten()
        } else {
            match resolve_resource_ref(&src, self.base_url.as_ref()) {
                Some(ResolvedResource::Url(url)) => StorageOrigin::from_url(&url),
                _ => None,
            }
        };
        self.register_tree(&document);
        self.document_origins.insert(document.identity(), origin);
        // Seed a dirty style cache entry for the freshly loaded sub-document so
        // its resolver is built from its own `<style>` rules on first query.
        self.document_styles.insert(
            document.identity(),
            DocumentStyleEntry {
                resolver: None,
                dirty: true,
                needs_full_sample: true,
            },
        );
        self.iframe_documents.insert(
            iframe_id,
            IframeDocument {
                document: document.clone(),
                loaded_src: src,
            },
        );
        document
    }

    /// Fetches and constructs a sub-document from an iframe `src` or object
    /// `data` resource reference.
    ///
    /// Returns an `about:blank` skeleton for an empty/`about:blank` reference, a
    /// fetch failure, or an unsupported content type. HTML resources are parsed
    /// as HTML; XML MIME types, including SVG, are parsed as XML.
    fn load_iframe_document(&mut self, src: &str) -> NodeHandle {
        if src.is_empty() || src.eq_ignore_ascii_case("about:blank") {
            return blank_html_document();
        }

        // Resolve the reference (shared with script loading) to either inline
        // `data:` bytes or an absolute URL, then obtain the (mime, body) pair.
        //
        // NOTE: unlike script loading (see `fetch_script_source`), an iframe
        // load intentionally does NOT inspect the HTTP status code. A real
        // browser renders even an error response's body into the frame, so we
        // adopt the response body as the sub-document regardless of status.
        // This asymmetry with `fetch_script_source` (which requires 200) is
        // deliberate.
        let fetched: Option<(String, Vec<u8>)> =
            match resolve_resource_ref(src, self.base_url.as_ref()) {
                Some(ResolvedResource::Data { mime_type, data }) => Some((mime_type, data)),
                Some(ResolvedResource::Url(url)) => self.http_client.get(&url).ok().map(|resp| {
                    let mime = resp.header("Content-Type").unwrap_or("").to_string();
                    (mime, resp.body().to_vec())
                }),
                None => None,
            };

        match fetched {
            Some((mime, body)) if is_html_mime_type(&mime) => {
                let html = String::from_utf8_lossy(&body);
                crate::html::TreeBuilder::parse(&html).document()
            }
            Some((mime, body)) if is_xml_mime_type(&mime) => {
                crate::xml::parse(&body).unwrap_or_else(|_| blank_html_document())
            }
            // Unsupported content types (image/png, text/plain, ...) leave the
            // sub-document as an empty skeleton so a page cannot mine markup
            // from them. Acid3 tests 14 and 15 depend on this (a PNG/text file
            // must not yield a <p>).
            _ => blank_html_document(),
        }
    }

    fn register_tree(&mut self, node: &NodeHandle) {
        self.nodes.insert(node.identity(), node.clone());
        if let Some(content) = node.template_content() {
            self.register_tree(&content);
        }
        if let Some(root) = node.shadow_root() {
            self.register_tree(&root);
        }
        for child in node.child_nodes() {
            self.register_tree(&child);
        }
    }

    /// Removes `node` and all its descendants from the id→node registry.
    ///
    /// Called when an iframe reloads (its `src` changed) so the previous
    /// sub-document tree is released and its ids can no longer be resolved,
    /// preventing stale nodes from accumulating in the registry across reloads.
    fn unregister_tree(&mut self, node: &NodeHandle) {
        self.nodes.remove(&node.identity());
        if let Some(content) = node.template_content() {
            self.unregister_tree(&content);
        }
        if let Some(root) = node.shadow_root() {
            self.unregister_tree(&root);
        }
        for child in node.child_nodes() {
            self.unregister_tree(&child);
        }
    }

    fn get_node(&self, id: usize) -> Option<NodeHandle> {
        self.nodes.get(&id).cloned()
    }

    /// Marks the given document's cached style resolver as stale, creating its
    /// entry if it does not yet exist.
    ///
    /// `document` must be the root [`Document`] node of a browsing context (the
    /// top-level document or an iframe sub-document); it is keyed by its node
    /// identity. When it is the main document, the cached layout tree is also
    /// dropped, because layout is only maintained for the main document.
    fn mark_document_style_dirty(&mut self, document: &NodeHandle) {
        let document_id = document.identity();
        self.document_styles.entry(document_id).or_default().dirty = true;
        if document_id == self.document.identity() {
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
        }
    }

    /// Invalidates values derived from the DOM without rebuilding the parsed
    /// stylesheet/rule-index portion of an existing resolver.
    fn invalidate_document_style_cache(&mut self, document: &NodeHandle) {
        let document_id = document.identity();
        if let Some(entry) = self.document_styles.get_mut(&document_id) {
            entry.needs_full_sample = true;
            if let Some(resolver) = entry.resolver.as_mut() {
                resolver.invalidate_style_cache();
            }
        }
        if document_id == self.document.identity() {
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
        }
    }

    /// Invalidates computed/selector results for a node mutation while keeping
    /// stylesheet parsing intact. Detached nodes affect no live document.
    fn invalidate_style_cache_for_node(&mut self, node: &NodeHandle) {
        if let Some(document) = document_root_for_node(node) {
            self.invalidate_document_style_cache(&document);
        }
        if node.tag_name().as_deref() == Some("iframe")
            && let Some(child) = self.iframe_documents.get(&node.identity())
        {
            let child_document = child.document.clone();
            self.mark_document_style_dirty(&child_document);
        }
    }

    /// Drops cached layout for a live main-document node without touching style caches.
    fn invalidate_layout_for_node(&mut self, node: &NodeHandle) {
        if document_root_for_node(node)
            .is_some_and(|document| document.identity() == self.document.identity())
        {
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
        }
    }

    /// Marks the document that `node` currently lives in as stale.
    ///
    /// A detached node cannot affect a live document's style or layout. Its
    /// eventual insertion invalidates the target document in the tree-mutation
    /// path, so mutations made while detached do not need to drop any cache.
    fn mark_style_dirty_for_node(&mut self, node: &NodeHandle) {
        if let Some(document) = document_root_for_node(node) {
            self.mark_document_style_dirty(&document);
        }
        // The iframe element belongs to the parent document, but its rendered
        // content-box establishes the child document's viewport. A width/height
        // style or attribute mutation must therefore invalidate both caches.
        if node.tag_name().as_deref() == Some("iframe")
            && let Some(child) = self.iframe_documents.get(&node.identity())
        {
            let child_document = child.document.clone();
            self.mark_document_style_dirty(&child_document);
        }
    }

    /// Marks every cached document's style resolver as stale and drops the main
    /// document's layout tree.
    ///
    /// Used when a change affects all documents at once — currently a viewport
    /// change, because every resolver shares the same viewport for `vw`/`vh`
    /// resolution.
    fn mark_all_document_styles_dirty(&mut self) {
        for entry in self.document_styles.values_mut() {
            entry.dirty = true;
        }
        self.capture_scroll_offsets_before_layout();
        self.layout_root = None;
    }

    fn capture_scroll_offsets_before_layout(&mut self) {
        let Some(layout_root) = self.layout_root.as_ref() else {
            return;
        };
        fn capture(layout: &LayoutBox, offsets: &mut HashMap<usize, (f32, f32)>) {
            let node_id = layout.node.identity();
            if !offsets.contains_key(&node_id) && layout.node.scroll_offset() != (0.0, 0.0) {
                let offset = layout.scroll_offset();
                if offset != (0.0, 0.0) {
                    offsets.insert(node_id, offset);
                }
            }
            for child in &layout.children {
                capture(child, offsets);
            }
        }
        capture(layout_root, &mut self.scroll_offsets_before_layout);
    }

    fn queue_scroll_target(&mut self, node_id: usize) {
        if !self.pending_scroll_targets.contains(&node_id) {
            self.pending_scroll_targets.push(node_id);
        }
    }

    /// Rebuilds `document`'s cached [`StyleResolver`] from its own inline
    /// stylesheets when that document is dirty (or nothing has been built yet).
    ///
    /// Only inline `<style>` element content belonging to `document` is
    /// collected; external stylesheets are intentionally not fetched here so
    /// that computed-style resolution stays synchronous and free of network
    /// side effects. `document` must be a root [`Document`] node.
    fn ensure_style_resolver(&mut self, document: &NodeHandle) {
        let document_id = document.identity();
        let needs_rebuild = match self.document_styles.get(&document_id) {
            Some(entry) => entry.dirty || entry.resolver.is_none(),
            None => true,
        };
        if !needs_rebuild {
            let transition_time_ms = self.event_loop.rendering_time_ms();
            let (time_changed, requires_layout) = self
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut())
                .map(|resolver| {
                    let changed = resolver.set_transition_time_ms(transition_time_ms);
                    (changed, resolver.running_transitions_require_layout())
                })
                .unwrap_or((false, false));
            if time_changed && requires_layout && document_id == self.document.identity() {
                self.layout_root = None;
            }
            return;
        }
        // Build the resolver as a local first so no `document_styles` borrow is
        // held while `self.viewport` / the document tree are read, then store it.
        let timeline = self
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .map(StyleResolver::take_transition_timeline);
        let transition_time_ms = self.event_loop.rendering_time_ms();
        let viewport = self.viewport_for_document(document);
        let mut resolver = StyleResolver::new();
        if let Some(timeline) = timeline {
            resolver.install_transition_timeline(timeline);
        }
        let _ = resolver.set_transition_time_ms(transition_time_ms);
        resolver.set_viewport(viewport.width, viewport.height);
        for (scope, implicit_scope_root, css) in collect_inline_stylesheets(document) {
            let sheet = crate::paint::stylesheet::parse_stylesheet_forgiving(&css);
            if let Some((scope, order)) = scope {
                resolver.add_scoped_stylesheet_in_order(Origin::Author, sheet, scope, order);
            } else if let Some(implicit_scope_root) = implicit_scope_root {
                resolver.add_stylesheet_with_implicit_scope_root(
                    Origin::Author,
                    sheet,
                    implicit_scope_root,
                );
            } else {
                resolver.add_stylesheet(Origin::Author, sheet);
            }
        }
        self.document_styles.insert(
            document_id,
            DocumentStyleEntry {
                resolver: Some(resolver),
                dirty: false,
                needs_full_sample: true,
            },
        );
        #[cfg(test)]
        {
            self.style_resolver_generation = self.style_resolver_generation.saturating_add(1);
        }
    }

    /// Returns the viewport belonging to a document. A child browsing context
    /// uses its owning iframe's laid-out content box. If layout produced no box,
    /// HTML width/height attributes are used, then the HTML defaults 300x150.
    fn viewport_for_document(&mut self, document: &NodeHandle) -> Rect {
        if document.identity() == self.document.identity() {
            return self.viewport;
        }
        let owner = self.iframe_documents.iter().find_map(|(iframe_id, entry)| {
            (entry.document.identity() == document.identity())
                .then(|| self.nodes.get(iframe_id).cloned())
                .flatten()
        });
        let Some(iframe) = owner else {
            return self.viewport;
        };

        self.ensure_layout();
        if let Some(layout_box) = self
            .layout_root
            .as_ref()
            .and_then(|root| find_layout_box(root, &iframe))
        {
            return Rect {
                x: 0.0,
                y: 0.0,
                width: layout_box.dimensions.content.width.max(0.0),
                height: layout_box.dimensions.content.height.max(0.0),
            };
        }

        let attrs = iframe.attributes().unwrap_or_default();
        let parse_dimension = |name: &str, default: f32| {
            attrs
                .get(name)
                .and_then(|value| value.trim().parse::<f32>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(default)
        };
        Rect {
            x: 0.0,
            y: 0.0,
            width: parse_dimension("width", DEFAULT_IFRAME_VIEWPORT_WIDTH),
            height: parse_dimension("height", DEFAULT_IFRAME_VIEWPORT_HEIGHT),
        }
    }

    /// Rebuilds the main document's cached layout tree when needed, first
    /// ensuring its style resolver is current. Runs a full synchronous layout
    /// (forced reflow). Layout is only maintained for the main document.
    fn ensure_layout(&mut self) {
        let document = self.document.clone();
        self.ensure_style_resolver(&document);
        if self.layout_root.is_some() {
            return;
        }
        let viewport = self.viewport;
        let document_id = document.identity();
        // Compute into a local so the `document_styles` borrow is released
        // before assigning `self.layout_root` (a different field).
        let layout = self
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .and_then(|resolver| crate::layout::layout_tree(&document, resolver, viewport));
        #[cfg(test)]
        if layout.is_some() {
            self.layout_generation = self.layout_generation.saturating_add(1);
        }
        self.layout_root = layout;
        let mut clamped_targets = Vec::new();
        if let Some(layout_root) = self.layout_root.as_ref() {
            for (node_id, previous) in std::mem::take(&mut self.scroll_offsets_before_layout) {
                let Some(node) = self.nodes.get(&node_id) else {
                    continue;
                };
                let Some(layout) = find_layout_box(layout_root, node) else {
                    continue;
                };
                if !layout.is_scroll_container() {
                    continue;
                }
                let next = layout.scroll_offset();
                if previous != next {
                    node.set_scroll_offset(next.0, next.1);
                    clamped_targets.push(node_id);
                }
            }
        }
        for node_id in clamped_targets {
            self.queue_scroll_target(node_id);
        }
    }

    fn window_scroll_extent(&mut self) -> (f32, f32) {
        self.ensure_layout();
        let (scroll_width, scroll_height) = self
            .layout_root
            .as_ref()
            .map(compute_layout_metrics)
            .map(|metrics| (metrics.scroll_width, metrics.scroll_height))
            .unwrap_or((self.viewport.width, self.viewport.height));
        (
            (scroll_width - self.viewport.width).max(0.0),
            (scroll_height - self.viewport.height).max(0.0),
        )
    }

    fn set_window_scroll(&mut self, x: f32, y: f32) -> bool {
        let (max_x, max_y) = self.window_scroll_extent();
        let next = (x.clamp(0.0, max_x), y.clamp(0.0, max_y));
        if self.window_scroll == next {
            return false;
        }
        self.window_scroll = next;
        let document_id = self.document.identity();
        self.queue_scroll_target(document_id);
        true
    }

    /// Returns the scroll offset in effect for `node`: the offset stored on the
    /// element, clamped to its current scrollable extent.
    ///
    /// An element with no box in the main document's layout — detached,
    /// `display: none`, or living in an iframe sub-document, whose layout is not
    /// maintained — reports zero without disturbing the stored value, so the
    /// offset comes back when its box does.
    fn element_scroll_offset(&mut self, node: &NodeHandle) -> (f32, f32) {
        self.ensure_layout();
        self.layout_root
            .as_ref()
            .and_then(|root| find_layout_box(root, node))
            .map(|layout| layout.scroll_offset())
            .unwrap_or((0.0, 0.0))
    }

    /// Stores a clamped scroll offset for `node`, returning whether the offset in
    /// effect changed (which is what makes a `scroll` event observable).
    ///
    /// Per CSSOM View, an element with no box or no scrolling box is left
    /// untouched rather than remembering an offset it cannot apply.
    fn set_element_scroll(&mut self, node: &NodeHandle, x: f32, y: f32) -> bool {
        self.ensure_layout();
        let Some(layout) = self
            .layout_root
            .as_ref()
            .and_then(|root| find_layout_box(root, node))
        else {
            return false;
        };
        if !layout.is_scroll_container() {
            return false;
        }
        let previous = layout.scroll_offset();
        let (max_x, max_y) = layout.max_scroll_offset();
        let next = (x.clamp(0.0, max_x), y.clamp(0.0, max_y));
        node.set_scroll_offset(next.0, next.1);
        let changed = previous != next;
        if changed {
            self.queue_scroll_target(node.identity());
        }
        changed
    }
}

/// Returns the root [`Document`] node of `node`'s tree, or `node` itself when it
/// is already a `Document`.
///
/// This is the key used to select a node's per-document style cache entry: an
/// attached node resolves to whichever document it currently lives in (the
/// top-level document or an iframe sub-document). A detached node whose tree
/// root is not a document (a freshly created, not-yet-inserted element) yields
/// `None`.
///
/// Unlike the DOM `Node.ownerDocument` accessor (see [`owner_document_for_node`]),
/// a `Document` node maps to itself here, because a document's own style cache is
/// keyed by that document node.
fn document_root_for_node(node: &NodeHandle) -> Option<NodeHandle> {
    if node.node_type() == NodeType::Document {
        return Some(node.clone());
    }
    let mut current = node.clone();
    loop {
        if let Some(parent) = current.parent_node() {
            current = parent;
        } else if let Some(host) = current.shadow_host() {
            current = host;
        } else {
            break;
        }
    }
    if current.node_type() == NodeType::Document {
        Some(current)
    } else {
        None
    }
}

/// Returns the [`Document`] that owns `node` per the DOM `Node.ownerDocument`
/// accessor: like [`document_root_for_node`] but a `Document` node has **no**
/// owner document and maps to `None`.
fn owner_document_for_node(node: &NodeHandle) -> Option<NodeHandle> {
    if node.node_type() == NodeType::Document {
        None
    } else {
        document_root_for_node(node)
    }
}

/// Sandbox configuration for JS execution.
#[derive(Clone)]
pub struct SandboxConfig {
    /// Maximum execution time per eval() call (default: 5 seconds).
    pub timeout: std::time::Duration,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(5),
        }
    }
}

pub struct JsRuntime {
    context: Context,
    host_state: Rc<RefCell<HostState>>,
    sandbox: SandboxConfig,
}

impl std::fmt::Debug for JsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsRuntime")
            .field(
                "sandbox",
                &format_args!("timeout={:?}", self.sandbox.timeout),
            )
            .finish_non_exhaustive()
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new().expect("default JS runtime should be constructible")
    }
}

impl JsRuntime {
    /// Creates a JavaScript runtime with a default document.
    pub fn new() -> JsResult<Self> {
        Self::with_document(default_document())
    }

    /// Creates a JavaScript runtime backed by `document`.
    pub fn with_document(document: NodeHandle) -> JsResult<Self> {
        Self::with_document_and_sandbox(document, SandboxConfig::default())
    }

    /// Creates a runtime whose initial Location and resource base are `url`.
    ///
    /// The URL is installed before the DOM bootstrap executes, so
    /// `location.href`, `document.URL`, and relative resource reflection never
    /// temporarily expose the default localhost URL for a navigated Document.
    pub fn with_document_and_url(document: NodeHandle, url: &str) -> JsResult<Self> {
        let storage = StorageManager::new();
        let session_id = storage.create_session();
        Self::with_document_sandbox_url_and_storage(
            document,
            SandboxConfig::default(),
            url,
            storage,
            session_id,
        )
    }

    pub fn with_document_url_and_storage(
        document: NodeHandle,
        url: &str,
        storage: StorageManager,
        session_id: u64,
    ) -> JsResult<Self> {
        Self::with_document_sandbox_url_and_storage(
            document,
            SandboxConfig::default(),
            url,
            storage,
            session_id,
        )
    }

    /// Creates a JavaScript runtime with custom sandbox configuration.
    pub fn with_document_and_sandbox(
        document: NodeHandle,
        sandbox: SandboxConfig,
    ) -> JsResult<Self> {
        Self::with_document_sandbox_and_url(document, sandbox, "http://localhost/")
    }

    fn with_document_sandbox_and_url(
        document: NodeHandle,
        sandbox: SandboxConfig,
        url: &str,
    ) -> JsResult<Self> {
        let storage = StorageManager::new();
        let session_id = storage.create_session();
        Self::with_document_sandbox_url_and_storage(document, sandbox, url, storage, session_id)
    }

    fn with_document_sandbox_url_and_storage(
        document: NodeHandle,
        sandbox: SandboxConfig,
        url: &str,
        storage_manager: StorageManager,
        storage_session_id: u64,
    ) -> JsResult<Self> {
        // Object URLs belong to the Document that created them. A fresh global
        // means the previous Document is gone, so nothing can resolve its blob
        // URLs any more and neither their bytes nor images decoded from them may
        // outlive it.
        crate::data::clear_blob_urls();
        crate::layout::forget_blob_url_images();
        let host_state = Rc::new(RefCell::new(HostState::new(
            document.clone(),
            url.to_string(),
            storage_manager,
            storage_session_id,
        )));
        let mut context = Context::builder()
            .module_loader(Rc::new(HttpModuleLoader::default()))
            .host_hooks(Rc::new(BrowserHostHooks))
            .build()?;

        register_host_bindings(&mut context, &host_state)?;

        let mut runtime = Self {
            context,
            host_state,
            sandbox,
        };
        runtime.eval(DOM_BOOTSTRAP)?;
        // Parsed resource elements are already connected before the JS wrapper
        // exists. Queue their loads only after bootstrap so dispatch can wrap
        // the target element when the macrotask runs.
        runtime
            .host_state
            .borrow_mut()
            .schedule_connected_resource_loads(&document, false);
        Ok(runtime)
    }

    /// Returns the current DOM document.
    pub fn document(&self) -> NodeHandle {
        self.host_state.borrow().document.clone()
    }

    /// Returns the top-level Window scroll offset in CSS pixels.
    pub(crate) fn window_scroll_offset(&self) -> (f32, f32) {
        self.host_state.borrow().window_scroll
    }

    /// Returns the virtual frame-scheduler timestamp used for rendering.
    pub(crate) fn rendering_time_ms(&self) -> u64 {
        self.host_state.borrow().event_loop.rendering_time_ms() as u64
    }

    /// Returns the topmost event-target element at viewport coordinates.
    /// Layout is cached across calls and cloned only so current scroll offsets
    /// can be applied without changing CSSOM's document-coordinate geometry.
    pub(crate) fn hit_test(&mut self, x: f32, y: f32) -> Option<NodeHandle> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let mut state = self.host_state.borrow_mut();
        state.ensure_layout();
        let mut layout = state.layout_root.clone()?;
        let viewport = state.viewport;
        let scroll = state.window_scroll;
        let document_id = state.document.identity();
        let resolver = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())?;
        crate::paint::apply_scroll_offsets(&mut layout, resolver, viewport, scroll);
        crate::paint::hit_test_layout(&layout, resolver, viewport, x, y)
    }

    /// Sets the User-Agent exposed to scripts in this runtime.
    pub fn set_user_agent(&mut self, user_agent: impl Into<String>) {
        let user_agent = user_agent.into();
        self.host_state.borrow_mut().navigator_user_agent = user_agent.clone();
        if let Ok(quoted) = serde_json::to_string(&user_agent) {
            let _ = self.eval(&format!("globalThis.navigator.userAgent = {quoted};"));
        }
    }

    /// Sets the viewport dimensions (px) used by `getComputedStyle` and the
    /// layout-metric bindings (`getBoundingClientRect`, `offsetWidth`, ...).
    ///
    /// Invalidates any cached style resolver and layout tree so the next query
    /// recomputes against the new viewport. It also updates the script-visible
    /// window metrics (`window.innerWidth`/`innerHeight`, the matching
    /// `outerWidth`/`outerHeight`, and `screen.*`) so page scripts observe the
    /// same viewport that `vw`/`vh` units resolve against. Without this the DOM
    /// bootstrap's fixed 1280x720 defaults would leak through even after the
    /// embedder configured a different render viewport.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        // Sanitize the caller-supplied dimensions before they reach the style
        // resolver and layout engine. A non-finite (`NaN`, `±∞`) or negative
        // width/height would otherwise flow into `StyleResolver::set_viewport`
        // and `layout_tree`, yielding invalid geometry (`vw`/`vh` resolving to
        // `NaN`) or an overflowing `as i64` cast when syncing the JS-visible
        // metrics. Clamp to a finite, non-negative value so the stored native
        // viewport and the script-visible `window.*`/`screen.*` values stay
        // consistent and well-defined.
        let width = sanitize_viewport_dimension(width);
        let height = sanitize_viewport_dimension(height);
        {
            let mut state = self.host_state.borrow_mut();
            state.viewport = Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            };
            // Every document shares this viewport for `vw`/`vh` resolution, so
            // invalidate all cached resolvers (and the main layout tree).
            state.mark_all_document_styles_dirty();
            let (scroll_x, scroll_y) = state.window_scroll;
            if (scroll_x, scroll_y) != (0.0, 0.0) {
                state.set_window_scroll(scroll_x, scroll_y);
            }
        }
        // `window.innerWidth`/`screen.width` are CSSOM integers, so round to the
        // nearest pixel. For integer viewports this exactly matches the `vw`/`vh`
        // resolution (which divides the same dimension by 100). `width`/`height`
        // are already finite and non-negative, so the round/cast cannot overflow.
        let w = width.round() as i64;
        let h = height.round() as i64;
        let sync = format!(
            "globalThis.innerWidth = {w}; globalThis.innerHeight = {h}; \
             globalThis.outerWidth = {w}; globalThis.outerHeight = {h}; \
             if (globalThis.screen) {{ \
             globalThis.screen.width = {w}; globalThis.screen.height = {h}; \
             globalThis.screen.availWidth = {w}; globalThis.screen.availHeight = {h}; }} \
             if (typeof globalThis.__omoikane_media_query_viewport_changed === 'function') \
             globalThis.__omoikane_media_query_viewport_changed(); \
             if (typeof globalThis.__omoikane_layout_observers_changed === 'function') \
             globalThis.__omoikane_layout_observers_changed();"
        );
        // The bootstrap always defines these globals before any embedder call,
        // so this eval cannot fail in practice; ignore the result defensively.
        let _ = self.eval(&sync);
    }

    /// Sets the base URL used to resolve relative resource references such as
    /// `<iframe src="empty.html">`.
    ///
    /// [`execute_document_scripts`](Self::execute_document_scripts) sets this
    /// automatically from its `base_url` argument; call this directly when
    /// driving the runtime without running document scripts.
    pub fn set_base_url(&mut self, url: crate::http::Url) {
        self.host_state.borrow_mut().base_url = Some(url);
    }

    /// Takes navigation requests queued by Location/History APIs in FIFO order.
    pub fn take_navigation_requests(&mut self) -> Vec<NavigationRequest> {
        self.host_state
            .borrow_mut()
            .navigation_requests
            .drain(..)
            .collect()
    }

    /// Returns metadata for the Window modal dialog currently blocking script.
    pub fn pending_javascript_dialog(&self) -> Option<JavaScriptDialog> {
        self.javascript_dialog_controller().pending()
    }

    /// Returns a cloneable handle that remains usable while async evaluation
    /// holds the runtime's mutable borrow.
    pub fn javascript_dialog_controller(&self) -> JavaScriptDialogController {
        JavaScriptDialogController {
            host_state: Rc::clone(&self.host_state),
        }
    }

    /// Resolves the currently pending Window modal dialog exactly once.
    ///
    /// `prompt_text` is used only for an accepted prompt. If it is omitted, the
    /// prompt's default value is returned. Dismissing a prompt produces `null`;
    /// dismissing a confirm produces `false`; alert always produces `undefined`.
    pub fn handle_javascript_dialog(
        &mut self,
        dialog_id: u64,
        accept: bool,
        prompt_text: Option<String>,
    ) -> Result<(), JavaScriptDialogError> {
        self.javascript_dialog_controller()
            .handle(dialog_id, accept, prompt_text)
    }

    pub(crate) fn call_function_with_value(
        &mut self,
        function_source: &str,
        value: JsValue,
    ) -> JsResult<JsValue> {
        let result = self.with_active_host_value(|context| {
            let function = context.eval(Source::from_bytes(function_source))?;
            let callable = function.as_callable().ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ().with_message("CDP serializer is not callable"),
                )
            })?;
            callable.call(&JsValue::undefined(), &[value], context)
        });
        // This helper is a synchronous execution boundary. If a serializer
        // getter/toJSON attempts to open a modal dialog, Boa rejects the
        // suspension; discard the corresponding host metadata on every exit.
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        result
    }

    /// Returns the console log buffer captured from `console.log`.
    pub fn console_logs(&self) -> Vec<String> {
        self.host_state.borrow().console_logs.clone()
    }

    /// Evaluates JavaScript source code.
    ///
    /// Script errors are returned as `JsError`.
    /// Note: `SandboxConfig.timeout` is stored but not yet enforced due to
    /// boa 0.21 lacking a runtime interrupt API.
    pub fn eval(&mut self, source: &str) -> JsResult<JsValue> {
        let result = self.with_active_host(|context| context.eval(Source::from_bytes(source)));
        // A synchronous evaluator cannot hand control to an embedder while a
        // modal dialog is pending. Boa cancels that suspension; discard the
        // matching host metadata as soon as evaluation returns.
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        result
    }

    /// Evaluates JavaScript while allowing native host calls to suspend until
    /// their [`boa_engine::native_function::NativeCallSuspension`] is resumed.
    ///
    /// The returned future is local to this runtime because its DOM host state
    /// is single-threaded. Native bindings remain associated with this runtime
    /// on every poll, including after a suspended call wakes the evaluation.
    pub fn eval_async<'a>(
        &'a mut self,
        source: &str,
    ) -> impl Future<Output = JsResult<JsValue>> + 'a {
        let source = source.to_owned();
        let host_state = Rc::clone(&self.host_state);
        ActiveHostFuture {
            future: Box::pin(async move {
                let script = Script::parse(Source::from_bytes(&source), None, &mut self.context)?;
                script.evaluate_async(&mut self.context).await
            }),
            host_state,
        }
    }

    /// Evaluates JavaScript source code, converting `JsError` into `Err(String)`.
    ///
    /// This does not catch Rust panics; it only converts JS-level errors.
    pub fn eval_safe(&mut self, source: &str) -> Result<JsValue, String> {
        match self.eval(source) {
            Ok(value) => Ok(value),
            Err(error) => Err(format!("{error}")),
        }
    }

    fn eval_safe_timed(
        &mut self,
        source: &str,
    ) -> (
        Result<JsValue, String>,
        std::time::Duration,
        std::time::Duration,
        std::time::Duration,
    ) {
        let result = self.with_active_host_value(|context| {
            let parse_start = std::time::Instant::now();
            let script = match Script::parse(Source::from_bytes(source), None, context) {
                Ok(script) => script,
                Err(error) => {
                    return (
                        Err(error.to_string()),
                        parse_start.elapsed(),
                        std::time::Duration::ZERO,
                        std::time::Duration::ZERO,
                    );
                }
            };
            let parse_elapsed = parse_start.elapsed();

            let compile_start = std::time::Instant::now();
            if let Err(error) = script.codeblock(context) {
                return (
                    Err(error.to_string()),
                    parse_elapsed,
                    compile_start.elapsed(),
                    std::time::Duration::ZERO,
                );
            }
            let compile_elapsed = compile_start.elapsed();

            let execute_start = std::time::Instant::now();
            let result = script.evaluate(context).map_err(|error| error.to_string());
            (
                result,
                parse_elapsed,
                compile_elapsed,
                execute_start.elapsed(),
            )
        });
        // Like `eval`, this synchronous document-script path cannot yield to
        // an embedder to resolve a modal dialog. Boa cancels the suspension;
        // discard the corresponding host metadata when execution returns.
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        result
    }

    fn eval_module_timed(
        &mut self,
        source: &str,
        url: &str,
    ) -> (
        Result<JsValue, String>,
        std::time::Duration,
        std::time::Duration,
    ) {
        let parse_start = std::time::Instant::now();
        let module = match Module::parse(
            Source::from_reader(source.as_bytes(), Some(Path::new(url))),
            None,
            &mut self.context,
        ) {
            Ok(module) => module,
            Err(error) => {
                return (
                    Err(error.to_string()),
                    parse_start.elapsed(),
                    std::time::Duration::ZERO,
                );
            }
        };
        let parse_elapsed = parse_start.elapsed();
        let execute_start = std::time::Instant::now();
        let promise = module.load_link_evaluate(&mut self.context);
        let result =
            self.run_jobs()
                .map_err(|error| error.to_string())
                .and_then(|()| match promise.state() {
                    PromiseState::Fulfilled(_) => Ok(JsValue::undefined()),
                    PromiseState::Rejected(error) => Err(error.display().to_string()),
                    PromiseState::Pending => Err("module evaluation remained pending".to_string()),
                });
        // Module evaluation is also driven synchronously here. A modal-dialog
        // suspension therefore cannot outlive this call, even when evaluation
        // exits through the pending/error cases above.
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        (result, parse_elapsed, execute_start.elapsed())
    }

    /// Runs pending promise jobs.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        self.with_active_host(|context| context.run_jobs())
    }

    /// Schedules a timeout task from Rust that evaluates `source` as code.
    pub fn set_timeout(&mut self, source: impl Into<String>, delay_ms: u64) -> u64 {
        self.host_state.borrow_mut().event_loop.schedule_timer(
            TimerPayload::Source(source.into()),
            delay_ms,
            false,
        )
    }

    /// Schedules an interval task from Rust that evaluates `source` as code.
    pub fn set_interval(&mut self, source: impl Into<String>, interval_ms: u64) -> u64 {
        self.host_state.borrow_mut().event_loop.schedule_timer(
            TimerPayload::Source(source.into()),
            interval_ms,
            true,
        )
    }

    /// Clears a previously scheduled timer.
    pub fn clear_timer(&mut self, id: u64) {
        self.host_state.borrow_mut().event_loop.clear_timer(id);
    }

    /// Advances the event loop clock and runs due macrotasks and pending jobs.
    ///
    /// Due timers fire in fire-time order (ties broken by registration order).
    /// Both string-source timers and function-callback timers are supported;
    /// callbacks re-scheduled from within a firing callback (e.g. an
    /// `setTimeout(update, delay)` chain) become due on subsequent ticks.
    pub fn tick(&mut self, elapsed_ms: u64) -> JsResult<()> {
        self.host_state.borrow_mut().event_loop.advance(elapsed_ms);
        self.run_until_idle()
    }

    /// Runs queued macrotasks and pending promise jobs until idle.
    ///
    /// Propagates the first error thrown by a timer callback or source string.
    pub fn run_until_idle(&mut self) -> JsResult<()> {
        // An embedder call may have completed a script task and left promise
        // jobs pending. Its checkpoint can itself enqueue host tasks.
        self.run_jobs()?;
        loop {
            let task = { self.host_state.borrow_mut().event_loop.pop_task() };
            let Some((_, task)) = task else {
                break;
            };
            self.run_task(task)?;
            // HTML performs a microtask checkpoint after every task, including
            // host-only navigation tasks that do not directly invoke script.
            self.run_jobs()?;
        }
        Ok(())
    }

    /// Records a page-script error raised while a task ran.
    ///
    /// Bounded so a broken `setInterval` cannot fill memory or bury the first,
    /// most useful error under thousands of repeats. The overflow is counted, so
    /// the drained report never understates how much went wrong.
    fn record_task_error(&mut self, error: String) {
        let mut state = self.host_state.borrow_mut();
        if state.task_errors.len() < MAX_TASK_ERRORS {
            state.task_errors.push(error);
        } else {
            state.suppressed_task_errors = state.suppressed_task_errors.saturating_add(1);
        }
    }

    /// Records `result`'s error, if any, against `label`.
    fn record_error_from<T>(&mut self, label: &str, result: JsResult<T>) {
        if let Err(error) = result {
            self.record_task_error(format!("[{label}] {error}"));
        }
    }

    /// Drains the page-script errors collected while tasks ran.
    ///
    /// Embedders call this after pumping the event loop and report them the same
    /// way they report the errors `execute_document_scripts` returns.
    pub fn take_task_errors(&mut self) -> Vec<String> {
        let mut state = self.host_state.borrow_mut();
        let mut errors = std::mem::take(&mut state.task_errors);
        let suppressed = std::mem::take(&mut state.suppressed_task_errors);
        if suppressed > 0 {
            errors.push(format!("{suppressed} further task errors suppressed"));
        }
        errors
    }

    /// Returns true if any timers are still scheduled (pending or repeating).
    pub fn has_pending_timers(&self) -> bool {
        self.host_state.borrow().event_loop.has_pending_timers()
    }

    fn has_pending_css_transition_work(&self) -> bool {
        self.host_state.borrow().document_styles.values().any(|entry| {
            entry.dirty
                || entry.needs_full_sample
                || entry
                    .resolver
                    .as_ref()
                    .is_some_and(StyleResolver::has_running_transitions)
        })
    }

    /// Runs one rendering opportunity and invokes its animation-frame callbacks.
    ///
    /// Pending macrotasks and promise jobs are drained before the frame starts.
    /// All callbacks present at the start of the frame receive the same
    /// monotonically increasing timestamp and run in registration order.
    /// Callbacks registered while the frame is running are retained for the
    /// next explicit call; [`run_jobs`](Self::run_jobs) alone never invokes
    /// animation-frame callbacks.
    pub fn run_animation_frame(&mut self, elapsed_ms: u64) -> JsResult<usize> {
        self.host_state.borrow_mut().event_loop.advance(elapsed_ms);
        self.run_until_idle()?;
        if self.has_pending_scroll_steps() {
            self.flush_pending_scroll_events()?;
        }

        let (timestamp, callback_ids) = self
            .host_state
            .borrow_mut()
            .event_loop
            .begin_animation_frame();
        let mut callbacks_run = 0;
        let mut first_error = None;

        for id in callback_ids {
            let callback = self
                .host_state
                .borrow_mut()
                .event_loop
                .take_animation_frame_callback(id);
            let Some(callback) = callback else {
                continue;
            };

            let result = self.with_active_host(|context| {
                let callable = callback.as_callable().ok_or_else(|| {
                    JsError::from(
                        JsNativeError::typ()
                            .with_message("animation frame callback is not callable"),
                    )
                })?;
                callable.call(
                    &JsValue::undefined(),
                    &[JsValue::from(timestamp)],
                    context,
                )?;
                Ok(())
            });
            callbacks_run += 1;
            if let Err(error) = result
                && first_error.is_none()
            {
                // Browser callback exceptions are reported without preventing
                // the remaining callbacks in the same frame from running.
                first_error = Some(error);
            }
        }

        // DOM mutations performed by frame callbacks queue observer delivery;
        // the microtask checkpoint completes that work before the embedder
        // proceeds to style/layout/paint.
        let jobs_result = self.run_jobs();
        if let Some(error) = first_error {
            return Err(error);
        }
        jobs_result?;

        // A rendering opportunity samples CSS transitions after animation-frame
        // callbacks and their microtask checkpoint. This also queues transition
        // events without requiring script to force a computed-style read.
        self.update_css_transitions()?;
        // Continue the event-loop iteration for host tasks queued by rAF or
        // rendering callbacks (notably navigation). A newly requested rAF is
        // still retained for the next rendering opportunity.
        self.run_until_idle()?;
        Ok(callbacks_run)
    }

    /// Runs CSSOM View's pending scroll steps for this rendering opportunity.
    /// Taking the set before dispatch ensures a listener that scrolls again
    /// queues work for the next frame instead of recursively dispatching.
    fn flush_pending_scroll_events(&mut self) -> JsResult<usize> {
        // Force layout so style/DOM changes can clamp existing offsets and add
        // their elements to the same pending set as API-driven scrolling.
        self.host_state.borrow_mut().ensure_layout();
        let (document_id, targets) = {
            let mut state = self.host_state.borrow_mut();
            (
                state.document.identity(),
                std::mem::take(&mut state.pending_scroll_targets),
            )
        };
        let count = targets.len();
        for node_id in targets {
            self.eval(&format!(
                "__omoikane_dispatch_scroll_event({node_id}, {})",
                node_id == document_id
            ))?;
        }
        Ok(count)
    }

    /// Returns whether a callback is waiting for the next rendering opportunity.
    pub fn has_pending_animation_frames(&self) -> bool {
        self.host_state
            .borrow()
            .event_loop
            .has_pending_animation_frames()
    }

    fn has_pending_scroll_steps(&self) -> bool {
        let state = self.host_state.borrow();
        !state.pending_scroll_targets.is_empty()
            || !state.scroll_offsets_before_layout.is_empty()
    }

    /// Drives a bounded number of rendering opportunities until no callback is pending.
    ///
    /// Callback errors are logged when script diagnostics are enabled and do
    /// not prevent later frames from settling, matching the render pipeline's
    /// best-effort timer pump.
    pub fn run_animation_frames(
        &mut self,
        max_frames: usize,
        frame_interval_ms: u64,
    ) -> usize {
        let mut callbacks_run = 0;
        for _ in 0..max_frames {
            if !self.has_pending_animation_frames() && !self.has_pending_scroll_steps() {
                break;
            }
            match self.run_animation_frame(frame_interval_ms) {
                Ok(count) => callbacks_run += count,
                Err(error) => {
                    if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
                        eprintln!("[omoikane][animation-frame-error] {error}");
                    }
                }
            }
        }
        callbacks_run
    }

    /// Drives the event loop forward in virtual time, firing due timer tasks
    /// until the timer queue empties or a safety budget is exhausted.
    ///
    /// This is the pipeline-facing pump: it advances a virtual clock in
    /// `step_ms` increments (minimum 1ms), firing `setTimeout`/`setInterval`
    /// callbacks as they come due, and stops as soon as no timers remain. Two
    /// caps guard against runaway pages: `max_virtual_ms` bounds total virtual
    /// time (so a plain `setInterval` cannot spin forever), and `max_tasks`
    /// bounds the total number of timer tasks executed (so a callback that
    /// re-schedules many zero-delay timers cannot explode).
    ///
    /// Unlike [`tick`](Self::tick), individual callback errors are swallowed so
    /// that one throwing timer does not halt the remaining pipeline work.
    ///
    /// Returns the number of timer tasks that were actually executed.
    pub fn run_timers(&mut self, max_virtual_ms: u64, step_ms: u64, max_tasks: usize) -> usize {
        let step = step_ms.max(1);
        let mut advanced: u64 = 0;
        let mut tasks_run: usize = 0;

        while advanced < max_virtual_ms && tasks_run < max_tasks {
            if !self.has_pending_timers() && !self.has_pending_css_transition_work() {
                break;
            }
            self.host_state.borrow_mut().event_loop.advance(step);
            advanced = advanced.saturating_add(step);

            loop {
                if tasks_run >= max_tasks {
                    break;
                }
                let Some((_, task)) = self.host_state.borrow_mut().event_loop.pop_task() else {
                    break;
                };
                let is_timer = matches!(task, Task::Timer(_));
                {
                    // Swallow per-task JS errors: a single failing timer must
                    // not abort the whole pump during rendering. Diagnostics
                    // remain available on demand for complex app bootstraps.
                    let task_kind = match &task {
                        Task::Timer(payload) => payload.kind(),
                        Task::Navigation(_) => "navigation",
                    };
                    let callback_start = std::time::Instant::now();
                    let callback_result = self.run_task(task);
                    let callback_elapsed = callback_start.elapsed();
                    if let Err(error) = callback_result
                        && std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some()
                    {
                        eprintln!("[omoikane][timer-error] {error}");
                    }
                    let jobs_start = std::time::Instant::now();
                    let jobs_result = self.run_jobs();
                    let jobs_elapsed = jobs_start.elapsed();
                    if let Err(error) = jobs_result
                        && std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some()
                    {
                        eprintln!("[omoikane][timer-job-error] {error}");
                    }
                    if std::env::var_os("OMOIKANE_LOG_TIMERS").is_some() {
                        eprintln!(
                            "[omoikane][timer] task={} kind={} callback_ms={:.3} jobs_ms={:.3}",
                            tasks_run,
                            task_kind,
                            callback_elapsed.as_secs_f64() * 1_000.0,
                            jobs_elapsed.as_secs_f64() * 1_000.0,
                        );
                    }
                    if is_timer {
                        tasks_run += 1;
                    }
                }
            }

            // Browsers have rendering opportunities between event-loop tasks,
            // even when a page has not requested an animation-frame callback.
            // Sampling here lets short transitions finish and dispatch events
            // while the embedder is pumping timers (notably testharness.js).
            if let Err(error) = self.update_css_transitions()
                && std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some()
            {
                eprintln!("[omoikane][transition-frame-error] {error}");
            }
        }

        tasks_run
    }

    fn update_css_transitions(&mut self) -> JsResult<()> {
        self.eval("__omoikane_sample_css_transitions()")?;
        self.run_jobs()
    }

    fn run_task(&mut self, task: Task) -> JsResult<()> {
        match task {
            Task::Timer(payload) => self.run_timer_payload(payload),
            Task::Navigation(request) => {
                self.host_state.borrow_mut().navigation_requests.push_back(request);
                Ok(())
            }
        }
    }

    /// Executes a single timer payload: evaluates a source string, or invokes a
    /// retained function callback with its bound extra arguments.
    fn run_timer_payload(&mut self, payload: TimerPayload) -> JsResult<()> {
        match payload {
            // A timer's code is the page's, so its failure is recorded and the
            // loop continues. `run_timers` already worked this way; propagating
            // here made the same page abort navigation when it was driven through
            // `run_until_idle` instead (issue #303).
            TimerPayload::Source(source) => {
                let result = self.eval(&source);
                self.record_error_from("timer", result);
                Ok(())
            }
            TimerPayload::Callback { callback, args } => {
                let result = self.with_active_host(|context| {
                    if let Some(callable) = callback.as_callable() {
                        callable.call(&JsValue::undefined(), &args, context)?;
                    }
                    Ok(())
                });
                self.record_error_from("timer callback", result);
                Ok(())
            }
            TimerPayload::ResourceLoad { node_id } => {
                let (should_dispatch, xhtml_scripts, dynamic_script) = {
                    let mut state = self.host_state.borrow_mut();
                    state.pending_resource_loads.remove(&node_id);
                    let Some(node) = state.get_node(node_id) else {
                        return Ok(());
                    };
                    if document_root_for_node(&node).is_none() {
                        (false, Vec::new(), None)
                    } else {
                        let mut xhtml_scripts = Vec::new();
                        // A dynamically inserted external script is classified by
                        // the same `type` gate the parsed-document path uses, so a
                        // script runs the same way however it reached the tree.
                        let dynamic_script = if node
                            .tag_name()
                            .is_some_and(|tag| tag.eq_ignore_ascii_case("script"))
                        {
                            node.get_attribute("src").map(|src| {
                                let kind = ScriptKind::from_type_attribute(
                                    node.get_attribute("type").as_deref(),
                                );
                                (src, kind, state.base_url.clone())
                            })
                        } else {
                            None
                        };
                        if node
                            .tag_name()
                            .is_some_and(|tag| tag.eq_ignore_ascii_case("iframe"))
                        {
                            // A newly connected iframe starts a fresh navigation.
                            // This also makes detach/reconnect reload rather than
                            // merely replaying the old document's event.
                            let previous = state.iframe_documents.remove(&node_id);
                            if let Some(previous) = previous.as_ref() {
                                state.document_styles.remove(&previous.document.identity());
                                state.unregister_tree(&previous.document);
                            }
                            let document = state.iframe_content_document(&node);
                            let is_xhtml = document
                                .child_nodes()
                                .into_iter()
                                .find(|child| child.node_type() == NodeType::Element)
                                .and_then(|root| root.namespace_uri())
                                .as_deref()
                                == Some("http://www.w3.org/1999/xhtml");
                            if is_xhtml {
                                xhtml_scripts = collect_script_elements(&document)
                                    .into_iter()
                                    .filter(|script| {
                                        script.namespace_uri().as_deref()
                                            == Some("http://www.w3.org/1999/xhtml")
                                    })
                                    .filter(is_inline_classic_script)
                                    .map(|script| collect_text_content(&script))
                                    .collect();
                            }
                            // JS wrappers are cached by native identity, so keep
                            // the old Rc alive until its replacement has been
                            // allocated and cannot reuse the same address.
                            drop(previous);
                        }
                        (true, xhtml_scripts, dynamic_script)
                    }
                };
                for source in xhtml_scripts {
                    // Like top-level document scripts, one failing XHTML
                    // script must not prevent later scripts or the iframe load
                    // event from running.
                    let _ = self.eval(&source);
                    let _ = self.run_jobs();
                }
                // A script whose type Omoikane does not execute is not fetched and
                // does not load, so it must not go on to dispatch `load` either.
                let mut dispatch_load = should_dispatch;
                if let Some((src, kind, base_url)) = dynamic_script {
                    let log_scripts = std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some();
                    if kind == ScriptKind::NotExecutable {
                        if log_scripts {
                            eprintln!("[omoikane][script] skipped dynamic {src}");
                        }
                        dispatch_load = false;
                    } else {
                        if log_scripts {
                            eprintln!("[omoikane][script] loading dynamic {src} kind={kind:?}");
                        }
                        let source = {
                            let mut state = self.host_state.borrow_mut();
                            fetch_script_source_with_client(
                                &src,
                                base_url.as_ref(),
                                &mut state.http_client,
                            )
                        };
                        match source {
                            // Every failure below is the page's, not the engine's:
                            // it is recorded and execution continues, exactly as
                            // `execute_document_scripts` treats a parsed script.
                            // Propagating instead would abort the event loop and,
                            // through it, the whole navigation.
                            None => {
                                // A script that never arrived did not load: it
                                // fires `error` instead, so a loader waiting on
                                // one of the two is not left with neither.
                                self.record_task_error(format!(
                                    "[dynamic script: {src}] failed to fetch"
                                ));
                                dispatch_load = false;
                                let dispatched = self.eval(&format!(
                                    "__omoikane_dispatch_resource_error({node_id})"
                                ));
                                self.record_error_from(&src, dispatched);
                                let jobs = self.run_jobs();
                                self.record_error_from(&src, jobs);
                            }
                            Some(source) => {
                                let marked = self
                                    .eval(&format!("__omoikane_set_current_script({node_id})"));
                                self.record_error_from(&src, marked);
                                let result = match kind {
                                    ScriptKind::Module => self
                                        .eval_module_timed(
                                            &source,
                                            &module_script_url(&src, base_url.as_ref(), false),
                                        )
                                        .0,
                                    _ => self
                                        .eval(&source)
                                        .map_err(|error| error.to_string())
                                        .map(|_| JsValue::undefined()),
                                };
                                let _ = self.eval("__omoikane_set_current_script(null)");
                                if let Err(error) = result {
                                    let context = script_source_context(&source);
                                    self.record_task_error(format!(
                                        "[dynamic script: {src}; {context}] {error}"
                                    ));
                                }
                                let jobs = self.run_jobs();
                                self.record_error_from(&src, jobs);
                                if log_scripts {
                                    eprintln!("[omoikane][script] completed dynamic {src}");
                                }
                            }
                        }
                    }
                }
                if dispatch_load {
                    let dispatched =
                        self.eval(&format!("__omoikane_dispatch_resource_load({node_id})"));
                    self.record_error_from("resource load", dispatched);
                    let jobs = self.run_jobs();
                    self.record_error_from("resource load", jobs);
                }
                Ok(())
            }
        }
    }

    /// Dispatches a `DOMContentLoaded` event on the document.
    ///
    /// Call this after the DOM tree is fully constructed (e.g., after parsing HTML
    /// and executing inline scripts). Listeners registered via
    /// `document.addEventListener('DOMContentLoaded', fn)` will be invoked.
    pub fn fire_dom_content_loaded(&mut self) -> JsResult<()> {
        self.eval(
            "document.__readyState = 'interactive'; document.dispatchEvent(new Event('DOMContentLoaded'))",
        )?;
        self.run_jobs()
    }

    /// Dispatches a named event on the document.
    ///
    /// The event type is escaped to prevent JS injection from untrusted input.
    pub fn fire_document_event(&mut self, event_type: &str) -> JsResult<()> {
        let escaped = event_type
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        self.eval(&format!("document.dispatchEvent(new Event('{}'))", escaped))?;
        self.run_jobs()
    }

    /// Wires `on*` inline event-handler content attributes to event listeners.
    ///
    /// Walks the document tree and, for every element attribute whose name
    /// starts with `on` (e.g. `onload`, `onclick`), compiles the attribute
    /// value as the body of `function (event) { ... }` and registers it as an
    /// event listener for the corresponding event type. The window-reflected
    /// events on `<body>`/`<frameset>` (`load`, `unload`, `resize`, ...) are
    /// registered on the Window, so `<body onload="...">` fires when the `load`
    /// event is dispatched; every other handler is registered on its element.
    ///
    /// Call this after the DOM is built (typically after running scripts and
    /// before firing `load`). It is a no-op for attributes whose value fails to
    /// compile.
    pub fn wire_inline_event_handlers(&mut self) -> JsResult<()> {
        self.eval("__omoikane_wire_inline_handlers()")?;
        self.run_jobs()
    }

    /// Dispatches the `load` event on the Window (and thus the document).
    ///
    /// In the load pipeline this fires after scripts have executed and
    /// `DOMContentLoaded` has been dispatched, matching the HTML spec ordering
    /// (scripts → `DOMContentLoaded` → resource loads → `load`). Combined with
    /// [`wire_inline_event_handlers`](Self::wire_inline_event_handlers), a
    /// page's `<body onload="...">` handler runs at this point.
    pub fn fire_load(&mut self) -> JsResult<()> {
        // The load event does not bubble.
        self.eval(
            "document.__readyState = 'complete'; window.dispatchEvent(new Event('load', { bubbles: false }))",
        )?;
        self.run_jobs()
    }

    /// Collects and executes all `<script>` elements in the document.
    ///
    /// - Inline scripts: text content is executed directly.
    /// - External scripts (`src` attribute): fetched via HTTP and executed.
    /// - `type` selects how the element runs: absent, empty, `text/javascript` or
    ///   `application/javascript` runs as a classic script, `module` runs as an ES
    ///   module, and every other value is not executed at all.
    /// - `defer` scripts are collected and executed after all inline/sync scripts.
    /// - After all scripts, `DOMContentLoaded` is fired.
    ///
    /// Errors in individual scripts are logged but do not stop execution of remaining scripts.
    pub fn execute_document_scripts(&mut self, base_url: Option<&crate::http::Url>) -> Vec<String> {
        // Record the base URL so relative resource references discovered later
        // (e.g. an `<iframe src="empty.html">` whose contentDocument is accessed
        // during the timer loop) can be resolved.
        if let Some(base) = base_url {
            self.host_state.borrow_mut().base_url = Some(base.clone());
        }
        let _ = self.eval("__omoikane_install_window_named_properties()");
        let _ = self.eval("document.__readyState = 'loading'");

        let document = self.document();
        let scripts = collect_script_elements(&document);
        let mut errors = Vec::new();
        let mut deferred = Vec::new();
        let log_scripts = std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some();

        for (script_index, script) in scripts.iter().enumerate() {
            let attrs = script.attributes().unwrap_or_default();
            let is_module = attrs
                .get("type")
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("module"));

            // Skip the types Omoikane does not execute at all. Modules are not
            // among them — they run below through `eval_module_timed` — so this
            // only filters values like `application/json` or an import map.
            // Shares the type gate with `is_inline_classic_script` and with the
            // dynamic-insertion path in `run_timer_payload`, so a script executes
            // identically however it reached the tree.
            if !is_module
                && !is_executable_classic_script_type(attrs.get("type").map(|s| s.as_str()))
            {
                if log_scripts {
                    eprintln!(
                        "[omoikane][script] skipped type={:?} src={:?}",
                        attrs.get("type"),
                        attrs.get("src")
                    );
                }
                continue;
            }

            let src = attrs.get("src").cloned();
            let has_src = src.is_some();
            // HTML spec: defer only applies to external (src) scripts.
            let is_defer = is_module || (attrs.contains_key("defer") && src.is_some());

            let (source_code, script_label) = if let Some(src_url) = src {
                // External script: fetch
                let fetch_start = std::time::Instant::now();
                let fetched = {
                    let mut state = self.host_state.borrow_mut();
                    fetch_script_source_with_client(
                        &src_url,
                        base_url,
                        &mut state.http_client,
                    )
                };
                match fetched {
                    Some(code) => {
                        if log_scripts {
                            eprintln!(
                                "[omoikane][script] fetched {src_url} elapsed_ms={:.3}",
                                fetch_start.elapsed().as_secs_f64() * 1_000.0,
                            );
                        }
                        (code, src_url.clone())
                    }
                    None => {
                        errors.push(format!("failed to fetch script: {src_url}"));
                        continue;
                    }
                }
            } else {
                // Inline script: collect text content
                (
                    collect_text_content(script),
                    format!("inline-script-{}", script_index + 1),
                )
            };

            if source_code.trim().is_empty() {
                continue;
            }

            let module_url =
                is_module.then(|| module_script_url(&script_label, base_url, !has_src));

            if log_scripts {
                eprintln!(
                    "[omoikane][script] queued {script_label} ({} chars, defer={is_defer})",
                    source_code.chars().count()
                );
            }

            if is_defer {
                // Keep the <script> node alongside its source so the deferred
                // execution loop below can point `document.write`'s insertion
                // reference at this script (exactly like the inline path), rather
                // than letting a deferred write() fall back to appending at
                // <body>.
                deferred.push((source_code, script.clone(), script_label, module_url));
                continue;
            }

            // Point `document.write`'s insertion reference at this script so any
            // content it writes lands as the script's following siblings (the
            // HTML tokenizer inserts written text at the "insertion point",
            // i.e. right where the running <script> sits in the tree).
            self.host_state.borrow_mut().write_insertion_ref = Some(script.clone());
            let _ = self.eval(&format!(
                "__omoikane_set_current_script({})",
                script.identity()
            ));
            // Execute immediately
            let script_context = script_source_context(&source_code);
            let (eval_result, parse_elapsed, compile_elapsed, execute_elapsed) =
                self.eval_safe_timed(&source_code);
            if let Err(err) = eval_result {
                errors.push(format!("[script: {script_label}; {script_context}] {err}"));
            }
            let jobs_start = std::time::Instant::now();
            let jobs_result = self.run_jobs();
            let jobs_elapsed = jobs_start.elapsed();
            if let Err(err) = jobs_result {
                errors.push(format!("[script jobs: {script_label}] {err}"));
            }
            if log_scripts {
                eprintln!(
                    "[omoikane][script] completed {script_label} parse_ms={:.3} compile_ms={:.3} execute_ms={:.3} jobs_ms={:.3}",
                    parse_elapsed.as_secs_f64() * 1_000.0,
                    compile_elapsed.as_secs_f64() * 1_000.0,
                    execute_elapsed.as_secs_f64() * 1_000.0,
                    jobs_elapsed.as_secs_f64() * 1_000.0,
                );
            }

            // The insertion point and currentScript are only defined while a script runs.
            let _ = self.eval("__omoikane_set_current_script(null)");
            self.host_state.borrow_mut().write_insertion_ref = None;
        }

        // Execute deferred scripts. Each runs with its own insertion point set
        // to its <script> element, so a `document.write` from a deferred script
        // lands as that script's following siblings — the same treatment the
        // inline path applies above.
        for (source_code, script, script_label, module_url) in deferred {
            if log_scripts {
                eprintln!("[omoikane][script] running deferred {script_label}");
            }
            self.host_state.borrow_mut().write_insertion_ref = Some(script.clone());
            let _ = self.eval(&format!(
                "__omoikane_set_current_script({})",
                script.identity()
            ));
            let script_context = script_source_context(&source_code);
            let (result, parse_elapsed, compile_elapsed, execute_elapsed) =
                if let Some(module_url) = module_url {
                    let (result, parse_elapsed, execute_elapsed) =
                        self.eval_module_timed(&source_code, &module_url);
                    (
                        result,
                        parse_elapsed,
                        std::time::Duration::ZERO,
                        execute_elapsed,
                    )
                } else {
                    self.eval_safe_timed(&source_code)
                };
            if let Err(err) = result {
                errors.push(format!("[script: {script_label}; {script_context}] {err}"));
            }
            let jobs_start = std::time::Instant::now();
            let jobs_result = self.run_jobs();
            let jobs_elapsed = jobs_start.elapsed();
            if let Err(err) = jobs_result {
                errors.push(format!("[script jobs: {script_label}] {err}"));
            }
            if log_scripts {
                eprintln!(
                    "[omoikane][script] completed deferred {script_label} parse_ms={:.3} compile_ms={:.3} execute_ms={:.3} jobs_ms={:.3}",
                    parse_elapsed.as_secs_f64() * 1_000.0,
                    compile_elapsed.as_secs_f64() * 1_000.0,
                    execute_elapsed.as_secs_f64() * 1_000.0,
                    jobs_elapsed.as_secs_f64() * 1_000.0,
                );
            }
            let _ = self.eval("__omoikane_set_current_script(null)");
            self.host_state.borrow_mut().write_insertion_ref = None;
        }

        // Fire DOMContentLoaded
        if let Err(err) = self.fire_dom_content_loaded() {
            errors.push(format!("{err}"));
        }

        errors
    }

    fn with_active_host<T>(&mut self, f: impl FnOnce(&mut Context) -> JsResult<T>) -> JsResult<T> {
        self.with_active_host_value(f)
    }

    fn with_active_host_value<T>(&mut self, f: impl FnOnce(&mut Context) -> T) -> T {
        let _guard = activate_host_state(Rc::clone(&self.host_state));
        f(&mut self.context)
    }
}

fn script_source_context(source: &str) -> String {
    let preview: String = source
        .chars()
        .take(160)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    format!("{} chars; starts with: {preview}", source.chars().count())
}

fn module_script_url(
    script_label: &str,
    base_url: Option<&crate::http::Url>,
    inline: bool,
) -> String {
    if inline {
        return base_url
            .map(|base| {
                let base = base.to_string();
                let base = base.split_once('#').map_or(base.as_str(), |(head, _)| head);
                format!("{base}#{script_label}")
            })
            .unwrap_or_else(|| script_label.to_string());
    }

    match resolve_resource_ref(script_label, base_url) {
        Some(ResolvedResource::Url(url)) => url,
        _ => script_label.to_string(),
    }
}

/// Collects all `<script>` elements from the document tree in document order.
fn collect_script_elements(node: &NodeHandle) -> Vec<NodeHandle> {
    let mut scripts = Vec::new();
    collect_script_elements_recursive(node, &mut scripts);
    scripts
}

fn collect_script_elements_recursive(node: &NodeHandle, out: &mut Vec<NodeHandle>) {
    if node.tag_name().as_deref() == Some("script") {
        out.push(node.clone());
        return; // Don't recurse into <script> children
    }
    for child in node.child_nodes() {
        collect_script_elements_recursive(&child, out);
    }
}

/// Collects text content from a node's text-node children (for inline script content).
/// Only includes Text nodes, not comments or other node types.
fn collect_text_content(node: &NodeHandle) -> String {
    use crate::dom::NodeType;
    let mut text = String::new();
    for child in node.child_nodes() {
        if child.node_type() == NodeType::Text {
            if let Some(data) = child.data() {
                text.push_str(&data);
            }
        } else {
            text.push_str(&collect_text_content(&child));
        }
    }
    text
}

/// A resource reference (`src`) resolved to something fetchable.
enum ResolvedResource {
    /// A `data:` URI decoded inline (RFC 2397): the parsed media type and bytes.
    Data { mime_type: String, data: Vec<u8> },
    /// An absolute `http:`/`https:` URL to fetch over the network.
    Url(String),
}

/// Resolves a resource reference (`src`) to either inline `data:` bytes or an
/// absolute HTTP(S) URL. Shared by iframe document loading
/// ([`HostState::load_iframe_document`]) and external script loading
/// ([`fetch_script_source`]) so the classification stays in one place.
///
/// `src` is classified case-insensitively:
/// - a `data:` URI is decoded inline;
/// - an absolute `http://`/`https://` URL is used verbatim;
/// - anything else is treated as a relative reference and resolved against
///   `base_url`.
///
/// Returns `None` when a `data:` URI fails to parse, or when a relative
/// reference cannot be resolved (no base URL, or a resolution error).
fn resolve_resource_ref(
    src: &str,
    base_url: Option<&crate::http::Url>,
) -> Option<ResolvedResource> {
    if src
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        let parsed = crate::http::parse_data_uri(src)?;
        return Some(ResolvedResource::Data {
            mime_type: parsed.mime_type,
            data: parsed.data,
        });
    }

    // Scheme match is case-insensitive: `HTTP://…` must not fall through to
    // relative resolution.
    let is_absolute_http = src
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || src
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));

    if is_absolute_http {
        Some(ResolvedResource::Url(src.to_string()))
    } else {
        let base = base_url?;
        let url = crate::http::url::resolve_url(base, src).ok()?;
        Some(ResolvedResource::Url(url.to_string()))
    }
}

fn same_origin_url(a: &crate::http::Url, b: &crate::http::Url) -> bool {
    a.scheme() == b.scheme() && a.host() == b.host() && a.port() == b.port()
}

fn requires_public_fetch(url: &crate::http::Url, base_url: Option<&crate::http::Url>) -> bool {
    base_url.is_none_or(|base| !same_origin_url(url, base))
}

/// Fetches an external script's source.
///
/// Supports `http:`/`https:` (fetched over the network) and `data:` URIs
/// (decoded inline via [`crate::http::parse_data_uri`], RFC 2397). Relative
/// references are resolved against `base_url` when provided.
///
/// For `data:` URIs, only JavaScript media types (see
/// [`is_javascript_mime_type`]) are executed as classic scripts; a `data:` URI
/// with a non-JavaScript media type returns `None` (treated as a fetch
/// failure), matching how browsers refuse to run non-script `data:` sources.
#[cfg(test)]
fn fetch_script_source(src: &str, base_url: Option<&crate::http::Url>) -> Option<String> {
    fetch_script_source_with_client(src, base_url, &mut Client::new())
}

/// Fetches script source through the supplied page-scoped HTTP client.
///
/// Keeping one client for a document preserves cookies and allows its
/// connection pool to reuse TLS connections across external scripts. The
/// standalone wrapper above remains useful for focused URI-decoding tests.
fn fetch_script_source_with_client(
    src: &str,
    base_url: Option<&crate::http::Url>,
    client: &mut Client,
) -> Option<String> {
    match resolve_resource_ref(src, base_url)? {
        // data: URI scripts are decoded inline without a network fetch. Only
        // JavaScript media types are executed as classic scripts; any other
        // media type is treated as a fetch failure (returns None), matching how
        // browsers refuse to run non-script `data:` sources.
        ResolvedResource::Data { mime_type, data } => {
            if !is_javascript_mime_type(&mime_type) {
                return None;
            }
            String::from_utf8(data).ok()
        }
        ResolvedResource::Url(url) => {
            let resolved = url.parse::<crate::http::Url>().ok()?;
            let public_only = requires_public_fetch(&resolved, base_url);
            let response = if public_only {
                client.get_public(&url).ok()?
            } else {
                client.get(&url).ok()?
            };
            // Scripts require a successful (200) response; error pages are not
            // executed. This differs deliberately from iframe loading, which
            // adopts even an error response's body as the sub-document.
            if response.status_code() != 200 {
                return None;
            }
            std::str::from_utf8(response.body())
                .ok()
                .map(|s| s.to_string())
        }
    }
}

/// Returns whether `mime` is a JavaScript media type per the HTML spec's
/// "JavaScript MIME type essence match" (case-insensitive, parameters already
/// stripped). Only these types are executed as classic scripts.
fn is_javascript_mime_type(mime: &str) -> bool {
    matches!(
        mime.trim().to_ascii_lowercase().as_str(),
        "application/ecmascript"
            | "application/javascript"
            | "application/x-ecmascript"
            | "application/x-javascript"
            | "text/ecmascript"
            | "text/javascript"
            | "text/javascript1.0"
            | "text/javascript1.1"
            | "text/javascript1.2"
            | "text/javascript1.3"
            | "text/javascript1.4"
            | "text/javascript1.5"
            | "text/jscript"
            | "text/livescript"
            | "text/x-ecmascript"
            | "text/x-javascript"
    )
}

fn register_host_bindings(
    context: &mut Context,
    host_state: &Rc<RefCell<HostState>>,
) -> JsResult<()> {
    let state = host_state.borrow();
    context.register_global_property(
        js_string!("__omoikane_document_id"),
        state.document.identity() as f64,
        boa_engine::property::Attribute::all(),
    )?;
    context.register_global_property(
        js_string!("__omoikane_location_href"),
        js_string!(state.location_href.as_str()),
        boa_engine::property::Attribute::all(),
    )?;
    context.register_global_property(
        js_string!("__omoikane_navigator_user_agent"),
        js_string!(state.navigator_user_agent.as_str()),
        boa_engine::property::Attribute::all(),
    )?;
    context.register_global_property(
        js_string!("__omoikane_performance_time_origin"),
        state.performance_time_origin,
        boa_engine::property::Attribute::READONLY
            | boa_engine::property::Attribute::NON_ENUMERABLE
            | boa_engine::property::Attribute::PERMANENT,
    )?;
    drop(state);

    for (name, length, function) in [
        (
            js_string!("__omoikane_open_javascript_dialog"),
            3,
            NativeFunction::from_copy_closure(open_javascript_dialog_native),
        ),
        (
            js_string!("__omoikane_performance_now"),
            0,
            NativeFunction::from_copy_closure(performance_now_native),
        ),
        (
            js_string!("__omoikane_crypto_random"),
            1,
            NativeFunction::from_copy_closure(crypto_random_native),
        ),
        (
            js_string!("__omoikane_crypto_digest"),
            2,
            NativeFunction::from_copy_closure(crypto_digest_native),
        ),
        (
            js_string!("__omoikane_storage_origin"),
            1,
            NativeFunction::from_copy_closure(storage_origin_native),
        ),
        (
            js_string!("__omoikane_storage_length"),
            2,
            NativeFunction::from_copy_closure(storage_length_native),
        ),
        (
            js_string!("__omoikane_storage_key"),
            3,
            NativeFunction::from_copy_closure(storage_key_native),
        ),
        (
            js_string!("__omoikane_storage_get"),
            3,
            NativeFunction::from_copy_closure(storage_get_native),
        ),
        (
            js_string!("__omoikane_storage_set"),
            4,
            NativeFunction::from_copy_closure(storage_set_native),
        ),
        (
            js_string!("__omoikane_storage_remove"),
            3,
            NativeFunction::from_copy_closure(storage_remove_native),
        ),
        (
            js_string!("__omoikane_storage_clear"),
            2,
            NativeFunction::from_copy_closure(storage_clear_native),
        ),
        (
            js_string!("__omoikane_query_selector"),
            2,
            NativeFunction::from_copy_closure(query_selector_native),
        ),
        (
            js_string!("__omoikane_create_element"),
            1,
            NativeFunction::from_copy_closure(create_element_native),
        ),
        (
            js_string!("__omoikane_is_valid_xml_name"),
            1,
            NativeFunction::from_copy_closure(is_valid_xml_name_native),
        ),
        (
            js_string!("__omoikane_append_child"),
            2,
            NativeFunction::from_copy_closure(append_child_native),
        ),
        (
            js_string!("__omoikane_parent_node"),
            1,
            NativeFunction::from_copy_closure(parent_node_native),
        ),
        (
            js_string!("__omoikane_node_name"),
            1,
            NativeFunction::from_copy_closure(node_name_native),
        ),
        (
            js_string!("__omoikane_node_local_name"),
            1,
            NativeFunction::from_copy_closure(node_local_name_native),
        ),
        (
            js_string!("__omoikane_node_namespace_uri"),
            1,
            NativeFunction::from_copy_closure(node_namespace_uri_native),
        ),
        (
            js_string!("__omoikane_node_prefix"),
            1,
            NativeFunction::from_copy_closure(node_prefix_native),
        ),
        (
            js_string!("__omoikane_doctype_public_id"),
            1,
            NativeFunction::from_copy_closure(doctype_public_id_native),
        ),
        (
            js_string!("__omoikane_doctype_system_id"),
            1,
            NativeFunction::from_copy_closure(doctype_system_id_native),
        ),
        (
            js_string!("__omoikane_get_attribute"),
            2,
            NativeFunction::from_copy_closure(get_attribute_native),
        ),
        (
            js_string!("__omoikane_attribute_names"),
            1,
            NativeFunction::from_copy_closure(attribute_names_native),
        ),
        (
            js_string!("__omoikane_set_attribute"),
            3,
            NativeFunction::from_copy_closure(set_attribute_native),
        ),
        (
            js_string!("__omoikane_get_checked"),
            1,
            NativeFunction::from_copy_closure(get_checked_native),
        ),
        (
            js_string!("__omoikane_set_checked"),
            2,
            NativeFunction::from_copy_closure(set_checked_native),
        ),
        (
            js_string!("__omoikane_set_text_control_state"),
            5,
            NativeFunction::from_copy_closure(set_text_control_state_native),
        ),
        (
            js_string!("__omoikane_console_log"),
            1,
            NativeFunction::from_copy_closure(console_log_native),
        ),
        (
            js_string!("setTimeout"),
            2,
            NativeFunction::from_copy_closure(set_timeout_native),
        ),
        (
            js_string!("setInterval"),
            2,
            NativeFunction::from_copy_closure(set_interval_native),
        ),
        (
            js_string!("clearTimeout"),
            1,
            NativeFunction::from_copy_closure(clear_timer_native),
        ),
        (
            js_string!("clearInterval"),
            1,
            NativeFunction::from_copy_closure(clear_timer_native),
        ),
        (
            js_string!("requestAnimationFrame"),
            1,
            NativeFunction::from_copy_closure(request_animation_frame_native),
        ),
        (
            js_string!("cancelAnimationFrame"),
            1,
            NativeFunction::from_copy_closure(cancel_animation_frame_native),
        ),
        (
            js_string!("__omoikane_fetch"),
            4,
            NativeFunction::from_copy_closure(fetch_native),
        ),
        (
            js_string!("__omoikane_register_object_url"),
            3,
            NativeFunction::from_copy_closure(register_object_url_native),
        ),
        (
            js_string!("__omoikane_revoke_object_url"),
            1,
            NativeFunction::from_copy_closure(revoke_object_url_native),
        ),
        (
            js_string!("__omoikane_queue_file_reading_task"),
            1,
            NativeFunction::from_copy_closure(queue_file_reading_task_native),
        ),
        (
            js_string!("__omoikane_queue_networking_task"),
            1,
            NativeFunction::from_copy_closure(queue_networking_task_native),
        ),
        (
            js_string!("__omoikane_canvas_commit"),
            4,
            NativeFunction::from_copy_closure(canvas_commit_native),
        ),
        (
            js_string!("__omoikane_canvas_data_url"),
            1,
            NativeFunction::from_copy_closure(canvas_data_url_native),
        ),
        (
            js_string!("__omoikane_canvas_image_source"),
            1,
            NativeFunction::from_copy_closure(canvas_image_source_native),
        ),
        (
            js_string!("__omoikane_websocket_connect"),
            2,
            NativeFunction::from_copy_closure(websocket_connect_native),
        ),
        (
            js_string!("__omoikane_websocket_send"),
            3,
            NativeFunction::from_copy_closure(websocket_send_native),
        ),
        (
            js_string!("__omoikane_websocket_poll"),
            1,
            NativeFunction::from_copy_closure(websocket_poll_native),
        ),
        (
            js_string!("__omoikane_websocket_close"),
            3,
            NativeFunction::from_copy_closure(websocket_close_native),
        ),
        (
            js_string!("__omoikane_event_source_fetch"),
            3,
            NativeFunction::from_copy_closure(event_source_fetch_native),
        ),
        (
            js_string!("__omoikane_get_text_content"),
            1,
            NativeFunction::from_copy_closure(get_text_content_native),
        ),
        (
            js_string!("__omoikane_set_text_content"),
            2,
            NativeFunction::from_copy_closure(set_text_content_native),
        ),
        (
            js_string!("__omoikane_get_inner_html"),
            1,
            NativeFunction::from_copy_closure(get_inner_html_native),
        ),
        (
            js_string!("__omoikane_set_inner_html"),
            2,
            NativeFunction::from_copy_closure(set_inner_html_native),
        ),
        (
            js_string!("__omoikane_child_node_ids"),
            1,
            NativeFunction::from_copy_closure(child_node_ids_native),
        ),
        (
            js_string!("__omoikane_next_sibling"),
            1,
            NativeFunction::from_copy_closure(next_sibling_native),
        ),
        (
            js_string!("__omoikane_previous_sibling"),
            1,
            NativeFunction::from_copy_closure(previous_sibling_native),
        ),
        (
            js_string!("__omoikane_remove_child"),
            2,
            NativeFunction::from_copy_closure(remove_child_native),
        ),
        (
            js_string!("__omoikane_insert_before"),
            3,
            NativeFunction::from_copy_closure(insert_before_native),
        ),
        (
            js_string!("__omoikane_query_selector_all"),
            2,
            NativeFunction::from_copy_closure(query_selector_all_native),
        ),
        (
            js_string!("__omoikane_matches_selector"),
            2,
            NativeFunction::from_copy_closure(matches_selector_native),
        ),
        (
            js_string!("__omoikane_node_type"),
            1,
            NativeFunction::from_copy_closure(node_type_native),
        ),
        (
            js_string!("__omoikane_clone_node"),
            2,
            NativeFunction::from_copy_closure(clone_node_native),
        ),
        (
            js_string!("__omoikane_remove_attribute"),
            2,
            NativeFunction::from_copy_closure(remove_attribute_native),
        ),
        (
            js_string!("__omoikane_create_text_node"),
            1,
            NativeFunction::from_copy_closure(create_text_node_native),
        ),
        (
            js_string!("__omoikane_create_document_fragment"),
            0,
            NativeFunction::from_copy_closure(create_document_fragment_native),
        ),
        (
            js_string!("__omoikane_template_content"),
            1,
            NativeFunction::from_copy_closure(template_content_native),
        ),
        (
            js_string!("__omoikane_attach_shadow"),
            2,
            NativeFunction::from_copy_closure(attach_shadow_native),
        ),
        (
            js_string!("__omoikane_shadow_root"),
            1,
            NativeFunction::from_copy_closure(shadow_root_native),
        ),
        (
            js_string!("__omoikane_shadow_host"),
            1,
            NativeFunction::from_copy_closure(shadow_host_native),
        ),
        (
            js_string!("__omoikane_shadow_mode"),
            1,
            NativeFunction::from_copy_closure(shadow_mode_native),
        ),
        (
            js_string!("__omoikane_assigned_slot"),
            1,
            NativeFunction::from_copy_closure(assigned_slot_native),
        ),
        (
            js_string!("__omoikane_internal_assigned_slot"),
            1,
            NativeFunction::from_copy_closure(internal_assigned_slot_native),
        ),
        (
            js_string!("__omoikane_assigned_nodes"),
            2,
            NativeFunction::from_copy_closure(assigned_nodes_native),
        ),
        (
            js_string!("__omoikane_create_document"),
            0,
            NativeFunction::from_copy_closure(create_document_native),
        ),
        (
            js_string!("__omoikane_create_document_type"),
            1,
            NativeFunction::from_copy_closure(create_document_type_native),
        ),
        (
            js_string!("__omoikane_create_processing_instruction"),
            2,
            NativeFunction::from_copy_closure(create_processing_instruction_native),
        ),
        (
            js_string!("__omoikane_create_comment"),
            1,
            NativeFunction::from_copy_closure(create_comment_native),
        ),
        (
            js_string!("__omoikane_computed_style"),
            1,
            NativeFunction::from_copy_closure(computed_style_native),
        ),
        (
            js_string!("__omoikane_is_rendered_for_focus"),
            1,
            NativeFunction::from_copy_closure(is_rendered_for_focus_native),
        ),
        (
            js_string!("__omoikane_is_actually_disabled"),
            1,
            NativeFunction::from_copy_closure(is_actually_disabled_native),
        ),
        (
            js_string!("__omoikane_normalize_style_value"),
            2,
            NativeFunction::from_copy_closure(normalize_style_value_native),
        ),
        (
            js_string!("__omoikane_take_transition_events"),
            0,
            NativeFunction::from_copy_closure(take_transition_events_native),
        ),
        (
            js_string!("__omoikane_sample_css_transition_styles"),
            0,
            NativeFunction::from_copy_closure(sample_css_transition_styles_native),
        ),
        (
            js_string!("__omoikane_layout_metrics"),
            1,
            NativeFunction::from_copy_closure(layout_metrics_native),
        ),
        (
            js_string!("__omoikane_element_scroll_offset"),
            1,
            NativeFunction::from_copy_closure(element_scroll_offset_native),
        ),
        (
            js_string!("__omoikane_set_element_scroll"),
            3,
            NativeFunction::from_copy_closure(set_element_scroll_native),
        ),
        (
            js_string!("__omoikane_window_scroll_offset"),
            0,
            NativeFunction::from_copy_closure(window_scroll_offset_native),
        ),
        (
            js_string!("__omoikane_set_window_scroll"),
            2,
            NativeFunction::from_copy_closure(set_window_scroll_native),
        ),
        (
            js_string!("__omoikane_css_rule_count"),
            1,
            NativeFunction::from_copy_closure(css_rule_count_native),
        ),
        (
            js_string!("__omoikane_css_supports"),
            2,
            NativeFunction::from_copy_closure(css_supports_native),
        ),
        (
            js_string!("__omoikane_css_supports_condition"),
            1,
            NativeFunction::from_copy_closure(css_supports_condition_native),
        ),
        (
            js_string!("__omoikane_match_media"),
            1,
            NativeFunction::from_copy_closure(match_media_native),
        ),
        (
            // (documentId, text) — the target document id plus the markup to
            // write, so a write to an iframe sub-document routes correctly.
            js_string!("__omoikane_document_write"),
            2,
            NativeFunction::from_copy_closure(document_write_native),
        ),
        (
            js_string!("__omoikane_iframe_content_document"),
            1,
            NativeFunction::from_copy_closure(iframe_content_document_native),
        ),
        (
            js_string!("__omoikane_owner_document"),
            1,
            NativeFunction::from_copy_closure(owner_document_native),
        ),
        (
            js_string!("__omoikane_document_owner_iframe"),
            1,
            NativeFunction::from_copy_closure(document_owner_iframe_native),
        ),
        (
            js_string!("__omoikane_document_reset"),
            1,
            NativeFunction::from_copy_closure(document_reset_native),
        ),
        (
            js_string!("__omoikane_resolve_url"),
            1,
            NativeFunction::from_copy_closure(resolve_url_native),
        ),
        (
            js_string!("__omoikane_schedule_navigation"),
            3,
            NativeFunction::from_copy_closure(schedule_navigation_native),
        ),
        (
            js_string!("__omoikane_submit_form"),
            4,
            NativeFunction::from_copy_closure(submit_form_native),
        ),
    ] {
        context.register_global_builtin_callable(name, length, function)?;
    }

    Ok(())
}

fn default_document() -> NodeHandle {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    document.append_child(html.clone());
    html.append_child(body);
    document
}

/// Builds an empty `about:blank`-equivalent HTML document
/// (`<html><head></head><body></body></html>`).
///
/// Used for iframes with no `src` and for iframe resources that must not be
/// parsed as HTML. The document always has an `<html>` document element with
/// `<head>` and `<body>` children so callers relying on `documentElement`,
/// `head`, and `body` never observe a missing node.
fn blank_html_document() -> NodeHandle {
    let document = NodeHandle::document();
    let html = NodeHandle::element("html");
    let head = NodeHandle::element("head");
    let body = NodeHandle::element("body");
    html.append_child(head);
    html.append_child(body);
    document.append_child(html);
    document
}

/// Returns whether `content_type` denotes an HTML document that a browsing
/// context should parse into a DOM tree. Parameters (e.g. `; charset=utf-8`)
/// are stripped and the essence is matched case-insensitively.
fn is_html_mime_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    essence == "text/html"
}

fn is_xml_mime_type(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or("").trim();
    matches!(
        essence.to_ascii_lowercase().as_str(),
        "text/xml" | "application/xml" | "image/svg+xml" | "application/xhtml+xml"
    )
}

fn parse_node_id(value: Option<&JsValue>, context: &mut Context) -> JsResult<usize> {
    Ok(value.cloned().unwrap_or_default().to_number(context)? as usize)
}

fn node_to_js_value(node: Option<NodeHandle>) -> JsValue {
    match node {
        Some(node) => JsValue::from(node.identity() as f64),
        None => JsValue::null(),
    }
}

fn with_host_state<T>(f: impl FnOnce(&Rc<RefCell<HostState>>) -> JsResult<T>) -> JsResult<T> {
    ACTIVE_HOST_STATE.with(|slot| {
        let state = slot.borrow().clone().ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("host state is not active"))
        })?;
        f(&state)
    })
}

fn open_javascript_dialog_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let kind = string_argument(args.first(), "", context)?;
    let kind = match kind.as_str() {
        "alert" => JavaScriptDialogKind::Alert,
        "confirm" => JavaScriptDialogKind::Confirm,
        "prompt" => JavaScriptDialogKind::Prompt,
        _ => {
            return Err(JsNativeError::typ()
                .with_message("unknown JavaScript dialog kind")
                .into());
        }
    };
    let message = string_argument(args.get(1), "", context)?;
    let default_prompt = (kind == JavaScriptDialogKind::Prompt)
        .then(|| string_argument(args.get(2), "", context))
        .transpose()?;

    with_host_state(|state| {
        let dialog = {
            let mut state = state.borrow_mut();
            if state.pending_javascript_dialog.is_some() {
                return Err(JsNativeError::error()
                    .with_message("a JavaScript dialog is already pending")
                    .into());
            }
            let id = state.next_javascript_dialog_id;
            state.next_javascript_dialog_id = id.checked_add(1).ok_or_else(|| {
                JsError::from(
                    JsNativeError::error().with_message("JavaScript dialog id space exhausted"),
                )
            })?;
            JavaScriptDialog {
                id,
                kind,
                message,
                default_prompt,
            }
        };
        let suspension = context.suspend_native_call()?;
        state.borrow_mut().pending_javascript_dialog = Some(PendingJavaScriptDialog {
            dialog,
            suspension,
        });
        Ok(JsValue::undefined())
    })
}

fn performance_now_native(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        Ok(JsValue::from(
            state.borrow().performance_start.elapsed().as_secs_f64() * 1_000.0,
        ))
    })
}

fn crypto_random_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let length = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)?;
    if !length.is_finite() || length < 0.0 || length.fract() != 0.0 || length > 65_536.0 {
        return Err(JsError::from(
            JsNativeError::range().with_message("invalid random byte length"),
        ));
    }
    let mut bytes = vec![0; length as usize];
    getrandom::fill(&mut bytes).map_err(|error| {
        JsError::from(JsNativeError::error().with_message(format!(
            "secure random generation failed: {error}"
        )))
    })?;
    let json = serde_json::to_string(&bytes).map_err(|error| {
        JsError::from(JsNativeError::error().with_message(error.to_string()))
    })?;
    Ok(js_string!(json).into())
}

fn crypto_digest_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    use sha1::Digest;

    let algorithm = string_argument(args.first(), "", context)?;
    let encoded = string_argument(args.get(1), "[]", context)?;
    let bytes: Vec<u8> = serde_json::from_str(&encoded).map_err(|error| {
        JsError::from(
            JsNativeError::typ().with_message(format!("invalid digest input: {error}")),
        )
    })?;
    let digest = match algorithm.as_str() {
        "SHA-1" => sha1::Sha1::digest(&bytes).to_vec(),
        "SHA-256" => sha2::Sha256::digest(&bytes).to_vec(),
        "SHA-384" => sha2::Sha384::digest(&bytes).to_vec(),
        "SHA-512" => sha2::Sha512::digest(&bytes).to_vec(),
        _ => {
            return Err(JsError::from(
                JsNativeError::error().with_message("unsupported digest algorithm"),
            ));
        }
    };
    let json = serde_json::to_string(&digest).map_err(|error| {
        JsError::from(JsNativeError::error().with_message(error.to_string()))
    })?;
    Ok(js_string!(json).into())
}

fn storage_arguments(
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<(bool, usize, StorageManager, u64, StorageOrigin)> {
    let local = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped()
        == "local";
    let document_id = parse_node_id(args.get(1), context)?;
    with_host_state(|state| {
        let state = state.borrow();
        let origin = state
            .document_origins
            .get(&document_id)
            .cloned()
            .flatten()
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("opaque origin")))?;
        Ok((
            local,
            document_id,
            state.storage_manager.clone(),
            state.storage_session_id,
            origin,
        ))
    })
}

fn storage_origin_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .document_origins
            .get(&document_id)
            .cloned()
            .flatten()
            .map(|origin| JsValue::from(js_string!(origin.serialize())))
            .unwrap_or_else(JsValue::null))
    })
}

fn storage_length_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (local, _, manager, session, origin) = storage_arguments(args, context)?;
    Ok(JsValue::from(manager.length(session, &origin, local) as f64))
}

fn storage_key_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (local, _, manager, session, origin) = storage_arguments(args, context)?;
    let index = args.get(2).cloned().unwrap_or_default().to_u32(context)? as usize;
    Ok(manager
        .key(session, &origin, local, index)
        .map(|key| JsValue::from(js_string!(key)))
        .unwrap_or_else(JsValue::null))
}

fn storage_get_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (local, _, manager, session, origin) = storage_arguments(args, context)?;
    let key = args.get(2).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    Ok(manager
        .get(session, &origin, local, &key)
        .map(|value| JsValue::from(js_string!(value)))
        .unwrap_or_else(JsValue::null))
}

fn storage_set_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (local, _, manager, session, origin) = storage_arguments(args, context)?;
    let key = args.get(2).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    let value = args.get(3).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    Ok(manager
        .set(session, &origin, local, key, value)
        .map(|old| JsValue::from(js_string!(old)))
        .unwrap_or_else(JsValue::null))
}

fn storage_remove_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (local, _, manager, session, origin) = storage_arguments(args, context)?;
    let key = args.get(2).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    Ok(manager
        .remove(session, &origin, local, &key)
        .map(|old| JsValue::from(js_string!(old)))
        .unwrap_or_else(JsValue::null))
}

fn storage_clear_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let (local, _, manager, session, origin) = storage_arguments(args, context)?;
    Ok(JsValue::from(manager.clear(session, &origin, local)))
}

// ---------------------------------------------------------------------------
// Computed style + layout metrics (issues 016-8, 044-2)
// ---------------------------------------------------------------------------

/// Recursively collects the text content of every inline `<style>` element in
/// the document tree, returning one CSS string per `<style>` element.
///
/// Only inline styles are gathered; linked stylesheets are not fetched here to
/// keep computed-style resolution synchronous and side-effect free.
fn collect_inline_stylesheets(
    document: &NodeHandle,
) -> Vec<(Option<(NodeHandle, usize)>, Option<NodeHandle>, String)> {
    fn walk(
        node: &NodeHandle,
        scope: Option<&(NodeHandle, usize)>,
        next_scope_order: &mut usize,
        out: &mut Vec<(Option<(NodeHandle, usize)>, Option<NodeHandle>, String)>,
    ) {
        if node.node_type() == NodeType::Element
            && node
                .tag_name()
                .as_deref()
                .is_some_and(|tag| tag.eq_ignore_ascii_case("style"))
        {
            let css = collect_text_recursive(node);
            if !css.trim().is_empty() {
                let implicit_scope_root = node
                    .parent_node()
                    .filter(|parent| parent.node_type() == NodeType::Element);
                out.push((scope.cloned(), implicit_scope_root, css));
            }
        }
        if let Some(root) = node.shadow_root() {
            *next_scope_order += 1;
            let root_scope = (root.clone(), *next_scope_order);
            walk(&root, Some(&root_scope), next_scope_order, out);
        }
        for child in node.child_nodes() {
            walk(&child, scope, next_scope_order, out);
        }
    }
    let mut out = Vec::new();
    let mut next_scope_order = 0;
    walk(document, None, &mut next_scope_order, &mut out);
    out
}

/// Formats an `f32` as a CSS number, dropping a redundant trailing `.0` so that
/// integer-valued lengths serialize as `16px` rather than `16.0px` and
/// `z-index` values serialize as `0` / `3` (matching what Acid3's `selectorTest`
/// compares against).
fn format_css_number(value: f32) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        // Round to a few decimals to avoid noisy floating-point tails.
        let rounded = (value * 1000.0).round() / 1000.0;
        format!("{rounded}")
    }
}

/// Serializes a single [`ComputedValue`] to its CSS string form.
fn computed_value_to_css_string(value: &ComputedValue) -> String {
    match value {
        ComputedValue::Keyword(keyword) => keyword.clone(),
        ComputedValue::Color(color) => crate::paint::color::parse_color(color).map_or_else(
            || color.clone(),
            |parsed| {
                if parsed.a == 255 {
                    format!("rgb({}, {}, {})", parsed.r, parsed.g, parsed.b)
                } else {
                    format!(
                        "rgba({}, {}, {}, {})",
                        parsed.r,
                        parsed.g,
                        parsed.b,
                        format_css_number(f32::from(parsed.a) / 255.0)
                    )
                }
            },
        ),
        ComputedValue::String(string) => string.clone(),
        ComputedValue::Px(px) => format!("{}px", format_css_number(*px)),
        ComputedValue::Percentage(pct) => format!("{}%", format_css_number(*pct)),
        ComputedValue::Number(number) => format_css_number(*number),
        ComputedValue::CalcPxPercent(px, pct) => {
            format!(
                "calc({}px + {}%)",
                format_css_number(*px),
                format_css_number(*pct)
            )
        }
    }
}

/// Serializes a resolved [`ComputedStyle`] to a JSON object mapping each CSS
/// property name (kebab-case) to its computed string value.
fn serialize_computed_style(style: &ComputedStyle) -> String {
    let mut json = String::from("{");
    let mut first = true;
    for (name, value) in style.properties() {
        if !first {
            json.push(',');
        }
        first = false;
        json.push('"');
        json.push_str(&escape_json_string(name));
        json.push_str("\":\"");
        json.push_str(&escape_json_string(&computed_value_to_css_string(value)));
        json.push('"');
    }
    json.push('}');
    json
}

/// Finds the [`LayoutBox`] produced for `node`, searching the layout tree by
/// node identity. Returns `None` for nodes that produced no box (e.g.
/// `display: none`, `<head>` content, or detached nodes).
fn find_layout_box<'a>(root: &'a LayoutBox, node: &NodeHandle) -> Option<&'a LayoutBox> {
    if &root.node == node {
        return Some(root);
    }
    for child in &root.children {
        if let Some(found) = find_layout_box(child, node) {
            return Some(found);
        }
    }
    None
}

fn find_layout_box_with_transform<'a>(
    root: &'a LayoutBox,
    node: &NodeHandle,
    ancestor_transform: AffineTransform,
    fragments: &mut Vec<InlineFragmentGeometry>,
) -> Option<(&'a LayoutBox, AffineTransform)> {
    let transform = ancestor_transform.multiply(root.transform);
    if &root.node == node {
        return Some((root, transform));
    }
    collect_matching_image_fragments(root, node, transform, (0.0, 0.0), fragments);
    for child in &root.children {
        if let Some(found) = find_layout_box_with_transform(child, node, transform, fragments) {
            return Some(found);
        }
    }
    None
}

struct InlineFragmentGeometry {
    rect: Rect,
    style: ComputedStyle,
    transform: AffineTransform,
    scroll: (f32, f32),
}

fn collect_matching_image_fragments(
    root: &LayoutBox,
    node: &NodeHandle,
    transform: AffineTransform,
    scroll: (f32, f32),
    output: &mut Vec<InlineFragmentGeometry>,
) {
    for line in &root.lines {
        for fragment in &line.fragments {
            if &fragment.node == node
                && let InlineFragmentContent::Image(_, style) = &fragment.content
            {
                output.push(InlineFragmentGeometry {
                    rect: fragment.rect,
                    style: style.clone(),
                    transform,
                    scroll,
                });
            }
        }
    }
}

/// The eight geometry values `getBoundingClientRect()` exposes plus the derived
/// `offset*` / `client*` / `scroll*` metrics, all in CSS pixels.
struct LayoutMetrics {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    content_x: f32,
    content_y: f32,
    content_width: f32,
    content_height: f32,
    offset_width: f32,
    offset_height: f32,
    offset_top: f32,
    offset_left: f32,
    client_width: f32,
    client_height: f32,
    client_top: f32,
    client_left: f32,
    scroll_width: f32,
    scroll_height: f32,
    client_rects: Vec<Rect>,
    /// Whether the element produced a layout box at all. `false` for elements
    /// that generate no box (e.g. `display: none`, or a missing node), which
    /// lets `getClientRects()` distinguish "no rendered box" (empty list) from a
    /// rendered but zero-sized box (one rect), per CSSOM.
    has_box: bool,
}

impl LayoutMetrics {
    fn zero() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            content_x: 0.0,
            content_y: 0.0,
            content_width: 0.0,
            content_height: 0.0,
            offset_width: 0.0,
            offset_height: 0.0,
            offset_top: 0.0,
            offset_left: 0.0,
            client_width: 0.0,
            client_height: 0.0,
            client_top: 0.0,
            client_left: 0.0,
            scroll_width: 0.0,
            scroll_height: 0.0,
            client_rects: Vec::new(),
            has_box: false,
        }
    }

    /// Serializes the metrics to a JSON object. `top`/`left`/`right`/`bottom`
    /// mirror the CSSOM `DOMRect` shape; `scrollTop`/`scrollLeft` are always 0
    /// because the engine does not model scroll offsets.
    fn to_json(&self) -> String {
        format!(
            "{{\"x\":{x},\"y\":{y},\"width\":{w},\"height\":{h},\
\"top\":{y},\"left\":{x},\"right\":{right},\"bottom\":{bottom},\
\"contentX\":{content_x},\"contentY\":{content_y},\
\"contentWidth\":{content_width},\"contentHeight\":{content_height},\
\"offsetWidth\":{ow},\"offsetHeight\":{oh},\"offsetTop\":{ot},\"offsetLeft\":{ol},\
\"clientWidth\":{cw},\"clientHeight\":{ch},\"clientTop\":{ct},\"clientLeft\":{cl},\
\"scrollWidth\":{sw},\"scrollHeight\":{sh},\"scrollTop\":0,\"scrollLeft\":0,\
\"hasBox\":{has_box},\"clientRects\":[{client_rects}]}}",
            x = json_number(self.x),
            y = json_number(self.y),
            w = json_number(self.width),
            h = json_number(self.height),
            right = json_number(self.x + self.width),
            bottom = json_number(self.y + self.height),
            content_x = json_number(self.content_x),
            content_y = json_number(self.content_y),
            content_width = json_number(self.content_width),
            content_height = json_number(self.content_height),
            ow = json_number(self.offset_width),
            oh = json_number(self.offset_height),
            ot = json_number(self.offset_top),
            ol = json_number(self.offset_left),
            cw = json_number(self.client_width),
            ch = json_number(self.client_height),
            ct = json_number(self.client_top),
            cl = json_number(self.client_left),
            sw = json_number(self.scroll_width),
            sh = json_number(self.scroll_height),
            has_box = self.has_box,
            client_rects = self
                .client_rects
                .iter()
                .map(rect_to_json)
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

fn rect_to_json(rect: &Rect) -> String {
    format!(
        "{{\"x\":{x},\"y\":{y},\"width\":{width},\"height\":{height},\
\"top\":{y},\"left\":{x},\"right\":{right},\"bottom\":{bottom}}}",
        x = json_number(rect.x),
        y = json_number(rect.y),
        width = json_number(rect.width),
        height = json_number(rect.height),
        right = json_number(rect.x + rect.width),
        bottom = json_number(rect.y + rect.height),
    )
}

/// Rounds a metric to a whole pixel when it is integer-valued (the common case
/// for block layout) and otherwise emits up to three decimals, producing clean
/// JSON numbers like `100` rather than `100.0`.
fn json_number(value: f32) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        let rounded = (value * 1000.0).round() / 1000.0;
        format!("{rounded}")
    }
}

/// Computes the layout metrics for a single [`LayoutBox`].
///
/// - `getBoundingClientRect` / `offsetWidth` / `offsetHeight` use the border
///   box (content + padding + border).
/// - `offsetTop` / `offsetLeft` are the border-box position relative to the
///   initial containing block (the viewport origin); this coincides with the
///   CSSOM definition when the offset parent is the root box at the origin.
/// - `clientWidth` / `clientHeight` use the padding box (content + padding),
///   and `clientTop` / `clientLeft` are the top/left border widths.
/// - `scrollWidth` / `scrollHeight` are the padding box extended to enclose the
///   border boxes of every overflowing descendant plus the container's
///   end-edge padding (not just the direct children). Traversal stops at any
///   descendant that clips its overflow
///   (`overflow` other than `visible`): such a box still contributes its own
///   border box, but its clipped content cannot overflow past it into this
///   element's scrollable area. See [`expand_scroll_bounds`].
fn compute_layout_metrics(layout: &LayoutBox) -> LayoutMetrics {
    let content = layout.dimensions.content;
    let padding = layout.dimensions.padding;
    let border = layout.dimensions.border;

    let border_x = content.x - padding.left - border.left;
    let border_y = content.y - padding.top - border.top;
    let border_width = content.width + padding.left + padding.right + border.left + border.right;
    let border_height = content.height + padding.top + padding.bottom + border.top + border.bottom;

    let client_width = content.width + padding.left + padding.right;
    let client_height = content.height + padding.top + padding.bottom;

    let (scroll_width, scroll_height) = layout.scrollable_overflow();

    LayoutMetrics {
        x: border_x,
        y: border_y,
        width: border_width,
        height: border_height,
        content_x: content.x,
        content_y: content.y,
        content_width: content.width,
        content_height: content.height,
        offset_width: border_width,
        offset_height: border_height,
        offset_top: border_y,
        offset_left: border_x,
        client_width,
        client_height,
        client_top: border.top,
        client_left: border.left,
        scroll_width,
        scroll_height,
        client_rects: vec![Rect {
            x: border_x,
            y: border_y,
            width: border_width,
            height: border_height,
        }],
        has_box: true,
    }
}

fn compute_transformed_layout_metrics(
    layout: &LayoutBox,
    transform: AffineTransform,
) -> LayoutMetrics {
    let mut metrics = compute_layout_metrics(layout);
    if transform.is_identity() {
        return metrics;
    }
    let transformed = transform_rect(
        Rect {
            x: metrics.x,
            y: metrics.y,
            width: metrics.width,
            height: metrics.height,
        },
        transform,
    );
    metrics.x = transformed.x;
    metrics.y = transformed.y;
    metrics.width = transformed.width;
    metrics.height = transformed.height;
    metrics.client_rects = vec![transformed];
    metrics
}

fn transform_rect(rect: Rect, transform: AffineTransform) -> Rect {
    if transform.is_identity() {
        return rect;
    }
    let corners = [
        transform.transform_point(rect.x, rect.y),
        transform.transform_point(rect.x + rect.width, rect.y),
        transform.transform_point(rect.x, rect.y + rect.height),
        transform.transform_point(rect.x + rect.width, rect.y + rect.height),
    ];
    let min_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min);
    let min_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min);
    let max_x = corners
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = corners
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max);
    Rect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

fn compute_image_fragment_metrics(fragments: Vec<InlineFragmentGeometry>) -> LayoutMetrics {
    let Some(first) = fragments.first() else {
        return LayoutMetrics::zero();
    };
    let layout_rect = first.rect;
    let padding = edge_sizes(&first.style, "padding");
    let border = edge_sizes(&first.style, "border");
    let client_width = (first.rect.width - border.left - border.right).max(0.0);
    let client_height = (first.rect.height - border.top - border.bottom).max(0.0);
    let content_width = (client_width - padding.left - padding.right).max(0.0);
    let content_height = (client_height - padding.top - padding.bottom).max(0.0);
    let mut client_rects = Vec::with_capacity(fragments.len());
    for fragment in fragments {
        let mut rect = transform_rect(fragment.rect, fragment.transform);
        rect.x -= fragment.scroll.0;
        rect.y -= fragment.scroll.1;
        client_rects.push(rect);
    }
    let min_x = client_rects.iter().map(|rect| rect.x).fold(f32::INFINITY, f32::min);
    let min_y = client_rects.iter().map(|rect| rect.y).fold(f32::INFINITY, f32::min);
    let max_x = client_rects
        .iter()
        .map(|rect| rect.x + rect.width)
        .fold(f32::NEG_INFINITY, f32::max);
    let max_y = client_rects
        .iter()
        .map(|rect| rect.y + rect.height)
        .fold(f32::NEG_INFINITY, f32::max);
    LayoutMetrics {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
        content_x: min_x + border.left + padding.left,
        content_y: min_y + border.top + padding.top,
        content_width,
        content_height,
        offset_width: layout_rect.width,
        offset_height: layout_rect.height,
        offset_top: layout_rect.y,
        offset_left: layout_rect.x,
        client_width,
        client_height,
        client_top: border.top,
        client_left: border.left,
        scroll_width: client_width,
        scroll_height: client_height,
        client_rects,
        has_box: true,
    }
}

/// `__omoikane_computed_style(nodeId)` -> JSON string of computed CSS
/// properties (kebab-case name to CSS string value). Forces a synchronous
/// style recompute if the DOM changed since the last query.
fn computed_style_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else {
            return Ok(js_string!("{}").into());
        };
        if node.node_type() != NodeType::Element {
            return Ok(js_string!("{}").into());
        }
        // Resolve against the cascade of the document this node actually lives
        // in, so a sub-document (iframe) node uses the iframe's `<style>` rules
        // and never the main document's, and vice versa. A detached node has no
        // document root and keeps the empty-object result.
        let Some(document) = document_root_for_node(&node) else {
            return Ok(js_string!("{}").into());
        };
        let document_id = document.identity();
        let mut state = state.borrow_mut();
        state.ensure_style_resolver(&document);
        let needs_container_layout = state
            .document_styles
            .get(&document_id)
            .and_then(|entry| entry.resolver.as_ref())
            .is_some_and(StyleResolver::has_container_queries);
        if needs_container_layout {
            if document_id == state.document.identity() {
                state.ensure_layout();
            } else {
                let viewport = state.viewport_for_document(&document);
                let _ = state
                    .document_styles
                    .get_mut(&document_id)
                    .and_then(|entry| entry.resolver.as_mut())
                    .and_then(|resolver| crate::layout::layout_tree(&document, resolver, viewport));
            }
        }
        let json = match state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
        {
            Some(resolver) => serialize_computed_style(&resolver.computed_style(&node)),
            None => "{}".to_string(),
        };
        Ok(js_string!(json.as_str()).into())
    })
}

/// `__omoikane_is_rendered_for_focus(nodeId)` reports the rendered-ness parts
/// of focusability that cannot be determined from the DOM alone. `display:none`
/// removes the whole subtree, while `visibility` is inherited and therefore is
/// read from the target's computed style (allowing a descendant's `visible` to
/// restore visibility).
fn is_rendered_for_focus_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else { return Ok(JsValue::from(false)); };
        if node.node_type() != NodeType::Element {
            return Ok(JsValue::from(false));
        }
        let Some(document) = document_root_for_node(&node) else {
            return Ok(JsValue::from(false));
        };
        let mut state = state.borrow_mut();
        state.ensure_style_resolver(&document);
        let document_id = document.identity();

        let mut current = Some(node.clone());
        while let Some(element) = current {
            let style = state
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut())
                .map(|resolver| resolver.computed_style(&element));
            let Some(style) = style else { return Ok(JsValue::from(false)); };
            if matches!(style.get("display"), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("none")) {
                return Ok(JsValue::from(false));
            }
            current = element
                .assigned_slot()
                .or_else(|| element.parent_node().and_then(|parent| {
                    if parent.node_type() == NodeType::Element {
                        Some(parent)
                    } else {
                        parent.shadow_host()
                    }
                }));
        }

        let style = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .map(|resolver| resolver.computed_style(&node));
        let visible = style.is_none_or(|style| {
            !matches!(
                style.get("visibility"),
                Some(ComputedValue::Keyword(value))
                    if value.eq_ignore_ascii_case("hidden")
                        || value.eq_ignore_ascii_case("collapse")
            )
        });
        Ok(JsValue::from(visible))
    })
}

/// `__omoikane_is_actually_disabled(nodeId)` applies HTML's inherited
/// `fieldset[disabled]` state, including the first-legend exception.
fn is_actually_disabled_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let disabled = state
            .borrow()
            .get_node(node_id)
            .is_some_and(|node| is_actually_disabled(&node));
        Ok(JsValue::from(disabled))
    })
}

/// `__omoikane_layout_metrics(nodeId)` -> JSON string of geometry metrics for
/// the element (see [`compute_layout_metrics`]). Forces a synchronous reflow if
/// the DOM changed since the last query. Elements that produce no box (e.g.
/// `display: none`) report all-zero metrics.
fn layout_metrics_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else {
            return Ok(js_string!(LayoutMetrics::zero().to_json().as_str()).into());
        };
        let is_root_element = node
            .parent_node()
            .is_some_and(|parent| parent.node_type() == NodeType::Document);
        let document = document_root_for_node(&node);
        let mut state = state.borrow_mut();
        let viewport = document
            .as_ref()
            .map(|document| state.viewport_for_document(document));
        state.ensure_layout();
        let current_scroll = state.window_scroll;
        state.set_window_scroll(current_scroll.0, current_scroll.1);
        let main_document_id = state.document.identity();
        let is_main_document = document
            .as_ref()
            .is_some_and(|document| document.identity() == main_document_id);
        // Client geometry comes from the same paint-time clone used by hit
        // testing and rendering. Besides ordinary scroll offsets this applies
        // `position: sticky` without changing document-coordinate layout.
        let window_scroll = state.window_scroll;
        let mut metrics = LayoutMetrics::zero();
        {
            let state = &mut *state;
            let mut resolver = state
                .document_styles
                .get_mut(&main_document_id)
                .and_then(|entry| entry.resolver.as_mut());
            if let Some(root) = state.layout_root.as_ref() {
                let mut fragments = Vec::new();
                if let Some((layout, transform)) = find_layout_box_with_transform(
                    root, &node, AffineTransform::identity(), &mut fragments,
                ) {
                    metrics = compute_transformed_layout_metrics(layout, transform);
                } else {
                    metrics = compute_image_fragment_metrics(fragments);
                }

                if is_main_document && let Some(resolver) = resolver.as_deref_mut() {
                    let mut painted_root = root.clone();
                    crate::paint::apply_scroll_offsets(
                        &mut painted_root, resolver, state.viewport, window_scroll,
                    );
                    let mut painted_fragments = Vec::new();
                    let painted = if let Some((layout, transform)) = find_layout_box_with_transform(
                        &painted_root, &node, AffineTransform::identity(), &mut painted_fragments,
                    ) {
                        compute_transformed_layout_metrics(layout, transform)
                    } else {
                        compute_image_fragment_metrics(painted_fragments)
                    };
                    if painted.has_box {
                        metrics.x = painted.x;
                        metrics.y = painted.y;
                        metrics.width = painted.width;
                        metrics.height = painted.height;
                        metrics.client_rects = painted.client_rects;
                    }
                }
            }
        }
        if is_root_element && let Some(viewport) = viewport {
            metrics.client_width = viewport.width;
            metrics.client_height = viewport.height;
            metrics.client_top = 0.0;
            metrics.client_left = 0.0;
            metrics.scroll_width = metrics.scroll_width.max(viewport.width);
            metrics.scroll_height = metrics.scroll_height.max(viewport.height);
        }
        Ok(js_string!(metrics.to_json().as_str()).into())
    })
}

/// `__omoikane_element_scroll_offset(nodeId)` -> `{"x":..,"y":..}`, the scroll
/// offset in effect for the element.
fn element_scroll_offset_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let (x, y) = match node {
            Some(node) => state.borrow_mut().element_scroll_offset(&node),
            None => (0.0, 0.0),
        };
        let json = format!("{{\"x\":{},\"y\":{}}}", json_number(x), json_number(y));
        Ok(js_string!(json).into())
    })
}

/// Sets and clamps an element's scroll offset. Non-finite coordinates scroll to
/// zero, matching how browsers normalize them.
fn set_element_scroll_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    let coordinate = |value: Option<&JsValue>, context: &mut Context| -> JsResult<f32> {
        let value = value.cloned().unwrap_or_default().to_number(context)? as f32;
        Ok(if value.is_finite() { value } else { 0.0 })
    };
    let x = coordinate(args.get(1), context)?;
    let y = coordinate(args.get(2), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else {
            return Ok(JsValue::undefined());
        };
        state.borrow_mut().set_element_scroll(&node, x, y);
        Ok(JsValue::undefined())
    })
}

/// Returns the top-level Window scroll offset as a JSON object.
fn window_scroll_offset_native(
    _: &JsValue,
    _: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let current = state.window_scroll;
        if current != (0.0, 0.0) {
            state.set_window_scroll(current.0, current.1);
        }
        let (x, y) = state.window_scroll;
        let json = format!("{{\"x\":{},\"y\":{}}}", json_number(x), json_number(y));
        Ok(js_string!(json).into())
    })
}

/// Sets and clamps the top-level Window scroll offset.
fn set_window_scroll_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let coordinate = |value: Option<&JsValue>, context: &mut Context| -> JsResult<f32> {
        let value = value.cloned().unwrap_or_default().to_number(context)? as f32;
        Ok(if value.is_finite() { value } else { 0.0 })
    };
    let x = coordinate(args.first(), context)?;
    let y = coordinate(args.get(1), context)?;
    with_host_state(|state| {
        state.borrow_mut().set_window_scroll(x, y);
        Ok(JsValue::undefined())
    })
}

fn set_timeout_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer_from_js(args, context, false)
}

fn set_interval_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer_from_js(args, context, true)
}

fn clear_timer_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_u32(context)
        .unwrap_or(0) as u64;

    with_host_state(|state| {
        state.borrow_mut().event_loop.clear_timer(id);
        Ok(JsValue::undefined())
    })
}

fn request_animation_frame_native(
    _: &JsValue,
    args: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("requestAnimationFrame callback must be callable")
            .into());
    }
    with_host_state(|state| {
        let id = state
            .borrow_mut()
            .event_loop
            .schedule_animation_frame(callback);
        Ok(JsValue::from(id as f64))
    })
}

fn cancel_animation_frame_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_u32(context)
        .unwrap_or(0) as u64;
    with_host_state(|state| {
        state
            .borrow_mut()
            .event_loop
            .cancel_animation_frame(id);
        Ok(JsValue::undefined())
    })
}

fn schedule_timer_from_js(
    args: &[JsValue],
    context: &mut Context,
    repeat: bool,
) -> JsResult<JsValue> {
    let handler = args.first().cloned().unwrap_or_default();
    let delay_ms = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_u32(context)
        .unwrap_or(0) as u64;

    // A function handler is retained as a live callback (preserving its closure
    // scope), together with any extra arguments passed after the delay. A
    // non-callable handler falls back to the HTML string-source behaviour.
    let payload = if handler.is_callable() {
        let extra_args: Vec<JsValue> = args.iter().skip(2).cloned().collect();
        TimerPayload::Callback {
            callback: handler,
            args: extra_args,
        }
    } else {
        TimerPayload::Source(handler.to_string(context)?.to_std_string_escaped())
    };

    with_host_state(|state| {
        let id = state
            .borrow_mut()
            .event_loop
            .schedule_timer(payload, delay_ms, repeat);
        Ok(JsValue::from(id as f64))
    })
}

fn query_selector_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    let selector = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let selectors = parse_dom_selector_list(&selector)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        Ok(node_to_js_value(node.and_then(|node| {
            query_first_matching_descendant(&node, &selectors)
        })))
    })
}

fn create_element_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let tag_name = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let node = NodeHandle::element(tag_name);
        let id = node.identity();
        state.borrow_mut().register_tree(&node);
        Ok(JsValue::from(id as f64))
    })
}

fn is_valid_xml_name_start_char(cp: u32) -> bool {
    cp == 0x3a
        || (0x41..=0x5a).contains(&cp)
        || cp == 0x5f
        || (0x61..=0x7a).contains(&cp)
        || (0xc0..=0xd6).contains(&cp)
        || (0xd8..=0xf6).contains(&cp)
        || (0xf8..=0x2ff).contains(&cp)
        || (0x370..=0x37d).contains(&cp)
        || (0x37f..=0x1fff).contains(&cp)
        || (0x200c..=0x200d).contains(&cp)
        || (0x2070..=0x218f).contains(&cp)
        || (0x2c00..=0x2fef).contains(&cp)
        || (0x3001..=0xd7ff).contains(&cp)
        || (0xf900..=0xfdcf).contains(&cp)
        || (0xfdf0..=0xfffd).contains(&cp)
        || (0x10000..=0xeffff).contains(&cp)
}

fn is_valid_xml_name_char(cp: u32) -> bool {
    is_valid_xml_name_start_char(cp)
        || cp == 0x2d
        || cp == 0x2e
        || (0x30..=0x39).contains(&cp)
        || cp == 0xb7
        || (0x300..=0x36f).contains(&cp)
        || (0x203f..=0x2040).contains(&cp)
}

/// Validates an XML Name outside Boa bytecode. This is intentionally native:
/// Boa 0.21.1 can return a stale inline-cache slot for the JavaScript
/// `codePointAt` call site after some core-js polyfills mutate built-in shapes.
fn is_valid_xml_name_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|first| is_valid_xml_name_start_char(first as u32))
        && chars.all(|ch| is_valid_xml_name_char(ch as u32));
    Ok(JsValue::from(valid))
}

fn append_child_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = parse_node_id(args.first(), context)?;
    let child_id = parse_node_id(args.get(1), context)?;
    with_host_state(|state| {
        let (parent, child) = {
            let borrowed = state.borrow();
            (borrowed.get_node(parent_id), borrowed.get_node(child_id))
        };
        let parent = parent.ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("parent node not found"))
        })?;
        let child = child.ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("child node not found"))
        })?;
        // `append_child` may move `child` out of another document into
        // `parent`'s document. Note both documents *before* the move so both
        // resolvers are invalidated: the source loses a node, the target gains
        // one. (A detached side has no document root and needs no invalidation;
        // it acquires one only when later inserted into a live document.) The
        // pair is also compared below to decide whether the move is a fresh
        // navigation for iframe/object resource elements.
        let source_document = document_root_for_node(&child);
        let target_document = document_root_for_node(&parent);
        parent.append_child(child.clone());
        {
            let mut state = state.borrow_mut();
            state.register_tree(&child);
            if let Some(document) = &source_document {
                state.mark_document_style_dirty(document);
            }
            if let Some(document) = &target_document {
                state.mark_document_style_dirty(document);
            }
            // Schedule iframe/object resource loads whenever the move lands the
            // subtree in a live document that differs from where it came from:
            // a detached origin (`source_document == None`) becoming connected,
            // or a *direct* move between two different documents (e.g. main
            // document ↔ iframe sub-document). Both are fresh navigations for
            // the moved resource elements. A pure in-document reorder
            // (`source_document == target_document`) is intentionally left alone
            // so it does not re-navigate — matching the existing model, where
            // only a detach/reconnect reloads (real browsers reload here too,
            // but that broader change is out of scope for this fix).
            if target_document.is_some() && source_document != target_document {
                state.schedule_connected_resource_loads(&child, true);
            }
        }
        Ok(JsValue::from(child.identity() as f64))
    })
}

fn parent_node_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        Ok(node_to_js_value(node.and_then(|node| node.parent_node())))
    })
}

/// `__omoikane_owner_document(nodeId)` — returns the node id of the Document
/// that owns `node`, found by walking to the root of its tree.
///
/// Returns `null` for a document node itself (a document has no owner
/// document), and also for a detached node whose tree root is not a document
/// (a freshly created, not-yet-inserted element): the JS layer then falls back
/// to the node's creation-time owner. An attached node correctly reports the
/// top-level document or the iframe sub-document it currently lives in.
fn owner_document_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let s = state.borrow();
        let Some(node) = s.get_node(node_id) else {
            return Ok(JsValue::null());
        };
        // DOM `ownerDocument` semantics: a document node has no owner document,
        // and a detached node whose root is not a document reports none.
        Ok(node_to_js_value(owner_document_for_node(&node)))
    })
}

/// `__omoikane_document_owner_iframe(documentId)` — returns the node id of the
/// `<iframe>` element that owns the sub-browsing-context document `documentId`,
/// or `null` when it is the top-level (main) document, a reloaded/stale document
/// no longer tracked, or any unknown id.
///
/// Backs `Document.defaultView`: a sub-document routes to its owning iframe's
/// `contentWindow`, while an unknown/stale document must NOT be treated as the
/// main window. The main document is reported as `null` here because the JS
/// layer already routes it to `globalThis` before calling this binding.
///
/// The lookup is a linear scan of the (typically small) `iframe_documents`
/// table; a reverse index is deliberately avoided to keep reload cleanup from
/// having to maintain two maps in lockstep.
fn document_owner_iframe_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let s = state.borrow();
        // The main document is not owned by any iframe.
        if document_id == s.document.identity() {
            return Ok(JsValue::null());
        }
        for (iframe_id, entry) in s.iframe_documents.iter() {
            if entry.document.identity() == document_id {
                return Ok(JsValue::from(*iframe_id as f64));
            }
        }
        // Unknown or reloaded (stale) sub-document: no live owning iframe.
        Ok(JsValue::null())
    })
}

fn node_name_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        Ok(js_string!(node.node_name().as_str()).into())
    })
}

fn node_local_name_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(id)
            .and_then(|n| n.local_name())
            .map(|s| js_string!(s.as_str()).into())
            .unwrap_or_else(JsValue::null))
    })
}

fn node_namespace_uri_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(id)
            .and_then(|n| n.namespace_uri())
            .map(|s| js_string!(s.as_str()).into())
            .unwrap_or_else(JsValue::null))
    })
}

fn node_prefix_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(id)
            .and_then(|n| n.prefix())
            .map(|s| js_string!(s.as_str()).into())
            .unwrap_or_else(JsValue::null))
    })
}

fn doctype_public_id_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(id)
            .and_then(|n| n.public_id())
            .map(|s| js_string!(s.as_str()).into())
            .unwrap_or_else(|| js_string!("").into()))
    })
}

fn doctype_system_id_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(id)
            .and_then(|n| n.system_id())
            .map(|s| js_string!(s.as_str()).into())
            .unwrap_or_else(|| js_string!("").into()))
    })
}

fn attribute_names_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let names: Vec<JsValue> = node
            .and_then(|node| node.attributes())
            .map(|attributes| {
                attributes
                    .keys()
                    .map(|key| js_string!(key.as_str()).into())
                    .collect()
            })
            .unwrap_or_default();
        Ok(boa_engine::JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(names, context),
        ))
    })
}

fn get_attribute_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    let name = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let value = node
            .and_then(|node| {
                let is_html = node.is_html_element();
                node.attributes().map(|attributes| (attributes, is_html))
            })
            .and_then(|(attributes, is_html)| {
                attributes.get(&name).cloned().or_else(|| {
                    is_html
                        .then(|| attributes.get(&name.to_ascii_lowercase()).cloned())
                        .flatten()
                })
            });
        Ok(match value {
            Some(value) => js_string!(value.as_str()).into(),
            None => JsValue::null(),
        })
    })
}

/// Parses CSS with the engine parser and returns its top-level rule count.
/// CSSOM uses this both to enumerate a style block and to require that an
/// `insertRule` argument is exactly one syntactically valid rule.
fn css_rule_count_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let css = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let sheet = crate::css::parse_stylesheet(&css)
        .map_err(|error| JsError::from(JsNativeError::syntax().with_message(error.to_string())))?;
    Ok(JsValue::from(sheet.rules.len() as f64))
}

/// Evaluates the two-argument form of `CSS.supports()` against the same parser
/// and supported-property table used by style resolution.
fn css_supports_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let property = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let value = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    Ok(JsValue::from(crate::css::supports_declaration(
        &property, &value,
    )))
}

/// Normalizes values assigned through CSSStyleDeclaration. Transition values
/// use native grammar validation so invalid assignments are ignored and
/// specified-value serialization is canonical.
fn normalize_style_value_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let property = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped()
        .to_ascii_lowercase();
    let value = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let normalized = if property == "transition" {
        crate::css::normalize_transition_shorthand(&value)
    } else if matches!(
        property.as_str(),
        "transition-property"
            | "transition-duration"
            | "transition-timing-function"
            | "transition-delay"
    ) {
        crate::css::normalize_transition_longhand(&property, &value)
    } else {
        Some(value)
    };
    Ok(normalized
        .map(|value| js_string!(value).into())
        .unwrap_or_else(JsValue::null))
}

fn take_transition_events_native(
    _: &JsValue,
    _: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let events = state
            .document_styles
            .values_mut()
            .filter_map(|entry| entry.resolver.as_mut())
            .flat_map(StyleResolver::take_transition_events)
            .map(|event| {
                serde_json::json!({
                    "nodeId": event.node_id,
                    "type": event.event_type,
                    "propertyName": event.property_name,
                    "elapsedTime": event.elapsed_time,
                    "pseudoElement": "",
                })
            })
            .collect::<Vec<_>>();
        Ok(js_string!(serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string())).into())
    })
}

fn sample_css_transition_styles_native(
    _: &JsValue,
    _: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let documents = {
            let state = state.borrow();
            state
                .document_styles
                .keys()
                .filter_map(|document_id| state.nodes.get(document_id).cloned())
                .collect::<Vec<_>>()
        };
        let mut state = state.borrow_mut();
        for document in documents {
            let document_id = document.identity();
            let full_sample = state
                .document_styles
                .get(&document_id)
                .is_none_or(|entry| {
                    entry.dirty || entry.needs_full_sample || entry.resolver.is_none()
                });
            state.ensure_style_resolver(&document);
            let running_node_ids = state
                .document_styles
                .get(&document_id)
                .and_then(|entry| entry.resolver.as_ref())
                .map(StyleResolver::running_transition_node_ids)
                .unwrap_or_default();
            if !full_sample && running_node_ids.is_empty() {
                continue;
            }
            let elements = if full_sample {
                collect_element_nodes(&document)
            } else {
                running_node_ids
                    .iter()
                    .filter_map(|node_id| state.nodes.get(node_id).cloned())
                    .filter(|node| {
                        document_root_for_node(node)
                            .is_some_and(|root| root.identity() == document_id)
                    })
                    .collect()
            };
            let active_node_ids = elements.iter().map(NodeHandle::identity).collect::<HashSet<_>>();
            if let Some(resolver) = state
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut())
            {
                for element in elements {
                    resolver.computed_style(&element);
                }
                if full_sample {
                    resolver.finish_transition_sample(&active_node_ids);
                } else {
                    resolver.cancel_detached_transitions(&active_node_ids);
                }
            }
            if full_sample
                && let Some(entry) = state.document_styles.get_mut(&document_id)
            {
                entry.needs_full_sample = false;
            }
        }
        Ok(JsValue::undefined())
    })
}

fn collect_element_nodes(root: &NodeHandle) -> Vec<NodeHandle> {
    fn visit(node: &NodeHandle, elements: &mut Vec<NodeHandle>) {
        if node.node_type() == NodeType::Element {
            elements.push(node.clone());
        }
        for child in node.child_nodes() {
            visit(&child, elements);
        }
    }

    let mut elements = Vec::new();
    visit(root, &mut elements);
    elements
}

/// Evaluates the condition-text form of `CSS.supports()` using the same
/// parser used by `@supports` during cascade collection.
fn css_supports_condition_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let condition = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    Ok(JsValue::from(crate::css::supports_condition_matches(
        &condition,
    )))
}

/// Evaluates a media query list against the runtime's current viewport. This
/// deliberately reuses the `@media` parser/evaluator so CSS and script-visible
/// feature detection cannot disagree.
fn match_media_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let query = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let viewport = state.borrow().viewport;
        let matches = crate::css::parse_media_query_list(&query).is_some_and(|queries| {
            queries.iter().any(|query| {
                crate::css::evaluate_media_query(
                    query,
                    viewport.width,
                    viewport.height,
                    false,
                )
            })
        });
        Ok(JsValue::from(matches))
    })
}

fn set_attribute_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    let name = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let value = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        // Setting the resource attribute of a connected `<iframe>`/`<object>`
        // starts a fresh navigation; detect it before `name` is moved into
        // `set_attribute`. Any other attribute leaves navigation untouched.
        let resource_attr = node.tag_name().and_then(|tag| {
            if (tag.eq_ignore_ascii_case("iframe") || tag.eq_ignore_ascii_case("script"))
                && name.eq_ignore_ascii_case("src")
            {
                Some("src")
            } else if tag.eq_ignore_ascii_case("object") && name.eq_ignore_ascii_case("data") {
                Some("data")
            } else {
                None
            }
        });
        node.set_attribute(name, value);
        // Any attribute may participate in a selector (id/class/attribute
        // selectors), so invalidate the element's live document. Detached
        // elements cannot affect it until the insertion path invalidates it.
        if node.tag_name().as_deref() == Some("style") {
            state.borrow_mut().mark_style_dirty_for_node(&node);
        } else {
            state.borrow_mut().invalidate_style_cache_for_node(&node);
        }
        if let Some(resource_attr) = resource_attr {
            state
                .borrow_mut()
                .schedule_resource_load_on_attribute_change(&node, resource_attr);
        }
        Ok(JsValue::undefined())
    })
}

fn get_checked_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let checked = state
            .borrow()
            .get_node(node_id)
            .is_some_and(|node| node.checked());
        Ok(JsValue::from(checked))
    })
}

fn set_checked_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    let checked = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        node.set_checked(checked);
        state.borrow_mut().invalidate_style_cache_for_node(&node);
        Ok(JsValue::undefined())
    })
}

fn set_text_control_state_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    let value = string_argument(args.get(1), "", context)?;
    let selection_start = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let selection_end = args
        .get(3)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let focused = args.get(4).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        node.set_text_control_state(value, selection_start, selection_end, focused);
        state.borrow_mut().invalidate_layout_for_node(&node);
        Ok(JsValue::undefined())
    })
}

fn console_log_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let mut parts = Vec::new();
    for arg in args {
        parts.push(arg.to_string(context)?.to_std_string_escaped());
    }

    with_host_state(|state| {
        state.borrow_mut().console_logs.push(parts.join(" "));
        Ok(JsValue::undefined())
    })
}

fn string_argument(
    value: Option<&JsValue>,
    default: &str,
    context: &mut Context,
) -> JsResult<String> {
    value
        .cloned()
        .unwrap_or_else(|| JsValue::from(js_string!(default)))
        .to_string(context)
        .map(|value| value.to_std_string_escaped())
}

/// Reads a request/response payload argument as raw bytes.
///
/// A `Uint8Array` is taken byte for byte, which is how bodies that are not text
/// reach the host: `Blob` bodies, and `multipart/form-data` carrying file parts.
/// Anything else is stringified and encoded as UTF-8, preserving the plain-text
/// path (`fetch(url, { body: "a=1" })`) exactly as before.
///
/// `null` and `undefined` mean "no body" and yield `None`.
fn body_bytes_argument(value: Option<&JsValue>, context: &mut Context) -> JsResult<Option<Vec<u8>>> {
    let Some(value) = value.filter(|value| !value.is_null_or_undefined()) else {
        return Ok(None);
    };
    if let Some(object) = value.as_object()
        && let Ok(view) = JsUint8Array::from_object(object.clone())
    {
        let offset = view.byte_offset(context)?;
        let length = view.byte_length(context)?;
        // Read the backing store in one copy. Going through element accessors
        // instead would cost a JsValue conversion per byte, which is a real cost
        // for a multi-megabyte body.
        let buffer = view
            .buffer(context)?
            .as_object()
            .and_then(|buffer| JsArrayBuffer::from_object(buffer.clone()).ok())
            .ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ().with_message("request body is not backed by an ArrayBuffer"),
                )
            })?;
        let data = buffer.data().ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("request body buffer is detached"))
        })?;
        // Two `get`s rather than an `offset..offset + length` range: the sum
        // cannot overflow, and each failure keeps its own diagnosis.
        let bytes = data
            .get(offset..)
            .and_then(|tail| tail.get(..length))
            .ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ().with_message("request body view is out of bounds"),
                )
            })?
            .to_vec();
        return Ok(Some(bytes));
    }
    Ok(Some(
        value
            .clone()
            .to_string(context)?
            .to_std_string_escaped()
            .into_bytes(),
    ))
}

/// Backs `URL.createObjectURL()`: mirrors a blob's bytes into the host-side blob
/// URL store so resource loads that happen after script has finished — `<img
/// src>`, CSS `url(...)` — can still resolve the URL. See [`crate::data`].
fn register_object_url_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = string_argument(args.first(), "", context)?;
    let bytes = body_bytes_argument(args.get(1), context)?.unwrap_or_default();
    let media_type = string_argument(args.get(2), "", context)?;
    crate::data::register_blob_url(url, bytes, media_type);
    Ok(JsValue::undefined())
}

/// Backs `URL.revokeObjectURL()`. Returns whether the URL was registered;
/// unknown URLs are ignored, as the File API requires.
fn revoke_object_url_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = string_argument(args.first(), "", context)?;
    Ok(JsValue::from(crate::data::revoke_blob_url(&url)))
}

/// Queues `callback` on the file reading task source.
///
/// `FileReader` owes its events to a task rather than a microtask, so a read
/// started during a script sees its `load` only after that script — and any
/// other already-queued task — has finished.
fn queue_file_reading_task_native(
    _: &JsValue,
    args: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("file reading task callback must be callable")
            .into());
    }
    with_host_state(|state| {
        state
            .borrow_mut()
            .event_loop
            .enqueue_file_reading(TimerPayload::Callback {
                callback,
                args: Vec::new(),
            });
        Ok(JsValue::undefined())
    })
}

/// Queues a callback on HTML's networking task source.
fn queue_networking_task_native(
    _: &JsValue,
    args: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("networking task callback must be callable")
            .into());
    }
    with_host_state(|state| {
        state.borrow_mut().event_loop.enqueue_networking(
            TimerPayload::Callback { callback, args: Vec::new() },
        );
        Ok(JsValue::undefined())
    })
}

fn canvas_commit_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let width = args.get(1).cloned().unwrap_or_default().to_number(context)? as u32;
    let height = args.get(2).cloned().unwrap_or_default().to_number(context)? as u32;
    let pixels = body_bytes_argument(args.get(3), context)?.unwrap_or_default();
    with_host_state(|state| {
        let Some(node) = state.borrow().get_node(id) else {
            return Ok(JsValue::from(false));
        };
        Ok(JsValue::from(crate::canvas::commit(&node, width, height, pixels)))
    })
}

fn canvas_data_url_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    Ok(js_string!(crate::canvas::png_data_url(id).unwrap_or_else(|| "data:,".into())).into())
}

fn canvas_image_source_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let Some(node) = state.borrow().get_node(id) else {
            return Ok(JsValue::null());
        };
        let Some((_, image)) = crate::layout::element_inline_image(&node) else {
            return Ok(JsValue::null());
        };
        let payload = serde_json::json!({
            "width": image.width(),
            "height": image.height(),
            "pixels": base64::engine::general_purpose::STANDARD.encode(image.pixels()),
        });
        Ok(js_string!(payload.to_string()).into())
    })
}

fn websocket_connect_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = string_argument(args.first(), "", context)?;
    let protocols_json = string_argument(args.get(1), "[]", context)?;
    let protocols: Vec<String> = serde_json::from_str(&protocols_json).map_err(|_| {
        JsError::from(JsNativeError::typ().with_message("invalid WebSocket protocols"))
    })?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let origin = state.base_url.as_ref().map(|url| format!("{}://{}", url.scheme(), url.authority()));
        let client = crate::realtime::WebSocketClient::connect(&url, &protocols, origin.as_deref())
            .map_err(|error| JsError::from(JsNativeError::error().with_message(error)))?;
        let protocol = client.protocol().to_string();
        let mut reader = client.try_clone()
            .map_err(|error| JsError::from(JsNativeError::error().with_message(error)))?;
        let (sender, incoming) = channel();
        thread::spawn(move || loop {
            match reader.read_message() {
                Ok(message) => {
                    let closed = matches!(message, crate::realtime::WebSocketMessage::Close { .. });
                    if sender.send(WebSocketReadResult::Message(message)).is_err() || closed { break; }
                }
                Err(error) => { let _ = sender.send(WebSocketReadResult::Error(error)); break; }
            }
        });
        let id = state.next_websocket_id;
        state.next_websocket_id = state.next_websocket_id.saturating_add(1);
        state.websocket_clients.insert(id, WebSocketConnection { client, incoming });
        Ok(js_string!(serde_json::json!({"id": id, "protocol": protocol}).to_string()).into())
    })
}

fn websocket_send_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as u64;
    let encoded = string_argument(args.get(1), "", context)?;
    let binary = args.get(2).is_some_and(JsValue::to_boolean);
    let payload = base64::engine::general_purpose::STANDARD.decode(encoded)
        .map_err(|_| JsError::from(JsNativeError::typ().with_message("invalid WebSocket payload")))?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let connection = state.websocket_clients.get_mut(&id).ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("WebSocket is not connected"))
        })?;
        connection.client.send(payload, binary).map_err(|e| JsError::from(JsNativeError::error().with_message(e)))?;
        Ok(JsValue::undefined())
    })
}

fn websocket_poll_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as u64;
    with_host_state(|state| {
        let state = state.borrow();
        let connection = state.websocket_clients.get(&id).ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("WebSocket is not connected"))
        })?;
        let values: Vec<_> = connection.incoming.try_iter().map(|result| match result {
            WebSocketReadResult::Message(crate::realtime::WebSocketMessage::Text(data)) => serde_json::json!({"kind":"text", "data":data}),
            WebSocketReadResult::Message(crate::realtime::WebSocketMessage::Binary(data)) => serde_json::json!({"kind":"binary", "data":base64::engine::general_purpose::STANDARD.encode(data)}),
            WebSocketReadResult::Message(crate::realtime::WebSocketMessage::Close { code, reason }) => serde_json::json!({"kind":"close", "code":code, "reason":reason}),
            WebSocketReadResult::Error(error) => serde_json::json!({"kind":"error", "message":error}),
        }).collect();
        Ok(js_string!(serde_json::to_string(&values).unwrap()).into())
    })
}

fn websocket_close_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as u64;
    let code = args.get(1).cloned().unwrap_or_else(|| JsValue::from(1000)).to_number(context)? as u16;
    let reason = string_argument(args.get(2), "", context)?;
    with_host_state(|state| {
        let mut connection = state.borrow_mut().websocket_clients.remove(&id).ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("WebSocket is not connected"))
        })?;
        connection.client.close(code, &reason).map_err(|e| JsError::from(JsNativeError::error().with_message(e)))?;
        Ok(JsValue::undefined())
    })
}

fn event_source_fetch_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = string_argument(args.first(), "", context)?;
    let last_event_id = string_argument(args.get(1), "", context)?;
    let with_credentials = args.get(2).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let parsed = url.parse::<crate::http::Url>()
            .map_err(|error| JsError::from(JsNativeError::typ().with_message(error.to_string())))?;
        let origin = state.base_url.as_ref().map(CorsOrigin::from_url).unwrap_or_else(CorsOrigin::opaque);
        let mut request = HttpRequest::new(Method::Get, parsed);
        request.set_header("Accept", "text/event-stream");
        if !last_event_id.is_empty() { request.set_header("Last-Event-ID", last_event_id); }
        let HostState { http_client, cors_preflight_cache, .. } = &mut *state;
        let fetched = crate::http::cors::fetch(
            http_client, request, &origin, RequestMode::Cors,
            if with_credentials { CredentialsMode::Include } else { CredentialsMode::SameOrigin },
            RedirectMode::Follow, cors_preflight_cache,
        ).map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
        if fetched.response.status_code() != 200 {
            return Err(JsNativeError::error().with_message("EventSource response must be HTTP 200").into());
        }
        let content_type = fetched.response.header("content-type").unwrap_or_default();
        if !content_type.to_ascii_lowercase().starts_with("text/event-stream") {
            return Err(JsNativeError::error().with_message("EventSource response must be text/event-stream").into());
        }
        Ok(js_string!(String::from_utf8_lossy(fetched.response.body()).as_ref()).into())
    })
}

fn fetch_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let method_name = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| js_string!("GET").into())
        .to_string(context)?
        .to_std_string_escaped();
    let method = match method_name.as_str() {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "HEAD" => Method::Head,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "OPTIONS" => Method::Options,
        "PATCH" => Method::Patch,
        _ => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported HTTP method: {method_name}"))
                .into());
        }
    };
    let headers_json = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| js_string!("[]").into())
        .to_string(context)?
        .to_std_string_escaped();
    let headers: Vec<(String, String)> = serde_json::from_str(&headers_json).map_err(|error| {
        JsError::from(JsNativeError::typ().with_message(format!(
            "invalid request headers: {error}"
        )))
    })?;
    let body = body_bytes_argument(args.get(3), context)?;
    let mode = match string_argument(args.get(4), "cors", context)?.as_str() {
        "same-origin" => RequestMode::SameOrigin,
        "cors" => RequestMode::Cors,
        "no-cors" => RequestMode::NoCors,
        value => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported request mode: {value}"))
                .into());
        }
    };
    let credentials = match string_argument(args.get(5), "same-origin", context)?.as_str() {
        "omit" => CredentialsMode::Omit,
        "same-origin" => CredentialsMode::SameOrigin,
        "include" => CredentialsMode::Include,
        value => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported credentials mode: {value}"))
                .into());
        }
    };
    let redirect_mode = match string_argument(args.get(6), "follow", context)?.as_str() {
        "follow" => RedirectMode::Follow,
        "error" => RedirectMode::Error,
        "manual" => RedirectMode::Manual,
        value => {
            return Err(JsNativeError::typ()
                .with_message(format!("unsupported redirect mode: {value}"))
                .into());
        }
    };

    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let parsed_url = url.parse::<crate::http::Url>().map_err(|error| {
            JsError::from(JsNativeError::typ().with_message(error.to_string()))
        })?;
        let normalized_url = parsed_url.to_string();
        let origin = state
            .base_url
            .as_ref()
            .map(CorsOrigin::from_url)
            .unwrap_or_else(CorsOrigin::opaque);
        let mut request = HttpRequest::new(method, parsed_url);
        for (name, value) in headers {
            request.set_header(name, value);
        }
        if let Some(body) = body {
            request.set_body(body);
        }

        let HostState {
            http_client,
            cors_preflight_cache,
            ..
        } = &mut *state;
        let fetched = crate::http::cors::fetch(
            http_client,
            request,
            &origin,
            mode,
            credentials,
            redirect_mode,
            cors_preflight_cache,
        )
        .map_err(|error| {
            JsError::from(JsNativeError::error().with_message(error.to_string()))
        })?;
        let response = fetched.response;
        let opaque = matches!(
            fetched.response_type,
            ResponseType::Opaque | ResponseType::OpaqueRedirect
        );
        // `bodyText` is the lossy UTF-8 decoding the Fetch and XHR text paths are
        // defined in terms of, so it stays the primary representation. It cannot
        // represent a payload that is not valid UTF-8 though (an image, a font),
        // and `Response.blob()`/`arrayBuffer()` must hand back the original
        // bytes. Carry those separately, and only when decoding actually lost
        // information, so text responses pay nothing for it.
        //
        // `from_utf8_lossy` borrows when the input is already valid UTF-8 and
        // only allocates to substitute replacement characters, so an owned `Cow`
        // is the signal that bytes were lost — no second validation pass needed.
        let decoded_body = (!opaque).then(|| String::from_utf8_lossy(response.body()));
        let body_base64 = matches!(decoded_body, Some(std::borrow::Cow::Owned(_)))
            .then(|| base64::engine::general_purpose::STANDARD.encode(response.body()));
        let body_text = decoded_body.map(std::borrow::Cow::into_owned);
        let effective_url = (!opaque).then(|| {
            response
                .effective_url()
                .map(ToString::to_string)
                .unwrap_or_else(|| normalized_url.clone())
        });
        let response_type = match fetched.response_type {
            ResponseType::Basic => "basic",
            ResponseType::Cors => "cors",
            ResponseType::Opaque => "opaque",
            ResponseType::OpaqueRedirect => "opaqueredirect",
        };
        let exposed_headers =
            exposed_response_headers(&response, fetched.response_type, credentials);
        let payload = serde_json::json!({
            "status": if opaque { 0 } else { response.status_code() },
            "statusText": if opaque { "" } else { response.reason() },
            "ok": !opaque && (200..300).contains(&response.status_code()),
            "url": effective_url.as_deref().unwrap_or(""),
            "redirected": !opaque && fetched.redirected,
            "type": response_type,
            "headers": exposed_headers,
            "bodyText": body_text.as_deref().unwrap_or(""),
            "bodyBase64": body_base64,
        })
        .to_string();
        Ok(js_string!(payload.as_str()).into())
    })
}

/// Escapes `value` so it can be embedded inside a JSON string literal.
///
/// This handles the two mandatory JSON escapes (`\` and `"`), uses the short
/// forms for the common whitespace control characters (`\n`, `\r`, `\t`), and
/// escapes every remaining C0 control character (U+0000–U+001F) as a `\u00XX`
/// sequence. The C0 handling matters because computed values (e.g. a `content`
/// property or a URL) can contain arbitrary control characters — for example a
/// CSS hex escape such as `content: "\1 "` yields a literal U+0001. Emitting
/// such a byte raw would produce invalid JSON, causing the `JSON.parse()` in
/// `dom_bootstrap.js` to throw and `getComputedStyle` to silently degrade to an
/// empty `{}` object.
///
/// Used for both JSON object keys (CSS property names) and values, so the
/// output is always a valid JSON string body regardless of input.
fn escape_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            // All other C0 control characters (U+0000–U+001F) must be escaped
            // to keep the output valid JSON.
            c if (c as u32) < 0x20 => {
                escaped.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => escaped.push(c),
        }
    }
    escaped
}

// ── Additional DOM native bindings ──────────────────────────────────────────

fn get_text_content_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        match node.node_type() {
            // DocumentType returns null per DOM spec
            crate::dom::NodeType::DocumentType => Ok(JsValue::null()),
            // Text and Comment return their data
            crate::dom::NodeType::Text
            | crate::dom::NodeType::Comment
            | crate::dom::NodeType::ProcessingInstruction => {
                let data = node.data().unwrap_or_default();
                Ok(js_string!(data.as_str()).into())
            }
            // Element, Document, DocumentFragment: concatenate descendant text
            _ => {
                let text = collect_text_recursive(&node);
                Ok(js_string!(text.as_str()).into())
            }
        }
    })
}

fn collect_text_recursive(node: &NodeHandle) -> String {
    let mut text = String::new();
    for child in node.child_nodes() {
        match child.node_type() {
            crate::dom::NodeType::Text => {
                if let Some(data) = child.data() {
                    text.push_str(&data);
                }
            }
            crate::dom::NodeType::Comment
            | crate::dom::NodeType::ProcessingInstruction
            | crate::dom::NodeType::DocumentType => {}
            _ => {
                text.push_str(&collect_text_recursive(&child));
            }
        }
    }
    text
}

fn set_text_content_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let text = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let is_character_data = matches!(
            node.node_type(),
            crate::dom::NodeType::Text
                | crate::dom::NodeType::Comment
                | crate::dom::NodeType::ProcessingInstruction
        );
        let rebuild_stylesheets = !is_character_data
            || node
                .parent_node()
                .and_then(|parent| parent.tag_name())
                .is_some_and(|tag| tag.eq_ignore_ascii_case("style"));
        // For text/comment leaf nodes, update data directly
        if is_character_data {
            node.set_data(&text);
        } else {
            // Remove all children
            for child in node.child_nodes() {
                let _ = node.remove_child(&child);
            }
            // Add single text node
            if !text.is_empty() {
                let text_node = NodeHandle::text(&text);
                node.append_child(text_node);
            }
        }
        // Element textContent replaces an entire subtree and may add/remove a
        // style element. CharacterData normally only affects selector/style
        // results, except beneath <style>, where it changes stylesheet source.
        if rebuild_stylesheets {
            state.borrow_mut().mark_style_dirty_for_node(&node);
        } else {
            state.borrow_mut().invalidate_style_cache_for_node(&node);
        }
        Ok(JsValue::undefined())
    })
}

fn get_inner_html_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let html = serialize_inner_html(&node);
        Ok(js_string!(html.as_str()).into())
    })
}

fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_html_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn serialize_inner_html(node: &NodeHandle) -> String {
    let mut html = String::new();
    let children = node
        .template_content()
        .map(|content| content.child_nodes())
        .unwrap_or_else(|| node.child_nodes());
    for child in children {
        serialize_node(&child, &mut html);
    }
    html
}

fn serialize_node(node: &NodeHandle, html: &mut String) {
    match node.node_type() {
        crate::dom::NodeType::Text => {
            if let Some(data) = node.data() {
                html.push_str(&escape_html_text(&data));
            }
        }
        crate::dom::NodeType::Comment => {
            if let Some(data) = node.data() {
                html.push_str("<!--");
                html.push_str(&data);
                html.push_str("-->");
            }
        }
        crate::dom::NodeType::ProcessingInstruction => {
            html.push_str("<?");
            html.push_str(&node.node_name());
            if let Some(data) = node.data()
                && !data.is_empty()
            {
                html.push(' ');
                html.push_str(&data);
            }
            html.push_str("?>");
        }
        crate::dom::NodeType::DocumentType => {
            if let Some(name) = node.data() {
                html.push_str("<!DOCTYPE ");
                html.push_str(&name);
                if let Some(public_id) = node.public_id() {
                    html.push_str(" PUBLIC \"");
                    html.push_str(&public_id);
                    html.push_str("\" \"");
                    html.push_str(node.system_id().as_deref().unwrap_or(""));
                    html.push('"');
                } else if let Some(system_id) = node.system_id() {
                    html.push_str(" SYSTEM \"");
                    html.push_str(&system_id);
                    html.push('"');
                }
                html.push('>');
            }
        }
        crate::dom::NodeType::Element => {
            if let Some(tag) = node.tag_name() {
                html.push('<');
                html.push_str(&tag);
                if let Some(attrs) = node.attributes() {
                    for (name, value) in &attrs {
                        html.push(' ');
                        html.push_str(name);
                        html.push_str("=\"");
                        html.push_str(&escape_html_attr(value));
                        html.push('"');
                    }
                }
                html.push('>');
                html.push_str(&serialize_inner_html(node));
                html.push_str("</");
                html.push_str(&tag);
                html.push('>');
            }
        }
        _ => {
            // Document/DocumentFragment: serialize children
            html.push_str(&serialize_inner_html(node));
        }
    }
}

fn set_inner_html_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let html = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let target = node.template_content().unwrap_or_else(|| node.clone());
        for child in target.child_nodes() {
            let _ = target.remove_child(&child);
        }
        if !html.is_empty() {
            // Parse as fragment: wrap in body context and extract children
            let parsed =
                crate::html::TreeBuilder::parse(&format!("<body>{html}</body>")).document();
            let body = parsed.query_selector("body");
            let source = body.as_ref().map(|b| b.child_nodes()).unwrap_or_default();
            for child in source {
                target.append_child(child);
            }
        }
        state.borrow_mut().register_tree(&target);
        // The node stays attached to its document; invalidate that document's
        // resolver so a `<style>` inside the new markup is picked up.
        state.borrow_mut().mark_style_dirty_for_node(&node);
        Ok(JsValue::undefined())
    })
}

fn child_node_ids_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    with_host_state(|state| {
        let children = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("node not found"))
            })?;
            node.child_nodes()
        };
        {
            let mut s = state.borrow_mut();
            for child in &children {
                s.register_tree(child);
            }
        }
        let ids: Vec<JsValue> = children
            .iter()
            .map(|c| JsValue::from(c.identity() as f64))
            .collect();
        Ok(boa_engine::JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(ids, context),
        ))
    })
}

fn next_sibling_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let parent = match node.parent_node() {
            Some(p) => p,
            None => return Ok(JsValue::null()),
        };
        let siblings = parent.child_nodes();
        let mut found = false;
        for sibling in &siblings {
            if found {
                return Ok(JsValue::from(sibling.identity() as f64));
            }
            if sibling.identity() == id {
                found = true;
            }
        }
        Ok(JsValue::null())
    })
}

fn previous_sibling_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let parent = match node.parent_node() {
            Some(p) => p,
            None => return Ok(JsValue::null()),
        };
        let siblings = parent.child_nodes();
        let mut prev: Option<&NodeHandle> = None;
        for sibling in &siblings {
            if sibling.identity() == id {
                return Ok(prev
                    .map(|p| JsValue::from(p.identity() as f64))
                    .unwrap_or(JsValue::null()));
            }
            prev = Some(sibling);
        }
        Ok(JsValue::null())
    })
}

fn remove_child_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let child_id = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    with_host_state(|state| {
        let (parent, child) = {
            let state = state.borrow();
            let parent = state.get_node(parent_id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("parent not found"))
            })?;
            let child = state.get_node(child_id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("child not found"))
            })?;
            (parent, child)
        };
        // Save the parent's document *before* removing `child`: afterwards the
        // detached child has no document root, so the affected document could no
        // longer be found from it. The parent keeps its place in the tree.
        let parent_document = document_root_for_node(&parent);
        parent
            .remove_child(&child)
            .map_err(|e| JsError::from(JsNativeError::error().with_message(e.to_string())))?;
        if let Some(document) = &parent_document {
            state.borrow_mut().mark_document_style_dirty(document);
        }
        Ok(JsValue::undefined())
    })
}

fn insert_before_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let new_id = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let ref_value = args.get(2).cloned().unwrap_or_default();
    with_host_state(|state| {
        let ref_node = if ref_value.is_null() || ref_value.is_undefined() {
            None
        } else {
            let ref_id = ref_value.to_number(context)? as usize;
            state.borrow().get_node(ref_id)
        };
        let (parent, new_node) = {
            let state = state.borrow();
            let parent = state.get_node(parent_id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("parent not found"))
            })?;
            let new_node = state.get_node(new_id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("new node not found"))
            })?;
            (parent, new_node)
        };
        // Like `append_child`, the inserted node may move out of another
        // document into `parent`'s. Note both documents before the move so both
        // resolvers are invalidated and so the pair can be compared below to
        // decide whether the move re-navigates resource elements. The reference
        // node's document is irrelevant — it is `parent` (the new home) that
        // matters.
        let source_document = document_root_for_node(&new_node);
        let target_document = document_root_for_node(&parent);
        match ref_node {
            Some(ref_node) => {
                let _ = parent.insert_before(new_node.clone(), &ref_node);
            }
            None => parent.append_child(new_node.clone()),
        }
        {
            let mut state = state.borrow_mut();
            if let Some(document) = &source_document {
                state.mark_document_style_dirty(document);
            }
            if let Some(document) = &target_document {
                state.mark_document_style_dirty(document);
            }
            // See `append_child_native`: a fresh navigation for iframe/object
            // resource elements is scheduled when the move lands the subtree in
            // a live document that differs from its origin (detached origin, or
            // a direct move across two different documents), but not for an
            // in-document reorder.
            if target_document.is_some() && source_document != target_document {
                state.schedule_connected_resource_loads(&new_node, true);
            }
        }
        Ok(JsValue::undefined())
    })
}

fn query_selector_all_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let parent_id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let selector = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let selectors = parse_dom_selector_list(&selector)?;
    with_host_state(|state| {
        let results = {
            let s = state.borrow();
            let parent = s.get_node(parent_id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("node not found"))
            })?;
            query_all_matching_descendants(&parent, &selectors)
        };
        {
            let mut s = state.borrow_mut();
            for node in &results {
                s.register_tree(node);
            }
        }
        let ids: Vec<JsValue> = results
            .iter()
            .map(|n| JsValue::from(n.identity() as f64))
            .collect();
        Ok(boa_engine::JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(ids, context),
        ))
    })
}

fn matches_selector_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let selector = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let selectors = parse_dom_selector_list(&selector)?;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        Ok(JsValue::from(
            selectors
                .iter()
                .any(|selector| matches_selector(&node, selector)),
        ))
    })
}

fn parse_dom_selector_list(selector: &str) -> JsResult<Vec<Selector>> {
    parse_selector_list(selector)
        .map_err(|error| JsError::from(JsNativeError::syntax().with_message(error.to_string())))
}

fn query_first_matching_descendant(
    node: &NodeHandle,
    selectors: &[Selector],
) -> Option<NodeHandle> {
    for child in node.child_nodes() {
        if child.node_type() == NodeType::Element
            && selectors
                .iter()
                .any(|selector| matches_selector(&child, selector))
        {
            return Some(child);
        }
        if let Some(found) = query_first_matching_descendant(&child, selectors) {
            return Some(found);
        }
    }
    None
}

fn query_all_matching_descendants(node: &NodeHandle, selectors: &[Selector]) -> Vec<NodeHandle> {
    let mut results = Vec::new();
    for child in node.child_nodes() {
        if child.node_type() == NodeType::Element
            && selectors
                .iter()
                .any(|selector| matches_selector(&child, selector))
        {
            results.push(child.clone());
        }
        results.extend(query_all_matching_descendants(&child, selectors));
    }
    results
}

fn node_type_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let node_type = match node.node_type() {
            crate::dom::NodeType::Element => 1,
            crate::dom::NodeType::Text => 3,
            crate::dom::NodeType::ProcessingInstruction => 7,
            crate::dom::NodeType::Comment => 8,
            crate::dom::NodeType::Document => 9,
            crate::dom::NodeType::DocumentType => 10,
            crate::dom::NodeType::DocumentFragment => 11,
        };
        Ok(JsValue::from(node_type))
    })
}

fn clone_node_impl(node: &NodeHandle, deep: bool) -> NodeHandle {
    let clone = match node.node_type() {
        crate::dom::NodeType::Element => {
            let tag = node.tag_name().unwrap_or_default();
            let el = NodeHandle::element(&tag);
            if let Some(attrs) = node.attributes() {
                for (name, value) in &attrs {
                    el.set_attribute(name, value);
                }
            }
            el
        }
        crate::dom::NodeType::Text => NodeHandle::text(node.data().unwrap_or_default()),
        crate::dom::NodeType::Comment => NodeHandle::comment(node.data().unwrap_or_default()),
        crate::dom::NodeType::ProcessingInstruction => {
            NodeHandle::processing_instruction(node.node_name(), node.data().unwrap_or_default())
        }
        crate::dom::NodeType::Document => NodeHandle::document(),
        crate::dom::NodeType::DocumentFragment => NodeHandle::document_fragment(),
        crate::dom::NodeType::DocumentType => NodeHandle::document_type(
            node.data().unwrap_or_default(),
            node.public_id().unwrap_or_default(),
            node.system_id().unwrap_or_default(),
        ),
    };
    if deep {
        if let (Some(source_content), Some(clone_content)) =
            (node.template_content(), clone.template_content())
        {
            for child in source_content.child_nodes() {
                clone_content.append_child(clone_node_impl(&child, true));
            }
        } else {
            for child in node.child_nodes() {
                clone.append_child(clone_node_impl(&child, true));
            }
        }
    }
    clone
}

fn clone_node_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let deep = args.get(1).cloned().unwrap_or_default().to_boolean();
    with_host_state(|state| {
        let clone = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("node not found"))
            })?;
            clone_node_impl(&node, deep)
        };
        let clone_id = clone.identity() as f64;
        state.borrow_mut().register_tree(&clone);
        Ok(JsValue::from(clone_id))
    })
}

fn remove_attribute_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    let name = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        node.remove_attribute(&name);
        // Any attribute may participate in a selector, so invalidate the
        // element's live document. Detached elements affect no document yet.
        if node.tag_name().as_deref() == Some("style") {
            state.borrow_mut().mark_style_dirty_for_node(&node);
        } else {
            state.borrow_mut().invalidate_style_cache_for_node(&node);
        }
        Ok(JsValue::undefined())
    })
}

fn create_text_node_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let node = NodeHandle::text(&text);
    let id = node.identity() as f64;
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(node.identity(), node);
        Ok(JsValue::from(id))
    })
}

fn create_document_fragment_native(
    _: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let node = NodeHandle::document_fragment();
    let id = node.identity() as f64;
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(node.identity(), node);
        Ok(JsValue::from(id))
    })
}

fn template_content_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let content = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.template_content())
            .ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ().with_message("node is not an HTML template element"),
                )
            })?;
        Ok(JsValue::from(content.identity() as f64))
    })
}

fn attach_shadow_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    let mode = if args.get(1).is_some_and(JsValue::to_boolean) {
        ShadowRootMode::Closed
    } else {
        ShadowRootMode::Open
    };
    with_host_state(|state| {
        let host = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let Some(root) = host.attach_shadow(mode) else {
            return Ok(JsValue::null());
        };
        let root_id = root.identity();
        state.borrow_mut().register_tree(&root);
        Ok(JsValue::from(root_id as f64))
    })
}

fn shadow_root_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let root = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.shadow_root());
        Ok(node_to_js_value(root))
    })
}

fn shadow_host_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let host = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.shadow_host());
        Ok(node_to_js_value(host))
    })
}

fn shadow_mode_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let mode = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.shadow_root_mode());
        Ok(match mode {
            Some(ShadowRootMode::Open) => js_string!("open").into(),
            Some(ShadowRootMode::Closed) => js_string!("closed").into(),
            None => JsValue::null(),
        })
    })
}

fn assigned_slot_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let slot = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.assigned_slot())
            .filter(|slot| {
                slot.containing_shadow_root()
                    .and_then(|root| root.shadow_root_mode())
                    != Some(ShadowRootMode::Closed)
            });
        Ok(node_to_js_value(slot))
    })
}

/// Event path construction must traverse assigned slots even when the slot is
/// inside a closed shadow root. The public `assignedSlot` binding above applies
/// the required visibility filter, while this bootstrap-only binding exposes
/// the unfiltered tree relationship to the dispatch algorithm.
fn internal_assigned_slot_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let slot = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.assigned_slot());
        Ok(node_to_js_value(slot))
    })
}

fn assigned_nodes_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    let flatten = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let nodes = state
            .borrow()
            .get_node(id)
            .map(|node| node.assigned_nodes(flatten))
            .unwrap_or_default();
        let ids = nodes
            .into_iter()
            .map(|node| JsValue::from(node.identity() as f64));
        Ok(JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(ids, context),
        ))
    })
}

/// Creates an independent, initially empty Document and enrolls it in the same
/// node/style registries as the main and iframe documents.
fn create_document_native(
    _: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let document = NodeHandle::document();
    let id = document.identity();
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        state.nodes.insert(id, document);
        state.document_styles.insert(
            id,
            DocumentStyleEntry {
                resolver: None,
                dirty: true,
                needs_full_sample: true,
            },
        );
        Ok(JsValue::from(id as f64))
    })
}

/// Materialises the already validated DOMImplementation doctype descriptor so
/// createDocument can insert it into the native document tree.
fn create_document_type_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let name = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let public_id = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let system_id = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let node = NodeHandle::document_type(&name, &public_id, &system_id);
    let id = node.identity();
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(id, node);
        Ok(JsValue::from(id as f64))
    })
}

fn create_processing_instruction_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let target = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let data = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let node = NodeHandle::processing_instruction(target, data);
    let id = node.identity() as f64;
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(node.identity(), node);
        Ok(JsValue::from(id))
    })
}

fn create_comment_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let data = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let node = NodeHandle::comment(&data);
    let id = node.identity() as f64;
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(node.identity(), node);
        Ok(JsValue::from(id))
    })
}

/// Inserts `child` into `parent` before `reference` when given, otherwise
/// appends it. If `insert_before` fails — for example the reference node is no
/// longer a child of `parent` — this falls back to appending.
///
/// This is not infallible, and the two operations fail differently.
/// `insert_before` returns `Err` — `HierarchyRequest` if the insertion would
/// create a cycle (`child` is an inclusive ancestor of `parent`), or
/// `ReferenceChildNotFound` if `reference` is no longer a child of `parent`.
/// `append_child`, by contrast, never returns an error: it silently no-ops on a
/// cyclic insertion and otherwise appends. So a stale reference falls back to a
/// successful append, but a genuinely cyclic `child` is dropped either way — the
/// `insert_before` error falls through to `append_child`, which also refuses the
/// cycle and does nothing. The guarantee is therefore narrow: **as long as the
/// insertion would not create a cycle, the child lands in `parent`'s subtree**
/// (via `insert_before` or the `append` fallback) rather than being silently
/// dropped. Callers rely on this: `document.write` registers each inserted node
/// and advances its insertion point, both of which are only valid for nodes that
/// are actually in the tree.
fn insert_or_append(parent: &NodeHandle, child: &NodeHandle, reference: Option<&NodeHandle>) {
    match reference {
        Some(reference) => {
            if parent.insert_before(child.clone(), reference).is_err() {
                parent.append_child(child.clone());
            }
        }
        None => parent.append_child(child.clone()),
    }
}

/// Returns whether a `<script>`'s `type` attribute selects a classic script
/// that Omoikane executes.
///
/// This is intentionally narrower than the full "JavaScript MIME type essence
/// match" ([`is_javascript_mime_type`]): only an **absent, empty,
/// `text/javascript`, or `application/javascript`** type runs. `type="module"`
/// and every other value — including other JavaScript MIME essences such as
/// `text/ecmascript` — are treated as non-classic. Non-classic is not the same as
/// non-executable: [`ScriptKind::from_type_attribute`] routes `module` to module
/// evaluation and only everything else to no execution at all.
///
/// Both [`is_inline_classic_script`] (the `document.write` path) and
/// `Runtime::execute_document_scripts` (the normal parse path) gate on this
/// helper, so a `<script>` element runs identically no matter which path
/// reached it.
fn is_executable_classic_script_type(type_attr: Option<&str>) -> bool {
    match type_attr {
        None => true,
        Some(t) => {
            // Strip any MIME parameters (e.g. "text/javascript; charset=utf-8").
            let mime = t
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            mime.is_empty() || mime == "text/javascript" || mime == "application/javascript"
        }
    }
}

/// How a `<script>` element's `type` attribute says it should be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Classic,
    Module,
    /// A type Omoikane does not execute (`application/json`, an import map, a
    /// template language, ...). The HTML script algorithm stops before fetching
    /// such an element, so it neither runs nor fires `load`.
    NotExecutable,
}

impl ScriptKind {
    fn from_type_attribute(type_attr: Option<&str>) -> Self {
        if type_attr.is_some_and(|value| value.trim().eq_ignore_ascii_case("module")) {
            Self::Module
        } else if is_executable_classic_script_type(type_attr) {
            Self::Classic
        } else {
            Self::NotExecutable
        }
    }
}

/// Returns whether `node` is an inline classic `<script>` — one that
/// `document.write` should execute synchronously.
///
/// A script qualifies only when it has no `src` attribute (external scripts
/// carry no inline code to run) and its `type` selects a classic script that
/// Omoikane executes (see [`is_executable_classic_script_type`], which excludes
/// `type="module"` and non-executed types). This shares its type gate with
/// `execute_document_scripts`, so written and normally parsed scripts agree on
/// what runs.
fn is_inline_classic_script(node: &NodeHandle) -> bool {
    if node.tag_name().as_deref() != Some("script") {
        return false;
    }
    let attrs = node.attributes().unwrap_or_default();
    if attrs.contains_key("src") {
        return false;
    }
    is_executable_classic_script_type(attrs.get("type").map(|s| s.as_str()))
}

/// Backs `document.open()`'s reset semantics: removes every child of the given
/// document node so a following `document.write` builds fresh content into an
/// empty document (HTML's "document open steps" replace the document with an
/// empty one). Works for any Document node id, so it applies equally to the
/// top-level document and to iframe sub-documents (an iframe's
/// `contentDocument`).
///
/// Only the main document owns the parser insertion point tracked by
/// [`HostState::write_insertion_ref`], so that field is cleared **only** when
/// the main document is the one being reset. Resetting a sub-document leaves it
/// untouched: an outer `<script>` may be mid-write into the main document, and
/// clearing the point here (as an earlier version did unconditionally) would
/// silently redirect the rest of that script's `document.write` output to the
/// `<body>` tail instead of the script's position.
fn document_reset_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let (node, is_main_document) = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("document node not found"))
            })?;
            let is_main_document = node == s.document;
            (node, is_main_document)
        };
        let removed_any = !node.child_nodes().is_empty();
        for child in node.child_nodes() {
            let _ = node.remove_child(&child);
        }
        // Emptying the document (document.open) mutates its tree, so its cached
        // style resolver (and, for the main document, its layout tree) is stale.
        // Invalidate only the document being reset — a sub-document reset must
        // not touch the main document's resolver, and vice versa. `node` is the
        // document node itself, whose own document root is itself.
        if removed_any {
            state.borrow_mut().mark_style_dirty_for_node(&node);
        }
        if is_main_document {
            // The emptied main document has no insertion point; a following
            // write() appends into the now-childless document node.
            state.borrow_mut().write_insertion_ref = None;
        }
        Ok(JsValue::undefined())
    })
}

/// `__omoikane_resolve_url(reference)` -> `reference` resolved against the
/// document's base URL and serialized as an absolute URL string. Backs URL IDL
/// attribute reflection (e.g. `HTMLObjectElement.data`), which must expose an
/// absolute URL rather than the raw attribute value.
///
/// Unlike [`crate::http::url::resolve_url`] (which targets request URLs and so
/// unconditionally drops any `#fragment`), URL IDL reflection must preserve the
/// fragment. This wrapper therefore:
///
/// - splits the reference at the first `#`, resolves only the part before it,
///   then re-attaches the `#fragment` to the resolved result;
/// - treats an empty reference (empty once the fragment is removed) as resolving
///   to the base URL itself (RFC 3986 §5.2), so `""` reflects the base URL and
///   `"#frag"` reflects the base URL plus that fragment — rather than being
///   resolved as a relative path against the base directory;
/// - falls back to the raw reference (fragment included) when there is no base
///   URL, or when resolution of the non-fragment part fails (e.g. a non-HTTP(S)
///   scheme such as `mailto:`), matching the spec's "return the attribute value"
///   fallback. A missing argument yields the empty string.
fn resolve_url_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let reference = match args.first() {
        Some(value) => value.to_string(context)?.to_std_string_escaped(),
        None => return Ok(js_string!("").into()),
    };
    with_host_state(|state| {
        let base = state.borrow().base_url.clone();
        let resolved = match base {
            Some(base) => {
                // Preserve any fragment: resolve only the part before the first
                // `#`, then re-attach `#fragment` to the resolved output.
                let (without_fragment, fragment) = match reference.split_once('#') {
                    Some((before, after)) => (before, Some(after)),
                    None => (reference.as_str(), None),
                };
                // An empty reference (RFC 3986 §5.2) resolves to the base URL
                // itself. `None` marks a resolution failure -> raw fallback.
                let base_part = if without_fragment.is_empty() {
                    Some(base.to_string())
                } else {
                    crate::http::url::resolve_url(&base, without_fragment)
                        .ok()
                        .map(|url| url.to_string())
                };
                match base_part {
                    Some(mut s) => {
                        if let Some(frag) = fragment {
                            s.push('#');
                            s.push_str(frag);
                        }
                        s
                    }
                    None => reference.clone(),
                }
            }
            None => reference,
        };
        Ok(js_string!(resolved.as_str()).into())
    })
}

fn schedule_navigation_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let kind = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let value = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let state_json = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| js_string!("null").into())
        .to_string(context)?
        .to_std_string_escaped();
    let request = match kind.as_str() {
        "assign" => NavigationRequest::Navigate {
            url: value,
            replace: false,
        },
        "replace" => NavigationRequest::Navigate {
            url: value,
            replace: true,
        },
        "reload" => NavigationRequest::Reload,
        "push-state" => NavigationRequest::UpdateHistory {
            url: value,
            replace: false,
            state_json,
        },
        "replace-state" => NavigationRequest::UpdateHistory {
            url: value,
            replace: true,
            state_json,
        },
        "traverse" => NavigationRequest::Traverse {
            delta: value.parse::<i32>().unwrap_or(0),
        },
        _ => {
            return Err(JsNativeError::typ()
                .with_message("unknown navigation request kind")
                .into());
        }
    };
    with_host_state(|state| {
        state.borrow_mut().event_loop.enqueue_navigation(request);
        Ok(JsValue::undefined())
    })
}

fn submit_form_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let string_arg = |index: usize, context: &mut Context| -> JsResult<String> {
        Ok(args
            .get(index)
            .cloned()
            .unwrap_or_default()
            .to_string(context)?
            .to_std_string_escaped())
    };
    let url = string_arg(0, context)?;
    let method = string_arg(1, context)?;
    let body = body_bytes_argument(args.get(2), context)?;
    let content_type = if args.get(3).is_none_or(JsValue::is_null_or_undefined) {
        None
    } else {
        Some(string_arg(3, context)?)
    };
    let request = NavigationRequest::FormSubmit {
        url,
        method,
        body,
        content_type,
    };
    with_host_state(|state| {
        state.borrow_mut().event_loop.enqueue_navigation(request);
        Ok(JsValue::undefined())
    })
}

/// Backs `document.write` / `document.writeln`.
///
/// The written text is parsed one of two ways depending on the target
/// document's state:
///
/// - **Complete document** — when the target document has no `documentElement`
///   (a root `<html>`), as is the case immediately after `document.open()`
///   emptied it. The text is parsed as a whole document with
///   [`crate::html::TreeBuilder::parse`], and the parsed document's children
///   (a `<!DOCTYPE>` plus the implicit `<html>`/`<head>`/`<body>` structure)
///   are appended to the target document. This reproduces the doctype and the
///   head/body split that Acid3 test 71 checks.
/// - **Fragment** — while a `documentElement` already exists (a normal
///   mid-parse write). The text is tokenized as an HTML fragment (in `<body>`
///   context) and its nodes are spliced into the live tree at the current
///   insertion point, preserving the streaming behaviour below.
///
/// For the fragment case:
///
/// - While a `<script>` is executing (see
///   [`HostState::write_insertion_ref`]), the fragment is inserted as the
///   script's following siblings, and the reference advances so subsequent
///   writes stay in document order — mirroring a streaming parser resuming at
///   the tokenizer insertion point.
/// - Outside of script execution (e.g. from a timer), the fragment is appended
///   to `<body>` rather than triggering the destructive `document.open()`
///   reset, which no supported page relies on.
///
/// Newly inserted nodes are registered so they are reachable by id from JS.
/// Returns an array of the ids of the *inline classic* `<script>` elements in
/// the written fragment, in document order, so the JS wrapper can execute them
/// synchronously (classic `document.write('<script>...')` behaviour). External
/// (`src`) and `type="module"` scripts are inserted into the tree but not
/// returned, because the JS wrapper only eval()s an element's text content:
/// that text is empty for `src` scripts and must not run synchronously as a
/// classic script for modules. See [`is_inline_classic_script`].
///
/// Known limitations (tracked as follow-ups, out of scope for 016-7):
/// - When a single write fragment mixes a `<script>` with following nodes, the
///   spec's streaming insertion point would run the script *before* parsing the
///   later nodes. Here every node is spliced in first and the scripts run
///   afterward, so the script sees siblings that a streaming parser would not
///   yet have created, and a nested `document.write` from that script inserts
///   after those later nodes rather than immediately after the script.
/// - There is no recursion-depth guard: a written script that itself writes a
///   script (and so on) recurses through the JS wrapper unbounded and can
///   overflow the stack. No supported page does this today.
fn document_write_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    // `document.write` passes the target document's node id first, so a write to
    // an iframe sub-document (`iframe.contentDocument.write(...)`) is routed to
    // that sub-document rather than the top-level document.
    let target_id = parse_node_id(args.first(), context)?;
    let text = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();

    with_host_state(|state| {
        // The document being written to. `document.write` passes its id; an
        // unresolved id falls back to the top-level document.
        let (target_doc, is_main) = {
            let s = state.borrow();
            let target_doc = s.get_node(target_id).unwrap_or_else(|| s.document.clone());
            let is_main = target_doc == s.document;
            (target_doc, is_main)
        };
        // A `documentElement` (root `<html>`) exists unless `document.open()`
        // just emptied the document. Its absence selects the complete-document
        // parse; its presence keeps the fragment/insertion-point behaviour.
        let has_document_element = target_doc
            .child_nodes()
            .iter()
            .any(|n| n.tag_name().as_deref() == Some("html"));

        // Parse the written text and decide where its nodes are spliced in.
        // `parent == None` means there is nothing to insert (empty write).
        let (parsed_children, parent, reference_child): (
            Vec<NodeHandle>,
            Option<NodeHandle>,
            Option<NodeHandle>,
        ) = if text.is_empty() {
            (Vec::new(), None, None)
        } else if !has_document_element {
            // Complete-document parse into the emptied document: append the
            // parsed throwaway document's children ([doctype, html]) to the
            // target document itself, reproducing the implicit
            // html/head/body structure a streaming parser would build.
            let parsed = crate::html::TreeBuilder::parse(&text).document();
            (parsed.child_nodes(), Some(target_doc.clone()), None)
        } else {
            // Fragment parse spliced at the insertion point. Parsing per-call
            // matches the innerHTML path; a single write() call must contain
            // balanced-enough markup (Acid3 writes its whole fragment in one
            // call), which is the common case.
            let parsed =
                crate::html::TreeBuilder::parse(&format!("<body>{text}</body>")).document();
            let children = parsed
                .query_selector("body")
                .map(|body| body.child_nodes())
                .unwrap_or_default();
            // Fallback target when there is no active insertion point: the
            // target document's <body>, or the document node itself.
            let fallback_parent = || {
                target_doc
                    .query_selector("body")
                    .unwrap_or_else(|| target_doc.clone())
            };
            let (parent, reference_child) = if is_main {
                let s = state.borrow();
                match s.write_insertion_ref.clone() {
                    // Active insertion point: insert right after the reference
                    // node (i.e. before the reference node's next sibling).
                    Some(anchor) => match anchor.parent_node() {
                        Some(parent) => {
                            let siblings = parent.child_nodes();
                            let next = siblings
                                .iter()
                                .position(|n| n == &anchor)
                                .and_then(|i| siblings.get(i + 1).cloned());
                            (Some(parent), next)
                        }
                        // The anchor was detached from the tree; fall back.
                        None => (Some(fallback_parent()), None),
                    },
                    // No script running: append to the fallback parent.
                    None => (Some(fallback_parent()), None),
                }
            } else {
                // Sub-browsing-context documents have no per-frame parser
                // insertion point here; append the written fragment to the
                // sub-document's <body>. The main document's insertion point is
                // left untouched.
                (Some(fallback_parent()), None)
            };
            (children, parent, reference_child)
        };

        let Some(parent) = parent else {
            // No insertion target (empty write); nothing to do.
            return Ok(JsValue::from(
                boa_engine::object::builtins::JsArray::from_iter(Vec::<JsValue>::new(), context),
            ));
        };

        // Splice the parsed nodes in, preserving order. `insert_or_append`
        // lands each child in the tree even if `insert_before` fails on a stale
        // reference — but only as long as the insertion would not create a cycle
        // (see its docs). For `document.write` that proviso always holds:
        // `parsed_children` is a freshly parsed fragment, so its nodes cannot be
        // ancestors of `parent` and no cycle can form. Every child therefore
        // lands in the tree, keeping the register/advance steps below consistent.
        let mut last_inserted: Option<NodeHandle> = None;
        for child in &parsed_children {
            insert_or_append(&parent, child, reference_child.as_ref());
            last_inserted = Some(child.clone());
        }

        // Register the freshly inserted subtree and advance the insertion point.
        {
            let mut s = state.borrow_mut();
            for child in &parsed_children {
                s.register_tree(child);
                s.schedule_connected_resource_loads(child, true);
            }
            // `document.write` splices new nodes into the live tree, so the
            // written document's cached resolver is now stale. Invalidate only
            // that document (`target_doc`) so a write into an iframe sub-document
            // does not pollute the main document's resolver and vice versa. A
            // following `getComputedStyle` then re-collects the written `<style>`
            // / element instead of returning pre-write results.
            if !parsed_children.is_empty() {
                s.mark_document_style_dirty(&target_doc);
            }
            // Only the top-level document's parser insertion point advances; a
            // sub-document write must never repoint the main document's anchor
            // at one of the sub-document's nodes.
            if is_main
                && let Some(last) = last_inserted
                && s.write_insertion_ref.is_some()
            {
                s.write_insertion_ref = Some(last);
            }
        }

        // Collect the inline classic <script> descendants in document order for
        // the JS wrapper to execute. External (`src`) and module scripts are
        // left in the tree but not returned (see `is_inline_classic_script`).
        let mut script_nodes = Vec::new();
        for child in &parsed_children {
            collect_script_elements_recursive(child, &mut script_nodes);
        }
        let script_ids: Vec<JsValue> = script_nodes
            .iter()
            .filter(|n| is_inline_classic_script(n))
            .map(|n| JsValue::from(n.identity() as f64))
            .collect();
        Ok(JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(script_ids, context),
        ))
    })
}

/// `__omoikane_iframe_content_document(iframeId)` — returns the node id of the
/// sub-browsing-context document owned by an `<iframe>` element, loading it on
/// first access. Returns `null` if the node id does not resolve.
fn iframe_content_document_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let iframe = state.borrow().get_node(node_id);
        match iframe {
            Some(iframe) => {
                let document = state.borrow_mut().iframe_content_document(&iframe);
                Ok(JsValue::from(document.identity() as f64))
            }
            None => Ok(JsValue::null()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::native_function::NativeCallSuspension;
    use boa_gc::{Gc, GcRefCell};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::net::TcpStream;
    use std::task::{Context as FutureContext, Poll, Waker};
    use std::thread;

    fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_len.is_none()
                && let Some(headers_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
            {
                let headers = String::from_utf8_lossy(&request[..headers_end]);
                let content_len = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                expected_len = Some(headers_end + 4 + content_len);
            }
            if expected_len.is_some_and(|expected| request.len() >= expected) {
                break;
            }
        }
        request
    }

    fn sample_document() -> NodeHandle {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let main = NodeHandle::element("main");

        main.set_attribute("id", "app");
        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(main);
        document
    }

    #[test]
    fn creates_runtime_and_evaluates_scripts() {
        let mut runtime = JsRuntime::new().unwrap();
        let value = runtime.eval("1 + 2 + 3").unwrap();

        assert_eq!(value.as_number(), Some(6.0));
    }

    fn poll_until_dialog<F>(
        mut evaluation: Pin<&mut F>,
        controller: &JavaScriptDialogController,
        cx: &mut FutureContext<'_>,
    ) -> JavaScriptDialog
    where
        F: Future<Output = JsResult<JsValue>>,
    {
        for _ in 0..64 {
            if let Some(dialog) = controller.pending() {
                return dialog;
            }
            if let Poll::Ready(result) = evaluation.as_mut().poll(cx) {
                panic!("evaluation completed before opening a dialog: {result:?}");
            }
        }
        panic!("evaluation did not open a dialog within the poll budget");
    }

    fn poll_until_ready<F>(
        mut evaluation: Pin<&mut F>,
        cx: &mut FutureContext<'_>,
    ) -> JsResult<JsValue>
    where
        F: Future<Output = JsResult<JsValue>>,
    {
        for _ in 0..64 {
            if let Poll::Ready(result) = evaluation.as_mut().poll(cx) {
                return result;
            }
        }
        panic!("evaluation did not complete within the poll budget");
    }

    #[test]
    fn alert_blocks_script_until_exactly_once_resolution() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut evaluation = Box::pin(runtime.eval_async(
            "globalThis.afterAlert = false; alert('Stop'); afterAlert = true; afterAlert",
        ));
        let waker: &'static Waker = Waker::noop();
        let mut cx = FutureContext::from_waker(waker);

        let dialog = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        assert_eq!(
            dialog,
            JavaScriptDialog {
                id: 1,
                kind: JavaScriptDialogKind::Alert,
                message: "Stop".to_string(),
                default_prompt: None,
            }
        );
        assert_eq!(
            controller.handle(dialog.id + 1, true, None),
            Err(JavaScriptDialogError::StaleDialog {
                expected: dialog.id,
                actual: dialog.id + 1,
            })
        );
        assert_eq!(controller.pending(), Some(dialog.clone()));

        controller.handle(dialog.id, true, None).unwrap();
        assert_eq!(
            controller.handle(dialog.id, true, None),
            Err(JavaScriptDialogError::NoPendingDialog)
        );
        assert_eq!(
            poll_until_ready(evaluation.as_mut(), &mut cx)
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn confirm_and_prompt_resume_with_typed_results() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut evaluation = Box::pin(runtime.eval_async(
            "const confirmed = confirm('Continue?');\n             const name = prompt('Name', 'Ada');\n             `${confirmed}:${name}`",
        ));
        let waker: &'static Waker = Waker::noop();
        let mut cx = FutureContext::from_waker(waker);

        let confirm = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        assert_eq!(confirm.kind, JavaScriptDialogKind::Confirm);
        assert_eq!(confirm.message, "Continue?");
        assert_eq!(confirm.default_prompt, None);
        controller.handle(confirm.id, false, None).unwrap();

        let prompt = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        assert_eq!(prompt.kind, JavaScriptDialogKind::Prompt);
        assert_eq!(prompt.message, "Name");
        assert_eq!(prompt.default_prompt.as_deref(), Some("Ada"));
        controller
            .handle(prompt.id, true, Some("Grace".to_string()))
            .unwrap();

        let result = poll_until_ready(evaluation.as_mut(), &mut cx).unwrap();
        assert_eq!(
            result.as_string().unwrap().to_std_string_escaped(),
            "false:Grace"
        );
    }

    #[test]
    fn dismissed_prompt_returns_null_and_dropped_eval_clears_dialog() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut evaluation = Box::pin(runtime.eval_async("prompt('Name', 'Ada') === null"));
        let waker: &'static Waker = Waker::noop();
        let mut cx = FutureContext::from_waker(waker);

        let prompt = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        controller.handle(prompt.id, false, None).unwrap();
        assert_eq!(
            poll_until_ready(evaluation.as_mut(), &mut cx)
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        drop(evaluation);

        let mut cancelled = Box::pin(runtime.eval_async("alert('Cancelled')"));
        let dialog = poll_until_dialog(cancelled.as_mut(), &controller, &mut cx);
        drop(cancelled);
        assert_eq!(controller.pending(), None);
        assert_eq!(
            controller.handle(dialog.id, true, None),
            Err(JavaScriptDialogError::NoPendingDialog)
        );
        assert_eq!(runtime.eval("1 + 1").unwrap().as_number(), Some(2.0));
    }

    #[test]
    fn synchronous_dialog_attempt_is_rejected_without_leaking_state() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_eq!(
            runtime
                .eval("[alert.length, confirm.length, prompt.length].join(',')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "1,1,2"
        );

        assert!(runtime.eval("alert('cannot block sync eval')").is_err());
        assert_eq!(runtime.pending_javascript_dialog(), None);
        assert_eq!(runtime.eval("6 * 7").unwrap().as_number(), Some(42.0));
    }

    #[test]
    fn synchronous_document_and_module_dialog_attempts_do_not_leak_state() {
        let mut runtime = JsRuntime::new().unwrap();

        let (script_result, ..) = runtime.eval_safe_timed("alert('document script')");
        assert!(script_result.is_err());
        assert_eq!(runtime.pending_javascript_dialog(), None);

        let (module_result, ..) =
            runtime.eval_module_timed("alert('module script')", "https://example.test/a.js");
        assert!(module_result.is_err());
        assert_eq!(runtime.pending_javascript_dialog(), None);

        assert_eq!(runtime.eval("6 * 7").unwrap().as_number(), Some(42.0));
    }

    #[test]
    fn synchronous_serializer_callbacks_do_not_leak_dialog_state() {
        let mut runtime = JsRuntime::new().unwrap();
        for source in [
            "({ toJSON() { alert('serializer toJSON'); return 1; } })",
            "({ get value() { alert('serializer getter'); return 1; } })",
        ] {
            let value = runtime.eval(source).unwrap();
            assert!(
                runtime
                    .call_function_with_value("value => JSON.stringify(value)", value)
                    .is_err()
            );
            assert_eq!(runtime.pending_javascript_dialog(), None);
        }
        assert_eq!(runtime.eval("6 * 7").unwrap().as_number(), Some(42.0));
    }

    #[test]
    fn dialog_arguments_follow_alert_overload_and_web_idl_defaults() {
        let mut runtime = JsRuntime::new().unwrap();
        let controller = runtime.javascript_dialog_controller();
        let mut evaluation = Box::pin(runtime.eval_async(
            "alert(); alert(undefined); confirm(undefined); prompt(undefined, undefined); 'done'",
        ));
        let waker: &'static Waker = Waker::noop();
        let mut cx = FutureContext::from_waker(waker);

        let alert_omitted = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        assert_eq!(alert_omitted.message, "");
        controller.handle(alert_omitted.id, true, None).unwrap();

        let alert_undefined = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        assert_eq!(alert_undefined.message, "undefined");
        controller.handle(alert_undefined.id, true, None).unwrap();

        let confirm_undefined = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        assert_eq!(confirm_undefined.kind, JavaScriptDialogKind::Confirm);
        assert_eq!(confirm_undefined.message, "");
        controller
            .handle(confirm_undefined.id, true, None)
            .unwrap();

        let prompt_undefined = poll_until_dialog(evaluation.as_mut(), &controller, &mut cx);
        assert_eq!(prompt_undefined.kind, JavaScriptDialogKind::Prompt);
        assert_eq!(prompt_undefined.message, "");
        assert_eq!(prompt_undefined.default_prompt.as_deref(), Some(""));
        controller
            .handle(prompt_undefined.id, true, None)
            .unwrap();

        assert_eq!(
            poll_until_ready(evaluation.as_mut(), &mut cx)
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "done"
        );
    }

    #[test]
    fn async_eval_suspends_and_resumes_a_native_host_call() {
        let mut runtime = JsRuntime::new().unwrap();
        let slot = Gc::new(GcRefCell::new(None::<NativeCallSuspension>));
        let after_host_call = Gc::new(GcRefCell::new(false));
        runtime
            .context
            .register_global_builtin_callable(
                js_string!("suspendHostCall"),
                0,
                NativeFunction::from_copy_closure_with_captures(
                    |_, _, slot, context| {
                        *slot.borrow_mut() = Some(context.suspend_native_call()?);
                        Ok(JsValue::undefined())
                    },
                    slot.clone(),
                ),
            )
            .unwrap();
        runtime
            .context
            .register_global_builtin_callable(
                js_string!("markAfterHostCall"),
                0,
                NativeFunction::from_copy_closure_with_captures(
                    |_, _, after_host_call, _| {
                        *after_host_call.borrow_mut() = true;
                        Ok(JsValue::undefined())
                    },
                    after_host_call.clone(),
                ),
            )
            .unwrap();

        let mut evaluation = Box::pin(runtime.eval_async(
            "const answer = suspendHostCall(); markAfterHostCall(); answer + 1",
        ));
        let waker: &'static Waker = Waker::noop();
        let mut cx = FutureContext::from_waker(waker);

        let reached_suspension = (0..8).any(|_| match evaluation.as_mut().poll(&mut cx) {
            Poll::Pending => slot.borrow().is_some(),
            Poll::Ready(result) => {
                panic!("evaluation completed before the host call suspended: {result:?}")
            }
        });
        assert!(reached_suspension, "evaluation did not reach the host call");
        assert!(!*after_host_call.borrow());
        slot.borrow()
            .as_ref()
            .unwrap()
            .resume(Ok(JsValue::from(41)))
            .unwrap();

        let result = (0..8)
            .find_map(|_| match evaluation.as_mut().poll(&mut cx) {
                Poll::Ready(result) => Some(result),
                Poll::Pending => None,
            })
            .expect("resuming the host call must eventually complete evaluation");
        assert_eq!(result.unwrap().as_number(), Some(42.0));
        assert!(*after_host_call.borrow());
    }

    #[test]
    fn dropping_async_eval_cancels_its_suspended_host_call() {
        let mut runtime = JsRuntime::new().unwrap();
        let slot = Gc::new(GcRefCell::new(None::<NativeCallSuspension>));
        runtime
            .context
            .register_global_builtin_callable(
                js_string!("suspendHostCall"),
                0,
                NativeFunction::from_copy_closure_with_captures(
                    |_, _, slot, context| {
                        *slot.borrow_mut() = Some(context.suspend_native_call()?);
                        Ok(JsValue::undefined())
                    },
                    slot.clone(),
                ),
            )
            .unwrap();

        let mut evaluation = Box::pin(runtime.eval_async("suspendHostCall()"));
        let waker: &'static Waker = Waker::noop();
        let mut cx = FutureContext::from_waker(waker);
        let reached_suspension = (0..8).any(|_| match evaluation.as_mut().poll(&mut cx) {
            Poll::Pending => slot.borrow().is_some(),
            Poll::Ready(result) => {
                panic!("evaluation completed before the host call suspended: {result:?}")
            }
        });
        assert!(reached_suspension, "evaluation did not reach the host call");
        drop(evaluation);

        assert!(
            slot.borrow()
                .as_ref()
                .unwrap()
                .resume(Ok(JsValue::undefined()))
                .is_err()
        );
        assert_eq!(runtime.eval("1 + 1").unwrap().as_number(), Some(2.0));
    }

    #[test]
    fn promise_executor_runtime_limit_returns_an_error_without_panicking() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .context
            .runtime_limits_mut()
            .set_loop_iteration_limit(10);

        let error = runtime
            .eval("new Promise(() => { for (let i = 0; i < 1_000; ++i) {} })")
            .expect_err("the runtime limit must escape Promise construction");

        assert!(
            error
                .as_native()
                .is_some_and(JsNativeError::is_runtime_limit)
        );
    }

    #[test]
    fn canvas_2d_pixels_transform_clip_image_data_draw_image_and_png() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = eval_str(&mut runtime, r#"(() => {
          const canvas=document.createElement('canvas'); canvas.width=6; canvas.height=4;
          const ctx=canvas.getContext('2d');
          ctx.fillStyle='#ff0000'; ctx.fillRect(0,0,2,2);
          ctx.save(); ctx.translate(2,0); ctx.fillStyle='#0000ff'; ctx.fillRect(0,0,2,2); ctx.restore();
          ctx.beginPath(); ctx.rect(0,2,3,2); ctx.clip(); ctx.fillStyle='#00ff00'; ctx.fillRect(0,2,6,2);
          const patch=ctx.createImageData(1,1); patch.data.set([9,8,7,255]); ctx.putImageData(patch,5,3);
          const copy=document.createElement('canvas'); copy.width=2; copy.height=2; copy.getContext('2d').drawImage(canvas,0,0,2,2,0,0,2,2);
          const p=ctx.getImageData(0,0,6,4).data;
          globalThis.canvasUrl=canvas.toDataURL('image/png');
          globalThis.canvasWidth=ctx.measureText('abcd').width;
          globalThis.copyPixel=Array.from(copy.getContext('2d').getImageData(0,0,1,1).data).join(',');
          return [p.slice(0,4),p.slice(8,12),p.slice(48,52),p.slice(92,96)].map(x=>Array.from(x).join(',')).join('|');
        })()"#);
        assert_eq!(result, "255,0,0,255|0,0,255,255|0,255,0,255|9,8,7,255");
        assert_eq!(eval_str(&mut runtime, "copyPixel"), "255,0,0,255");
        assert_eq!(eval_str(&mut runtime, "String(canvasWidth)"), "24");
        let data_url = eval_str(&mut runtime, "canvasUrl");
        let png = base64::engine::general_purpose::STANDARD.decode(data_url.split_once(',').unwrap().1).unwrap();
        let image = crate::paint::Image::decode_png(&png).unwrap();
        assert_eq!((image.width(), image.height()), (6, 4));
        runtime.eval("globalThis.canvas = document.createElement('canvas'); canvas.width=2; globalThis.c=canvas.getContext('2d'); c.fillRect(0,0,1,1); canvas.width=2;").unwrap();
        assert_eq!(eval_str(&mut runtime, "Array.from(c.getImageData(0,0,1,1).data).join(',')"), "0,0,0,0");

        let mut source = crate::paint::Canvas::new(1, 1);
        source.set_pixel(0, 0, crate::paint::Color::rgb(12, 34, 56));
        let encoded = base64::engine::general_purpose::STANDARD.encode(source.encode_png());
        let script = format!(r#"(() => {{
          const image=document.createElement('img');
          image.src='data:image/png;base64,{encoded}';
          const target=document.createElement('canvas'); target.width=1; target.height=1;
          target.getContext('2d').drawImage(image,0,0);
          return Array.from(target.getContext('2d').getImageData(0,0,1,1).data).join(',');
        }})()"#);
        assert_eq!(eval_str(&mut runtime, &script), "12,34,56,255");
        assert_eq!(eval_str(&mut runtime, r#"(() => {
          const target=document.createElement('canvas'); target.width=5; target.height=5;
          const context=target.getContext('2d');
          context.beginPath(); context.rect(0,0,5,5); context.rect(1,1,3,3); context.clip('evenodd');
          context.fillStyle='red'; context.fillRect(0,0,5,5);
          const outer=context.getImageData(0,0,1,1).data;
          const hole=context.getImageData(2,2,1,1).data;
          let error=''; try { context.getImageData(0,0,0,1); } catch (value) { error=value.name; }
          return [outer[0],outer[3],hole[3],error].join(',');
        })()"#), "255,255,0,IndexSizeError");
    }

    #[test]
    fn websocket_api_echo_close_and_networking_task_order() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
            let key = request.lines().find_map(|line| line.strip_prefix("Sec-WebSocket-Key: ")).unwrap();
            write!(stream, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", crate::cdp::websocket_accept_key(key)).unwrap();
            let mut bytes = Vec::new();
            let message = loop {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
                if let Ok((frame, consumed)) = crate::cdp::WebSocketFrame::decode(&bytes)
                    && consumed == bytes.len() { break frame; }
            };
            assert_eq!(message.payload, b"hello");
            stream.write_all(&crate::cdp::WebSocketFrame::text("hello").encode(false)).unwrap();
            let mut close = Vec::new();
            loop {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).unwrap();
                close.push(byte[0]);
                if crate::cdp::WebSocketFrame::decode(&close).is_ok() { break; }
            }
        });
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval(&format!(r#"
            globalThis.realtimeLog = [];
            const socket = new WebSocket("ws://{address}/echo");
            socket.onopen = () => {{ realtimeLog.push("open"); socket.send("hello"); }};
            socket.onmessage = event => {{ globalThis.messageOrigin = event.origin; realtimeLog.push("message:" + event.data); socket.close(1000, "done"); }};
            socket.onclose = event => realtimeLog.push("close:" + event.code);
            setTimeout(() => realtimeLog.push("timer"), 0);
        "#)).unwrap();
        runtime.tick(0).unwrap();
        runtime.run_timers(1_000, 1, 2_000);
        assert_eq!(eval_str(&mut runtime, "realtimeLog.join('|')"), "open|timer|message:hello|close:1000");
        assert_eq!(eval_str(&mut runtime, "messageOrigin"), format!("ws://{address}"));
        server.join().unwrap();
    }

    #[test]
    fn event_source_parses_events_and_reconnects_with_last_event_id() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
                let request_lower = request.to_ascii_lowercase();
                assert!(request_lower.contains("accept: text/event-stream"));
                if attempt == 1 { assert!(request_lower.contains("last-event-id: 7")); }
                let body = if attempt == 0 {
                    "id: 7\nevent: update\ndata: one\ndata: two\nretry: 1\n\n"
                } else {
                    "id: 8\nevent: update\ndata: done\n\n"
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/page").parse().unwrap());
        runtime.eval(&format!(r#"
            globalThis.sseLog = [];
            const source = new EventSource("http://{address}/events");
            source.onerror = () => sseLog.push("error");
            source.addEventListener("update", event => {{
              globalThis.sseOrigin = event.origin;
              sseLog.push(event.data + ":" + event.lastEventId);
              if (sseLog.length === 2) source.close();
            }});
        "#)).unwrap();
        runtime.run_jobs().unwrap();
        runtime.run_until_idle().unwrap();
        runtime.run_timers(20, 1, 100);
        assert_eq!(eval_str(&mut runtime, "sseLog.join('|')"), "one\ntwo:7|done:8");
        assert_eq!(eval_str(&mut runtime, "String(source.readyState)"), "2");
        assert_eq!(eval_str(&mut runtime, "sseOrigin"), format!("http://{address}"));
        server.join().unwrap();
    }

    #[test]
    fn active_host_state_is_restored_after_panic() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        let outer_runtime = JsRuntime::new().unwrap();
        let outer_state = Rc::clone(&outer_runtime.host_state);
        ACTIVE_HOST_STATE.with(|slot| slot.replace(Some(Rc::clone(&outer_state))));

        let mut inner_runtime = JsRuntime::new().unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            inner_runtime.with_active_host_value(|_| panic!("active host panic test"));
        }));

        assert!(result.is_err());
        ACTIVE_HOST_STATE.with(|slot| {
            let restored = slot
                .replace(None)
                .expect("outer host state should be restored");
            assert!(Rc::ptr_eq(&restored, &outer_state));
        });
    }

    #[test]
    fn can_register_and_clear_timeout_from_rust() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.counter = 0;").unwrap();

        let id = runtime.set_timeout("globalThis.counter += 1;", 10);
        runtime.clear_timer(id);
        runtime.tick(10).unwrap();

        let value = runtime.eval("globalThis.counter").unwrap();
        assert_eq!(value.as_number(), Some(0.0));
    }

    #[test]
    fn runs_timeout_and_interval_tasks() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.counter = 0;").unwrap();
        runtime.set_timeout("globalThis.counter += 1;", 0);
        runtime.set_interval("globalThis.counter += 2;", 5);

        runtime.tick(0).unwrap();
        assert_eq!(
            runtime.eval("globalThis.counter").unwrap().as_number(),
            Some(1.0)
        );

        runtime.tick(5).unwrap();
        assert_eq!(
            runtime.eval("globalThis.counter").unwrap().as_number(),
            Some(3.0)
        );
    }

    #[test]
    fn exposes_timer_functions_to_javascript() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.counter = 0;").unwrap();
        runtime
            .eval(r#"setTimeout("globalThis.counter = 7", 0);"#)
            .unwrap();
        runtime.tick(0).unwrap();

        assert_eq!(
            runtime.eval("globalThis.counter").unwrap().as_number(),
            Some(7.0)
        );
    }

    #[test]
    fn runs_promise_jobs_via_job_queue() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.value = 0;").unwrap();
        runtime
            .eval("Promise.resolve(21).then(v => { globalThis.value = v * 2; });")
            .unwrap();
        runtime.run_jobs().unwrap();

        let value = runtime.eval("globalThis.value").unwrap();
        assert_eq!(value.as_number(), Some(42.0));
    }

    #[test]
    fn exposes_document_search_apis() {
        let mut runtime = JsRuntime::with_document(sample_document()).unwrap();

        let by_id = runtime
            .eval("document.getElementById('app').nodeName")
            .unwrap();
        let by_selector = runtime
            .eval("document.querySelector('#app').nodeName")
            .unwrap();

        assert_eq!(by_id.as_string().unwrap().to_std_string_escaped(), "MAIN");
        assert_eq!(
            by_selector.as_string().unwrap().to_std_string_escaped(),
            "MAIN"
        );
    }

    #[test]
    fn supports_dom_creation_and_append_child() {
        let mut runtime = JsRuntime::with_document(sample_document()).unwrap();
        runtime
            .eval(
                "const child = document.createElement('section'); \
                 child.id = 'created'; \
                 document.getElementById('app').appendChild(child);",
            )
            .unwrap();

        let found = runtime.document().query_selector("#created");
        assert!(found.is_some());
        assert_eq!(found.unwrap().tag_name().as_deref(), Some("section"));
    }

    #[test]
    fn traversal_registry_sweeps_unreachable_objects_after_repeated_create() {
        let mut runtime = JsRuntime::with_document(sample_document()).unwrap();
        runtime
            .eval(
                "for (let i = 0; i < 5000; i++) { \
                    document.createRange(); \
                    document.createNodeIterator(document); \
                }",
            )
            .unwrap();

        // WeakRef targets are deliberately kept alive until the end of their
        // current ECMAScript job. Force a collection between evaluations, then
        // read the diagnostic (which performs the same sweep as registration
        // and DOM mutation).
        runtime.context.clear_kept_objects();
        boa_gc::force_collect();
        let result = runtime
            .eval(
                r#"
                const counts = __omoikane_traversal_registry_counts(document);
                `${counts.ranges},${counts.iterators}`;
                "#,
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();

        assert_eq!(
            result, "0,0",
            "unreachable traversal objects must be swept from the registry"
        );
    }

    #[test]
    fn bulk_content_replacement_adjusts_live_iterators_and_ranges() {
        let mut runtime = JsRuntime::with_document(sample_document()).unwrap();
        let result = runtime
            .eval(
                r#"
                const host = document.getElementById('app');
                host.innerHTML = '<p>old</p><span>tail</span>';
                const oldText = host.lastChild.firstChild;
                const range = document.createRange();
                range.setStart(oldText, 1);
                range.setEnd(oldText, 2);
                const iterator = document.createNodeIterator(host);
                for (let i = 0; i < 5; i++) iterator.nextNode();
                host.textContent = 'new';
                const textResult = [range.startContainer === host, range.startOffset,
                    range.endContainer === host, range.endOffset,
                    iterator.referenceNode === host, iterator.nextNode().data].join(',');

                host.innerHTML = '<b>again</b><i>tail</i>';
                const innerText = host.lastChild.firstChild;
                range.setStart(innerText, 1);
                range.setEnd(innerText, 3);
                const iterator2 = document.createNodeIterator(host);
                for (let i = 0; i < 5; i++) iterator2.nextNode();
                host.innerHTML = '<u>replacement</u>';
                const htmlResult = [range.startContainer === host, range.startOffset,
                    range.endContainer === host, range.endOffset,
                    iterator2.referenceNode === host,
                    iterator2.nextNode() === host.firstChild].join(',');
                textResult + '|' + htmlResult;
                "#,
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();

        assert_eq!(result, "true,0,true,0,true,new|true,0,true,0,true,true");
    }

    #[test]
    fn range_set_start_and_end_reroot_across_documents() {
        let mut runtime = JsRuntime::with_document(sample_document()).unwrap();
        let result = runtime
            .eval(
                r#"
                const frame = document.createElement('iframe');
                document.body.appendChild(frame);
                const foreign = frame.contentDocument;
                const root = foreign.createElement('root');
                foreign.appendChild(root);
                const text = foreign.createTextNode('foreign');
                root.appendChild(text);
                const range = document.createRange();
                range.selectNodeContents(document.getElementById('app'));
                range.setStart(text, 2);
                const afterStart = [range.collapsed, range.startContainer === text,
                    range.endContainer === text, range.commonAncestorContainer === text,
                    range.toString()].join(',');

                const homeText = document.createTextNode('home');
                document.getElementById('app').appendChild(homeText);
                range.setEnd(homeText, 3);
                const afterEnd = [range.collapsed, range.startContainer === homeText,
                    range.endContainer === homeText, range.commonAncestorContainer === homeText,
                    range.toString()].join(',');
                afterStart + '|' + afterEnd;
                "#,
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();

        assert_eq!(result, "true,true,true,true,|true,true,true,true,");
    }

    #[test]
    fn supports_event_listeners_bubbling_and_capture() {
        let mut runtime = JsRuntime::with_document(sample_document()).unwrap();
        runtime
            .eval(
                "globalThis.events = []; \
                 const root = document.getElementById('app'); \
                 const child = document.createElement('button'); \
                 child.id = 'child'; \
                 root.appendChild(child); \
                 root.addEventListener('click', () => events.push('bubble')); \
                 root.addEventListener('click', () => events.push('capture'), true); \
                 child.addEventListener('click', () => events.push('target')); \
                 child.dispatchEvent(new Event('click', { bubbles: true }));",
            )
            .unwrap();

        let events = runtime
            .eval("events.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(events, "capture,target,bubble");
    }

    #[test]
    fn event_defaults_and_dispatch_internals_follow_dom_boundaries() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime
            .eval(
                r#"(() => {
                  const node = document.createElement("div");
                  const event = new Event("probe");
                  const nullInitDefaults = [
                    new Event("event", null).bubbles,
                    new UIEvent("ui", null).detail,
                    new CustomEvent("custom", null).detail,
                    new MessageEvent("message", null).data,
                    new MouseEvent("mouse", null).clientX,
                    new KeyboardEvent("key", null).key,
                    new FocusEvent("focus", null).relatedTarget,
                    new MediaQueryListEvent("change", null).matches,
                  ].join(",");
                  let pathLengthDuringDispatch = -1;
                  node.addEventListener("probe", current => {
                    pathLengthDuringDispatch = current.composedPath().length;
                  });
                  node.dispatchEvent(event);
                  return [
                    event.bubbles,
                    pathLengthDuringDispatch,
                    event.__path.length,
                    event.composedPath().length,
                    typeof globalThis.__omoikane_internal_assigned_slot,
                    nullInitDefaults,
                  ].join("|");
                })()"#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "false|1|0|0|undefined|false,0,,,0,,,false");
    }

    #[test]
    fn transition_event_exposes_transition_specific_fields() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"(() => {
                      const event = new TransitionEvent("transitionend", {
                        bubbles: true,
                        propertyName: "opacity",
                        elapsedTime: 1.25,
                        pseudoElement: "::before"
                      });
                      const defaults = new TransitionEvent("transitionrun", null);
                      return event instanceof Event && event.bubbles &&
                        event.propertyName === "opacity" && event.elapsedTime === 1.25 &&
                        event.pseudoElement === "::before" && defaults.propertyName === "" &&
                        defaults.elapsedTime === 0 && defaults.pseudoElement === "";
                    })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn exposes_window_console_and_navigator() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("console.log(window.location.href, navigator.userAgent);")
            .unwrap();

        let logs = runtime.console_logs();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("http://localhost/"));
        assert!(logs[0].contains(&default_user_agent()));
    }

    #[test]
    fn navigator_exposes_empty_plugin_and_mime_type_collections() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"navigator.plugins instanceof PluginArray &&
                       navigator.mimeTypes instanceof MimeTypeArray &&
                       navigator.plugins.length === 0 && navigator.mimeTypes.length === 0 &&
                       navigator.plugins === navigator.plugins &&
                       navigator.mimeTypes === navigator.mimeTypes &&
                       Object.prototype.toString.call(navigator.plugins) === "[object PluginArray]" &&
                       Object.prototype.toString.call(navigator.mimeTypes) === "[object MimeTypeArray]" &&
                       navigator.plugins.item.length === 1 &&
                       navigator.plugins.namedItem.length === 1 &&
                       navigator.mimeTypes.item.length === 1 &&
                       navigator.mimeTypes.namedItem.length === 1 &&
                       navigator.plugins.item(0) === null &&
                       navigator.plugins.namedItem("missing") === null &&
                       navigator.mimeTypes.item(0) === null &&
                       navigator.mimeTypes.namedItem("missing") === null &&
                       Array.from(navigator.plugins).length === 0 &&
                       Array.from(navigator.mimeTypes).length === 0"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn intl_formatters_expose_common_bootstrap_surface() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"Intl.NumberFormat("en").format(12) === "12" &&
                   Intl.PluralRules("en").select(1) === "one" &&
                   Intl.ListFormat("en").format(["a", "b"]) === "a, b" &&
                   Intl.getCanonicalLocales("en-US")[0] === "en-US""#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn common_html_elements_use_specific_interfaces() {
        let mut runtime = runtime_from_html("<html><head></head><body></body></html>");
        assert!(
            runtime
                .eval(
                    r#"document.body instanceof HTMLBodyElement &&
                   document.head instanceof HTMLHeadElement &&
                   document.documentElement instanceof HTMLHtmlElement &&
                   document.createElement("a") instanceof HTMLAnchorElement"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn queue_microtask_shares_the_microtask_queue_with_promise_reactions() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.microtaskResult = { order: [] };
                microtaskResult.shape = [
                  typeof queueMicrotask,
                  queueMicrotask.length,
                  queueMicrotask.name,
                ];
                microtaskResult.rejects = [42, null, undefined, "fn"].map(value => {
                  try { queueMicrotask(value); return "accepted"; }
                  catch (error) { return error instanceof TypeError; }
                });
                queueMicrotask(function () {
                  // The callback takes no arguments, and is called as a plain
                  // function rather than as a method.
                  microtaskResult.argumentCount = arguments.length;
                  microtaskResult.thisIsGlobal = this === globalThis;
                });
                // Interleaved registration must come out in registration order.
                queueMicrotask(() => microtaskResult.order.push("qm1"));
                Promise.resolve().then(() => microtaskResult.order.push("p1"));
                queueMicrotask(() => microtaskResult.order.push("qm2"));
                Promise.resolve().then(() => microtaskResult.order.push("p2"));
                queueMicrotask(() => {
                  microtaskResult.order.push("outer");
                  queueMicrotask(() => microtaskResult.order.push("nested"));
                });
                Promise.resolve().then(() => microtaskResult.order.push("p3"));
                // Nothing may run before the checkpoint.
                microtaskResult.syncCount = microtaskResult.order.length;"#,
            )
            .unwrap();

        assert_eq!(
            runtime
                .eval("microtaskResult.syncCount")
                .unwrap()
                .as_number(),
            Some(0.0),
            "queueMicrotask must not run its callback synchronously"
        );

        runtime.run_jobs().unwrap();

        assert_eq!(
            runtime
                .eval("microtaskResult.order.join(\",\")")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "qm1,p1,qm2,p2,outer,p3,nested",
            "queueMicrotask and promise reactions share one FIFO queue"
        );
        assert!(
            runtime
                .eval(
                    r#"JSON.stringify(microtaskResult.shape) === '["function",1,"queueMicrotask"]' &&
                    JSON.stringify(microtaskResult.rejects) === "[true,true,true,true]" &&
                    microtaskResult.argumentCount === 0 &&
                    microtaskResult.thisIsGlobal === true"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn queue_microtask_runs_before_the_next_task() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.taskOrder = [];
                setTimeout(() => taskOrder.push("task"), 0);
                queueMicrotask(() => taskOrder.push("microtask"));"#,
            )
            .unwrap();

        runtime.tick(1).unwrap();

        assert_eq!(
            runtime
                .eval("taskOrder.join(\",\")")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "microtask,task",
            "the microtask checkpoint precedes the next task"
        );
    }

    #[test]
    fn text_encoder_and_decoder_round_trip_utf8() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const bytes = new TextEncoder().encode("A日本");
                    return bytes.length === 7 && bytes[0] === 65 &&
                      new TextDecoder().decode(bytes) === "A日本" &&
                      btoa("hello") === "aGVsbG8=" && atob("aGVsbG8=") === "hello";
                })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn web_crypto_random_values_uuid_and_digest_core() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.cryptoResult = { digests: {} };
                const random = new Uint8Array(32);
                cryptoResult.sameView = crypto.getRandomValues(random) === random;
                cryptoResult.length = random.byteLength;
                cryptoResult.changed = random.some(value => value !== 0);
                const backing = new Uint8Array(12);
                const offsetView = new Uint16Array(backing.buffer, 4, 2);
                crypto.getRandomValues(offsetView);
                cryptoResult.offsetPreserved = backing.slice(0, 4).every(value => value === 0) &&
                  backing.slice(8).every(value => value === 0) &&
                  backing.slice(4, 8).some(value => value !== 0);
                cryptoResult.uuid = crypto.randomUUID();
                try { new Crypto(); }
                catch (error) { cryptoResult.cryptoConstructorError = error instanceof TypeError; }
                try { new SubtleCrypto(); }
                catch (error) { cryptoResult.subtleConstructorError = error instanceof TypeError; }
                try { crypto.getRandomValues(new Float32Array(1)); }
                catch (error) { cryptoResult.typeError = error instanceof TypeError; }
                try { crypto.getRandomValues(new Uint8Array(65537)); }
                catch (error) {
                  cryptoResult.quotaError = error instanceof DOMException &&
                    error.name === "QuotaExceededError";
                }
                const algorithms = ["SHA-1", "SHA-256", "SHA-384", "SHA-512"];
                const snapshotInput = new Uint8Array([97, 98, 99]);
                crypto.subtle.digest("SHA-256", snapshotInput).then(digest => {
                  cryptoResult.snapshotDigest = Array.from(new Uint8Array(digest))
                    .map(value => value.toString(16).padStart(2, "0")).join("");
                });
                snapshotInput.fill(0);
                Promise.all(algorithms.map(async algorithm => {
                  const identifier = algorithm === "SHA-256" ? { name: "sha-256" } : algorithm;
                  const digest = await crypto.subtle.digest(identifier, new Uint8Array());
                  cryptoResult.digests[algorithm] = Array.from(new Uint8Array(digest))
                    .map(value => value.toString(16).padStart(2, "0")).join("");
                })).then(() => { cryptoResult.done = true; });"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        assert!(
            runtime
                .eval(
                r#"cryptoResult.sameView && cryptoResult.length === 32 && cryptoResult.changed && cryptoResult.offsetPreserved &&
                /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(cryptoResult.uuid) &&
                cryptoResult.typeError && cryptoResult.quotaError && cryptoResult.cryptoConstructorError &&
                cryptoResult.subtleConstructorError && cryptoResult.done &&
                cryptoResult.digests["SHA-1"] === "da39a3ee5e6b4b0d3255bfef95601890afd80709" &&
                cryptoResult.digests["SHA-256"] === "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" &&
                cryptoResult.snapshotDigest === "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" &&
                cryptoResult.digests["SHA-384"] === "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b" &&
                cryptoResult.digests["SHA-512"] === "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e""#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn blob_constructor_normalizes_type_and_concatenates_parts() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.blobResult = {};
                const backing = new Uint8Array([1, 2, 3, 4, 5, 6]);
                const mixed = new Blob([
                  "ab",
                  new Uint8Array([99, 100]).buffer,
                  new Uint8Array([101]),
                  new Blob(["f"], { type: "text/plain" }),
                  42,
                ]);
                blobResult.mixedSize = mixed.size;
                // A nested blob contributes its bytes but never its type.
                blobResult.mixedType = mixed.type;
                // Buffer views contribute only the bytes they view.
                blobResult.viewSizes = [
                  new Blob([new Uint8Array(backing.buffer, 2, 2)]).size,
                  new Blob([new DataView(backing.buffer, 4)]).size,
                  new Blob([backing.buffer]).size,
                ];
                // A copy is taken, so later writes cannot change the blob.
                blobResult.snapshot = (() => {
                  const source = new Uint8Array([7, 8]);
                  const blob = new Blob([source]);
                  source[0] = 99;
                  return blob.size;
                })();
                blobResult.empty = [new Blob().size, new Blob().type, new Blob([]).size];
                blobResult.types = [
                  new Blob([], { type: "TEXT/Plain" }).type,
                  new Blob([], { type: "foo bar" }).type,
                  new Blob([], { type: "text/pléin" }).type,
                  new Blob([], { type: "text/plain\tx" }).type,
                ];
                blobResult.utf8Size = new Blob(["日本"]).size;
                blobResult.toStringTags = [
                  Object.prototype.toString.call(new Blob([])),
                  Object.prototype.toString.call(new File([], "n")),
                ];
                blobResult.readOnly = (() => {
                  const blob = new Blob(["a"], { type: "text/plain" });
                  blob.size = 99;
                  blob.type = "x/y";
                  return [blob.size, blob.type];
                })();
                // `sequence<BlobPart>` needs an iterable object: a bare string, a
                // primitive and a plain array-like are all rejected.
                blobResult.rejected = ["abc", null, 42, { length: 1, 0: "a" }].map(parts => {
                  try { new Blob(parts); return "accepted"; }
                  catch (error) { return error instanceof TypeError; }
                });
                blobResult.iterable = new Blob(new Set(["a", "bc"])).size;
                Promise.all([
                  mixed.text(),
                  new Blob(["\ud800"]).arrayBuffer(),
                  new Blob(["hi"]).bytes(),
                ]).then(([text, lone, bytes]) => {
                  blobResult.mixedText = text;
                  // An unpaired surrogate has no UTF-8 encoding and becomes U+FFFD.
                  blobResult.loneSurrogate = Array.from(new Uint8Array(lone));
                  blobResult.bytes = Array.from(bytes);
                  blobResult.done = true;
                });"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        assert!(
            runtime
                .eval(
                    r#"blobResult.done === true &&
                    blobResult.mixedSize === 8 &&
                    blobResult.mixedType === "" &&
                    blobResult.mixedText === "abcdef42" &&
                    JSON.stringify(blobResult.viewSizes) === "[2,2,6]" &&
                    blobResult.snapshot === 2 &&
                    JSON.stringify(blobResult.empty) === '[0,"",0]' &&
                    JSON.stringify(blobResult.types) === '["text/plain","foo bar","",""]' &&
                    blobResult.utf8Size === 6 &&
                    JSON.stringify(blobResult.toStringTags) === '["[object Blob]","[object File]"]' &&
                    JSON.stringify(blobResult.readOnly) === '[1,"text/plain"]' &&
                    JSON.stringify(blobResult.rejected) === "[true,true,true,true]" &&
                    blobResult.iterable === 3 &&
                    JSON.stringify(blobResult.loneSurrogate) === "[239,191,189]" &&
                    JSON.stringify(blobResult.bytes) === "[104,105]""#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn blob_slice_clamps_and_rounds_offsets() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.sliceResult = {};
                const blob = new Blob(["abcde"], { type: "text/plain" });
                sliceResult.types = [
                  blob.slice().type,
                  blob.slice(1, 2, "TEXT/Html").type,
                  blob.slice(1, 2, "bad type").type,
                ];
                sliceResult.sizes = [
                  blob.slice(3, 1).size,
                  blob.slice(1e21).size,
                  blob.slice(-1e21).size,
                ];
                Promise.all([
                  blob.slice().text(),
                  blob.slice(1).text(),
                  blob.slice(-2).text(),
                  blob.slice(1, -1).text(),
                  blob.slice(0, 100).text(),
                  blob.slice(-100, 2).text(),
                  blob.slice(undefined, undefined).text(),
                  blob.slice(NaN).text(),
                  // `[Clamp]` rounds to the nearest integer, ties to even.
                  blob.slice(1.5).text(),
                  blob.slice(2.5).text(),
                  blob.slice(3.5).text(),
                  blob.slice(1.7, 3.9).text(),
                ]).then(texts => { sliceResult.texts = texts; sliceResult.done = true; });"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        assert!(
            runtime
                .eval(
                    r#"sliceResult.done === true &&
                    JSON.stringify(sliceResult.types) === '["","text/html","bad type"]' &&
                    JSON.stringify(sliceResult.sizes) === "[0,0,5]" &&
                    JSON.stringify(sliceResult.texts) === JSON.stringify([
                      "abcde", "bcde", "de", "bcd", "abcde", "ab", "abcde", "abcde",
                      "cde", "cde", "e", "cd",
                    ])"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn file_exposes_name_type_and_last_modified() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const before = Date.now();
                    const file = new File(["hello"], "n.txt", { type: "Text/Plain", lastModified: 1234 });
                    const defaulted = new File([], "a");
                    let missingName = false;
                    try { new File([]); } catch (error) { missingName = error instanceof TypeError; }
                    return file.name === "n.txt" && file.type === "text/plain" && file.size === 5 &&
                      file.lastModified === 1234 && file.webkitRelativePath === "" &&
                      file instanceof Blob && file instanceof File &&
                      // The name is not sanitized.
                      new File([], "a/b\\c").name === "a/b\\c" &&
                      new File([], 5).name === "5" && new File([], null).name === "null" &&
                      defaulted.type === "" && defaulted.lastModified >= before &&
                      // Slicing a File yields a plain Blob.
                      !(file.slice(0, 2) instanceof File) && file.slice(0, 2) instanceof Blob &&
                      file.slice(0, 2).size === 2 && missingName;
                  })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn file_reader_fires_progress_events_from_a_task() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.readerLog = [];
                globalThis.reader = new FileReader();
                for (const type of ["loadstart", "progress", "load", "loadend", "error", "abort"]) {
                  reader.addEventListener(type, event => {
                    readerLog.push([
                      type,
                      reader.readyState,
                      reader.result === null ? "null" : typeof reader.result,
                      event.lengthComputable,
                      event.loaded,
                      event.total,
                      Object.prototype.toString.call(event),
                    ].join(","));
                  });
                }
                readerLog.push("before," + reader.readyState);
                reader.readAsText(new Blob(["hello world"]));
                // The read is queued on the file reading task source, so nothing has
                // been delivered yet even though the bytes are already in memory.
                readerLog.push("after," + reader.readyState + "," + (reader.result === null ? "null" : "set"));"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("readerLog.join(\" | \")")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "before,0 | after,1,null",
            "microtask checkpoints alone must not deliver a FileReader result"
        );

        runtime.run_until_idle().unwrap();

        assert_eq!(
            runtime
                .eval("readerLog.join(\" | \")")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "before,0 | after,1,null | \
             loadstart,1,null,true,0,11,[object ProgressEvent] | \
             progress,1,null,true,11,11,[object ProgressEvent] | \
             load,2,string,true,11,11,[object ProgressEvent] | \
             loadend,2,string,true,11,11,[object ProgressEvent]"
        );
        assert!(
            runtime
                .eval(
                    r#"reader.result === "hello world" && reader.error === null &&
                    FileReader.EMPTY === 0 && FileReader.LOADING === 1 && FileReader.DONE === 2 &&
                    reader.DONE === 2"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn file_reader_reads_text_array_buffer_and_data_url() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.readResult = {};
                const read = (method, blob, ...rest) => new Promise(resolve => {
                  const reader = new FileReader();
                  reader.onloadend = () => resolve(reader.result);
                  reader[method](blob, ...rest);
                });
                Promise.all([
                  read("readAsText", new Blob(["日本"])),
                  read("readAsArrayBuffer", new Blob([new Uint8Array([1, 2, 3])])),
                  // With no type the data URL falls back to application/octet-stream.
                  read("readAsDataURL", new Blob(["a"])),
                  read("readAsDataURL", new Blob(["a"], { type: "text/plain" })),
                  read("readAsDataURL", new Blob([])),
                  read("readAsDataURL", new File(["ab"], "x.bin")),
                  read("readAsBinaryString", new Blob([new Uint8Array([200, 10])])),
                  read("readAsText", new File(["file body"], "n.txt")),
                ]).then(([text, buffer, untyped, typed, empty, file, binary, fileText]) => {
                  readResult.text = text;
                  readResult.bufferTag = Object.prototype.toString.call(buffer);
                  readResult.bufferBytes = Array.from(new Uint8Array(buffer));
                  readResult.urls = [untyped, typed, empty, file];
                  readResult.binary = binary;
                  readResult.fileText = fileText;
                  readResult.done = true;
                });"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert!(
            runtime
                .eval(
                    r#"readResult.done === true &&
                    readResult.text === "日本" &&
                    readResult.bufferTag === "[object ArrayBuffer]" &&
                    JSON.stringify(readResult.bufferBytes) === "[1,2,3]" &&
                    JSON.stringify(readResult.urls) === JSON.stringify([
                      "data:application/octet-stream;base64,YQ==",
                      "data:text/plain;base64,YQ==",
                      "data:application/octet-stream;base64,",
                      "data:application/octet-stream;base64,YWI=",
                    ]) &&
                    readResult.binary === "È\n" &&
                    readResult.fileText === "file body""#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn file_reader_rejects_reentrant_reads_and_aborts() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.abortResult = { log: [] };
                const reentrant = new FileReader();
                reentrant.readAsText(new Blob(["a"]));
                try { reentrant.readAsText(new Blob(["b"])); abortResult.reentrant = "accepted"; }
                catch (error) {
                  abortResult.reentrant = error instanceof DOMException && error.name === "InvalidStateError";
                }
                let nonBlob = false;
                try { new FileReader().readAsText("not a blob"); }
                catch (error) { nonBlob = error instanceof TypeError; }
                abortResult.nonBlob = nonBlob;

                const aborted = new FileReader();
                for (const type of ["loadstart", "progress", "load", "loadend", "abort", "error"]) {
                  aborted.addEventListener(type, () => abortResult.log.push([
                    type,
                    aborted.readyState,
                    aborted.result === null ? "null" : "set",
                    aborted.error === null ? "null" : aborted.error.name,
                  ].join(",")));
                }
                aborted.readAsText(new Blob(["abc"]));
                aborted.abort();

                const idle = new FileReader();
                idle.abort();
                abortResult.idle = [idle.readyState, idle.result];

                // A reader can be reused after an abort.
                globalThis.reused = new FileReader();
                reused.readAsText(new Blob(["x"]));
                reused.abort();
                reused.readAsText(new Blob(["y"]));"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert_eq!(
            runtime
                .eval("abortResult.log.join(\" | \")")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "abort,2,null,AbortError | loadend,2,null,AbortError",
            "aborting a read must dispatch only abort and loadend"
        );
        assert!(
            runtime
                .eval(
                    r#"abortResult.reentrant === true && abortResult.nonBlob === true &&
                    JSON.stringify(abortResult.idle) === "[0,null]" &&
                    reused.result === "y" && reused.error === null && reused.readyState === 2"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn file_reader_chained_from_its_load_handler_still_reports_loadend() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.chainLog = [];
                const reader = new FileReader();
                let chained = false;
                for (const type of ["loadstart", "progress", "load", "loadend"]) {
                  reader.addEventListener(type, () => chainLog.push(type + ":" + reader.result));
                }
                reader.addEventListener("load", () => {
                  // Starting the next read from `load` must not cancel the
                  // `loadend` that belongs to the read that just finished.
                  if (chained) return;
                  chained = true;
                  reader.readAsText(new Blob(["second"]));
                });
                reader.readAsText(new Blob(["first"]));"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert_eq!(
            runtime
                .eval("chainLog.join(\" | \")")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "loadstart:null | progress:null | load:first | loadend:null | \
             loadstart:null | progress:null | load:second | loadend:second"
        );
    }

    #[test]
    fn object_url_resolves_through_fetch_and_fails_after_revoke() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.urlResult = {};
                const blob = new Blob(["hello"], { type: "text/plain" });
                globalThis.objectUrl = URL.createObjectURL(blob);
                urlResult.shape = objectUrl.replace(
                  /[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
                  "<uuid>",
                );
                urlResult.unique = URL.createObjectURL(blob) !== objectUrl;
                urlResult.rejects = ["x", null, undefined].map(value => {
                  try { URL.createObjectURL(value); return "accepted"; }
                  catch (error) { return error instanceof TypeError; }
                });
                // Revoking an unknown URL is defined to do nothing.
                urlResult.revokeUnknown = URL.revokeObjectURL("blob:null/missing") === undefined &&
                  URL.revokeObjectURL("garbage") === undefined;

                fetch(objectUrl).then(async response => {
                  urlResult.status = [response.status, response.statusText, response.type, response.ok];
                  urlResult.url = response.url === objectUrl;
                  urlResult.contentType = response.headers.get("content-type");
                  urlResult.contentLength = response.headers.get("content-length");
                  urlResult.body = await response.text();
                  const roundTrip = await (await fetch(objectUrl)).blob();
                  urlResult.roundTrip = [roundTrip.size, roundTrip.type, await roundTrip.text()];
                  const untyped = await fetch(URL.createObjectURL(new Blob(["z"])));
                  urlResult.untypedHeaders = [
                    untyped.headers.get("content-type"),
                    untyped.headers.get("content-length"),
                  ];
                  // Only GET is defined for a blob URL.
                  try { await fetch(objectUrl, { method: "POST" }); urlResult.post = "resolved"; }
                  catch (error) { urlResult.post = error instanceof TypeError; }

                  URL.revokeObjectURL(objectUrl);
                  try { await fetch(objectUrl); urlResult.revoked = "resolved"; }
                  catch (error) { urlResult.revoked = error instanceof TypeError; }
                  urlResult.done = true;
                });"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert!(
            runtime
                .eval(
                    r#"urlResult.done === true &&
                    urlResult.shape === "blob:http://localhost/<uuid>" &&
                    urlResult.unique === true &&
                    JSON.stringify(urlResult.rejects) === "[true,true,true]" &&
                    urlResult.revokeUnknown === true &&
                    JSON.stringify(urlResult.status) === '[200,"OK","basic",true]' &&
                    urlResult.url === true &&
                    urlResult.contentType === "text/plain" &&
                    urlResult.contentLength === "5" &&
                    urlResult.body === "hello" &&
                    JSON.stringify(urlResult.roundTrip) === '[5,"text/plain","hello"]' &&
                    JSON.stringify(urlResult.untypedHeaders) === '["","1"]' &&
                    urlResult.post === true &&
                    urlResult.revoked === true"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );

        // The host store mirrors the live URLs: three were created, one revoked.
        assert_eq!(crate::data::blob_url_count(), 2);
    }

    #[test]
    fn object_url_serves_xml_http_request_until_revoked() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.xhrBlobResult = {};
                globalThis.blobUrl = URL.createObjectURL(new Blob(["xhrbody"], { type: "text/plain" }));
                const live = new XMLHttpRequest();
                live.open("GET", blobUrl);
                live.onloadend = () => {
                  xhrBlobResult.live = [
                    live.status,
                    live.statusText,
                    live.responseText,
                    live.getResponseHeader("content-type"),
                    live.getResponseHeader("content-length"),
                  ];
                  URL.revokeObjectURL(blobUrl);
                  const dead = new XMLHttpRequest();
                  dead.open("GET", blobUrl);
                  dead.onloadend = () => {
                    xhrBlobResult.revoked = [dead.status, dead.responseText];
                    xhrBlobResult.done = true;
                  };
                  dead.send();
                };
                live.send();"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert!(
            runtime
                .eval(
                    r#"xhrBlobResult.done === true &&
                    JSON.stringify(xhrBlobResult.live) ===
                      '[200,"OK","xhrbody","text/plain","7"]' &&
                    JSON.stringify(xhrBlobResult.revoked) === '[0,""]'"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert_eq!(crate::data::blob_url_count(), 0);
    }

    #[test]
    fn form_data_accepts_blob_and_file_entries() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const data = new FormData();
                    // A bare Blob becomes a File named "blob".
                    data.append("bare", new Blob(["A"], { type: "text/plain" }));
                    data.append("named", new Blob(["B"]), "given.txt");
                    data.append("file", new File(["C"], "orig.txt", { type: "text/csv" }));
                    data.append("override", new File(["D"], "orig2.txt"), "override.txt");
                    data.set("replaced", new Blob(["E"]));
                    const bare = data.get("bare");
                    let filenameOnString = false;
                    try { data.append("text", "value", "nope.txt"); }
                    catch (error) { filenameOnString = error instanceof TypeError; }
                    return bare instanceof File && bare.name === "blob" &&
                      bare.type === "text/plain" && bare.size === 1 && bare.lastModified > 0 &&
                      data.get("named").name === "given.txt" && data.get("named").type === "" &&
                      data.get("file").name === "orig.txt" && data.get("file").type === "text/csv" &&
                      data.get("override").name === "override.txt" &&
                      data.get("replaced").name === "blob" &&
                      // A non-Blob value is still stringified.
                      (data.append("plain", 42), data.get("plain") === "42") &&
                      filenameOnString && data.has("text") === false;
                  })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn multipart_form_data_encodes_file_parts_and_escapes_header_values() {
        let mut runtime = JsRuntime::new().unwrap();
        let encoded = runtime
            .eval(
                r#"(() => {
                const data = new FormData();
                data.append("text", "val");
                data.append("nl", "line1\nline2\r\nline3\rline4");
                data.append("f", new File(["FILEDATA"], "we\"ird\r\nna\\me.txt", { type: "text/csv" }));
                data.append("noType", new Blob(["X"]), "nt.bin");
                data.append("uni", new File(["U"], "日本語.txt"));
                data.append("q\"uote\r\nname", "v2");
                const multipart = data.__multipart("BND");
                // A file entry makes the payload binary.
                globalThis.multipartIsBytes = multipart.body instanceof Uint8Array;
                globalThis.multipartContentType = multipart.contentType;
                return new TextDecoder().decode(multipart.body);
              })()"#,
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();

        assert_eq!(
            encoded,
            concat!(
                "--BND\r\nContent-Disposition: form-data; name=\"text\"\r\n\r\nval\r\n",
                "--BND\r\nContent-Disposition: form-data; name=\"nl\"\r\n\r\n",
                "line1\r\nline2\r\nline3\r\nline4\r\n",
                "--BND\r\nContent-Disposition: form-data; name=\"f\"; ",
                "filename=\"we%22ird%0D%0Ana\\me.txt\"\r\nContent-Type: text/csv\r\n\r\nFILEDATA\r\n",
                "--BND\r\nContent-Disposition: form-data; name=\"noType\"; filename=\"nt.bin\"\r\n",
                "Content-Type: application/octet-stream\r\n\r\nX\r\n",
                "--BND\r\nContent-Disposition: form-data; name=\"uni\"; filename=\"日本語.txt\"\r\n",
                "Content-Type: application/octet-stream\r\n\r\nU\r\n",
                "--BND\r\nContent-Disposition: form-data; name=\"q%22uote%0D%0Aname\"\r\n\r\nv2\r\n",
                "--BND--\r\n",
            )
        );
        assert!(
            runtime
                .eval(
                    r#"multipartIsBytes === true &&
                    multipartContentType === "multipart/form-data; boundary=BND""#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn multipart_file_part_bytes_survive_binary_data() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const data = new FormData();
                    // 0xFF/0xFE are not valid UTF-8 and must reach the payload intact.
                    data.append("bin", new File([new Uint8Array([0, 255, 254, 10])], "b.bin"));
                    const body = data.__multipart("B").body;
                    const marker = [0, 255, 254, 10];
                    for (let index = 0; index + marker.length <= body.length; index++) {
                      if (marker.every((byte, offset) => body[index + offset] === byte)) return true;
                    }
                    return false;
                  })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn file_input_contributes_an_empty_file_entry() {
        let mut runtime = runtime_from_html(
            "<html><body><form id='target' action='/submit'>\
             <input type='file' name='up'><input name='t' value='v'>\
             </form></body></html>",
        );
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const form = document.getElementById("target");
                    const input = form.querySelector("input[type=file]");
                    const data = new FormData(form);
                    const entry = data.get("up");
                    return JSON.stringify([...data.keys()]) === '["up","t"]' &&
                      entry instanceof File && entry.name === "" &&
                      entry.type === "application/octet-stream" && entry.size === 0 &&
                      // A file control exposes a stable, empty FileList.
                      Object.prototype.toString.call(input.files) === "[object FileList]" &&
                      input.files.length === 0 && input.files === input.files &&
                      input.files.item(0) === null &&
                      document.querySelector("input[name=t]").files === null;
                  })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );

        // multipart keeps the empty filename and the octet-stream part type.
        assert_eq!(
            runtime
                .eval(
                    r#"new TextDecoder().decode(
                    new FormData(document.getElementById("target")).__multipart("B").body
                  )"#,
                )
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            concat!(
                "--B\r\nContent-Disposition: form-data; name=\"up\"; filename=\"\"\r\n",
                "Content-Type: application/octet-stream\r\n\r\n\r\n",
                "--B\r\nContent-Disposition: form-data; name=\"t\"\r\n\r\nv\r\n",
                "--B--\r\n",
            )
        );

        // urlencoded and text/plain submit a file entry as its filename.
        runtime
            .eval("document.getElementById(\"target\").submit()")
            .unwrap();
        runtime.run_until_idle().unwrap();
        let requests = runtime.take_navigation_requests();
        assert_eq!(
            requests,
            vec![NavigationRequest::FormSubmit {
                url: "http://localhost/submit?up=&t=v".to_string(),
                method: "GET".to_string(),
                body: None,
                content_type: None,
            }]
        );
    }

    #[test]
    fn response_and_request_round_trip_blob_bodies() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.bodyResult = {};
                const blob = new Blob(["body"], { type: "text/csv" });
                bodyResult.contentTypes = [
                  new Response(blob).headers.get("content-type"),
                  new Response(new Blob(["b"])).headers.get("content-type"),
                  new Response("txt").headers.get("content-type"),
                  new Request("https://x.test/", { method: "POST", body: blob })
                    .headers.get("content-type"),
                ];
                let getWithBody = false;
                try { new Request("https://x.test/", { body: blob }); }
                catch (error) { getWithBody = error instanceof TypeError; }
                bodyResult.getWithBody = getWithBody;

                (async () => {
                  const fromBlob = await new Response(blob).blob();
                  bodyResult.fromBlob = [fromBlob.type, fromBlob.size, await fromBlob.text()];
                  // A blob's type comes from the Content-Type header, so a text body
                  // reports the charset the header carries.
                  const fromText = await new Response("txt").blob();
                  bodyResult.fromText = [fromText.type, fromText.size];
                  const fromNull = await new Response(null).blob();
                  bodyResult.fromNull = [fromNull.type, fromNull.size];
                  const fromBuffer = await new Response(new Uint8Array([1, 2])).blob();
                  bodyResult.fromBuffer = [fromBuffer.type, fromBuffer.size];
                  bodyResult.arrayBuffer = Array.from(new Uint8Array(
                    await new Response(new Blob([new Uint8Array([1, 2, 3])])).arrayBuffer(),
                  ));
                  const request = new Request("https://x.test/", { method: "POST", body: blob });
                  const requestBlob = await request.blob();
                  bodyResult.request = [requestBlob.type, await requestBlob.text(), await request.text()];
                  const clone = new Response(blob, { headers: { "content-type": "text/html" } }).clone();
                  bodyResult.clone = [(await clone.blob()).type, await clone.text()];
                  bodyResult.done = true;
                })();"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        assert!(
            runtime
                .eval(
                    r#"bodyResult.done === true &&
                    JSON.stringify(bodyResult.contentTypes) ===
                      '["text/csv",null,"text/plain;charset=UTF-8","text/csv"]' &&
                    bodyResult.getWithBody === true &&
                    JSON.stringify(bodyResult.fromBlob) === '["text/csv",4,"body"]' &&
                    JSON.stringify(bodyResult.fromText) === '["text/plain;charset=utf-8",3]' &&
                    JSON.stringify(bodyResult.fromNull) === '["",0]' &&
                    JSON.stringify(bodyResult.fromBuffer) === '["",2]' &&
                    JSON.stringify(bodyResult.arrayBuffer) === "[1,2,3]" &&
                    JSON.stringify(bodyResult.request) === '["text/csv","body","body"]' &&
                    JSON.stringify(bodyResult.clone) === '["text/html","body"]'"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn fetch_preserves_binary_response_bytes() {
        let payload: Vec<u8> = vec![0x89, 0x50, 0x4e, 0x47, 0x00, 0xff, 0xfe, 0x0a];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let served = payload.clone();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_http_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                served.len()
            )
            .unwrap();
            stream.write_all(&served).unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(
                r#"globalThis.binaryResult = {};
                fetch("/image.png").then(async response => {
                  const copy = response.clone();
                  const blob = await response.blob();
                  binaryResult.blob = [blob.type, blob.size];
                  binaryResult.bytes = Array.from(new Uint8Array(await blob.arrayBuffer()));
                  binaryResult.buffer = Array.from(new Uint8Array(await copy.arrayBuffer()));
                  // The text path still sees the lossy UTF-8 decoding it is defined
                  // to return, with one replacement character per invalid byte.
                  binaryResult.text = Array.from(await copy.text(), c => c.codePointAt(0));
                  binaryResult.done = true;
                });"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert_eq!(
            runtime
                .eval("JSON.stringify(binaryResult.bytes)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            serde_json::to_string(&payload).unwrap(),
            "a non-UTF-8 response body must reach Response.blob() unchanged"
        );
        assert_eq!(
            runtime
                .eval("JSON.stringify(binaryResult.buffer)")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            serde_json::to_string(&payload).unwrap(),
            "Response.arrayBuffer() must expose the same bytes"
        );
        assert!(
            runtime
                .eval(
                    r#"binaryResult.done === true &&
                    JSON.stringify(binaryResult.blob) === '["image/png",8]' &&
                    JSON.stringify(binaryResult.text) ===
                      JSON.stringify([65533, 80, 78, 71, 0, 65533, 65533, 10])"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn url_and_search_params_parse_common_web_urls() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r##"(() => {
                    const url = new URL("../asset.js?q=hello+world", "https://example.com/app/page.js");
                    return url.origin === "https://example.com" &&
                      url.pathname === "/app/../asset.js" &&
                      url.searchParams.get("q") === "hello world";
                })()"##,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn fetch_standard_objects_expose_headers_and_body_helpers() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const headers = new Headers({ "X-Test": "one" });
                    headers.append("x-test", "two");
                    const request = new Request("https://example.com", { headers });
                    const response = Response.json({ ok: true });
                    return request.headers.get("X-Test") === "one, two" &&
                      response.ok && response.headers.get("content-type") === "application/json";
                })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn abort_controller_exposes_signal_state_and_events() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const controller = new AbortController();
                    let events = 0;
                    controller.signal.addEventListener("abort", () => events++);
                    controller.abort("stopped");
                    controller.abort("ignored");
                    return controller.signal instanceof AbortSignal &&
                      controller.signal.aborted && controller.signal.reason === "stopped" &&
                      events === 1 && AbortSignal.abort().reason.name === "AbortError";
                })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn document_get_elements_by_name_filters_exact_attribute_values() {
        let mut runtime = runtime_from_html(
            r#"<html><body><input name="failedScript"><div name="other"></div><span name="failedScript"></span></body></html>"#,
        );
        assert!(runtime
            .eval(r#"document.getElementsByName("failedScript").length === 2 && document.getElementsByName("missing").length === 0"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn document_cookie_round_trips_name_value_pairs() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r#"document.cookie = "theme=dark; Path=/"; document.cookie = "token=abc"; document.cookie === "theme=dark; token=abc" && navigator.cookieEnabled"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn history_api_tracks_state_and_same_origin_urls() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r##"(() => { history.pushState({page: 1}, "", "/one?q=1"); history.replaceState({page: 2}, "", "/two#hash"); return history.length === 2 && history.state.page === 2 && location.pathname === "/two" && location.hash === "#hash"; })()"##)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn location_exposes_navigation_methods() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r##"(() => { const result = location.reload(); location.assign("/assigned?q=1"); location.replace("/replaced#ok"); return result === undefined && location.pathname === "/replaced" && location.hash === "#ok"; })()"##)
            .unwrap()
            .as_boolean()
            .unwrap());
        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.take_navigation_requests(),
            vec![
                NavigationRequest::Reload,
                NavigationRequest::Navigate {
                    url: "http://localhost/assigned?q=1".to_string(),
                    replace: false,
                },
                NavigationRequest::Navigate {
                    url: "http://localhost/replaced#ok".to_string(),
                    replace: true,
                },
            ]
        );
    }

    #[test]
    fn form_data_preserves_order_duplicates_and_mutation_semantics() {
        let mut runtime = runtime_from_html(r#"<form id="f"><input name="a" value="one"><input name="a" value="two"><input name="off" value="x" disabled><input type="checkbox" name="unchecked"><input type="checkbox" name="checked" value="yes" checked><textarea name="note">line1
line2</textarea><select name="choice"><option value="x">X</option><option value="y" selected>Y</option></select><select name="fallback"><option value="disabled" disabled>Disabled</option><option value="usable">Usable</option></select></form>"#);
        assert!(runtime.eval(r#"(() => { const data = new FormData(document.getElementById("f")); const initial = JSON.stringify([...data]); data.set("a", "changed"); data.append("a", "last"); data.delete("checked"); return initial === JSON.stringify([["a","one"],["a","two"],["checked","yes"],["note","line1\nline2"],["choice","y"],["fallback","usable"]]) && data.get("a") === "changed" && data.getAll("a").join(",") === "changed,last" && !data.has("checked") && data.get("missing") === null; })()"#).unwrap().as_boolean().unwrap());
    }

    #[test]
    fn form_submission_encodes_get_post_and_submitter() {
        let mut runtime = runtime_from_html(r#"<form id="getForm" action="/search?old=1#result"><input name="q" value="hello world"><input name="symbol" value="a*b"><button id="go" name="source" value="button">Go</button></form><form id="postForm" action="/save" method="post" enctype="text/plain"><textarea name="note">a
b</textarea></form>"#);
        runtime.eval(r#"document.getElementById("go").click(); document.getElementById("postForm").submit()"#).unwrap();
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.take_navigation_requests(), vec![
            NavigationRequest::FormSubmit { url: "http://localhost/search?q=hello+world&symbol=a*b&source=button#result".to_string(), method: "GET".to_string(), body: None, content_type: None },
            NavigationRequest::FormSubmit { url: "http://localhost/save".to_string(), method: "POST".to_string(), body: Some(b"note=a\r\nb\r\n".to_vec()), content_type: Some("text/plain".to_string()) },
        ]);
    }

    #[test]
    fn multipart_form_submission_carries_file_part_bytes() {
        let mut runtime = runtime_from_html(
            r#"<form id="upload" action="/upload" method="post" enctype="multipart/form-data">
               <input name="note" value="hi"><input type="file" name="doc">
               </form>"#,
        );
        runtime
            .eval("document.getElementById(\"upload\").submit()")
            .unwrap();
        runtime.run_until_idle().unwrap();

        let requests = runtime.take_navigation_requests();
        let [NavigationRequest::FormSubmit {
            url,
            method,
            body: Some(body),
            content_type: Some(content_type),
        }] = requests.as_slice()
        else {
            panic!("expected one multipart form submission, got {requests:?}");
        };
        assert_eq!(url, "http://localhost/upload");
        assert_eq!(method, "POST");
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .expect("multipart content type with a boundary");
        assert_eq!(
            String::from_utf8(body.clone()).unwrap(),
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi\r\n\
                 --{boundary}\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n\r\n\
                 --{boundary}--\r\n"
            )
        );
    }

    #[test]
    fn request_submit_and_enter_dispatch_cancelable_submit_events() {
        let mut runtime = runtime_from_html(r#"<form id="f" action="/send"><input id="text" name="q" value="ok"><button id="send" name="via" value="enter">Send</button></form>"#);
        assert_eq!(eval_str(&mut runtime, r#"(() => { const seen = []; const f = document.getElementById("f"), send = document.getElementById("send"), text = document.getElementById("text"); f.addEventListener("submit", event => { seen.push(event.submitter && event.submitter.id); if (seen.length === 1) event.preventDefault(); }); f.requestSubmit(send); text.focus(); __omoikane_dispatch_keyboard_input("keydown", { key: "Enter" }); return seen.join(","); })()"#), "send,send");
        runtime.run_until_idle().unwrap();
        assert_eq!(runtime.take_navigation_requests(), vec![NavigationRequest::FormSubmit { url: "http://localhost/send?q=ok&via=enter".to_string(), method: "GET".to_string(), body: None, content_type: None }]);
    }

    #[test]
    fn multipart_form_data_is_used_by_request_and_xhr() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime.eval(r#"(() => { const data = new FormData(); data.append("a", "one"); data.append("a", "two"); const encoded = data.__multipart("fixed-boundary"); const request = new Request("/upload", { method: "POST", body: data }); const xhr = new XMLHttpRequest(); xhr.open("POST", "/upload"); xhr.send(data); return encoded.body === "--fixed-boundary\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\none\r\n--fixed-boundary\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\ntwo\r\n--fixed-boundary--\r\n" && request.headers.get("content-type").startsWith("multipart/form-data; boundary=") && request.body.includes("name=\"a\"") && xhr._headers["content-type"].startsWith("multipart/form-data; boundary="); })()"#).unwrap().as_boolean().unwrap());
    }

    #[test]
    fn runtime_initial_url_is_visible_during_bootstrap_and_resolves_resources() {
        let mut runtime = JsRuntime::with_document_and_url(
            default_document(),
            "https://example.com/dir/page.html?q=1#top",
        )
        .unwrap();

        assert!(runtime
            .eval(
                r#"location.href === "https://example.com/dir/page.html?q=1#top" &&
                   document.URL === location.href &&
                   new Request("asset.json").url === "https://example.com/dir/asset.json""#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn location_href_window_location_and_document_location_queue_navigation() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"location.href = "/href";
                   window.location = "/window";
                   document.location = "/document";"#,
            )
            .unwrap();
        runtime.run_until_idle().unwrap();

        assert_eq!(
            runtime.take_navigation_requests(),
            vec![
                NavigationRequest::Navigate {
                    url: "http://localhost/href".to_string(),
                    replace: false,
                },
                NavigationRequest::Navigate {
                    url: "http://localhost/window".to_string(),
                    replace: false,
                },
                NavigationRequest::Navigate {
                    url: "http://localhost/document".to_string(),
                    replace: false,
                },
            ]
        );
    }

    #[test]
    fn event_listener_objects_invoke_handle_event() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r#"(() => { const target = document.createElement("div"); const listener = { calls: 0, handleEvent(event) { this.calls++; this.type = event.type; } }; target.addEventListener("ready", listener); target.dispatchEvent(new Event("ready")); return listener.calls === 1 && listener.type === "ready"; })()"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn web_animations_finish_and_commit_final_keyframe() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(r#"(() => { const element = document.createElement("div"); document.body.appendChild(element); globalThis.finishedAnimation = false; const animation = element.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 20 }); animation.finished.then(() => { finishedAnimation = true; }); globalThis.testAnimation = animation; globalThis.animatedElement = element; })()"#)
            .unwrap();
        runtime.run_timers(100, 10, 100);
        assert!(runtime
            .eval(r#"finishedAnimation && testAnimation instanceof Animation && testAnimation.playState === "finished" && animatedElement.style.opacity === "1" && animatedElement.getAnimations().length === 1 && document.getAnimations().length === 1"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn streams_support_enqueue_read_and_transform() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(r#"globalThis.streamResult = ""; globalThis.streamSource = new ReadableStream({ start: function(c) { c.enqueue("a"); c.close(); } }); globalThis.streamReader = streamSource.getReader(); streamReader.read().then(function(r) { streamResult = r.value; });"#)
            .unwrap();
        runtime.run_jobs().unwrap();
        let result = runtime
            .eval("streamResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "a");
        runtime
            .eval(r#"globalThis.transformResult = ""; globalThis.transformStream = new TransformStream({ transform: function(value, controller) { controller.enqueue(value.toUpperCase()); } }); globalThis.transformWriter = transformStream.writable.getWriter(); transformWriter.write("b"); transformStream.readable.getReader().read().then(function(result) { transformResult = result.value; });"#)
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("transformResult")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "B"
        );
    }

    #[test]
    fn implements_fetch_api() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
            stream.write_all(response).unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(&format!(
                "globalThis.fetchResult = ''; fetch('http://127.0.0.1:{}/').then(r => r.text()).then(t => {{ globalThis.fetchResult = t; }});",
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();

        let result = runtime
            .eval("fetchResult")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        handle.join().unwrap();

        assert_eq!(result, "hello");
    }

    #[test]
    fn fetch_sends_method_headers_and_body_and_exposes_response_metadata() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("POST /submit HTTP/1.1\r\n"));
            assert!(request.to_ascii_lowercase().contains("x-client: omoikane\r\n"));
            assert!(request.ends_with("\r\n\r\nname=miku"));
            let body = b"created";
            write!(
                stream,
                "HTTP/1.1 201 Created\r\nX-Reply: accepted\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.fetchMetadata = null;
                    fetch("http://127.0.0.1:{}/submit", {{
                      method: "POST",
                      headers: {{ "X-Client": "omoikane" }},
                      body: "name=miku"
                    }}).then(async response => {{
                      fetchMetadata = {{
                        status: response.status,
                        statusText: response.statusText,
                        reply: response.headers.get("x-reply"),
                        url: response.url,
                        redirected: response.redirected,
                        body: await response.text()
                      }};
                    }});"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert!(runtime
            .eval(&format!(
                r#"fetchMetadata.status === 201 &&
                    fetchMetadata.statusText === "Created" &&
                    fetchMetadata.reply === "accepted" &&
                    fetchMetadata.url === "http://127.0.0.1:{}/submit" &&
                    fetchMetadata.redirected === false &&
                    fetchMetadata.body === "created""#,
                address.port()
            ))
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn fetch_reports_redirected_final_url() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let first_request = String::from_utf8(read_http_request(&mut first)).unwrap();
            assert!(first_request.starts_with("GET /start HTTP/1.1\r\n"));
            first
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();

            let (mut second, _) = listener.accept().unwrap();
            let second_request = String::from_utf8(read_http_request(&mut second)).unwrap();
            assert!(second_request.starts_with("GET /final HTTP/1.1\r\n"));
            second
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndone",
                )
                .unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.redirectResult = null;
                    fetch("http://127.0.0.1:{}/start").then(response => {{
                      redirectResult = {{ url: response.url, redirected: response.redirected }};
                    }});"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert!(runtime
            .eval(&format!(
                r#"redirectResult.redirected === true && redirectResult.url === "http://127.0.0.1:{}/final""#,
                address.port()
            ))
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn fetch_does_not_report_url_parser_normalization_as_a_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
            assert!(request.starts_with("GET / HTTP/1.1\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.normalizedResponse = null; fetch("http://127.0.0.1:{}").then(response => normalizedResponse = response);"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert!(runtime
            .eval(&format!(
                r#"normalizedResponse.redirected === false && normalizedResponse.url === "http://127.0.0.1:{}/""#,
                address.port()
            ))
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn network_failures_reject_fetch_and_fire_xhr_error_without_sync_throw() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(&format!(
                r#"globalThis.fetchThrew = false;
                    globalThis.fetchRejected = false;
                    try {{ fetch("http://127.0.0.1:{port}/fetch").catch(() => fetchRejected = true); }}
                    catch (_) {{ fetchThrew = true; }}
                    globalThis.xhrThrew = false;
                    globalThis.xhrRequestLocked = false;
                    globalThis.failureEvents = [];
                    const failedXhr = new XMLHttpRequest();
                    failedXhr.onerror = () => failureEvents.push("error");
                    failedXhr.onload = () => failureEvents.push("load");
                    failedXhr.onloadend = () => failureEvents.push("loadend");
                    failedXhr.open("POST", "http://127.0.0.1:{port}/xhr");
                    try {{ failedXhr.send("body"); }} catch (_) {{ xhrThrew = true; }}
                    try {{ failedXhr.setRequestHeader("X-Late", "no"); }} catch (_) {{ xhrRequestLocked = true; }}"#
            ))
            .unwrap();
        runtime.run_jobs().unwrap();

        assert!(runtime
            .eval(r#"!fetchThrew && fetchRejected && !xhrThrew && xhrRequestLocked && failedXhr.status === 0 && failedXhr.readyState === XMLHttpRequest.DONE && failureEvents.join(",") === "error,loadend""#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn fetch_and_xhr_share_the_page_cookie_jar() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let first_request = String::from_utf8(read_http_request(&mut first)).unwrap();
            assert!(first_request.starts_with("GET /session HTTP/1.1\r\n"));
            first
                .write_all(
                    b"HTTP/1.1 204 No Content\r\nSet-Cookie: session=miku; Path=/\r\nSet-Cookie2: legacy=hidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();

            let (mut second, _) = listener.accept().unwrap();
            let second_request = String::from_utf8(read_http_request(&mut second)).unwrap();
            assert!(second_request.starts_with("GET /profile HTTP/1.1\r\n"));
            assert!(second_request.contains("Cookie: session=miku\r\n"));
            second
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.cookieResponse = null; fetch("http://127.0.0.1:{}/session").then(response => cookieResponse = response);"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval("cookieResponse.headers.get('set-cookie') === null && cookieResponse.headers.get('set-cookie2') === null")
            .unwrap()
            .as_boolean()
            .unwrap());
        runtime
            .eval(&format!(
                r#"const cookieXhr = new XMLHttpRequest(); cookieXhr.open("GET", "http://127.0.0.1:{}/profile"); cookieXhr.send();"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert_eq!(runtime.eval("cookieXhr.status").unwrap().as_number(), Some(200.0));
    }

    #[test]
    fn cross_origin_fetch_checks_origin_and_filters_response_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
            assert!(request.contains("Origin: http://origin.test\r\n"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: http://origin.test\r\nAccess-Control-Expose-Headers: X-Public\r\nContent-Type: text/plain\r\nX-Public: visible\r\nX-Secret: hidden\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )
                .unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://origin.test/page".parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.corsResult = null;
                    fetch("http://127.0.0.1:{}/data").then(async response => corsResult = {{
                      type: response.type,
                      body: await response.text(),
                      contentType: response.headers.get("content-type"),
                      public: response.headers.get("x-public"),
                      secret: response.headers.get("x-secret")
                    }});"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert!(runtime
            .eval(r#"corsResult.type === "cors" && corsResult.body === "ok" && corsResult.contentType === "text/plain" && corsResult.public === "visible" && corsResult.secret === null"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn fetch_policy_values_are_validated_and_response_type_is_internal() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.policyChecks = [
                    ["credentials", ""],
                    ["mode", ""],
                    ["redirect", ""],
                  ].map(([name, value]) => {
                    try { new Request("http://example.test/", { [name]: value }); return false; }
                    catch (error) { return error instanceof TypeError; }
                  });
                  const constructed = new Response("visible", { type: "opaque" });
                  const forgedClone = constructed.clone();"#,
            )
            .unwrap();

        assert!(runtime
            .eval(r#"policyChecks.every(Boolean) && constructed.type === "basic" && forgedClone.type === "basic""#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn no_cors_is_opaque_and_xhr_cors_failure_fires_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
                if index == 0 {
                    assert!(!request.contains("Origin:"));
                } else {
                    assert!(request.contains("Origin: http://origin.test\r\n"));
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nX-Secret: hidden\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret").unwrap();
            }
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://origin.test/page".parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.opaqueResult = null;
                    fetch("http://127.0.0.1:{0}/opaque", {{ mode: "no-cors" }}).then(async response => opaqueResult = [response.type, response.status, response.url, await response.text(), response.headers.get("x-secret")]);
                    globalThis.corsFailureEvents = [];
                    globalThis.corsFailureXhr = new XMLHttpRequest();
                    corsFailureXhr.onerror = () => corsFailureEvents.push("error");
                    corsFailureXhr.onload = () => corsFailureEvents.push("load");
                    corsFailureXhr.onloadend = () => corsFailureEvents.push("loadend");
                    corsFailureXhr.open("GET", "http://127.0.0.1:{0}/xhr");
                    corsFailureXhr.send();"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert!(runtime
            .eval(r#"opaqueResult.join(",") === "opaque,0,,," && corsFailureXhr.status === 0 && corsFailureEvents.join(",") === "error,loadend""#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn cors_preflight_is_validated_and_cached() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
                if index == 0 {
                    assert!(request.starts_with("OPTIONS /data HTTP/1.1\r\n"));
                    assert!(request.contains("Access-Control-Request-Method: PUT\r\n"));
                    assert!(request.contains("Access-Control-Request-Headers: x-token\r\n"));
                    assert!(!request.contains("Cookie:"));
                    stream.write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: http://origin.test\r\nAccess-Control-Allow-Methods: PUT\r\nAccess-Control-Allow-Headers: X-Token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                } else {
                    assert!(request.starts_with("PUT /data HTTP/1.1\r\n"));
                    assert!(request.to_ascii_lowercase().contains("x-token: yes\r\n"));
                    stream.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: http://origin.test\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
                }
            }
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://origin.test/page".parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.preflightBodies = [];
                    fetch("http://127.0.0.1:{0}/data", {{ method: "PUT", headers: {{ "X-Token": "yes" }} }}).then(r => r.text()).then(body => preflightBodies.push(body));
                    fetch("http://127.0.0.1:{0}/data", {{ method: "PUT", headers: {{ "X-Token": "yes" }} }}).then(r => r.text()).then(body => preflightBodies.push(body));"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();
        assert_eq!(eval_str(&mut runtime, "preflightBodies.join(',')"), "ok,ok");
    }

    #[test]
    fn credentials_mode_and_xhr_with_credentials_send_cookies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
                if index < 2 {
                    assert!(!request.contains("Cookie:"));
                    stream.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: http://origin.test\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
                } else {
                    assert!(request.contains("Cookie: session=miku\r\n"));
                    stream.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: http://origin.test\r\nAccess-Control-Allow-Credentials: true\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
                }
            }
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://origin.test/page".parse().unwrap());
        let target: crate::http::Url = format!("http://{address}/data").parse().unwrap();
        runtime
            .host_state
            .borrow_mut()
            .http_client
            .cookie_jar_mut()
            .add_from_header_for_url("session=miku; Path=/", &target);
        runtime
            .eval(&format!(
                r#"globalThis.credentialFetch = null;
                    globalThis.omitFetch = null;
                    globalThis.sameOriginFetch = null;
                    fetch("http://127.0.0.1:{0}/omit", {{ credentials: "omit" }}).then(r => omitFetch = r.status);
                    fetch("http://127.0.0.1:{0}/same-origin").then(r => sameOriginFetch = r.status);
                    fetch("http://127.0.0.1:{0}/fetch", {{ credentials: "include" }}).then(r => credentialFetch = r.status);
                    globalThis.credentialXhr = new XMLHttpRequest();
                    credentialXhr.open("GET", "http://127.0.0.1:{0}/xhr");
                    credentialXhr.withCredentials = true;
                    credentialXhr.send();"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();
        assert!(runtime
            .eval("omitFetch === 200 && sameOriginFetch === 200 && credentialFetch === 200 && credentialXhr.status === 200")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn fetch_redirect_modes_follow_error_and_manual() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
                if index == 1 {
                    assert!(request.starts_with("GET /final HTTP/1.1\r\n"));
                    stream.write_all(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: http://origin.test\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndone").unwrap();
                } else {
                    stream.write_all(b"HTTP/1.1 302 Found\r\nAccess-Control-Allow-Origin: http://origin.test\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }
            }
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://origin.test/page".parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.redirectModes = {{}};
                    fetch("http://127.0.0.1:{0}/follow").then(async r => redirectModes.follow = [r.redirected, await r.text()]);
                    fetch("http://127.0.0.1:{0}/error", {{ redirect: "error" }}).catch(() => redirectModes.error = true);
                    fetch("http://127.0.0.1:{0}/manual", {{ redirect: "manual" }}).then(r => redirectModes.manual = [r.type, r.status, r.url]);"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();
        assert!(runtime
            .eval(r#"redirectModes.follow[0] === true && redirectModes.follow[1] === "done" && redirectModes.error === true && redirectModes.manual.join(",") === "opaqueredirect,0,""#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn exposes_script_element_and_message_event_constructors() {
        let html = r#"<html><head><script id="script"></script></head><body></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert!(
            runtime
                .eval(r#"document.getElementById("script") instanceof HTMLScriptElement"#)
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(
            runtime
                .eval(r#"(() => { const event = new MessageEvent("message", { data: "ok", origin: "https://example.com" }); return event instanceof Event && event.data === "ok" && event.origin === "https://example.com"; })()"#)
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn exposes_document_location_and_date_locale_time_string() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval("document.location === window.location && document.charset === \"UTF-8\" && document.referrer === \"\"")
            .unwrap()
            .as_boolean()
            .unwrap());
        let value = runtime
            .eval("new Date(2020, 0, 1, 2, 3, 4).toLocaleTimeString()")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(value, "02:03:04");
    }

    #[test]
    fn exposes_css_namespace_and_escapes_identifiers() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r#"CSS.escape("0a b") === "\\30 a\\ b" && CSS.supports("display", "block") === true && CSS.supports("(display: block)") === true && CSS.supports("(display: grid) and (color: red)") === true && CSS.supports("not (future-property: value)") === true && CSS.supports("unknown", "value") === false && CSS.supports("width", "12") === false"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn image_constructor_creates_an_html_image_element() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r#"(() => { const image = new Image(40, 30); image.src = "asset.png"; return image instanceof HTMLImageElement && image.width === 40 && image.height === 30 && image.src === "asset.png"; })()"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn request_and_xhr_resolve_relative_urls() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r#"(() => { const xhr = new XMLHttpRequest(); xhr.open("GET", "/api/data"); return new Request("asset.js").url === "http://localhost/asset.js" && xhr._url === "http://localhost/api/data"; })()"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn xml_http_request_get_completes_and_abort_suppresses_completion() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
            stream.write_all(response).unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.xhrEvents = []; const xhr = new XMLHttpRequest(); xhr.onload = () => xhrEvents.push("load"); xhr.onloadend = () => xhrEvents.push("loadend"); xhr.open("GET", "http://127.0.0.1:{}/"); xhr.send(); globalThis.xhr = xhr;"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();
        assert!(runtime
            .eval(r#"xhr.readyState === XMLHttpRequest.DONE && xhr.status === 200 && xhr.responseText === "hello" && xhrEvents.join(",") === "load,loadend""#)
            .unwrap()
            .as_boolean()
            .unwrap());

        let abort_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let abort_address = abort_listener.local_addr().unwrap();
        let abort_handle = thread::spawn(move || {
            let (mut stream, _) = abort_listener.accept().unwrap();
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nlater";
            stream.write_all(response).unwrap();
        });
        runtime
            .eval(&format!(
                r#"xhrEvents = []; xhr.open("GET", "http://127.0.0.1:{}/"); xhr.send(); xhr.abort();"#,
                abort_address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        abort_handle.join().unwrap();
        assert!(runtime
            .eval(r#"xhr.readyState === XMLHttpRequest.UNSENT && xhr.status === 0 && xhr.responseText === "" && xhrEvents.join(",") === "loadend""#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn xml_http_request_sends_request_and_exposes_states_and_response_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("PUT /api/item HTTP/1.1\r\n"));
            assert!(request.to_ascii_lowercase().contains("x-requested-with: omoikane\r\n"));
            assert!(request.ends_with("\r\n\r\nupdated"));
            let body = b"accepted";
            write!(
                stream,
                "HTTP/1.1 202 Accepted\r\nX-Result: saved\r\nSet-Cookie: secret=value\r\nSet-Cookie2: legacy=hidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });

        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(format!("http://{address}/").parse().unwrap());
        runtime
            .eval(&format!(
                r#"globalThis.xhrStates = [];
                    const semanticXhr = new XMLHttpRequest();
                    semanticXhr.onreadystatechange = () => xhrStates.push(semanticXhr.readyState);
                    semanticXhr.open("PUT", "http://127.0.0.1:{}/api/item");
                    semanticXhr.setRequestHeader("X-Requested-With", "omoikane");
                    semanticXhr.send("updated");"#,
                address.port()
            ))
            .unwrap();
        runtime.run_jobs().unwrap();
        handle.join().unwrap();

        assert!(runtime
            .eval(&format!(
                r#"semanticXhr.status === 202 &&
                    semanticXhr.statusText === "Accepted" &&
                    semanticXhr.responseText === "accepted" &&
                    semanticXhr.responseURL === "http://127.0.0.1:{}/api/item" &&
                    semanticXhr.getResponseHeader("X-Result") === "saved" &&
                    semanticXhr.getResponseHeader("Set-Cookie") === null &&
                    semanticXhr.getResponseHeader("Set-Cookie2") === null &&
                    semanticXhr.getAllResponseHeaders().includes("x-result: saved\r\n") &&
                    !semanticXhr.getAllResponseHeaders().includes("set-cookie") &&
                    xhrStates.join(",") === "1,2,3,4""#,
                address.port()
            ))
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn xml_http_request_open_resets_request_and_response_state() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(r#"(() => { const xhr = new XMLHttpRequest(); xhr.open("POST", "https://example.com/log"); xhr.status = 204; xhr.statusText = "No Content"; xhr.responseText = "stale"; xhr.responseURL = "https://example.com/old"; xhr.setRequestHeader("x-old", "yes"); xhr.open("PUT", "https://example.com/next"); return xhr.readyState === XMLHttpRequest.OPENED && xhr.status === 0 && xhr.statusText === "" && xhr.responseText === "" && xhr.responseURL === "" && Object.keys(xhr._headers).length === 0; })()"#)
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn eval_safe_catches_syntax_error() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime.eval_safe("this is not valid javascript }{");
        assert!(
            result.is_err(),
            "eval_safe should return Err for syntax errors"
        );
    }

    #[test]
    fn eval_safe_catches_runtime_error() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime.eval_safe("undefinedFunction()");
        assert!(
            result.is_err(),
            "eval_safe should return Err for runtime errors"
        );
    }

    #[test]
    fn eval_safe_returns_ok_for_valid_code() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime.eval_safe("1 + 2");
        assert!(result.is_ok(), "eval_safe should return Ok for valid code");
    }

    #[test]
    fn sandbox_config_has_default_timeout() {
        let config = SandboxConfig::default();
        assert_eq!(config.timeout, std::time::Duration::from_secs(5));
    }

    #[test]
    fn runtime_with_custom_sandbox() {
        let sandbox = SandboxConfig {
            timeout: std::time::Duration::from_secs(1),
        };
        let doc = crate::dom::NodeHandle::document();
        let mut runtime = JsRuntime::with_document_and_sandbox(doc, sandbox).unwrap();
        let result = runtime.eval_safe("1 + 1");
        assert!(result.is_ok());
    }

    #[test]
    fn process_and_require_do_not_exist_in_global() {
        let mut runtime = JsRuntime::new().unwrap();
        // Verify process is not defined at all (not just undefined value)
        let result = runtime.eval_safe("'process' in globalThis");
        assert_eq!(
            result.unwrap().as_boolean(),
            Some(false),
            "'process' should not exist in globalThis"
        );
        let result = runtime.eval_safe("'require' in globalThis");
        assert_eq!(
            result.unwrap().as_boolean(),
            Some(false),
            "'require' should not exist in globalThis"
        );
        // Accessing them directly should throw ReferenceError
        let result = runtime.eval_safe("process");
        assert!(
            result.is_err(),
            "accessing 'process' should throw ReferenceError"
        );
        let result = runtime.eval_safe("require");
        assert!(
            result.is_err(),
            "accessing 'require' should throw ReferenceError"
        );
    }

    #[test]
    fn classname_getter_and_setter() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.className = "foo bar";
        "#,
            )
            .unwrap();

        let result = runtime
            .eval("document.querySelector('div').className")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "foo bar");
    }

    #[test]
    fn classlist_add_remove_contains() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.classList.add("alpha", "beta");
        "#,
            )
            .unwrap();

        let has_alpha = runtime
            .eval("document.querySelector('div').classList.contains('alpha')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(has_alpha, "classList should contain 'alpha'");

        runtime
            .eval("document.querySelector('div').classList.remove('alpha')")
            .unwrap();
        let has_alpha = runtime
            .eval("document.querySelector('div').classList.contains('alpha')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            !has_alpha,
            "classList should not contain 'alpha' after remove"
        );

        let has_beta = runtime
            .eval("document.querySelector('div').classList.contains('beta')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(has_beta, "classList should still contain 'beta'");
    }

    #[test]
    fn classlist_toggle() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.classList.toggle("active");
        "#,
            )
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(result, "toggle should return true when adding");

        let result = runtime
            .eval("document.querySelector('div').classList.toggle('active')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(!result, "toggle should return false when removing");
    }

    #[test]
    fn style_getter_and_setter() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.backgroundColor = "red";
            el.style.fontSize = "16px";
        "#,
            )
            .unwrap();

        let bg = runtime
            .eval("document.querySelector('div').style.backgroundColor")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(bg, "red");

        let fs = runtime
            .eval("document.querySelector('div').style.fontSize")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(fs, "16px");

        // Verify the style attribute on the DOM node
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert!(
            style_attr.contains("background-color: red"),
            "style attr: {style_attr}"
        );
        assert!(
            style_attr.contains("font-size: 16px"),
            "style attr: {style_attr}"
        );
    }

    #[test]
    fn style_parser_preserves_quoted_and_unquoted_data_uri_semicolons() {
        let doc = NodeHandle::document();
        let unquoted = NodeHandle::element("div");
        let quoted = NodeHandle::element("div");
        unquoted.set_attribute("id", "unquoted");
        quoted.set_attribute("id", "quoted");
        doc.append_child(unquoted.clone());
        doc.append_child(quoted.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
                const unquoted = document.getElementById("unquoted");
                const quoted = document.getElementById("quoted");
                unquoted.setAttribute(
                    "style",
                    "background-image: url(data:image/png;base64,AAAA); color: red;"
                );
                quoted.setAttribute(
                    "style",
                    'background-image: url("data:image/svg+xml;utf8,<svg></svg>"); color: blue;'
                );
                // Force a read-modify-write cycle; both data URLs must survive
                // when the declaration list is serialized again.
                unquoted.style.marginTop = "1px";
                quoted.style.marginTop = "2px";
                "#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "unquoted.style.backgroundImage"),
            "url(data:image/png;base64,AAAA)"
        );
        assert_eq!(eval_str(&mut runtime, "unquoted.style.color"), "red");
        assert_eq!(
            eval_str(&mut runtime, "quoted.style.backgroundImage"),
            r#"url("data:image/svg+xml;utf8,<svg></svg>")"#
        );
        assert_eq!(eval_str(&mut runtime, "quoted.style.color"), "blue");
        assert!(unquoted.attributes().unwrap()["style"].contains("data:image/png;base64,AAAA"));
        assert!(
            quoted.attributes().unwrap()["style"].contains("data:image/svg+xml;utf8,<svg></svg>")
        );
    }

    #[test]
    fn style_set_property_kebab_and_camel_are_consistent() {
        // setProperty uses CSS (kebab-case) names; the value must be readable via
        // both the camelCase accessor and getPropertyValue, and vice versa.
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.setProperty("background-color", "blue");
            el.style.marginTop = "5px";
        "#,
            )
            .unwrap();

        // kebab set -> camelCase read and getPropertyValue read.
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.backgroundColor"
            ),
            "blue"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyValue('background-color')"
            ),
            "blue"
        );
        // camelCase set -> kebab getPropertyValue read.
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyValue('margin-top')"
            ),
            "5px"
        );
    }

    #[test]
    fn style_set_property_records_priority_without_leaking_into_value() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.setProperty("color", "red", "important");
        "#,
            )
            .unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyValue('color')"
            ),
            "red",
            "getPropertyValue must return the value without the priority flag"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyPriority('color')"
            ),
            "important"
        );
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            style_attr, "color: red !important;",
            "priority must be serialized into the style attribute"
        );
    }

    #[test]
    fn style_item_length_and_css_text() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.setProperty("color", "red");
            el.style.setProperty("display", "block");
        "#,
            )
            .unwrap();

        assert_eq!(
            eval_num(&mut runtime, "document.querySelector('div').style.length"),
            2.0
        );
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.item(0)"),
            "color"
        );
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.item(1)"),
            "display"
        );
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.item(2)"),
            "",
            "out-of-range item() must return an empty string"
        );
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.cssText"),
            "color: red; display: block;"
        );
    }

    #[test]
    fn style_css_text_setter_replaces_all_declarations() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.color = "red";
            el.style.cssText = "font-size: 12px; display: flex";
        "#,
            )
            .unwrap();

        // The prior `color` declaration is gone; the two new ones are present.
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.color"),
            ""
        );
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.fontSize"),
            "12px"
        );
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.display"),
            "flex"
        );
        assert_eq!(
            eval_num(&mut runtime, "document.querySelector('div').style.length"),
            2.0
        );

        // Setting cssText to the empty string clears every declaration.
        runtime
            .eval("document.querySelector('div').style.cssText = ''")
            .unwrap();
        assert_eq!(
            eval_num(&mut runtime, "document.querySelector('div').style.length"),
            0.0
        );
    }

    #[test]
    fn style_remove_property_returns_previous_value_and_updates_attribute() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.setProperty("color", "red");
            el.style.setProperty("display", "block");
            globalThis.__removed = el.style.removeProperty("color");
            globalThis.__missing = el.style.removeProperty("margin");
        "#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "globalThis.__removed"),
            "red",
            "removeProperty must return the value the property had before removal"
        );
        assert_eq!(
            eval_str(&mut runtime, "globalThis.__missing"),
            "",
            "removeProperty on an unset property returns an empty string"
        );
        // The removed declaration is gone; the untouched one remains.
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.color"),
            ""
        );
        assert_eq!(
            eval_str(&mut runtime, "document.querySelector('div').style.display"),
            "block"
        );
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            style_attr, "display: block;",
            "style attribute must no longer contain the removed declaration"
        );
    }

    #[test]
    fn style_remove_property_reflects_into_get_computed_style() {
        // An inline declaration overrides the cascade; removing it must restore
        // the cascaded value in getComputedStyle.
        let html = r#"<html><head><style>
            #target { white-space: nowrap; }
        </style></head><body><p id="target">hi</p></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // Baseline: the cascade wins.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').whiteSpace"
            ),
            "nowrap"
        );

        // Inline setProperty overrides the cascade.
        runtime
            .eval("document.getElementById('target').style.setProperty('white-space', 'pre-wrap')")
            .unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').whiteSpace"
            ),
            "pre-wrap",
            "inline setProperty must override the cascade in getComputedStyle"
        );

        // removeProperty returns the inline value and restores the cascade.
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.getElementById('target').style.removeProperty('white-space')"
            ),
            "pre-wrap"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').whiteSpace"
            ),
            "nowrap",
            "after removeProperty the cascaded value must be visible again"
        );
    }

    #[test]
    fn style_gsap_border_top_measure_then_remove_roundtrip() {
        // Regression for kasaneteto.jp: GSAP ScrollTrigger sets `borderTop` to
        // measure the scrollbar, then restores via `removeProperty("border-top")`.
        // The whole sequence must run without a "not a callable function" error
        // and leave the element in its original (undeclared) state.
        let doc = NodeHandle::document();
        let body = NodeHandle::element("body");
        doc.append_child(body.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const r = document.querySelector("body").style;
            r.borderTop = "1px solid #000";
            globalThis.__measured = r.borderTop;      // measurement step
            globalThis.__removed = r.removeProperty("border-top");
            globalThis.__after = r.borderTop;
        "#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "globalThis.__measured"),
            "1px solid #000",
            "camelCase set must be readable during the measurement step"
        );
        assert_eq!(
            eval_str(&mut runtime, "globalThis.__removed"),
            "1px solid #000",
            "removeProperty(kebab) must return the value set via camelCase"
        );
        assert_eq!(
            eval_str(&mut runtime, "globalThis.__after"),
            "",
            "the property must be undeclared after removeProperty"
        );
        let style_attr = body
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert!(
            !style_attr.contains("border-top"),
            "style attribute must no longer mention border-top: {style_attr}"
        );

        // removeProperty must be an actual callable function on the live object.
        assert_eq!(
            eval_str(
                &mut runtime,
                "typeof document.querySelector('body').style.removeProperty"
            ),
            "function"
        );
    }

    #[test]
    fn get_set_attribute_round_trip() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.setAttribute("data-value", "42");
        "#,
            )
            .unwrap();

        let result = runtime
            .eval("document.querySelector('div').getAttribute('data-value')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "42");

        let missing = runtime
            .eval("document.querySelector('div').getAttribute('nonexistent')")
            .unwrap();
        assert!(
            missing.is_null(),
            "getAttribute for missing attr should return null"
        );
    }

    #[test]
    fn classlist_length_and_item() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.classList.add("a", "b", "c");
        "#,
            )
            .unwrap();

        let len = runtime
            .eval("document.querySelector('div').classList.length")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(len, 3.0);

        let item0 = runtime
            .eval("document.querySelector('div').classList.item(0)")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(item0, "a");

        let item_oob = runtime
            .eval("document.querySelector('div').classList.item(99)")
            .unwrap();
        assert!(item_oob.is_null(), "out-of-range item should return null");
    }

    #[test]
    fn classlist_toggle_with_force() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // force=true always adds
        let result = runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.classList.toggle("x", true);
        "#,
            )
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(result, "toggle(cls, true) should return true");

        // force=true when already present keeps it
        let result = runtime
            .eval("document.querySelector('div').classList.toggle('x', true)")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(result, "toggle(cls, true) when present should return true");

        // force=false always removes
        let result = runtime
            .eval("document.querySelector('div').classList.toggle('x', false)")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(!result, "toggle(cls, false) should return false");

        let has = runtime
            .eval("document.querySelector('div').classList.contains('x')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(!has, "x should be removed after toggle(x, false)");
    }

    #[test]
    fn style_value_zero_is_preserved() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.marginTop = 0;
        "#,
            )
            .unwrap();

        let result = runtime
            .eval("document.querySelector('div').style.marginTop")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            result, "0",
            "style value 0 should be preserved, not removed"
        );
    }

    #[test]
    fn style_normalization_host_call_is_limited_to_transition_properties() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_eq!(
            runtime
                .eval(
                    r#"(() => {
                      const original = __omoikane_normalize_style_value;
                      let calls = 0;
                      globalThis.__omoikane_normalize_style_value = (...args) => {
                        calls++;
                        return original(...args);
                      };
                      const target = document.createElement("div");
                      target.style.width = "10px";
                      target.style.opacity = "0.5";
                      target.style.transition = "opacity 1s linear";
                      return calls;
                    })()"#,
                )
                .unwrap()
                .as_number(),
            Some(1.0)
        );
    }

    #[test]
    fn style_custom_property_preserves_case_across_roundtrip() {
        // Custom properties are case-sensitive: `--Foo` must not be folded to
        // lowercase or camelCase→kebab mangled into `---foo`. The value set via
        // setProperty must be readable via getPropertyValue and removable via
        // removeProperty using the same mixed-case name.
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(r#"document.querySelector("div").style.setProperty("--Foo", "10px");"#)
            .unwrap();

        // The declaration is serialized with its case preserved.
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            style_attr, "--Foo: 10px;",
            "custom property name must keep its original case in the style attribute"
        );

        // Round-trip read with the exact mixed-case name.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"document.querySelector('div').style.getPropertyValue('--Foo')"#
            ),
            "10px",
            "getPropertyValue must resolve the case-preserved custom property"
        );
        // A differently-cased name must NOT match (case-sensitive).
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"document.querySelector('div').style.getPropertyValue('--foo')"#
            ),
            "",
            "custom property lookup is case-sensitive"
        );

        // removeProperty returns the prior value and clears the declaration.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"document.querySelector('div').style.removeProperty('--Foo')"#
            ),
            "10px",
            "removeProperty must return the prior value of the custom property"
        );
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            style_attr, "",
            "the custom property must be gone after removeProperty"
        );
    }

    #[test]
    fn style_set_property_normalizes_duplicate_declarations() {
        // A duplicate declaration (here injected via cssText, e.g. as authored)
        // is resolved last-wins on read; a subsequent setProperty must update the
        // winning value AND collapse the block to a single declaration.
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.cssText = "color: red; color: blue";
            el.style.setProperty("color", "green");
        "#,
            )
            .unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyValue('color')"
            ),
            "green",
            "setProperty must update the winning declaration even with duplicates"
        );
        assert_eq!(
            eval_num(&mut runtime, "document.querySelector('div').style.length"),
            1.0,
            "duplicate declarations must collapse to a single one"
        );
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            style_attr, "color: green;",
            "the style attribute must contain exactly one normalized declaration"
        );
    }

    #[test]
    fn style_remove_property_returns_last_wins_and_removes_all_duplicates() {
        // With duplicate declarations, removeProperty must return the last-wins
        // value (not the first) and remove every occurrence.
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.cssText = "color: red; color: blue; display: block";
            globalThis.__removed = el.style.removeProperty("color");
        "#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "globalThis.__removed"),
            "blue",
            "removeProperty must return the last-wins value, not the first"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyValue('color')"
            ),
            "",
            "every duplicate occurrence of the property must be removed"
        );
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            style_attr, "display: block;",
            "only the unrelated declaration must remain"
        );
    }

    #[test]
    fn style_set_property_priority_only_accepts_important() {
        // Per CSSOM, only "important" (ASCII case-insensitive) is a valid
        // priority. Any other non-empty token is treated as no priority; a
        // case-varied "IMPORTANT" is accepted.
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            el.style.setProperty("color", "red", "foo");
            el.style.setProperty("display", "block", "IMPORTANT");
        "#,
            )
            .unwrap();

        // "foo" is not a valid priority -> treated as none, no bogus `!foo`.
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyPriority('color')"
            ),
            "",
            "an invalid priority token must be treated as no priority"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyValue('color')"
            ),
            "red",
            "the value must still be set when the priority is invalid"
        );
        // "IMPORTANT" matches ASCII case-insensitively.
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.querySelector('div').style.getPropertyPriority('display')"
            ),
            "important",
            "priority matching is ASCII case-insensitive"
        );
        let style_attr = div
            .attributes()
            .unwrap()
            .get("style")
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            style_attr, "color: red; display: block !important;",
            "invalid priority must not serialize a bogus flag; valid one must"
        );
    }

    #[test]
    fn remove_event_listener_stops_callback() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            let count = 0;
            const el = document.querySelector("div");
            function handler() { count++; }
            el.addEventListener("click", handler);
            el.dispatchEvent(new Event("click"));
            el.removeEventListener("click", handler);
            el.dispatchEvent(new Event("click"));
        "#,
            )
            .unwrap();

        let count = runtime
            .eval("count")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(
            count, 1.0,
            "handler should fire once before removal, not after"
        );
    }

    #[test]
    fn fire_dom_content_loaded_invokes_listeners() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"
            let loaded = false;
            document.addEventListener("DOMContentLoaded", () => { loaded = true; });
        "#,
            )
            .unwrap();

        let before = runtime.eval("loaded").unwrap().as_boolean().unwrap();
        assert!(!before, "loaded should be false before DOMContentLoaded");

        runtime.fire_dom_content_loaded().unwrap();

        let after = runtime.eval("loaded").unwrap().as_boolean().unwrap();
        assert!(after, "loaded should be true after DOMContentLoaded");
    }

    #[test]
    fn fire_document_event_dispatches_custom_event() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"
            let eventFired = "";
            document.addEventListener("myevent", (e) => { eventFired = e.type; });
        "#,
            )
            .unwrap();

        runtime.fire_document_event("myevent").unwrap();

        let result = runtime
            .eval("eventFired")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "myevent");
    }

    #[test]
    fn fire_document_event_escapes_special_chars() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"
            let special = "";
            document.addEventListener("te'st", (e) => { special = e.type; });
        "#,
            )
            .unwrap();

        runtime.fire_document_event("te'st").unwrap();

        let result = runtime
            .eval("special")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "te'st");
    }

    #[test]
    fn add_event_listener_deduplicates() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            let count = 0;
            const el = document.querySelector("div");
            function handler() { count++; }
            el.addEventListener("click", handler);
            el.addEventListener("click", handler);
            el.addEventListener("click", handler);
            el.dispatchEvent(new Event("click"));
        "#,
            )
            .unwrap();

        let count = runtime
            .eval("count")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(
            count, 1.0,
            "duplicate addEventListener should only fire once"
        );
    }

    #[test]
    fn execute_document_scripts_runs_inline_script() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <div id="target"></div>
            <script>
                document.getElementById("target").setAttribute("data-ran", "yes");
            </script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "should have no errors: {errors:?}");

        let target = doc.query_selector("#target").expect("should find #target");
        let attrs = target.attributes().unwrap_or_default();
        assert_eq!(attrs.get("data-ran").map(|s| s.as_str()), Some("yes"));
    }

    #[test]
    fn execute_document_scripts_fires_dom_content_loaded() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <script>
                var loaded = false;
                document.addEventListener("DOMContentLoaded", function() { loaded = true; });
            </script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.execute_document_scripts(None);

        let result = runtime.eval("loaded").unwrap().as_boolean().unwrap();
        assert!(
            result,
            "DOMContentLoaded should fire after execute_document_scripts"
        );
    }

    #[test]
    fn execute_document_scripts_skips_non_js_type() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <script type="application/json">{"key": "value"}</script>
            <script>var jsRan = true;</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "errors: {errors:?}");

        let result = runtime.eval("jsRan").unwrap().as_boolean().unwrap();
        assert!(result, "JS script should run");
    }

    #[test]
    fn document_current_script_tracks_and_can_remove_running_script() {
        let html = r#"<html><body><script>globalThis.currentScriptId = document.currentScript.id; document.currentScript.remove();</script><script id="second">globalThis.secondRan = true;</script></body></html>"#;
        let mut runtime = runtime_from_html(html);
        let document = runtime.document();
        let first = document.query_selector("script").unwrap();
        first.set_attribute("id", "first");
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(document.query_selector("#first").is_none());
        assert_eq!(
            runtime
                .eval("currentScriptId")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "first"
        );
        assert!(runtime.eval("secondRan").unwrap().as_boolean().unwrap());
        assert!(
            runtime
                .eval("document.currentScript === null")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn execute_document_scripts_error_does_not_stop_others() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <script>var first = true;</script>
            <script>undefinedFunction();</script>
            <script>var third = true;</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert_eq!(errors.len(), 1, "should have 1 error");

        let first = runtime.eval("first").unwrap().as_boolean().unwrap();
        assert!(first, "first script should run");
        let third = runtime.eval("third").unwrap().as_boolean().unwrap();
        assert!(third, "third script should run despite second failing");
    }

    #[test]
    fn get_attribute_is_case_sensitive_for_xml_and_case_insensitive_for_html() {
        use crate::html::TreeBuilder;

        let xml = crate::xml::parse(b"<Root a='lower'/>").unwrap();
        let mut xml_runtime = JsRuntime::with_document(xml).unwrap();
        assert_eq!(
            eval_string_value(
                &mut xml_runtime,
                "[document.documentElement.getAttribute('A') === null, document.documentElement.getAttribute('a')].join('|')",
            )
            .as_deref(),
            Some("true|lower"),
            "XML getAttribute('A') must not return the lowercase a attribute",
        );

        let html = TreeBuilder::parse("<html><body><div a='html'></div></body></html>").document();
        let mut html_runtime = JsRuntime::with_document(html).unwrap();
        assert_eq!(
            eval_string_value(
                &mut html_runtime,
                "document.querySelector('div').getAttribute('A')"
            )
            .as_deref(),
            Some("html"),
            "HTML getAttribute must retain ASCII case-insensitive lookup",
        );
    }

    #[test]
    fn execute_document_scripts_defer_only_applies_to_external() {
        use crate::html::TreeBuilder;
        // defer on inline script should be ignored (HTML spec);
        // both scripts execute in document order.
        let html = r#"<html><body>
            <script defer>var order = (typeof order === 'undefined' ? '' : order) + 'first,';</script>
            <script>var order = (typeof order === 'undefined' ? '' : order) + 'second,';</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.execute_document_scripts(None);

        let result = runtime
            .eval("order")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            result, "first,second,",
            "defer on inline should be ignored; both run in order"
        );
    }

    // --- 016-4: load event + on* inline handlers + drivers primitives ---

    #[test]
    fn body_onload_attribute_fires_on_load_and_can_touch_dom() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body onload="document.body.setAttribute('data-loaded', 'yes'); globalThis.loadCount = (globalThis.loadCount || 0) + 1;">
            <p id="p">hi</p>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.execute_document_scripts(None);

        // Before load fires, the handler must not have run.
        let before = runtime
            .eval("globalThis.loadCount || 0")
            .unwrap()
            .as_number()
            .unwrap();
        assert_eq!(before, 0.0, "onload handler must not fire before load");

        runtime.wire_inline_event_handlers().unwrap();
        runtime.fire_load().unwrap();

        let count = runtime
            .eval("globalThis.loadCount")
            .unwrap()
            .as_number()
            .unwrap();
        assert_eq!(count, 1.0, "body onload should fire exactly once on load");
        let attr = runtime
            .eval("document.body.getAttribute('data-loaded')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(attr, "yes", "onload handler should have mutated the DOM");
    }

    #[test]
    fn inline_onclick_handler_receives_event_argument() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body><button id="b" onclick="globalThis.clickedType = event.type;">go</button></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.execute_document_scripts(None);
        runtime.wire_inline_event_handlers().unwrap();

        runtime
            .eval("document.getElementById('b').dispatchEvent(new Event('click'))")
            .unwrap();
        let ty = runtime
            .eval("globalThis.clickedType")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            ty, "click",
            "onclick handler should receive the event argument"
        );
    }

    #[test]
    fn load_fires_after_dom_content_loaded() {
        use crate::html::TreeBuilder;
        // Record the order in which DOMContentLoaded and load fire.
        let html = r#"<html><body onload="globalThis.order.push('load');">
            <script>
                globalThis.order = [];
                document.addEventListener('DOMContentLoaded', function () { globalThis.order.push('dcl'); });
            </script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        // execute_document_scripts fires DOMContentLoaded at the end.
        runtime.execute_document_scripts(None);
        runtime.wire_inline_event_handlers().unwrap();
        runtime.fire_load().unwrap();

        let order = runtime
            .eval("globalThis.order.join(',')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(order, "dcl,load", "DOMContentLoaded must fire before load");
    }

    #[test]
    fn text_node_data_get_and_set() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let value = runtime
            .eval(
                "const t = document.createTextNode('hello'); \
                 const before = t.data; \
                 t.data = 'world'; \
                 before + '|' + t.data + '|' + t.textContent + '|' + (t instanceof Text) + '|' + t.length",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            value, "hello|world|world|true|5",
            "Text.data get/set must map to character data"
        );
    }

    #[test]
    fn comment_node_data_get_and_set() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let value = runtime
            .eval(
                "const c = document.createComment('note'); \
                 const before = c.data; \
                 c.data = 'changed'; \
                 before + '|' + c.data + '|' + (c instanceof Comment)",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            value, "note|changed|true",
            "Comment.data get/set must map to character data"
        );
    }

    #[test]
    fn character_data_setter_distinguishes_null_from_undefined() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const nodes = [
                    document.createTextNode('x'),
                    document.createComment('x'),
                    document.createProcessingInstruction('target', 'x')
                ];
                return nodes.map(node => {
                    node.data = null;
                    const nullValue = node.data;
                    node.data = undefined;
                    return [nullValue, node.data, node.length].join('|');
                }).join(';');
            })()"#,
        );
        assert_eq!(actual, "|undefined|9;|undefined|9;|undefined|9");
    }

    #[test]
    fn element_and_node_interfaces_have_distinct_prototype_chains() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const generic = document.createElement('article');
                const input = document.createElement('input');
                const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
                const xml = document.createElementNS('urn:test', 'widget');
                const nullNs = document.createElementNS(null, 'widget');
                const xhtml = document.createElementNS('http://www.w3.org/1999/xhtml', 'input');
                return [
                    Node === Element,
                    Element === HTMLElement,
                    generic instanceof Node, generic instanceof Element, generic instanceof HTMLElement,
                    input instanceof Node, input instanceof Element, input instanceof HTMLElement, input instanceof HTMLInputElement,
                    svg instanceof Node, svg instanceof Element, svg instanceof HTMLElement, svg instanceof SVGElement,
                    xml instanceof Node, xml instanceof Element, xml instanceof HTMLElement,
                    nullNs instanceof Element, nullNs instanceof HTMLElement,
                    xhtml instanceof Element, xhtml instanceof HTMLElement, xhtml instanceof HTMLInputElement,
                    document instanceof Node, document instanceof Element, document instanceof HTMLElement,
                    document.createDocumentFragment() instanceof Node, document.createDocumentFragment() instanceof Element,
                    typeof Node.prototype.remove, typeof Element.prototype.remove
                ].join('|');
            })()"#,
        );
        assert_eq!(
            actual,
            "false|false|true|true|true|true|true|true|true|true|true|false|true|true|true|false|true|false|true|true|true|true|false|false|true|false|undefined|function"
        );
    }

    #[test]
    fn element_only_members_are_owned_by_element_prototype() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const names = [
                    "namespaceURI", "prefix", "localName", "tagName",
                    "id", "className", "classList",
                    "getAttribute", "setAttribute", "hasAttribute", "removeAttribute",
                    "setAttributeNS", "getAttributeNS", "removeAttributeNS", "attributes",
                    "matches", "closest"
                ];
                const element = document.createElement("div");
                const text = document.createTextNode("text");
                return [
                    names.every(name => Object.prototype.hasOwnProperty.call(Element.prototype, name)),
                    names.every(name => !Object.prototype.hasOwnProperty.call(Node.prototype, name)),
                    names.every(name => !(name in document)),
                    names.every(name => !(name in text)),
                    element.getAttribute("missing") === null,
                    element.matches("div")
                ].join("|");
            })()"#,
        );
        assert_eq!(actual, "true|true|true|true|true|true");
    }

    #[test]
    fn dom_mixins_and_html_members_have_spec_scoped_prototypes() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const own = (prototype, name) =>
                    Object.prototype.hasOwnProperty.call(prototype, name);
                const fragment = document.createDocumentFragment();
                const text = document.createTextNode("x");
                const div = document.createElement("div");
                const input = document.createElement("input");
                const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
                const moved = [
                    "querySelector", "querySelectorAll", "children",
                    "firstElementChild", "lastElementChild", "childElementCount",
                    "getElementsByTagName", "getElementsByClassName",
                    "nextElementSibling", "previousElementSibling",
                    "innerHTML", "style", "dataset", "title", "innerText",
                    "getBoundingClientRect", "clientWidth", "scrollTop",
                    "offsetWidth", "click", "focus", "hidden",
                    "checked", "defaultChecked", "disabled", "value", "type", "name",
                    "publicId", "systemId", "internalSubset"
                ];
                return [
                    moved.every(name => !own(Node.prototype, name)),
                    own(Document.prototype, "querySelector"),
                    own(DocumentFragment.prototype, "querySelector"),
                    own(Element.prototype, "querySelector"),
                    own(Element.prototype, "innerHTML"),
                    own(Document.prototype, "innerHTML"),
                    own(CharacterData.prototype, "nextElementSibling"),
                    own(HTMLElement.prototype, "title"),
                    own(HTMLElement.prototype, "style"),
                    own(SVGElement.prototype, "style"),
                    own(HTMLInputElement.prototype, "checked"),
                    own(DocumentType.prototype, "publicId"),
                    document.implementation.createDocumentType("html", "", "") instanceof DocumentType,
                    !("innerHTML" in text),
                    !("style" in text),
                    !("checked" in div),
                    "checked" in input,
                    "style" in svg,
                    typeof fragment.querySelector === "function"
                ].join("|");
            })()"#,
        );
        assert_eq!(
            actual,
            "true|true|true|true|true|true|true|true|true|true|true|true|true|true|true|true|true|true|true"
        );
    }

    #[test]
    fn character_data_preserves_utf16_and_updates_ranges() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const text = document.createTextNode("abcdef");
                document.body.appendChild(text);
                const range = document.createRange();
                range.setStart(text, 2);
                range.setEnd(text, 5);
                text.replaceData(1, 3, "XY");
                const offsets = [range.startOffset, range.endOffset];
                text.data = "\uD83C--start";
                text.appendData("--\uDF20");
                const clone = text.cloneNode();
                return [
                    text.data.charCodeAt(0) === 0xD83C,
                    text.data.charCodeAt(text.length - 1) === 0xDF20,
                    clone.data === text.data,
                    offsets[0], offsets[1]
                ].join("|");
            })()"#,
        );
        assert_eq!(actual, "true|true|true|1|4");
    }

    #[test]
    fn character_data_review_regressions_preserve_ranges_pi_and_deep_clones() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const text = document.createTextNode("abcd");
                document.body.appendChild(text);
                const valueRange = document.createRange();
                valueRange.setStart(text, 1);
                valueRange.setEnd(text, 3);
                text.nodeValue = "xy";

                const pi = document.createProcessingInstruction("target", "abcdef");
                document.body.appendChild(pi);
                const piRange = document.createRange();
                piRange.setStart(pi, 1);
                piRange.setEnd(pi, 4);
                const clonedPi = piRange.cloneContents().firstChild;
                const extractedPi = piRange.extractContents().firstChild;

                const parent = document.createElement("div");
                const surrogate = parent.appendChild(document.createTextNode(""));
                surrogate.data = "\uD83Cmiddle\uDF20";
                const deepClone = parent.cloneNode(true).firstChild;
                return [
                    valueRange.startOffset, valueRange.endOffset,
                    clonedPi.nodeType, clonedPi.target, clonedPi.data,
                    extractedPi.data, pi.data,
                    deepClone.data.charCodeAt(0) === 0xD83C,
                    deepClone.data.charCodeAt(deepClone.length - 1) === 0xDF20
                ].join("|");
            })()"#,
        );
        assert_eq!(actual, "0|0|7|target|bcd|bcd|aef|true|true");
    }

    #[test]
    fn normalize_merges_text_and_preserves_range_boundaries() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const div = document.createElement("div");
                const first = div.appendChild(document.createTextNode("ab"));
                const second = div.appendChild(document.createTextNode("cd"));
                div.appendChild(document.createTextNode(""));
                document.body.appendChild(div);
                const range = document.createRange();
                range.setStart(second, 1);
                range.setEnd(second, 2);
                div.normalize();
                const xml = new DOMParser().parseFromString("<root/>", "text/xml");
                const cdata = xml.documentElement.appendChild(xml.createCDATASection(""));
                xml.documentElement.normalize();
                const directClone = cdata.cloneNode();
                const deepClone = xml.documentElement.cloneNode(true).lastChild;
                let invalidCdata, missingArgs, unsupportedMime;
                try { xml.createCDATASection("bad]]>data"); } catch (error) { invalidCdata = error.name; }
                try { new DOMParser().parseFromString("<x/>"); } catch (error) { missingArgs = error.name; }
                try { new DOMParser().parseFromString("<x/>", "text/plain"); } catch (error) { unsupportedMime = error.name; }
                const html = new DOMParser().parseFromString("<p>parsed</p>", "text/html");
                return [
                    div.childNodes.length, first.data,
                    range.startContainer === first, range.startOffset,
                    range.endContainer === first, range.endOffset,
                    document.textContent === null,
                    cdata.nodeType, cdata instanceof Text,
                    xml.documentElement.lastChild === cdata,
                    invalidCdata, missingArgs, unsupportedMime,
                    directClone.nodeType, directClone instanceof CDATASection,
                    deepClone.nodeType, deepClone instanceof CDATASection,
                    html !== document, html.body.textContent
                ].join("|");
            })()"#,
        );
        assert_eq!(
            actual,
            "1|abcd|true|3|true|4|true|4|true|true|InvalidCharacterError|TypeError|TypeError|4|true|4|true|true|parsed"
        );
    }

    #[test]
    fn mutation_observer_validates_lifecycle_and_overlapping_registrations() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const parent = document.createElement("div");
                const child = parent.appendChild(document.createElement("span"));
                const observer = new MutationObserver(() => {});
                let nullOptions;
                try { observer.observe(child, null); } catch (error) { nullOptions = error.name; }

                child.setAttribute("data-x", "before");
                observer.observe(parent, { attributes: true, subtree: true });
                observer.observe(child, { attributes: true, attributeOldValue: true });
                child.setAttribute("data-x", "after");
                const overlapping = observer.takeRecords();

                observer.disconnect();
                child.setAttribute("data-x", "disconnected");
                const disconnected = observer.takeRecords().length;
                observer.observe(child, { attributes: true });
                child.setAttribute("data-x", "reconnected");
                const reconnected = observer.takeRecords();
                return [
                    nullOptions, overlapping.length, overlapping[0].oldValue,
                    disconnected, reconnected.length, reconnected[0].oldValue
                ].join("|");
            })()"#,
        );
        assert_eq!(actual, "TypeError|1|before|0|1|");
    }

    #[test]
    fn mutation_dom_operations_validate_before_mutating() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = eval_str(
            &mut runtime,
            r#"(() => {
                const parent = document.createElement("div");
                const other = document.createElement("div");
                const reference = other.appendChild(document.createElement("span"));
                const observer = new MutationObserver(() => {});
                observer.observe(parent, { childList: true });
                let insertError;
                try { parent.insertBefore(document.createElement("b"), reference); }
                catch (error) { insertError = error.name; }

                const element = document.createElement("div");
                element.setAttributeNS(null, "plain", "one");
                const nullNamespaceValue = element.getAttributeNS(null, "plain");
                element.removeAttributeNS(null, "plain");

                const ns = "https://example.test/ns";
                element.setAttributeNS(ns, "old:item", "first");
                element.setAttributeNS(ns, "new:item", "second");
                const oldPrefixRemoved = !element.hasAttribute("old:item");
                const namespacedValue = element.getAttributeNS(ns, "item");

                let namespaceError;
                try { element.setAttributeNS(null, "prefix:item", "bad"); }
                catch (error) { namespaceError = error.name; }
                let namedItemError;
                try { element.attributes.removeNamedItem("missing"); }
                catch (error) { namedItemError = error.name; }

                return [
                    insertError, parent.childNodes.length, observer.takeRecords().length,
                    nullNamespaceValue, !element.hasAttribute("plain"),
                    oldPrefixRemoved, namespacedValue, namespaceError, namedItemError
                ].join("|");
            })()"#,
        );
        assert_eq!(
            actual,
            "NotFoundError|0|0|one|true|true|second|NamespaceError|NotFoundError"
        );
    }

    #[test]
    fn element_has_no_character_data_property() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        // .data must not be defined on Element nodes (it is a CharacterData API).
        let is_undefined = runtime
            .eval("document.createElement('div').data === undefined")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            is_undefined,
            "Element nodes must not expose CharacterData.data"
        );
    }

    #[test]
    fn document_default_view_is_global() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let same = runtime
            .eval(
                "document.defaultView === globalThis &&
                 document.defaultView === window &&
                 parent === window && top === window",
            )
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            same,
            "document.defaultView and top-level browsing-context aliases must be global"
        );
    }

    #[test]
    fn window_global_class_recognizes_only_window_objects() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><div id="d"></div><iframe id="f"></iframe></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let actual = runtime
            .eval(
                "[
                    typeof Window === 'function',
                    globalThis.window === globalThis,
                    globalThis instanceof Window,
                    window instanceof Window,
                    document.defaultView instanceof Window,
                    document.getElementById('f').contentWindow instanceof Window,
                    document.getElementById('f').contentDocument.defaultView instanceof Window,
                    document instanceof Window,
                    document.getElementById('d') instanceof Window,
                    ({w: window}).w instanceof Window ? 'win' : 'other'
                ].join('|')",
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            actual, "true|true|true|true|true|true|true|false|false|win",
            "Window must recognize the main global and iframe facades, but no DOM nodes",
        );
    }

    #[test]
    fn node_type_constants_are_exposed() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        // On the Node constructor.
        assert_eq!(
            runtime
                .eval("Node.ELEMENT_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            1.0
        );
        assert_eq!(
            runtime.eval("Node.TEXT_NODE").unwrap().as_number().unwrap(),
            3.0
        );
        assert_eq!(
            runtime
                .eval("Node.COMMENT_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            8.0
        );
        assert_eq!(
            runtime
                .eval("Node.DOCUMENT_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            9.0
        );
        assert_eq!(
            runtime
                .eval("Node.DOCUMENT_TYPE_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            10.0
        );
        assert_eq!(
            runtime
                .eval("Node.DOCUMENT_FRAGMENT_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            11.0
        );
        assert_eq!(
            runtime
                .eval("Node.NOTATION_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            12.0
        );
        // On instances via the prototype (as Acid3 test 19 checks).
        assert_eq!(
            runtime
                .eval("document.DOCUMENT_FRAGMENT_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            11.0,
            "document must inherit DOCUMENT_FRAGMENT_NODE"
        );
        assert_eq!(
            runtime
                .eval("document.createTextNode('').ELEMENT_NODE")
                .unwrap()
                .as_number()
                .unwrap(),
            1.0,
            "text node must inherit ELEMENT_NODE"
        );
    }

    #[test]
    fn local_name_is_owned_by_elements_only() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let el = runtime
            .eval("document.createElement('DIV').localName")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            el, "div",
            "element localName must be the lower-cased tag name"
        );
        // localName is declared by Element, so non-element nodes do not expose it.
        assert!(
            runtime
                .eval("document.createTextNode('x').localName === undefined")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(
            runtime
                .eval("document.createComment('x').localName === undefined")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(
            runtime
                .eval("document.localName === undefined")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    // --- 016-5: data: URI scripts ---

    #[test]
    fn fetch_script_source_decodes_acid3_data_uri_vectors() {
        // The five Acid3 (test 97) vectors and the JS source each must yield.
        let cases = [
            ("data:text/javascript,d1%20%3D%20'one'%3B", "d1 = 'one';"),
            (
                "data:text/javascript;base64,ZDIgPSAndHdvJzs%3D",
                "d2 = 'two';",
            ),
            (
                "data:text/javascript;base64,%5a%44%4d%67%50%53%41%6e%64%47%68%79%5a%57%55%6e%4f%77%3D%3D",
                "d3 = 'three';",
            ),
            (
                "data:text/javascript;base64,%20ZD%20Qg%0D%0APS%20An%20Zm91cic%0D%0A%207%20",
                "d4 = 'four';",
            ),
            (
                "data:text/javascript,d5%20%3D%20'five%5Cu0027s'%3B",
                "d5 = 'five\\u0027s';",
            ),
        ];
        for (uri, expected) in cases {
            let source = fetch_script_source(uri, None)
                .unwrap_or_else(|| panic!("data: URI should decode: {uri}"));
            assert_eq!(source, expected, "wrong decoded source for {uri}");
        }
    }

    #[test]
    fn fetch_script_source_rejects_non_javascript_data_uri() {
        // A data: URI whose media type is not JavaScript is not executed.
        assert!(
            fetch_script_source("data:text/plain,alert(1)", None).is_none(),
            "non-JavaScript data: media type must not be treated as a script"
        );
    }

    #[test]
    fn data_uri_scripts_define_globals_end_to_end() {
        // Executing the five decoded sources must define d1..d5 as Acid3 expects.
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let vectors = [
            "data:text/javascript,d1%20%3D%20'one'%3B",
            "data:text/javascript;base64,ZDIgPSAndHdvJzs%3D",
            "data:text/javascript;base64,%5a%44%4d%67%50%53%41%6e%64%47%68%79%5a%57%55%6e%4f%77%3D%3D",
            "data:text/javascript;base64,%20ZD%20Qg%0D%0APS%20An%20Zm91cic%0D%0A%207%20",
            "data:text/javascript,d5%20%3D%20'five%5Cu0027s'%3B",
        ];
        for uri in vectors {
            let source = fetch_script_source(uri, None).unwrap();
            runtime.eval(&source).unwrap();
        }
        let combined = runtime
            .eval("[d1, d2, d3, d4, d5].join('|')")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(combined, "one|two|three|four|five's");
    }

    #[test]
    fn intersection_observer_fires_callback_on_observe() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            var observed = false;
            var intersecting = false;
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => {
                observed = true;
                intersecting = entries[0].isIntersecting;
            });
            observer.observe(el);
        "#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        let observed = runtime.eval("observed").unwrap().as_boolean().unwrap();
        assert!(
            observed,
            "IntersectionObserver callback should fire after observe()"
        );

        let intersecting = runtime.eval("intersecting").unwrap().as_boolean().unwrap();
        assert!(
            intersecting,
            "entry.isIntersecting should be true in headless mode"
        );
    }

    #[test]
    fn intersection_observer_reobserve_before_delivery_is_coalesced() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            var count = 0;
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => { count++; });
            observer.observe(el);
            observer.unobserve(el);
            observer.observe(el);
        "#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        let count = runtime
            .eval("count")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(
            count, 1.0,
            "changes before the observer checkpoint should be coalesced"
        );
    }

    #[test]
    fn intersection_observer_disconnect_clears_targets() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            var count = 0;
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => { count++; });
            observer.observe(el);
            observer.disconnect();
            observer.observe(el);
        "#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        let count = runtime
            .eval("count")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(count, 1.0, "disconnect should cancel the pending observation");
    }

    #[test]
    fn resize_observer_reports_real_boxes_and_size_changes() {
        let mut runtime = runtime_from_html(
            r#"<div id="box" style="width:100px;height:40px;padding:5px;border:2px solid black"></div>"#,
        );
        runtime
            .eval(
                r#"globalThis.resizeEntries = [];
                   globalThis.resizeObserver = new ResizeObserver(entries => {
                     resizeEntries.push({
                       contentWidth: entries[0].contentRect.width,
                       contentHeight: entries[0].contentRect.height,
                       borderWidth: entries[0].borderBoxSize[0].inlineSize,
                       borderHeight: entries[0].borderBoxSize[0].blockSize,
                     });
                   });
                   resizeObserver.observe(document.getElementById('box'));"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        assert!(runtime
            .eval("resizeEntries.length === 1 && resizeEntries[0].contentWidth === 100 && resizeEntries[0].contentHeight === 40 && resizeEntries[0].borderWidth === 114 && resizeEntries[0].borderHeight === 54")
            .unwrap()
            .as_boolean()
            .unwrap());

        runtime
            .eval("document.getElementById('box').style.width = '120px'")
            .unwrap();
        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval("resizeEntries.length === 2 && resizeEntries[1].contentWidth === 120")
            .unwrap()
            .as_boolean()
            .unwrap());

        runtime
            .eval("document.getElementById('box').style.width = '120px'")
            .unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(runtime.eval("resizeEntries.length").unwrap().as_number(), Some(2.0));
    }

    #[test]
    fn intersection_observer_computes_partial_and_outside_geometry() {
        let mut runtime = runtime_from_html(
            r#"<div id="box" style="position:absolute;left:900px;top:0;width:200px;height:100px"></div>"#,
        );
        runtime.set_viewport(1000.0, 500.0);
        runtime
            .eval(
                r#"globalThis.intersections = [];
                   globalThis.geometryObserver = new IntersectionObserver(entries => {
                     const entry = entries[0];
                     intersections.push({
                       intersecting: entry.isIntersecting,
                       ratio: entry.intersectionRatio,
                       targetWidth: entry.boundingClientRect.width,
                       intersectionWidth: entry.intersectionRect.width,
                       rootWidth: entry.rootBounds.width,
                     });
                   }, { threshold: [0, 0.5, 1] });
                   geometryObserver.observe(document.getElementById('box'));"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        assert!(runtime
            .eval("intersections.length === 1 && intersections[0].intersecting && intersections[0].ratio === 0.5 && intersections[0].targetWidth === 200 && intersections[0].intersectionWidth === 100 && intersections[0].rootWidth === 1000")
            .unwrap()
            .as_boolean()
            .unwrap());

        runtime.set_viewport(800.0, 500.0);
        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval("intersections.length === 2 && !intersections[1].intersecting && intersections[1].ratio === 0 && intersections[1].intersectionWidth === 0")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn intersection_observer_take_records_drains_before_callback() {
        let mut runtime = runtime_from_html(r#"<div id="box" style="width:10px;height:10px"></div>"#);
        runtime
            .eval(
                r#"globalThis.intersectionCallbacks = 0;
                   globalThis.takenIntersections = [];
                   const observer = new IntersectionObserver(() => intersectionCallbacks++);
                   observer.observe(document.getElementById('box'));
                   Promise.resolve().then(() => { takenIntersections = observer.takeRecords(); });"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval("takenIntersections.length === 1 && intersectionCallbacks === 0")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn intersection_observer_normalizes_options_and_rejects_invalid_values() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const observer = new IntersectionObserver(() => {}, {
                    rootMargin: '10px 5%', threshold: [1, 0.5, 0, 0.5]
                  });
                  let badMargin = false;
                  let badThreshold = false;
                  try { new IntersectionObserver(() => {}, { rootMargin: '1em' }); }
                  catch (error) { badMargin = error.name === 'SyntaxError'; }
                  try { new IntersectionObserver(() => {}, { threshold: 2 }); }
                  catch (error) { badThreshold = error instanceof RangeError; }
                  return observer.rootMargin === '10px 5% 10px 5%' &&
                    observer.thresholds.join(',') === '0,0.5,1' && badMargin && badThreshold;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn intersection_observer_classlist_add_pattern() {
        // Simulate the common pattern: IO + classList.add('on') for fade-in
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        div.set_attribute("class", "fade");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => {
                entries.forEach(entry => {
                    if (entry.isIntersecting) {
                        entry.target.classList.add("on");
                    }
                });
            });
            observer.observe(el);
        "#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        let has_on = runtime
            .eval("document.querySelector('div').classList.contains('on')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(has_on, "IO should add 'on' class via classList.add");

        // Verify the DOM attribute
        let class_attr = div
            .attributes()
            .unwrap()
            .get("class")
            .cloned()
            .unwrap_or_default();
        assert!(
            class_attr.contains("on"),
            "DOM class attr should contain 'on': {class_attr}"
        );
    }

    #[test]
    fn text_content_getter_and_setter() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let text = NodeHandle::text("Hello world");
        div.append_child(text);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval("document.querySelector('div').textContent")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "Hello world");

        runtime
            .eval("document.querySelector('div').textContent = 'Changed'")
            .unwrap();
        let result = runtime
            .eval("document.querySelector('div').textContent")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "Changed");
    }

    #[test]
    fn cssom_lists_style_sheets_and_rules_in_tree_order() {
        let doc = crate::html::TreeBuilder::parse("<html><head><style>p { color: red; } span { display: block; }</style><style>#x { width: 7px; }</style></head><body><p id='x'></p></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(
            runtime
                .eval("document.styleSheets === document.styleSheets")
                .unwrap()
                .as_boolean()
                .unwrap(),
            "StyleSheetList identity must be stable"
        );
        assert_eq!(eval_num(&mut runtime, "document.styleSheets.length"), 2.0);
        assert_eq!(
            eval_num(&mut runtime, "document.styleSheets[0].cssRules.length"),
            2.0
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[0].selectorText"
            ),
            "p"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[1].selectorText"
            ),
            "span"
        );
        assert!(
            runtime
                .eval("document.styleSheets[0].ownerNode === document.querySelector('style')")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(
            runtime
                .eval("document.querySelector('style').sheet === document.styleSheets[0]")
                .unwrap()
                .as_boolean()
                .unwrap(),
            "HTMLStyleElement.sheet must expose the document's live stylesheet"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[0].style.color"
            ),
            "red"
        );
    }

    #[test]
    fn css_supports_rules_expose_conditions_and_nested_rules() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><head><style>@supports (display: grid) { main { display: grid; } } @supports/* comment */(display: block) { section { display: block; } }</style></head><body><main></main></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                    const rule = document.styleSheets[0].cssRules[0];
                    return rule instanceof CSSSupportsRule &&
                        rule.conditionText === "(display: grid)" &&
                        rule.matches === true &&
                        rule.cssRules.length === 1 &&
                        rule.cssRules[0].selectorText === "main" &&
                        document.styleSheets[0].cssRules[1] instanceof CSSSupportsRule &&
                        document.styleSheets[0].cssRules[1].matches === true;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn css_scope_rules_expose_boundaries_and_nested_rules() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><head><style>@scope { p { color: black; } } @scope (.card) to (.stop) { p { color: red; } } @scope/* comment */(.commented) { span { color: blue; } } @scope to(.limit) { em { color: green; } }</style></head><body></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                    const implicit = document.styleSheets[0].cssRules[0];
                    const bounded = document.styleSheets[0].cssRules[1];
                    const commented = document.styleSheets[0].cssRules[2];
                    const limitOnly = document.styleSheets[0].cssRules[3];
                    return implicit instanceof CSSScopeRule &&
                        implicit instanceof CSSGroupingRule &&
                        !(implicit instanceof CSSConditionRule) &&
                        implicit.start === null && implicit.end === null &&
                        bounded instanceof CSSScopeRule &&
                        bounded.start === ".card" && bounded.end === ".stop" &&
                        bounded.cssRules.length === 1 &&
                        bounded.cssRules[0].selectorText === "p" &&
                        commented instanceof CSSScopeRule &&
                        commented.start === ".commented" &&
                        limitOnly.start === null && limitOnly.end === ".limit";
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn css_container_rules_expose_name_query_and_nested_rules() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><head><style>@container card (width >= 400px) { main { width: 20px; } } @container/* c */(inline-size > 10px) { p { color: red; } }</style></head><body></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                    const named = document.styleSheets[0].cssRules[0];
                    const unnamed = document.styleSheets[0].cssRules[1];
                    return named instanceof CSSContainerRule &&
                        named instanceof CSSConditionRule &&
                        named.containerName === "card" &&
                        named.containerQuery === "(width >= 400px)" &&
                        named.conditionText === "card (width >= 400px)" &&
                        named.cssRules.length === 1 &&
                        named.cssRules[0].selectorText === "main" &&
                        unnamed instanceof CSSContainerRule &&
                        unnamed.containerName === "" &&
                        unnamed.containerQuery === "(inline-size > 10px)";
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn size_container_query_controls_javascript_computed_style() {
        let doc = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #shell { width: 500px; container-type: inline-size; container-name: shell; }
                #item { width: 10px; }
                @container shell (inline-size >= 400px) { #item { width: 75px; } }
            </style></head><body><section id="shell"><article id="item"></article></section></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('item')).width"
            ),
            "75px"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('shell')).containerType"
            ),
            "inline-size"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { document.getElementById('shell').style.width = '300px'; return getComputedStyle(document.getElementById('item')).width; })()"
            ),
            "10px",
            "a forced layout after container geometry changes must re-evaluate the query"
        );
    }

    #[test]
    fn size_container_query_uses_iframe_document_layout() {
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="frame" width="500" height="200"></iframe></body></html>"#,
        );
        runtime
            .eval(
                r#"const childDocument = document.getElementById('frame').contentDocument;
                   const style = childDocument.createElement('style');
                   style.textContent = '#shell { width: 420px; container-type: inline-size; } #item { width: 2px; } @container (width >= 400px) { #item { width: 42px; } }';
                   childDocument.head.appendChild(style);
                   const shell = childDocument.createElement('section'); shell.id = 'shell';
                   const childItem = childDocument.createElement('div'); childItem.id = 'item';
                   shell.appendChild(childItem); childDocument.body.appendChild(shell);"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(childItem).width"),
            "42px"
        );
    }

    #[test]
    fn container_properties_resolve_css_wide_keywords_to_initial_values() {
        let doc = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { container-type: inline-size; container-type: initial;
                          container-name: card; container-name: unset; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target')).containerType"
            ),
            "normal"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target')).containerName"
            ),
            "none"
        );
    }

    #[test]
    fn parsed_ids_are_window_named_properties_but_script_globals_can_override() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><body><style id='theme'></style><div id='log'></div></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.execute_document_scripts(None);
        assert!(runtime
            .eval("theme === document.getElementById('theme')")
            .unwrap()
            .as_boolean()
            .unwrap());
        assert_eq!(
            eval_num(&mut runtime, "var log = []; log.push(1); log.length"),
            1.0
        );
    }

    #[test]
    fn scope_rules_control_computed_style_in_javascript() {
        let doc = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                p { color: black; width: 1px; }
                @scope (.card) to (.stop) {
                    p { color: red; }
                    :scope > p { width: 9px; }
                }
            </style></head><body>
                <section class="card"><p id="inside"></p><div class="stop"><p id="limited"></p></div></section>
            </body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('inside')).color"
            ),
            "rgb(255, 0, 0)"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('inside')).width"
            ),
            "9px"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('limited')).color"
            ),
            "rgb(0, 0, 0)"
        );
    }

    #[test]
    fn implicit_scope_uses_inline_style_owner_parent() {
        let doc = crate::html::TreeBuilder::parse(
            r#"<html><body>
                <section id="root"><style>@scope { .item { z-index: 7; } :scope { width: 11px; } }</style><div id="inside" class="item"></div></section>
                <div id="outside" class="item"></div>
            </body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('inside')).zIndex"
            ),
            "7"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('outside')).zIndex"
            ),
            ""
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('root')).width"
            ),
            "11px"
        );
    }

    #[test]
    fn supports_conditions_control_the_author_cascade() {
        let doc = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { width: 1px; height: 2px; color: red; }
                @supports (display: grid) { #target { width: 20px; } }
                @supports (future-property: value) { #target { height: 30px; } }
                @supports not (future-property: value) { #target { color: green; } }
                @media all { @supports (display: block) { #target { height: 40px; } } }
                @supports (width: calc(-1px)) { #target { width: calc(-1px); } }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target')).width"
            ),
            "0px"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target')).height"
            ),
            "40px"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target')).color"
            ),
            "rgb(0, 128, 0)"
        );
    }

    #[test]
    fn dynamic_style_element_sheet_supports_rule_insertion() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><head></head><body><div id='target'></div></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r##"
                const style = document.createElement("style");
                style.id = "runtime-styles";
                document.head.insertBefore(style, document.head.firstChild);
                style.sheet.insertRule("#target { display: none; }", 0);
                "##,
            )
            .unwrap();

        assert_eq!(eval_num(&mut runtime, "document.styleSheets.length"), 1.0);
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.getElementById('target')).display"),
            "none"
        );
    }

    #[test]
    fn cssom_insert_and_delete_are_live_and_restyle_synchronously() {
        let doc = crate::html::TreeBuilder::parse("<html><head><style>img { height: 10px; } img { height: 20px; }</style></head><body><img></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval("globalThis.rules = document.styleSheets[0].cssRules; document.styleSheets[0].insertRule('img { height: 40px; }', 2)").unwrap();
        assert_eq!(
            eval_num(&mut runtime, "rules.length"),
            3.0,
            "retained CSSRuleList must be live"
        );
        assert_eq!(eval_num(&mut runtime, "document.images[0].height"), 40.0);
        runtime
            .eval("document.styleSheets[0].deleteRule(2)")
            .unwrap();
        assert_eq!(eval_num(&mut runtime, "rules.length"), 2.0);
        assert_eq!(eval_num(&mut runtime, "document.images[0].height"), 20.0);
    }

    #[test]
    fn cssom_selector_text_setter_is_live_and_restyles_synchronously() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><head><style>.before { color: red; }</style></head><body><div class='after'></div></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval("document.styleSheets[0].cssRules[0].selectorText = ':is(.after, .missing)'")
            .unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[0].selectorText"
            ),
            ":is(.after, .missing)"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.querySelector('div')).color"),
            "rgb(255, 0, 0)"
        );

        runtime
            .eval("document.styleSheets[0].cssRules[0].selectorText = 'div,'")
            .unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[0].selectorText"
            ),
            ":is(.after, .missing)",
            "an invalid selector must leave selectorText unchanged"
        );
    }

    #[test]
    fn retained_css_rule_tracks_insertions_and_deletions_before_it() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><head><style>.first { width: 1px; } .second { width: 2px; }</style></head><body><div class='target'></div></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"
                globalThis.retainedRule = document.styleSheets[0].cssRules[1];
                document.styleSheets[0].insertRule('.inserted { width: 3px; }', 0);
                retainedRule.selectorText = '.target';
                "#,
            )
            .unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[2].selectorText"
            ),
            ".target"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.querySelector('div')).width"),
            "2px"
        );

        runtime
            .eval(
                "document.styleSheets[0].deleteRule(0); retainedRule.selectorText = '.retargeted'",
            )
            .unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[1].selectorText"
            ),
            ".retargeted"
        );
    }

    #[test]
    fn cssom_insert_and_delete_report_dom_exceptions() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><head><style>p { color: red; }</style></head><body></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(runtime.eval("(() => { try { document.styleSheets[0].insertRule('not css', 0); return false; } catch (e) { return e.name === 'SyntaxError' && e instanceof DOMException; } })()").unwrap().as_boolean().unwrap(), "invalid insertRule input must throw a SyntaxError DOMException");
        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { try { document.styleSheets[0].insertRule('p { color: blue; }', 2); return ''; } catch (e) { return e.name + ':' + e.code; } })()"
            ),
            "IndexSizeError:1"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { try { document.styleSheets[0].deleteRule(1); return ''; } catch (e) { return e.name + ':' + e.code; } })()"
            ),
            "IndexSizeError:1"
        );
        assert!(runtime.eval("(() => { document.styleSheets[0].ownerNode.textContent = 'not css'; try { document.styleSheets[0].insertRule('p { color: blue; }', 0); return false; } catch (e) { return e.name === 'SyntaxError' && e instanceof DOMException; } })()").unwrap().as_boolean().unwrap(), "stylesheet enumeration errors must be wrapped as SyntaxError DOMExceptions");
    }

    #[test]
    fn cssom_braceless_at_rule_has_healthy_rule_views() {
        let doc = crate::html::TreeBuilder::parse("<html><head><style>@charset \"UTF-8\"; p { color: red; }</style></head><body></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_num(&mut runtime, "document.styleSheets[0].cssRules.length"),
            2.0
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[0].selectorText"
            ),
            ""
        );
        assert_eq!(
            eval_str(&mut runtime, "document.styleSheets[0].cssRules[0].cssText"),
            "@charset \"UTF-8\";"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[0].style.cssText"
            ),
            ""
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.styleSheets[0].cssRules[1].selectorText"
            ),
            "p"
        );
        assert_eq!(
            eval_str(&mut runtime, "document.styleSheets[0].cssRules[1].cssText"),
            "p { color: red; }"
        );
    }

    #[test]
    fn child_nodes_returns_array() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let span1 = NodeHandle::element("span");
        let span2 = NodeHandle::element("span");
        div.append_child(span1);
        div.append_child(span2);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let len = runtime
            .eval("document.querySelector('div').childNodes.length")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(len, 2.0);
    }

    #[test]
    fn document_body_and_create_text_node() {
        use crate::html::TreeBuilder;
        let html = "<html><body></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let has_body = runtime
            .eval("document.body !== null")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(has_body, "document.body should not be null");

        runtime
            .eval(
                r#"
            const t = document.createTextNode("Hello");
            document.body.appendChild(t);
        "#,
            )
            .unwrap();

        let result = runtime
            .eval("document.body.textContent")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn query_selector_all_returns_multiple() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let s1 = NodeHandle::element("span");
        let s2 = NodeHandle::element("span");
        let p = NodeHandle::element("p");
        div.append_child(s1);
        div.append_child(p);
        div.append_child(s2);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let len = runtime
            .eval("document.querySelectorAll('span').length")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(len, 2.0);
    }

    #[test]
    fn node_lists_expose_the_dom_collection_interface() {
        let document = sample_document();
        let mut runtime = JsRuntime::with_document(document).unwrap();

        let result = runtime
            .eval(
                r#"(() => {
                    const childNodes = document.documentElement.childNodes;
                    const matches = document.querySelectorAll("body, main");
                    let visited = "";
                    matches.forEach(node => visited += node.tagName + ",");
                    return [
                        typeof NodeList,
                        childNodes instanceof NodeList,
                        matches instanceof NodeList,
                        matches.item(0).tagName,
                        matches.item(99) === null,
                        [...matches].length,
                        visited,
                        Object.prototype.toString.call(matches),
                    ].join("|");
                })()"#,
            )
            .unwrap();

        assert_eq!(
            result.as_string().unwrap().to_std_string_escaped(),
            "function|true|true|BODY|true|2|BODY,MAIN,|[object NodeList]"
        );
    }

    #[test]
    fn link_rel_list_supports_modulepreload_detection() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let actual = runtime
            .eval(
                r#"(() => {
                    const link = document.createElement("link");
                    link.rel = "stylesheet preload";
                    return [
                        link instanceof HTMLLinkElement,
                        link.rel,
                        link.relList.length,
                        link.relList.contains("preload"),
                        link.relList.item(0),
                        link.relList.supports("modulepreload")
                    ].join("|");
                })()"#,
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(actual, "true|stylesheet preload|2|true|stylesheet|true");
    }

    #[test]
    fn query_selector_apis_use_full_css_matcher_and_strict_parser() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><main id="scope"><p id="first" class="a" data-kind="x"></p><section><p id="second" class="a"></p></section></main></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval(
                r##"
                (() => {
                  const scope = document.querySelector("#scope");
                  const first = scope.querySelector("section > p.a, p[data-kind=x]");
                  const all = scope.querySelectorAll("p.a, #first");
                  const snapshot = all.length;
                  scope.appendChild(document.createElement("p"));
                  let syntax = false;
                  let atomic = false;
                  try { scope.querySelector("p,"); } catch (e) {
                    syntax = e instanceof DOMException && e.name === "SyntaxError";
                  }
                  try { scope.querySelectorAll("p, :unknown"); } catch (e) {
                    atomic = e instanceof DOMException && e.name === "SyntaxError";
                  }
                  return [first.id, all.length, snapshot, scope.querySelector("#scope") === null, syntax, atomic, scope.querySelector("::before") === null].join("|");
                })()
                "##,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "first|2|2|true|true|true|true");
    }

    #[test]
    fn selector_list_pseudos_work_in_dom_queries_and_cascade() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><head><style>
                :is(.missing, #target) { width: 20px; }
                .target { width: 10px; color: red; height: 10px; }
                :where(#target) { color: blue; }
                :not(.missing, #other) { height: 30px; }
            </style></head><body><main><div id="target" class="target"></div><p class="other"></p></main></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { const target = document.querySelector('main > :is(.target, p)'); return [target.id, document.querySelector('main').querySelectorAll(':not(.target)').length, document.querySelector(':is(:unknown, #target)').id].join('|'); })()"
            ),
            "target|1|target"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.getElementById('target')).width"),
            "20px"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.getElementById('target')).color"),
            "rgb(255, 0, 0)"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.getElementById('target')).height"),
            "30px"
        );
    }

    #[test]
    fn has_relative_selectors_work_in_dom_queries_and_cascade() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><head><style>
                main { width: 10px; }
                main:has(> #target) { width: 20px; }
                main:has(.nested + #target) { height: 30px; }
                #dynamic:has(> .added) { width: 40px; }
            </style></head><body>
                <main id="scope"><div class="nested"></div><div id="target"></div></main>
                <main id="dynamic"></main>
            </body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { const scope = document.getElementById('scope'); return [document.querySelector('main:has(> #target)').id, scope.matches(':has(.nested + #target)'), scope.querySelectorAll(':has(+ #target)').length].join('|'); })()"
            ),
            "scope|true|1"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.getElementById('scope')).width"),
            "20px"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(document.getElementById('scope')).height"),
            "30px"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { const dynamic = document.getElementById('dynamic'); const before = getComputedStyle(dynamic).width; const child = document.createElement('span'); child.className = 'added'; dynamic.appendChild(child); return before + '|' + getComputedStyle(dynamic).width; })()"
            ),
            "10px|40px"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { const subject = document.createElement('section'); subject.innerHTML = '<div><span></span></div>'; return [subject.matches(':has(span)'), subject.matches(':has(> div)'), subject.matches(':has(> span)')].join('|'); })()"
            ),
            "true|true|false"
        );
    }

    #[test]
    fn get_element_by_id_does_not_parse_id_as_a_selector() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><div id="plain"></div></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(
            runtime
                .eval("document.getElementById('plain\\0suffix') === null")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(
            runtime
                .eval("document.getElementById('plain').id === 'plain'")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn checkedness_is_limited_to_checkable_inputs() {
        use crate::html::TreeBuilder;
        // A `checked` attribute on a text input or a non-input element must
        // not surface through the `.checked` IDL property (nor `:checked`);
        // checkedness only applies to checkbox/radio inputs.
        let doc = TreeBuilder::parse(
            r#"<html><body><input id="text" type="text" checked><div id="div" checked></div><input id="box" type="checkbox" checked></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval(
                r#"
                (() => {
                  const text = document.getElementById("text");
                  const div = document.getElementById("div");
                  const box = document.getElementById("box");
                  return [text.checked, div.checked === undefined, box.checked,
                          document.querySelector(":checked") === box].join("|");
                })()
                "#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "false|true|true|true");
    }

    #[test]
    fn click_dispatches_on_non_disableable_elements_with_disabled_attribute() {
        use crate::html::TreeBuilder;
        // `disabled` only suppresses activation on form controls: a
        // `<div disabled>` still dispatches click, a `<button disabled>`
        // does not.
        let doc = TreeBuilder::parse(
            r#"<html><body><div id="div" disabled></div><button id="btn" disabled></button></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval(
                r#"
                (() => {
                  const div = document.getElementById("div");
                  const btn = document.getElementById("btn");
                  let divClicked = false;
                  let btnClicked = false;
                  div.addEventListener("click", () => { divClicked = true; });
                  btn.addEventListener("click", () => { btnClicked = true; });
                  div.click();
                  btn.click();
                  return [divClicked, btnClicked].join("|");
                })()
                "#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "true|false");
    }

    #[test]
    fn disabled_fieldset_blocks_focus_and_activation_except_in_first_legend() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><fieldset disabled><input id="direct" type="checkbox"><legend><input id="first" type="checkbox"></legend><legend><input id="second" type="checkbox"></legend><fieldset><input id="nested" type="checkbox"></fieldset></fieldset></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval(
                r#"
                (() => {
                  const direct = document.getElementById("direct");
                  const first = document.getElementById("first");
                  const second = document.getElementById("second");
                  const nested = document.getElementById("nested");
                  let directClicks = 0;
                  direct.addEventListener("click", () => directClicks++);
                  direct.focus();
                  const directFocusBlocked = document.activeElement === document.body;
                  direct.click();
                  first.focus();
                  const firstFocusAllowed = document.activeElement === first;
                  first.click();
                  second.click();
                  nested.click();
                  return [direct.matches(":disabled"), !direct.matches(":enabled"),
                    first.matches(":enabled"), second.matches(":disabled"),
                    nested.matches(":disabled"), directFocusBlocked, firstFocusAllowed,
                    directClicks === 0, !direct.checked, first.checked,
                    !second.checked, !nested.checked, direct.disabled === false].every(Boolean);
                })()
                "#,
            )
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(result);
    }

    #[test]
    fn form_checkedness_is_live_dirty_and_radio_exclusive() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><input id="box" type="checkbox" checked><input id="r1" type="radio" name="g"><input id="r2" type="radio" name="g" checked><input id="off" type="checkbox" disabled></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval(
                r#"
                (() => {
                  const box = document.getElementById("box");
                  const r1 = document.getElementById("r1");
                  const r2 = document.getElementById("r2");
                  const off = document.getElementById("off");
                  box.click();
                  box.setAttribute("checked", "");
                  const dirtyStayedOff = !box.checked && box.defaultChecked;
                  r1.click();
                  const clickExclusive = r1.checked && !r2.checked;
                  r2.checked = true;
                  const propertyExclusive = !r1.checked && r2.checked;
                  off.click();
                  return [dirtyStayedOff, clickExclusive, propertyExclusive, !off.checked, off.disabled].join("|");
                })()
                "#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "true|true|true|true|true");
    }

    #[test]
    fn query_selector_observes_live_form_state_pseudos() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><input id="box" type="checkbox"><input id="button" type="button" disabled></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval(
                r#"
                (() => {
                  const box = document.getElementById("box");
                  box.click();
                  const checked = document.querySelector(":checked") === box;
                  box.disabled = true;
                  const disabled = document.querySelectorAll(":disabled").length;
                  const enabled = document.querySelectorAll(":enabled").length;
                  box.type = "text";
                  return [checked, disabled, enabled, document.querySelector(":checked") === null].join("|");
                })()
                "#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "true|2|0|true");
    }

    #[test]
    fn node_type_returns_correct_values() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div>text</div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let div_type = runtime
            .eval("document.querySelector('div').nodeType")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(div_type, 1.0, "element nodeType should be 1");

        let doc_type = runtime
            .eval("document.nodeType")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(doc_type, 9.0, "document nodeType should be 9");
    }

    #[test]
    fn clone_node_shallow_and_deep() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        div.set_attribute("class", "original");
        let span = NodeHandle::element("span");
        div.append_child(span);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // Shallow clone should copy attributes but not children
        let shallow_children = runtime
            .eval(
                r#"
            const el = document.querySelector('div');
            const shallow = el.cloneNode(false);
            shallow.childNodes.length;
        "#,
            )
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(
            shallow_children, 0.0,
            "shallow clone should have no children"
        );

        let shallow_class = runtime
            .eval("shallow.className")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            shallow_class, "original",
            "shallow clone should preserve attributes"
        );

        // Deep clone should copy children too
        let deep_children = runtime
            .eval(
                r#"
            const deep = el.cloneNode(true);
            deep.childNodes.length;
        "#,
            )
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(deep_children, 1.0, "deep clone should have children");
    }

    #[test]
    fn parsed_template_exposes_inert_content_fragment() {
        let mut runtime = runtime_from_html(
            r#"<template id="tpl"><script>globalThis.templateScriptRan = true;</script><div class="inside">content</div></template><p>outside</p>"#,
        );
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "unexpected script errors: {errors:?}");

        assert!(runtime
            .eval(
                r#"(() => {
                  const template = document.getElementById("tpl");
                  const inside = template.content.querySelector(".inside");
                  return template instanceof HTMLTemplateElement &&
                    template.childNodes.length === 0 &&
                    template.content instanceof DocumentFragment &&
                    template.content.parentNode === null &&
                    template.content.ownerDocument !== document &&
                    template.content.ownerDocument.nodeType === 9 &&
                    inside.ownerDocument === template.content.ownerDocument &&
                    template.content.isConnected === false &&
                    inside.textContent === "content" &&
                    inside.isConnected === false &&
                    document.querySelector(".inside") === null &&
                    template.innerHTML.includes('class="inside"') &&
                    globalThis.templateScriptRan === undefined;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn dynamic_template_inner_html_and_deep_clone_use_independent_contents() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const template = document.createElement("template");
                  template.innerHTML = '<span data-kind="original">one</span>';
                  document.body.appendChild(template);
                  const clone = template.cloneNode(true);
                  const originalSpan = template.content.querySelector("span");
                  const cloneSpan = clone.content.querySelector("span");
                  cloneSpan.textContent = "two";
                  clone.content.appendChild(document.createElement("b"));
                  return template.content === template.content &&
                    clone instanceof HTMLTemplateElement &&
                    clone.content !== template.content &&
                    originalSpan !== cloneSpan &&
                    originalSpan.textContent === "one" &&
                    cloneSpan.textContent === "two" &&
                    template.content.children.length === 1 &&
                    clone.content.children.length === 2 &&
                    template.childNodes.length === 0 &&
                    originalSpan.isConnected === false;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn open_shadow_root_preserves_tree_boundaries_and_composed_connectivity() {
        let mut runtime = runtime_from_html(
            r#"<div id="host"><span id="light">light</span></div>"#,
        );
        assert!(runtime
            .eval(
                r##"(() => {
                  const host = document.getElementById("host");
                  const root = host.attachShadow({ mode: "open" });
                  root.innerHTML = '<span id="inside">shadow</span>';
                  const inside = root.querySelector("#inside");
                  const clone = host.cloneNode(true);
                  const connectedBeforeDetach = root.isConnected && inside.isConnected;
                  host.remove();
                  const disconnectedWithHost = !root.isConnected && !inside.isConnected;
                  document.body.appendChild(host);
                  const reconnectedWithHost = root.isConnected && inside.isConnected;
                  return root instanceof ShadowRoot &&
                    root instanceof DocumentFragment &&
                    root.host === host &&
                    root.mode === "open" &&
                    root.delegatesFocus === false &&
                    host.shadowRoot === root &&
                    host.childNodes.length === 1 &&
                    root.childNodes.length === 1 &&
                    host.querySelector("#light") !== null &&
                    host.querySelector("#inside") === null &&
                    document.querySelector("#inside") === null &&
                    inside.parentNode === root &&
                    root.parentNode === null &&
                    root.ownerDocument === document &&
                    root.getRootNode() === root &&
                    inside.getRootNode() === root &&
                    inside.getRootNode({ composed: true }) === document &&
                    connectedBeforeDetach && disconnectedWithHost && reconnectedWithHost &&
                    !host.contains(inside) && root.contains(inside) &&
                    root.innerHTML.includes('id="inside"') &&
                    clone.shadowRoot === null;
                })()"##,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn closed_shadow_root_and_invalid_hosts_follow_attach_shadow_errors() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const host = document.createElement("section");
                  document.body.appendChild(host);
                  const root = host.attachShadow({ mode: "closed" });
                  let duplicate = "";
                  let invalidHost = "";
                  let invalidMode = "";
                  let constructor = "";
                  try { host.attachShadow({ mode: "open" }); } catch (e) { duplicate = e.name; }
                  try { document.createElement("img").attachShadow({ mode: "open" }); }
                  catch (e) { invalidHost = e.name; }
                  try { document.createElement("div").attachShadow({ mode: "invalid" }); }
                  catch (e) { invalidMode = e.name; }
                  try { new ShadowRoot(); } catch (e) { constructor = e.name; }
                  return root.mode === "closed" &&
                    root.host === host &&
                    host.shadowRoot === null &&
                    root.isConnected &&
                    duplicate === "NotSupportedError" &&
                    invalidHost === "NotSupportedError" &&
                    invalidMode === "TypeError" &&
                    constructor === "TypeError";
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn shadow_styles_apply_to_internal_host_and_slotted_elements_in_both_modes() {
        for mode in ["open", "closed"] {
            let mut runtime = JsRuntime::new().unwrap();
            let result = runtime
                .eval(&format!(
                    r#"(() => {{
                      const documentStyle = document.createElement("style");
                      documentStyle.textContent =
                        ".inside {{ width: 99px; }} .item {{ margin-left: 44px; padding-left: 6px !important; }}";
                      document.body.appendChild(documentStyle);
                      const host = document.createElement("x-card");
                      host.className = "active";
                      const light = document.createElement("span");
                      light.className = "item";
                      host.appendChild(light);
                      document.body.appendChild(host);
                      const root = host.attachShadow({{ mode: "{mode}" }});
                      root.innerHTML = `<style>
                        .inside {{ width: 11px; }}
                        :host(.active) {{ height: 22px; }}
                        ::slotted(.item) {{ margin-left: 33px; padding-left: 5px !important; }}
                      </style><span class="inside"></span><slot></slot>`;
                      const inside = root.querySelector(".inside");
                      return [
                        getComputedStyle(inside).width,
                        getComputedStyle(host).height,
                        getComputedStyle(light).marginLeft,
                        getComputedStyle(light).paddingLeft,
                      ].join("|");
                    }})()"#,
                ))
                .unwrap()
                .to_string(&mut runtime.context)
                .unwrap()
                .to_std_string_escaped();
            assert_eq!(result, "11px|22px|44px|5px", "shadow mode: {mode}");
        }
    }

    #[test]
    fn shadow_style_and_slot_matching_recompute_after_dom_mutation() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime
            .eval(
                r#"(() => {
                  const host = document.createElement("x-card");
                  const light = document.createElement("span");
                  host.appendChild(light);
                  document.body.appendChild(host);
                  const root = host.attachShadow({ mode: "open" });
                  root.innerHTML = '<style>.inside { width: 10px; } ::slotted(.selected) { height: 20px; }</style>' +
                    '<span class="inside"></span><slot></slot>';
                  const style = root.querySelector("style");
                  const inside = root.querySelector(".inside");
                  const before = getComputedStyle(inside).width;
                  style.textContent = '.inside { width: 12px; } ::slotted(.selected) { height: 20px; }';
                  light.className = "selected";
                  return [before, getComputedStyle(inside).width, getComputedStyle(light).height].join("|");
                })()"#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "10px|12px|20px");
    }

    #[test]
    fn html_slot_element_assigns_named_default_and_fallback_nodes_dynamically() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const host = document.createElement("div");
                  const title = document.createElement("h1");
                  title.slot = "title";
                  const text = document.createTextNode("default");
                  const unmatched = document.createElement("i");
                  unmatched.slot = "missing";
                  host.appendChild(title);
                  host.appendChild(text);
                  host.appendChild(unmatched);
                  document.body.appendChild(host);

                  const root = host.attachShadow({ mode: "open" });
                  root.innerHTML = '<slot name="title"><b>title fallback</b></slot>' +
                    '<slot><em>default fallback</em></slot>';
                  const titleSlot = root.firstElementChild;
                  const defaultSlot = root.lastElementChild;
                  const initial =
                    titleSlot instanceof HTMLSlotElement &&
                    titleSlot.name === "title" &&
                    titleSlot.assignedNodes() instanceof NodeList &&
                    titleSlot.assignedNodes().length === 1 &&
                    titleSlot.assignedNodes()[0] === title &&
                    titleSlot.assignedElements().length === 1 &&
                    title.assignedSlot === titleSlot &&
                    text.assignedSlot === defaultSlot &&
                    unmatched.assignedSlot === null;

                  title.slot = "";
                  const reassigned = title.assignedSlot === defaultSlot &&
                    defaultSlot.assignedNodes().length === 2;
                  host.removeChild(title);
                  host.removeChild(text);
                  const fallback = defaultSlot.assignedNodes({ flatten: true });
                  let illegalConstructor = false;
                  try { new HTMLSlotElement(); } catch (error) {
                    illegalConstructor = error instanceof TypeError;
                  }
                  return initial && reassigned &&
                    defaultSlot.assignedNodes().length === 0 &&
                    fallback.length === 1 && fallback[0].tagName === "EM" &&
                    illegalConstructor;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn assigned_slot_hides_closed_roots_and_slotchange_is_microtask_coalesced() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const openHost = document.createElement("div");
                  document.body.appendChild(openHost);
                  const root = openHost.attachShadow({ mode: "open" });
                  root.innerHTML = "<slot></slot>";
                  const slot = root.firstChild;
                  globalThis.slotChanges = 0;
                  slot.addEventListener("slotchange", () => slotChanges++);
                  const first = document.createElement("span");
                  const second = document.createTextNode("two");
                  openHost.appendChild(first);
                  openHost.appendChild(second);

                  const closedHost = document.createElement("section");
                  const closedChild = document.createElement("b");
                  closedHost.appendChild(closedChild);
                  document.body.appendChild(closedHost);
                  const closedRoot = closedHost.attachShadow({ mode: "closed" });
                  closedRoot.innerHTML = "<slot></slot>";
                  return slot.assignedNodes().length === 2 &&
                    closedRoot.firstChild.assignedNodes()[0] === closedChild &&
                    closedChild.assignedSlot === null && slotChanges === 0;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("slotChanges")
                .unwrap()
                .to_number(&mut runtime.context)
                .unwrap(),
            1.0
        );
    }

    #[test]
    fn fallback_mutations_signal_nested_slots_and_onslotchange() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r##"(() => {
                  const host = document.createElement("div");
                  const root = host.attachShadow({ mode: "open" });
                  root.innerHTML = "<slot id='outer'><slot id='inner'></slot></slot>";
                  const outer = root.querySelector("#outer");
                  const inner = root.querySelector("#inner");
                  globalThis.outerSlotChanges = 0;
                  globalThis.innerSlotChanges = 0;
                  outer.onslotchange = event => {
                    if (event.target === outer) outerSlotChanges++;
                  };
                  inner.addEventListener("slotchange", () => innerSlotChanges++);
                  inner.appendChild(document.createElement("span"));
                  return outerSlotChanges === 0 && innerSlotChanges === 0;
                })()"##,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
        runtime.run_jobs().unwrap();
        let result = runtime
            .eval("outerSlotChanges + '|' + innerSlotChanges")
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "1|1");
    }

    #[test]
    fn shadow_events_respect_composed_boundaries_and_retarget_at_hosts() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime
            .eval(
                r#"(() => {
                  const host = document.createElement("div");
                  document.body.appendChild(host);
                  const root = host.attachShadow({ mode: "open" });
                  root.innerHTML = "<button>inside</button>";
                  const inside = root.firstChild;
                  const order = [];
                  const listen = (node, name, capture) => node.addEventListener("probe", event => {
                    order.push(name + ":" + event.eventPhase + ":" +
                      (event.target === inside ? "inside" : event.target === host ? "host" : "other"));
                  }, capture);
                  listen(window, "window-capture", true);
                  listen(document, "document-capture", true);
                  listen(host, "host-capture", true);
                  listen(root, "root-capture", true);
                  listen(inside, "inside-capture", true);
                  listen(inside, "inside-bubble", false);
                  listen(root, "root-bubble", false);
                  listen(host, "host-bubble", false);
                  listen(document, "document-bubble", false);
                  listen(window, "window-bubble", false);

                  const local = new Event("probe", { bubbles: true });
                  inside.dispatchEvent(local);
                  const localOrder = order.splice(0).join("|");
                  const crossing = new Event("probe", { bubbles: true, composed: true });
                  inside.dispatchEvent(crossing);
                  return [
                    local.composed,
                    local.target === null,
                    crossing.composed,
                    localOrder,
                    order.join("|"),
                    crossing.target === host,
                    crossing.currentTarget === null,
                    crossing.eventPhase,
                    crossing.composedPath().length,
                  ].join("\n");
                })()"#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            result,
            concat!(
                "false\ntrue\ntrue\n",
                "root-capture:1:inside|inside-capture:2:inside|inside-bubble:2:inside|root-bubble:3:inside\n",
                "window-capture:1:host|document-capture:1:host|host-capture:2:host|",
                "root-capture:1:inside|inside-capture:2:inside|inside-bubble:2:inside|",
                "root-bubble:3:inside|host-bubble:2:host|document-bubble:3:host|window-bubble:3:host\n",
                "true\ntrue\n0\n0"
            )
        );
    }

    #[test]
    fn slotted_events_follow_slots_without_retargeting_light_dom_target() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r##"(() => {
                  const host = document.createElement("div");
                  const light = document.createElement("span");
                  host.appendChild(light);
                  document.body.appendChild(host);
                  const root = host.attachShadow({ mode: "open" });
                  root.innerHTML = "<slot></slot>";
                  const slot = root.firstChild;
                  const seen = [];
                  for (const [node, name] of [[slot, "slot"], [root, "root"], [host, "host"], [document, "document"]]) {
                    node.addEventListener("probe", event => {
                      seen.push(name + ":" + (event.target === light) + ":" +
                        event.composedPath().map(node => node === window ? "window" : node.nodeName).join(","));
                    });
                  }
                  light.dispatchEvent(new Event("probe", { bubbles: true }));
                  return seen.length === 4 && seen.every(value => value.includes(":true:")) &&
                    seen[0].includes("SPAN,SLOT,#document-fragment,DIV") &&
                    seen[3].endsWith("#document,window");
                })()"##,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn closed_shadow_event_paths_hide_internals_from_outside_listeners() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const host = document.createElement("div");
                  document.body.appendChild(host);
                  const root = host.attachShadow({ mode: "closed" });
                  root.innerHTML = "<span></span>";
                  const inside = root.firstChild;
                  let internalPath;
                  let externalPath;
                  let externalPathAfterMutation;
                  let internalTarget;
                  let externalTarget;
                  root.addEventListener("probe", event => {
                    internalPath = event.composedPath();
                    internalTarget = event.target;
                  });
                  document.addEventListener("probe", event => {
                    externalPath = event.composedPath();
                    externalTarget = event.target;
                    host.appendChild(inside);
                    externalPathAfterMutation = event.composedPath();
                  });
                  inside.dispatchEvent(new Event("probe", { bubbles: true, composed: true }));
                  return internalTarget === inside && externalTarget === host &&
                    internalPath[0] === inside && internalPath.includes(root) &&
                    externalPath[0] === host && !externalPath.includes(inside) &&
                    !externalPath.includes(root) && externalPath.at(-1) === window &&
                    externalPath.length === externalPathAfterMutation.length &&
                    externalPath.every((node, index) => node === externalPathAfterMutation[index]);
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn related_target_retargeting_suppresses_indistinguishable_outer_events() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime
            .eval(
                r#"(() => {
                  const host = document.createElement("div");
                  document.body.appendChild(host);
                  const root = host.attachShadow({ mode: "open" });
                  root.innerHTML = "<span></span><b></b>";
                  const first = root.firstChild;
                  const second = root.lastChild;
                  let rootCalls = 0;
                  let hostCalls = 0;
                  let documentCalls = 0;
                  let rootRelated = null;
                  root.addEventListener("mouseout", event => { rootCalls++; rootRelated = event.relatedTarget; });
                  host.addEventListener("mouseout", () => hostCalls++);
                  document.addEventListener("mouseout", () => documentCalls++);
                  first.dispatchEvent(new MouseEvent("mouseout", {
                    bubbles: true, composed: true, relatedTarget: second,
                  }));
                  return [rootCalls, hostCalls, documentCalls, rootRelated === second].join("|");
                })()"#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "1|0|0|true");
    }

    #[test]
    fn event_dispatch_reentry_and_listener_removal_follow_dom_semantics() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime
            .eval(
                r#"(() => {
                  const node = document.createElement("div");
                  let calls = 0;
                  let reentry = "";
                  const removed = () => calls += 100;
                  node.addEventListener("probe", event => {
                    calls++;
                    node.removeEventListener("probe", removed);
                    try { node.dispatchEvent(event); } catch (error) { reentry = error.name; }
                  });
                  node.addEventListener("probe", removed);
                  node.addEventListener("probe", () => calls += 10, { once: true });
                  const event = new Event("probe");
                  node.dispatchEvent(event);
                  node.dispatchEvent(event);
                  return [calls, reentry, event.currentTarget, event.eventPhase].join("|");
                })()"#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "12|InvalidStateError||0");
    }

    #[test]
    fn custom_element_registry_defines_and_constructs_autonomous_elements() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  let illegalConstructor = false;
                  let invalidName = "";
                  let duplicateName = "";
                  let duplicateConstructor = "";
                  try { new CustomElementRegistry(); } catch (e) { illegalConstructor = e.name === "TypeError"; }
                  try { customElements.define("invalid", class extends HTMLElement {}); }
                  catch (e) { invalidName = e.name; }

                  let calls = 0;
                  class TestElement extends HTMLElement {
                    constructor() {
                      super();
                      calls++;
                      this.constructed = true;
                    }
                  }
                  globalThis.customElementWhenDefinedResolved = false;
                  const pendingDefinition = customElements.whenDefined("test-element");
                  const samePendingPromise =
                    pendingDefinition === customElements.whenDefined("test-element");
                  pendingDefinition.then(value => {
                    customElementWhenDefinedResolved = value === TestElement;
                  });
                  customElements.define("test-element", TestElement);
                  try { customElements.define("test-element", class extends HTMLElement {}); }
                  catch (e) { duplicateName = e.name; }
                  try { customElements.define("other-element", TestElement); }
                  catch (e) { duplicateConstructor = e.name; }
                  customElements.define("base-html-element", HTMLElement);
                  let htmlElementStayedIllegal = false;
                  try { new HTMLElement(); } catch (e) {
                    htmlElementStayedIllegal = e.name === "TypeError";
                  }
                  class NonAsciiElement extends HTMLElement {}
                  customElements.define("x-À", NonAsciiElement);

                  const created = document.createElement("test-element");
                  const directlyConstructed = new TestElement();
                  const nonAscii = document.createElement("x-À");
                  return typeof CustomElementRegistry === "function" &&
                    customElements instanceof CustomElementRegistry &&
                    illegalConstructor && invalidName === "SyntaxError" &&
                    duplicateName === "NotSupportedError" &&
                    duplicateConstructor === "NotSupportedError" &&
                    htmlElementStayedIllegal &&
                    customElements.get("test-element") === TestElement &&
                    customElements.get("missing-element") === undefined &&
                    customElements.getName(TestElement) === "test-element" &&
                    customElements.getName(class extends HTMLElement {}) === null &&
                    samePendingPromise &&
                    customElements.whenDefined("test-element") === pendingDefinition &&
                    created instanceof TestElement && created.constructed &&
                    directlyConstructed instanceof TestElement &&
                    directlyConstructed.localName === "test-element" &&
                    nonAscii instanceof NonAsciiElement &&
                    nonAscii.localName === "x-À" &&
                    calls === 2;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval("customElementWhenDefinedResolved")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn custom_elements_upgrade_existing_inner_html_and_shadow_tree_in_order() {
        let mut runtime = runtime_from_html(
            r#"<x-upgrade id="first"><x-upgrade id="nested"></x-upgrade></x-upgrade><x-host id="host"></x-host>"#,
        );
        assert!(runtime
            .eval(
                r##"(() => {
                  const order = [];
                  class UpgradeElement extends HTMLElement {
                    constructor() {
                      super();
                      order.push(this.id);
                    }
                  }
                  customElements.define("x-upgrade", UpgradeElement);

                  const container = document.createElement("div");
                  container.innerHTML = '<x-upgrade id="inner"></x-upgrade>';
                  const host = document.getElementById("host");
                  const root = host.attachShadow({ mode: "closed" });
                  root.innerHTML = '<x-shadow id="shadow"></x-shadow>';
                  class ShadowElement extends HTMLElement {
                    constructor() {
                      super();
                      this.attachShadow({ mode: "open" });
                    }
                  }
                  customElements.define("x-shadow", ShadowElement);
                  const shadowElement = root.querySelector("#shadow");

                  return order.join(",") === "first,nested,inner" &&
                    document.getElementById("first") instanceof UpgradeElement &&
                    document.getElementById("nested") instanceof UpgradeElement &&
                    container.firstChild instanceof UpgradeElement &&
                    shadowElement instanceof ShadowElement &&
                    shadowElement.shadowRoot instanceof ShadowRoot;
                })()"##,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn failed_custom_elements_are_not_retried_and_iframe_registry_is_isolated() {
        let mut runtime = runtime_from_html(
            r#"<x-fails id="candidate"></x-fails><iframe id="frame"></iframe>"#,
        );
        assert!(runtime
            .eval(
                r#"(() => {
                  let attempts = 0;
                  class FailingElement extends HTMLElement {
                    constructor() {
                      super();
                      attempts++;
                      throw new Error("expected failure");
                    }
                  }
                  customElements.define("x-fails", FailingElement);
                  const candidate = document.getElementById("candidate");
                  customElements.upgrade(candidate);

                  class TopElement extends HTMLElement {}
                  customElements.define("x-isolated", TopElement);
                  const frame = document.getElementById("frame");
                  const childRegistry = frame.contentWindow.customElements;
                  const childDocument = frame.contentDocument;
                  const beforeDefinition = childDocument.createElement("x-isolated");
                  class ChildElement extends HTMLElement {}
                  childRegistry.define("x-isolated", ChildElement);
                  const remainedUndefined = !(beforeDefinition instanceof ChildElement);
                  childRegistry.upgrade(beforeDefinition);
                  const afterDefinition = childDocument.createElement("x-isolated");

                  return attempts === 1 &&
                    candidate.__customElementState === "failed" &&
                    childRegistry !== customElements &&
                    childRegistry.get("x-isolated") === ChildElement &&
                    customElements.get("x-isolated") === TopElement &&
                    remainedUndefined &&
                    beforeDefinition instanceof ChildElement &&
                    !(beforeDefinition instanceof TopElement) &&
                    afterDefinition instanceof ChildElement;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn custom_element_lifecycle_orders_upgrade_attributes_and_connection() {
        let mut runtime = runtime_from_html(
            r#"<x-lifecycle id="candidate" data-value="one" ignored="no"></x-lifecycle>"#,
        );
        assert!(runtime
            .eval(
                r#"(() => {
                  const calls = [];
                  class LifecycleElement extends HTMLElement {
                    static get observedAttributes() { return ["data-value"]; }
                    constructor() {
                      super();
                      calls.push("constructor");
                    }
                    attributeChangedCallback(name, oldValue, newValue, namespace) {
                      calls.push(`attribute:${name}:${oldValue}:${newValue}:${namespace}`);
                    }
                    connectedCallback() { calls.push("connected"); }
                    disconnectedCallback() { calls.push("disconnected"); }
                  }
                  customElements.define("x-lifecycle", LifecycleElement);
                  const element = document.getElementById("candidate");
                  element.setAttribute("ignored", "yes");
                  element.setAttribute("DATA-VALUE", "two");
                  element.removeAttribute("data-value");
                  element.remove();
                  document.body.appendChild(element);
                  return element instanceof LifecycleElement && calls.join("|") === [
                    "constructor",
                    "attribute:data-value:null:one:null",
                    "connected",
                    "attribute:data-value:one:two:null",
                    "attribute:data-value:two:null:null",
                    "disconnected",
                    "connected",
                  ].join("|");
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn custom_element_subtree_reactions_follow_tree_order_and_inner_html_removal() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const calls = [];
                  class TreeElement extends HTMLElement {
                    connectedCallback() { calls.push("connect:" + this.id); }
                    disconnectedCallback() { calls.push("disconnect:" + this.id); }
                  }
                  customElements.define("x-tree", TreeElement);
                  const container = document.createElement("div");
                  container.innerHTML = '<x-tree id="outer"><x-tree id="inner"></x-tree></x-tree>';
                  calls.length = 0;
                  document.body.appendChild(container);
                  container.remove();
                  document.body.appendChild(container);
                  container.textContent = "replaced";
                  return calls.join("|") === [
                    "connect:outer", "connect:inner",
                    "disconnect:outer", "disconnect:inner",
                    "connect:outer", "connect:inner",
                    "disconnect:outer", "disconnect:inner",
                  ].join("|");
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn custom_element_callback_failures_do_not_stop_later_reactions() {
        let mut runtime = runtime_from_html(
            r#"<x-throws id="first"></x-throws><x-throws id="second"></x-throws><x-constructor-fails></x-constructor-fails>"#,
        );
        assert!(runtime
            .eval(
                r#"(() => {
                  const connected = [];
                  let failedLifecycle = 0;
                  class ThrowingElement extends HTMLElement {
                    connectedCallback() {
                      connected.push(this.id);
                      if (this.id === "first") throw new Error("expected callback error");
                    }
                  }
                  const originalCallback = ThrowingElement.prototype.connectedCallback;
                  customElements.define("x-throws", ThrowingElement);
                  ThrowingElement.prototype.connectedCallback = () => connected.push("replacement");

                  class FailingElement extends HTMLElement {
                    constructor() { super(); throw new Error("expected constructor error"); }
                    connectedCallback() { failedLifecycle++; }
                    disconnectedCallback() { failedLifecycle++; }
                  }
                  customElements.define("x-constructor-fails", FailingElement);
                  const failed = document.querySelector("x-constructor-fails");
                  failed.remove();
                  document.body.appendChild(failed);

                  return connected.join(",") === "first,second" &&
                    ThrowingElement.prototype.connectedCallback !== originalCallback &&
                    document.getElementById("first").__customElementCallbackErrors.length === 1 &&
                    failed.__customElementState === "failed" && failedLifecycle === 0;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn custom_element_reactions_preserve_upgrade_and_reentrant_connection_state() {
        let mut runtime = runtime_from_html(r#"<x-removes-itself></x-removes-itself>"#);
        assert!(runtime
            .eval(
                r#"(() => {
                  const calls = [];
                  const removedItself = document.querySelector("x-removes-itself");
                  class RemovesItself extends HTMLElement {
                    constructor() {
                      super();
                      this.remove();
                      calls.push("constructor:connected=" + this.isConnected);
                    }
                    connectedCallback() { calls.push("connected"); }
                    disconnectedCallback() { calls.push("disconnected"); }
                  }
                  customElements.define("x-removes-itself", RemovesItself);
                  document.body.appendChild(removedItself);

                  let detachedAttributes = 0;
                  let detachedConnections = 0;
                  const detached = document.createElement("x-connects-itself");
                  class ConnectsItself extends HTMLElement {
                    static get observedAttributes() { return ["data-created"]; }
                    constructor() {
                      super();
                      this.setAttribute("data-created", "yes");
                      document.body.appendChild(this);
                    }
                    attributeChangedCallback() { detachedAttributes++; }
                    connectedCallback() { detachedConnections++; }
                  }
                  customElements.define("x-connects-itself", ConnectsItself);
                  customElements.upgrade(detached);

                  let childConnections = 0;
                  class ReparentingParent extends HTMLElement {
                    connectedCallback() {
                      const child = this.firstChild;
                      child.remove();
                      this.appendChild(child);
                    }
                  }
                  class ReparentedChild extends HTMLElement {
                    connectedCallback() { childConnections++; }
                  }
                  customElements.define("x-reparenting-parent", ReparentingParent);
                  customElements.define("x-reparented-child", ReparentedChild);
                  const container = document.createElement("div");
                  container.innerHTML =
                    "<x-reparenting-parent><x-reparented-child></x-reparented-child></x-reparenting-parent>";
                  document.body.appendChild(container);

                  return calls.join("|") ===
                      "constructor:connected=false|connected|connected" &&
                    detached.isConnected && detachedAttributes === 0 &&
                    detachedConnections === 0 && childConnections === 1;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn remove_attribute_removes_from_dom() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        div.set_attribute("data-value", "42");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let has = runtime
            .eval("document.querySelector('div').hasAttribute('data-value')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(has, "attribute should exist before removal");

        runtime
            .eval("document.querySelector('div').removeAttribute('data-value')")
            .unwrap();

        let has = runtime
            .eval("document.querySelector('div').hasAttribute('data-value')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(!has, "attribute should be removed");
    }

    #[test]
    fn create_document_fragment_has_correct_node_type() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let node_type = runtime
            .eval("document.createDocumentFragment().nodeType")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(node_type, 11.0, "DocumentFragment nodeType should be 11");
    }

    #[test]
    fn document_fragment_can_hold_children() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div id='target'></div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const frag = document.createDocumentFragment();
            frag.appendChild(document.createElement('p'));
            frag.appendChild(document.createElement('span'));
            document.getElementById('target').appendChild(frag);
        "#,
            )
            .unwrap();

        let children = runtime
            .eval("document.getElementById('target').childNodes.length")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        // Fragment itself is appended (not its children individually) since we don't have
        // special fragment append semantics yet, but the fragment node holds the children.
        assert!(
            children > 0.0,
            "target should have children after appending fragment"
        );
    }

    #[test]
    fn inner_html_round_trip() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div id='box'><span class=\"a\">Hello</span></div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let inner = runtime
            .eval("document.getElementById('box').innerHTML")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert!(inner.contains("<span"), "innerHTML should contain span tag");
        assert!(inner.contains("Hello"), "innerHTML should contain text");

        // Set and re-read
        runtime
            .eval(r#"document.getElementById('box').innerHTML = '<b>Bold</b>'"#)
            .unwrap();
        let inner = runtime
            .eval("document.getElementById('box').innerHTML")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert!(
            inner.contains("<b>"),
            "innerHTML should contain b tag after set"
        );
        assert!(
            inner.contains("Bold"),
            "innerHTML should contain text after set"
        );
    }

    #[test]
    fn inner_html_escapes_text() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let text = NodeHandle::text("<script>alert(1)</script>");
        div.append_child(text);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let inner = runtime
            .eval("document.querySelector('div').innerHTML")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert!(
            inner.contains("&lt;script&gt;"),
            "innerHTML should escape angle brackets in text: {inner}"
        );
        assert!(
            !inner.contains("<script>"),
            "innerHTML should not contain raw script tag"
        );
    }

    #[test]
    fn text_content_null_sets_empty() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let text = NodeHandle::text("Hello");
        div.append_child(text);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval("document.querySelector('div').textContent = null")
            .unwrap();
        let result = runtime
            .eval("document.querySelector('div').textContent")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "", "textContent = null should produce empty string");
    }

    #[test]
    fn inner_html_null_sets_empty() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let span = NodeHandle::element("span");
        div.append_child(span);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval("document.querySelector('div').innerHTML = null")
            .unwrap();
        let result = runtime
            .eval("document.querySelector('div').innerHTML")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "", "innerHTML = null should produce empty string");
    }

    #[test]
    fn text_content_excludes_comments() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div>Hello<!-- comment -->World</div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval("document.querySelector('div').textContent")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            result, "HelloWorld",
            "textContent should not include comment data"
        );
    }

    #[test]
    fn document_fragment_appends_children_not_self() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div id='target'></div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        runtime
            .eval(
                r#"
            const frag = document.createDocumentFragment();
            const p = document.createElement('p');
            const span = document.createElement('span');
            frag.appendChild(p);
            frag.appendChild(span);
            document.getElementById('target').appendChild(frag);
        "#,
            )
            .unwrap();

        // Fragment's children should be directly under target
        let target = doc.query_selector("#target").unwrap();
        let children = target.child_nodes();
        let tags: Vec<_> = children.iter().filter_map(|c| c.tag_name()).collect();
        assert_eq!(
            tags,
            vec!["p", "span"],
            "fragment children should be appended directly"
        );
    }

    #[test]
    fn owner_document_is_null_for_document() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let is_null = runtime
            .eval("document.ownerDocument === null")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(is_null, "document.ownerDocument should be null");

        runtime
            .eval("const el = document.createElement('div')")
            .unwrap();
        let is_doc = runtime
            .eval("el.ownerDocument === document")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(is_doc, "element.ownerDocument should be document");
    }

    #[test]
    fn get_root_node_returns_document_for_connected_nodes() {
        let doc = crate::html::TreeBuilder::parse(
            "<html><body><main><span id='target'></span></main></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert!(
            runtime
                .eval("document.getElementById('target').getRootNode() === document")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(
            runtime
                .eval("document.getElementById('target').getRootNode({ composed: true }) === document")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn tag_name_undefined_for_non_elements() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let is_undef = runtime
            .eval("document.createTextNode('x').tagName === undefined")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(is_undef, "text node tagName should be undefined");

        let tag = runtime
            .eval("document.createElement('div').tagName")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(tag, "DIV", "element tagName should be uppercase tag");
    }

    #[test]
    fn comment_text_content_returns_data() {
        use crate::html::TreeBuilder;
        let html = "<html><body><!-- hello --></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        // Comment nodes are in the body's childNodes
        let result = runtime
            .eval(
                r#"
            const body = document.body;
            let commentText = null;
            const nodes = body.childNodes;
            for (let i = 0; i < nodes.length; i++) {
                if (nodes[i].nodeType === 8) {
                    commentText = nodes[i].textContent;
                    break;
                }
            }
            commentText;
        "#,
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(
            result.trim(),
            "hello",
            "comment textContent should return its data"
        );
    }

    #[test]
    fn prevent_default_and_stop_immediate_propagation() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            let count = 0;
            let prevented = false;
            let dispatchReturn = null;
            const el = document.querySelector("div");
            el.addEventListener("click", (e) => { count++; e.preventDefault(); e.stopImmediatePropagation(); });
            el.addEventListener("click", () => { count++; });
            const evt = new MouseEvent("click", { cancelable: true });
            dispatchReturn = el.dispatchEvent(evt);
            prevented = evt.defaultPrevented;
        "#).unwrap();

        let count = runtime
            .eval("count")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(
            count, 1.0,
            "stopImmediatePropagation should prevent later listeners"
        );

        let prevented = runtime.eval("prevented").unwrap().as_boolean().unwrap();
        assert!(prevented, "preventDefault should set defaultPrevented");

        let dispatch_return = runtime
            .eval("dispatchReturn")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            !dispatch_return,
            "dispatchEvent should return false when default prevented"
        );
    }

    #[test]
    fn stop_propagation_allows_same_node_listeners() {
        let doc = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let div = NodeHandle::element("div");
        doc.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            let count = 0;
            let parentFired = false;
            const el = document.querySelector("div");
            el.addEventListener("click", (e) => { count++; e.stopPropagation(); });
            el.addEventListener("click", () => { count++; });
            document.querySelector("body").addEventListener("click", () => { parentFired = true; });
            el.dispatchEvent(new Event("click", { bubbles: true }));
        "#,
            )
            .unwrap();

        let count = runtime
            .eval("count")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(
            count, 2.0,
            "stopPropagation should NOT prevent other listeners on same node"
        );

        let parent_fired = runtime.eval("parentFired").unwrap().as_boolean().unwrap();
        assert!(
            !parent_fired,
            "stopPropagation should prevent bubbling to parent"
        );
    }

    #[test]
    fn custom_event_has_detail() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            let detail = null;
            document.addEventListener("custom", (e) => { detail = e.detail; });
            document.dispatchEvent(new CustomEvent("custom", { detail: 42 }));
        "#,
            )
            .unwrap();

        let detail = runtime
            .eval("detail")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(detail, 42.0, "CustomEvent detail should be accessible");
    }

    #[test]
    fn dataset_proxy_read_write() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        div.set_attribute("data-foo", "bar");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime
            .eval("document.querySelector('div').dataset.foo")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "bar");

        runtime
            .eval("document.querySelector('div').dataset.baz = 'qux'")
            .unwrap();
        let attr = div.attributes().unwrap().get("data-baz").cloned();
        assert_eq!(
            attr.as_deref(),
            Some("qux"),
            "dataset setter should set data- attribute"
        );
    }

    #[test]
    fn is_connected_property() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let connected = runtime
            .eval("document.querySelector('div').isConnected")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(connected, "element in document should be connected");

        let disconnected = runtime
            .eval("document.createElement('span').isConnected")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(!disconnected, "orphan element should not be connected");

        let removed = runtime
            .eval(
                r#"
            const child = document.createElement('span');
            document.querySelector('div').appendChild(child);
            child.remove();
            child.parentNode === null && child.isConnected === false
        "#,
            )
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            removed,
            "remove() should detach an element and update isConnected"
        );

        let orphan_remove = runtime
            .eval(
                r#"
            const orphan = document.createElement('span');
            orphan.remove();
            orphan.parentNode === null && orphan.isConnected === false
        "#,
            )
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            orphan_remove,
            "remove() should be a no-op for an orphan element"
        );

        let child_node_surface = runtime
            .eval(
                "typeof document.remove === 'undefined' && typeof document.createDocumentFragment().remove === 'undefined' && typeof document.createElement('div').remove === 'function' && typeof document.createTextNode('x').remove === 'function'",
            )
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            child_node_surface,
            "remove() should only be exposed on ChildNode implementations"
        );
    }

    #[test]
    fn document_create_processing_instruction() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const pi = document.createProcessingInstruction('xml-stylesheet', 'href="style.css"');
                const clone = pi.cloneNode();
                const errors = [];
                for (const args of [['bad name', 'data'], ['ok', 'bad?>data']]) {
                    try { document.createProcessingInstruction(...args); errors.push('none'); }
                    catch (error) { errors.push(error.name); }
                }
                return [pi.nodeType, pi.nodeName, pi.target, pi.data, clone.nodeType, clone.nodeName, clone.data, ...errors].join('|');
            })()"#,
        );
        assert_eq!(
            result,
            "7|xml-stylesheet|xml-stylesheet|href=\"style.css\"|7|xml-stylesheet|href=\"style.css\"|InvalidCharacterError|InvalidCharacterError"
        );
    }

    #[test]
    fn document_create_comment() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let node_type = runtime
            .eval("document.createComment('test').nodeType")
            .unwrap()
            .to_number(&mut runtime.context)
            .unwrap();
        assert_eq!(node_type, 8.0, "comment nodeType should be 8");
    }

    #[test]
    fn document_ready_state() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let state = runtime
            .eval("document.readyState")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(state, "complete");
    }

    #[test]
    fn document_ready_state_follows_script_and_load_lifecycle() {
        let mut runtime = runtime_from_html(
            r#"<html><body><script>
                globalThis.readyStates = [document.readyState];
                document.addEventListener("DOMContentLoaded", () => readyStates.push(document.readyState));
                window.addEventListener("load", () => readyStates.push(document.readyState));
            </script></body></html>"#,
        );

        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(
            runtime
                .eval("readyStates.join(',') + '|' + document.readyState")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "loading,interactive|interactive"
        );

        runtime.fire_load().unwrap();
        assert_eq!(
            runtime
                .eval("readyStates.join(',') + '|' + document.readyState")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "loading,interactive,complete|complete"
        );
    }

    #[test]
    fn module_script_executes_before_dom_content_loaded() {
        let mut runtime = runtime_from_html(
            r#"<html><body>
                <script type="module">globalThis.moduleRan = true;</script>
                <script>document.addEventListener("DOMContentLoaded", () => globalThis.moduleWasReady = moduleRan);</script>
            </body></html>"#,
        );
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(
            runtime
                .eval("moduleRan && moduleWasReady")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn network_scripts_require_public_fetch_outside_document_origin() {
        let document_url: crate::http::Url = "https://example.com/app/index.html".parse().unwrap();
        let same_origin: crate::http::Url = "https://example.com/app.js".parse().unwrap();
        let cross_origin: crate::http::Url = "https://cdn.example.net/app.js".parse().unwrap();

        assert!(!requires_public_fetch(&same_origin, Some(&document_url)));
        assert!(requires_public_fetch(&cross_origin, Some(&document_url)));
        assert!(requires_public_fetch(&cross_origin, None));
    }

    #[test]
    fn inline_module_url_uses_parseable_document_base() {
        let base: crate::http::Url = "https://example.com/app/index.html".parse().unwrap();
        assert_eq!(
            module_script_url("inline-script-1", Some(&base), true),
            "https://example.com/app/index.html#inline-script-1"
        );
    }

    #[test]
    fn inline_module_resolves_relative_import_against_document_url() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let size = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..size]).contains("GET /app/dep.js "));
            let body = b"export const answer = 42;";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });

        let mut runtime = runtime_from_html(
            r#"<script type="module">import { answer } from './dep.js'; globalThis.answer = answer;</script>"#,
        );
        let base: crate::http::Url = format!("http://{address}/app/index.html").parse().unwrap();
        let errors = runtime.execute_document_scripts(Some(&base));
        handle.join().unwrap();

        assert!(errors.is_empty(), "unexpected module errors: {errors:?}");
        assert_eq!(runtime.eval("answer").unwrap().as_number(), Some(42.0));
    }

    #[test]
    fn matches_and_closest() {
        let doc = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let div = NodeHandle::element("div");
        div.set_attribute("class", "wrapper");
        let span = NodeHandle::element("span");
        doc.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(div.clone());
        div.append_child(span.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let matches = runtime
            .eval("document.querySelector('span').matches('span')")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(matches, "span should match 'span'");

        let closest = runtime
            .eval("document.querySelector('span').closest('.wrapper').tagName")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(closest, "DIV");

        assert!(runtime
            .eval("(() => { try { document.querySelector('span').matches(); } catch (error) { return error instanceof TypeError; } return false; })()")
            .unwrap()
            .as_boolean()
            .unwrap());
        assert!(runtime
            .eval("(() => { try { document.querySelector('span').matches(''); } catch (error) { return error instanceof DOMException && error.name === 'SyntaxError'; } return false; })()")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn storage_methods_follow_webidl_string_conversion_and_ordering() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                    localStorage.clear();
                    localStorage.setItem(1, null);
                    localStorage.setItem(undefined, 42);
                    localStorage.setItem("last", "value");
                    const ordered = localStorage.length === 3 &&
                        localStorage.key(0) === "1" &&
                        localStorage.key(1) === "undefined" &&
                        localStorage.key(2) === "last" &&
                        localStorage.key(3) === null &&
                        localStorage.getItem(1) === "null" &&
                        localStorage.getItem() === "42";
                    localStorage.setItem("1", "updated");
                    const stable = localStorage.key(0) === "1";
                    localStorage.removeItem(1);
                    const removed = localStorage.getItem("1") === null &&
                        localStorage.key(0) === "undefined";
                    localStorage.clear();
                    return ordered && stable && removed && localStorage.length === 0;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn storage_is_scoped_by_origin_and_top_level_session() {
        let storage = StorageManager::new();
        let first_session = storage.create_session();
        let second_session = storage.create_session();
        let mut first = JsRuntime::with_document_url_and_storage(
            default_document(), "https://example.com/first", storage.clone(), first_session,
        ).unwrap();
        first.eval("localStorage.setItem('shared', 'local'); sessionStorage.setItem('shared', 'session-a')").unwrap();

        let mut same_origin = JsRuntime::with_document_url_and_storage(
            default_document(), "https://example.com:443/second", storage.clone(), first_session,
        ).unwrap();
        assert!(same_origin.eval("localStorage.getItem('shared') === 'local' && sessionStorage.getItem('shared') === 'session-a'").unwrap().as_boolean().unwrap());

        let mut other_session = JsRuntime::with_document_url_and_storage(
            default_document(), "https://example.com/third", storage.clone(), second_session,
        ).unwrap();
        assert!(other_session.eval("localStorage.getItem('shared') === 'local' && sessionStorage.getItem('shared') === null").unwrap().as_boolean().unwrap());

        let mut other_origin = JsRuntime::with_document_url_and_storage(
            default_document(), "https://other.example/", storage, first_session,
        ).unwrap();
        assert!(other_origin.eval("localStorage.getItem('shared') === null && sessionStorage.getItem('shared') === null").unwrap().as_boolean().unwrap());
    }

    #[test]
    fn same_origin_iframe_shares_storage_and_receives_storage_event() {
        let document = crate::html::TreeBuilder::parse(
            "<html><body><iframe id='frame'></iframe></body></html>",
        ).document();
        let mut runtime = JsRuntime::with_document_and_url(document, "https://example.com/").unwrap();

        assert!(runtime.eval(
            r#"(() => {
                const frame = document.querySelector('#frame');
                const child = frame.contentWindow;
                let observed = null;
                child.addEventListener('storage', event => {
                    observed = [event.key, event.oldValue, event.newValue,
                        event.url, event.storageArea === child.localStorage].join('|');
                });
                localStorage.clear();
                localStorage.setItem('shared', 'yes');
                return child.localStorage.getItem('shared') === 'yes' &&
                    observed === 'shared||yes|https://example.com/|true';
            })()"#,
        ).unwrap().as_boolean().unwrap());
    }

    #[test]
    fn opaque_origin_storage_access_throws_security_error() {
        let mut runtime = JsRuntime::with_document_and_url(default_document(), "about:blank").unwrap();
        assert!(runtime.eval(
            "(() => { try { return localStorage.length; } catch (error) { return error instanceof DOMException && error.name === 'SecurityError'; } })()",
        ).unwrap().as_boolean().unwrap());
    }

    #[test]
    fn nested_about_blank_iframe_inherits_its_owning_documents_opaque_origin() {
        let document = crate::html::TreeBuilder::parse(
            "<html><body><iframe id='outer' src='data:text/html,%3Chtml%3E%3Cbody%3E%3C/body%3E%3C/html%3E'></iframe></body></html>",
        ).document();
        let mut runtime = JsRuntime::with_document_and_url(document, "https://example.com/").unwrap();

        assert!(runtime.eval(
            r#"(() => {
                const childDocument = document.querySelector('#outer').contentDocument;
                const nested = childDocument.createElement('iframe');
                childDocument.body.appendChild(nested);
                try {
                    return nested.contentWindow.localStorage.length;
                } catch (error) {
                    return error instanceof DOMException && error.name === 'SecurityError';
                }
            })()"#,
        ).unwrap().as_boolean().unwrap());
    }

    #[test]
    fn performance_now_is_monotonic_and_has_epoch_time_origin() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        assert!(runtime
            .eval(
                "(() => { const first = performance.now(); const second = performance.now(); \
                 return Number.isFinite(performance.timeOrigin) && performance.timeOrigin > 0 && \
                 first >= 0 && second >= first && \
                 Math.abs(Date.now() - (performance.timeOrigin + second)) < 1000 && \
                 Object.getOwnPropertyDescriptor(performance, 'timeOrigin').writable === false && \
                 (() => { const origin = performance.timeOrigin; performance.timeOrigin = 0; \
                   return performance.timeOrigin === origin; })(); })()"
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn user_timing_records_orders_and_clears_entries() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const late = performance.mark("same", { startTime: 20, detail: { id: 1 } });
                  const early = performance.mark("same", { startTime: 10 });
                  const measure = performance.measure("span", "early-missing");
                  return false;
                })()"#
            )
            .is_err(), "an unknown mark must throw");

        assert!(runtime
            .eval(
                r#"(() => {
                  performance.clearMarks();
                  performance.clearMeasures();
                  const late = performance.mark("same", { startTime: 20, detail: { id: 1 } });
                  const early = performance.mark("same", { startTime: 10 });
                  const measure = performance.measure("span", { start: 10, end: 25, detail: "d" });
                  const entries = performance.getEntries();
                  const valid = late instanceof PerformanceMark && late instanceof PerformanceEntry &&
                    measure instanceof PerformanceMeasure && measure.duration === 15 && measure.detail === "d" &&
                    late.detail.id === 1 && entries.length === 3 &&
                    entries[0] === early && entries[1] === measure && entries[2] === late &&
                    performance.getEntriesByName("same", "mark").length === 2 &&
                    performance.getEntriesByType("measure")[0] === measure;
                  performance.clearMarks("same");
                  performance.clearMeasures("span");
                  return valid && performance.getEntries().length === 0;
                })()"#
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn user_timing_measure_resolves_marks_and_validates_options() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  performance.mark("begin", { startTime: 4 });
                  performance.mark("finish", { startTime: 9 });
                  const byMarks = performance.measure("by-marks", "begin", "finish");
                  const byDuration = performance.measure("by-duration", { start: "begin", duration: 3 });
                  function markOptions() {}
                  markOptions.startTime = 12;
                  markOptions.detail = "function-mark";
                  const functionMark = performance.mark("function-mark", markOptions);
                  function measureOptions() {}
                  measureOptions.start = 12;
                  measureOptions.duration = 2;
                  measureOptions.detail = "function-measure";
                  const functionMeasure = performance.measure("function-measure", measureOptions);
                  const nullStart = performance.measure("null-start", null, "finish");
                  let missingIsSyntaxError = false;
                  let invalidOptionsTypeError = false;
                  let negativeTypeError = false;
                  let entryConstructorTypeError = false;
                  let measureConstructorTypeError = false;
                  try { performance.measure("missing", "unknown"); }
                  catch (error) { missingIsSyntaxError = error instanceof DOMException && error.name === "SyntaxError"; }
                  try { performance.measure("invalid", {}); }
                  catch (error) { invalidOptionsTypeError = error instanceof TypeError; }
                  try { performance.mark("negative", { startTime: -1 }); }
                  catch (error) { negativeTypeError = error instanceof TypeError; }
                  try { new PerformanceEntry("entry", "mark", 0, 0); }
                  catch (error) { entryConstructorTypeError = error instanceof TypeError; }
                  try { new PerformanceMeasure("measure", 0, 1); }
                  catch (error) { measureConstructorTypeError = error instanceof TypeError; }
                  return byMarks.startTime === 4 && byMarks.duration === 5 &&
                    byDuration.startTime === 4 && byDuration.duration === 3 &&
                    functionMark instanceof PerformanceMark && functionMark instanceof PerformanceEntry &&
                    functionMark.startTime === 12 && functionMark.detail === "function-mark" &&
                    functionMeasure instanceof PerformanceMeasure && functionMeasure instanceof PerformanceEntry &&
                    functionMeasure.startTime === 12 && functionMeasure.duration === 2 &&
                    functionMeasure.detail === "function-measure" &&
                    nullStart.startTime === 0 && nullStart.duration === 9 &&
                    missingIsSyntaxError && invalidOptionsTypeError && negativeTypeError &&
                    entryConstructorTypeError && measureConstructorTypeError;
                })()"#
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn performance_observer_delivers_marks_and_measures_in_one_checkpoint() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        runtime
            .eval(
                r#"(() => {
                  globalThis.observerDeliveries = [];
                  const observer = new PerformanceObserver((list, receivedObserver) => {
                    observerDeliveries.push({
                      observerMatches: receivedObserver === observer,
                      all: list.getEntries().map(entry => entry.name + ":" + entry.entryType),
                      marks: list.getEntriesByType("mark").map(entry => entry.name),
                      named: list.getEntriesByName("middle", "measure").length,
                      listType: list instanceof PerformanceObserverEntryList,
                    });
                  });
                  observer.observe({ entryTypes: ["mark", "measure"] });
                  performance.mark("late", { startTime: 20 });
                  performance.mark("early", { startTime: 5 });
                  performance.measure("middle", { start: 10, end: 15 });
                })()"#,
            )
            .unwrap();
        assert!(runtime
            .eval("observerDeliveries.length === 0")
            .unwrap()
            .as_boolean()
            .unwrap());

        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval(
                r#"observerDeliveries.length === 1 &&
                   observerDeliveries[0].observerMatches && observerDeliveries[0].listType &&
                   observerDeliveries[0].all.join(",") === "early:mark,middle:measure,late:mark" &&
                   observerDeliveries[0].marks.join(",") === "early,late" &&
                   observerDeliveries[0].named === 1"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn performance_observer_supports_buffered_take_records_and_disconnect() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        runtime
            .eval(
                r#"(() => {
                  performance.mark("before", { startTime: 1 });
                  globalThis.bufferedDeliveries = [];
                  globalThis.bufferedObserver = new PerformanceObserver(list => {
                    bufferedDeliveries.push(list.getEntries().map(entry => entry.name).join(","));
                  });
                  bufferedObserver.observe({ type: "mark", buffered: true });
                  performance.mark("taken", { startTime: 2 });
                  globalThis.takenRecords = bufferedObserver.takeRecords().map(entry => entry.name).join(",");
                })()"#,
            )
            .unwrap();
        assert_eq!(
            runtime
                .eval("takenRecords")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "before,taken"
        );
        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval("bufferedDeliveries.length === 0")
            .unwrap()
            .as_boolean()
            .unwrap());

        runtime
            .eval(
                r#"performance.mark("delivered", { startTime: 3 });
                   bufferedObserver.disconnect();
                   performance.mark("after-disconnect", { startTime: 4 });"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        assert!(runtime
            .eval("bufferedDeliveries.length === 0 && bufferedObserver.takeRecords().length === 0")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn performance_observer_validates_options_and_survives_callback_errors() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        assert!(runtime
            .eval(
                r#"(() => {
                  const throwsTypeError = callback => {
                    try { callback(); } catch (error) { return error instanceof TypeError; }
                    return false;
                  };
                  const observer = new PerformanceObserver(() => {});
                  const modes = new PerformanceObserver(() => {});
                  modes.observe({ type: "mark" });
                  let modeError = false;
                  try { modes.observe({ entryTypes: ["mark"] }); }
                  catch (error) { modeError = error instanceof DOMException && error.name === "InvalidModificationError"; }
                  return throwsTypeError(() => new PerformanceObserver(null)) &&
                    throwsTypeError(() => new PerformanceObserverEntryList()) &&
                    throwsTypeError(() => observer.observe()) &&
                    throwsTypeError(() => observer.observe(null)) &&
                    throwsTypeError(() => observer.observe({})) &&
                    throwsTypeError(() => observer.observe({ type: "mark", entryTypes: ["mark"] })) &&
                    throwsTypeError(() => observer.observe({ entryTypes: [] })) && modeError &&
                    Object.isFrozen(PerformanceObserver.supportedEntryTypes) &&
                    PerformanceObserver.supportedEntryTypes.join(",") === "mark,measure";
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());

        runtime
            .eval(
                r#"globalThis.goodObserverCalls = 0;
                   const throwing = new PerformanceObserver(() => { throw new Error("callback failure"); });
                   const continuing = new PerformanceObserver(() => { goodObserverCalls++; });
                   throwing.observe({ type: "mark" });
                   continuing.observe({ type: "mark" });
                   performance.mark("first");"#,
            )
            .unwrap();
        runtime.run_jobs().unwrap();
        runtime.eval("performance.mark('second')").unwrap();
        runtime.run_jobs().unwrap();
        assert_eq!(
            runtime
                .eval("goodObserverCalls")
                .unwrap()
                .as_number()
                .unwrap(),
            2.0
        );
    }

    #[test]
    fn match_media_evaluates_current_viewport() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.set_viewport(1024.0, 768.0);
        let matches = runtime
            .eval("(() => { const query = matchMedia('screen and (min-width: 768px) and (orientation: landscape)'); return query instanceof MediaQueryList && query instanceof EventTarget && query.matches && !matchMedia('(max-width: 600px)').matches && !matchMedia('print').matches; })()")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(matches, "media queries should use the runtime viewport");
    }

    #[test]
    fn match_media_notifies_when_viewport_changes_result() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.set_viewport(1024.0, 768.0);
        runtime
            .eval(
                r#"globalThis.viewportQuery = matchMedia('(min-width: 900px)');
                   globalThis.mediaChanges = [];
                   viewportQuery.addEventListener('change', event => mediaChanges.push([event.matches, event.media]));"#,
            )
            .unwrap();

        runtime.set_viewport(800.0, 768.0);
        assert!(runtime
            .eval("!viewportQuery.matches && mediaChanges.length === 1 && mediaChanges[0][0] === false && mediaChanges[0][1] === '(min-width: 900px)'")
            .unwrap()
            .as_boolean()
            .unwrap());

        runtime.set_viewport(700.0, 768.0);
        assert_eq!(
            runtime.eval("mediaChanges.length").unwrap().as_number(),
            Some(1.0),
            "a viewport change that preserves the result must not fire change"
        );
    }

    #[test]
    fn request_animation_frame_requires_explicit_rendering_opportunity() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                "globalThis.rafCalled = false; globalThis.rafTimestamp = -1; \
                 requestAnimationFrame(timestamp => { rafCalled = true; rafTimestamp = timestamp; });",
            )
            .unwrap();
        runtime.run_jobs().unwrap();

        assert_eq!(runtime.eval("rafCalled").unwrap().as_boolean(), Some(false));
        assert_eq!(runtime.run_animation_frame(16).unwrap(), 1);
        assert!(runtime
            .eval("rafCalled && rafTimestamp === 16")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn css_transition_uses_animation_frame_time_for_style_and_layout() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                html, body { margin: 0; }
                #target { opacity: 0; width: 10px; height: 10px;
                          transition: opacity 1s linear, width 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target')).opacity"
            ),
            "0"
        );
        assert_eq!(
            runtime
                .eval("document.getElementById('target').offsetWidth")
                .unwrap()
                .as_number(),
            Some(10.0)
        );

        runtime
            .eval(
                "const target = document.getElementById('target'); \
                 target.style.opacity = '1'; target.style.width = '30px';",
            )
            .unwrap();
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).opacity"),
            "0"
        );

        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).opacity"),
            "0.5"
        );
        assert_eq!(
            runtime.eval("target.offsetWidth").unwrap().as_number(),
            Some(20.0)
        );

        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).opacity"),
            "1"
        );
        assert_eq!(
            runtime.eval("target.offsetWidth").unwrap().as_number(),
            Some(30.0)
        );
    }

    #[test]
    fn paint_only_transition_reuses_layout_between_frames() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { width: 10px; height: 10px; opacity: 0;
                          transition: opacity 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.target = document.getElementById('target'); \
                 target.offsetWidth; target.style.opacity = '1'; \
                 getComputedStyle(target).opacity;",
            )
            .unwrap();

        runtime.run_animation_frame(250).unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(10.0));
        let first_sample_generation = runtime.host_state.borrow().layout_generation;

        runtime.run_animation_frame(250).unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(10.0));
        assert_eq!(
            runtime.host_state.borrow().layout_generation,
            first_sample_generation,
            "opacity sampling must retain the cached layout tree"
        );
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(target).opacity"), "0.5");
    }

    #[test]
    fn filter_transitions_reuse_layout_between_frames() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { width: 10px; height: 10px;
                          filter: blur(0px); backdrop-filter: brightness(1);
                          transition: filter 1s linear, backdrop-filter 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.target = document.getElementById('target'); \
                 target.offsetWidth; target.style.filter = 'blur(10px)'; \
                 target.style.backdropFilter = 'brightness(0.5)'; \
                 getComputedStyle(target).filter; \
                 getComputedStyle(target).backdropFilter;",
            )
            .unwrap();

        runtime.run_animation_frame(250).unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(10.0));
        let first_sample_generation = runtime.host_state.borrow().layout_generation;

        runtime.run_animation_frame(250).unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(10.0));
        assert_eq!(
            runtime.host_state.borrow().layout_generation,
            first_sample_generation,
            "filter and backdrop-filter sampling must retain the cached layout tree"
        );
    }

    #[test]
    fn detached_mutations_reuse_layout_until_insertion() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>.wide { width: 30px; }</style></head>
               <body><div id="target" style="width: 10px"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        assert_eq!(
            runtime
                .eval("document.getElementById('target').offsetWidth")
                .unwrap()
                .as_number(),
            Some(10.0)
        );
        let initial_generation = runtime.host_state.borrow().layout_generation;

        runtime
            .eval(
                "globalThis.detached = document.createElement('div'); \
                 detached.setAttribute('class', 'wide'); \
                 detached.style.height = '10px';",
            )
            .unwrap();
        assert_eq!(
            runtime
                .eval("document.getElementById('target').offsetWidth")
                .unwrap()
                .as_number(),
            Some(10.0)
        );
        assert_eq!(
            runtime.host_state.borrow().layout_generation,
            initial_generation,
            "mutating a detached element must retain the live document layout"
        );

        runtime.eval("document.body.appendChild(detached)").unwrap();
        assert_eq!(runtime.eval("detached.offsetWidth").unwrap().as_number(), Some(30.0));
        assert!(
            runtime.host_state.borrow().layout_generation > initial_generation,
            "inserting the detached element must invalidate and rebuild layout"
        );
    }

    #[test]
    fn ordinary_dom_mutations_reuse_parsed_stylesheets() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style id="sheet">div { width: 10px; } .active { width: 30px; }</style></head>
               <body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        assert_eq!(
            runtime
                .eval("globalThis.target = document.getElementById('target'); target.offsetWidth")
                .unwrap()
                .as_number(),
            Some(10.0)
        );
        let initial_resolver_generation =
            runtime.host_state.borrow().style_resolver_generation;

        runtime.eval("target.setAttribute('class', 'active')").unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(30.0));
        assert_eq!(
            runtime.host_state.borrow().style_resolver_generation,
            initial_resolver_generation,
            "attribute mutation must retain parsed stylesheets and rule indexes"
        );

        runtime.eval("target.style.width = '40px'").unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(40.0));
        assert_eq!(
            runtime.host_state.borrow().style_resolver_generation,
            initial_resolver_generation,
            "inline style mutation must retain the resolver"
        );

        runtime
            .eval(
                "target.style.removeProperty('width'); \
                 document.getElementById('sheet').textContent = \
                   'div { width: 10px; } .active { width: 50px; }';",
            )
            .unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(50.0));
        assert!(
            runtime.host_state.borrow().style_resolver_generation
                > initial_resolver_generation,
            "style element content mutation must rebuild parsed stylesheets"
        );
    }

    #[test]
    fn stylesheet_tree_text_mutations_rebuild_resolver() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>.active { opacity: 0.3; }</style></head>
               <body><div id="target" class="active"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval("globalThis.target = document.getElementById('target')")
            .unwrap();
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(target).opacity"), "0.3");
        let initial_generation = runtime.host_state.borrow().style_resolver_generation;

        runtime
            .eval("document.querySelector('style').firstChild.data = '.active { opacity: 0.4; }'")
            .unwrap();
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(target).opacity"), "0.4");
        let text_generation = runtime.host_state.borrow().style_resolver_generation;
        assert!(text_generation > initial_generation);

        runtime.eval("document.head.textContent = ''").unwrap();
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(target).opacity"), "");
        assert!(runtime.host_state.borrow().style_resolver_generation > text_generation);
    }

    #[test]
    fn geometry_transition_invalidates_layout_between_frames() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { width: 10px; height: 10px; transition: width 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.target = document.getElementById('target'); \
                 target.offsetWidth; target.style.width = '30px'; \
                 getComputedStyle(target).width;",
            )
            .unwrap();

        runtime.run_animation_frame(250).unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(15.0));
        let first_sample_generation = runtime.host_state.borrow().layout_generation;

        runtime.run_animation_frame(250).unwrap();
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(20.0));
        assert!(
            runtime.host_state.borrow().layout_generation > first_sample_generation,
            "width sampling must invalidate cached layout geometry"
        );
    }

    #[test]
    fn timer_and_animation_frame_share_one_transition_clock() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { opacity: 0; transition: opacity 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.target = document.getElementById('target'); \
                 getComputedStyle(target).opacity; target.style.opacity = '1'; \
                 getComputedStyle(target).opacity;",
            )
            .unwrap();

        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        runtime.tick(16).unwrap();
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).opacity"),
            "0.516"
        );
        runtime
            .eval("requestAnimationFrame(timestamp => globalThis.sharedTimestamp = timestamp)")
            .unwrap();
        assert_eq!(runtime.run_animation_frame(16).unwrap(), 1);
        assert_eq!(
            runtime.eval("sharedTimestamp").unwrap().as_number(),
            Some(532.0)
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).opacity"),
            "0.532"
        );
    }

    #[test]
    fn css_transition_interpolates_transform_lists_with_relative_lengths() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { transform: none; transition: transform 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.target = document.getElementById('target'); \
                 getComputedStyle(target).transform; \
                 target.style.transform = 'translate(50%, 2rem) rotate(180deg)'; \
                 getComputedStyle(target).transform;",
            )
            .unwrap();

        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).transform"),
            "translate(25%, 1rem) rotate(1.570796rad)"
        );
        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).transform"),
            "translate(50%, 2rem) rotate(180deg)"
        );
    }

    #[test]
    fn css_transition_interpolates_mixed_px_and_percentage_lengths_for_layout() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                html, body { margin: 0; width: 200px; }
                #target { width: 10px; height: 10px; transition: width 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.target = document.getElementById('target'); \
                 target.offsetWidth; target.style.width = '50%'; \
                 getComputedStyle(target).width;",
            )
            .unwrap();

        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target).width"),
            "calc(5px + 25%)"
        );
        assert_eq!(runtime.eval("target.offsetWidth").unwrap().as_number(), Some(55.0));
    }

    #[test]
    fn css_transition_value_outranks_css_animation_value() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                @keyframes hold { from { opacity: 0.75; } to { opacity: 0.75; } }
                #target { opacity: 0; transition: opacity 1s linear;
                          animation: hold 1s infinite; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                "globalThis.target = document.getElementById('target'); \
                 getComputedStyle(target).opacity; \
                 target.style.animation = 'none'; target.style.opacity = '0.25'; \
                 getComputedStyle(target).opacity;",
            )
            .unwrap();

        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(target).opacity"), "0.5");
    }

    #[test]
    fn css_transition_dispatches_run_start_and_end_events() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { opacity: 0; transition: opacity 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                r#"globalThis.transitionEvents = [];
                   globalThis.transitionTarget = document.getElementById("target");
                   getComputedStyle(transitionTarget).opacity;
                   for (const type of ["transitionrun", "transitionstart", "transitionend", "transitioncancel"]) {
                     transitionTarget.addEventListener(type, event => transitionEvents.push([
                       event.type, event.propertyName, event.elapsedTime, event.pseudoElement
                     ].join(":")));
                   }
                   transitionTarget.style.opacity = "1";
                   getComputedStyle(transitionTarget).opacity;"#,
            )
            .unwrap();
        assert_eq!(
            eval_str(&mut runtime, "transitionEvents.join('|')"),
            "transitionrun:opacity:0:|transitionstart:opacity:0:"
        );

        assert_eq!(runtime.run_animation_frame(1_000).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "transitionEvents.join('|')"),
            "transitionrun:opacity:0:|transitionstart:opacity:0:|transitionend:opacity:1:"
        );
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(transitionTarget).opacity"), "1");
    }

    #[test]
    fn css_transition_dispatches_delayed_start_and_interruption_cancel() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { opacity: 0; transition: opacity 1s linear 500ms; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                r#"globalThis.transitionEvents = [];
                   globalThis.transitionTarget = document.getElementById("target");
                   getComputedStyle(transitionTarget).opacity;
                   for (const type of ["transitionrun", "transitionstart", "transitioncancel"]) {
                     transitionTarget.addEventListener(type, event => transitionEvents.push([
                       event.type, event.propertyName, event.elapsedTime
                     ].join(":")));
                   }
                   transitionTarget.style.opacity = "1";
                   getComputedStyle(transitionTarget).opacity;"#,
            )
            .unwrap();
        assert_eq!(
            eval_str(&mut runtime, "transitionEvents.join('|')"),
            "transitionrun:opacity:0"
        );

        assert_eq!(runtime.run_animation_frame(500).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "transitionEvents.join('|')"),
            "transitionrun:opacity:0|transitionstart:opacity:0"
        );

        assert_eq!(runtime.run_animation_frame(250).unwrap(), 0);
        runtime
            .eval(
                "getComputedStyle(transitionTarget).opacity; \
                 transitionTarget.style.opacity = '0'; \
                 getComputedStyle(transitionTarget).opacity;",
            )
            .unwrap();
        assert!(
            eval_str(&mut runtime, "transitionEvents.join('|')")
                .contains("transitioncancel:opacity:0.25")
        );
    }

    #[test]
    fn css_transition_cancels_when_its_element_is_detached() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head><style>
                #target { opacity: 0; transition: opacity 1s linear; }
            </style></head><body><div id="target"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                r#"globalThis.detachedEvents = [];
                   globalThis.detachedTarget = document.getElementById("target");
                   getComputedStyle(detachedTarget).opacity;
                   detachedTarget.addEventListener("transitioncancel", event =>
                     detachedEvents.push([event.propertyName, event.elapsedTime].join(":")));
                   detachedTarget.style.opacity = "1";
                   getComputedStyle(detachedTarget).opacity;"#,
            )
            .unwrap();
        assert_eq!(runtime.run_animation_frame(250).unwrap(), 0);
        runtime.eval("detachedTarget.remove()").unwrap();
        assert_eq!(runtime.run_animation_frame(250).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "detachedEvents.join('|')"),
            "opacity:0.5"
        );
    }

    #[test]
    fn css_transition_shorthand_matches_changed_longhands_from_initial_values() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><head></head><body><div id="target"
                style="transition: padding 10ms linear"></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                r#"globalThis.paddingEvents = [];
                   globalThis.paddingTarget = document.getElementById("target");
                   getComputedStyle(paddingTarget).paddingLeft;
                   paddingTarget.addEventListener("transitionend", event =>
                     paddingEvents.push(event.propertyName));
                   paddingTarget.style.padding = "10px";"#,
            )
            .unwrap();

        assert_eq!(runtime.run_animation_frame(0).unwrap(), 0);
        assert_eq!(runtime.run_animation_frame(10).unwrap(), 0);
        assert_eq!(
            eval_str(&mut runtime, "paddingEvents.sort().join('|')"),
            "padding-bottom|padding-left|padding-right|padding-top"
        );
    }

    #[test]
    fn css_transitions_run_concurrently_on_multiple_elements() {
        let document = crate::html::TreeBuilder::parse(
            r#"<html><body></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        runtime
            .eval(
                r#"globalThis.concurrentEvents = [];
                   for (let index = 0; index < 3; index++) {
                     const target = document.createElement("div");
                     target.style.cssText = "transition: all 10ms linear; padding: 1px";
                     document.body.appendChild(target);
                     getComputedStyle(target).paddingLeft;
                     target.addEventListener("transitionend", event =>
                       concurrentEvents.push(index + ":" + event.propertyName));
                     target.style.padding = "10px";
                   }"#,
            )
            .unwrap();
        assert_eq!(runtime.run_animation_frame(0).unwrap(), 0);
        assert_eq!(runtime.run_animation_frame(10).unwrap(), 0);
        assert_eq!(
            runtime.eval("concurrentEvents.length").unwrap().as_number(),
            Some(12.0)
        );
    }

    #[test]
    fn animation_frame_callbacks_preserve_order_and_can_cancel_same_frame() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.frameOrder = [];
                   requestAnimationFrame(() => { frameOrder.push("first"); cancelAnimationFrame(third); });
                   requestAnimationFrame(() => frameOrder.push("second"));
                   globalThis.third = requestAnimationFrame(() => frameOrder.push("third"));"#,
            )
            .unwrap();

        assert_eq!(runtime.run_animation_frame(16).unwrap(), 2);
        assert_eq!(
            runtime
                .eval("frameOrder.join(',')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "first,second"
        );
    }

    #[test]
    fn animation_frame_registered_during_callback_waits_for_next_frame() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.frameTimestamps = [];
                   requestAnimationFrame(first => {
                     frameTimestamps.push(first);
                     requestAnimationFrame(second => frameTimestamps.push(second));
                   });"#,
            )
            .unwrap();

        assert_eq!(runtime.run_animation_frame(16).unwrap(), 1);
        assert_eq!(runtime.eval("frameTimestamps.length").unwrap().as_number(), Some(1.0));
        assert_eq!(runtime.run_animation_frame(16).unwrap(), 1);
        assert!(runtime
            .eval("frameTimestamps.length === 2 && frameTimestamps[0] === 16 && frameTimestamps[1] === 32")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn timer_and_microtask_run_before_animation_frame() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.eventLoopOrder = [];
                   setTimeout(() => {
                     eventLoopOrder.push("timer");
                     Promise.resolve().then(() => eventLoopOrder.push("microtask"));
                     requestAnimationFrame(() => eventLoopOrder.push("animation-frame"));
                   }, 0);"#,
            )
            .unwrap();

        runtime.tick(0).unwrap();
        assert_eq!(runtime.run_animation_frame(16).unwrap(), 1);
        assert_eq!(
            runtime
                .eval("eventLoopOrder.join(',')")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "timer,microtask,animation-frame"
        );
    }

    #[test]
    fn every_timer_task_gets_a_microtask_checkpoint() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.eventLoopOrder = [];
                   setTimeout(() => {
                     eventLoopOrder.push("timer-1");
                     Promise.resolve().then(() => eventLoopOrder.push("microtask-1"));
                   }, 0);
                   setTimeout(() => eventLoopOrder.push("timer-2"), 0);"#,
            )
            .unwrap();

        runtime.tick(0).unwrap();
        assert_eq!(
            eval_str(&mut runtime, "eventLoopOrder.join(',')"),
            "timer-1,microtask-1,timer-2"
        );
    }

    #[test]
    fn timer_task_budget_does_not_discard_the_next_task() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.timerOrder = [];
                   setTimeout(() => timerOrder.push("first"), 0);
                   setTimeout(() => timerOrder.push("second"), 0);"#,
            )
            .unwrap();

        assert_eq!(runtime.run_timers(1, 1, 1), 1);
        assert_eq!(eval_str(&mut runtime, "timerOrder.join(',')"), "first");
        assert_eq!(runtime.run_timers(1, 1, 1), 1);
        assert_eq!(eval_str(&mut runtime, "timerOrder.join(',')"), "first,second");
    }

    #[test]
    fn navigation_becomes_ready_after_earlier_task_microtasks() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.eventLoopOrder = [];
                   setTimeout(() => {
                     eventLoopOrder.push("timer");
                     Promise.resolve().then(() => eventLoopOrder.push("microtask"));
                     location.assign("/next");
                   }, 0);"#,
            )
            .unwrap();

        assert!(runtime.take_navigation_requests().is_empty());
        runtime.tick(0).unwrap();
        assert_eq!(eval_str(&mut runtime, "eventLoopOrder.join(',')"), "timer,microtask");
        assert_eq!(
            runtime.take_navigation_requests(),
            vec![NavigationRequest::Navigate {
                url: "http://localhost/next".into(),
                replace: false,
            }]
        );
    }

    #[test]
    fn microtask_can_enqueue_a_navigation_task_in_the_same_pump() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("Promise.resolve().then(() => location.assign('/from-microtask'))")
            .unwrap();

        runtime.run_until_idle().unwrap();
        assert_eq!(
            runtime.take_navigation_requests(),
            vec![NavigationRequest::Navigate {
                url: "http://localhost/from-microtask".into(),
                replace: false,
            }]
        );
    }

    #[test]
    fn request_animation_frame_rejects_non_callable_callback() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime.eval("requestAnimationFrame(null)").is_err());
    }

    #[test]
    fn replace_child_works() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let old_span = NodeHandle::element("span");
        old_span.set_attribute("id", "old");
        div.append_child(old_span);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"
            const parent = document.querySelector('div');
            const oldChild = document.querySelector('#old');
            const newChild = document.createElement('b');
            newChild.id = 'new';
            parent.replaceChild(newChild, oldChild);
        "#,
            )
            .unwrap();

        let found_new = runtime
            .eval("document.querySelector('#new') !== null")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(found_new, "new child should be in DOM");

        let found_old = runtime.eval("document.querySelector('#old')").unwrap();
        assert!(found_old.is_null(), "old child should be removed");
    }

    // ── Timer callback / event-loop tests (issue 016-3) ──────────────────────

    #[test]
    fn set_timeout_function_callback_preserves_closure() {
        let mut runtime = JsRuntime::new().unwrap();
        // The captured `base` lives only in the IIFE scope; if the callback
        // were stringified via toString() this closure would be lost.
        runtime
            .eval(
                r#"
                globalThis.result = 0;
                (function () {
                    let base = 40;
                    setTimeout(function () { globalThis.result = base + 2; }, 0);
                })();
                "#,
            )
            .unwrap();

        // Not fired before the loop runs.
        assert_eq!(
            runtime.eval("globalThis.result").unwrap().as_number(),
            Some(0.0)
        );

        runtime.tick(0).unwrap();

        assert_eq!(
            runtime.eval("globalThis.result").unwrap().as_number(),
            Some(42.0),
            "callback should fire preserving captured closure variable"
        );
    }

    #[test]
    fn connected_dynamic_external_script_loads_executes_and_fires_load() {
        let mut runtime = runtime_from_html("<html><head></head><body></body></html>");
        runtime
            .eval(
                r#"const script = document.createElement("script");
                   script.src = "data:text/javascript,globalThis.dynamicScriptRan%20%3D%20true";
                   script.addEventListener("load", () => globalThis.dynamicScriptLoaded = true);
                   document.head.appendChild(script);"#,
            )
            .unwrap();

        assert_eq!(runtime.run_timers(100, 1, 10), 1);
        assert!(
            runtime
                .eval("dynamicScriptRan")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        assert!(
            runtime
                .eval("dynamicScriptLoaded")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn script_type_property_reflects_to_the_attribute() {
        let mut runtime = runtime_from_html("<html><head></head><body></body></html>");
        assert!(
            runtime
                .eval(
                    r#"(() => {
                    const script = document.createElement("script");
                    if (script.type !== "") return false;
                    script.type = "module";
                    // Evaluation is decided from the attribute, so assigning the
                    // property has to reach it.
                    if (script.getAttribute("type") !== "module") return false;
                    script.setAttribute("type", "text/javascript");
                    return script.type === "text/javascript" &&
                      document.createElement("script") instanceof HTMLScriptElement;
                  })()"#,
                )
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn dynamic_module_script_is_evaluated_as_a_module() {
        let mut runtime = runtime_from_html("<html><head></head><body></body></html>");
        // `export` is a syntax error in a classic script, so this only parses if
        // the dynamic path honours `type="module"` the way the parsed-document
        // path already does.
        runtime
            .eval(
                r#"const script = document.createElement("script");
                   script.type = "module";
                   script.src = "data:text/javascript,globalThis.moduleRan%20%3D%20true%3B%20export%20const%20answer%20%3D%2042%3B";
                   script.addEventListener("load", () => globalThis.moduleLoaded = true);
                   document.head.appendChild(script);"#,
            )
            .unwrap();

        runtime.run_until_idle().unwrap();

        assert_eq!(
            runtime.take_task_errors(),
            Vec::<String>::new(),
            "a module must not be evaluated as a classic script"
        );
        assert!(
            runtime
                .eval("globalThis.moduleRan === true && globalThis.moduleLoaded === true")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
    }

    #[test]
    fn dynamic_script_with_unsupported_type_is_neither_executed_nor_loaded() {
        let mut runtime = runtime_from_html("<html><head></head><body></body></html>");
        runtime
            .eval(
                r#"globalThis.ranAnyway = false;
                   globalThis.loadFired = false;
                   const script = document.createElement("script");
                   script.type = "application/json";
                   script.src = "data:text/javascript,globalThis.ranAnyway%20%3D%20true";
                   script.addEventListener("load", () => globalThis.loadFired = true);
                   document.head.appendChild(script);"#,
            )
            .unwrap();

        runtime.run_until_idle().unwrap();

        assert!(
            runtime
                .eval("globalThis.ranAnyway === false && globalThis.loadFired === false")
                .unwrap()
                .as_boolean()
                .unwrap(),
            "a script whose type is not executed must neither run nor fire load"
        );
        assert_eq!(runtime.take_task_errors(), Vec::<String>::new());
    }

    #[test]
    fn throwing_dynamic_script_does_not_abort_the_event_loop() {
        let mut runtime = runtime_from_html("<html><head></head><body></body></html>");
        runtime
            .eval(
                r#"globalThis.afterRan = false;
                   const script = document.createElement("script");
                   script.src = "data:text/javascript,throw%20new%20Error('boom')";
                   document.head.appendChild(script);
                   setTimeout(() => globalThis.afterRan = true, 0);"#,
            )
            .unwrap();

        // The failing script must not stop the loop, so the later timer still runs.
        runtime.tick(1).unwrap();

        assert!(
            runtime
                .eval("globalThis.afterRan === true")
                .unwrap()
                .as_boolean()
                .unwrap(),
            "a task queued after a failing script must still run"
        );
        let errors = runtime.take_task_errors();
        assert_eq!(errors.len(), 1, "expected one recorded error, got {errors:?}");
        assert!(
            errors[0].contains("dynamic script") && errors[0].contains("boom"),
            "the swallowed error must be reported: {errors:?}"
        );
        assert!(
            runtime.take_task_errors().is_empty(),
            "draining must clear the recorded errors"
        );
    }

    #[test]
    fn unfetchable_dynamic_script_fires_error_and_does_not_abort_the_event_loop() {
        let mut runtime = runtime_from_html("<html><head></head><body></body></html>");
        runtime
            .eval(
                r#"globalThis.loadFired = false;
                   globalThis.errorFired = false;
                   const script = document.createElement("script");
                   script.src = "data:text/javascript;base64,!!!not-base64!!!";
                   script.addEventListener("load", () => globalThis.loadFired = true);
                   script.addEventListener("error", () => globalThis.errorFired = true);
                   document.head.appendChild(script);"#,
            )
            .unwrap();

        runtime.run_until_idle().unwrap();

        assert!(
            runtime
                .eval("globalThis.errorFired === true && globalThis.loadFired === false")
                .unwrap()
                .as_boolean()
                .unwrap(),
            "a script that never arrived must fire error, not load"
        );
        let errors = runtime.take_task_errors();
        assert_eq!(errors.len(), 1, "expected one recorded error, got {errors:?}");
        assert!(
            errors[0].contains("failed to fetch"),
            "a fetch failure must be reported: {errors:?}"
        );
    }

    #[test]
    fn throwing_timer_callback_does_not_abort_the_event_loop() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.secondRan = false;
                   setTimeout(() => { throw new Error("timer boom"); }, 0);
                   setTimeout(() => globalThis.secondRan = true, 0);"#,
            )
            .unwrap();

        // Driving the loop through `run_until_idle` must behave like `run_timers`:
        // one failing callback cannot take the rest of the queue with it.
        runtime.tick(1).unwrap();

        assert!(
            runtime
                .eval("globalThis.secondRan === true")
                .unwrap()
                .as_boolean()
                .unwrap()
        );
        let errors = runtime.take_task_errors();
        assert_eq!(errors.len(), 1, "expected one recorded error, got {errors:?}");
        assert!(
            errors[0].contains("timer") && errors[0].contains("timer boom"),
            "the swallowed error must be reported: {errors:?}"
        );
    }

    #[test]
    fn recorded_task_errors_are_bounded() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"for (let index = 0; index < 200; index++) {
                     setTimeout(() => { throw new Error("repeat " + index); }, 0);
                   }"#,
            )
            .unwrap();

        runtime.tick(1).unwrap();

        let errors = runtime.take_task_errors();
        assert_eq!(
            errors.len(),
            MAX_TASK_ERRORS + 1,
            "the cap plus one suppression notice"
        );
        assert_eq!(
            errors[MAX_TASK_ERRORS],
            format!("{} further task errors suppressed", 200 - MAX_TASK_ERRORS),
            "the overflow must be counted, not just noted"
        );
        assert!(
            runtime.take_task_errors().is_empty(),
            "draining must clear the suppressed count too"
        );
    }

    #[test]
    fn external_scripts_share_page_http_cookies() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut second_request_has_cookie = false;
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let read = stream.read(&mut buffer).unwrap();
                    bytes.extend_from_slice(&buffer[..read]);
                    if read == 0 || bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&bytes);
                if request_index == 1 {
                    second_request_has_cookie = request
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("Cookie: page_session=ready"));
                }
                let body = if request_index == 0 {
                    "globalThis.firstExternalScript = true;"
                } else {
                    "globalThis.secondExternalScript = true;"
                };
                let cookie = if request_index == 0 {
                    "Set-Cookie: page_session=ready; Path=/\r\n"
                } else {
                    ""
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\n{cookie}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
            second_request_has_cookie
        });

        let html = r#"<html><head><script src="/first.js"></script><script src="/second.js"></script></head><body></body></html>"#;
        let document = crate::html::TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(document).unwrap();
        let base_url = format!("http://{address}/")
            .parse::<crate::http::Url>()
            .unwrap();
        assert!(runtime.execute_document_scripts(Some(&base_url)).is_empty());
        assert_eq!(
            runtime
                .eval("firstExternalScript && secondExternalScript")
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert!(
            handle.join().unwrap(),
            "the second script request must receive the first response's cookie"
        );
    }

    #[test]
    fn set_timeout_function_fires_only_at_delay_boundary() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("globalThis.fired = false; setTimeout(function () { globalThis.fired = true; }, 10);")
            .unwrap();

        runtime.tick(9).unwrap();
        assert_eq!(
            runtime.eval("globalThis.fired").unwrap().as_boolean(),
            Some(false),
            "timer must not fire before its delay elapses"
        );

        runtime.tick(1).unwrap();
        assert_eq!(
            runtime.eval("globalThis.fired").unwrap().as_boolean(),
            Some(true),
            "timer must fire once the delay is reached"
        );
    }

    #[test]
    fn set_timeout_callback_reschedule_chain_advances() {
        // Mirrors the Acid3 update() loop: each callback re-schedules the next.
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"
                globalThis.n = 0;
                function step() {
                    globalThis.n += 1;
                    if (globalThis.n < 5) {
                        setTimeout(step, 10);
                    }
                }
                setTimeout(step, 10);
                "#,
            )
            .unwrap();

        // Each tick(10) fires the currently-due `step`, which schedules the next.
        for _ in 0..6 {
            runtime.tick(10).unwrap();
        }

        assert_eq!(
            runtime.eval("globalThis.n").unwrap().as_number(),
            Some(5.0),
            "re-scheduled setTimeout chain should advance exactly 5 times"
        );
    }

    #[test]
    fn set_interval_repeats_and_clear_interval_stops() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("globalThis.count = 0; globalThis.tid = setInterval(function () { globalThis.count += 1; }, 5);")
            .unwrap();

        runtime.tick(5).unwrap();
        assert_eq!(
            runtime.eval("globalThis.count").unwrap().as_number(),
            Some(1.0)
        );
        runtime.tick(5).unwrap();
        assert_eq!(
            runtime.eval("globalThis.count").unwrap().as_number(),
            Some(2.0)
        );

        runtime.eval("clearInterval(globalThis.tid);").unwrap();
        runtime.tick(5).unwrap();
        runtime.tick(5).unwrap();
        assert_eq!(
            runtime.eval("globalThis.count").unwrap().as_number(),
            Some(2.0),
            "cleared interval must not fire again"
        );
    }

    #[test]
    fn clear_timeout_removes_pending_function_timer() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"
                globalThis.ran = false;
                globalThis.tid = setTimeout(function () { globalThis.ran = true; }, 10);
                clearTimeout(globalThis.tid);
                "#,
            )
            .unwrap();

        runtime.tick(20).unwrap();
        assert_eq!(
            runtime.eval("globalThis.ran").unwrap().as_boolean(),
            Some(false),
            "clearTimeout must cancel a pending function-callback timer"
        );
    }

    #[test]
    fn set_timeout_string_source_still_evaluates() {
        // The HTML string form must keep working alongside function callbacks.
        let mut runtime = JsRuntime::new().unwrap();
        runtime.eval("globalThis.marker = 0;").unwrap();
        runtime
            .eval(r#"setTimeout("globalThis.marker = 99;", 5);"#)
            .unwrap();

        runtime.tick(4).unwrap();
        assert_eq!(
            runtime.eval("globalThis.marker").unwrap().as_number(),
            Some(0.0)
        );
        runtime.tick(1).unwrap();
        assert_eq!(
            runtime.eval("globalThis.marker").unwrap().as_number(),
            Some(99.0),
            "string-source setTimeout must still evaluate as code"
        );
    }

    #[test]
    fn set_timeout_passes_extra_arguments_to_callback() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                "globalThis.sum = 0; setTimeout(function (a, b) { globalThis.sum = a + b; }, 0, 3, 4);",
            )
            .unwrap();

        runtime.tick(0).unwrap();
        assert_eq!(
            runtime.eval("globalThis.sum").unwrap().as_number(),
            Some(7.0),
            "extra setTimeout arguments must be forwarded to the callback"
        );
    }

    #[test]
    fn execute_document_scripts_then_pump_settles_settimeout_dom_change() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <div id="target"></div>
            <script>
                setTimeout(function () {
                    document.getElementById("target").setAttribute("data-done", "yes");
                }, 20);
            </script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();

        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        // Before pumping the timers, the DOM mutation has not happened yet.
        let target = doc.query_selector("#target").expect("find #target");
        assert_eq!(
            target
                .attributes()
                .unwrap_or_default()
                .get("data-done")
                .map(|s| s.as_str()),
            None,
            "attribute must not be set before the timer fires"
        );

        let tasks = runtime.run_timers(1_000, 10, 10_000);
        assert_eq!(tasks, 1, "exactly one timer callback should have run");

        let attrs = target.attributes().unwrap_or_default();
        assert_eq!(
            attrs.get("data-done").map(|s| s.as_str()),
            Some("yes"),
            "setTimeout callback should have mutated the DOM after run_timers"
        );
    }

    #[test]
    fn run_timers_caps_infinite_interval_by_virtual_time() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("globalThis.ticks = 0; setInterval(function () { globalThis.ticks += 1; }, 10);")
            .unwrap();

        // 100ms of virtual time at a 10ms step fires the interval exactly 10 times.
        let tasks = runtime.run_timers(100, 10, 10_000);
        assert_eq!(
            tasks, 10,
            "virtual-time budget should bound interval firings"
        );
        assert_eq!(
            runtime.eval("globalThis.ticks").unwrap().as_number(),
            Some(10.0),
            "infinite interval must stop at the virtual-time cap"
        );
    }

    // ── DOM2 Core / DOMException / namespaces (issue 016-12) ─────────────────

    /// Evaluates `source` (expected to be an IIFE returning 0 on success) and
    /// asserts the numeric result is 0, reporting the failing step otherwise.
    fn assert_js_ok(runtime: &mut JsRuntime, source: &str) {
        let result = runtime
            .eval(source)
            .unwrap_or_else(|e| panic!("eval failed: {e}"))
            .as_number()
            .expect("expected a numeric result");
        assert_eq!(result, 0.0, "JS check failed at step {result}");
    }

    #[test]
    fn create_element_rejects_invalid_names_with_code_5() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                function codeFor(name) {
                    try { document.createElement(name); return -1; }
                    catch (e) { return e.code; }
                }
                var invalid = ['<div>', '0div', 'di v', 'di<v', '-div', '.div'];
                for (var i = 0; i < invalid.length; i += 1) {
                    if (codeFor(invalid[i]) !== 5) return i + 1;
                }
                if (codeFor('div') !== -1) return 100;      // valid name must not throw
                if (codeFor('form div') !== 5) return 101; // NUL byte is invalid
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn xml_name_validation_does_not_depend_on_string_method_dispatch() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                // Models Boa 0.21.1 resolving this call site to `concat` after
                // core-js mutates built-in shapes (issue 057).
                String.prototype.codePointAt = String.prototype.concat;
                if (document.createElement('style').tagName !== 'STYLE') return 1;
                try { document.createElement('0style'); }
                catch (e) { return e.code === 5 ? 0 : 2; }
                return 3;
            })()
        "#,
        );
    }

    #[test]
    fn dom_exception_exposes_code_and_all_constants() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var e = new DOMException("boom", "InvalidCharacterError");
                if (e.code !== 5) return 1;
                if (e.message !== "boom") return 2;
                if (e.name !== "InvalidCharacterError") return 3;
                if (e.INDEX_SIZE_ERR !== 1) return 4;
                if (e.HIERARCHY_REQUEST_ERR !== 3) return 5;
                if (e.NAMESPACE_ERR !== 14) return 6;
                if (e.INVALID_ACCESS_ERR !== 15) return 7;
                if (DOMException.INVALID_CHARACTER_ERR !== 5) return 8;
                if (DOMException.TYPE_MISMATCH_ERR !== 17) return 9;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn create_element_ns_exposes_namespace_properties() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var el = document.createElementNS('http://ns.example.com/', 'prefix:localname');
                if (el.tagName !== 'prefix:localname') return 1;
                if (el.nodeName !== 'prefix:localname') return 2;
                if (el.prefix !== 'prefix') return 3;
                if (el.localName !== 'localname') return 4;
                if (el.namespaceURI !== 'http://ns.example.com/') return 5;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn create_element_ns_validates_qualified_names() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                function codeFor(ns, name) {
                    try { document.createElementNS(ns, name); return -1; }
                    catch (e) { return e.code; }
                }
                if (codeFor(null, '<div>') !== 5) return 1;           // invalid char
                if (codeFor(null, '0div') !== 5) return 2;
                if (codeFor('http://example.com/', 'di<v') !== 5) return 3;
                if (codeFor(null, ':div') !== 14) return 4;           // malformed qname
                if (codeFor(null, 'd:iv') !== 14) return 5;           // prefix, null ns
                if (codeFor('http://example.com/', 'xml:test') !== 14) return 6;
                if (codeFor('http://example.com/', 'xmlns:test') !== 14) return 7;
                if (codeFor('http://www.w3.org/2000/xmlns/', 'x:test') !== 14) return 8;
                if (codeFor('http://www.w3.org/2000/xmlns/', 'xmlns:test') !== -1) return 9; // valid
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn implementation_create_document_type_rejects_malformed_qname() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                try {
                    document.implementation.createDocumentType('a:', '', '');
                    return 1;
                } catch (e) {
                    if (e.code !== e.NAMESPACE_ERR) return 2;
                    if (e.INVALID_ACCESS_ERR !== 15) return 3;
                }
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn iframe_implementation_created_doctype_keeps_its_owner_document() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var frame = document.createElement('iframe');
                document.body.appendChild(frame);
                var foreign = frame.contentDocument;
                var doctype = foreign.implementation.createDocumentType('html', '', '');
                if (doctype.ownerDocument !== foreign) return 1;
                if (doctype.ownerDocument === document) return 2;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn document_type_identifiers_are_exposed_and_serialized() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime
            .eval(
                r#"
                var publicType = document.implementation.createDocumentType(
                    'html', '-//W3C//DTD XHTML 1.0 Strict//EN',
                    'http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd');
                var publicDoc = document.implementation.createDocument(null, 'html', publicType);
                var systemType = document.implementation.createDocumentType(
                    'example', '', 'example.dtd');
                var systemDoc = document.implementation.createDocument(null, 'example', systemType);
                [publicType.publicId, publicType.systemId, publicDoc.innerHTML,
                 systemType.publicId, systemType.systemId, systemDoc.innerHTML].join('|');
                "#,
            )
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();

        assert_eq!(
            result,
            "-//W3C//DTD XHTML 1.0 Strict//EN|http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd|<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\"><html></html>||example.dtd|<!DOCTYPE example SYSTEM \"example.dtd\"><example></example>"
        );
    }

    #[test]
    fn implementation_create_document_builds_independent_document() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var empty = document.implementation.createDocument(null, null, null);
                if (empty.nodeType !== 9 || empty.documentElement !== null) return 1;
                if (empty.defaultView !== null) return 2;

                var doc = document.implementation.createDocument('urn:test', 'p:root', null);
                var root = doc.documentElement;
                if (!root || root.namespaceURI !== 'urn:test') return 3;
                if (root.prefix !== 'p' || root.localName !== 'root') return 4;
                if (root.ownerDocument !== doc) return 5;

                var child = doc.createElement('child');
                var text = doc.createTextNode('hello');
                child.id = 'created-only';
                child.appendChild(text);
                root.appendChild(child);
                if (doc.getElementById('created-only') !== child) return 6;
                if (document.getElementById('created-only') !== null) return 7;

                var range = doc.createRange();
                range.setStartBefore(child);
                range.setEndAfter(child);
                if (range.startContainer !== root || range.startOffset !== 0) return 8;
                if (range.endContainer !== root || range.endOffset !== 1) return 9;

                var style = getComputedStyle(root);
                if (style === null || typeof style !== 'object') return 10;

                var dt = document.implementation.createDocumentType('p:root', 'pub', 'sys');
                var withType = document.implementation.createDocument('urn:test', 'p:root', dt);
                if (withType.firstChild !== dt || withType.documentElement.previousSibling !== dt) return 11;
                if (dt.ownerDocument !== withType) return 12;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn implementation_create_html_document_is_independent() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var originalBody = document.body;
                var originalHtml = document.documentElement.innerHTML;
                var doc = document.implementation.createHTMLDocument("Detached title");
                if (doc === document || doc.nodeType !== 9) return 1;
                if (!doc.doctype || doc.doctype.name !== "html") return 2;
                if (!doc.documentElement || !doc.head || !doc.body) return 3;
                if (doc.title !== "Detached title") return 4;
                if (doc.defaultView !== null) return 5;
                if (doc.body.ownerDocument !== doc) return 6;
                if (doc.URL !== "about:blank") return 7;
                if (doc.documentURI !== "about:blank") return 8;

                doc.body.innerHTML = "<form></form><form></form>";
                if (doc.body.children.length !== 2) return 9;
                if (document.body !== originalBody) return 10;
                if (document.documentElement.innerHTML !== originalHtml) return 11;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn document_title_setter_creates_a_missing_title_element() {
        let mut runtime = runtime_from_html("<html><body><p>content</p></body></html>");
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const stray = document.createElement("title");
                stray.textContent = "Stray";
                document.body.appendChild(stray);
                const before = document.title;
                document.title = "Created";
                return [
                  before,
                  document.title,
                  document.head.firstChild.tagName,
                  document.head.firstChild.textContent,
                  document.body.firstChild.tagName,
                  stray.textContent,
                ].join(":");
            })()"#,
        );
        assert_eq!(result, ":Created:TITLE:Created:P:Stray");
    }

    #[test]
    fn implementation_create_document_validates_qualified_name() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                function codeFor(ns, name) {
                    try { document.implementation.createDocument(ns, name, null); return -1; }
                    catch (e) { return e.code; }
                }
                if (codeFor(null, '<root>') !== 5) return 1;
                if (codeFor(null, ':root') !== 14) return 2;
                if (codeFor(null, 'p:root') !== 14) return 3;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn append_child_cycle_throws_hierarchy_request_error() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var e = null;
                try {
                    document.body.appendChild(document.documentElement);
                } catch (err) {
                    e = err;
                }
                if (!e) return 1;
                if (e.HIERARCHY_REQUEST_ERR !== 3) return 2;
                if (e.code !== 3) return 3;
                return 0;
            })()
        "#,
        );
    }

    // ── Events: createEvent / initUIEvent (issue 016-12) ─────────────────────

    #[test]
    fn create_event_ui_event_supports_init_ui_event_and_dispatch() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var container = document.createElement('div');
                var child = document.createElement('span');
                container.appendChild(child);
                document.body.appendChild(container);
                var count = 0;
                var seenDetail = null;
                container.addEventListener('test', function (event) {
                    count += 1;
                    seenDetail = event.detail;
                }, false);
                var event = document.createEvent('UIEvents');
                event.initUIEvent('test', true, false, null, 6);
                var returned = child.dispatchEvent(event);
                if (returned !== true) return 1;
                if (count !== 1) return 2;
                if (seenDetail !== 6) return 3;
                if (event.type !== 'test') return 4;
                if (event.bubbles !== true) return 5;
                return 0;
            })()
        "#,
        );
    }

    // ── Table / form / input / button / label / meta / select (issue 016-13) ─

    #[test]
    fn table_caption_head_foot_accessors() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var t = document.createElement('table');
                if (t.caption) return 1;
                if (!t.tBodies || t.tBodies.length !== 0) return 2;
                if (!t.rows || t.rows.length !== 0) return 3;
                if (t.tFoot || t.tHead) return 4;
                var caption = t.createCaption();
                var thead = t.createTHead();
                var tfoot = t.createTFoot();
                if (t.caption !== caption) return 5;
                if (t.tHead !== thead) return 6;
                if (t.tFoot !== tfoot) return 7;
                if (t.childNodes.length !== 3) return 8;
                t.deleteCaption();
                t.deleteTHead();
                t.deleteTFoot();
                if (t.caption || t.tHead || t.tFoot) return 9;
                if (t.hasChildNodes()) return 10;
                if (t.childNodes.length !== 0) return 11;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn table_insert_row_placement_and_bounds() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                // Empty table: insertRow auto-creates a tbody and puts the row in it.
                var t = document.createElement('table');
                var r0 = t.insertRow(0);
                if (t.tBodies.length !== 1) return 1;
                if (t.tBodies[0].firstChild !== r0) return 2;
                if (t.rows.length !== 1) return 3;
                if (r0.tagName !== 'TR') return 4;

                // Row count 1, insert at -1 (append) goes to the last section (tbody).
                var r1 = t.insertRow(-1);
                if (t.rows.length !== 2) return 5;
                if (t.rows[1] !== r1) return 6;
                if (t.tBodies[0].childNodes.length !== 2) return 7;

                // Insert at index 0 positions before the current first row, same section.
                var r2 = t.insertRow(0);
                if (t.rows[0] !== r2) return 8;
                if (t.tBodies[0].firstChild !== r2) return 9;
                if (t.rows.length !== 3) return 10;

                // Out-of-range indices throw IndexSizeError (code 1).
                function codeFor(i) {
                    try { t.insertRow(i); return -1; } catch (e) { return e.code; }
                }
                if (codeFor(-2) !== 1) return 11;
                if (codeFor(99) !== 1) return 12;

                // A table that already has an (empty) tbody reuses it rather than
                // creating a second one.
                var t2 = document.createElement('table');
                t2.appendChild(document.createElement('tbody'));
                var x = t2.insertRow(0);
                if (t2.tBodies.length !== 1) return 13;
                if (t2.tBodies[0].firstChild !== x) return 14;

                // deleteRow(-1) removes the last row; out of range throws.
                t2.deleteRow(-1);
                if (t2.rows.length !== 0) return 15;
                var t3 = document.createElement('table');
                var d = t3.insertRow(0);
                function delCode(i) {
                    try { t3.deleteRow(i); return -1; } catch (e) { return e.code; }
                }
                if (delCode(5) !== 1) return 16;
                t3.deleteRow(0);
                if (t3.rows.length !== 0) return 17;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn table_row_index_spans_sections_in_collection_order() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                // Build <table><thead><tr/></thead><tbody><tr/></tbody><tfoot><tr/></tfoot>
                // plus a stray tr child of the table itself.
                var t = document.createElement('table');
                var head = t.createTHead();
                var foot = t.createTFoot();
                var body = document.createElement('tbody');
                t.insertBefore(body, foot);
                var hr = head.insertRow(0);
                var br = body.insertRow(0);
                var fr = foot.insertRow(0);
                var direct = document.createElement('tr');
                t.insertBefore(direct, foot); // table-direct row, after tbody

                // rows collection order: thead, then body(table-direct + tbody in
                // tree order), then tfoot.
                var rows = t.rows;
                if (rows.length !== 4) return 1;
                if (rows[0] !== hr) return 2;
                if (rows[1] !== br) return 3;   // tbody row precedes direct row (tree order)
                if (rows[2] !== direct) return 4;
                if (rows[3] !== fr) return 5;

                // rowIndex is the index in that collection.
                if (hr.rowIndex !== 0) return 6;
                if (br.rowIndex !== 1) return 7;
                if (direct.rowIndex !== 2) return 8;
                if (fr.rowIndex !== 3) return 9;

                // sectionRowIndex is per-section.
                if (hr.sectionRowIndex !== 0) return 10;
                if (br.sectionRowIndex !== 0) return 11;
                if (fr.sectionRowIndex !== 0) return 12;

                // A row that is a direct child of the table (no thead/tbody/tfoot
                // section between it and the table) has no section row index,
                // even though it still participates in table.rows (rowIndex 2).
                if (direct.sectionRowIndex !== -1) return 15;

                // A detached row reports -1 for rowIndex.
                var orphan = document.createElement('tr');
                if (orphan.rowIndex !== -1) return 13;
                if (orphan.sectionRowIndex !== -1) return 14;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn table_section_rows_insert_delete_and_row_cells() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var tbody = document.createElement('tbody');
                if (tbody.rows.length !== 0) return 1;
                var r0 = tbody.insertRow(0);
                var r1 = tbody.insertRow(-1);
                var rMid = tbody.insertRow(1);
                if (tbody.rows.length !== 3) return 2;
                if (tbody.rows[0] !== r0) return 3;
                if (tbody.rows[1] !== rMid) return 4;
                if (tbody.rows[2] !== r1) return 5;

                // Section insertRow bounds.
                function codeFor(i) {
                    try { tbody.insertRow(i); return -1; } catch (e) { return e.code; }
                }
                if (codeFor(99) !== 1) return 6;

                tbody.deleteRow(1);
                if (tbody.rows.length !== 2) return 7;
                if (tbody.rows[0] !== r0) return 8;
                if (tbody.rows[1] !== r1) return 9;

                // tr.cells returns td and th in tree order.
                var tr = tbody.rows[0];
                tr.appendChild(document.createElement('th'));
                tr.appendChild(document.createElement('td'));
                tr.appendChild(document.createTextNode('ignored'));
                tr.appendChild(document.createElement('td'));
                if (tr.cells.length !== 3) return 10;
                if (tr.cells[0].tagName !== 'TH') return 11;
                if (tr.cells[1].tagName !== 'TD') return 12;
                if (tr.cells[2].tagName !== 'TD') return 13;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn acid3_test50_row_construction_and_ordering() {
        // Deterministic regression mirroring Acid3 test 50: section insertRow,
        // rowIndex/sectionRowIndex, and rows ordering after appending into thead.
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var table = document.createElement('table');
                table.appendChild(document.createElement('tbody'));
                var tr1 = document.createElement('tr');
                table.appendChild(tr1);
                table.appendChild(document.createElement('caption'));
                table.appendChild(document.createElement('thead'));
                table.insertBefore(table.firstChild.nextSibling, null);
                table.replaceChild(table.firstChild, table.lastChild);
                var tr2 = table.tBodies[0].insertRow(0);
                if (table.tBodies[0].rows[0].rowIndex !== 0) return 1;
                if (table.tBodies[0].rows[0].sectionRowIndex !== 0) return 2;
                if (table.childNodes.length !== 3) return 3;
                if (!table.caption) return 4;
                if (!table.tHead) return 5;
                if (table.tFoot) return 6;
                if (table.tBodies.length !== 1) return 7;
                if (table.rows.length !== 1) return 8;
                if (tr1.parentNode) return 9;
                if (table.caption !== table.createCaption()) return 10;
                if (table.tFoot !== null) return 11;
                if (table.tHead !== table.createTHead()) return 12;
                if (table.createTFoot() !== table.tFoot) return 13;
                table.tHead.appendChild(tr1);
                if (table.rows[0] !== table.tHead.firstChild) return 14;
                if (table.rows.length !== 2) return 15;
                if (table.rows[1] !== table.tBodies[0].firstChild) return 16;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn acid3_test51_row_ordering_across_sections() {
        // Deterministic regression mirroring Acid3 test 51: cross-section row
        // ordering, table insertRow with existing sections, and tree-order
        // traversal via getElementsByTagName.
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var table = document.createElement('table');
                var rows = [
                    document.createElement('tr'),
                    document.createElement('tr'),
                    document.createElement('tr'),
                    document.createElement('tr'),
                    document.createElement('tr'),
                    table.insertRow(0),
                    table.createTFoot().insertRow(0)
                ];
                rows[6].parentNode.appendChild(rows[0]);
                table.appendChild(rows[1]);
                table.insertBefore(document.createElement('thead'), table.firstChild);
                table.firstChild.appendChild(rows[2]);
                rows[2].parentNode.appendChild(rows[3]);
                rows[4].appendChild(rows[5].parentNode);
                table.insertRow(0);
                table.tFoot.appendChild(rows[6]);
                if (table.rows.length !== 6) return 1;
                if (table.getElementsByTagName('tr').length !== 6) return 2;
                if (table.childNodes.length !== 3) return 3;
                if (table.childNodes[0] !== table.tHead) return 4;
                var trs = table.getElementsByTagName('tr');
                if (trs[0] !== table.tHead.childNodes[0]) return 5;
                if (trs[1] !== table.tHead.childNodes[1]) return 6;
                if (trs[1] !== rows[2]) return 7;
                if (trs[2] !== table.tHead.childNodes[2]) return 8;
                if (trs[2] !== rows[3]) return 9;
                if (table.childNodes[1] !== table.tFoot) return 10;
                if (trs[3] !== table.tFoot.childNodes[0]) return 11;
                if (trs[3] !== rows[0]) return 12;
                if (trs[4] !== table.tFoot.childNodes[1]) return 13;
                if (trs[4] !== rows[6]) return 14;
                if (trs[5] !== table.childNodes[2]) return 15;
                if (trs[5] !== rows[1]) return 16;
                if (table.tBodies.length !== 0) return 17;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn acid3_test29_cloned_table_section_rows_and_cells() {
        // Regression for Acid3 test 29: a cloned <table> must expose section
        // rows and row cells so that tBodies[0].rows[0].cells[0] resolves.
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var t1 = document.createElement('table');
                var tbody = document.createElement('tbody');
                var tr = document.createElement('tr');
                var td = document.createElement('td');
                td.appendChild(document.createElement('p'));
                tr.appendChild(td);
                tbody.appendChild(tr);
                t1.appendChild(tbody);
                var t2 = t1.cloneNode(true);
                if (t2.tBodies[0].rows[0].cells[0].firstChild.tagName !== 'P') return 1;
                if (t2.tBodies[0].rows[0].cells[0].firstChild.childNodes.length !== 0) return 2;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn event_handler_idl_attribute_registers_and_replaces_listener() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var d = document.createElement('div');
                var n = 0;
                d.onclick = function () { n += 1; };
                if (typeof d.onclick !== 'function') return 1;
                d.click();
                if (n !== 1) return 2;
                // Reassigning replaces the previous handler (no double-firing).
                var m = 0;
                d.onclick = function () { m += 1; };
                d.click();
                if (n !== 1) return 3;
                if (m !== 1) return 4;
                // Assigning null removes the handler.
                d.onclick = null;
                if (d.onclick !== null) return 5;
                d.click();
                if (m !== 1) return 6;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn submit_button_click_fires_cancelable_submit_on_owning_form() {
        // Regression for Acid3 test 54: input.click() on a submit control must
        // synchronously dispatch a submit event to the owning form.
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var f = document.createElement('form');
                var i = document.createElement('input');
                f.appendChild(i);
                i.type = 'submit';
                var called = 0;
                var wasCancelable = false;
                f.onsubmit = function (e) {
                    wasCancelable = e.cancelable;
                    e.preventDefault();
                    called += 1;
                };
                i.click();
                if (called !== 1) return 1;
                if (!wasCancelable) return 2;

                // A reset button dispatches a reset event instead.
                var r = document.createElement('input');
                r.type = 'reset';
                f.appendChild(r);
                var resetCalled = 0;
                f.onreset = function () { resetCalled += 1; };
                r.click();
                if (resetCalled !== 1) return 3;

                // A plain text input's click does not submit the form.
                var t = document.createElement('input');
                t.type = 'text';
                f.appendChild(t);
                called = 0;
                t.click();
                if (called !== 0) return 4;

                // A submit control with no owning form does not throw.
                var lone = document.createElement('button');
                lone.click();

                // addEventListener('submit', ...) also receives the event, and a
                // nested submit button still finds its ancestor form.
                var wrap = document.createElement('div');
                var i2 = document.createElement('input');
                i2.type = 'submit';
                wrap.appendChild(i2);
                var f2 = document.createElement('form');
                f2.appendChild(wrap);
                var listened = 0;
                f2.addEventListener('submit', function (e) { e.preventDefault(); listened += 1; });
                i2.click();
                if (listened !== 1) return 5;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn disabled_form_control_click_runs_no_activation_behavior() {
        // A disabled submit/reset control has no activation behavior: neither the
        // native click() nor a synthetic click dispatched through dispatchEvent
        // submits or resets its owning form.
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var f = document.createElement('form');
                var submits = 0;
                var resets = 0;
                f.addEventListener('submit', function (e) { e.preventDefault(); submits += 1; });
                f.addEventListener('reset', function () { resets += 1; });

                // Baseline: an ENABLED submit button submits the form even when
                // the click arrives as a synthetic event dispatched through
                // dispatchEvent (not only via the native click() method).
                var enabled = document.createElement('button');
                enabled.type = 'submit';
                f.appendChild(enabled);
                enabled.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
                if (submits !== 1) return 1;

                // The `disabled` IDL attribute reflects the content attribute on
                // a <button>, and a disabled submit button does not submit the
                // form via the native click().
                var b = document.createElement('button');
                b.type = 'submit';
                b.disabled = true;
                if (b.disabled !== true) return 2;
                if (!b.hasAttribute('disabled')) return 3;
                f.appendChild(b);
                submits = 0;
                b.click();
                if (submits !== 0) return 4;

                // ...nor via a synthetic click dispatched directly through
                // dispatchEvent (which does reach __runActivationBehavior).
                b.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
                if (submits !== 0) return 5;

                // A disabled reset control likewise runs no activation behavior,
                // by either path.
                var r = document.createElement('input');
                r.type = 'reset';
                r.disabled = true;
                f.appendChild(r);
                r.click();
                r.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
                if (resets !== 0) return 6;

                // Re-enabling the button restores its activation behavior.
                b.disabled = false;
                submits = 0;
                b.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
                if (submits !== 1) return 7;

                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn input_value_is_dirty_and_preserves_lone_surrogate() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var i = document.createElement('input');
                i.name = 'first';
                i.type = 'text';
                i.value = 'test';
                if (i.getAttribute('name') !== 'first') return 1;
                if (i.name !== 'first') return 2;
                if (i.hasAttribute('value')) return 3;   // value is not reflected
                if (i.value !== 'test') return 4;
                // A lone surrogate must survive round-tripping through the value IDL attribute.
                var before = String.fromCharCode(0xd863) + 'text';
                i.value = before;
                var after = i.value;
                if (!(after === before && before.length === 5)) return 5;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn text_control_editing_updates_selection_and_input_events() {
        let mut runtime = runtime_from_html(
            r#"<html><body><input id="field" value="abcd"><textarea id="area">hello</textarea></body></html>"#,
        );
        assert!(runtime
            .eval(
                r#"(() => {
                    const field = document.getElementById("field");
                    globalThis.editEvents = [];
                    for (const type of ["beforeinput", "input", "change"]) {
                      field.addEventListener(type, event => editEvents.push(
                        [type, event.inputType || "", event.data === null ? "null" : event.data].join(":")));
                    }
                    field.focus();
                    field.setSelectionRange(1, 3, "forward");
                    __omoikane_dispatch_keyboard_input("keydown", { key: "X", text: "X" });
                    const inserted = field.value === "aXd" && field.selectionStart === 2 &&
                      field.selectionEnd === 2 && field.selectionDirection === "none";
                    __omoikane_dispatch_keyboard_input("keydown", { key: "Backspace" });
                    const deleted = field.value === "ad" && field.selectionStart === 1;
                    field.blur();

                    const area = document.getElementById("area");
                    area.value = "hello";
                    area.setSelectionRange(1, 4, "backward");
                    const textareaSelection = area.selectionStart === 1 && area.selectionEnd === 4 &&
                      area.selectionDirection === "backward";
                    area.select();
                    return inserted && deleted && textareaSelection && area.selectionStart === 0 &&
                      area.selectionEnd === 5 && editEvents.join("|") ===
                      "beforeinput:insertText:X|input:insertText:X|beforeinput:deleteContentBackward:null|input:deleteContentBackward:null|change::";
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn text_control_editing_respects_cancelation_readonly_and_maxlength() {
        let mut runtime = runtime_from_html(
            r#"<html><body><input id="field" value="ab" maxlength="3"><input id="readonly" value="locked" readonly></body></html>"#,
        );
        assert!(runtime
            .eval(
                r#"(() => {
                    const field = document.getElementById("field");
                    field.focus();
                    field.setSelectionRange(2, 2);
                    __omoikane_dispatch_keyboard_input("keydown", { key: "c", text: "c" });
                    __omoikane_dispatch_keyboard_input("keydown", { key: "d", text: "d" });
                    const maxlengthHeld = field.value === "abc" && field.selectionStart === 3;
                    field.addEventListener("beforeinput", event => event.preventDefault(), { once: true });
                    __omoikane_dispatch_keyboard_input("keydown", { key: "Backspace" });
                    const beforeInputCanceled = field.value === "abc";
                    field.addEventListener("keydown", event => event.preventDefault(), { once: true });
                    __omoikane_dispatch_keyboard_input("keydown", { key: "Backspace" });
                    const keydownCanceled = field.value === "abc";
                    field.disabled = true;
                    __omoikane_dispatch_keyboard_input("keydown", { key: "x", text: "x" });
                    const disabledHeld = field.value === "abc";
                    const readonly = document.getElementById("readonly");
                    readonly.focus();
                    readonly.select();
                    __omoikane_dispatch_keyboard_input("keydown", { key: "x", text: "x" });
                    return maxlengthHeld && beforeInputCanceled && keydownCanceled && disabledHeld &&
                      readonly.value === "locked" && readonly.selectionStart === 0 && readonly.selectionEnd === 6;
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn text_control_navigation_and_forward_delete_move_the_caret() {
        let mut runtime = runtime_from_html(r#"<html><body><input id="field" value="abcd"></body></html>"#);
        assert!(runtime
            .eval(
                r#"(() => {
                    const field = document.getElementById("field");
                    field.focus();
                    field.setSelectionRange(2, 2);
                    __omoikane_dispatch_keyboard_input("keydown", { key: "Delete" });
                    const deleted = field.value === "abd" && field.selectionStart === 2;
                    __omoikane_dispatch_keyboard_input("keydown", { key: "Backspace" });
                    const backspaced = field.value === "ad" && field.selectionStart === 1;
                    __omoikane_dispatch_keyboard_input("keydown", { key: "Home" });
                    const home = field.selectionStart === 0;
                    __omoikane_dispatch_keyboard_input("keydown", { key: "ArrowRight" });
                    const right = field.selectionStart === 1;
                    __omoikane_dispatch_keyboard_input("keydown", { key: "End" });
                    const end = field.selectionStart === 2;
                    __omoikane_dispatch_keyboard_input("keydown", { key: "ArrowLeft", shiftKey: true });
                    return deleted && backspaced && home && right && end &&
                      field.selectionStart === 1 && field.selectionEnd === 2 &&
                      field.selectionDirection === "backward";
                })()"#,
            )
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn text_control_selection_invalidates_layout_without_resampling_styles() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>input { width: 120px; }</style></head><body><input id="field" value="abc"></body></html>"#,
        );
        runtime
            .eval("globalThis.field = document.getElementById('field'); document.body.offsetWidth")
            .unwrap();
        let layout_generation = runtime.host_state.borrow().layout_generation;
        let resolver_generation = runtime.host_state.borrow().style_resolver_generation;

        runtime
            .eval("field.focus(); field.setSelectionRange(1, 2); document.body.offsetWidth")
            .unwrap();

        assert!(runtime.host_state.borrow().layout_generation > layout_generation);
        assert_eq!(
            runtime.host_state.borrow().style_resolver_generation,
            resolver_generation,
            "caret and selection changes must reuse the existing style resolver"
        );
    }

    #[test]
    fn form_elements_named_and_indexed_access() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var f = document.createElement('form');
                var i = document.createElement('input');
                i.name = 'first';
                f.appendChild(i);
                if (f.elements === f) return 1;
                if (f.elements.length !== 1) return 2;
                if (f.elements[0] !== i) return 3;
                if (f.elements.first !== i) return 4;
                if (f.elements.second !== null) return 5;
                i.name = 'second';
                if (f.elements.second !== i) return 6;
                if (f.elements.first !== null) return 7;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn document_forms_indexed_named_and_live() {
        use crate::html::TreeBuilder;
        // Two forms in tree order: one named via `name`, one via `id`.
        let doc = TreeBuilder::parse(
            r#"<html><body><form name="alpha"><input name="x"></form><div><form id="beta"></form></div></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var forms = document.forms;
                if (forms.length !== 2) return 1;
                if (forms[0].getAttribute('name') !== 'alpha') return 2;
                if (forms[1].getAttribute('id') !== 'beta') return 3;
                if (forms.alpha !== forms[0]) return 4;      // named access by name
                if (forms.beta !== forms[1]) return 5;       // named access by id
                if (forms.namedItem('alpha') !== forms[0]) return 6;
                if (forms.item(1) !== forms[1]) return 7;
                if (forms.alpha.elements[0].getAttribute('name') !== 'x') return 8;
                if (forms.missing != null) return 9;         // absent name is nullish
                // Liveness: a retained collection reflects later mutations.
                var extra = document.createElement('form');
                document.body.appendChild(extra);
                if (forms.length !== 3) return 10;
                if (forms[2] !== extra) return 11;
                if (forms[3] != null) return 12;             // out of range is null
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn document_links_filters_href_and_keeps_tree_order() {
        use crate::html::TreeBuilder;
        // Only <a>/<area> with an href attribute are links, in tree order.
        let doc = TreeBuilder::parse(
            r#"<html><body><map name="m"><area href="a" alt=""><area alt="no-href"></map><a href="b">link</a><a>no href</a><a name="anchor">named only</a></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var links = document.links;
                if (links.length !== 2) return 1;
                if (links[0].tagName !== 'AREA') return 2;
                if (links[0].getAttribute('href') !== 'a') return 3;
                if (links[1].tagName !== 'A') return 4;
                if (links[1].getAttribute('href') !== 'b') return 5;
                // The href-less <area> and <a>, and the <a name> without href, are excluded.
                for (var i = 0; i < links.length; i++) {
                    if (!links[i].hasAttribute('href')) return 6;
                }
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn document_images_and_anchors_collections() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><img id="i1" src="x"><img name="i2" src="y"><a name="top">anchor</a><a href="z">link</a></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                if (document.images.length !== 2) return 1;
                if (document.images[0].getAttribute('id') !== 'i1') return 2;
                if (document.images.i1 !== document.images[0]) return 3;  // by id
                if (document.images.i2 !== document.images[1]) return 4;  // by name
                // anchors are only <a> elements carrying a name attribute.
                if (document.anchors.length !== 1) return 5;
                if (document.anchors[0].getAttribute('name') !== 'top') return 6;
                if (document.anchors.top !== document.anchors[0]) return 7;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn document_collections_scoped_to_each_document() {
        use crate::html::TreeBuilder;
        // A form inside an iframe's contentDocument must not appear in the main
        // document's forms collection, and vice versa.
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval("globalThis.frame = document.getElementById('f');")
            .unwrap();
        // The iframe's blank document is materialized as a zero-delay task.
        pump_zero_delay_tasks(&mut runtime);
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var mainForm = document.createElement('form');
                mainForm.setAttribute('name', 'mainForm');
                document.body.appendChild(mainForm);
                var sub = frame.contentDocument;
                var subForm = sub.createElement('form');
                sub.body.appendChild(subForm);
                if (document.forms.length !== 1) return 1;
                if (document.forms[0] !== mainForm) return 2;
                if (sub.forms.length !== 1) return 3;
                if (sub.forms[0] !== subForm) return 4;
                if (document.forms[0] === sub.forms[0]) return 5;
                if (document.forms.mainForm !== mainForm) return 6;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn acid3_test4_and_test5_document_forms_and_links_regression() {
        use crate::html::TreeBuilder;
        // Mirrors the Acid3 body fragment exercised by tests 4/5: a named form
        // with a control, plus an <area href> and an <a href> as document.links.
        let doc = TreeBuilder::parse(
            r#"<html><body><map name=""><area href="" shape="rect" coords="2,2,4,4" alt="x"><form action="" name="form"><input type="hidden"></form></map><p id="instructions">To pass the test,<span></span> a browser must, like <a href="reference.html">this reference rendering</a>.</p></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                // Acid3 test 4: document.forms[0] and document.forms.form.elements[0].
                if (document.forms.length !== 1) return 1;
                var form = document.forms[0];
                if (form.tagName !== 'FORM') return 2;
                if (document.forms.form !== form) return 3;
                if (form.elements[0].tagName !== 'INPUT') return 4;
                if (document.forms.form.elements[0] !== form.elements[0]) return 5;
                // Acid3 test 5: document.links[1] is the <a href> after the <area href>.
                if (document.links.length !== 2) return 6;
                if (document.links[0].tagName !== 'AREA') return 7;
                if (document.links[1].tagName !== 'A') return 8;
                if (document.links[1].getAttribute('href') !== 'reference.html') return 9;
                if (document.links[1].firstChild.nodeType !== 3) return 10;
                if (document.links[1].firstChild.data !== 'this reference rendering') return 11;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn button_type_defaults_to_submit() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var button = document.createElement('button');
                if (button.type !== 'submit') return 1;
                button.setAttribute('type', 'button');
                if (button.type !== 'button') return 2;
                button.removeAttribute('type');
                if (button.type !== 'submit') return 3;
                button.setAttribute('value', 'apple');
                button.appendChild(document.createTextNode('banana'));
                if (button.value !== 'apple') return 4;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn label_html_for_and_meta_http_equiv_reflect_content_attributes() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var label = document.createElement('label');
                label.htmlFor = 'jars';
                if (label.htmlFor !== 'jars') return 1;
                if (label.getAttribute('for') !== 'jars') return 2;
                if (label.hasAttribute('htmlFor')) return 3;
                if ('for' in label) return 4;
                var meta = document.createElement('meta');
                meta.setAttribute('http-equiv', 'boxes');
                if (meta.httpEquiv !== 'boxes') return 5;
                meta.httpEquiv = 'cans';
                if (meta.getAttribute('http-equiv') !== 'cans') return 6;
                if (meta.hasAttribute('httpEquiv')) return 7;
                return 0;
            })()
        "#,
        );
    }

    #[test]
    fn select_add_and_options_collection() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_js_ok(
            &mut runtime,
            r#"
            (function () {
                var s = document.createElement('select');
                var o = document.createElement('option');
                s.add(o, null);
                if (s.firstChild !== o) return 1;
                if (s.childNodes.length !== 1) return 2;
                if (s.options.length !== 1) return 3;
                if (s.options[0] !== o) return 4;
                return 0;
            })()
        "#,
        );
    }

    // ── ECMAScript Annex B (issue 016-6) ─────────────────────────────────────

    #[test]
    fn string_substr_supports_negative_start() {
        let mut runtime = JsRuntime::new().unwrap();
        let matches = runtime
            .eval(r#""scathing".substr(-7, 3) === "cat""#)
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(
            matches,
            "String.prototype.substr must handle negative start"
        );
    }

    #[test]
    fn run_timers_caps_infinite_interval_by_task_count() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("globalThis.ticks = 0; setInterval(function () { globalThis.ticks += 1; }, 1);")
            .unwrap();

        // Effectively unlimited virtual time, but the task cap stops it.
        let tasks = runtime.run_timers(u64::MAX, 1, 50);
        assert_eq!(
            tasks, 50,
            "task-count cap should bound total timer executions"
        );
        assert_eq!(
            runtime.eval("globalThis.ticks").unwrap().as_number(),
            Some(50.0),
            "infinite interval must stop at the task-count cap"
        );
    }

    // -- getComputedStyle (issue 016-8) + layout metrics (issue 044-2) --------

    /// Evaluates `expr`, coercing the result to a JS string, and returns it as a
    /// Rust `String`.
    fn eval_str(runtime: &mut JsRuntime, expr: &str) -> String {
        let wrapped = format!("String({expr})");
        runtime
            .eval(&wrapped)
            .unwrap_or_else(|e| panic!("eval `{expr}` failed: {e}"))
            .as_string()
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_else(|| panic!("`{expr}` did not evaluate to a string"))
    }

    /// Evaluates `expr` and returns the result as an `f64`.
    fn eval_num(runtime: &mut JsRuntime, expr: &str) -> f64 {
        runtime
            .eval(expr)
            .unwrap_or_else(|e| panic!("eval `{expr}` failed: {e}"))
            .as_number()
            .unwrap_or_else(|| panic!("`{expr}` did not evaluate to a number"))
    }

    fn runtime_from_html(html: &str) -> JsRuntime {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(html).document();
        JsRuntime::with_document(doc).unwrap()
    }

    #[test]
    fn get_computed_style_returns_cascade_white_space() {
        let html = r#"<html><head><style>
            #target { white-space: pre-wrap; }
        </style></head><body><p id="target">hi</p></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // camelCase property access.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').whiteSpace"
            ),
            "pre-wrap"
        );
        // getPropertyValue with the CSS (kebab-case) name.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').getPropertyValue('white-space')"
            ),
            "pre-wrap"
        );
    }

    #[test]
    fn detached_element_has_no_computed_style_properties() {
        let mut runtime = runtime_from_html("<html><body></body></html>");

        // Browsers expose no resolved properties for an element outside the
        // document tree; its inline declaration remains available via `style`.
        assert_eq!(
            eval_str(
                &mut runtime,
                "(() => { const el = document.createElement('div'); \
                 el.setAttribute('style', 'color: red'); \
                 return getComputedStyle(el).getPropertyValue('color'); })()"
            ),
            ""
        );
    }

    #[test]
    fn get_computed_style_drops_invalid_white_space_keyword() {
        // Mirrors Acid3 test 0: an invalid later declaration must not override a
        // valid earlier one.
        let html = r#"<html><head><style>
            #target { white-space: pre-wrap; white-space: x-bogus; }
        </style></head><body><p id="target">hi</p></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').whiteSpace"
            ),
            "pre-wrap",
            "invalid `x-bogus` declaration must be discarded, keeping `pre-wrap`"
        );
    }

    #[test]
    fn get_computed_style_drops_invalid_cursor_keyword() {
        // Acid3 test 47 control case: `cursor: bogus` is invalid, so computed
        // `cursor` falls back to the initial value `auto`.
        let html = r#"<html><head><style>
            #target { cursor: bogus; }
        </style></head><body><div id="target"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').cursor"
            ),
            "auto",
            "invalid `cursor: bogus` must be discarded, leaving the initial `auto`"
        );
    }

    #[test]
    fn get_computed_style_returns_valid_cursor_keyword() {
        // A valid keyword is returned verbatim (Acid3 test 47 positive cases).
        let html = r#"<html><head><style>
            #target { cursor: pointer; }
        </style></head><body><div id="target"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').cursor"
            ),
            "pointer"
        );
    }

    #[test]
    fn get_computed_style_inline_cursor_bogus_is_dropped() {
        // The inline getComputedStyle override must apply the same validation as
        // the cascade: an invalid inline `cursor` is dropped, not applied raw.
        let html = r#"<html><head></head>
            <body><div id="target" style="cursor: bogus"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').cursor"
            ),
            "auto",
            "invalid inline `cursor: bogus` must be dropped, leaving the initial `auto`"
        );
    }

    #[test]
    fn get_computed_style_inline_cursor_valid_is_applied() {
        // A valid inline `cursor` still overrides the cascade and is normalized.
        let html = r#"<html><head><style>
            #target { cursor: pointer; }
        </style></head><body><div id="target" style="cursor: MOVE"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').cursor"
            ),
            "move",
            "valid inline `cursor` must override the cascade and be lowercased"
        );
    }

    #[test]
    fn get_computed_style_exposes_float_as_css_float() {
        let html = r#"<html><head><style>
            #target { float: right; }
        </style></head><body><div id="target"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').cssFloat"
            ),
            "right"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').float"
            ),
            "right"
        );
    }

    #[test]
    fn get_computed_style_exposes_z_index_like_selector_test() {
        // Reproduces the shape of Acid3's selectorTest: a universal rule sets a
        // baseline z-index and a specific rule raises it. getComputedStyle must
        // report the numeric value (never "auto").
        let html = r#"<html><head><style>
            * { z-index: 0; position: absolute; }
            #target { z-index: 3; }
        </style></head><body><div id="target"></div><div id="other"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "document.defaultView.getComputedStyle(document.getElementById('target'), '').zIndex"
            ),
            "3"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "document.defaultView.getComputedStyle(document.getElementById('other'), '').zIndex"
            ),
            "0"
        );
    }

    #[test]
    fn get_computed_style_inline_style_overrides_cascade() {
        let html = r#"<html><head><style>
            #target { color: red; }
        </style></head><body><div id="target" style="color: blue"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').color"
            ),
            "rgb(0, 0, 255)",
            "inline style must win over the cascaded stylesheet rule"
        );
    }

    #[test]
    fn get_computed_style_is_shared_across_default_view_window_and_global() {
        let html = r#"<html><head><style>
            #target { white-space: pre-wrap; }
        </style></head><body><p id="target">hi</p></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle === window.getComputedStyle && \
                 window.getComputedStyle === document.defaultView.getComputedStyle"
            ),
            "true",
            "global / window / defaultView getComputedStyle must be the same function"
        );
        // And all three produce the same value.
        assert_eq!(
            eval_str(
                &mut runtime,
                "[getComputedStyle, window.getComputedStyle, document.defaultView.getComputedStyle]\
                 .map(f => f(document.getElementById('target'), '').whiteSpace).join(',')"
            ),
            "pre-wrap,pre-wrap,pre-wrap"
        );
    }

    #[test]
    fn get_computed_style_recomputes_last_child_after_removal() {
        // Acid3 test 0 core: removing the final child makes the previous element
        // the new `:last-child`, which must recompute to `pre-wrap`.
        let html = r#"<html><head><style>
            #keep:last-child { white-space: pre-wrap; }
        </style></head><body><div id="parent"><p id="keep">a</p><p id="last">b</p></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // Before removal `#keep` is not the last child, so no pre-wrap.
        assert_ne!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('keep'), '').whiteSpace"
            ),
            "pre-wrap"
        );

        // Remove the last element; forced reflow must observe the new structure.
        runtime
            .eval("(function(){ var l = document.getElementById('last'); l.parentNode.removeChild(l); })()")
            .unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('keep'), '').whiteSpace"
            ),
            "pre-wrap",
            "`:last-child` must re-match after the final child is removed"
        );
    }

    #[test]
    fn document_write_marks_style_dirty_so_getcomputedstyle_recomputes() {
        // Regression (integration review): a getComputedStyle query caches the
        // style resolver and clears the dirty flag. A subsequent
        // `document.write` that adds a <style> must re-dirty that cache so the
        // next query observes the newly written rule instead of returning the
        // stale pre-write result.
        let html = r#"<html><head></head><body><p id="target">hi</p></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // Prime the cache: no rule applies yet, so this is not pre-wrap. The
        // query computes and caches the resolver (dirty = false).
        assert_ne!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').whiteSpace"
            ),
            "pre-wrap",
            "no white-space rule applies before the write"
        );

        // Write a <style> targeting #target. Outside script execution the
        // fragment appends to <body>; collect_inline_stylesheets still picks it
        // up. This must mark the cached resolver dirty.
        runtime
            .eval("document.write('<style>#target { white-space: pre-wrap; }</style>')")
            .unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').whiteSpace"
            ),
            "pre-wrap",
            "getComputedStyle after document.write must observe the newly written rule"
        );
    }

    #[test]
    fn document_open_marks_style_dirty_so_getcomputedstyle_recomputes() {
        // Regression (integration review): document.open() empties the document,
        // removing the <style> that fed the cached resolver. Without marking the
        // style cache dirty, a getComputedStyle query that already primed the
        // cache would keep reporting the pre-open rule.
        let html = r#"<html><head><style>
            #target { white-space: pre-wrap; }
        </style></head><body><p id="target">hi</p></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // Retain the element so it stays queryable after the tree is emptied,
        // then prime the cache (dirty = false).
        runtime
            .eval("globalThis.__t = document.getElementById('target');")
            .unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(globalThis.__t, '').whiteSpace"
            ),
            "pre-wrap",
            "the <style> rule applies before document.open()"
        );

        // Empty the document via document.open(); the <style> is now gone.
        runtime.eval("document.open();").unwrap();

        assert_ne!(
            eval_str(
                &mut runtime,
                "getComputedStyle(globalThis.__t, '').whiteSpace"
            ),
            "pre-wrap",
            "after document.open() the removed <style> must no longer apply"
        );
    }

    #[test]
    fn get_computed_style_has_trap_tolerates_symbols() {
        // Regression (integration review): the `has` trap must guard symbols the
        // same way `get` does, so `Symbol.x in getComputedStyle(el)` neither
        // throws nor is run through the CSS-name mapping. The underlying
        // declaration has no such symbol key, so membership is false.
        let html = r#"<html><body><div id="target"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "Symbol.iterator in getComputedStyle(document.getElementById('target'), '')"
            ),
            "false",
            "Symbol.iterator must not be a member and must not throw"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "Symbol.toStringTag in getComputedStyle(document.getElementById('target'), '')"
            ),
            "false",
            "Symbol.toStringTag must not be a member and must not throw"
        );
    }

    #[test]
    fn get_computed_style_is_read_only() {
        // Regression (integration review): getComputedStyle returns a read-only
        // CSSStyleDeclaration. Assigning to a property must be ignored and must
        // never pollute the underlying declaration, so a later read still
        // reports the computed value.
        let html = r#"<html><head><style>
            #target { white-space: pre-wrap; }
        </style></head><body><p id="target">hi</p></body></html>"#;
        let mut runtime = runtime_from_html(html);

        let result = eval_str(
            &mut runtime,
            r#"(function() {
                var cs = getComputedStyle(document.getElementById('target'), '');
                // Overwriting a known computed property must be ignored.
                try { cs.whiteSpace = 'nowrap'; } catch (e) { /* strict-mode TypeError ok */ }
                // Writing an unknown property must not land on the underlying decl.
                try { cs.notARealProp = 'polluted'; } catch (e) {}
                return cs.whiteSpace + '|' + cs.notARealProp;
            })()"#,
        );

        assert_eq!(
            result, "pre-wrap|",
            "writes to a computed style must be ignored and leave the declaration unpolluted"
        );
    }

    #[test]
    fn window_metrics_default_to_bootstrap_viewport() {
        // Without an explicit set_viewport the DOM bootstrap's 1280x720 defaults
        // apply and are visible to scripts unchanged.
        let mut runtime = runtime_from_html("<html><body></body></html>");
        assert_eq!(eval_num(&mut runtime, "window.innerWidth"), 1280.0);
        assert_eq!(eval_num(&mut runtime, "window.innerHeight"), 720.0);
        assert_eq!(eval_num(&mut runtime, "window.outerWidth"), 1280.0);
        assert_eq!(eval_num(&mut runtime, "window.outerHeight"), 720.0);
        assert_eq!(eval_num(&mut runtime, "screen.width"), 1280.0);
        assert_eq!(eval_num(&mut runtime, "screen.height"), 720.0);
        assert_eq!(eval_num(&mut runtime, "screen.availWidth"), 1280.0);
        assert_eq!(eval_num(&mut runtime, "screen.availHeight"), 720.0);
    }

    #[test]
    fn set_viewport_syncs_window_and_screen_metrics() {
        // set_viewport must wire the render viewport into the script-visible
        // window metrics so `window.innerWidth`/`innerHeight` (and `screen.*`)
        // agree with the render viewport, and match the `vw`/`vh` units resolved
        // by getComputedStyle against the same viewport. Regression: the fixed
        // 1280x720 bootstrap defaults previously leaked through.
        let html = r#"<html><head><style>
            #target { width: 100vw; height: 100vh; }
        </style></head><body><div id="target"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);
        runtime.set_viewport(800.0, 600.0);

        assert_eq!(eval_num(&mut runtime, "window.innerWidth"), 800.0);
        assert_eq!(eval_num(&mut runtime, "window.innerHeight"), 600.0);
        assert_eq!(eval_num(&mut runtime, "window.outerWidth"), 800.0);
        assert_eq!(eval_num(&mut runtime, "window.outerHeight"), 600.0);
        assert_eq!(eval_num(&mut runtime, "screen.width"), 800.0);
        assert_eq!(eval_num(&mut runtime, "screen.height"), 600.0);
        assert_eq!(eval_num(&mut runtime, "screen.availWidth"), 800.0);
        assert_eq!(eval_num(&mut runtime, "screen.availHeight"), 600.0);

        // getComputedStyle's `vw`/`vh` resolution must agree with window.inner*.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').width"
            ),
            "800px",
            "100vw must resolve against the same viewport window.innerWidth reports"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').height"
            ),
            "600px",
            "100vh must resolve against the same viewport window.innerHeight reports"
        );
    }

    #[test]
    fn set_viewport_clamps_non_finite_and_negative_dimensions() {
        // Regression (integration review): raw f64/f32 viewport dimensions were
        // stored unchecked and flowed into StyleResolver::set_viewport and
        // layout_tree. A NaN/+inf/negative value must clamp to 0 (a safe finite
        // dimension) without panicking, and the script-visible metrics must stay
        // consistent with the vw/vh resolution.
        let html = r#"<html><head><style>
            #target { width: 100vw; height: 100vh; }
        </style></head><body><div id="target"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // NaN width and +inf height both clamp to 0.
        runtime.set_viewport(f32::NAN, f32::INFINITY);
        assert_eq!(eval_num(&mut runtime, "window.innerWidth"), 0.0);
        assert_eq!(eval_num(&mut runtime, "window.innerHeight"), 0.0);
        assert_eq!(eval_num(&mut runtime, "screen.width"), 0.0);
        assert_eq!(eval_num(&mut runtime, "screen.height"), 0.0);
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').width"
            ),
            "0px",
            "100vw against a clamped-to-zero viewport resolves to 0px, not NaN"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').height"
            ),
            "0px",
            "100vh against a clamped-to-zero viewport resolves to 0px, not NaN"
        );

        // Negative dimensions clamp to 0 as well.
        runtime.set_viewport(-100.0, -50.0);
        assert_eq!(eval_num(&mut runtime, "window.innerWidth"), 0.0);
        assert_eq!(eval_num(&mut runtime, "window.innerHeight"), 0.0);

        // A subsequent valid viewport is unaffected by the clamp.
        runtime.set_viewport(640.0, 480.0);
        assert_eq!(eval_num(&mut runtime, "window.innerWidth"), 640.0);
        assert_eq!(eval_num(&mut runtime, "window.innerHeight"), 480.0);
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').width"
            ),
            "640px",
            "a valid viewport still resolves 100vw normally after a clamped one"
        );
    }

    #[test]
    fn get_bounding_client_rect_returns_block_geometry() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 50px; }
        </style></head><body><div id="box"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().width"
            ),
            100.0
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().height"
            ),
            50.0
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().left"
            ),
            0.0
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().top"
            ),
            0.0
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().right"
            ),
            100.0
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().bottom"
            ),
            50.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetWidth"),
            100.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetHeight"),
            50.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetLeft"),
            0.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetTop"),
            0.0
        );
    }

    #[test]
    fn bounding_client_rect_includes_transforms_but_offset_size_does_not() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 50px; transform-origin: 0 0;
                   transform: translateX(100px) rotate(90deg); }
        </style></head><body><div id="box"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);
        let expression = r#"(() => {
            const box = document.getElementById('box');
            const rect = box.getBoundingClientRect();
            return [rect.left, rect.top, rect.width, rect.height,
                    box.offsetWidth, box.offsetHeight].join('|');
        })()"#;

        assert_eq!(eval_str(&mut runtime, expression), "50|0|50|100|100|50");
    }

    #[test]
    fn bounding_client_rect_composes_ancestor_and_element_transforms() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #parent { width: 20px; height: 20px; transform: translateX(30px); }
            #child { width: 10px; height: 10px; transform: scale(2); }
        </style></head><body><div id="parent"><div id="child"></div></div></body></html>"#;
        let mut runtime = runtime_from_html(html);
        let expression = r#"(() => {
            const rect = document.getElementById('child').getBoundingClientRect();
            return [rect.left, rect.top, rect.width, rect.height].join('|');
        })()"#;

        // scale(2) around the child's center expands -5..15, then the parent
        // translation moves that bounding box to 25..45.
        assert_eq!(eval_str(&mut runtime, expression), "25|-5|20|20");
    }

    #[test]
    fn inline_style_is_the_source_for_computed_width_and_layout() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 80px; }
        </style></head><body><div id="box" style="width: 100px"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetWidth"),
            100.0
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('box')).width"
            ),
            "100px"
        );

        runtime
            .eval("document.getElementById('box').setAttribute('style', 'width: 250px');")
            .unwrap();
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetWidth"),
            250.0
        );

        runtime
            .eval("document.getElementById('box').removeAttribute('style');")
            .unwrap();
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetWidth"),
            80.0
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('box')).width"
            ),
            "80px"
        );
    }

    #[test]
    fn computed_style_preserves_inline_data_uri() {
        let html = r#"<html><body><div id="box"
            style="background-image: url(data:image/png;base64,AAA)"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('box')).getPropertyValue('background-image')"
            ),
            "url(data:image/png;base64,AAA)"
        );
    }

    #[test]
    fn inline_important_beats_author_important_in_computed_style() {
        let html = r#"<html><head><style>
            #box { color: red !important; }
        </style></head><body><div id="box" style="color: blue !important"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('box')).color"
            ),
            "rgb(0, 0, 255)"
        );
    }

    #[test]
    fn client_metrics_account_for_padding_and_border() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 50px; padding: 10px; border: 5px solid black; }
        </style></head><body><div id="box"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // clientWidth/Height = content + padding (no border).
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').clientWidth"),
            120.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').clientHeight"),
            70.0
        );
        // offsetWidth/Height = content + padding + border.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetWidth"),
            130.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetHeight"),
            80.0
        );
        // clientTop/Left = border widths.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').clientTop"),
            5.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').clientLeft"),
            5.0
        );
        // Border box starts at the origin.
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().left"
            ),
            0.0
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('box').getBoundingClientRect().top"
            ),
            0.0
        );
    }

    #[test]
    fn root_client_metrics_use_viewport_instead_of_document_height() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            body { height: 1400px; }
        </style></head><body></body></html>"#;
        let mut runtime = runtime_from_html(html);
        runtime.set_viewport(800.0, 600.0);

        assert_eq!(
            eval_num(&mut runtime, "document.documentElement.clientWidth"),
            800.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.documentElement.clientHeight"),
            600.0
        );
        assert!(
            eval_num(&mut runtime, "document.documentElement.scrollHeight") > 600.0,
            "root scrollHeight must still expose the full document height"
        );
    }

    #[test]
    fn window_scroll_api_clamps_syncs_aliases_dispatches_and_updates_rects() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            body { width: 600px; height: 1000px; }
            #flow { position: absolute; left: 120px; top: 300px; width: 10px; height: 10px; }
            #fixed { position: fixed; left: 5px; top: 20px; width: 10px; height: 10px; }
            #fixed-child { display: block; width: 5px; height: 5px; }
        </style></head><body><div id="flow"></div><div id="fixed"><span id="fixed-child"></span></div></body></html>"#;
        let mut runtime = runtime_from_html(html);
        runtime.set_viewport(200.0, 100.0);
        runtime
            .eval("globalThis.scrollEvents = 0; addEventListener('scroll', () => scrollEvents++);")
            .unwrap();

        runtime.eval("scrollTo(50, 200)").unwrap();
        assert_eq!(eval_num(&mut runtime, "scrollEvents"), 0.0);
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "[scrollX,scrollY,pageXOffset,pageYOffset,scrollEvents].join('|')"
            ),
            "50|200|50|200|1"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "[document.getElementById('flow').getBoundingClientRect().left,document.getElementById('flow').getBoundingClientRect().top,document.getElementById('fixed').getBoundingClientRect().left,document.getElementById('fixed').getBoundingClientRect().top,document.getElementById('fixed-child').getBoundingClientRect().top].join('|')"
            ),
            "70|100|5|20|20"
        );

        runtime.eval("scrollBy({ left: 25, top: 50 })").unwrap();
        runtime.eval("scroll({ top: 275 })").unwrap();
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(eval_str(&mut runtime, "[scrollX,scrollY,scrollEvents].join('|')"), "75|275|2");

        // An unchanged request emits no event; non-finite coordinates normalize to zero.
        runtime.eval("scrollTo({ left: 75, top: 275 })").unwrap();
        runtime.eval("scrollTo(NaN, Infinity)").unwrap();
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(eval_str(&mut runtime, "[scrollX,scrollY,scrollEvents].join('|')"), "0|0|3");

        runtime.eval("scrollTo(1e9, 1e9)").unwrap();
        assert!(runtime
            .eval("scrollX === document.documentElement.scrollWidth - innerWidth && scrollY === document.documentElement.scrollHeight - innerHeight")
            .unwrap()
            .as_boolean()
            .unwrap());
    }

    #[test]
    fn viewport_resize_reclamps_window_scroll() {
        let html = r#"<html><head><style>* { margin: 0; padding: 0; } body { height: 500px; }</style></head><body></body></html>"#;
        let mut runtime = runtime_from_html(html);
        runtime.set_viewport(100.0, 100.0);
        runtime
            .eval("globalThis.scrollEvents = 0; addEventListener('scroll', () => scrollEvents++); scrollTo(0, 350)")
            .unwrap();
        runtime.set_viewport(100.0, 300.0);
        assert_eq!(eval_str(&mut runtime, "[scrollY,scrollEvents].join('|')"), "200|0");
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(eval_num(&mut runtime, "scrollEvents"), 1.0);

        runtime.eval("document.body.style.height = '320px'").unwrap();
        assert_eq!(eval_str(&mut runtime, "[scrollY,scrollEvents].join('|')"), "20|1");
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(eval_num(&mut runtime, "scrollEvents"), 2.0);
    }

    #[test]
    fn window_scroll_state_is_isolated_per_runtime() {
        let html = r#"<html><head><style>* { margin: 0; } body { height: 500px; }</style></head><body></body></html>"#;
        let mut first = runtime_from_html(html);
        let mut second = runtime_from_html(html);
        first.set_viewport(100.0, 100.0);
        second.set_viewport(100.0, 100.0);
        first.eval("scrollTo(0, 120)").unwrap();
        assert_eq!(eval_num(&mut first, "scrollY"), 120.0);
        assert_eq!(eval_num(&mut second, "scrollY"), 0.0);
    }

    #[test]
    fn layout_metrics_force_reflow_after_class_change() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 50px; }
            #box.wide { width: 250px; }
        </style></head><body><div id="box"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetWidth"),
            100.0
        );

        // Adding the `wide` class matches a wider rule; the next metric query
        // must reflect the change via a forced reflow.
        runtime
            .eval("document.getElementById('box').className = 'wide';")
            .unwrap();

        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').offsetWidth"),
            250.0,
            "forced reflow must observe the class-driven width change"
        );
    }

    #[test]
    fn scroll_size_encloses_overflowing_child() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 50px; overflow: hidden; }
            #tall { width: 100px; height: 200px; }
        </style></head><body><div id="box"><div id="tall"></div></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // clientHeight is the (clipped) padding box; scrollHeight encloses the
        // taller child.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').clientHeight"),
            50.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').scrollHeight"),
            200.0,
            "scrollHeight must enclose the overflowing child"
        );
    }

    #[test]
    fn scroll_size_and_clamp_include_end_padding_but_not_border() {
        let html = r#"<html><head><style>
            * { margin: 0; }
            #box { width: 100px; height: 50px; padding: 7px;
                   border: 3px solid black; overflow: hidden; }
            #child { width: 300px; height: 200px; }
        </style></head><body><div id="box"><div id="child"></div></div></body></html>"#;
        let mut runtime = runtime_from_html(html);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const box = document.getElementById("box");
                const child = document.getElementById("child");
                const overflowing = [box.clientWidth, box.clientHeight,
                  box.scrollWidth, box.scrollHeight].join(",");
                box.scrollLeft = 99999;
                box.scrollTop = 99999;
                const clamped = [box.scrollLeft, box.scrollTop].join(",");
                child.style.width = "50px";
                child.style.height = "20px";
                const fitting = [box.scrollWidth, box.scrollHeight,
                  box.scrollLeft, box.scrollTop].join(",");
                return [overflowing, clamped, fitting].join("|");
            })()"#,
        );
        assert_eq!(result, "114,64,314,214|200,150|114,64,0,0");
    }

    #[test]
    fn scroll_size_encloses_overflowing_grandchild() {
        // Regression: scrollWidth/scrollHeight must recurse into descendants, not
        // just the direct children. The grandchild `#wide` (300x200) overflows the
        // 100x100 `#mid`, which in turn overflows the 100x100 `#box`. The old
        // direct-child-only scan would report 100x100 (the size of `#mid`).
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 100px; }
            #mid { width: 100px; height: 100px; }
            #wide { width: 300px; height: 200px; }
        </style></head><body>
            <div id="box"><div id="mid"><div id="wide"></div></div></div>
        </body></html>"#;
        let mut runtime = runtime_from_html(html);

        // clientWidth/clientHeight stay at the padding box of `#box`.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').clientWidth"),
            100.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').clientHeight"),
            100.0
        );
        // scrollWidth/scrollHeight enclose the overflowing grandchild's border box.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').scrollWidth"),
            300.0,
            "scrollWidth must enclose the overflowing grandchild, not just direct children"
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').scrollHeight"),
            200.0,
            "scrollHeight must enclose the overflowing grandchild, not just direct children"
        );
    }

    #[test]
    fn scroll_size_stops_at_clipping_descendant() {
        // A descendant that clips its overflow (`overflow: hidden`) establishes its
        // own scroll region: its clipped content must not spill into an ancestor's
        // scrollable area. `#box` should see only `#clip`'s 100x100 border box,
        // while `#clip` itself reports the overflowing `#wide` (300x200).
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 100px; }
            #clip { width: 100px; height: 100px; overflow: hidden; }
            #wide { width: 300px; height: 200px; }
        </style></head><body>
            <div id="box"><div id="clip"><div id="wide"></div></div></div>
        </body></html>"#;
        let mut runtime = runtime_from_html(html);

        // The outer box does not count the grandchild clipped by `#clip`.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').scrollWidth"),
            100.0,
            "content clipped by a descendant must not extend the ancestor's scrollWidth"
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').scrollHeight"),
            100.0,
            "content clipped by a descendant must not extend the ancestor's scrollHeight"
        );
        // The clipping element itself still reports its overflowing direct child.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('clip').scrollWidth"),
            300.0,
            "the clipping container reports its own overflowing content"
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('clip').scrollHeight"),
            200.0,
            "the clipping container reports its own overflowing content"
        );
    }

    #[test]
    fn get_client_rects_distinguishes_zero_size_from_no_box() {
        // CSSOM: a rendered box (even zero-sized) yields one client rect, while an
        // element that generates no box yields none. Uses a stylesheet rule for
        // `display: none` so the distinction does not depend on inline-style
        // layout application.
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #zero { width: 0; height: 0; }
            #gone { display: none; }
        </style></head><body>
            <div id="zero"></div>
            <div id="gone"></div>
        </body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('zero').getClientRects().length"
            ),
            1.0,
            "a zero-sized rendered box must still return one client rect"
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('gone').getClientRects().length"
            ),
            0.0,
            "an element that generates no box must return an empty client-rect list"
        );
        // The zero-sized box's single rect reports zero width/height.
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('zero').getClientRects()[0].width"
            ),
            0.0
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "document.getElementById('zero').getClientRects()[0].height"
            ),
            0.0
        );
    }

    #[test]
    fn replaced_elements_expose_fragment_layout_metrics() {
        let html = r#"<html><head><style>
            * { margin: 0; }
            img { width: 100px; height: 50px; }
            #inline { padding: 10px; border: 5px solid black; }
            #block { display: block; }
            #absolute { position: absolute; left: 30px; top: 40px; }
            #gone { display: none; }
        </style></head><body>
            <img id="inline" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC">
            <img id="block" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC">
            <img id="absolute" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC">
            <img id="gone" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC">
        </body></html>"#;
        let mut runtime = runtime_from_html(html);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const values = id => {
                    const element = document.getElementById(id);
                    const rect = element.getBoundingClientRect();
                    return [rect.width, rect.height, element.offsetWidth,
                      element.offsetHeight, element.clientWidth, element.clientHeight,
                      element.clientLeft, element.clientTop,
                      element.getClientRects().length].join(",");
                };
                const absolute = document.getElementById("absolute").getBoundingClientRect();
                return [values("inline"), values("block"),
                  [absolute.left, absolute.top, absolute.width, absolute.height].join(","),
                  values("gone")].join("|");
            })()"#,
        );
        assert_eq!(
            result,
            "130,80,130,80,120,70,5,5,1|100,50,100,50,100,50,0,0,1|30,40,100,50|0,0,0,0,0,0,0,0,0"
        );
    }

    #[test]
    fn replaced_fragment_client_rect_tracks_ancestor_scroll() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #scroller { width: 200px; height: 40px; overflow: hidden; }
            #content { height: 120px; }
            #image { width: 100px; height: 50px; }
        </style></head><body>
            <div id="scroller"><div id="content"><img id="image" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADElEQVR4AQEFAPr/AP8AAP9zftimAAAAAElFTkSuQmCC"></div></div>
        </body></html>"#;
        let mut runtime = runtime_from_html(html);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const scroller = document.getElementById("scroller");
                const image = document.getElementById("image");
                const before = image.getBoundingClientRect();
                scroller.scrollTop = 20;
                const after = image.getBoundingClientRect();
                const client = image.getClientRects()[0];
                return [before.left, before.top, after.left, after.top,
                  client.left, client.top, after.width, after.height].join(",");
            })()"#,
        );
        assert_eq!(result, "0,0,0,-20,0,-20,100,50");
    }

    #[test]
    fn escape_json_string_escapes_all_c0_control_characters() {
        // Short forms are preserved for the common whitespace controls.
        assert_eq!(escape_json_string("a\nb\rc\td"), "a\\nb\\rc\\td");
        // The two mandatory JSON escapes: backslash and double-quote.
        assert_eq!(escape_json_string("a\\\"b"), "a\\\\\\\"b");
        // Every other C0 control character becomes a `\u00XX` sequence.
        assert_eq!(escape_json_string("\u{0}"), "\\u0000");
        assert_eq!(escape_json_string("\u{1}"), "\\u0001");
        assert_eq!(escape_json_string("\u{8}"), "\\u0008");
        assert_eq!(escape_json_string("\u{b}"), "\\u000b");
        assert_eq!(escape_json_string("\u{c}"), "\\u000c");
        assert_eq!(escape_json_string("\u{1f}"), "\\u001f");
        // A realistic mixed value: a `content` string with an embedded U+0001.
        assert_eq!(escape_json_string("\u{1}x"), "\\u0001x");
        // Non-control characters (including multi-byte) pass through untouched.
        assert_eq!(escape_json_string("héllo\u{1f600}"), "héllo\u{1f600}");
    }

    #[test]
    fn get_computed_style_survives_control_char_in_content_value() {
        // A CSS hex escape injects a literal U+0001 control character into the
        // `content` string value. Before escape_json_string handled C0 controls,
        // serialize_computed_style emitted a raw control byte inside a JSON
        // string, so JSON.parse in dom_bootstrap.js threw and getComputedStyle
        // silently degraded to an empty `{}`. The value must now round-trip.
        let html = "<html><head><style>\
            #target { content: \"\\1 x\"; }\
            </style></head><body><div id=\"target\"></div></body></html>";
        let mut runtime = runtime_from_html(html);

        // The declaration itself carries a literal U+0001 (sanity check that the
        // CSS hex escape produced a control character, not the text "\\1").
        assert!(
            html.contains("\\1 x"),
            "test fixture must use a CSS hex escape, not a literal control char"
        );

        // The object must not have degraded to `{}`: a valid computed style
        // exposes many properties, so `length` is well above zero.
        assert!(
            eval_num(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').length"
            ) > 0.0,
            "getComputedStyle must not degrade to an empty object when a computed \
             value contains a control character"
        );
        // The control character survives the JSON round-trip: U+0001 then 'x'.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('target'), '').content"
            ),
            "\u{1}x",
            "the U+0001 control character in `content` must round-trip through JSON"
        );
    }

    // ── document.write (issue 016-7) ────────────────────────────────────────

    /// An inline script that writes an `<iframe id="selectors">` must create a
    /// real iframe element that `getElementById` can find — the exact pattern
    /// Acid3 relies on to build its `getTestDocument()` scaffold.
    #[test]
    fn document_write_creates_iframe_findable_by_id() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <script>document.write('<iframe id="selectors"></iframe>');</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        let iframe = doc
            .query_selector("#selectors")
            .expect("document.write should have created #selectors");
        assert_eq!(
            iframe.tag_name().as_deref(),
            Some("iframe"),
            "#selectors must be an iframe element"
        );

        // The element must also be reachable through the JS DOM API.
        let tag = runtime
            .eval("document.getElementById('selectors').tagName")
            .unwrap()
            .as_string()
            .map(|s| s.to_std_string_escaped());
        assert_eq!(tag.as_deref(), Some("IFRAME"));
    }

    /// Written content is spliced in at the running script's position, so nodes
    /// that followed the script in the source stay after the written fragment.
    #[test]
    fn document_write_inserts_at_script_position() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body><script>document.write('<div id="written"></div>');</script><p id="after"></p></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        let body = doc.query_selector("body").expect("body");
        let tags: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            tags,
            vec!["script".to_string(), "div".to_string(), "p".to_string()],
            "written <div> must land between the <script> and the following <p>"
        );
    }

    /// Multiple writes from the same script accumulate in call order.
    #[test]
    fn document_write_multiple_calls_stay_in_order() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body><script>document.write('<i id="one"></i>');document.write('<b id="two"></b>');</script><span id="tail"></span></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        let body = doc.query_selector("body").expect("body");
        let tags: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            tags,
            vec![
                "script".to_string(),
                "i".to_string(),
                "b".to_string(),
                "span".to_string()
            ],
            "both writes must be ordered after the script and before the tail"
        );
    }

    /// A `<script>` inside written markup executes synchronously in global scope.
    #[test]
    fn document_write_executes_written_script() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <script>document.write('<script>globalThis.__written_ran = 41 + 1;<\/script>');</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        assert_eq!(
            runtime
                .eval("globalThis.__written_ran")
                .unwrap()
                .as_number(),
            Some(42.0),
            "a <script> written via document.write must run in global scope"
        );
    }

    /// document.writeln behaves like write but appends a newline.
    #[test]
    fn document_writeln_appends_newline() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body><script>document.writeln('<pre id="p">x</pre>');</script></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        let pre = doc.query_selector("#p").expect("writeln should create #p");
        assert_eq!(pre.tag_name().as_deref(), Some("pre"));
        // The trailing newline lands as a text node after the written element.
        let body = doc.query_selector("body").expect("body");
        let has_newline_text = body.child_nodes().iter().any(|n| {
            n.node_type() == crate::dom::NodeType::Text
                && n.data()
                    .as_deref()
                    .map(|d| d.contains('\n'))
                    .unwrap_or(false)
        });
        assert!(has_newline_text, "writeln must append a newline text node");
    }

    /// A write from outside any running script (e.g. from `eval` after parsing)
    /// appends to `<body>` instead of wiping the document.
    #[test]
    fn document_write_without_active_script_appends_to_body() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body><p id="existing"></p></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        // No execute_document_scripts: the insertion point is undefined.
        runtime
            .eval("document.write('<span id=\"late\"></span>')")
            .unwrap();

        // The pre-existing content survives (no document.open() reset).
        assert!(
            doc.query_selector("#existing").is_some(),
            "existing content must not be erased"
        );
        let body = doc.query_selector("body").expect("body");
        let tags: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            tags,
            vec!["p".to_string(), "span".to_string()],
            "late write must append after existing body children"
        );
    }

    /// The verbatim Acid3 fragment must produce the map/iframe/form/table
    /// scaffold, with `#selectors` iframe reachable and later mutable.
    #[test]
    fn document_write_acid3_fragment_builds_selectors_scaffold() {
        use crate::html::TreeBuilder;
        // Exactly the fragment acid3.html writes at the end of <body>.
        let html = r#"<html><body>
            <script>document.write('<map name=""><area href="" shape="rect" coords="2,2,4,4" alt="<\'>"><iframe src="empty.png">FAIL<\/iframe><iframe src="empty.txt">FAIL<\/iframe><iframe src="empty.html" id="selectors"><\/iframe><form action="" name="form"><input type=HIDDEN><\/form><table><tr><td><p><\/tbody> <\/table><\/map>');</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        // The selectors iframe exists and is an iframe.
        let selectors = doc
            .query_selector("#selectors")
            .expect("Acid3 fragment must yield #selectors");
        assert_eq!(selectors.tag_name().as_deref(), Some("iframe"));

        // The other scaffold elements are present too.
        assert!(doc.query_selector("map").is_some(), "map must exist");
        assert!(doc.query_selector("form").is_some(), "form must exist");
        assert!(doc.query_selector("table").is_some(), "table must exist");
        assert!(doc.query_selector("area").is_some(), "area must exist");

        // The iframe is registered, so a later attribute mutation succeeds
        // (Acid3 test 32 does exactly this on #selectors).
        runtime
            .eval("document.getElementById('selectors').setAttribute('style', 'height: 100px')")
            .unwrap();
        assert_eq!(
            selectors
                .attributes()
                .unwrap_or_default()
                .get("style")
                .map(|s| s.as_str()),
            Some("height: 100px"),
            "the written iframe must be mutable through the DOM API"
        );
    }

    // ── DOM Traversal / Range ───────────────────────────────────────────────

    #[test]
    fn node_iterator_honors_mask_filter_exceptions_and_live_removal() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse("<html><body><div id='r'>a<i>b</i><b>c</b></div></body></html>")
                .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{
          var r=document.getElementById('r'), seen=[];
          var it=document.createNodeIterator(r, NodeFilter.SHOW_ELEMENT, function(n) {
            if (n.tagName === 'I') return NodeFilter.FILTER_REJECT;
            return NodeFilter.FILTER_ACCEPT;
          });
          var n; while(n=it.nextNode()) seen.push(n.tagName);
          return seen.join(',')})()"#
            ),
            "DIV,B"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{
          var r=document.getElementById('r');
          try { document.createNodeIterator(r, NodeFilter.SHOW_ALL, function(){throw 'filter-error'}).nextNode(); return 'miss' }
          catch(e) { return String(e) }})()"#
            ),
            "filter-error"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{
          var x=document.createElement('div'); var a=document.createElement('a'); var b=document.createElement('b');
          x.appendChild(a); x.appendChild(b); var live=document.createNodeIterator(x); live.nextNode(); live.nextNode();
          x.removeChild(a); return live.nextNode().tagName})()"#
            ),
            "B"
        );
    }

    #[test]
    fn tree_walker_distinguishes_reject_from_skip_and_stays_in_root() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            "<html><body><div id='r'><section><i></i></section><b></b></div></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{
          var r=document.getElementById('r');
          var w=document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT, function(n) {
            return n.tagName==='SECTION' ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT;
          }); var out=[],n; while(n=w.nextNode()) out.push(n.tagName); return out.join(',')})()"#
            ),
            "I,B"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{
          var r=document.getElementById('r'); var w=document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT, function(n) {
            return n.tagName==='SECTION' ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
          }); var out=[],n; while(n=w.nextNode()) out.push(n.tagName); return out.join(',')})()"#
            ),
            "B"
        );
    }

    #[test]
    fn range_boundaries_clone_string_and_legacy_exception_are_explicit() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse("<html><body><p id='p'>Hello <em>World</em>!</p></body></html>")
                .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var p=document.getElementById('p'),r=document.createRange();
          r.selectNodeContents(p); var c=r.cloneContents();
          return [r.toString(),c.nodeType,c.childNodes.length,r.collapsed,r.commonAncestorContainer===p].join('|')})()"#
            ),
            "Hello World!|11|3|false|true"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var p=document.getElementById('p'),r=document.createRange();
          r.setStart(p.firstChild,2); r.setEnd(p.firstChild,5); return [r.toString(),r.cloneRange().toString()].join('|')})()"#
            ),
            "llo|llo"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var r=document.createRange();try{r.setEndBefore(document);return 'none'}catch(e){return e.name+'|'+e.code+'|'+e.INVALID_NODE_TYPE_ERR}})()"#
            ),
            "InvalidNodeTypeError|24|24"
        );
    }

    #[test]
    fn range_extract_returns_fragment_with_partial_ancestor_clones() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            "<html><body><h1>Hello <em>Wonderful</em> Kitty</h1><p>How are you?</p></body></html>",
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var h=document.querySelector('h1'),em=document.querySelector('em'),p=document.querySelector('p');
          var r=document.createRange();r.setStart(em.firstChild,6);r.setEnd(p,0);var f=r.extractContents();
          return [f.nodeType,f.childNodes.length,f.firstChild.tagName,f.firstChild.firstChild.tagName,f.firstChild.firstChild.textContent,f.firstChild.lastChild.textContent,f.lastChild.tagName,p.childNodes.length].join('|')})()"#
            ),
            "11|2|H1|EM|ful| Kitty|P|1"
        );
    }

    #[test]
    fn range_insert_splits_text_and_keeps_inserted_node_in_selection() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><p id='p'>12345</p></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var p=document.getElementById('p'),t1=p.firstChild,t2=document.createTextNode('ABCDE');p.appendChild(t2);
          var r=document.createRange();r.setStart(t1,2);r.setEnd(t1,3);r.insertNode(t2);
          return [p.childNodes.length,p.childNodes[0].data,p.childNodes[1].data,p.childNodes[2].data,r.toString()].join('|')})()"#
            ),
            "3|12|ABCDE|345|ABCDE3"
        );
    }

    #[test]
    fn range_live_boundaries_follow_removed_subtree() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><p id='p'>12345</p></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var p=document.getElementById('p'),b=document.body,r=document.createRange();
          r.setEnd(b,1);r.setStart(p.firstChild,2);b.removeChild(p);
          return [r.collapsed,r.startContainer===b,r.startOffset,r.endContainer===b,r.endOffset].join('|')})()"#
            ),
            "true|true|0|true|0"
        );
    }

    #[test]
    fn range_surround_reports_hierarchy_and_partial_character_data_errors() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var c=document.createComment('11111');document.appendChild(c);var r=document.createRange();r.selectNode(c);
          try{r.surroundContents(document.createElement('a'));return 'none'}catch(e){document.removeChild(c);return e.name+'|'+e.code}})()"#
            ),
            "HierarchyRequestError|3"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var b=document.body,c1=document.createComment('111'),c2=document.createComment('222');b.appendChild(c1);b.appendChild(c2);
          var r=document.createRange();r.setStart(c1,1);r.setEnd(c2,1);try{r.surroundContents(document.createElement('a'));return 'none'}catch(e){return e.name+'|'+e.code}})()"#
            ),
            "InvalidStateError|11"
        );
    }

    #[test]
    fn acid3_traversal_filter_mutation_and_tree_regrafting_regressions() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse("<html><head><title></title></head><body></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var b=document.body;for(var k=0;k<5;k++){var s=document.createElement('section');s.title=k;b.appendChild(s)}
          var count=0,it=document.createNodeIterator(b,0xffffffff,function(){if(count>3&&count<12)b.appendChild(b.firstChild);count++;return count%2===0?1:2});
          var out=[],n;while(n=it.nextNode())out.push(n.title);return out.join(',')})()"#
            ),
            "0,2,4,1,3,0,2"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(()=>{var b=document.body;b.textContent='';var p=document.createElement('p');b.appendChild(p);var w=document.createTreeWalker(b);
          w.lastChild();w.previousNode();document.documentElement.removeChild(b);var a=w.lastChild()===p,z=w.nextNode()===null;
          document.documentElement.appendChild(p);var title=w.previousNode();p.appendChild(b);return [a,z,title.tagName,w.nextNode()===p,w.nextNode()===b,w.previousNode()===null].join('|')})()"#
            ),
            "true|true|TITLE|true|true|true"
        );
    }

    // ── iframe / contentDocument (sub-browsing contexts) ────────────────────

    /// A tiny static HTTP/1.1 server that answers every request with the same
    /// status, `Content-Type`, and body. It stays alive for the whole process
    /// (detached, like the other HTTP client tests), so a lazily-loaded iframe
    /// can both fetch and later reload its document.
    fn spawn_static_http_server(content_type: &'static str, body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    content_type,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    fn eval_string_value(runtime: &mut JsRuntime, source: &str) -> Option<String> {
        runtime
            .eval(source)
            .unwrap()
            .as_string()
            .map(|s| s.to_std_string_escaped())
    }

    fn pump_zero_delay_tasks(runtime: &mut JsRuntime) {
        runtime
            .tick(0)
            .expect("zero-delay resource tasks should run");
    }

    #[test]
    fn svg_namespace_elements_use_svg_dom_interfaces() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_eq!(
            runtime
                .eval(
                    r#"var d = document.implementation.createDocument('http://www.w3.org/2000/svg', 'svg', null);
                       var rect = d.createElementNS('http://www.w3.org/2000/svg', 'rect');
                       var text = d.createElementNS('http://www.w3.org/2000/svg', 'text');
                       var circle = d.createElementNS('http://www.w3.org/2000/svg', 'circle');
                       text.appendChild(d.createTextNode('abc'));
                       [d.documentElement instanceof SVGSVGElement,
                        rect instanceof SVGRectElement,
                        rect.constructor.name,
                        !!rect.width,
                        text instanceof SVGTextElement,
                        text instanceof SVGTextContentElement,
                        text.getNumberOfChars(),
                        circle instanceof SVGElement].join('|')"#,
                )
                .unwrap()
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .as_deref(),
            Some("true|true|SVGRectElement|true|true|true|3|true")
        );
        assert_eq!(
            runtime
                .eval("document.createElement('rect') instanceof SVGRectElement")
                .unwrap()
                .as_boolean(),
            Some(false),
            "an HTML rect element must not receive an SVG interface"
        );
    }

    #[test]
    fn embedded_svg_documents_are_exposed_by_iframe_and_object() {
        let port = spawn_static_http_server(
            "image/svg+xml",
            r#"<svg xmlns="http://www.w3.org/2000/svg"><text>svg</text></svg>"#,
        );
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url(
            format!("http://127.0.0.1:{port}/index.html")
                .parse()
                .unwrap(),
        );
        assert_eq!(
            runtime
                .eval(
                    r#"var frame = document.createElement('iframe'); frame.src = '/asset.svg';
                       var object = document.createElement('object'); object.data = '/asset.svg';
                       document.body.appendChild(frame); document.body.appendChild(object);
                       [frame.getSVGDocument() === frame.contentDocument,
                        object.getSVGDocument() === object.contentDocument,
                        frame.getSVGDocument().documentElement instanceof SVGSVGElement,
                        object.getSVGDocument().getElementsByTagName('text')[0] instanceof SVGTextElement].join('|')"#,
                )
                .unwrap()
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .as_deref(),
            Some("true|true|true|true")
        );

        assert_eq!(
            runtime
                .eval(
                    "var htmlFrame = document.createElement('iframe'); htmlFrame.getSVGDocument()"
                )
                .unwrap()
                .is_null(),
            true,
            "getSVGDocument must return null for a non-SVG document"
        );
    }

    #[test]
    fn connected_iframe_load_supports_property_attribute_and_listener_handlers() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><iframe id="property"></iframe><iframe id="attribute" onload="globalThis.attributeLoads++"></iframe><iframe id="listener"></iframe></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"globalThis.propertyLoads = 0;
                   globalThis.attributeLoads = 0;
                   globalThis.listenerLoads = 0;
                   document.getElementById('property').onload = () => propertyLoads++;
                   document.getElementById('listener').addEventListener('load', () => listenerLoads++);"#,
            )
            .unwrap();
        runtime.wire_inline_event_handlers().unwrap();

        assert_eq!(
            runtime
                .eval("propertyLoads + attributeLoads + listenerLoads")
                .unwrap()
                .as_number(),
            Some(0.0)
        );
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("propertyLoads").unwrap().as_number(),
            Some(1.0)
        );
        assert_eq!(
            runtime.eval("attributeLoads").unwrap().as_number(),
            Some(1.0)
        );
        assert_eq!(
            runtime.eval("listenerLoads").unwrap().as_number(),
            Some(1.0)
        );
    }

    #[test]
    fn detached_iframe_load_waits_until_connected() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   globalThis.frame = document.createElement('iframe');
                   frame.src = 'about:blank';
                   frame.onload = () => loads++;"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(0.0));

        runtime.eval("document.body.appendChild(frame)").unwrap();
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(0.0));
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));
    }

    #[test]
    fn reconnected_iframe_reloads_and_dispatches_load_again() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   globalThis.frame = document.createElement('iframe');
                   frame.onload = () => loads++;
                   document.body.insertBefore(frame, null);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        runtime
            .eval(
                r#"globalThis.firstDocument = frame.contentDocument;
                   document.body.removeChild(frame);
                   document.body.appendChild(frame);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(2.0));
        assert_eq!(
            runtime
                .eval("frame.contentDocument !== firstDocument")
                .unwrap()
                .as_boolean(),
            Some(true),
            "reconnection should start a fresh iframe navigation"
        );
    }

    /// Moving a connected `<iframe>` *directly* from the main document into an
    /// iframe sub-document (no detach in between) is a cross-document move and
    /// must re-navigate the frame, dispatching `load` again while it is
    /// connected to the new (sub-)document.
    #[test]
    fn iframe_moved_from_main_to_sub_document_redispatches_load() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   // A host iframe supplies a live sub-document to move into.
                   globalThis.host = document.createElement('iframe');
                   document.body.appendChild(host);
                   globalThis.moved = document.createElement('iframe');
                   moved.onload = () => loads++;
                   document.body.appendChild(moved);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        // Only `moved` carries a load handler; its initial connection fires it.
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));

        runtime
            .eval(
                r#"globalThis.subDoc = host.contentDocument;
                   globalThis.firstMovedDoc = moved.contentDocument;
                   // Direct move: `moved` is still connected to the main document
                   // and is appended into the sub-document without a removeChild.
                   subDoc.body.appendChild(moved);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(2.0),
            "a cross-document move must re-navigate and dispatch load again"
        );
        assert_eq!(
            runtime
                .eval("moved.ownerDocument === subDoc")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the moved iframe must now be owned by the sub-document"
        );
        assert_eq!(
            runtime
                .eval("moved.contentDocument !== firstMovedDoc")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the cross-document move must start a fresh sub-document navigation"
        );
    }

    /// The reverse direction: moving a connected `<iframe>` out of an iframe
    /// sub-document into the main document is also a cross-document move and
    /// re-dispatches `load` in the main document.
    #[test]
    fn iframe_moved_from_sub_to_main_document_redispatches_load() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   globalThis.host = document.createElement('iframe');
                   document.body.appendChild(host);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        runtime
            .eval(
                r#"globalThis.subDoc = host.contentDocument;
                   globalThis.moved = document.createElement('iframe');
                   moved.onload = () => loads++;
                   // Connect `moved` inside the sub-document first.
                   subDoc.body.appendChild(moved);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(1.0),
            "the initial connection inside the sub-document fires load once"
        );

        runtime
            .eval(
                r#"globalThis.firstMovedDoc = moved.contentDocument;
                   // Direct move back out into the main document. Uses
                   // insertBefore so the `insert_before_native` cross-document
                   // branch is exercised (the appendChild path is covered by the
                   // main→sub test above).
                   document.body.insertBefore(moved, null);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(2.0),
            "moving out of the sub-document must re-navigate and dispatch load"
        );
        assert_eq!(
            runtime
                .eval("moved.ownerDocument === document")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the moved iframe must now be owned by the main document"
        );
    }

    /// A *direct* in-document reorder (re-appending an already-connected iframe
    /// within the same document, with no detach) is deliberately NOT treated as
    /// a fresh navigation under the current model, so `load` does not re-fire.
    /// (Real browsers do reload here; matching that requires the broader
    /// navigation rework and is out of scope.)
    #[test]
    fn same_document_direct_reinsertion_does_not_reload_iframe() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   globalThis.frame = document.createElement('iframe');
                   frame.onload = () => loads++;
                   document.body.appendChild(frame);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));

        // Re-append within the same document (a move, no removeChild).
        runtime.eval("document.body.appendChild(frame);").unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(1.0),
            "an in-document reorder must not re-navigate under the current model"
        );
    }

    #[test]
    fn document_write_iframe_dispatches_load() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   document.write('<iframe id="written" onload="globalThis.loads++"></iframe>');"#,
            )
            .unwrap();
        runtime.wire_inline_event_handlers().unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));
    }

    #[test]
    fn connected_object_with_data_dispatches_load() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   const object = document.createElement('object');
                   object.data = 'fixture.svg';
                   object.onload = () => loads++;
                   document.body.appendChild(object);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));
    }

    /// Reassigning the `src` of a *connected* iframe (the `.src` IDL setter
    /// path) starts a fresh navigation whose `load` event fires once the queued
    /// task runs, matching the paired
    /// [`same_document_direct_reinsertion_does_not_reload_iframe`] which must NOT
    /// reload on a mere in-document reorder.
    #[test]
    fn connected_iframe_src_change_renavigates_and_fires_load() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/html",
            r#"<html><body><p id="x">frame</p></body></html>"#,
        );
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   document.getElementById('f').addEventListener('load', () => loads++);"#,
            )
            .unwrap();
        // The initial connection (from document construction) fires load once.
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));

        runtime
            .eval(&format!(
                "document.getElementById('f').src = 'http://127.0.0.1:{port}/next.html';"
            ))
            .unwrap();
        // The re-navigation load is queued, not fired synchronously.
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(2.0),
            "changing src on a connected iframe must re-navigate and dispatch load again"
        );
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('x').textContent"
            )
            .as_deref(),
            Some("frame"),
            "the re-navigation must load the new sub-document"
        );
    }

    /// The `setAttribute('src', ...)` path re-navigates a connected iframe just
    /// like the `.src` IDL setter does, fetching and parsing the new resource
    /// (not merely re-firing `load`): after the change `contentDocument` exposes
    /// the server's document, which the initial about:blank sub-document lacked.
    #[test]
    fn connected_iframe_set_attribute_src_renavigates_and_fires_load() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/html",
            r#"<html><body><p id="loaded">via-setattr</p></body></html>"#,
        );
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   document.getElementById('f').addEventListener('load', () => loads++);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));

        runtime
            .eval(&format!(
                "document.getElementById('f').setAttribute('src', 'http://127.0.0.1:{port}/next.html');"
            ))
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(2.0),
            "setAttribute('src', ...) on a connected iframe must re-navigate and dispatch load"
        );
        // The re-navigation must actually load the new sub-document's content,
        // which the initial about:blank document did not contain.
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('loaded').textContent"
            )
            .as_deref(),
            Some("via-setattr"),
            "setAttribute('src', ...) must parse the new sub-document's content"
        );
    }

    /// Acid3 test 48 minimal repro: an `onload` handler attached *after* load
    /// via `setAttribute('onload', "code")` runs when a subsequent `src`
    /// re-navigation completes (removing a class from an unrelated element).
    #[test]
    fn dynamic_onload_attribute_runs_on_iframe_src_renavigation() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server("text/html", r#"<html><body></body></html>"#);
        let doc = TreeBuilder::parse(
            r#"<html><body><span id="target" class="hide"></span><iframe id="f"></iframe></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        // Drain the initial about:blank load so it cannot run the handler set below.
        pump_zero_delay_tasks(&mut runtime);

        runtime
            .eval(&format!(
                r#"var f = document.getElementById('f');
                   f.setAttribute('onload', "document.getElementById('target').removeAttribute('class')");
                   f.src = 'http://127.0.0.1:{port}/next.html';"#
            ))
            .unwrap();
        // The class survives until the queued re-navigation load dispatches.
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('target').getAttribute('class')"
            )
            .as_deref(),
            Some("hide")
        );
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime
                .eval("document.getElementById('target').getAttribute('class')")
                .unwrap()
                .is_null(),
            true,
            "the dynamically wired onload handler must run on re-navigation and remove the class"
        );
    }

    /// A detached iframe must never navigate on a `src` change (whether via the
    /// IDL setter or setAttribute); its load waits until it is connected. This
    /// upholds the [`detached_iframe_load_waits_until_connected`] invariant for
    /// the new attribute-driven navigation path.
    #[test]
    fn detached_iframe_src_change_does_not_fire_load() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   globalThis.frame = document.createElement('iframe');
                   frame.addEventListener('load', () => loads++);
                   frame.src = 'about:blank';
                   frame.setAttribute('src', 'about:blank?again');"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(0.0),
            "a detached iframe must not navigate on src changes"
        );
    }

    /// Removing a dynamically wired `on*` content attribute detaches its
    /// listener, and re-setting one replaces (does not stack) the handler.
    #[test]
    fn remove_on_attribute_detaches_dynamically_wired_handler() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.clicks = 0;
                   globalThis.el = document.createElement('div');
                   el.setAttribute('onclick', 'globalThis.clicks++');"#,
            )
            .unwrap();
        runtime
            .eval("el.dispatchEvent(new Event('click'))")
            .unwrap();
        assert_eq!(
            runtime.eval("clicks").unwrap().as_number(),
            Some(1.0),
            "a dynamically set onclick attribute must fire"
        );

        runtime.eval("el.removeAttribute('onclick');").unwrap();
        runtime
            .eval("el.dispatchEvent(new Event('click'))")
            .unwrap();
        assert_eq!(
            runtime.eval("clicks").unwrap().as_number(),
            Some(1.0),
            "removing the onclick attribute must detach its handler"
        );

        // Re-setting twice must leave exactly one (the latest) handler wired.
        runtime
            .eval(
                r#"clicks = 0;
                   el.setAttribute('onclick', 'globalThis.clicks++');
                   el.setAttribute('onclick', 'globalThis.clicks += 10');"#,
            )
            .unwrap();
        runtime
            .eval("el.dispatchEvent(new Event('click'))")
            .unwrap();
        assert_eq!(
            runtime.eval("clicks").unwrap().as_number(),
            Some(10.0),
            "re-setting an on* attribute must replace, not stack, the handler"
        );
    }

    /// A crafted `on*` content-attribute name whose derived event type collides
    /// with an `Object.prototype` member (`on__proto__` -> `"__proto__"`,
    /// `onconstructor` -> `"constructor"`) must wire without throwing and
    /// without corrupting the per-node handler store. Regression test for the
    /// null-prototype (`Object.create(null)`) handler dictionary: a plain `{}`
    /// store resolves the inherited member as a bogus "previous" handler and
    /// throws while (re)wiring, besides letting such a key reach the prototype.
    #[test]
    fn crafted_on_attribute_name_does_not_break_or_pollute_handler_store() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval(
                r#"globalThis.threw = null;
                   globalThis.clicks = 0;
                   globalThis.el = document.createElement('div');
                   try {
                     // Handlers add 100 if they ever fire; they are wired to the
                     // "__proto__"/"constructor" event types, which are never
                     // dispatched below, so a passing run leaves them dormant.
                     el.setAttribute('on__proto__', 'globalThis.clicks += 100');
                     el.setAttribute('onconstructor', 'globalThis.clicks += 100');
                   } catch (e) { globalThis.threw = String(e); }"#,
            )
            .unwrap();
        assert_eq!(
            eval_string_value(&mut runtime, "threw === null ? 'no-throw' : threw").as_deref(),
            Some("no-throw"),
            "wiring a crafted on* attribute name must not throw (null-prototype store)"
        );

        // The store stayed intact: a subsequent legitimate handler wires and
        // fires exactly once, and the crafted-name event types did not fire.
        runtime
            .eval(
                r#"el.setAttribute('onclick', 'globalThis.clicks += 1');
                   el.dispatchEvent(new Event('click'));"#,
            )
            .unwrap();
        assert_eq!(
            runtime.eval("clicks").unwrap().as_number(),
            Some(1.0),
            "a normal on* handler must still fire once after crafted names were wired"
        );
    }

    /// Changing a connected `<object>`'s `data` resource re-navigates it and
    /// dispatches `load` again, mirroring the iframe `src` path. Beyond the
    /// re-fired `load`, the new resource must actually be fetched and parsed:
    /// two servers hand out distinguishable documents, and `contentDocument`
    /// must expose the second document's content once the change is pumped.
    #[test]
    fn connected_object_data_change_renavigates_and_fires_load() {
        use crate::html::TreeBuilder;
        let first_port = spawn_static_http_server(
            "text/html",
            r#"<html><body><p id="marker">first</p></body></html>"#,
        );
        let second_port = spawn_static_http_server(
            "text/html",
            r#"<html><body><p id="marker">second</p></body></html>"#,
        );
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><object id="o" data="http://127.0.0.1:{first_port}/first.html"></object></body></html>"#
        ))
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval(
                r#"globalThis.loads = 0;
                   document.getElementById('o').addEventListener('load', () => loads++);"#,
            )
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("loads").unwrap().as_number(), Some(1.0));
        // The initial navigation parses the first document into the sub-context.
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('o').contentDocument.getElementById('marker').textContent"
            )
            .as_deref(),
            Some("first"),
            "the initial object navigation must load the first sub-document"
        );

        runtime
            .eval(&format!(
                "document.getElementById('o').setAttribute('data', 'http://127.0.0.1:{second_port}/second.html');"
            ))
            .unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime.eval("loads").unwrap().as_number(),
            Some(2.0),
            "changing an object's data must re-navigate and dispatch load again"
        );
        // The re-navigation must swap the sub-document's content for the second
        // server's document, not just re-fire `load` over the stale first one.
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('o').contentDocument.getElementById('marker').textContent"
            )
            .as_deref(),
            Some("second"),
            "re-navigating the object must load the second sub-document's content"
        );
    }

    #[test]
    fn missing_title_reflects_as_empty_string() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_eq!(
            eval_string_value(&mut runtime, "document.createElement('p').title").as_deref(),
            Some("")
        );
    }

    #[test]
    fn is_html_mime_type_matches_html_essences_only() {
        assert!(is_html_mime_type("text/html"));
        assert!(is_html_mime_type("text/html; charset=utf-8"));
        assert!(!is_html_mime_type("APPLICATION/XHTML+XML"));
        assert!(!is_html_mime_type("image/png"));
        assert!(!is_html_mime_type("text/plain; charset=utf-8"));
        assert!(!is_html_mime_type("application/xml"));
        assert!(!is_html_mime_type("image/svg+xml"));
        assert!(!is_html_mime_type(""));
    }

    #[test]
    fn is_xml_mime_type_matches_required_essences() {
        for mime in [
            "text/xml",
            "application/xml;charset=utf-8",
            "image/svg+xml",
            "APPLICATION/XHTML+XML",
        ] {
            assert!(is_xml_mime_type(mime), "{mime}");
        }
        assert!(!is_xml_mime_type("text/html"));
    }

    #[test]
    fn blank_html_document_has_html_head_and_body_but_no_content() {
        let doc = blank_html_document();
        assert_eq!(doc.node_type(), crate::dom::NodeType::Document);
        assert_eq!(
            doc.query_selector("html")
                .and_then(|h| h.tag_name())
                .as_deref(),
            Some("html")
        );
        assert!(doc.query_selector("head").is_some(), "must have a head");
        assert!(doc.query_selector("body").is_some(), "must have a body");
        // An about:blank skeleton carries no minable markup.
        assert!(doc.query_selector("p").is_none(), "must have no <p>");
    }

    /// An iframe with no `src` exposes an independent about:blank document with
    /// its own `<html>`/`<head>`/`<body>`, distinct from the top-level document.
    #[test]
    fn iframe_empty_content_document_is_independent_document() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert_eq!(
            runtime
                .eval("document.getElementById('f').contentDocument.nodeType")
                .unwrap()
                .as_number(),
            Some(9.0),
            "contentDocument must be a Document node (nodeType 9)"
        );
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.documentElement.tagName"
            )
            .as_deref(),
            Some("HTML")
        );
        assert_eq!(
            runtime
                .eval("document.getElementById('f').contentDocument === document")
                .unwrap()
                .as_boolean(),
            Some(false),
            "the sub-document must not be the top-level document"
        );
        assert_eq!(
            runtime
                .eval("!!document.getElementById('f').contentDocument.body && !!document.getElementById('f').contentDocument.head")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the about:blank sub-document must expose head and body"
        );
    }

    /// DOM mutations in a sub-document do not leak into the parent, and vice
    /// versa: `getElementById`/`getElementsByTagName` stay scoped per document.
    #[test]
    fn iframe_empty_content_document_dom_ops_are_isolated_from_parent() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><span id="parentonly"></span><iframe id="f"></iframe></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var p = d.createElement('p');
                   p.id = 'inner';
                   d.body.appendChild(p);"#,
            )
            .unwrap();

        // The sub-document sees the node it created.
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('inner').tagName"
            )
            .as_deref(),
            Some("P")
        );
        // The parent does not.
        assert!(
            runtime
                .eval("document.getElementById('inner')")
                .unwrap()
                .is_null(),
            "the parent document must not find a child-document node by id"
        );
        // getElementsByTagName is scoped: 0 <p> in the parent, 1 in the child.
        assert_eq!(
            runtime
                .eval("document.getElementsByTagName('p').length")
                .unwrap()
                .as_number(),
            Some(0.0)
        );
        assert_eq!(
            runtime
                .eval(
                    "document.getElementById('f').contentDocument.getElementsByTagName('p').length"
                )
                .unwrap()
                .as_number(),
            Some(1.0)
        );
        // The sub-document must not see the parent-only element.
        assert!(
            runtime
                .eval("document.getElementById('f').contentDocument.getElementById('parentonly')")
                .unwrap()
                .is_null(),
            "the child document must not find a parent-only node by id"
        );
    }

    /// `getElementById` returns the element belonging to the document it was
    /// called on, even when parent and child share an id.
    #[test]
    fn iframe_content_document_get_element_by_id_is_scoped_per_document() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><b id="dup" data-where="parent"></b><iframe id="f"></iframe></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var c = d.createElement('b');
                   c.id = 'dup';
                   c.setAttribute('data-where', 'child');
                   d.body.appendChild(c);"#,
            )
            .unwrap();

        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('dup').getAttribute('data-where')"
            )
            .as_deref(),
            Some("parent")
        );
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('dup').getAttribute('data-where')"
            )
            .as_deref(),
            Some("child")
        );
    }

    /// Repeated `contentDocument` reads return the same document instance while
    /// `src` is unchanged, so nodes created in it persist between accesses.
    #[test]
    fn iframe_content_document_is_stable_across_accesses() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert_eq!(
            runtime
                .eval("document.getElementById('f').contentDocument === document.getElementById('f').contentDocument")
                .unwrap()
                .as_boolean(),
            Some(true),
            "contentDocument must be a stable instance across reads"
        );

        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   d.body.appendChild(d.createElement('i'));"#,
            )
            .unwrap();
        assert_eq!(
            runtime
                .eval(
                    "document.getElementById('f').contentDocument.getElementsByTagName('i').length"
                )
                .unwrap()
                .as_number(),
            Some(1.0),
            "a node appended to the sub-document must survive a later access"
        );
    }

    /// An HTML `src` is fetched and parsed into the sub-document's DOM tree.
    #[test]
    fn iframe_html_src_is_parsed_into_content_document() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/html; charset=utf-8",
            r#"<!DOCTYPE html><html><head></head><body><p id="loaded">hi</p></body></html>"#,
        );
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/page.html"></iframe></body></html>"#
        ))
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert_eq!(
            runtime
                .eval(
                    "document.getElementById('f').contentDocument.getElementsByTagName('p').length"
                )
                .unwrap()
                .as_number(),
            Some(1.0)
        );
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('loaded').textContent"
            )
            .as_deref(),
            Some("hi")
        );
    }

    #[test]
    fn iframe_xml_src_preserves_case_namespace_entities_and_doctype() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "application/xml; charset=utf-8",
            r#"<?xml version='1.0'?><!DOCTYPE Root SYSTEM 'urn:test'><Root xmlns='urn:root' xmlns:p='urn:child' A='&lt;&#65;'><p:Child/></Root>"#,
        );
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/doc.xml"></iframe></body></html>"#
        )).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_string_value(&mut runtime,
            "var d=document.getElementById('f').contentDocument,c=d.documentElement.childNodes[0]; [d.doctype.nodeType,d.doctype.nodeName,d.doctype.name,d.doctype.systemId,d.documentElement.tagName,d.documentElement.namespaceURI,d.documentElement.getAttribute('A'),c.localName].join('|')"
        ).as_deref(), Some("10|Root|Root|urn:test|Root|urn:root|<A|Child"));
    }

    #[test]
    fn malformed_xml_discards_the_whole_partial_tree() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server("text/xml", "<root><test/></wrong>");
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/bad.xml"></iframe></body></html>"#
        )).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(runtime.eval("document.getElementById('f').contentDocument.getElementsByTagName('test').length").unwrap().as_number(), Some(0.0));
    }

    #[test]
    fn xhtml_scripts_run_only_for_well_formed_correct_namespace_documents() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/xml",
            r#"<html xmlns='http://www.w3.org/1999/xhtml'><body><script>parent.xmlNotice=(parent.xmlNotice||0)+1</script></body></html>"#,
        );
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/x.xhtml"></iframe></body></html>"#
        )).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("xmlNotice").unwrap().as_number(), Some(1.0));

        let wrong_port = spawn_static_http_server(
            "text/xml",
            r#"<html xmlns='http://www.w3.org/1999/xhtml#'><body><script>parent.wrongNotice=1</script></body></html>"#,
        );
        runtime.eval(&format!("var f=document.getElementById('f');f.src='http://127.0.0.1:{wrong_port}/wrong.xhtml';document.body.removeChild(f);document.body.appendChild(f)")).unwrap();
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(
            runtime
                .eval("typeof wrongNotice")
                .unwrap()
                .as_string()
                .unwrap()
                .to_std_string_escaped(),
            "undefined"
        );
    }

    #[test]
    fn failing_xhtml_script_does_not_stop_later_scripts_or_iframe_load() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "application/xhtml+xml",
            r#"<html xmlns='http://www.w3.org/1999/xhtml'><body><script>throw new Error('expected')</script><script>parent.xhtmlAfterError='ran'</script></body></html>"#,
        );
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/x.xhtml"></iframe></body></html>"#
        ))
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime
            .eval("document.getElementById('f').addEventListener('load', () => globalThis.xhtmlLoadCount = (globalThis.xhtmlLoadCount || 0) + 1)")
            .unwrap();

        pump_zero_delay_tasks(&mut runtime);

        assert_eq!(
            eval_string_value(&mut runtime, "globalThis.xhtmlAfterError").as_deref(),
            Some("ran"),
            "a later XHTML script must run after an earlier script throws"
        );
        assert_eq!(
            runtime
                .eval("globalThis.xhtmlLoadCount")
                .unwrap()
                .as_number(),
            Some(1.0),
            "the iframe load event must still dispatch exactly once"
        );
    }

    /// A resource served as `image/png` must never be parsed as HTML, even when
    /// its bytes look like markup (Acid3 test 14).
    #[test]
    fn iframe_png_src_is_not_parsed_as_html() {
        use crate::html::TreeBuilder;
        let port =
            spawn_static_http_server("image/png", r#"<html><body><p>FAIL</p></body></html>"#);
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/empty.png"></iframe></body></html>"#
        ))
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            runtime
                .eval(
                    "document.getElementById('f').contentDocument.getElementsByTagName('p').length"
                )
                .unwrap()
                .as_number(),
            Some(0.0),
            "image/png content must not be parsed as HTML"
        );
    }

    /// A resource served as `text/plain` must never be parsed as HTML even when
    /// it contains markup (Acid3 test 15).
    #[test]
    fn iframe_text_plain_src_is_not_parsed_as_html() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/plain; charset=utf-8",
            r#"<html><body><p>FAIL</p></body></html>"#,
        );
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/empty.txt"></iframe></body></html>"#
        ))
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            runtime
                .eval(
                    "document.getElementById('f').contentDocument.getElementsByTagName('p').length"
                )
                .unwrap()
                .as_number(),
            Some(0.0),
            "text/plain content must not be parsed as HTML"
        );
    }

    /// A relative `src` is resolved against the document base URL before fetch.
    #[test]
    fn iframe_relative_src_resolves_against_base_url() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/html",
            r#"<html><body><p id="rel">yes</p></body></html>"#,
        );
        let doc = TreeBuilder::parse(
            r#"<html><body><iframe id="f" src="sub.html"></iframe></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let base: crate::http::Url = format!("http://127.0.0.1:{port}/index.html")
            .parse()
            .unwrap();
        runtime.set_base_url(base);

        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('rel').textContent"
            )
            .as_deref(),
            Some("yes"),
            "a relative iframe src must resolve against the base URL"
        );
    }

    /// `HTMLObjectElement.data` reflects as an absolute URL resolved against the
    /// document base URL. A bare relative reference and a `./`-prefixed one must
    /// resolve to the same absolute URL (Acid3 test 64).
    #[test]
    fn object_data_reflects_as_absolute_url_resolved_against_base_url() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://example.com/dir/page.html".parse().unwrap());
        let script = r#"
            var a = document.createElement('object');
            a.setAttribute('data', 'test.html');
            var b = document.createElement('object');
            b.setAttribute('data', './test.html');
            [a.data, b.data].join('|')
        "#;
        assert_eq!(
            eval_string_value(&mut runtime, script).as_deref(),
            Some("http://example.com/dir/test.html|http://example.com/dir/test.html"),
            "object.data must reflect both relative forms as the same absolute URL"
        );
    }

    /// An `<object>` with no `data` attribute reflects `.data` as the empty
    /// string rather than `null`/`undefined`.
    #[test]
    fn object_data_absent_reflects_as_empty_string() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://example.com/dir/page.html".parse().unwrap());
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "var o = document.createElement('object'); JSON.stringify(o.data)"
            )
            .as_deref(),
            Some("\"\""),
            "object.data with no data attribute must be the empty string"
        );
    }

    /// `HTMLObjectElement.data` reflection preserves a `#fragment` on the
    /// reference: the part before `#` is resolved against the base URL and the
    /// fragment is re-attached to the resolved absolute URL.
    #[test]
    fn object_data_preserves_fragment_when_resolving() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://example.com/dir/page.html".parse().unwrap());
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "var o = document.createElement('object'); o.setAttribute('data', 'test.html#frag'); o.data"
            )
            .as_deref(),
            Some("http://example.com/dir/test.html#frag"),
            "object.data must resolve the path and keep the fragment"
        );
    }

    /// A fragment-only reference (`#frag`) is an empty reference once the
    /// fragment is removed, so it resolves to the base URL itself (RFC 3986
    /// §5.2) with the fragment re-attached — not to the base directory.
    #[test]
    fn object_data_fragment_only_reference_resolves_to_base_with_fragment() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://example.com/dir/page.html".parse().unwrap());
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "var o = document.createElement('object'); o.setAttribute('data', '#frag'); o.data"
            )
            .as_deref(),
            Some("http://example.com/dir/page.html#frag"),
            "a fragment-only reference must resolve to the base URL plus the fragment"
        );
    }

    /// An empty reference (`data=""`) resolves to the base URL string itself
    /// (RFC 3986 §5.2), rather than being treated as a relative path against the
    /// base directory.
    #[test]
    fn object_data_empty_reference_resolves_to_base_url() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_base_url("http://example.com/dir/page.html".parse().unwrap());
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "var o = document.createElement('object'); o.setAttribute('data', ''); o.data"
            )
            .as_deref(),
            Some("http://example.com/dir/page.html"),
            "an empty reference must resolve to the base URL itself"
        );
    }

    /// `setAttribute` for a non-reflected attribute must not leak the attribute
    /// as a same-named JS property on the wrapper: it stays reachable only via
    /// `getAttribute` (Acid3 test 64's non-existent-property check).
    #[test]
    fn element_setattribute_does_not_leak_to_js_property() {
        let mut runtime = JsRuntime::new().unwrap();
        let script = r#"
            var p = document.createElement('p');
            p.setAttribute('foo', 'x');
            [!('foo' in p), p.foo === undefined, p.getAttribute('foo') === 'x'].join('|')
        "#;
        assert_eq!(
            eval_string_value(&mut runtime, script).as_deref(),
            Some("true|true|true"),
            "setAttribute must not create a JS property for the attribute"
        );
    }

    /// Changing an iframe's `src` reloads its document on the next access.
    #[test]
    fn iframe_changing_src_reloads_content_document() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/html",
            r#"<html><body><p id="x">loaded</p></body></html>"#,
        );
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // No src yet: an empty about:blank skeleton with no <p>.
        assert_eq!(
            runtime
                .eval(
                    "document.getElementById('f').contentDocument.getElementsByTagName('p').length"
                )
                .unwrap()
                .as_number(),
            Some(0.0)
        );

        // Point src at the HTML resource; the next read reloads and parses it.
        runtime
            .eval(&format!(
                "document.getElementById('f').src = 'http://127.0.0.1:{port}/x.html';"
            ))
            .unwrap();
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('x').textContent"
            )
            .as_deref(),
            Some("loaded"),
            "changing src must reload the sub-document"
        );
    }

    /// `ownerDocument` reports the document each node belongs to, keeping the
    /// top-level and iframe sub-document contexts separated; a document node
    /// itself has no owner document.
    #[test]
    fn sub_document_nodes_report_their_own_owner_document() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(
            r#"<html><body><span id="host"></span><iframe id="f"></iframe></body></html>"#,
        )
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // A document node has no owner document.
        assert!(
            runtime.eval("document.ownerDocument").unwrap().is_null(),
            "document.ownerDocument must be null"
        );

        // A top-level element is owned by the top-level document.
        assert_eq!(
            runtime
                .eval("document.getElementById('host').ownerDocument === document")
                .unwrap()
                .as_boolean(),
            Some(true),
            "a main-document node must be owned by the main document"
        );

        runtime
            .eval("var sub = document.getElementById('f').contentDocument;")
            .unwrap();

        // A node created by the sub-document is owned by it (even while
        // detached), not by the top-level document.
        assert_eq!(
            runtime
                .eval("sub.createElement('p').ownerDocument === sub")
                .unwrap()
                .as_boolean(),
            Some(true),
            "a node created by the sub-document must be owned by it"
        );
        assert_eq!(
            runtime
                .eval("sub.createElement('p').ownerDocument === document")
                .unwrap()
                .as_boolean(),
            Some(false),
            "a sub-document-created node must not be owned by the top-level document"
        );

        // An existing node inside the sub-document tree is owned by the
        // sub-document, which itself has no owner document.
        assert_eq!(
            runtime
                .eval("sub.body.ownerDocument === sub")
                .unwrap()
                .as_boolean(),
            Some(true),
            "an existing sub-document node must be owned by the sub-document"
        );
        assert!(
            runtime.eval("sub.ownerDocument").unwrap().is_null(),
            "the sub-document node must have no owner document"
        );
    }

    /// `cloneNode` must carry the original's owning document onto the (detached)
    /// clone, including deep-clone descendants. Without the wrapper stamp a
    /// clone of a sub-document node would report the top-level document as its
    /// ownerDocument until inserted somewhere.
    #[test]
    fn cloned_sub_document_node_reports_sub_document_owner() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"var sub = document.getElementById('f').contentDocument;
                   var orig = sub.createElement('p');
                   orig.appendChild(sub.createElement('span'));"#,
            )
            .unwrap();

        // Shallow clone: the clone itself is owned by the sub-document.
        assert_eq!(
            runtime
                .eval("orig.cloneNode(false).ownerDocument === sub")
                .unwrap()
                .as_boolean(),
            Some(true),
            "a shallow clone of a sub-document node must be owned by the sub-document"
        );

        // Deep clone: the clone root is owned by the sub-document...
        runtime.eval("var deep = orig.cloneNode(true);").unwrap();
        assert_eq!(
            runtime
                .eval("deep.ownerDocument === sub")
                .unwrap()
                .as_boolean(),
            Some(true),
            "a deep clone's root must be owned by the sub-document"
        );
        assert_eq!(
            runtime
                .eval("deep.ownerDocument === document")
                .unwrap()
                .as_boolean(),
            Some(false),
            "a deep clone's root must not be owned by the top-level document"
        );
        // ...and so is every descendant of the deep clone.
        assert_eq!(
            runtime
                .eval("deep.firstChild.ownerDocument === sub")
                .unwrap()
                .as_boolean(),
            Some(true),
            "a deep clone's descendant must be owned by the sub-document"
        );
        assert_eq!(
            runtime
                .eval("deep.firstChild.ownerDocument === document")
                .unwrap()
                .as_boolean(),
            Some(false),
            "a deep clone's descendant must not be owned by the top-level document"
        );
    }

    /// Reloading an iframe (its `src` changed) must drop the previous
    /// sub-document tree from the host node registry instead of leaking it, so
    /// the registry does not grow without bound across reloads.
    #[test]
    fn iframe_reload_unregisters_previous_sub_document_tree() {
        use crate::html::TreeBuilder;
        let port =
            spawn_static_http_server("text/html", r#"<html><body><p id="x">A</p></body></html>"#);
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let base = runtime.host_state.borrow().nodes.len();

        // First load registers the sub-document tree.
        runtime
            .eval(&format!(
                "document.getElementById('f').src = 'http://127.0.0.1:{port}/a.html'; \
                 document.getElementById('f').contentDocument;"
            ))
            .unwrap();
        let after_first = runtime.host_state.borrow().nodes.len();
        let sub_tree_size = after_first - base;
        assert!(
            sub_tree_size > 0,
            "loading the sub-document must register its nodes"
        );

        // Changing src to a different URL with identical markup reloads the
        // sub-document. The old tree must be unregistered, so the registry size
        // is unchanged (base + one sub-tree), not doubled.
        runtime
            .eval(&format!(
                "document.getElementById('f').src = 'http://127.0.0.1:{port}/b.html'; \
                 document.getElementById('f').contentDocument;"
            ))
            .unwrap();
        let after_reload = runtime.host_state.borrow().nodes.len();
        assert_eq!(
            after_reload,
            base + sub_tree_size,
            "reloading must unregister the old sub-document tree \
             (expected {} nodes, found {} — the stale tree leaked)",
            base + sub_tree_size,
            after_reload
        );
    }

    /// Reloading an iframe must discard the previous sub-document's per-document
    /// style cache entry (not just its node tree) and seed a fresh entry for the
    /// new document, so the reloaded frame never resolves against the old
    /// document's resolver and the cache does not leak an entry per reload.
    #[test]
    fn iframe_reload_discards_old_document_style_entry() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/html",
            r#"<html><body><style>#t { z-index: 1; position: absolute; }</style><div id="t"></div></body></html>"#,
        );
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // First load, then a computed-style query to actually build the
        // sub-document's resolver (not just seed a dirty placeholder).
        runtime
            .eval(&format!(
                "document.getElementById('f').src = 'http://127.0.0.1:{port}/a.html'; \
                 var d = document.getElementById('f').contentDocument; \
                 getComputedStyle(d.getElementById('t'), '').zIndex;"
            ))
            .unwrap();

        // Hold a strong reference to the old document so its heap address (and
        // thus its `identity()`, which is pointer-based) cannot be reused by the
        // reloaded document. This makes the old/new identity comparison below
        // deterministic — otherwise the freed old document node's address could
        // be recycled for the new one.
        let old_document: NodeHandle = {
            let s = runtime.host_state.borrow();
            s.iframe_documents
                .values()
                .next()
                .expect("one iframe document after first load")
                .document
                .clone()
        };
        let old_doc_id = old_document.identity();
        {
            let s = runtime.host_state.borrow();
            assert!(
                s.document_styles.contains_key(&old_doc_id),
                "first load must create the sub-document's style entry"
            );
            assert!(
                s.document_styles
                    .get(&old_doc_id)
                    .unwrap()
                    .resolver
                    .is_some(),
                "the computed-style query must have built the resolver"
            );
            assert!(
                s.nodes.contains_key(&old_doc_id),
                "first load must register the sub-document node"
            );
        }

        // Reload the iframe with a different URL (identical markup).
        runtime
            .eval(&format!(
                "document.getElementById('f').src = 'http://127.0.0.1:{port}/b.html'; \
                 var d2 = document.getElementById('f').contentDocument; \
                 getComputedStyle(d2.getElementById('t'), '').zIndex;"
            ))
            .unwrap();

        let new_doc_id = {
            let s = runtime.host_state.borrow();
            let (_id, entry) = s
                .iframe_documents
                .iter()
                .next()
                .expect("one iframe document after reload");
            entry.document.identity()
        };
        assert_ne!(
            new_doc_id, old_doc_id,
            "reloading must produce a new document node with a new identity"
        );

        let s = runtime.host_state.borrow();
        assert!(
            !s.document_styles.contains_key(&old_doc_id),
            "the old document's style entry must be discarded on reload"
        );
        assert!(
            !s.nodes.contains_key(&old_doc_id),
            "the old document node must be unregistered on reload"
        );
        assert!(
            s.document_styles.contains_key(&new_doc_id),
            "the reloaded document must have its own style entry"
        );
        assert!(
            s.document_styles
                .get(&new_doc_id)
                .unwrap()
                .resolver
                .is_some(),
            "the reloaded document's resolver must be built after the re-query"
        );
    }

    /// `iframe.contentWindow` returns one stable object: identity holds,
    /// assigned properties persist, and its `document` getter reflects reloads.
    #[test]
    fn iframe_content_window_is_stable_and_reflects_reload() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server(
            "text/html",
            r#"<html><body><p id="loaded">hi</p></body></html>"#,
        );
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert_eq!(
            runtime
                .eval("var f = document.getElementById('f'); f.contentWindow === f.contentWindow")
                .unwrap()
                .as_boolean(),
            Some(true),
            "contentWindow must return a stable object"
        );

        runtime.eval("f.contentWindow.marker = 42;").unwrap();
        assert_eq!(
            runtime.eval("f.contentWindow.marker").unwrap().as_number(),
            Some(42.0),
            "properties set on contentWindow must persist"
        );

        // Before load the sub-document is an empty skeleton (no <p>).
        assert_eq!(
            runtime
                .eval("f.contentWindow.document.getElementsByTagName('p').length")
                .unwrap()
                .as_number(),
            Some(0.0),
            "before load the sub-document has no <p>"
        );

        // After a src change the stable window's document getter reflects the
        // freshly loaded sub-document.
        runtime
            .eval(&format!("f.src = 'http://127.0.0.1:{port}/page.html';"))
            .unwrap();
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "f.contentWindow.document.getElementById('loaded').textContent"
            )
            .as_deref(),
            Some("hi"),
            "the stable contentWindow.document getter must reflect the reload"
        );
        // The window object (and its state) survives the src change.
        assert_eq!(
            runtime.eval("f.contentWindow.marker").unwrap().as_number(),
            Some(42.0),
            "contentWindow identity and state must survive a src change"
        );
    }

    /// A sub-document's `defaultView` is its iframe's `contentWindow` (stable and
    /// round-tripping through `.document`), while the main document's
    /// `defaultView` remains the global window.
    #[test]
    fn iframe_document_default_view_routes_to_content_window() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // The main document still routes to the global window.
        assert_eq!(
            runtime
                .eval("document.defaultView === globalThis && document.defaultView === window")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the main document's defaultView must remain the global window"
        );

        // A sub-document routes to its iframe's contentWindow.
        assert_eq!(
            runtime
                .eval("var f = document.getElementById('f'); f.contentDocument.defaultView === f.contentWindow")
                .unwrap()
                .as_boolean(),
            Some(true),
            "a sub-document's defaultView must be its iframe's contentWindow"
        );
        assert_eq!(
            runtime
                .eval("f.contentDocument.defaultView === f.contentDocument.defaultView")
                .unwrap()
                .as_boolean(),
            Some(true),
            "defaultView must return a stable object across reads"
        );
        assert_eq!(
            runtime
                .eval("f.contentDocument.defaultView.document === f.contentDocument")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the sub-document window's document must be the sub-document itself"
        );
    }

    /// After a `src` change the reloaded document is a new document that routes
    /// to the same stable `contentWindow`; the previous (now stale) document's
    /// `defaultView` reports null rather than the main window.
    #[test]
    fn reloaded_iframe_default_view_follows_new_document() {
        use crate::html::TreeBuilder;
        // Distinct markup per URL so the reload is verified by *content* rather
        // than by node identity (which is pointer-based and may be recycled for
        // the reloaded document, so pre/post-reload identity comparison is not
        // reliable — the stale-document case is pinned in the native-contract
        // test `document_owner_iframe_native_maps_document_to_owning_iframe`).
        let port_a =
            spawn_static_http_server("text/html", r#"<html><body><p id="a">A</p></body></html>"#);
        let port_b =
            spawn_static_http_server("text/html", r#"<html><body><p id="b">B</p></body></html>"#);
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(&format!(
                "var f = document.getElementById('f'); \
                 f.src = 'http://127.0.0.1:{port_a}/a.html'; \
                 f.contentDocument;"
            ))
            .unwrap();

        // Reload with different content.
        runtime
            .eval(&format!(
                "f.src = 'http://127.0.0.1:{port_b}/b.html'; f.contentDocument;"
            ))
            .unwrap();

        // The reload actually happened: the new content is present, the old is
        // gone.
        assert!(
            runtime
                .eval("f.contentDocument.getElementById('b')")
                .unwrap()
                .is_object(),
            "the reloaded document must contain the new content"
        );
        assert!(
            runtime
                .eval("f.contentDocument.getElementById('a')")
                .unwrap()
                .is_null(),
            "the reloaded document must not contain the old content"
        );
        // The reloaded document routes to the same stable contentWindow, and the
        // window's document getter reflects the reload.
        assert_eq!(
            runtime
                .eval("f.contentDocument.defaultView === f.contentWindow")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the reloaded document must route to the same stable contentWindow"
        );
        assert_eq!(
            runtime
                .eval("f.contentWindow.document === f.contentDocument")
                .unwrap()
                .as_boolean(),
            Some(true),
            "the contentWindow's document getter must reflect the reload"
        );
    }

    /// The `__omoikane_document_owner_iframe` binding backing `defaultView`
    /// maps the main document and unknown/stale ids to null, and a live
    /// sub-document to its owning iframe's node id.
    #[test]
    fn document_owner_iframe_native_maps_document_to_owning_iframe() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // The main document is owned by no iframe.
        assert!(
            runtime
                .eval("__omoikane_document_owner_iframe(document.__id)")
                .unwrap()
                .is_null(),
            "the main document must map to null (its window is globalThis)"
        );

        // A live sub-document maps to its owning iframe's node id.
        assert_eq!(
            runtime
                .eval(
                    "var f = document.getElementById('f'); \
                     __omoikane_document_owner_iframe(f.contentDocument.__id) === f.__id"
                )
                .unwrap()
                .as_boolean(),
            Some(true),
            "a sub-document must map to its owning iframe's node id"
        );

        // An id that is neither the main document nor any tracked sub-document
        // (unknown or reloaded/stale) maps to null.
        assert!(
            runtime
                .eval("__omoikane_document_owner_iframe(999999999)")
                .unwrap()
                .is_null(),
            "an unknown/stale document id must map to null, not an iframe"
        );
    }

    /// An unbound `getElementById` (`var g = document.getElementById; g('x')`)
    /// must fall back to the top-level document instead of returning null.
    #[test]
    fn get_element_by_id_unbound_falls_back_to_main_document() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><b id="target">hi</b></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        assert_eq!(
            eval_string_value(
                &mut runtime,
                "var g = document.getElementById; g('target').tagName"
            )
            .as_deref(),
            Some("B"),
            "an unbound getElementById must fall back to the main document"
        );
        // A normal bound call still resolves.
        assert_eq!(
            eval_string_value(&mut runtime, "document.getElementById('target').tagName").as_deref(),
            Some("B")
        );
    }

    /// A sub-document supports `open()`/`write()`/`close()`: the write targets
    /// the sub-document (not the parent), and `open()` clears the previous
    /// content so a following write replaces rather than accumulates.
    #[test]
    fn iframe_sub_document_open_write_close_replaces_content() {
        use crate::html::TreeBuilder;
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   d.open();
                   d.write('<p id="first">one</p>');
                   d.close();"#,
            )
            .unwrap();

        // The written node lands in the sub-document...
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('first').textContent"
            )
            .as_deref(),
            Some("one"),
            "a sub-document write must target the sub-document"
        );
        // ...and not the parent document.
        assert!(
            runtime
                .eval("document.getElementById('first')")
                .unwrap()
                .is_null(),
            "a sub-document write must not leak into the parent document"
        );

        // A second open/write/close replaces, not accumulates.
        runtime
            .eval(
                r#"var d2 = document.getElementById('f').contentDocument;
                   d2.open();
                   d2.write('<p id="second">two</p>');
                   d2.close();"#,
            )
            .unwrap();
        assert!(
            runtime
                .eval("document.getElementById('f').contentDocument.getElementById('first')")
                .unwrap()
                .is_null(),
            "open() must clear the previous sub-document content"
        );
        assert_eq!(
            eval_string_value(
                &mut runtime,
                "document.getElementById('f').contentDocument.getElementById('second').textContent"
            )
            .as_deref(),
            Some("two")
        );
        assert_eq!(
            runtime
                .eval(
                    "document.getElementById('f').contentDocument.getElementsByTagName('p').length"
                )
                .unwrap()
                .as_number(),
            Some(1.0),
            "only the freshly written <p> must remain"
        );
    }

    /// Acid3 test 71 (first write): `open()` + `write(complete document)` +
    /// `close()` on an iframe sub-document must build a doctype plus the
    /// implicit `html`/`head`/`body` structure. The written text is a whole
    /// document (doctype, a head-level `<title>`, and body-level
    /// `<span>`/`<script>`), so it must NOT be treated as a `<body>` fragment.
    #[test]
    fn iframe_open_write_close_builds_full_document_with_doctype() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var doc = document.getElementById('f').contentDocument;
                   doc.open();
                   doc.write('<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.0 Transitional//EN"><title></title><span></span><script type="text/javascript"></script>');
                   doc.close();"#,
            )
            .unwrap();

        // #document children are exactly [doctype, html].
        assert_eq!(eval_num(&mut runtime, "doc.childNodes.length"), 2.0);
        assert_eq!(
            eval_str(&mut runtime, "doc.firstChild.name.toUpperCase()"),
            "HTML"
        );
        assert_eq!(
            eval_str(&mut runtime, "doc.firstChild.publicId"),
            "-//W3C//DTD HTML 4.0 Transitional//EN"
        );
        // No system id was given: native returns "" (test 71 accepts null | "").
        assert_eq!(eval_str(&mut runtime, "doc.firstChild.systemId"), "");
        assert!(
            runtime
                .eval("doc.firstChild.internalSubset")
                .unwrap()
                .is_null(),
            "internalSubset must be null"
        );

        // documentElement = [HEAD, BODY]; head = [TITLE]; body = [SPAN, SCRIPT].
        assert_eq!(
            eval_num(&mut runtime, "doc.documentElement.childNodes.length"),
            2.0
        );
        assert_eq!(
            eval_str(&mut runtime, "doc.documentElement.firstChild.nodeName"),
            "HEAD"
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "doc.documentElement.firstChild.childNodes.length"
            ),
            1.0
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "doc.documentElement.firstChild.firstChild.tagName"
            ),
            "TITLE"
        );
        assert_eq!(
            eval_str(&mut runtime, "doc.documentElement.lastChild.nodeName"),
            "BODY"
        );
        assert_eq!(
            eval_num(
                &mut runtime,
                "doc.documentElement.lastChild.childNodes.length"
            ),
            2.0
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "doc.documentElement.lastChild.firstChild.tagName"
            ),
            "SPAN"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "doc.documentElement.lastChild.lastChild.tagName"
            ),
            "SCRIPT"
        );
    }

    /// Acid3 test 71 (second write): a PUBLIC + SYSTEM doctype exposes both
    /// identifiers, and a `<script>` nested inside `<span>` keeps the nesting
    /// (so `<body>` has a single child).
    #[test]
    fn iframe_open_write_close_reads_system_id_and_keeps_nesting() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var doc = document.getElementById('f').contentDocument;
                   doc.open();
                   doc.write('<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "http://www.w3.org/TR/html4/loose.dtd"><title></title><span><script type="text/javascript"></script></span>');
                   doc.close();"#,
            )
            .unwrap();

        assert_eq!(eval_num(&mut runtime, "doc.childNodes.length"), 2.0);
        assert_eq!(
            eval_str(&mut runtime, "doc.firstChild.publicId"),
            "-//W3C//DTD HTML 4.01 Transitional//EN"
        );
        assert_eq!(
            eval_str(&mut runtime, "doc.firstChild.systemId"),
            "http://www.w3.org/TR/html4/loose.dtd"
        );

        // body = [SPAN]; span = [SCRIPT] (the script is nested in the span).
        assert_eq!(
            eval_num(
                &mut runtime,
                "doc.documentElement.lastChild.childNodes.length"
            ),
            1.0
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "doc.documentElement.lastChild.firstChild.tagName"
            ),
            "SPAN"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "doc.documentElement.lastChild.firstChild.firstChild.tagName"
            ),
            "SCRIPT"
        );
    }

    /// A sub-document `open()` called from a running main-document script must
    /// not disturb the main document's parser insertion point: a following
    /// main-document `document.write` must still land at the script's position,
    /// not be appended to `<body>`. This guards the earlier bug where
    /// `document_reset_native` cleared `write_insertion_ref` unconditionally.
    #[test]
    fn sub_document_open_during_script_preserves_main_write_insertion_point() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body><iframe id="f"></iframe><script>document.getElementById('f').contentDocument.open();document.write('<div id="written"></div>');</script><p id="after"></p></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        let body = doc.query_selector("body").expect("body");
        let tags: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            tags,
            vec![
                "iframe".to_string(),
                "script".to_string(),
                "div".to_string(),
                "p".to_string()
            ],
            "the main-document write must land between the <script> and the \
             following <p> even after a sub-document open() ran mid-script"
        );
    }

    /// `insert_or_append` must append the child when `insert_before` cannot
    /// place it (here the reference node is not a child of the parent), so the
    /// node still lands in the tree instead of being silently dropped.
    #[test]
    fn insert_or_append_falls_back_when_reference_not_a_child() {
        let parent = NodeHandle::element("div");
        let existing = NodeHandle::element("span");
        parent.append_child(existing.clone());

        // A reference node that is NOT a child of `parent` makes insert_before
        // fail with ReferenceChildNotFound.
        let detached_reference = NodeHandle::element("p");
        let child = NodeHandle::element("b");
        insert_or_append(&parent, &child, Some(&detached_reference));

        // The child was appended (fallback), landing at the end of the parent.
        let tags: Vec<String> = parent
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            tags,
            vec!["span".to_string(), "b".to_string()],
            "fallback must append the child so it stays in the tree"
        );
        assert_eq!(
            child.parent_node(),
            Some(parent),
            "the appended child must actually be parented"
        );
    }

    /// `is_inline_classic_script` gates which written `<script>`s run: inline
    /// classic scripts do, but external (`src`) and module scripts do not.
    #[test]
    fn is_inline_classic_script_classifies_scripts() {
        // Inline classic: no src, no/empty/JS type.
        assert!(is_inline_classic_script(&NodeHandle::element("script")));

        let typed = NodeHandle::element("script");
        typed.set_attribute("type", "text/javascript");
        assert!(is_inline_classic_script(&typed));

        // External: has src.
        let external = NodeHandle::element("script");
        external.set_attribute("src", "x.js");
        assert!(!is_inline_classic_script(&external));

        // application/javascript is also a classic type.
        let app_js = NodeHandle::element("script");
        app_js.set_attribute("type", "application/javascript");
        assert!(is_inline_classic_script(&app_js));

        // A MIME type with parameters still matches on its essence.
        let with_params = NodeHandle::element("script");
        with_params.set_attribute("type", "text/javascript; charset=utf-8");
        assert!(is_inline_classic_script(&with_params));

        // Module: type="module".
        let module = NodeHandle::element("script");
        module.set_attribute("type", "module");
        assert!(!is_inline_classic_script(&module));

        // Non-JS type.
        let non_js = NodeHandle::element("script");
        non_js.set_attribute("type", "text/plain");
        assert!(!is_inline_classic_script(&non_js));

        // Other JavaScript MIME essences (e.g. text/ecmascript) are *not*
        // executed — the classic-script gate is narrower than the full
        // JavaScript MIME type match and mirrors `execute_document_scripts`.
        let ecmascript = NodeHandle::element("script");
        ecmascript.set_attribute("type", "text/ecmascript");
        assert!(!is_inline_classic_script(&ecmascript));

        // Not a <script> at all.
        assert!(!is_inline_classic_script(&NodeHandle::element("div")));
    }

    /// A deferred (external) script's `document.write` must insert at that
    /// script's position, not fall back to appending at the end of `<body>`.
    #[test]
    fn document_write_from_defer_script_inserts_at_script_position() {
        use crate::html::TreeBuilder;
        // The defer script is an external (data: URI) script — the only kind to
        // which `defer` applies. Its body writes `<b id="written"></b>`.
        // The percent-encoded source decodes to:
        //   document.write('<b id="written"></b>')
        let html = r#"<html><body><script defer src="data:text/javascript,document.write('%3Cb%20id=%22written%22%3E%3C/b%3E')"></script><p id="after"></p></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        let body = doc.query_selector("body").expect("body");
        let tags: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            tags,
            vec!["script".to_string(), "b".to_string(), "p".to_string()],
            "the deferred write must land right after its <script>, before the <p>"
        );
    }

    /// A written external (`src`) `<script>` is inserted into the DOM but is
    /// NOT executed — its inline text is ignored per the HTML spec, and only
    /// inline classic scripts run synchronously via document.write.
    #[test]
    fn document_write_external_script_present_but_not_executed() {
        use crate::html::TreeBuilder;
        // The inline text would set a global *if* it were (wrongly) executed.
        let html = r#"<html><body>
            <script>document.write('<script src="x.js" id="ext">globalThis.__ext_ran = true;<\/script>');</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        // Present in the DOM.
        let ext = doc
            .query_selector("#ext")
            .expect("written <script src> must exist in the DOM");
        assert_eq!(ext.tag_name().as_deref(), Some("script"));
        assert_eq!(
            ext.attributes()
                .unwrap_or_default()
                .get("src")
                .map(|s| s.as_str()),
            Some("x.js")
        );

        // But not executed.
        let ran = runtime
            .eval("typeof globalThis.__ext_ran")
            .unwrap()
            .as_string()
            .map(|s| s.to_std_string_escaped());
        assert_eq!(
            ran.as_deref(),
            Some("undefined"),
            "an external (src) script must not run via document.write"
        );
    }

    /// A written `type="module"` script must be inserted but NOT executed as a
    /// classic script.
    #[test]
    fn document_write_module_script_not_executed_as_classic() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body>
            <script>document.write('<script type="module" id="mod">globalThis.__mod_ran = true;<\/script>');</script>
        </body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        let errors = runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");

        let module = doc
            .query_selector("#mod")
            .expect("written module script must exist in the DOM");
        assert_eq!(module.tag_name().as_deref(), Some("script"));

        let ran = runtime
            .eval("typeof globalThis.__mod_ran")
            .unwrap()
            .as_string()
            .map(|s| s.to_std_string_escaped());
        assert_eq!(
            ran.as_deref(),
            Some("undefined"),
            "a module script must not run synchronously as a classic script"
        );
    }

    /// A `type="text/ecmascript"` script must be treated identically whether it
    /// is parsed normally or inserted via `document.write`: with the shared type
    /// gate, neither path executes it (the classic-script gate is narrower than
    /// the full JavaScript MIME type match). This pins the two paths together so
    /// they cannot drift apart.
    #[test]
    fn ecmascript_type_script_is_not_executed_by_either_path() {
        use crate::html::TreeBuilder;

        // Path 1: normal parse.
        let normal_html = r#"<html><body>
            <script type="text/ecmascript" id="ecma">globalThis.__ecma_normal = true;</script>
        </body></html>"#;
        let normal_doc = TreeBuilder::parse(normal_html).document();
        let mut normal_runtime = JsRuntime::with_document(normal_doc.clone()).unwrap();
        let errors = normal_runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");
        assert!(
            normal_doc.query_selector("#ecma").is_some(),
            "the ecmascript <script> must remain in the DOM"
        );
        let normal_ran = normal_runtime
            .eval("typeof globalThis.__ecma_normal")
            .unwrap()
            .as_string()
            .map(|s| s.to_std_string_escaped());
        assert_eq!(
            normal_ran.as_deref(),
            Some("undefined"),
            "a text/ecmascript script must not run when parsed normally"
        );

        // Path 2: inserted via document.write.
        let written_html = r#"<html><body>
            <script>document.write('<script type="text/ecmascript" id="ecma">globalThis.__ecma_written = true;<\/script>');</script>
        </body></html>"#;
        let written_doc = TreeBuilder::parse(written_html).document();
        let mut written_runtime = JsRuntime::with_document(written_doc.clone()).unwrap();
        let errors = written_runtime.execute_document_scripts(None);
        assert!(errors.is_empty(), "no script errors expected: {errors:?}");
        assert!(
            written_doc.query_selector("#ecma").is_some(),
            "the written ecmascript <script> must be inserted into the DOM"
        );
        let written_ran = written_runtime
            .eval("typeof globalThis.__ecma_written")
            .unwrap()
            .as_string()
            .map(|s| s.to_std_string_escaped());
        assert_eq!(
            written_ran.as_deref(),
            Some("undefined"),
            "a text/ecmascript script must not run when inserted via document.write, \
             matching the normal-parse path"
        );
    }

    /// `document.open()` must empty the document (HTML's document open steps).
    #[test]
    fn document_open_empties_the_document() {
        use crate::html::TreeBuilder;
        let html = r#"<html><head></head><body><p id="old"></p></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        assert!(
            !doc.child_nodes().is_empty(),
            "precondition: the document starts with children"
        );

        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        runtime.eval("document.open()").unwrap();

        assert!(
            doc.child_nodes().is_empty(),
            "document.open() must remove every document child"
        );
    }

    /// `document.open()` followed by `write()`/`close()` must leave only the
    /// freshly written content, wrapped in the implicit `html`/`head`/`body`
    /// structure a real parser builds. The written `<div>` is a body fragment,
    /// so after open() emptied the document the document node's sole element
    /// child is `<html>` (not the bare `<div>`), and the `<div>` lands in
    /// `<body>` — matching browser behaviour for a complete-document write.
    #[test]
    fn document_open_write_close_leaves_only_written_content() {
        use crate::html::TreeBuilder;
        let html = r#"<html><body><p id="old"></p></body></html>"#;
        let doc = TreeBuilder::parse(html).document();
        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();

        runtime.eval("document.open()").unwrap();
        runtime
            .eval(r#"document.write('<div id="fresh"></div>')"#)
            .unwrap();
        runtime.eval("document.close()").unwrap();

        // The old content is gone.
        assert!(
            doc.query_selector("#old").is_none(),
            "document.open() must have erased the old content"
        );
        // The document's only element child is the implicit <html> root.
        let element_children: Vec<String> = doc
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            element_children,
            vec!["html".to_string()],
            "the emptied document must be rebuilt with a single <html> root"
        );
        // <html> holds the implicit <head> and <body>.
        let html = doc.query_selector("html").expect("implicit <html>");
        let html_children: Vec<String> = html
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            html_children,
            vec!["head".to_string(), "body".to_string()],
            "the implicit <html> must contain <head> then <body>"
        );
        // The freshly written <div> is a body-level fragment, so it lands in
        // <body>, reachable by id.
        let fresh = doc
            .query_selector("#fresh")
            .expect("the written content must be present after open/write/close");
        assert_eq!(fresh.tag_name().as_deref(), Some("div"));
        let body = doc.query_selector("body").expect("implicit <body>");
        let body_children: Vec<String> = body
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            body_children,
            vec!["div".to_string()],
            "the written <div> must live inside <body>"
        );
    }

    // -- document-scoped computed style (issue 016-15) ------------------------
    //
    // `getComputedStyle` must resolve against the cascade of the document the
    // queried node actually lives in: an iframe sub-document uses its own
    // `<style>` rules, never the main document's, and vice versa. These tests
    // exercise the per-document resolver cache and its invalidation.

    /// A `<style>` added to an iframe sub-document is reflected by
    /// `getComputedStyle` on a node in that sub-document — the core regression
    /// behind Acid3's selectorTest (which styles the iframe contentDocument).
    #[test]
    fn get_computed_style_uses_iframe_subdocument_stylesheet() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var style = d.createElement('style');
                   style.textContent = '#target { z-index: 7; position: absolute; }';
                   d.body.appendChild(style);
                   var target = d.createElement('div');
                   target.id = 'target';
                   d.body.appendChild(target);"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(target, '').zIndex"),
            "7",
            "getComputedStyle must reflect the iframe sub-document's own <style> rule"
        );
    }

    /// Neither document's rules leak into the other: a rule that exists only in
    /// the main document does not apply in the sub-document, and vice versa.
    /// Elements with no matching rule report the default z-index (empty string).
    #[test]
    fn get_computed_style_does_not_leak_between_main_and_iframe() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>#mainonly { z-index: 3; position: absolute; }</style></head>
               <body><div id="mainonly"></div><div id="subonly"></div>
               <iframe id="f"></iframe></body></html>"#,
        );
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#subonly { z-index: 9; position: absolute; }';
                   d.body.appendChild(s);
                   var subMain = d.createElement('div'); subMain.id = 'mainonly'; d.body.appendChild(subMain);
                   var subOnly = d.createElement('div'); subOnly.id = 'subonly'; d.body.appendChild(subOnly);"#,
            )
            .unwrap();

        // Main document: its own rule applies; the sub-document's rule does not.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('mainonly'), '').zIndex"
            ),
            "3",
            "the main document's own rule must apply in the main document"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('subonly'), '').zIndex"
            ),
            "",
            "the sub-document's rule must NOT leak into the main document"
        );
        // Sub-document: its own rule applies; the main document's rule does not.
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(subOnly, '').zIndex"),
            "9",
            "the sub-document's own rule must apply in the sub-document"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(subMain, '').zIndex"),
            "",
            "the main document's rule must NOT leak into the sub-document"
        );
    }

    /// Two iframe sub-documents keep independent resolvers: the same selector
    /// with a different z-index resolves per document, proving the cache is
    /// keyed on each document's identity rather than shared.
    #[test]
    fn get_computed_style_keeps_two_iframe_styles_isolated() {
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="a"></iframe><iframe id="b"></iframe></body></html>"#,
        );
        runtime
            .eval(
                r#"var da = document.getElementById('a').contentDocument;
                   var sa = da.createElement('style');
                   sa.textContent = '#t { z-index: 4; position: absolute; }';
                   da.body.appendChild(sa);
                   var ta = da.createElement('div'); ta.id = 't'; da.body.appendChild(ta);

                   var db = document.getElementById('b').contentDocument;
                   var sb = db.createElement('style');
                   sb.textContent = '#t { z-index: 8; position: absolute; }';
                   db.body.appendChild(sb);
                   var tb = db.createElement('div'); tb.id = 't'; db.body.appendChild(tb);"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(ta, '').zIndex"),
            "4",
            "iframe A must resolve against its own stylesheet"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(tb, '').zIndex"),
            "8",
            "iframe B must resolve against its own stylesheet"
        );
    }

    /// Mutating a sub-document `<style>`'s `textContent` after it has been
    /// queried recomputes that sub-document's resolver, and only its resolver —
    /// the main document's cached value is untouched.
    #[test]
    fn iframe_style_text_content_mutation_recomputes_only_subdocument() {
        // The main document carries its own `<div id="t">` — the same id the
        // sub-document's rule targets — so a leak of the sub-document rule into
        // the main resolver would be directly observable on it.
        let mut runtime = runtime_from_html(
            r#"<html><head><style>#m { z-index: 5; position: absolute; }</style></head>
               <body><div id="m"></div><div id="t"></div><iframe id="f"></iframe></body></html>"#,
        );
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var subStyle = d.createElement('style');
                   subStyle.textContent = '#t { z-index: 1; position: absolute; }';
                   d.body.appendChild(subStyle);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        // Prime both resolvers.
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "1"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('m'), '').zIndex"
            ),
            "5"
        );
        // The main document's own `#t` has no rule, so its baseline is default.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('t'), '').zIndex"
            ),
            "",
            "the main document has no #t rule, so its #t element starts at the default z-index"
        );

        // Mutate only the sub-document's stylesheet.
        runtime
            .eval("subStyle.textContent = '#t { z-index: 2; position: absolute; }';")
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "2",
            "the sub-document must recompute after its <style> text changed"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('m'), '').zIndex"
            ),
            "5",
            "the main document's resolver must be untouched by a sub-document mutation"
        );
        // The sub-document's `#t` rule must not leak onto the main document's
        // `#t` element, which shares the id the rule targets.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('t'), '').zIndex"
            ),
            "",
            "the sub-document #t rule must not apply to the main document's #t element"
        );
    }

    /// Appending a `<style>` element to a sub-document *after* an initial query
    /// (when the resolver is already cached) recomputes it — the exact shape of
    /// Acid3's selectorTest, which reads a node, then adds a rule, then re-reads.
    #[test]
    fn iframe_style_element_append_after_first_query_recomputes() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        // First query, before any rule exists: default z-index.
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "",
            "with no rule the target must report the default z-index"
        );

        // Add a rule after the resolver was already built and cached.
        runtime
            .eval(
                r#"var s = d.createElement('style');
                   s.textContent = '#t { z-index: 5; position: absolute; }';
                   d.body.appendChild(s);"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "5",
            "appending a <style> after the first query must recompute the resolver"
        );
    }

    /// Removing a sub-document `<style>` element recomputes the sub-document's
    /// resolver so the previously applied rule no longer matches.
    #[test]
    fn iframe_remove_style_recomputes_subdocument() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#t { z-index: 6; position: absolute; }';
                   d.body.appendChild(s);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "6"
        );

        runtime.eval("s.parentNode.removeChild(s);").unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "",
            "removing the <style> must drop its rule from the sub-document resolver"
        );
    }

    /// `document.open()` on a sub-document clears its cached styles: after a
    /// following `write()` only the newly written rules apply; a previously
    /// present rule for the same id no longer matches.
    #[test]
    fn iframe_document_open_clears_cached_styles() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#old { z-index: 1; position: absolute; }';
                   d.body.appendChild(s);
                   var o = d.createElement('div'); o.id = 'old'; d.body.appendChild(o);"#,
            )
            .unwrap();
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(o, '').zIndex"),
            "1"
        );

        runtime
            .eval(
                r#"d.open();
                   d.write('<style>#new { z-index: 4; position: absolute; }</style><div id="new"></div><div id="old"></div>');
                   d.close();"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(d.getElementById('new'), '').zIndex"
            ),
            "4",
            "the freshly written rule must apply after open()/write()"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(d.getElementById('old'), '').zIndex"
            ),
            "",
            "the pre-open() rule must not survive document.open()"
        );
    }

    /// A sub-document `document.write` of a `<style>` marks only that
    /// sub-document dirty: the new rule applies there and the main document's
    /// cached resolver is not polluted.
    #[test]
    fn iframe_document_write_marks_own_document_dirty() {
        // The main document carries a `<div id="w">` — the id the written
        // sub-document rule targets — so any leak of that rule into the main
        // resolver would be directly observable on it.
        let mut runtime = runtime_from_html(
            r#"<html><head><style>#m { z-index: 7; position: absolute; }</style></head>
               <body><div id="m"></div><div id="w"></div><iframe id="f"></iframe></body></html>"#,
        );
        // Prime both resolvers.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('m'), '').zIndex"
            ),
            "7"
        );
        // The main document has no #w rule, so its #w element starts at default.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('w'), '').zIndex"
            ),
            "",
            "the main document has no #w rule, so its #w element starts at the default z-index"
        );
        runtime
            .eval("var d = document.getElementById('f').contentDocument; d.body;")
            .unwrap();

        runtime
            .eval(
                r#"d.write('<style>#w { z-index: 2; position: absolute; }</style><div id="w"></div>');"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(d.getElementById('w'), '').zIndex"
            ),
            "2",
            "a written sub-document rule must apply in the sub-document"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('m'), '').zIndex"
            ),
            "7",
            "a sub-document write must not pollute the main document's resolver"
        );
        // The written sub-document `#w` rule must not leak onto the main
        // document's `#w` element, which shares the id the rule targets.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('w'), '').zIndex"
            ),
            "",
            "the written sub-document #w rule must not apply to the main document's #w element"
        );
    }

    /// Moving a `<style>` element from the main document into a sub-document
    /// invalidates *both* resolvers: the rule disappears from the main document
    /// and appears in the sub-document. Guards the cross-document-move case
    /// where marking only the destination would leave the source stale.
    #[test]
    fn moving_style_between_documents_invalidates_both_resolvers() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style id="s">#t { z-index: 3; position: absolute; }</style></head>
               <body><div id="t"></div><iframe id="f"></iframe></body></html>"#,
        );
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var subT = d.createElement('div'); subT.id = 't'; d.body.appendChild(subT);"#,
            )
            .unwrap();

        // Prime: rule applies in the main document, not in the sub-document.
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('t'), '').zIndex"
            ),
            "3"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(subT, '').zIndex"),
            ""
        );

        // Move the <style> element from the main document into the sub-document.
        runtime
            .eval("d.body.appendChild(document.getElementById('s'));")
            .unwrap();

        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(document.getElementById('t'), '').zIndex"
            ),
            "",
            "the rule must disappear from the source (main) document"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(subT, '').zIndex"),
            "3",
            "the rule must appear in the destination (sub) document"
        );
    }

    /// A sub-document resolver uses its owning iframe's laid-out content box
    /// for `vw`/`vh` resolution. This fixture's iframe fills the 800px parent
    /// width and has zero laid-out height.
    #[test]
    fn iframe_computed_style_uses_configured_viewport() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime.set_viewport(800.0, 600.0);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#t { width: 50vw; height: 25vh; }';
                   d.body.appendChild(s);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').width"),
            "400px",
            "50vw of an 800px viewport must resolve to 400px in the sub-document"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').height"),
            "0px",
            "25vh of the iframe's zero-height viewport must resolve to 0px"
        );
    }

    /// The iframe viewport follows the constrained layout result, rather than
    /// the unconstrained width/height declarations in its style attribute.
    #[test]
    fn iframe_viewport_uses_layout_size_when_max_size_constrains_style() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 iframe { width: 200px; max-width: 80px; height: 120px; max-height: 40px; }
               </style></head><body>
                 <iframe id="f" style="width: 200px; height: 120px"></iframe>
               </body></html>"#,
        );
        runtime.set_viewport(800.0, 600.0);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#t { width: 50vw; height: 50vh; }';
                   d.body.appendChild(s);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').width"),
            "40px",
            "50vw must use the max-width-constrained 80px layout width, not style width 200px"
        );
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').height"),
            "20px",
            "50vh must use the max-height-constrained 40px layout height, not style height 120px"
        );
    }

    /// Changing the viewport after a sub-document resolver was already built
    /// invalidates it, so a re-query resolves `vw`/`vh` against the new size.
    #[test]
    fn set_viewport_invalidates_existing_iframe_resolver() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime.set_viewport(800.0, 600.0);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#t { width: 50vw; }';
                   d.body.appendChild(s);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').width"),
            "400px"
        );

        // Widen the viewport: the cached sub-document resolver must rebuild.
        runtime.set_viewport(1000.0, 600.0);
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').width"),
            "500px",
            "a viewport change must invalidate the cached sub-document resolver"
        );
    }

    /// Acid3 test 46 regression: media features are evaluated against the
    /// iframe's own 0x0 viewport and re-evaluated after its CSS size changes.
    #[test]
    fn acid3_media_queries_use_and_recompute_iframe_viewport() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 iframe { width: 0; height: 0; }
                 iframe.large { width: 100px; height: 100px; }
               </style></head>
               <body><iframe id="f"></iframe></body></html>"#,
        );
        runtime.eval(r#"
            var d = document.getElementById('f').contentDocument;
            var s = d.createElement('style');
            s.textContent =
                '@media all and (min-color: 0) { #a { text-transform: uppercase; } }' +
                '@media (bogus), all { #h { text-transform: uppercase; } }' +
                '@media (min-color: 1), (min-monochrome: 1) { #v { text-transform: uppercase; } }' +
                '@media all and (min-color: 0) and (min-monochrome: 0) { #w { text-transform: uppercase; } }' +
                '@media all and (min-height: 1em) and (min-width: 1em) { #y1 { text-transform: uppercase; } }' +
                '@media all and (max-height: 1em) and (max-width: 1em) { #y4 { text-transform: uppercase; } }';
            d.head.appendChild(s);
            for (const id of ['a', 'plain', 'h', 'v', 'w', 'y1', 'y4']) {
                var p = d.createElement('p'); p.id = id; d.body.appendChild(p);
            }
        "#,
            )
            .unwrap();

        for id in ["a", "h", "v", "w", "y4"] {
            assert_eq!(
                eval_str(
                    &mut runtime,
                    &format!("getComputedStyle(d.getElementById('{id}'), '').textTransform")
                ),
                "uppercase"
            );
        }
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(d.getElementById('plain'), '').textTransform"
            ),
            "none"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(d.getElementById('y1'), '').textTransform"
            ),
            "none"
        );

        runtime
            .eval("document.getElementById('f').setAttribute('class', 'large')")
            .unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(d.getElementById('y1'), '').textTransform"
            ),
            "uppercase"
        );
        assert_eq!(
            eval_str(
                &mut runtime,
                "getComputedStyle(d.getElementById('y4'), '').textTransform"
            ),
            "none"
        );
    }

    /// Injecting a `<style>` into a sub-document element via `innerHTML` (after
    /// the resolver was primed with a first query) must invalidate that
    /// sub-document so the new rule is picked up on the next `getComputedStyle`.
    #[test]
    fn iframe_inner_html_style_injection_recomputes_subdocument() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var host = d.createElement('div'); d.body.appendChild(host);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        // Prime the sub-document resolver before any rule exists.
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "",
            "with no rule the target must report the default z-index"
        );

        // Inject a `<style>` nested inside `host` through innerHTML.
        runtime
            .eval(r#"host.innerHTML = '<style>#t { z-index: 8; position: absolute; }</style>';"#)
            .unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "8",
            "an innerHTML-injected <style> must recompute the primed sub-document resolver"
        );
    }

    /// Changing a sub-document element's `class` via `setAttribute` after the
    /// resolver was primed must invalidate that sub-document so a class selector
    /// re-matches on the next query.
    #[test]
    fn iframe_set_attribute_class_rematches_after_prime() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="f"></iframe></body></html>"#);
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '.hot { z-index: 9; position: absolute; }';
                   d.body.appendChild(s);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        // Prime: the target has no `class`, so `.hot` does not match yet.
        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "",
            "before the class is set the .hot rule must not match"
        );

        runtime.eval("t.setAttribute('class', 'hot');").unwrap();

        assert_eq!(
            eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"),
            "9",
            "a setAttribute('class', ...) must invalidate the primed sub-document and re-match .hot"
        );
    }

    /// Deterministic port of Acid3 bucket-3 `selectorTest` cases (tests
    /// 33/35/36/41/42) that use selectors the matcher already supports. This
    /// exercises the exact path Acid3 uses — append `* { z-index: 0; position:
    /// absolute; }` plus `selector { z-index: N }` to a fresh iframe
    /// sub-document, then read `doc.defaultView.getComputedStyle(node,'').zIndex`
    /// — so it pins 016-15's contribution (document-scoped resolver +
    /// sub-document `defaultView`). Selector *coverage* itself is issue 016-10,
    /// so only already-supported selectors appear here.
    #[test]
    fn acid3_supported_selector_cases_resolve_in_iframe_document() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="selectors"></iframe></body></html>"#);
        // Harness mirroring Acid3's getTestDocument()/selectorTest(): a fresh
        // sub-document with a `<style>` seeded with the universal baseline rule,
        // an `addRule` that appends `sel { z-index: N }`, and a `zi` reader that
        // goes through `defaultView.getComputedStyle`.
        runtime
            .eval(
                r#"
                function setupTestDoc() {
                  var doc = document.getElementById('selectors').contentDocument;
                  var de = doc.documentElement;
                  while (de.firstChild) de.removeChild(de.firstChild);
                  var head = doc.createElement('head'); de.appendChild(head);
                  var body = doc.createElement('body'); de.appendChild(body);
                  var style = doc.createElement('style');
                  style.appendChild(doc.createTextNode('* { z-index: 0; position: absolute; }\n'));
                  head.appendChild(style);
                  globalThis.__doc = doc;
                  globalThis.__style = style;
                  globalThis.__n = 0;
                  return doc;
                }
                function addRule(sel) {
                  globalThis.__n += 1;
                  __style.appendChild(__doc.createTextNode(sel + ' { z-index: ' + __n + '; }\n'));
                  return __n;
                }
                function zi(node) { return __doc.defaultView.getComputedStyle(node, '').zIndex; }
                "#,
            )
            .unwrap();

        // test 33: class selector — matched element raises z-index, others keep
        // the universal baseline of 0.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var p = doc.createElement('p'); doc.body.appendChild(p);
                    p.className = 'selectorPingTest';
                    addRule('.selectorPingTest');
                    return [zi(p), zi(doc.body)].join(',');
                })()"#
            ),
            "1,0",
            "class selector must match the target and no other element"
        );

        // test 33: attribute selector `[title=...]` reached through the
        // `HTMLElement.title` IDL setter (Acid3 sets `p.title = ...`, not
        // `setAttribute`). The setter must reflect into the `title` attribute so
        // the selector matches.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var p = doc.createElement('p'); doc.body.appendChild(p);
                    p.title = 'selectorPingTest';
                    addRule('[title=selectorPingTest]');
                    return [zi(p), zi(doc.body)].join(',');
                })()"#
            ),
            "1,0",
            "the title IDL setter must reflect into [title=...] and match the target only"
        );

        // test 35: :first-child. The parentless root element must never match it
        // (its parent is the Document, not an element).
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var first = addRule(':first-child');
                    var p1 = doc.createElement('p'); doc.body.appendChild(p1);
                    var p2 = doc.createElement('p'); doc.body.appendChild(p2);
                    return [zi(doc.documentElement), zi(p1), zi(p2)].join(',');
                })()"#
            ),
            "0,1,0",
            ":first-child must match only the first child and never the parentless root element"
        );

        // test 36: :last-child.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var p1 = doc.createElement('p'); doc.body.appendChild(p1);
                    var p2 = doc.createElement('p'); doc.body.appendChild(p2);
                    addRule(':last-child');
                    return [zi(p1), zi(p2)].join(',');
                })()"#
            ),
            "0,1",
            ":last-child must match only the last child"
        );

        // test 41: :not(:root) — the root element is excluded, all others match.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var p = doc.createElement('p'); doc.body.appendChild(p);
                    addRule(':not(:root)');
                    return [zi(doc.documentElement), zi(doc.body), zi(p)].join(',');
                })()"#
            ),
            "0,1,1",
            ":not(:root) must exclude only the root element"
        );

        // test 42: descendant + child combinators in `#div1 > div div > div`.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var d1 = doc.createElement('div'); d1.id = 'div1'; doc.body.appendChild(d1);
                    var d2 = doc.createElement('div'); d1.appendChild(d2);
                    var d3 = doc.createElement('div'); d2.appendChild(d3);
                    var d4 = doc.createElement('div'); d3.appendChild(d4);
                    var d5 = doc.createElement('div'); d4.appendChild(d5);
                    var d6 = doc.createElement('div'); d5.appendChild(d6);
                    addRule('#div1 > div div > div');
                    return [zi(d1), zi(d2), zi(d3), zi(d4), zi(d5), zi(d6)].join(',');
                })()"#
            ),
            "0,0,0,1,1,1",
            "the combinator chain must match only the deepest three divs"
        );

        // test 42 (cont.): adjacent-sibling combinator `h1 + p` — only the `p`
        // immediately preceded by the `h1` matches.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var h = doc.createElement('h1'); doc.body.appendChild(h);
                    var p = doc.createElement('p'); doc.body.appendChild(p);
                    addRule('h1 + p');
                    return [zi(h), zi(p)].join(',');
                })()"#
            ),
            "0,1",
            "the adjacent-sibling combinator must match only the immediately following element"
        );

        // test 42 (cont.): general-sibling combinator `h1 ~ p` — every `p`
        // following the `h1` in the same parent matches.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    var h = doc.createElement('h1'); doc.body.appendChild(h);
                    var p1 = doc.createElement('p'); doc.body.appendChild(p1);
                    var p2 = doc.createElement('p'); doc.body.appendChild(p2);
                    addRule('h1 ~ p');
                    return [zi(h), zi(p1), zi(p2)].join(',');
                })()"#
            ),
            "0,1,1",
            "the general-sibling combinator must match all following siblings"
        );

        // test 42 (cont.): `insertBefore` into a *primed* sub-document must
        // recompute the sub-document's resolver (insert_before_native marks the
        // target document dirty). A `:first-child` rule is primed against `p2`
        // (then the only child → matches); inserting `p1` before it must move
        // the match to `p1` and drop it from `p2`.
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(function(){
                    var doc = setupTestDoc();
                    addRule(':first-child');
                    var p2 = doc.createElement('p'); doc.body.appendChild(p2);
                    var primed = zi(p2);
                    var p1 = doc.createElement('p'); doc.body.insertBefore(p1, p2);
                    return [primed, zi(p1), zi(p2)].join(',');
                })()"#
            ),
            "1,1,0",
            "insertBefore into a primed sub-document must re-match :first-child against the new first child"
        );
    }

    /// Deterministic iframe + computed-style regressions for the selectors
    /// added by issue 016-10 (Acid3 tests 34/37/38/39/40/43).
    #[test]
    fn acid3_extended_selector_cases_resolve_in_iframe_document() {
        let mut runtime =
            runtime_from_html(r#"<html><body><iframe id="selectors"></iframe></body></html>"#);
        let result = eval_str(
            &mut runtime,
            r#"
            (() => {
              const doc = document.getElementById('selectors').contentDocument;
              function setup(rules) {
                const de = doc.documentElement;
                while (de.firstChild) de.removeChild(de.firstChild);
                const head = doc.createElement('head'); de.appendChild(head);
                const body = doc.createElement('body'); de.appendChild(body);
                const style = doc.createElement('style');
                style.textContent = '* { z-index: 0; position: absolute; }\n' + rules;
                head.appendChild(style);
                return body;
              }
              const zi = node => doc.defaultView.getComputedStyle(node, '').zIndex;
              const out = [];

              let body = setup(':lang(en) { z-index: 1 }\n[class|=widget] { z-index: 2 }');
              const lang = doc.createElement('div'); lang.setAttribute('lang', 'en-GB'); body.appendChild(lang);
              const inherited = doc.createElement('p'); lang.appendChild(inherited);
              const dash = doc.createElement('p'); dash.className = 'widget-blue'; body.appendChild(dash);
              out.push([zi(lang), zi(inherited), zi(dash)].join(','));

              body = setup(':only-child { z-index: 1 }');
              const only = doc.createElement('p'); body.appendChild(doc.createTextNode('x')); body.appendChild(only);
              const before = zi(only); const extra = doc.createElement('p'); body.appendChild(extra);
              const after = zi(only); body.removeChild(extra); out.push([before, after, zi(only)].join(','));

              body = setup(':empty { z-index: 1 }');
              const empty = doc.createElement('p'); body.appendChild(empty); empty.appendChild(doc.createComment('x')); empty.appendChild(doc.createTextNode(''));
              const emptyBefore = zi(empty); empty.appendChild(doc.createTextNode(' ')); out.push([emptyBefore, zi(empty)].join(','));

              body = setup(':nth-child(-n+3) { z-index: 1 }\n:nth-last-child(2) { z-index: 2 }');
              const children = []; for (let i = 0; i < 5; i++) { const p = doc.createElement('p'); body.appendChild(p); children.push(p); }
              out.push(children.map(zi).join(','));

              body = setup(':first-of-type { z-index: 1 }\n:nth-of-type(3n-1) { z-index: 2 }\n:nth-last-of-type(-5n+3) { z-index: 3 }');
              const ps = []; for (let i = 0; i < 6; i++) { body.appendChild(doc.createElement('span')); const p = doc.createElement('p'); body.appendChild(p); ps.push(p); }
              out.push(ps.map(zi).join(','));

              body = setup(':enabled { z-index: 1 }\n:disabled { z-index: 2 }\n:checked { z-index: 3 }\n:checked:enabled { z-index: 4 }');
              const input = doc.createElement('input'); input.type = 'checkbox'; body.appendChild(input);
              const enabled = zi(input); input.click(); const checked = zi(input); input.disabled = true; const disabled = zi(input); input.checked = false;
              out.push([enabled, checked, disabled, zi(input), zi(body)].join(','));
              return out.join(';');
            })()
            "#,
        );
        assert_eq!(result, "1,1,2;1,0,1;1,0;1,1,1,2,0;1,2,0,3,2,0;1,4,3,2,0");
    }

    // --- Document focus state / focus event core (issue #243) ---

    fn focus_runtime() -> JsRuntime {
        runtime_from_html(
            r#"<html><body>
                 <input id="a">
                 <input id="b">
                 <input id="disabled" disabled>
                 <div id="divdisabled" disabled></div>
               </body></html>"#,
        )
    }

    /// Installs a recorder for all four focus events on the given targets.
    /// Each entry is `target:type:bubbles:relatedTarget:activeElement`.
    const FOCUS_LOG_SETUP: &str = r#"
        globalThis.focusLog = [];
        globalThis.recordFocusEvents = (label, target, capture) => {
          for (const type of ["blur", "focusout", "focus", "focusin"]) {
            target.addEventListener(type, event => {
              const related = event.relatedTarget ? event.relatedTarget.id : "null";
              const active = document.activeElement === document.body
                ? "body"
                : (document.activeElement ? document.activeElement.id : "null");
              focusLog.push([label, event.type, event.bubbles, related, active].join(":"));
            }, !!capture);
          }
        };
    "#;

    #[test]
    fn focus_tracks_active_element_and_blur_falls_back_to_body() {
        let mut runtime = focus_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const b = document.getElementById("b");
                const out = [];
                out.push(document.activeElement === document.body);
                a.focus();
                out.push(document.activeElement === a);
                b.focus();
                out.push(document.activeElement === b);
                b.blur();
                out.push(document.activeElement === document.body);
                return out.join(",");
            })()"#,
        );
        assert_eq!(result, "true,true,true,true");
    }

    #[test]
    fn focus_dispatches_blur_focusout_focus_focusin_in_order() {
        let mut runtime = focus_runtime();
        runtime.eval(FOCUS_LOG_SETUP).unwrap();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const b = document.getElementById("b");
                recordFocusEvents("a", a);
                recordFocusEvents("b", b);
                recordFocusEvents("doc", document);
                a.focus();
                b.focus();
                return focusLog.join("|");
            })()"#,
        );
        // The first focus fires no blur (nothing was focused) and carries a null
        // relatedTarget. Moving a -> b fires blur/focusout on a before
        // focus/focusin on b, and only the bubbling pair reaches the document.
        // During blur/focusout the active element is already the body fallback.
        assert_eq!(
            result,
            concat!(
                "a:focus:false:null:a",
                "|a:focusin:true:null:a",
                "|doc:focusin:true:null:a",
                "|a:blur:false:b:body",
                "|a:focusout:true:b:body",
                "|doc:focusout:true:b:body",
                "|b:focus:false:a:b",
                "|b:focusin:true:a:b",
                "|doc:focusin:true:a:b",
            )
        );
    }

    #[test]
    fn focus_events_are_composed_and_not_cancelable() {
        let mut runtime = focus_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const seen = [];
                for (const type of ["focus", "focusin"]) {
                  a.addEventListener(type, event => seen.push([
                    event.type,
                    event.composed,
                    event.cancelable,
                    event instanceof FocusEvent,
                  ].join(":")));
                }
                a.focus();
                return seen.join("|");
            })()"#,
        );
        assert_eq!(result, "focus:true:false:true|focusin:true:false:true");
    }

    #[test]
    fn focus_events_carry_related_target_both_directions() {
        let mut runtime = focus_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const b = document.getElementById("b");
                const related = [];
                for (const target of [a, b]) {
                  for (const type of ["blur", "focusout", "focus", "focusin"]) {
                    target.addEventListener(type, event => related.push(
                      target.id + "." + type + "=" +
                      (event.relatedTarget === null ? "null" : event.relatedTarget.id)
                    ));
                  }
                }
                a.focus();
                b.focus();
                a.focus();
                a.blur();
                return related.join(",");
            })()"#,
        );
        // relatedTarget is the element on the other side of the transition, and
        // null when focus arrives from or returns to the document viewport.
        assert_eq!(
            result,
            concat!(
                "a.focus=null,a.focusin=null,",
                "a.blur=b,a.focusout=b,b.focus=a,b.focusin=a,",
                "b.blur=a,b.focusout=a,a.focus=b,a.focusin=b,",
                "a.blur=null,a.focusout=null",
            )
        );
    }

    #[test]
    fn refocusing_the_same_element_dispatches_no_events() {
        let mut runtime = focus_runtime();
        runtime.eval(FOCUS_LOG_SETUP).unwrap();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                a.focus();
                recordFocusEvents("a", a);
                recordFocusEvents("doc", document);
                a.focus();
                a.focus();
                return [focusLog.length, document.activeElement === a].join(",");
            })()"#,
        );
        assert_eq!(result, "0,true");
    }

    #[test]
    fn blur_on_a_non_focused_element_is_a_no_op() {
        let mut runtime = focus_runtime();
        runtime.eval(FOCUS_LOG_SETUP).unwrap();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const b = document.getElementById("b");
                a.focus();
                recordFocusEvents("a", a);
                recordFocusEvents("b", b);
                recordFocusEvents("doc", document);
                b.blur();
                document.body.blur();
                return [focusLog.length, document.activeElement === a].join(",");
            })()"#,
        );
        assert_eq!(result, "0,true");
    }

    #[test]
    fn focus_ignores_disconnected_nodes_and_disabled_controls() {
        let mut runtime = focus_runtime();
        runtime.eval(FOCUS_LOG_SETUP).unwrap();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const out = [];
                recordFocusEvents("doc", document);

                const detached = document.createElement("input");
                detached.focus();
                out.push(document.activeElement === document.body);

                const disabled = document.getElementById("disabled");
                disabled.focus();
                out.push(document.activeElement === document.body);

                // A disabled control stays unfocusable even with a tabindex.
                disabled.setAttribute("tabindex", "0");
                disabled.focus();
                out.push(document.activeElement === document.body);

                // focus()/blur() live on HTMLElement, so a Text node does not
                // expose them at all (as in Firefox 152).
                const text = document.createTextNode("t");
                document.body.appendChild(text);
                out.push(text.focus === undefined && text.blur === undefined);

                out.push(focusLog.length === 0);

                // Disabling a control only blocks new focus attempts; the
                // already focused control keeps focus (as in Firefox 152).
                const b = document.getElementById("b");
                b.focus();
                b.disabled = true;
                out.push(document.activeElement === b);
                return out.join(",");
            })()"#,
        );
        assert_eq!(result, "true,true,true,true,true,true");
    }

    /// Focusability table verified against Firefox 152 via Marionette. Each
    /// entry is `selector=focused|unchanged` where `unchanged` means `focus()`
    /// left the previously focused anchor input alone.
    #[test]
    fn focus_only_applies_to_focusable_areas() {
        let mut runtime = runtime_from_html(
            r#"<html><body>
                 <input id="anchor">
                 <div id="tabindex-negative" tabindex="-1"></div>
                 <div id="tabindex-zero" tabindex="0"></div>
                 <span id="tabindex-float" tabindex="1.5"></span>
                 <span id="tabindex-signed" tabindex="+2"></span>
                 <span id="tabindex-invalid" tabindex="abc"></span>
                 <a id="anchor-href" href="x">l</a>
                 <a id="anchor-plain">l</a>
                 <div id="editing-host" contenteditable="true"><span id="editing-child">x</span></div>
                 <div id="editing-empty" contenteditable=""></div>
                 <div id="editing-false" contenteditable="false"></div>
                 <details><summary id="summary-in-details">s</summary>d</details>
                 <summary id="summary-orphan">s</summary>
                 <input id="input-hidden" type="hidden">
                 <input id="input-readonly" readonly>
                 <button id="button">b</button>
                 <button id="button-disabled" disabled>b</button>
                 <select id="select"><option id="option">o</option></select>
                 <textarea id="textarea"></textarea>
                 <iframe id="iframe"></iframe>
                 <embed id="embed">
                 <object id="object"></object>
                 <audio id="audio-controls" controls></audio>
                 <audio id="audio-plain"></audio>
                 <video id="video-controls" controls></video>
                 <fieldset id="fieldset"></fieldset>
                 <img id="img" alt="i">
                 <label id="label">l</label>
                 <div id="plain-div"></div>
                 <dialog id="dialog">d</dialog>
                 <details id="details">d</details>
               </body></html>"#,
        );
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const anchor = document.getElementById("anchor");
                const ids = [
                  "tabindex-negative", "tabindex-zero", "tabindex-float", "tabindex-signed",
                  "tabindex-invalid", "anchor-href", "anchor-plain", "editing-host",
                  "editing-child", "editing-empty", "editing-false", "summary-in-details",
                  "summary-orphan", "input-hidden", "input-readonly", "button",
                  "button-disabled", "select", "option", "textarea", "iframe", "embed",
                  "object", "audio-controls", "audio-plain", "video-controls", "fieldset",
                  "img", "label", "plain-div", "dialog", "details",
                ];
                return ids.map(id => {
                  anchor.focus();
                  const element = document.getElementById(id);
                  element.focus();
                  const state = document.activeElement === element
                    ? "focused"
                    : (document.activeElement === anchor ? "unchanged" : "other");
                  return id + "=" + state;
                }).join(",");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                "tabindex-negative=focused,tabindex-zero=focused,",
                "tabindex-float=focused,tabindex-signed=focused,tabindex-invalid=unchanged,",
                "anchor-href=focused,anchor-plain=unchanged,",
                "editing-host=focused,editing-child=unchanged,editing-empty=focused,",
                "editing-false=unchanged,",
                "summary-in-details=focused,summary-orphan=unchanged,",
                "input-hidden=unchanged,input-readonly=focused,",
                "button=focused,button-disabled=unchanged,",
                "select=focused,option=unchanged,textarea=focused,",
                "iframe=focused,embed=focused,object=unchanged,",
                "audio-controls=focused,audio-plain=unchanged,video-controls=focused,",
                "fieldset=unchanged,img=unchanged,label=unchanged,plain-div=unchanged,",
                "dialog=unchanged,details=unchanged",
            )
        );
    }

    /// The body is the active-element fallback but is not itself a focusable
    /// area, so `body.focus()` does not move focus — unless it carries a
    /// tabindex. Verified against Firefox 152.
    #[test]
    fn body_is_only_focusable_with_a_tabindex() {
        let mut runtime = focus_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const out = [];
                a.focus();
                document.body.focus();
                out.push(document.activeElement === a);
                document.body.setAttribute("tabindex", "-1");
                document.body.focus();
                out.push(document.activeElement === document.body);
                return out.join(",");
            })()"#,
        );
        assert_eq!(result, "true,true");
    }

    #[test]
    fn removing_the_focused_element_falls_back_to_body_without_events() {
        let mut runtime = focus_runtime();
        runtime.eval(FOCUS_LOG_SETUP).unwrap();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const b = document.getElementById("b");
                a.focus();
                recordFocusEvents("a", a);
                recordFocusEvents("b", b);
                recordFocusEvents("doc", document);

                const out = [];
                // The focus fixup rule moves the active element back to the
                // viewport without firing blur or focusout.
                a.remove();
                out.push(document.activeElement === document.body);
                out.push(focusLog.length === 0);

                // The removed element must not receive a blur when focus moves
                // on: only b's own focus pair is dispatched.
                b.focus();
                out.push(focusLog.join("/"));
                return out.join(",");
            })()"#,
        );
        assert_eq!(
            result,
            "true,true,b:focus:false:null:b/b:focusin:true:null:b/doc:focusin:true:null:b"
        );
    }

    #[test]
    fn moving_the_focused_element_to_another_document_clears_the_active_element() {
        let mut runtime = runtime_from_html(
            r#"<html><body><input id="a"><iframe id="f"></iframe></body></html>"#,
        );
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const sub = document.getElementById("f").contentDocument;
                a.focus();
                const out = [document.activeElement === a];
                sub.body.appendChild(a);
                out.push(document.activeElement === document.body);
                out.push(sub.activeElement === sub.body);
                return out.join(",");
            })()"#,
        );
        assert_eq!(result, "true,true,true");
    }

    #[test]
    fn active_element_falls_back_to_document_element_without_body() {
        let mut runtime = focus_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const out = [];
                document.body.remove();
                out.push(document.activeElement === document.documentElement);
                document.documentElement.remove();
                out.push(document.activeElement === null);
                return out.join(",");
            })()"#,
        );
        assert_eq!(result, "true,true");
    }

    #[test]
    fn active_element_and_has_focus_are_isolated_per_iframe_document() {
        let mut runtime = runtime_from_html(
            r#"<html><body><input id="a"><iframe id="f"></iframe></body></html>"#,
        );
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const a = document.getElementById("a");
                const sub = document.getElementById("f").contentDocument;
                const subInput = sub.createElement("input");
                sub.body.appendChild(subInput);
                const out = [];

                // The top-level document starts out focused.
                out.push(document.hasFocus() === true);
                out.push(sub.hasFocus() === false);
                out.push(sub.activeElement === sub.body);

                a.focus();
                out.push(document.activeElement === a);
                out.push(sub.activeElement === sub.body);
                out.push(document.hasFocus() === true && sub.hasFocus() === false);

                // Focusing inside the iframe moves the focused browsing context.
                // The sub-document points at its own element while the parent
                // points at the iframe hosting it.
                subInput.focus();
                out.push(sub.activeElement === subInput);
                out.push(document.activeElement === document.getElementById("f"));
                out.push(document.hasFocus() === true && sub.hasFocus() === true);

                // Blurring inside the iframe keeps that document focused.
                subInput.blur();
                out.push(sub.activeElement === sub.body);
                out.push(sub.hasFocus() === true);
                return out.join(",");
            })()"#,
        );
        assert_eq!(
            result,
            "true,true,true,true,true,true,true,true,true,true,true"
        );
    }

    #[test]
    fn detached_document_never_reports_focus() {
        let mut runtime = focus_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const detached = document.implementation.createHTMLDocument("t");
                return [detached.hasFocus() === false, document.hasFocus() === true].join(",");
            })()"#,
        );
        assert_eq!(result, "true,true");
    }

    // --- Element scroll offsets / scroll container state (issue #245) ---

    /// A 100x50 scroll container holding 300x200 of content, plus variants for
    /// the non-scrollable and box-less cases. Values below were verified against
    /// Firefox 152 over Marionette.
    fn scroll_runtime() -> JsRuntime {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 body { width: 600px; height: 400px }
                 .box { width: 100px; height: 50px }
                 .box > .content { width: 300px; height: 200px }
                 #hidden { overflow: hidden }
                 #visible { overflow: visible }
                 #clip { overflow: clip }
                 #auto { overflow: auto }
                 #scroll { overflow: scroll }
                 #fits { overflow: hidden }
                 #fits > .content { width: 10px; height: 10px }
                 #none { overflow: hidden; display: none }
               </style></head><body>
                 <div class="box" id="hidden"><div class="content"></div></div>
                 <div class="box" id="visible"><div class="content"></div></div>
                 <div class="box" id="clip"><div class="content"></div></div>
                 <div class="box" id="auto"><div class="content"></div></div>
                 <div class="box" id="scroll"><div class="content"></div></div>
                 <div class="box" id="fits"><div class="content"></div></div>
                 <div class="box" id="none"><div class="content"></div></div>
               </body></html>"#,
        );
        runtime.set_viewport(600.0, 400.0);
        runtime
    }

    #[test]
    fn element_scroll_offset_clamps_to_the_scrollable_extent() {
        let mut runtime = scroll_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const box = document.getElementById("hidden");
                const out = [];
                out.push([box.scrollTop, box.scrollLeft].join(","));
                box.scrollTop = 25;
                box.scrollLeft = 40;
                out.push([box.scrollTop, box.scrollLeft].join(","));
                // scrollWidth 300 - clientWidth 100, scrollHeight 200 - clientHeight 50.
                box.scrollTop = 99999;
                box.scrollLeft = 99999;
                out.push([box.scrollTop, box.scrollLeft].join(","));
                box.scrollTop = -10;
                box.scrollLeft = -10;
                out.push([box.scrollTop, box.scrollLeft].join(","));
                box.scrollTop = NaN;
                out.push(box.scrollTop);
                box.scrollLeft = Infinity;
                out.push(box.scrollLeft);
                return out.join("|");
            })()"#,
        );
        assert_eq!(result, "0,0|25,40|150,200|0,0|0|0");
    }

    #[test]
    fn element_scroll_requires_a_scroll_container_with_room_to_scroll() {
        let mut runtime = scroll_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const scrolled = id => {
                  const element = document.getElementById(id);
                  element.scrollTop = 25;
                  element.scrollLeft = 40;
                  return id + "=" + element.scrollTop + "," + element.scrollLeft;
                };
                return ["hidden", "auto", "scroll", "visible", "clip", "fits", "none"]
                  .map(scrolled).join("|");
            })()"#,
        );
        // `hidden`, `auto` and `scroll` are scroll containers; `visible` and
        // `clip` are not, content that fits has nowhere to go, and a
        // `display: none` element has no box at all.
        assert_eq!(
            result,
            "hidden=25,40|auto=25,40|scroll=25,40|visible=0,0|clip=0,0|fits=0,0|none=0,0"
        );
    }

    #[test]
    fn element_scroll_methods_accept_numbers_and_options() {
        let mut runtime = scroll_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const box = document.getElementById("hidden");
                const at = () => [box.scrollLeft, box.scrollTop].join(",");
                const out = [];
                box.scrollTo(5, 6);
                out.push(at());
                box.scrollBy(2, 3);
                out.push(at());
                box.scrollTo({ left: 11, top: 12 });
                out.push(at());
                box.scrollBy({ top: 1 });
                out.push(at());
                box.scroll(0, 0);
                out.push(at());
                // Absent dictionary members keep the current offset.
                box.scrollTo(30, 30);
                box.scrollTo({ left: 1 });
                out.push(at());
                box.scrollTo({ top: 2, behavior: "smooth" });
                out.push(at());
                return out.join("|");
            })()"#,
        );
        assert_eq!(result, "5,6|7,9|11,12|11,13|0,0|1,30|1,2");
    }

    #[test]
    fn element_scroll_dispatches_a_non_bubbling_scroll_event_on_change() {
        let mut runtime = scroll_runtime();
        runtime
            .eval(
            r#"(() => {
                const box = document.getElementById("hidden");
                globalThis.scrollLog = [];
                box.addEventListener("scroll", event => scrollLog.push(
                  "el:" + event.bubbles + ":" + event.cancelable + ":" + (event.target === box)
                ));
                document.addEventListener("scroll", () => scrollLog.push("doc"));
                globalThis.addEventListener("scroll", () => scrollLog.push("win"));
                box.onscroll = () => scrollLog.push("onscroll");

                box.scrollTop = 20;
                // Re-scrolling to the same offset, and clamped no-ops, change
                // nothing and must stay silent.
                box.scrollTop = 20;
                box.scrollTop = 99999;
                box.scrollTop = 99999;
                box.scrollLeft = -1;
                return scrollLog.length;
            })()"#,
        )
            .unwrap();
        assert_eq!(eval_num(&mut runtime, "scrollLog.length"), 0.0);
        runtime.run_animation_frame(16).unwrap();
        // Multiple changes in one frame coalesce; the event does not bubble.
        assert_eq!(
            eval_str(&mut runtime, "scrollLog.join('/')"),
            "el:false:false:true/onscroll"
        );
    }

    #[test]
    fn viewport_scroll_targets_document_and_bubbles_to_window() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>* { margin: 0 } body { height: 1000px }</style></head><body></body></html>"#,
        );
        runtime.set_viewport(200.0, 100.0);
        runtime
            .eval(
                r#"globalThis.viewportScrollLog = [];
                   document.addEventListener("scroll", event => viewportScrollLog.push(
                     "doc:" + (event.target === document) + ":" + event.bubbles));
                   addEventListener("scroll", event => viewportScrollLog.push(
                     "win:" + (event.target === document))); scrollTo(0, 50);"#,
            )
            .unwrap();
        assert_eq!(eval_num(&mut runtime, "viewportScrollLog.length"), 0.0);
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(
            eval_str(&mut runtime, "viewportScrollLog.join('/')"),
            "doc:true:true/win:true"
        );
    }

    #[test]
    fn element_scroll_clamp_queues_event_and_listener_rescroll_is_not_recursive() {
        let mut runtime = scroll_runtime();
        runtime
            .eval(
                r#"globalThis.clampEvents = 0;
                   const box = document.getElementById("hidden");
                   box.addEventListener("scroll", () => { clampEvents++; });
                   box.scrollTop = 150;"#,
            )
            .unwrap();
        runtime.run_animation_frame(16).unwrap();
        runtime.eval("clampEvents = 0; document.getElementById('hidden').firstElementChild.style.height = '60px'").unwrap();
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(
            eval_str(
                &mut runtime,
                "[document.getElementById('hidden').scrollTop, clampEvents].join('|')"
            ),
            "10|1"
        );

        let mut runtime = scroll_runtime();
        runtime
            .eval(
                r#"globalThis.clampEvents = 0;
                   const box = document.getElementById("hidden");
                   box.onscroll = () => { clampEvents++; box.scrollTop += 1; };
                   box.scrollTop = 1;"#,
            )
            .unwrap();
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(eval_num(&mut runtime, "clampEvents"), 1.0);
        runtime.run_animation_frame(16).unwrap();
        assert_eq!(eval_num(&mut runtime, "clampEvents"), 2.0);
    }

    #[test]
    fn element_scroll_moves_descendant_client_rects_but_not_its_own() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 body { width: 600px; height: 400px }
                 #outer { overflow: hidden; width: 100px; height: 100px }
                 #mid { width: 400px; height: 400px }
                 #static { width: 20px; height: 20px }
                 #inner { overflow: hidden; width: 60px; height: 60px }
                 #innerchild { width: 300px; height: 300px }
                 #innerstatic { width: 10px; height: 10px }
               </style></head><body><div id="outer"><div id="mid">
                 <div id="static"></div>
                 <div id="inner"><div id="innerchild"><div id="innerstatic"></div></div></div>
               </div></div></body></html>"#,
        );
        runtime.set_viewport(600.0, 400.0);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const at = id => {
                  const rect = document.getElementById(id).getBoundingClientRect();
                  return [rect.left, rect.top].join(",");
                };
                const outer = document.getElementById("outer");
                const inner = document.getElementById("inner");
                const out = [at("static"), at("inner"), at("innerstatic")];
                outer.scrollTop = 30;
                outer.scrollLeft = 15;
                out.push(at("static"), at("inner"), at("innerstatic"));
                inner.scrollTop = 40;
                inner.scrollLeft = 20;
                out.push(at("innerstatic"));
                // The container's own box and its layout-relative metrics stay
                // where layout put them.
                out.push(at("outer"));
                const target = document.getElementById("static");
                out.push([target.offsetTop, target.offsetLeft, inner.clientTop].join(","));
                return out.join("|");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                "0,0|0,20|0,20",
                "|-15,-30|-15,-10|-15,-10",
                "|-35,-50",
                "|0,0",
                "|0,0,0",
            )
        );
    }

    #[test]
    fn sticky_client_rect_and_hit_test_share_window_scroll_geometry() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 body { width: 80px; height: 300px }
                 #before { height: 30px }
                 #sticky { position: sticky; top: 5px; width: 20px; height: 10px }
                 #after { height: 260px }
               </style></head><body><div id="before"></div><div id="sticky"></div>
                 <div id="after"></div></body></html>"#,
        );
        runtime.set_viewport(80.0, 60.0);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const target = document.getElementById("sticky");
                const before = target.getBoundingClientRect().top;
                window.scrollTo(0, 40);
                const rect = target.getBoundingClientRect();
                return [getComputedStyle(target).position, before, rect.top,
                        rect.bottom, target.offsetTop].join(",");
            })()"#,
        );
        assert_eq!(result, "sticky,30,5,15,30");
        let hit = runtime.hit_test(5.0, 6.0).expect("sticky box must be hit");
        assert_eq!(hit.get_attribute("id").as_deref(), Some("sticky"));
    }

    #[test]
    fn sticky_uses_nearest_nested_scrollport_and_combines_window_scroll() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 body { width: 100px; height: 300px }
                 #lead { height: 50px }
                 #outer { width: 50px; height: 50px; overflow: hidden }
                 #content { width: 150px; height: 150px }
                 #before { width: 30px; height: 30px }
                 #sticky { position: sticky; top: 4px; left: 3px; width: 10px; height: 10px }
               </style></head><body><div id="lead"></div><div id="outer"><div id="content">
                 <div id="before"></div><div id="sticky"></div></div></div></body></html>"#,
        );
        runtime.set_viewport(100.0, 80.0);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const outer = document.getElementById("outer");
                const sticky = document.getElementById("sticky");
                outer.scrollTo(20, 40);
                const first = sticky.getBoundingClientRect();
                window.scrollTo(0, 30);
                const second = sticky.getBoundingClientRect();
                return [first.left, first.top, second.left, second.top,
                        outer.getBoundingClientRect().top].join(",");
            })()"#,
        );
        assert_eq!(result, "3,54,3,24,20");
        let hit = runtime.hit_test(5.0, 25.0).expect("nested sticky box must be hit");
        assert_eq!(hit.get_attribute("id").as_deref(), Some("sticky"));
    }

    #[test]
    fn sticky_preserves_transformed_scrollport_clip_for_rects_and_hit_test() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 #outer { width: 40px; height: 40px; overflow: hidden;
                          transform: translate(10px, 6px) }
                 #content { width: 40px; height: 100px }
                 #before { height: 20px }
                 #sticky { position: sticky; top: 2px; width: 10px; height: 10px }
               </style></head><body><div id="outer"><div id="content">
                 <div id="before"></div><div id="sticky"></div></div></div></body></html>"#,
        );
        runtime.set_viewport(80.0, 80.0);
        assert_eq!(
            eval_str(
                &mut runtime,
                r#"(() => {
                    const outer = document.getElementById("outer");
                    outer.scrollTop = 30;
                    const rect = document.getElementById("sticky").getBoundingClientRect();
                    return [rect.left, rect.top, rect.right, rect.bottom].join(",");
                })()"#,
            ),
            "10,8,20,18"
        );
        let hit = runtime.hit_test(11.0, 9.0).expect("transformed sticky box must be hit");
        assert_eq!(hit.get_attribute("id").as_deref(), Some("sticky"));
        assert!(runtime.hit_test(11.0, 47.0).is_none(), "overflow clip must remain active");
    }

    #[test]
    fn absolute_client_rect_uses_positioned_containing_block_scroll_chain() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 #outer { position: relative; width: 50px; height: 30px; overflow: hidden }
                 #outer-flow { width: 100px; height: 1px }
                 #inner { width: 20px; height: 20px; overflow: hidden }
                 #inner-flow { width: 60px; height: 20px }
                 #target { position: absolute; left: 30px; top: 5px; width: 10px; height: 10px }
               </style></head><body><div id="outer"><div id="outer-flow"></div>
                 <div id="inner"><div id="inner-flow"></div><div id="target"></div></div>
               </div></body></html>"#,
        );
        runtime.set_viewport(100.0, 100.0);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const outer = document.getElementById("outer");
                const inner = document.getElementById("inner");
                const target = document.getElementById("target");
                const left = () => target.getBoundingClientRect().left;
                const out = [left()];
                inner.scrollLeft = 10;
                out.push(left());
                outer.scrollLeft = 10;
                out.push(left());
                return out.join(",");
            })()"#,
        );
        assert_eq!(result, "30,30,20");
    }

    #[test]
    fn fixed_positioning_opts_out_of_ancestor_scroll_but_not_its_own() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 body { width: 600px; height: 2000px }
                 #outer { overflow: hidden; width: 100px; height: 100px }
                 #tall { width: 400px; height: 400px }
                 #pinned { position: fixed; left: 3px; top: 4px; width: 50px; height: 50px;
                           overflow: hidden }
                 #pinnedchild { width: 200px; height: 200px }
                 #pinnedtarget { width: 5px; height: 5px }
               </style></head><body><div id="outer"><div id="tall">
                 <div id="pinned"><div id="pinnedchild"><div id="pinnedtarget"></div></div></div>
               </div></div></body></html>"#,
        );
        runtime.set_viewport(600.0, 400.0);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const at = id => {
                  const rect = document.getElementById(id).getBoundingClientRect();
                  return [rect.left, rect.top].join(",");
                };
                const out = [at("pinned"), at("pinnedtarget")];
                scrollTo(0, 50);
                document.getElementById("outer").scrollTop = 30;
                document.getElementById("outer").scrollLeft = 15;
                // Neither the Window scroll nor the scroll container above it
                // moves a fixed box or its content.
                out.push(at("pinned"), at("pinnedtarget"));
                // The fixed box is itself a scroll container, so its own offset
                // still moves its content.
                document.getElementById("pinned").scrollTop = 20;
                out.push(at("pinned"), at("pinnedtarget"));
                return out.join("|");
            })()"#,
        );
        assert_eq!(result, "3,4|3,4|3,4|3,4|3,4|3,-16");
    }

    #[test]
    fn element_scroll_offset_resets_when_the_element_is_detached() {
        let mut runtime = scroll_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const box = document.getElementById("hidden");
                const parent = box.parentNode;
                const sibling = document.getElementById("visible");
                const out = [];

                box.scrollTop = 100;
                out.push(box.scrollTop);
                parent.removeChild(box);
                out.push(box.scrollTop);
                parent.appendChild(box);
                out.push(box.scrollTop);

                // Moving to another parent and reordering inside one parent both
                // detach the element first, so both reset the offset.
                box.scrollTop = 100;
                document.getElementById("auto").appendChild(box);
                out.push(box.scrollTop);
                box.scrollTop = 100;
                const before = box.scrollTop;
                parent.insertBefore(box, sibling);
                out.push([before, box.scrollTop].join(","));
                return out.join("|");
            })()"#,
        );
        assert_eq!(result, "100|0|0|0|100,0");
    }

    #[test]
    fn element_scroll_offset_returns_when_the_box_comes_back() {
        let mut runtime = scroll_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const box = document.getElementById("hidden");
                const out = [];
                box.scrollTop = 100;
                out.push(box.scrollTop);

                // No box: reported as zero, remembered for later.
                box.style.display = "none";
                out.push(box.scrollTop);
                box.style.display = "";
                out.push(box.scrollTop);

                // No scrolling box: same deal.
                box.style.overflow = "visible";
                out.push(box.scrollTop);
                box.style.overflow = "hidden";
                out.push(box.scrollTop);

                // An unrelated style change keeps it untouched.
                box.style.backgroundColor = "red";
                out.push(box.scrollTop);
                return out.join("|");
            })()"#,
        );
        assert_eq!(result, "100|0|100|0|100|100");
    }

    #[test]
    fn element_scroll_offset_reports_the_extent_after_content_shrinks() {
        let mut runtime = scroll_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const box = document.getElementById("hidden");
                const content = box.firstElementChild;
                box.scrollTop = 150;
                const before = box.scrollTop;
                content.style.height = "60px";
                // scrollHeight 60 against a 50px padding box leaves 10.
                return [before, box.scrollTop, box.scrollHeight].join(",");
            })()"#,
        );
        assert_eq!(result, "150,10,60");
    }

    #[test]
    fn element_scroll_setter_is_ignored_without_a_scrolling_box() {
        let mut runtime = scroll_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const orphan = document.createElement("div");
                orphan.style.overflow = "hidden";
                orphan.style.width = "10px";
                orphan.style.height = "10px";
                const child = document.createElement("div");
                child.style.width = "100px";
                child.style.height = "100px";
                orphan.appendChild(child);
                const out = [];
                orphan.scrollTop = 5;
                out.push(orphan.scrollTop);
                document.body.appendChild(orphan);
                // The discarded write must not resurface once the box exists.
                out.push(orphan.scrollTop);
                orphan.scrollTop = 5;
                out.push(orphan.scrollTop);
                return out.join("|");
            })()"#,
        );
        assert_eq!(result, "0|0|5");
    }

    #[test]
    fn document_element_scroll_reflects_the_window_scroll() {
        let mut runtime = runtime_from_html(
            r#"<html><head><style>
                 * { margin: 0; padding: 0 }
                 body { width: 600px; height: 2000px }
               </style></head><body></body></html>"#,
        );
        runtime.set_viewport(600.0, 700.0);
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const root = document.documentElement;
                const out = [];
                scrollTo(0, 300);
                out.push([root.scrollTop, document.body.scrollTop, scrollY].join(","));
                root.scrollTop = 100;
                out.push([scrollY, root.scrollTop].join(","));
                // The body is not a scroll container here, so writing to it does
                // not move the viewport.
                document.body.scrollTop = 55;
                out.push([scrollY, document.body.scrollTop].join(","));
                // scrollHeight 2000 against a 700px viewport.
                root.scrollTop = 99999;
                out.push([scrollY, root.scrollTop].join(","));
                root.scrollTo(0, 120);
                out.push(scrollY);
                root.scrollBy(0, 5);
                out.push(scrollY);
                return out.join("|");
            })()"#,
        );
        assert_eq!(result, "300,0,300|100,100|100,0|1300,1300|120|125");
    }

    #[test]
    fn dialog_show_modal_close_moves_and_restores_focus() {
        let mut runtime = runtime_from_html(
            r#"<html><body><button id="before">before</button><dialog id="dialog"><button id="inside" tabindex="-1" autofocus>inside</button></dialog></body></html>"#,
        );
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const before = document.getElementById("before");
                const dialog = document.getElementById("dialog");
                const log = [];
                before.focus();
                dialog.addEventListener("close", event => log.push([
                  event.type, event.bubbles, event.cancelable, dialog.returnValue,
                  document.activeElement.id,
                ].join(":")));
                dialog.showModal();
                const opened = [
                  dialog instanceof HTMLDialogElement,
                  dialog.open,
                  document.activeElement.id,
                ].join(":");
                dialog.close("accepted");
                return opened + "|" + log.join("|") + "|" + dialog.open;
            })()"#,
        );
        assert_eq!(result, "true:true:inside|close:false:false:accepted:before|false");
    }

    #[test]
    fn dialog_escape_cancels_only_the_top_modal_and_honors_prevent_default() {
        let mut runtime = runtime_from_html(
            r#"<html><body><button id="before">before</button><dialog id="outer"><button id="outerButton">outer</button></dialog><dialog id="inner"><button id="innerButton">inner</button></dialog></body></html>"#,
        );
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const before = document.getElementById("before");
                const outer = document.getElementById("outer");
                const inner = document.getElementById("inner");
                const log = [];
                before.focus();
                outer.addEventListener("cancel", () => log.push("outer-cancel"));
                outer.addEventListener("close", () => log.push("outer-close"));
                inner.addEventListener("cancel", event => {
                  log.push("inner-cancel");
                  if (!inner.dataset.allowed) event.preventDefault();
                });
                inner.addEventListener("close", () => log.push("inner-close"));
                outer.showModal();
                inner.showModal();
                __omoikane_dispatch_keyboard_input("keydown", { key: "Escape", code: "Escape" });
                log.push("blocked=" + inner.open + ":" + document.activeElement.id);
                inner.dataset.allowed = "yes";
                __omoikane_dispatch_keyboard_input("keydown", { key: "Escape", code: "Escape" });
                log.push("inner=" + inner.open + ":" + document.activeElement.id);
                __omoikane_dispatch_keyboard_input("keydown", { key: "Escape", code: "Escape" });
                log.push("outer=" + outer.open + ":" + document.activeElement.id);
                return log.join("|");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                "inner-cancel|blocked=true:innerButton|inner-cancel|inner-close|",
                "inner=false:outerButton|outer-cancel|outer-close|outer=false:before",
            )
        );
    }

    #[test]
    fn dialog_rejects_invalid_modal_state_and_non_modal_show_is_idempotent() {
        let mut runtime = runtime_from_html("<html><body></body></html>");
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const dialog = document.createElement("dialog");
                const before = document.createElement("button");
                const outside = document.createElement("button");
                const detachedNonModal = document.createElement("dialog");
                outside.id = "outside";
                document.body.appendChild(before);
                document.body.appendChild(outside);
                const out = [dialog.returnValue, dialog.open];
                detachedNonModal.show();
                out.push(detachedNonModal.open);
                detachedNonModal.close();
                try { dialog.showModal(); } catch (error) { out.push(error.name); }
                document.body.appendChild(dialog);
                before.focus();
                dialog.show();
                dialog.show();
                out.push(dialog.open, document.activeElement === dialog);
                try { dialog.showModal(); } catch (error) { out.push(error.name); }
                outside.focus();
                dialog.close();
                dialog.close();
                out.push(dialog.open, document.activeElement.id);
                dialog.returnValue = "preserved";
                dialog.showModal();
                dialog.showModal();
                dialog.close();
                out.push(dialog.returnValue);
                return out.join(":");
            })()"#,
        );
        assert_eq!(
            result,
            ":false:true:InvalidStateError:true:true:InvalidStateError:false:outside:preserved"
        );
    }

    // --- iframe focus chain (issue #254) ---

    /// Builds a three-level document chain: the top document, an iframe `f`
    /// holding `x` and a nested iframe `g`, and `g`'s document holding `y`.
    ///
    /// The returned prelude leaves `a`, `f`, `sub`, `x`, `g`, `sub2` and `y` on
    /// `globalThis` for the test body to use.
    const FOCUS_CHAIN_SETUP: &str = r#"
        globalThis.a = document.getElementById("a");
        globalThis.b = document.getElementById("b");
        globalThis.f = document.getElementById("f");
        globalThis.sub = f.contentDocument;
        globalThis.x = sub.createElement("input");
        sub.body.appendChild(x);
        globalThis.g = sub.createElement("iframe");
        sub.body.appendChild(g);
        globalThis.sub2 = g.contentDocument;
        globalThis.y = sub2.createElement("input");
        sub2.body.appendChild(y);
        globalThis.label = node => {
          if (node === a) return "a";
          if (node === b) return "b";
          if (node === f) return "f";
          if (node === g) return "g";
          if (node === x) return "x";
          if (node === y) return "y";
          if (node === document) return "doc";
          if (node === sub) return "sub";
          if (node === sub2) return "sub2";
          if (node === globalThis) return "win";
          if (node === f.contentWindow) return "subwin";
          if (node === g.contentWindow) return "sub2win";
          if (node === document.body) return "body";
          if (node === sub.body) return "subbody";
          if (node === sub2.body) return "sub2body";
          return String(node && node.nodeName);
        };
        globalThis.state = () => [
          label(document.activeElement),
          label(sub.activeElement),
          label(sub2.activeElement),
          [document.hasFocus(), sub.hasFocus(), sub2.hasFocus()].join("/"),
        ].join(" ");
        globalThis.focusLog = [];
        globalThis.watchFocus = targets => {
          for (const [node, name] of targets) {
            for (const type of ["focus", "blur", "focusin", "focusout"]) {
              node.addEventListener(type, event => focusLog.push(
                name + ":" + event.type + ":" + label(event.target) + ":" +
                (event.relatedTarget === null ? "null" : label(event.relatedTarget))
              ), true);
            }
          }
        };
    "#;

    fn focus_chain_runtime() -> JsRuntime {
        let mut runtime = runtime_from_html(
            r#"<html><body><input id="a"><input id="b"><iframe id="f"></iframe></body></html>"#,
        );
        runtime.eval(FOCUS_CHAIN_SETUP).unwrap();
        runtime
    }

    #[test]
    fn focusing_inside_an_iframe_points_each_ancestor_at_its_frame() {
        let mut runtime = focus_chain_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const out = [state()];
                a.focus();
                out.push(state());
                // The sub-document holds the element; the parent holds the frame.
                x.focus();
                out.push(state());
                // Three levels deep, every ancestor points at the next frame.
                y.focus();
                out.push(state());
                // Leaving the chain clears the sub-documents again.
                a.focus();
                out.push(state());
                return out.join("|");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                "body subbody sub2body true/false/false",
                "|a subbody sub2body true/false/false",
                "|f x sub2body true/true/false",
                "|f g y true/true/true",
                "|a subbody sub2body true/false/false",
            )
        );
    }

    /// The event sequence for a focus move that crosses documents, compared
    /// entry for entry against Firefox 152 over Marionette. Listeners are
    /// registered in the capture phase on every window, document and element
    /// involved, so the log shows the full propagation path.
    #[test]
    fn crossing_documents_blurs_the_old_browsing_context_and_focuses_the_new_one() {
        let mut runtime = focus_chain_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                watchFocus([
                  [globalThis, "win"], [document, "doc"], [a, "a"],
                  [f.contentWindow, "subwin"], [sub, "sub"], [x, "x"],
                ]);
                a.focus();
                focusLog.length = 0;
                x.focus();
                return focusLog.join("|");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                // The old element loses focus first, with no relatedTarget: the
                // new element lives in another document.
                "win:blur:a:null|doc:blur:a:null|a:blur:a:null",
                "|win:focusout:a:null|doc:focusout:a:null|a:focusout:a:null",
                // Then the browsing context it belonged to.
                "|win:blur:doc:null|doc:blur:doc:null",
                "|win:blur:win:null",
                // Then the one being entered, outermost target first.
                "|subwin:focus:sub:null|sub:focus:sub:null",
                "|subwin:focus:subwin:null",
                // And finally the new element.
                "|subwin:focus:x:null|sub:focus:x:null|x:focus:x:null",
                "|subwin:focusin:x:null|sub:focusin:x:null|x:focusin:x:null",
            )
        );
    }

    #[test]
    fn returning_from_an_iframe_blurs_only_the_innermost_document() {
        let mut runtime = focus_chain_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                watchFocus([
                  [globalThis, "win"], [document, "doc"], [a, "a"],
                  [f.contentWindow, "subwin"], [sub, "sub"], [x, "x"],
                  [g.contentWindow, "sub2win"], [sub2, "sub2"], [y, "y"],
                ]);
                a.focus();
                x.focus();
                focusLog.length = 0;
                // Going one level deeper blurs the document being left even
                // though it stays in the chain, and leaves the top alone.
                y.focus();
                const deeper = focusLog.join("|");
                focusLog.length = 0;
                // Coming back out only blurs the innermost document.
                a.focus();
                return [deeper, focusLog.join("|")].join(";;");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                "subwin:blur:x:null|sub:blur:x:null|x:blur:x:null",
                "|subwin:focusout:x:null|sub:focusout:x:null|x:focusout:x:null",
                "|subwin:blur:sub:null|sub:blur:sub:null",
                "|subwin:blur:subwin:null",
                "|sub2win:focus:sub2:null|sub2:focus:sub2:null",
                "|sub2win:focus:sub2win:null",
                "|sub2win:focus:y:null|sub2:focus:y:null|y:focus:y:null",
                "|sub2win:focusin:y:null|sub2:focusin:y:null|y:focusin:y:null",
                ";;",
                "sub2win:blur:y:null|sub2:blur:y:null|y:blur:y:null",
                "|sub2win:focusout:y:null|sub2:focusout:y:null|y:focusout:y:null",
                "|sub2win:blur:sub2:null|sub2:blur:sub2:null",
                "|sub2win:blur:sub2win:null",
                "|win:focus:doc:null|doc:focus:doc:null",
                "|win:focus:win:null",
                "|win:focus:a:null|doc:focus:a:null|a:focus:a:null",
                "|win:focusin:a:null|doc:focusin:a:null|a:focusin:a:null",
            )
        );
    }

    #[test]
    fn same_document_focus_moves_do_not_touch_the_browsing_context() {
        let mut runtime = focus_chain_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                watchFocus([[globalThis, "win"], [document, "doc"], [a, "a"], [b, "b"]]);
                a.focus();
                focusLog.length = 0;
                b.focus();
                // No document- or window-targeted event, and relatedTarget is
                // exposed because both elements share a document.
                return focusLog.join("|");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                "win:blur:a:b|doc:blur:a:b|a:blur:a:b",
                "|win:focusout:a:b|doc:focusout:a:b|a:focusout:a:b",
                "|win:focus:b:a|doc:focus:b:a|b:focus:b:a",
                "|win:focusin:b:a|doc:focusin:b:a|b:focusin:b:a",
            )
        );
    }

    #[test]
    fn blurring_inside_an_iframe_keeps_the_frame_focused() {
        let mut runtime = focus_chain_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                x.focus();
                const before = state();
                // The unfocusing steps hand focus to the sub-document's viewport,
                // so the frame stays the parent's active element and the
                // sub-document keeps system focus.
                x.blur();
                return [before, state()].join("|");
            })()"#,
        );
        assert_eq!(
            result,
            "f x sub2body true/true/false|f subbody sub2body true/true/false"
        );
    }

    #[test]
    fn removing_a_focused_iframe_hands_focus_back_to_the_top_document() {
        let mut runtime = focus_chain_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                y.focus();
                const before = state();
                f.remove();
                // The frame and its documents are gone, so the top-level
                // document is focused again with nothing focused inside it.
                return [
                  before,
                  label(document.activeElement),
                  document.hasFocus(),
                ].join("|");
            })()"#,
        );
        assert_eq!(result, "f g y true/true/true|body|true");
    }

    /// Every handler must see the state its own event announces. Snapshots taken
    /// inside each handler of a cross-document move, compared against Firefox
    /// 152: focus has left the old context by the time its `blur` pair runs, and
    /// the new chain is already installed when its `focus` pair runs, while the
    /// element itself is only focused for the element's own event.
    #[test]
    fn browsing_context_focus_events_observe_the_state_they_announce() {
        let mut runtime = focus_chain_runtime();
        let result = eval_str(
            &mut runtime,
            r#"(() => {
                const snapshots = [];
                const snap = tag => snapshots.push([
                  tag,
                  document.hasFocus(),
                  sub.hasFocus(),
                  label(document.activeElement),
                  label(sub.activeElement),
                ].join(":"));
                a.focus();
                a.addEventListener("blur", () => snap("element-blur"), true);
                document.addEventListener("blur", event => {
                  if (event.target === document) snap("doc-blur");
                }, true);
                globalThis.addEventListener("blur", event => {
                  if (event.target === globalThis) snap("win-blur");
                }, true);
                sub.addEventListener("focus", event => {
                  if (event.target === sub) snap("sub-focus");
                }, true);
                f.contentWindow.addEventListener("focus", event => {
                  if (event.target === f.contentWindow) snap("subwin-focus");
                }, true);
                x.addEventListener("focus", () => snap("element-focus"), true);

                x.focus();
                snap("after");
                return snapshots.join("|");
            })()"#,
        );
        assert_eq!(
            result,
            concat!(
                // The old context still holds focus while its element blurs.
                "element-blur:true:false:body:subbody",
                // In transit: no document reports focus.
                "|doc-blur:false:false:body:subbody",
                "|win-blur:false:false:body:subbody",
                // The chain is in place — the parent already points at the frame —
                // before the entered context is announced.
                "|sub-focus:true:true:f:subbody",
                "|subwin-focus:true:true:f:subbody",
                // The element is focused only for its own event.
                "|element-focus:true:true:f:x",
                "|after:true:true:f:x",
            )
        );
    }
}
