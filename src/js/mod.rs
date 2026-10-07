//! JavaScript engine embedding and DOM/Web API bindings.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context as TaskContext, Poll};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use boa_engine::JsString;
use boa_engine::Module;
use boa_engine::builtins::promise::{OperationType, PromiseState};
use boa_engine::builtins::typed_array::TypedArrayKind;
use boa_engine::context::HostHooks;
use boa_engine::job::AsyncContext;
use boa_engine::module::{ModuleLoader, Referrer};
use boa_engine::native_function::{NativeCallContinuation, NativeCallSuspension, NativeFunction};
use boa_engine::object::{
    FunctionObjectBuilder, JsObject,
    builtins::{
        AlignedVec, JsArray, JsArrayBuffer, JsDataView, JsPromise, JsTypedArray, JsUint8Array,
    },
};
use boa_engine::realm::Realm;
use boa_engine::value::TryIntoJs;
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, Script, Source, js_string};
use boa_gc::{Finalize, RootProvider, Trace, Tracer};

use crate::accessibility::{AccessibilityRenderState, AccessibilitySnapshotState};
use crate::css::{
    AffineTransform, ComputedStyle, ComputedValue, Origin, PseudoElement, Selector, StyleResolver,
    parse_scope_prelude, parse_selector_list,
};
use crate::css::{SelectorMatchCache, matches_selector_boundary_cached, matches_selector_cached};
use crate::dom::{
    Node, NodeHandle, NodeType, ShadowRootMode, WeakNodeHandle, is_actually_disabled,
};
use crate::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent,
};
use crate::http::cors::{
    CredentialsMode, Origin as CorsOrigin, PreflightCache, RedirectMode, RequestMode, ResponseType,
    exposed_response_headers,
};
use crate::http::{Client, HttpRequest, Method, default_user_agent};
use crate::layout::{InlineFragmentContent, LayoutBox, Rect, edge_sizes};

mod broadcast_channel;
mod cache_storage;
mod child_document;
#[cfg(test)]
mod child_document_tests;
mod cssom_normalization;
use child_document::{FetchedChildResource, LoadedChildDocument};
mod compression_stream;
#[cfg(test)]
mod compression_stream_tests;
#[cfg(test)]
mod computed_pseudo_tests;
#[cfg(test)]
mod computed_style_layout_tests;
#[cfg(test)]
mod computed_text_shadow_tests;
mod document_write;
mod errors;
use errors::JsHostError;
pub use errors::{FindInPageError, JsEvaluationError, NotificationPermissionError};
#[cfg(test)]
mod document_write_tests;
mod font_descriptors;
mod font_loading;
#[cfg(test)]
mod font_loading_tests;
mod form_state;
mod form_submission;
mod geolocation;
pub use geolocation::GeolocationPositionData;
mod form_validation;
#[cfg(test)]
mod fullscreen_tests;
mod iframe_navigation;
mod input_bridge;
mod nested_rendering;
pub(crate) mod render_demand;
pub use form_state::{FormStateRestoreMode, FormStateSnapshot};
#[cfg(test)]
mod layout_metrics_tests;
mod module_fetch;
#[cfg(test)]
mod module_loading_tests;
mod node_lifetime;
mod pointer_lock;
pub use pointer_lock::PointerLockTransition;
#[cfg(test)]
mod node_lifetime_tests;
#[cfg(test)]
mod popover_tests;
#[cfg(test)]
mod query_tests;
mod scroll_snap;
mod shared_worker;
use shared_worker::terminate_shared_worker_connections;
mod text_stream;
mod web_locks;
mod worklet;
use web_locks::{
    flush_web_lock_notifications, register_web_lock_client, unregister_web_lock_client,
};
use worklet::terminate_worklet_runtime;
#[cfg(test)]
mod text_stream_tests;
#[cfg(test)]
mod window_named_tests;
use module_fetch::{ModuleFetch, ModuleFetchPool};

mod csp;
mod event_loop;
mod storage;
mod stylesheet;
use csp::{CspPolicy, CspViolation, ResourceType};
use event_loop::{EventLoop, Task};
pub use storage::{StorageManager, StoragePersistencePolicy};
pub(crate) use storage::{StorageOrigin, VisitSource};
use storage::{
    WebLockMode, WebLockNotification, WebLockNotificationKind, WebLockRequestResult,
    WebLockStartResult,
};

/// Most page-script task errors retained per drain. See
/// [`JsRuntime::record_task_error`].
const MAX_TASK_ERRORS: usize = 32;
const MAX_CSP_VIOLATIONS: usize = 1024;

fn report_safe_javascript_failure(
    destination: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
    code: &'static str,
    task_kind: Option<&'static str>,
) {
    let Some((reporter, surface)) = destination else {
        return;
    };
    let Ok(code) = ErrorCode::new(code) else {
        return;
    };
    let mut context = [
        ("operation", "execute"),
        ("resource", "script"),
        ("task_kind", ""),
    ];
    let context_len = if let Some(kind) = task_kind {
        context[2].1 = kind;
        3
    } else {
        2
    };
    reporter.report(
        RawEvent::new(
            ErrorCategory::JavaScript,
            ErrorSeverity::Error,
            code,
            surface,
            "JavaScript execution failed",
            &context[..context_len],
        )
        .sanitize(),
    );
}

fn report_safe_worker_or_module_failure(
    destination: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
    category: ErrorCategory,
    code: &'static str,
    operation: &'static str,
) {
    let Some((reporter, surface)) = destination else {
        return;
    };
    let Ok(code) = ErrorCode::new(code) else {
        return;
    };
    let (resource, message) = match category {
        ErrorCategory::Worker => ("worker", "Worker execution failed"),
        ErrorCategory::Module => ("module", "Module loading failed"),
        _ => return,
    };
    reporter.report(
        RawEvent::new(
            category,
            ErrorSeverity::Error,
            code,
            surface,
            message,
            &[("operation", operation), ("resource", resource)],
        )
        .sanitize(),
    );
}

fn report_safe_stylesheet_parse_failure(
    destination: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
) {
    let Some((reporter, surface)) = destination else {
        return;
    };
    reporter.report(
        RawEvent::new(
            ErrorCategory::Css,
            ErrorSeverity::Error,
            ErrorCode::new("CSS_STYLESHEET_PARSE_FAILED").expect("static code"),
            surface,
            "Stylesheet parse failed",
            &[("operation", "parse"), ("resource", "stylesheet")],
        )
        .sanitize(),
    );
}

fn report_active_js_task_failure(code: &'static str, kind: &'static str) {
    let _ = with_host_state(|host| {
        let destination = host
            .try_borrow()
            .ok()
            .and_then(|state| state.error_reporter.clone());
        report_safe_javascript_failure(destination, code, Some(kind));
        Ok(())
    });
}

const DOM_CONTENT_LOADED_SCRIPT: &str = concat!(
    "document.__readyState = 'interactive'; ",
    "try { if (typeof __omoikane_performance_navigation_event === 'function') ",
    "__omoikane_performance_navigation_event('domInteractive'); } catch (_) { void 0; } ",
    "try { if (typeof __omoikane_performance_navigation_event === 'function') ",
    "__omoikane_performance_navigation_event('domContentLoadedStart'); } catch (_) { void 0; } ",
    "document.dispatchEvent(new Event('DOMContentLoaded', { bubbles: true })); ",
    "try { if (typeof __omoikane_performance_navigation_event === 'function') ",
    "__omoikane_performance_navigation_event('domContentLoadedEnd'); } catch (_) { void 0; }",
);
const LOAD_SCRIPT: &str = concat!(
    "document.__readyState = 'complete'; ",
    "try { if (typeof __omoikane_performance_navigation_event === 'function') ",
    "__omoikane_performance_navigation_event('domComplete'); } catch (_) { void 0; } ",
    "try { if (typeof __omoikane_performance_navigation_event === 'function') ",
    "__omoikane_performance_navigation_event('loadStart'); } catch (_) { void 0; } ",
    "{ const event = new Event('load', { bubbles: false }); ",
    "window.dispatchEvent(event); } ",
    "try { if (typeof __omoikane_performance_navigation_event === 'function') ",
    "__omoikane_performance_navigation_event('loadEnd'); } catch (_) { void 0; }",
);

thread_local! {
    #[cfg(test)]
    static TEST_LAYOUT_METRICS_CALLS: Cell<usize> = const { Cell::new(0) };
    #[cfg(test)]
    static TEST_STYLE_NORMALIZATION_CALLS: Cell<usize> = const { Cell::new(0) };
    #[cfg(test)]
    static TEST_FETCH_RESPONSE_OVERRIDE: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    static ACTIVE_HOST_STATE: RefCell<Option<Rc<RefCell<HostState>>>> = const { RefCell::new(None) };
    static ACTIVE_MODULE_DOCUMENT: Cell<Option<ActiveModuleDocument>> = const { Cell::new(None) };
}

/// Host clipboard storage shared by all page runtimes in this process.
/// `HostState` and Boa values remain thread-affine, but the text snapshot is
/// intentionally synchronized so runtimes hosted on different threads still
/// observe the same clipboard.
static HOST_CLIPBOARD: OnceLock<HostClipboard> = OnceLock::new();

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

#[derive(Clone, Copy)]
struct ActiveModuleDocument {
    host_state: *const RefCell<HostState>,
    document_id: usize,
}

struct ActiveModuleDocumentGuard(Option<ActiveModuleDocument>);

impl Drop for ActiveModuleDocumentGuard {
    fn drop(&mut self) {
        ACTIVE_MODULE_DOCUMENT.with(|slot| slot.set(self.0.take()));
    }
}

fn activate_module_document(
    host_state: &Rc<RefCell<HostState>>,
    document_id: usize,
) -> ActiveModuleDocumentGuard {
    let active = ActiveModuleDocument {
        host_state: Rc::as_ptr(host_state),
        document_id,
    };
    let previous = ACTIVE_MODULE_DOCUMENT.with(|slot| slot.replace(Some(active)));
    ActiveModuleDocumentGuard(previous)
}

fn active_document_id() -> Option<usize> {
    ACTIVE_HOST_STATE.with(|slot| {
        let active_host = slot.borrow();
        let host_state = active_host.as_ref()?;
        let host_state_identity = Rc::as_ptr(host_state);
        ACTIVE_MODULE_DOCUMENT
            .with(|slot| slot.get())
            .filter(|active| active.host_state == host_state_identity)
            .map(|active| active.document_id)
            .or_else(|| Some(host_state.borrow().document.identity()))
    })
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

/// Error text used for the cooperative wall-clock interrupt. Keep this
/// private and stable so page-task plumbing can distinguish a timeout from a
/// normal JavaScript exception without exposing a second public error type.
const WALL_CLOCK_TIMEOUT_MESSAGE: &str = boa_engine::vm::WALL_CLOCK_TIMEOUT_MESSAGE;

fn execution_deadline(timeout: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(timeout).unwrap_or(now)
}

fn wall_clock_timeout_error() -> JsError {
    JsNativeError::runtime_limit()
        .with_message(WALL_CLOCK_TIMEOUT_MESSAGE)
        .into()
}

fn is_wall_clock_timeout(error: &JsError) -> bool {
    error.as_native().is_some_and(|error| {
        error.is_runtime_limit() && error.message() == WALL_CLOCK_TIMEOUT_MESSAGE
    })
}

/// Wraps a Boa async evaluation with a cooperative wall-clock deadline.
///
/// Boa's async evaluator yields at deterministic instruction budgets. Checking
/// the deadline at each poll lets the host interrupt a long-running script
/// without killing a thread or leaving a second evaluator racing the runtime.
/// Dropping the inner future is intentional: `ActiveHostFuture` clears any
/// pending dialog metadata and Boa's async frame guard restores the VM stack.
struct TimedJsFuture<F> {
    future: Option<Pin<Box<F>>>,
    deadline: Instant,
    deadline_wake_scheduled: bool,
}

impl<F> TimedJsFuture<F> {
    fn new(future: F, deadline: Instant) -> Self {
        Self {
            future: Some(Box::pin(future)),
            deadline,
            deadline_wake_scheduled: false,
        }
    }
}

impl<F, T> Future for TimedJsFuture<F>
where
    F: Future<Output = JsResult<T>>,
{
    type Output = JsResult<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        if Instant::now() >= self.deadline {
            self.future.take();
            return Poll::Ready(Err(wall_clock_timeout_error()));
        }

        let result = self
            .future
            .as_mut()
            .expect("timed future polled after completion")
            .as_mut()
            .poll(cx);
        if result.is_pending() && !self.deadline_wake_scheduled {
            let delay = self.deadline.saturating_duration_since(Instant::now());
            let waker = cx.waker().clone();
            self.deadline_wake_scheduled = true;
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                waker.wake();
            });
        }

        // A single VM budget can take longer than the remaining deadline. Do
        // the second check after polling so the timeout remains a real
        // wall-clock bound instead of waiting for the next wake-up.
        if Instant::now() >= self.deadline {
            self.future.take();
            return Poll::Ready(Err(wall_clock_timeout_error()));
        }

        if result.is_ready() {
            self.future.take();
        }
        result
    }
}

/// Rebinds the script-owning Document for each poll of an asynchronous Boa
/// evaluation. Keeping the guard poll-scoped avoids leaking iframe ownership
/// while a host call has suspended the future.
struct ActiveDocumentFuture<F> {
    future: Pin<Box<F>>,
    host_state: Rc<RefCell<HostState>>,
    document_id: usize,
}

impl<F: Future> Future for ActiveDocumentFuture<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let _guard = activate_module_document(&self.host_state, self.document_id);
        self.future.as_mut().poll(cx)
    }
}

const DOM_BOOTSTRAP: &str = concat!(
    include_str!("dom_bootstrap.js"),
    "\n",
    include_str!("xpath.js"),
    "\n",
    include_str!("font_loading.js"),
    "\n",
    include_str!("find_in_page.js")
);

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

#[derive(Debug, Clone, Trace, Finalize, boa_engine::JsData)]
struct ModuleDocumentId(usize);

// A Realm may outlive its Document after navigation when page code retains a
// function or DOM wrapper from that Realm. Keep its immutable origin with the
// Realm so access checks still work after the old Document's native records
// have been collected.
#[derive(Debug, Clone, Trace, Finalize, boa_engine::JsData)]
struct ModuleDocumentOrigin(#[unsafe_ignore_trace] DocumentSecurityOrigin);

fn realm_origin_snapshot(
    host_state: &Rc<RefCell<HostState>>,
    document_id: usize,
) -> JsResult<ModuleDocumentOrigin> {
    host_state
        .borrow()
        .document_security_origins
        .get(&document_id)
        .cloned()
        .map(ModuleDocumentOrigin)
        .ok_or_else(|| {
            JsNativeError::error()
                .with_message("Document security origin is unavailable")
                .into()
        })
}

/// Host capabilities are handed only to the trusted bootstrap module. Page
/// scripts never receive these functions as properties of their global object.
struct BootstrapBindings {
    object: JsObject,
    names: Vec<String>,
}

impl BootstrapBindings {
    fn new() -> Self {
        Self {
            object: JsObject::with_null_proto(),
            names: Vec::new(),
        }
    }

    fn value(
        &mut self,
        name: JsString,
        value: impl Into<JsValue>,
        context: &mut Context,
    ) -> JsResult<()> {
        let text = name.to_std_string_escaped();
        if !self.names.contains(&text) {
            self.names.push(text);
        }
        self.object.set(name, value.into(), true, context)?;
        Ok(())
    }

    fn callable(
        &mut self,
        name: JsString,
        length: usize,
        function: NativeFunction,
        constructor: bool,
        context: &mut Context,
    ) -> JsResult<()> {
        let callable = FunctionObjectBuilder::new(context.realm(), function)
            .name(name.clone())
            .length(length)
            .constructor(constructor)
            .build();
        self.value(name, callable, context)
    }

    fn module_source(&self) -> String {
        let names: HashSet<&str> = self.names.iter().map(String::as_str).collect();
        let mut source = format!(
            "const {{ {} }} = import.meta.__omoikane_private_bindings;\n\
             delete import.meta.__omoikane_private_bindings;\n",
            self.names.join(", ")
        );
        // Legacy bootstrap files refer to some bindings as globalThis.name.
        // Resolve those references to the module-private lexical binding.
        // Explicit old delete statements become no-ops: no capability was
        // installed on the page global in the first place.
        let prefix = "globalThis.__omoikane_";
        let mut body = String::new();
        let mut cursor = 0;
        while let Some(offset) = DOM_BOOTSTRAP[cursor..].find(prefix) {
            let start = cursor + offset;
            body.push_str(&DOM_BOOTSTRAP[cursor..start]);
            let mut end = start + prefix.len();
            while DOM_BOOTSTRAP
                .as_bytes()
                .get(end)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'$')
            {
                end += 1;
            }
            let name = &DOM_BOOTSTRAP[start + "globalThis.".len()..end];
            if names.contains(name) {
                if body.ends_with("delete ") {
                    body.truncate(body.len() - "delete ".len());
                    body.push_str("void 0");
                } else {
                    body.push_str(name);
                }
            } else {
                body.push_str(&DOM_BOOTSTRAP[start..end]);
            }
            cursor = end;
        }
        body.push_str(&DOM_BOOTSTRAP[cursor..]);
        source.push_str(&body);
        source
    }
}

fn evaluate_dom_bootstrap(
    context: &mut Context,
    host_state: &Rc<RefCell<HostState>>,
    bindings: &BootstrapBindings,
) -> JsResult<()> {
    let document_id = context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .expect("document realm has an identity")
        .0;
    let loader = host_state
        .borrow()
        .module_loader
        .as_ref()
        .and_then(Weak::upgrade)
        .expect("browser module loader is live during bootstrap");
    let source = bindings.module_source();
    let module = Module::parse(Source::from_bytes(source.as_bytes()), None, context)?;
    loader
        .bootstrap_modules
        .borrow_mut()
        .insert(module.clone(), document_id);
    let complete = |promise: JsPromise| match promise.state() {
        PromiseState::Fulfilled(_) => Ok(()),
        PromiseState::Rejected(error) => Err(JsError::from_opaque(error)),
        PromiseState::Pending => Err(JsNativeError::error()
            .with_message("DOM bootstrap did not complete synchronously")
            .into()),
    };
    let result: JsResult<()> = (|| {
        complete(module.load(context))?;
        module.link(context)?;
        complete(module.evaluate(context))?;
        // Keep the invariant checked at every realm creation, including future
        // bindings added by a subsystem that might accidentally use a global.
        for name in &bindings.names {
            if context
                .global_object()
                .has_own_property(js_string!(name.as_str()), context)?
            {
                return Err(JsNativeError::error()
                    .with_message("a private host binding was exposed on the page global")
                    .into());
            }
        }
        Ok(())
    })();
    loader.bootstrap_modules.borrow_mut().remove(&module);
    host_state
        .borrow_mut()
        .bootstrap_bindings
        .remove(&document_id);
    result
}

#[derive(Debug, Default)]
struct HttpModuleLoader {
    /// Module records are scoped to the owning Document.  Boa can reuse a
    /// loader while iframe Documents execute in the same JS runtime, and a
    /// URL alone is not enough to identify the CSP context for an import.
    modules: RefCell<HashMap<(usize, String), Module>>,
    /// Exact module records allowed to receive bootstrap-only host bindings.
    bootstrap_modules: RefCell<HashMap<Module, usize>>,
    fetch_pool: RefCell<Option<ModuleFetchPool>>,
    pending: RefCell<HashMap<(usize, String), ModuleFetch>>,
    owner: Weak<RefCell<HostState>>,
    /// CSP context propagated from a module graph's root URL to every module
    /// it imports.  The same runtime can host iframe Documents with different
    /// policies, so a single global policy would be incorrect for nested
    /// module graphs.
    csp_contexts: RefCell<HashMap<(usize, String), CspPolicy>>,
    csp_violations: RefCell<Vec<(usize, String, String)>>,
    csp_violation_keys: RefCell<HashSet<(usize, String)>>,
}

impl ModuleLoader for HttpModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        specifier: JsString,
        context: &AsyncContext<'_>,
    ) -> impl Future<Output = JsResult<Module>> {
        async move {
            let realm = match &referrer {
                Referrer::Module(module) => module.realm(),
                Referrer::Script(script) => script.realm().clone(),
                Referrer::Realm(realm) => realm.clone(),
            };
            // A fetch can yield while jobs from another iframe run. Keep the
            // owning Document on its Realm instead of consulting thread-local
            // execution state after a suspension.
            let module_document_id = realm
                .host_defined()
                .get::<ModuleDocumentId>()
                .map(|document| document.0)
                .or_else(active_document_id)
                .unwrap_or(0);
            self.ensure_document_is_live(module_document_id)
                .inspect_err(|_| self.report_import_load_failure())?;
            // Classic-script/eval imports can have no registered module root.
            // Their Realm still identifies the Document whose base and CSP
            // govern the request; absence of a graph entry must not bypass CSP.
            let document_context = self.owner.upgrade().map(|owner| {
                let state = owner.borrow();
                (
                    state.base_url_for_document(module_document_id),
                    state.document_csp.get(&module_document_id).cloned(),
                )
            });
            let specifier = specifier.to_std_string_escaped();
            let referrer_url = referrer
                .path()
                .and_then(|path| path.to_str())
                .and_then(|path| path.parse::<crate::http::Url>().ok())
                .or_else(|| document_context.as_ref().and_then(|(base, _)| base.clone()));
            let resolved = if specifier.starts_with("http://") || specifier.starts_with("https://")
            {
                specifier
                    .parse::<crate::http::Url>()
                    .inspect_err(|_| self.report_import_load_failure())
                    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
            } else {
                let base = referrer_url.as_ref().ok_or_else(|| {
                    self.report_import_load_failure();
                    JsNativeError::typ()
                        .with_message(format!("cannot resolve module specifier: {specifier}"))
                })?;
                crate::http::url::resolve_url(base, &specifier)
                    .inspect_err(|_| self.report_import_load_failure())
                    .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?
            };
            let resolved_string = resolved.to_string();

            let csp_context = referrer
                .path()
                .and_then(|path| path.to_str())
                .and_then(|path| {
                    let document_id = module_document_id;
                    // Inline roots include a fragment identifying their script.
                    // HTTP URL normalization strips it; policy lookup must retain
                    // the exact module path registered for that root.
                    self.csp_contexts
                        .borrow()
                        .get(&(document_id, path.to_owned()))
                        .cloned()
                        .map(|policy| (document_id, policy))
                })
                .or_else(|| {
                    document_context
                        .as_ref()
                        .and_then(|(_, policy)| policy.clone())
                        .map(|policy| (module_document_id, policy))
                });

            if let Some((document_id, policy)) = csp_context.as_ref()
                && !policy.allows_url(ResourceType::Script, &resolved)
            {
                self.record_csp_violation(*document_id, resolved_string.clone());
                self.report_import_load_failure();
                return Err(JsNativeError::error()
                    .with_message(format!("CSP blocked module import: {resolved_string}"))
                    .into());
            }

            if let Some(module) = self
                .modules
                .borrow()
                .get(&(module_document_id, resolved_string.clone()))
            {
                if let Some((document_id, policy)) = csp_context.as_ref() {
                    self.csp_contexts
                        .borrow_mut()
                        .insert((*document_id, resolved_string.clone()), policy.clone());
                }
                if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
                    eprintln!("[omoikane][module] cache-hit {resolved_string}");
                }
                return Ok(module.clone());
            }

            let public_only = requires_public_fetch(&resolved, referrer_url.as_ref());
            let key = (module_document_id, resolved_string.clone());
            let wait_start = Instant::now();
            let fetch = {
                let mut pending = self.pending.borrow_mut();
                if let Some(fetch) = pending.get(&key) {
                    fetch.clone()
                } else {
                    let mut pool = self.fetch_pool.borrow_mut();
                    if pool.is_none() {
                        let cookies = self
                            .owner
                            .upgrade()
                            .ok_or_else(|| {
                                self.report_import_load_failure();
                                JsNativeError::error().with_message("module owner was discarded")
                            })?
                            .borrow()
                            .cookie_store
                            .clone();
                        *pool = Some(ModuleFetchPool::new(cookies).map_err(|error| {
                            self.report_import_load_failure();
                            JsNativeError::typ().with_message(error.to_string())
                        })?);
                    }
                    let fetch = pool.as_ref().unwrap().fetch(
                        resolved_string.clone(),
                        public_only,
                        self.owner.upgrade().and_then(|owner| {
                            owner
                                .borrow()
                                .location_href
                                .parse::<crate::http::Url>()
                                .ok()
                        }),
                    );
                    pending.insert(key.clone(), fetch.clone());
                    fetch
                }
            };
            let result = fetch.clone().await;
            // A retry may already have replaced a failed request while another
            // waiter was suspended. Only remove this particular download.
            let mut pending = self.pending.borrow_mut();
            if pending
                .get(&key)
                .is_some_and(|current| current.same_request(&fetch))
            {
                pending.remove(&key);
            }
            drop(pending);
            self.ensure_document_is_live(module_document_id)
                .inspect_err(|_| self.report_import_load_failure())?;
            let fetched = result
                .inspect_err(|_| self.report_import_load_failure())
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
            let response = &fetched.response;
            if response.status_code() != 200 {
                self.report_import_load_failure();
                return Err(JsNativeError::typ()
                    .with_message(format!(
                        "module request returned HTTP {}",
                        response.status_code()
                    ))
                    .into());
            }
            if let Some(effective_url) = response.effective_url()
                && let Some((document_id, policy)) = csp_context.as_ref()
                && !policy.allows_url_after_redirects(
                    ResourceType::Script,
                    effective_url,
                    response.redirect_count(),
                )
            {
                let blocked_uri = effective_url.to_string();
                self.record_csp_violation(*document_id, blocked_uri.clone());
                self.report_import_load_failure();
                return Err(JsNativeError::error()
                    .with_message(format!("CSP blocked module redirect: {blocked_uri}"))
                    .into());
            }
            // Other waiters may already have parsed this shared response.
            if let Some(module) = self.modules.borrow().get(&key) {
                return Ok(module.clone());
            }
            let fetch_elapsed = fetched.elapsed;
            let wait_elapsed = wait_start.elapsed();
            let source_bytes = response.body().len();
            let source = String::from_utf8_lossy(response.body());
            let parse_start = std::time::Instant::now();
            let module = Module::parse(
                Source::from_reader(source.as_bytes(), Some(Path::new(&resolved_string))),
                Some(realm),
                &mut context.borrow_mut(),
            )
            .inspect_err(|_| self.report_import_load_failure())?;
            let parse_elapsed = parse_start.elapsed();
            if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
                eprintln!(
                    "[omoikane][module] loaded {resolved_string} bytes={source_bytes} fetch_ms={:.3} parse_ms={:.3} wait_ms={:.3}",
                    fetch_elapsed.as_secs_f64() * 1_000.0,
                    parse_elapsed.as_secs_f64() * 1_000.0,
                    wait_elapsed.as_secs_f64() * 1_000.0,
                );
            }
            if let Some((document_id, policy)) = csp_context {
                self.csp_contexts
                    .borrow_mut()
                    .insert((document_id, resolved_string.clone()), policy);
            }
            self.modules
                .borrow_mut()
                .insert((module_document_id, resolved_string), module.clone());
            Ok(module)
        }
    }

    fn init_import_meta(
        self: Rc<Self>,
        import_meta: &JsObject,
        module: &Module,
        context: &mut Context,
    ) {
        if let Some(document_id) = self.bootstrap_modules.borrow_mut().remove(module) {
            if let Some(owner) = self.owner.upgrade() {
                let bindings = owner.borrow().bootstrap_bindings.get(&document_id).cloned();
                if let Some(bindings) = bindings {
                    let _ = import_meta.set(
                        js_string!("__omoikane_private_bindings"),
                        bindings,
                        true,
                        context,
                    );
                }
            }
        }
        if let Some(url) = module.path().and_then(|path| path.to_str()) {
            let _ = import_meta.set(js_string!("url"), js_string!(url), false, context);
        }
    }
}

impl HttpModuleLoader {
    fn report_import_load_failure(&self) {
        let destination = self.owner.upgrade().and_then(|owner| {
            owner
                .try_borrow()
                .ok()
                .and_then(|state| state.error_reporter.clone())
        });
        report_safe_worker_or_module_failure(
            destination,
            ErrorCategory::Module,
            "MODULE_IMPORT_LOAD_FAILED",
            "fetch",
        );
    }

    fn ensure_document_is_live(&self, document_id: usize) -> JsResult<()> {
        let live = self.owner.upgrade().is_some_and(|owner| {
            let state = owner.borrow();
            state.document.identity() == document_id
                || state
                    .iframe_documents
                    .values()
                    .any(|entry| entry.document.identity() == document_id)
        });
        if live {
            Ok(())
        } else {
            Err(JsNativeError::reference()
                .with_message("module document is no longer live")
                .into())
        }
    }

    fn set_csp_policy_for_module_graph(
        &self,
        document_id: usize,
        root_url: &str,
        policy: CspPolicy,
    ) {
        self.csp_contexts
            .borrow_mut()
            .insert((document_id, root_url.to_string()), policy);
    }

    fn clear_csp_context_for_document(&self, document_id: usize) {
        self.pending.borrow_mut().retain(|(owner_id, _), fetch| {
            if *owner_id == document_id {
                fetch.cancel();
                false
            } else {
                true
            }
        });
        self.csp_contexts
            .borrow_mut()
            .retain(|(context_document_id, _), _| *context_document_id != document_id);
        self.modules
            .borrow_mut()
            .retain(|(module_document_id, _), _| *module_document_id != document_id);
        self.csp_violations
            .borrow_mut()
            .retain(|(violation_document_id, _, _)| *violation_document_id != document_id);
        self.csp_violation_keys
            .borrow_mut()
            .retain(|(violation_document_id, _)| *violation_document_id != document_id);
    }

    fn record_csp_violation(&self, document_id: usize, blocked_uri: String) {
        let mut violations = self.csp_violations.borrow_mut();
        if violations.len() >= MAX_CSP_VIOLATIONS {
            return;
        }
        let mut keys = self.csp_violation_keys.borrow_mut();
        if !keys.insert((document_id, blocked_uri.clone())) {
            return;
        }
        violations.push((document_id, "script-src".to_string(), blocked_uri));
    }

    fn take_csp_violations(&self) -> Vec<(usize, String, String)> {
        let violations = std::mem::take(&mut *self.csp_violations.borrow_mut());
        self.csp_violation_keys.borrow_mut().clear();
        violations
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
    /// A form navigation into an existing iframe without changing its attributes.
    FormSubmission {
        node_id: usize,
        request: form_submission::Submission,
        visit_source: Option<VisitSource>,
    },
    /// A geolocation request whose timeout has elapsed.
    GeolocationTimeout { request_id: u64 },
    /// A script task captured while an iframe child Realm was active.
    ///
    /// Boa Promise jobs already carry their execution Realm internally, but
    /// host-owned tasks do not. Keeping the Realm and Document identity with
    /// the task prevents a child callback from running against the top global,
    /// and lets navigation discard callbacks belonging to a stale Document.
    Realm {
        payload: Box<TimerPayload>,
        realm: Realm,
        document_id: usize,
    },
}

/// A top-level navigation requested by script in the current browsing context.
///
/// The JavaScript runtime only queues the request. The owning browser session
/// decides when to fetch and install the next Document, keeping networking and
/// browsing-history ownership outside the ECMAScript embedding layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationRequest {
    Navigate {
        url: String,
        replace: bool,
    },
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
    Traverse {
        delta: i32,
    },
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
                write!(
                    f,
                    "stale JavaScript dialog id {actual}; expected {expected}"
                )
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

/// Source unit executed by an owned page task in FIFO order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTaskSource {
    Classic {
        source: String,
        label: String,
        script_node_id: Option<usize>,
    },
    Module {
        source: String,
        url: String,
        script_node_id: Option<usize>,
    },
}

/// Terminal failure of an owned page task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTaskError {
    Cancelled,
    TimedOut,
}

/// Result returned when an owned page task gives its runtime back to the host.
pub struct CompletedPageTask {
    pub runtime: JsRuntime,
    pub generation: u64,
    pub result: Result<Vec<String>, PageTaskError>,
}

type OwnedPageTaskFuture = Pin<Box<dyn Future<Output = CompletedPageTask>>>;

/// Pollable page-script task that owns its runtime while JavaScript is suspended.
///
/// Keeping the runtime inside the future avoids a self-reference between
/// `JsRuntime` and Boa's `&mut Context` evaluation future. Cancellation is
/// cooperative: set the flag and poll once to drop the active evaluation and
/// recover the unchanged runtime.
pub struct OwnedPageTask {
    future: OwnedPageTaskFuture,
    controller: JavaScriptDialogController,
    cancelled: Rc<Cell<bool>>,
    generation: u64,
}

impl OwnedPageTask {
    pub fn dialog_controller(&self) -> JavaScriptDialogController {
        self.controller.clone()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn cancel(&self) {
        self.cancelled.set(true);
    }
}

impl Future for OwnedPageTask {
    type Output = CompletedPageTask;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        self.future.as_mut().poll(cx)
    }
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

struct AdjustedLayoutCache {
    layout_generation: u64,
    scroll_generation: u64,
    style_generation: u64,
    paint_generation: u64,
    root: Rc<LayoutBox>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmoothScrollTarget {
    Document(usize),
    Element(usize),
}

#[derive(Debug, Clone, Copy)]
struct SmoothScrollAnimation {
    target: SmoothScrollTarget,
    start: (f32, f32),
    end: (f32, f32),
    started_ms: f64,
}

const SMOOTH_SCROLL_DURATION_MS: f64 = 300.0;

/// Monotonic generations shared by the top-level document's style, layout, and
/// paint cache layers.
///
/// A style generation advances when computed values are invalidated by DOM
/// changes or transition sampling; a layout generation advances when the
/// layout tree is rebuilt; and a paint generation advances when scroll-adjusted
/// geometry is invalidated. Consumers can compare a snapshot with a later one
/// without retaining engine internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderGenerations {
    pub style: u64,
    pub layout: u64,
    pub paint: u64,
}

/// A native window-state change requested by the page's Fullscreen API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullscreenTransition {
    /// Enter a native fullscreen presentation.
    Enter,
    /// Leave the native fullscreen presentation.
    Exit,
}

impl TimerPayload {
    fn kind(&self) -> &'static str {
        match self {
            Self::Source(_) => "source",
            Self::Callback { .. } => "callback",
            Self::ResourceLoad { .. } => "resource-load",
            Self::FormSubmission { .. } => "form-submission",
            Self::GeolocationTimeout { .. } => "geolocation-timeout",
            Self::Realm { payload, .. } => payload.kind(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VisualViewportState {
    width: f32,
    height: f32,
    offset_left: f32,
    offset_top: f32,
    scale: f32,
}

impl VisualViewportState {
    fn json(self, window_scroll: (f32, f32)) -> String {
        format!(
            "{{\"width\":{},\"height\":{},\"offsetLeft\":{},\"offsetTop\":{},\"pageLeft\":{},\"pageTop\":{},\"scale\":{}}}",
            json_number(self.width),
            json_number(self.height),
            json_number(self.offset_left),
            json_number(self.offset_top),
            json_number(window_scroll.0 + self.offset_left),
            json_number(window_scroll.1 + self.offset_top),
            json_number(self.scale),
        )
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
    /// Composed ancestor paths carrying user-action selector state.
    hover_path: Vec<usize>,
    active_path: Vec<usize>,
    focus_path: Vec<usize>,
    focus_subjects: Vec<usize>,
    focus_visible_id: Option<usize>,
    node_lifetimes: node_lifetime::NodeLifetimes,
    pointer_lock: pointer_lock::State,
    input_bridge: input_bridge::State,
    form_state: form_state::State,
    iframe_navigation: iframe_navigation::State,
    form_validation: form_validation::State,
    /// Bootstrap-private resolver that accepts only canonical DOM wrappers and
    /// returns their native node identity.
    canonical_node_identity_resolver: Option<JsValue>,
    /// Keeps newly constructed native capabilities rooted until the trusted
    /// bootstrap module receives them through its private import.meta hook.
    bootstrap_bindings: HashMap<usize, JsObject>,
    /// CDP remote object handles are retained by the host rather than by a
    /// page-visible global property.  Keeping these values in HostState also
    /// lets the runtime root provider trace them across Boa collections.
    remote_objects: HashMap<String, JsValue>,
    console_logs: Vec<String>,
    /// Errors raised by page script while an event-loop task ran.
    ///
    /// A task's failure belongs to the page, not to the embedder's request, so it
    /// is collected here and the loop continues. Draining is the embedder's job
    /// (see [`JsRuntime::take_task_errors`]); leaving them unreported is how the
    /// navigation-aborting bug in issue #303 stayed invisible.
    task_errors: Vec<String>,
    /// Optional shared sink for already sanitized browser error events.
    error_reporter: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
    /// How many task errors were dropped once `task_errors` hit its cap.
    suppressed_task_errors: usize,
    location_href: String,
    navigator_user_agent: String,
    clipboard: HostClipboard,
    clipboard_permission_granted: bool,
    notification_permission: String,
    geolocation: geolocation::State,
    compression_streams: compression_stream::Store,
    http_client: Client,
    cookie_store: Arc<Mutex<crate::http::CookieJar>>,
    websocket_clients: HashMap<u64, WebSocketConnection>,
    next_websocket_id: u64,
    /// Successful CORS preflight results for this environment settings object.
    cors_preflight_cache: PreflightCache,
    /// Viewport used when resolving computed styles and running layout for the
    /// `getComputedStyle` / layout-metrics bindings (issues 016-8 and 044-2).
    viewport: Rect,
    /// Top-level visual viewport supplied by the presentation host. It follows
    /// the layout viewport until the host reports a zoomed or occluded view.
    visual_viewport: VisualViewportState,
    visual_viewport_follows_layout: bool,
    /// Top-level Window scroll offset in document CSS pixels.
    window_scroll: (f32, f32),
    /// Nested browsing contexts keep an independent layout viewport scroll.
    /// Child layout is not painted by the top-level renderer yet, but its
    /// Window and VisualViewport geometry must not borrow the parent's offset.
    iframe_window_scrolls: HashMap<usize, (f32, f32)>,
    /// Last dimensions observed for live child browsing contexts. A change is
    /// converted into Window and VisualViewport resize steps at the next
    /// rendering opportunity.
    observed_iframe_viewports: HashMap<usize, (f32, f32)>,
    pending_window_resize_documents: Vec<usize>,
    pending_visual_viewport_resize_documents: Vec<usize>,
    pending_visual_viewport_scroll_documents: Vec<usize>,
    /// Scroll targets waiting for the next rendering opportunity. This is an
    /// ordered set: first-queue order is retained and duplicate ids are skipped.
    pending_scroll_targets: Vec<usize>,
    smooth_scrolls: Vec<SmoothScrollAnimation>,
    /// Effective element offsets captured before invalidating layout. Reflow
    /// compares these with the rebuilt scrolling extents to detect clamps.
    scroll_offsets_before_layout: HashMap<usize, (f32, f32)>,
    /// Per-document cached style resolvers, keyed by the identity of each
    /// document's root [`Document`] node (the top-level document and every
    /// `<iframe>` sub-browsing-context document). Each entry is rebuilt on
    /// demand from that document's author stylesheets when it is marked
    /// dirty, so `getComputedStyle` on a node resolves against the cascade of
    /// the document that node actually lives in — the main document's rules
    /// never leak into a sub-document and vice versa (issue 016-15).
    document_styles: HashMap<usize, DocumentStyleEntry>,
    /// Registrations created by `CSS.registerProperty()`, isolated by the
    /// currently executing Document and retained across stylesheet rebuilds.
    registered_custom_properties:
        HashMap<usize, BTreeMap<String, crate::css::style::RegisteredCustomProperty>>,
    font_loading: font_loading::FontStore,
    /// Cached layout tree for the **main** document, matching its entry in
    /// [`HostState::document_styles`]. Rebuilt lazily and only when a layout
    /// metric (not just a computed style) is requested. Sub-documents do not
    /// participate in layout (layout metrics for sub-document nodes report
    /// zero); only computed styles are document-scoped here.
    layout_root: Option<LayoutBox>,
    /// Results and persistent inputs for `content-visibility` layout.
    content_visibility_auto_nodes: HashSet<usize>,
    content_visibility_skipped_nodes: HashSet<usize>,
    content_visibility_remembered_sizes: HashMap<usize, (f32, f32)>,
    content_visibility_forced_nodes: HashSet<usize>,
    content_visibility_forced_in_layout: HashSet<usize>,
    content_visibility_focus_nodes: HashSet<usize>,
    content_visibility_selection_nodes: HashSet<usize>,
    style_generation: u64,
    layout_generation: u64,
    paint_generation: u64,
    scroll_generation: u64,
    /// Snap geometry is rebuilt at most once per scroll container and layout
    /// generation. User scrolls only scan that container's candidate areas.
    scroll_snap_cache: HashMap<usize, Option<scroll_snap::Geometry>>,
    /// The area selected on each axis, retained across layout changes.
    scroll_snap_selection: HashMap<usize, scroll_snap::Selection>,
    /// CSSOM geometry also observes child document roots and their lifetime.
    /// Child document changes also invalidate the composited paint output,
    /// while retaining the top-level document's style and layout generations.
    layout_metrics_generation: u64,
    adjusted_layout_cache: Option<AdjustedLayoutCache>,
    #[cfg(test)]
    adjusted_layout_builds: u64,
    #[cfg(test)]
    style_resolver_generation: u64,
    #[cfg(test)]
    document_script_executions: HashMap<usize, u64>,
    /// Currently executing parser script. A document's first write creates a
    /// streaming parser immediately after this node; subsequent writes reuse
    /// that parser's open-element stack and input state.
    write_insertion_ref: Option<NodeHandle>,
    /// Streaming input and tree-builder state, isolated by owning Document.
    write_parsers: HashMap<usize, Rc<RefCell<document_write::WriteState>>>,
    /// Global to this runtime so cross-document recursion cannot evade the cap.
    document_write_depth: usize,
    written_script_queue: VecDeque<document_write::WrittenScript>,
    /// Parser-inserted scripts must not run again as dynamic resource tasks.
    parser_inserted_scripts: HashSet<usize>,
    /// Scripts created by script APIs and therefore eligible for preparation
    /// when their node first becomes connected to an active document.
    runnable_inserted_scripts: HashSet<usize>,
    /// Runnable inserted scripts that have already reached the preparation
    /// step. Node identities keep this stable across wrapper recreation and
    /// same-document moves.
    started_inserted_scripts: HashSet<usize>,
    /// Explicit Realm root used when a child writes into the top Document.
    main_realm: Option<Realm>,
    /// Base URL of the top-level document, used to resolve relative resource
    /// references such as `<iframe src="empty.html">`. Populated when the
    /// document's scripts run (see [`JsRuntime::execute_document_scripts`]) or
    /// explicitly via [`JsRuntime::set_base_url`]. `None` means relative
    /// references cannot be resolved.
    base_url: Option<crate::http::Url>,
    /// Worker globals inherit the creator's secure-context state even when a
    /// data/blob script URL itself is not a trustworthy URL.
    secure_context_override: Option<bool>,
    /// Sub-browsing-context documents — one per `<iframe>` element whose
    /// `contentDocument` has been accessed. Keyed by the iframe element's node
    /// identity. Each entry records the loaded sub-document root and the `src`
    /// value it was loaded from, so a subsequent `src` change triggers a
    /// reload while an unchanged `src` returns the same document instance.
    iframe_documents: HashMap<usize, IframeDocument>,
    /// Auxiliary browsing contexts are independent of DOM iframe elements.
    auxiliary_contexts: HashMap<u64, AuxiliaryContext>,
    next_auxiliary_context_id: u64,
    /// Monotonic generation assigned to each nested Window created by iframe
    /// navigation or reconnect.
    next_iframe_generation: u64,
    /// Stable browsing-context identity per connected iframe. Cross-document
    /// navigation preserves it; detach destroys it and reconnect allocates a
    /// new identity so an old WindowProxy stays permanently closed.
    iframe_context_ids: HashMap<usize, u64>,
    next_iframe_context_id: u64,
    /// Live navigable names; zero identifies the top-level context.
    browsing_context_names: HashMap<usize, String>,
    /// Node-wrapper ids removed during nested browsing-context teardown. The
    /// bootstrap drains these before wrapping a replacement document so a
    /// pointer identity reused by Rust cannot resurrect a stale JS wrapper.
    discarded_node_ids: Vec<usize>,
    /// Resource elements that already have a queued load task. This prevents a
    /// move within one connected document from producing duplicate events.
    pending_resource_loads: HashSet<usize>,
    /// Source captured before an iframe navigation's asynchronous load.
    pending_iframe_visits: HashMap<usize, VisitSource>,
    /// Preserves a child Window's initiator while its callback changes `src`.
    active_child_navigation_frame: Option<usize>,
    navigation_requests: VecDeque<(NavigationRequest, Option<VisitSource>)>,
    /// Per-document fullscreen stacks, with the current element last.
    fullscreen_elements: HashMap<usize, Vec<usize>>,
    /// Whether this embedder exposes a fullscreen-capable presentation host.
    fullscreen_supported: bool,
    /// Deterministic host response used by embedders and rejection tests.
    fullscreen_transition_allowed: bool,
    fullscreen_host_active: bool,
    pending_fullscreen_transition: Option<FullscreenTransition>,
    pending_javascript_dialog: Option<PendingJavaScriptDialog>,
    next_javascript_dialog_id: u64,
    storage_manager: StorageManager,
    storage_session_id: u64,
    /// Web Locks identifies each environment settings object separately even
    /// when several same-origin Window realms share this runtime.
    web_lock_clients: HashMap<usize, u64>,
    document_origins: HashMap<usize, Option<StorageOrigin>>,
    /// Visibility of this top-level traversable, shared by its iframe Documents.
    page_hidden: bool,
    live_css_animations: bool,
    image_timeline: crate::paint::animation::ImageTimeline,
    visible_image_playbacks: Vec<Arc<crate::paint::animation::ImagePlayback>>,
    /// Committed URL per live Document. Nested Window/Document access must not
    /// accidentally expose the top-level Location after iframe navigation.
    document_urls: HashMap<usize, String>,
    /// The fragment-selected element for each live Document. Weak references
    /// avoid retaining nodes removed from the document tree.
    document_targets: HashMap<usize, WeakNodeHandle>,
    /// Effective HTTP(S) base URL per live Document. Missing entries are
    /// intentional for documents such as `data:` and must fail closed instead
    /// of falling back to the top-level base.
    document_base_urls: HashMap<usize, crate::http::Url>,
    document_security_origins: HashMap<usize, DocumentSecurityOrigin>,
    /// Origin metadata for retained, retired Documents. It is released with
    /// their JS wrapper leases, so old same-origin wrappers remain usable.
    retired_document_security_origins: HashMap<usize, DocumentSecurityOrigin>,
    next_opaque_origin_id: u64,
    /// Enforced CSP policies keyed by the root Document node identity.  A
    /// fresh runtime starts with an empty (allow-all) policy and navigation
    /// replaces the entry before any page script executes.
    document_csp: HashMap<usize, CspPolicy>,
    /// Iframe sandbox flags captured per active nested Document.
    document_sandbox: HashMap<usize, IframeSandboxPolicy>,
    csp_violations: Vec<CspViolation>,
    csp_violation_keys: HashSet<(usize, String, String)>,
    module_loader: Option<Weak<HttpModuleLoader>>,
    /// Dedicated workers owned by this global. The map lives on the page
    /// global; worker globals instead hold `worker_owner` and `worker_id`.
    workers: HashMap<u64, Rc<RefCell<WorkerRuntime>>>,
    /// Owner-side worker objects are kept separately from `workers` so the
    /// host root tracer never has to borrow a `WorkerRuntime` through its
    /// `RefCell` while a worker operation is in flight.
    worker_owner_objects: HashMap<u64, JsValue>,
    next_worker_id: u64,
    worker_owner: Option<Rc<RefCell<HostState>>>,
    worker_id: Option<u64>,
    worker_terminated: bool,
    worker_owner_bound: bool,
    worker_owner_object: Option<JsValue>,
    /// Realm of the page-side Worker object. Dedicated worker runtimes use
    /// this when queueing messages back to an iframe owner.
    worker_owner_realm: Option<Realm>,
    worker_startup_outgoing: VecDeque<String>,
    shared_worker: shared_worker::State,
    broadcast_channel: broadcast_channel::State,
    worklet: worklet::State,
    /// Constructable stylesheets adopted by a Document or ShadowRoot. The
    /// JavaScript wrapper keeps stylesheet objects; this native snapshot lets
    /// the synchronous style resolver include their parsed text without
    /// manufacturing DOM `<style>` nodes.
    adopted_stylesheets: HashMap<usize, Vec<String>>,
}

impl Finalize for HostState {}

// Values retained by the host event loop are not visible from Boa's VM root
// provider. Keep them alive for as long as this runtime's host state owns them.
unsafe impl Trace for HostState {
    unsafe fn trace(&self, tracer: &mut Tracer) {
        unsafe { self.event_loop.trace(tracer) };
        unsafe { self.node_lifetimes.trace(tracer) };
        unsafe { self.pointer_lock.trace(tracer) };
        unsafe { self.input_bridge.trace(tracer) };
        unsafe { self.form_state.trace(tracer) };
        unsafe { self.iframe_navigation.trace(tracer) };
        unsafe { self.form_validation.trace(tracer) };
        if let Some(maps) = &self.font_loading.maps {
            unsafe { maps.trace(tracer) };
        }
        // Iframe and auxiliary Document realms, plus rendering callbacks
        // retained by the event loop, are Boa `Realm` handles backed by
        // `Rooted<RealmInner>`. Retaining those handles keeps child intrinsics
        // alive without a separate Trace traversal (Realm exposes no `trace`).
        if let Some(resolver) = &self.canonical_node_identity_resolver {
            unsafe { resolver.trace(tracer) };
        }
        for bindings in self.bootstrap_bindings.values() {
            unsafe { bindings.trace(tracer) };
        }
        for value in self.remote_objects.values() {
            unsafe { value.trace(tracer) };
        }
        if let Some(owner) = &self.worker_owner_object {
            unsafe { owner.trace(tracer) };
        }
        for owner in self.worker_owner_objects.values() {
            unsafe { owner.trace(tracer) };
        }
        unsafe { self.geolocation.trace(tracer) };
        if let Some(dialog) = &self.pending_javascript_dialog {
            unsafe { dialog.suspension.trace(tracer) };
        }
        unsafe { self.broadcast_channel.trace(tracer) };
        unsafe { self.shared_worker.trace(tracer) };
    }

    fn run_finalizer(&self) {}
}

#[derive(Clone, Debug, Default)]
struct HostClipboard(Arc<Mutex<String>>);

impl HostClipboard {
    fn read_text(&self) -> String {
        match self.0.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn write_text(&self, text: String) {
        match self.0.lock() {
            Ok(mut guard) => *guard = text,
            Err(poisoned) => *poisoned.into_inner() = text,
        }
    }
}

fn host_clipboard() -> HostClipboard {
    HOST_CLIPBOARD.get_or_init(HostClipboard::default).clone()
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

/// State for one same-origin classic Dedicated Worker. A worker owns a fully
/// independent `JsRuntime`; only message queues and lifecycle metadata bridge
/// it back to the page global.
struct WorkerRuntime {
    runtime: JsRuntime,
    owner_state: Rc<RefCell<HostState>>,
    owner_object: Option<JsValue>,
    outgoing: VecDeque<String>,
    startup_error: Option<String>,
    terminated: bool,
}

fn is_nested_frame_tag(tag: &str) -> bool {
    tag.eq_ignore_ascii_case("iframe") || tag.eq_ignore_ascii_case("frame")
}

/// A loaded sub-browsing-context document owned by an `<iframe>` or `<frame>` element.
#[derive(Debug)]
struct IframeDocument {
    /// Root document node of the sub-browsing context.
    document: NodeHandle,
    /// The `src` attribute value this document was loaded from (`""` for an
    /// `about:blank` sub-document with no `src`).
    loaded_src: String,
    /// Effective URL used to initialize the child browsing-context global.
    document_url: String,
    /// Child browsing-context Realm. Same-origin WindowProxy access or script
    /// execution creates it lazily; navigation/detach drops it with the document.
    realm: Option<Realm>,
    /// The resource attribute (`src`, `srcdoc`, or `data`) which created this
    /// active document.
    loaded_attribute: &'static str,
    /// The resource attribute value used by the committed navigation.
    loaded_resource: String,
    /// Request retained by the active document for history replay.
    submission: Option<form_submission::Submission>,
    /// Parser-created initial scripts, consumed once by the load task.
    initial_scripts: Vec<usize>,
    /// Monotonic identity of the active Window/Document generation. Unlike a
    /// pointer-derived node identity this cannot be reused after teardown.
    generation: u64,
}

/// A popup's active Document and Realm. The numeric context id survives
/// navigation and is never reused, so retained WindowProxies can detect close.
#[derive(Debug)]
struct AuxiliaryContext {
    name: String,
    opener_document_id: usize,
    document: NodeHandle,
    document_url: String,
    realm: Option<Realm>,
}

/// Security origin used for same-origin WindowProxy access checks. Opaque
/// origins carry an identity so an inherited `about:blank`/`srcdoc` document
/// can remain same-origin with an opaque creator while an unrelated `data:` or
/// sandboxed document cannot accidentally compare equal.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DocumentSecurityOrigin {
    Tuple(StorageOrigin),
    Opaque(u64),
}

/// Sandbox policy captured when an iframe navigation creates its Document.
/// Attribute changes affect the next navigation rather than mutating the
/// security boundary of an already-active document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IframeSandboxPolicy {
    active: bool,
    allow_scripts: bool,
    allow_same_origin: bool,
    allow_pointer_lock: bool,
    allow_forms: bool,
    allow_top_navigation: bool,
}

impl Default for IframeSandboxPolicy {
    fn default() -> Self {
        Self {
            active: false,
            allow_scripts: true,
            allow_same_origin: true,
            allow_pointer_lock: true,
            allow_forms: true,
            allow_top_navigation: true,
        }
    }
}

impl IframeSandboxPolicy {
    fn from_iframe(iframe: &NodeHandle) -> Self {
        let Some(value) = iframe.get_attribute("sandbox") else {
            return Self::default();
        };
        let tokens: HashSet<String> = value
            .split_ascii_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        Self {
            active: true,
            allow_scripts: tokens.contains("allow-scripts"),
            allow_same_origin: tokens.contains("allow-same-origin"),
            allow_pointer_lock: tokens.contains("allow-pointer-lock"),
            allow_forms: tokens.contains("allow-forms"),
            allow_top_navigation: tokens.contains("allow-top-navigation"),
        }
    }

    fn exposes_document_to_parent(self) -> bool {
        !self.active || self.allow_same_origin
    }
}

/// A cached [`StyleResolver`] for one document (the top-level document or an
/// iframe sub-document), plus a dirty flag driving lazy rebuilds.
///
/// The resolver is seeded from that document's own author stylesheets;
/// it is rebuilt on the next computed-style query whenever [`dirty`] is set (or
/// the resolver has never been built), so a DOM mutation in one document does
/// not force every other document's resolver to rebuild.
///
/// [`dirty`]: DocumentStyleEntry::dirty
#[derive(Default)]
struct DocumentStyleEntry {
    /// Cached resolver seeded with this document's author stylesheets, or
    /// `None` until first built.
    resolver: Option<StyleResolver>,
    /// Viewport used when constructing this document's cascade.
    viewport_size: Option<(f32, f32)>,
    resources: stylesheet::StylesheetLoader,
    web_fonts: Arc<crate::font::WebFontRegistry>,
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

fn take_monotonic_id(next: &mut u64, label: &'static str) -> Result<u64, JsHostError> {
    let id = *next;
    let following = id
        .checked_add(1)
        .ok_or(JsHostError::IdSpaceExhausted(label))?;
    *next = following;
    Ok(id)
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
                viewport_size: None,
                resolver: None,
                resources: Default::default(),
                web_fonts: Default::default(),
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
        let main_storage_origin = StorageOrigin::from_url(&location_href);
        document_origins.insert(document.identity(), main_storage_origin.clone());
        let mut web_lock_clients = HashMap::new();
        web_lock_clients.insert(
            document.identity(),
            storage_manager.create_web_lock_client(),
        );
        let mut document_urls = HashMap::new();
        document_urls.insert(document.identity(), location_href.clone());
        let base_url = location_href.parse::<crate::http::Url>().ok();
        let mut document_base_urls = HashMap::new();
        if let Some(base_url) = base_url.clone() {
            document_base_urls.insert(document.identity(), base_url);
        }
        let main_security_origin = main_storage_origin
            .map(DocumentSecurityOrigin::Tuple)
            .unwrap_or(DocumentSecurityOrigin::Opaque(1));
        let mut document_security_origins = HashMap::new();
        document_security_origins.insert(document.identity(), main_security_origin);
        let cookie_store = Arc::new(Mutex::new(crate::http::CookieJar::new()));
        let mut http_client = Client::new();
        http_client.set_shared_cookie_store(Arc::clone(&cookie_store));
        let mut state = Self {
            runtime_identity: Rc::new(()),
            performance_start,
            performance_time_origin,
            event_loop: EventLoop::default(),
            document: document.clone(),
            nodes: HashMap::new(),
            hover_path: Vec::new(),
            active_path: Vec::new(),
            focus_path: Vec::new(),
            focus_subjects: Vec::new(),
            focus_visible_id: None,
            node_lifetimes: node_lifetime::NodeLifetimes::default(),
            pointer_lock: pointer_lock::State::default(),
            input_bridge: input_bridge::State::default(),
            form_state: form_state::State::default(),
            iframe_navigation: iframe_navigation::State::default(),
            form_validation: form_validation::State::default(),
            canonical_node_identity_resolver: None,
            bootstrap_bindings: HashMap::new(),
            remote_objects: HashMap::new(),
            console_logs: Vec::new(),
            task_errors: Vec::new(),
            error_reporter: None,
            suppressed_task_errors: 0,
            base_url,
            secure_context_override: None,
            location_href,
            navigator_user_agent: default_user_agent(),
            clipboard: host_clipboard(),
            clipboard_permission_granted: true,
            notification_permission: "default".to_string(),
            geolocation: geolocation::State::default(),
            compression_streams: compression_stream::Store::new(),
            http_client,
            cookie_store,
            websocket_clients: HashMap::new(),
            next_websocket_id: 1,
            cors_preflight_cache: PreflightCache::default(),
            viewport: Rect {
                x: 0.0,
                y: 0.0,
                width: DEFAULT_VIEWPORT_WIDTH,
                height: DEFAULT_VIEWPORT_HEIGHT,
            },
            visual_viewport: VisualViewportState {
                width: DEFAULT_VIEWPORT_WIDTH,
                height: DEFAULT_VIEWPORT_HEIGHT,
                offset_left: 0.0,
                offset_top: 0.0,
                scale: 1.0,
            },
            visual_viewport_follows_layout: true,
            window_scroll: (0.0, 0.0),
            iframe_window_scrolls: HashMap::new(),
            observed_iframe_viewports: HashMap::new(),
            pending_window_resize_documents: Vec::new(),
            pending_visual_viewport_resize_documents: Vec::new(),
            pending_visual_viewport_scroll_documents: Vec::new(),
            pending_scroll_targets: Vec::new(),
            smooth_scrolls: Vec::new(),
            scroll_offsets_before_layout: HashMap::new(),
            document_styles,
            registered_custom_properties: HashMap::new(),
            font_loading: Default::default(),
            layout_root: None,
            content_visibility_auto_nodes: HashSet::new(),
            content_visibility_skipped_nodes: HashSet::new(),
            content_visibility_remembered_sizes: HashMap::new(),
            content_visibility_forced_nodes: HashSet::new(),
            content_visibility_forced_in_layout: HashSet::new(),
            content_visibility_focus_nodes: HashSet::new(),
            content_visibility_selection_nodes: HashSet::new(),
            style_generation: 0,
            layout_generation: 0,
            paint_generation: 0,
            scroll_generation: 0,
            scroll_snap_cache: HashMap::new(),
            scroll_snap_selection: HashMap::new(),
            layout_metrics_generation: 0,
            adjusted_layout_cache: None,
            #[cfg(test)]
            adjusted_layout_builds: 0,
            #[cfg(test)]
            style_resolver_generation: 0,
            #[cfg(test)]
            document_script_executions: HashMap::new(),
            write_insertion_ref: None,
            write_parsers: HashMap::new(),
            document_write_depth: 0,
            written_script_queue: VecDeque::new(),
            parser_inserted_scripts: HashSet::new(),
            runnable_inserted_scripts: HashSet::new(),
            started_inserted_scripts: HashSet::new(),
            main_realm: None,
            iframe_documents: HashMap::new(),
            auxiliary_contexts: HashMap::new(),
            next_auxiliary_context_id: 1,
            next_iframe_generation: 1,
            iframe_context_ids: HashMap::new(),
            next_iframe_context_id: 1,
            browsing_context_names: HashMap::new(),
            discarded_node_ids: Vec::new(),
            pending_resource_loads: HashSet::new(),
            pending_iframe_visits: HashMap::new(),
            active_child_navigation_frame: None,
            navigation_requests: VecDeque::new(),
            fullscreen_elements: HashMap::new(),
            fullscreen_supported: true,
            fullscreen_transition_allowed: true,
            fullscreen_host_active: false,
            pending_fullscreen_transition: None,
            pending_javascript_dialog: None,
            next_javascript_dialog_id: 1,
            storage_manager,
            storage_session_id,
            web_lock_clients,
            document_origins,
            page_hidden: false,
            live_css_animations: false,
            image_timeline: Arc::new(Mutex::new(HashMap::new())),
            visible_image_playbacks: Vec::new(),
            document_urls,
            document_targets: HashMap::new(),
            document_base_urls,
            document_security_origins,
            retired_document_security_origins: HashMap::new(),
            next_opaque_origin_id: 2,
            document_csp: HashMap::from([(document.identity(), CspPolicy::default())]),
            document_sandbox: HashMap::new(),
            csp_violations: Vec::new(),
            csp_violation_keys: HashSet::new(),
            module_loader: None,
            workers: HashMap::new(),
            worker_owner_objects: HashMap::new(),
            next_worker_id: 1,
            worker_owner: None,
            worker_id: None,
            worker_terminated: false,
            worker_owner_bound: false,
            worker_owner_object: None,
            worker_owner_realm: None,
            worker_startup_outgoing: VecDeque::new(),
            shared_worker: shared_worker::State::default(),
            broadcast_channel: broadcast_channel::State::default(),
            worklet: worklet::State::default(),
            adopted_stylesheets: HashMap::new(),
        };
        state.register_tree(&document);
        let initial_url = state.location_href.clone();
        state.update_document_target(&document, &initial_url);
        state
    }

    fn update_document_target(&mut self, document: &NodeHandle, url: &str) {
        let document_id = document.identity();
        let next = find_fragment_target(document, url);
        let previous = self
            .document_targets
            .get(&document_id)
            .and_then(WeakNodeHandle::upgrade);
        if previous == next {
            return;
        }
        if let Some(previous) = previous {
            previous.set_user_action_state("target", false);
        }
        if let Some(next) = next {
            next.set_user_action_state("target", true);
            self.document_targets.insert(document_id, next.downgrade());
        } else {
            self.document_targets.remove(&document_id);
        }
        self.invalidate_document_style_cache(document);
    }

    fn visit_source_for_document(&self, document_id: usize) -> Option<VisitSource> {
        if !self.document_is_active(document_id) {
            return None;
        }
        let DocumentSecurityOrigin::Tuple(origin) =
            self.document_security_origins.get(&document_id)?
        else {
            return None;
        };
        let mut top_document_id = document_id;
        for _ in 0..=self.iframe_documents.len() {
            let Some(frame_id) = self.iframe_documents.iter().find_map(|(frame_id, entry)| {
                (entry.document.identity() == top_document_id).then_some(*frame_id)
            }) else {
                break;
            };
            let frame = self.get_node(frame_id)?;
            top_document_id = owner_document_for_node(&frame)?.identity();
        }
        let top_url = if top_document_id == self.document.identity() {
            self.location_href.as_str()
        } else {
            self.auxiliary_contexts
                .values()
                .find(|entry| entry.document.identity() == top_document_id)?
                .document_url
                .as_str()
        };
        VisitSource::new(origin.clone(), top_url)
    }

    /// Collects the current document's visited links for a private paint pass.
    /// The result is never copied into computed styles, layout, or a page API.
    fn visited_link_ids_for_paint(&self, document: &NodeHandle) -> Vec<usize> {
        // Most fresh profiles have no visits; avoid a DOM walk until the
        // private store has at least one entry.
        if self.storage_manager.visited_generation() == 0 {
            return Vec::new();
        }
        let document_id = document.identity();
        let Some(source) = self.visit_source_for_document(document_id) else {
            return Vec::new();
        };
        let Some(document_url) = self.document_urls.get(&document_id) else {
            return Vec::new();
        };
        let fallback = url::Url::parse(document_url).ok().or_else(|| {
            self.base_url_for_document(document_id)
                .and_then(|base| url::Url::parse(&base.to_string()).ok())
        });
        let Some(mut base) = fallback else {
            return Vec::new();
        };

        // A document's first light-DOM base element sets its navigation base.
        // Shadow trees do not contribute to document.querySelector("base[href]").
        let mut stack = document.child_nodes();
        stack.reverse();
        while let Some(node) = stack.pop() {
            if node.is_html_element()
                && node.tag_name().as_deref() == Some("base")
                && let Some(href) = node.get_attribute("href")
            {
                if let Ok(resolved) = base.join(&href) {
                    base = resolved;
                }
                break;
            }
            let mut children = node.child_nodes();
            children.reverse();
            stack.extend(children);
        }

        let mut visited = Vec::new();
        let mut stack = vec![document.clone()];
        while let Some(node) = stack.pop() {
            if node.is_html_element()
                && matches!(node.tag_name().as_deref(), Some("a" | "area"))
                && let Some(href) = node.get_attribute("href")
                && let Ok(destination) = base.join(&href)
                && self
                    .storage_manager
                    .has_visited_url(destination.as_str(), &source)
            {
                visited.push(node.identity());
            }
            let mut children = node.child_nodes();
            if let Some(shadow_root) = node.shadow_root() {
                children.push(shadow_root);
            }
            children.reverse();
            stack.extend(children);
        }
        visited
    }

    fn note_iframe_navigation_source(&mut self, iframe: &NodeHandle) {
        let iframe_id = iframe.identity();
        if self.active_child_navigation_frame == Some(iframe_id) {
            return;
        }
        let source = owner_document_for_node(iframe)
            .and_then(|owner| self.visit_source_for_document(owner.identity()));
        if let Some(source) = source {
            self.pending_iframe_visits.insert(iframe_id, source);
        } else {
            self.pending_iframe_visits.remove(&iframe_id);
        }
    }

    fn clear_document_target(&mut self, document_id: usize) {
        if let Some(target) = self
            .document_targets
            .remove(&document_id)
            .and_then(|target| target.upgrade())
        {
            target.set_user_action_state("target", false);
        }
    }

    fn set_main_base_url(&mut self, url: crate::http::Url) {
        self.base_url = Some(url.clone());
        self.document_base_urls
            .insert(self.document.identity(), url);
    }

    /// Queue loads for iframe and data-bearing object descendants when a
    /// detached subtree first becomes connected to a document.
    fn schedule_connected_resource_loads(&mut self, root: &NodeHandle, include_scripts: bool) {
        if !self.node_is_in_active_document(root) {
            return;
        }
        fn visit(state: &mut HostState, node: &NodeHandle, include_scripts: bool) {
            let tag = node.tag_name().unwrap_or_default();
            let style_has_import = tag.eq_ignore_ascii_case("style")
                && !crate::paint::stylesheet::extract_import_directives(&collect_text_content(
                    node,
                ))
                .is_empty();
            let is_resource = is_nested_frame_tag(&tag)
                || style_has_import
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
                if is_nested_frame_tag(&tag) {
                    state.note_iframe_navigation_source(node);
                }
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
        if !self.node_is_in_active_document(node) {
            return;
        }
        let attributes = node.attributes().unwrap_or_default();
        let is_iframe = node.tag_name().is_some_and(|tag| is_nested_frame_tag(&tag));
        let (effective_attribute, new_resource) = if is_iframe {
            match attributes
                .get("srcdoc")
                .filter(|_| node.has_tag_name("iframe"))
            {
                Some(srcdoc) => ("srcdoc", srcdoc.clone()),
                None => (
                    "src",
                    attributes
                        .get("src")
                        .map(|src| src.trim().to_string())
                        .unwrap_or_default(),
                ),
            }
        } else {
            (
                resource_attr,
                attributes
                    .get(resource_attr)
                    .map(|resource| resource.trim().to_string())
                    .unwrap_or_default(),
            )
        };
        if self
            .iframe_documents
            .get(&node.identity())
            .is_some_and(|entry| {
                entry.loaded_attribute == effective_attribute
                    && entry.loaded_resource == new_resource
            })
        {
            return;
        }
        // A later attribute navigation supersedes a pending form navigation.
        if is_iframe {
            self.note_iframe_navigation_source(node);
        }
        self.event_loop
            .cancel_resource_loads_for_nodes(&HashSet::from([node.identity()]));
        self.pending_resource_loads.remove(&node.identity());
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
    fn iframe_content_document(&mut self, iframe: &NodeHandle) -> Result<NodeHandle, JsHostError> {
        self.iframe_document_with_submission(iframe, None)
    }

    fn iframe_document_with_submission(
        &mut self,
        iframe: &NodeHandle,
        submission: Option<&form_submission::Submission>,
    ) -> Result<NodeHandle, JsHostError> {
        if !self.node_is_in_active_document(iframe) {
            return Err(JsHostError::InactiveIframeOwner);
        }
        let attributes = iframe.attributes().unwrap_or_default();
        let is_iframe = iframe
            .tag_name()
            .is_some_and(|tag| is_nested_frame_tag(&tag));
        let (resource_attribute, resource) = if is_iframe {
            match attributes
                .get("srcdoc")
                .filter(|_| iframe.has_tag_name("iframe"))
            {
                Some(srcdoc) => ("srcdoc", srcdoc.clone()),
                None => (
                    "src",
                    attributes
                        .get("src")
                        .map(|src| src.trim().to_string())
                        .unwrap_or_default(),
                ),
            }
        } else {
            (
                "data",
                attributes
                    .get("data")
                    .map(|data| data.trim().to_string())
                    .unwrap_or_default(),
            )
        };
        let iframe_id = iframe.identity();

        if submission.is_none()
            && let Some(entry) = self.iframe_documents.get(&iframe_id)
            && entry.loaded_attribute == resource_attribute
            && entry.loaded_resource == resource
        {
            return Ok(entry.document.clone());
        }

        let generation = take_monotonic_id(
            &mut self.next_iframe_generation,
            "iframe document generation",
        )?;
        let new_context_id = if self.iframe_context_ids.contains_key(&iframe_id) {
            None
        } else {
            Some(take_monotonic_id(
                &mut self.next_iframe_context_id,
                "iframe browsing context",
            )?)
        };
        let owner_document =
            owner_document_for_node(iframe).unwrap_or_else(|| self.document.clone());
        let owner_document_id = owner_document.identity();
        let resource_base = if owner_document_id == self.document.identity() {
            // `set_base_url` is the public override for resources owned by the
            // top-level document.
            self.base_url.clone()
        } else {
            // Nested documents must use their own effective base. Missing
            // metadata (notably `data:`) fails closed instead of borrowing the
            // unrelated top-level base URL.
            self.document_base_urls.get(&owner_document_id).cloned()
        };
        let navigation_url =
            submission.map_or_else(|| resource.clone(), |request| request.url.clone());
        let inherits_creator_origin = submission.is_none() && resource_attribute == "srcdoc"
            || navigation_url.is_empty()
            || matches_about_blank_url(&navigation_url);
        let LoadedChildDocument {
            document,
            csp_headers,
            url: child_url,
        } = if let Some(request) = submission {
            self.load_iframe_form_submission(request)?
        } else if resource_attribute == "srcdoc" {
            LoadedChildDocument::srcdoc(&resource)
        } else {
            self.load_iframe_document(&resource, resource_base.as_ref())
        };
        let child_base_url = if inherits_creator_origin {
            resource_base.clone()
        } else {
            child_url
                .as_deref()
                .and_then(|url| url.parse::<crate::http::Url>().ok())
        };
        let sandbox = if iframe.has_tag_name("iframe") {
            IframeSandboxPolicy::from_iframe(iframe)
        } else {
            IframeSandboxPolicy::default()
        };
        let (storage_origin, security_origin) = if sandbox.active && !sandbox.allow_same_origin {
            (None, self.new_opaque_security_origin()?)
        } else if inherits_creator_origin {
            (
                self.document_origins
                    .get(&owner_document.identity())
                    .cloned()
                    .flatten(),
                match self
                    .document_security_origins
                    .get(&owner_document.identity())
                    .cloned()
                {
                    Some(origin) => origin,
                    None => self.new_opaque_security_origin()?,
                },
            )
        } else {
            match child_url.as_deref().and_then(StorageOrigin::from_url) {
                Some(origin) => (Some(origin.clone()), DocumentSecurityOrigin::Tuple(origin)),
                None => (None, self.new_opaque_security_origin()?),
            }
        };

        // Prepare every fallible component before committing the navigation.
        // This keeps an exhausted monotonic-id space fail-closed without a
        // half-created context and keeps the previous Rc alive while the
        // replacement tree is allocated, preventing immediate pointer reuse.
        self.retire_iframe_document(iframe_id);
        if let Some(context_id) = new_context_id {
            self.iframe_context_ids.insert(iframe_id, context_id);
            self.browsing_context_names
                .insert(iframe_id, iframe.get_attribute("name").unwrap_or_default());
        }
        self.register_tree(&document);
        self.document_origins
            .insert(document.identity(), storage_origin);
        let committed_url = child_url
            .clone()
            .unwrap_or_else(|| "about:blank".to_string());
        self.document_urls
            .insert(document.identity(), committed_url.clone());
        self.update_document_target(&document, &committed_url);
        if let Some(child_base_url) = child_base_url.clone() {
            self.document_base_urls
                .insert(document.identity(), child_base_url);
        }
        self.document_security_origins
            .insert(document.identity(), security_origin);
        self.document_sandbox.insert(document.identity(), sandbox);
        let inherits_owner_csp = inherits_creator_origin;
        // `data:` documents have an opaque origin.  They must not inherit the
        // embedding document's URL as the CSP base, otherwise `'self'` in a
        // meta policy would incorrectly match the parent origin.
        let policy_base = if navigation_url
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        {
            String::new()
        } else {
            child_url
                .as_deref()
                .map(str::to_owned)
                .or_else(|| child_base_url.as_ref().map(ToString::to_string))
                .unwrap_or_default()
        };
        let policy = if inherits_owner_csp {
            self.document_csp
                .get(&owner_document.identity())
                .cloned()
                .unwrap_or_default()
                .with_document_meta(
                    &document,
                    &child_base_url
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                )
        } else {
            CspPolicy::from_headers_and_document(&csp_headers, &document, &policy_base)
        };
        self.document_csp.insert(document.identity(), policy);
        // Seed a dirty style cache entry for the freshly loaded sub-document so
        // its resolver is built from its own `<style>` rules on first query.
        self.document_styles.insert(
            document.identity(),
            DocumentStyleEntry {
                viewport_size: None,
                resolver: None,
                resources: Default::default(),
                web_fonts: Default::default(),
                dirty: true,
                needs_full_sample: true,
            },
        );
        let document_url = child_url
            .clone()
            .or_else(
                || match resolve_resource_ref(&resource, self.base_url.as_ref()) {
                    Some(ResolvedResource::Url(url)) => Some(url.to_string()),
                    Some(ResolvedResource::Data { .. }) => Some(resource.clone()),
                    None => None,
                },
            )
            .or_else(|| self.base_url.as_ref().map(ToString::to_string))
            .unwrap_or_else(|| "about:blank".to_string());
        self.iframe_documents.insert(
            iframe_id,
            IframeDocument {
                document: document.clone(),
                loaded_src: resource.clone(),
                document_url,
                realm: None,
                loaded_attribute: resource_attribute,
                loaded_resource: resource,
                submission: submission.cloned(),
                initial_scripts: collect_script_elements(&document)
                    .into_iter()
                    .filter(|script| {
                        script.is_html_element()
                            || script.namespace_uri().as_deref()
                                == Some("http://www.w3.org/1999/xhtml")
                    })
                    .map(|script| script.identity())
                    .collect(),
                generation,
            },
        );
        // A parsed child Document can itself contain connected iframes. Their
        // initial about:blank navigations must enqueue load events even when no
        // script ever reads their contentDocument.
        self.schedule_connected_resource_loads(&document, false);
        let visit_source = self.pending_iframe_visits.remove(&iframe_id);
        if let (Some(visit_source), Some(committed_url)) = (visit_source, child_url.as_deref()) {
            let requested_url = resolve_url_reference(&navigation_url, resource_base.as_ref());
            self.storage_manager.record_page_navigation(
                &requested_url,
                committed_url,
                visit_source,
            );
        }
        Ok(document)
    }

    fn new_opaque_security_origin(&mut self) -> Result<DocumentSecurityOrigin, JsHostError> {
        take_monotonic_id(&mut self.next_opaque_origin_id, "opaque origin")
            .map(DocumentSecurityOrigin::Opaque)
    }

    /// Creates the initial, creator-origin `about:blank` Document of a popup.
    /// No iframe node is created or registered in the opener's DOM.
    fn open_auxiliary_context(
        &mut self,
        opener_document_id: usize,
        name: &str,
    ) -> Result<u64, JsHostError> {
        if !name.is_empty()
            && name != "_blank"
            && let Some((id, _)) = self
                .auxiliary_contexts
                .iter()
                .find(|(_, entry)| entry.name == name)
        {
            return Ok(*id);
        }
        let id = take_monotonic_id(&mut self.next_auxiliary_context_id, "auxiliary context")?;
        let document = blank_html_document();
        let document_id = document.identity();
        let origin = self
            .document_origins
            .get(&opener_document_id)
            .cloned()
            .flatten();
        let security_origin = match self.document_security_origins.get(&opener_document_id) {
            Some(origin) => origin.clone(),
            None => self.new_opaque_security_origin()?,
        };
        let base = self.base_url_for_document(opener_document_id);
        let csp = self
            .document_csp
            .get(&opener_document_id)
            .cloned()
            .unwrap_or_default();
        self.register_tree(&document);
        self.document_origins.insert(document_id, origin);
        self.document_security_origins
            .insert(document_id, security_origin);
        self.document_urls
            .insert(document_id, "about:blank".to_owned());
        if let Some(base) = base {
            self.document_base_urls.insert(document_id, base);
        }
        self.document_csp.insert(document_id, csp);
        self.document_sandbox
            .insert(document_id, IframeSandboxPolicy::default());
        self.document_styles.insert(
            document_id,
            DocumentStyleEntry {
                viewport_size: None,
                resolver: None,
                resources: Default::default(),
                web_fonts: Default::default(),
                dirty: true,
                needs_full_sample: true,
            },
        );
        self.web_lock_clients
            .insert(document_id, self.storage_manager.create_web_lock_client());
        self.auxiliary_contexts.insert(
            id,
            AuxiliaryContext {
                name: name.to_owned(),
                opener_document_id,
                document,
                document_url: "about:blank".to_owned(),
                realm: None,
            },
        );
        Ok(id)
    }

    fn close_auxiliary_context(&mut self, id: u64) {
        let Some(entry) = self.auxiliary_contexts.remove(&id) else {
            return;
        };
        self.retire_document_tree(&entry.document);
    }

    /// Commits a new Document while preserving the popup's browsing-context
    /// identity. The old Realm stays rooted until the replacement is ready.
    fn navigate_auxiliary_context(
        &mut self,
        id: u64,
        requested: &str,
    ) -> Result<usize, JsHostError> {
        let (opener_document_id, current_document_id) = {
            let entry = self
                .auxiliary_contexts
                .get(&id)
                .ok_or(JsHostError::AuxiliaryContextClosed)?;
            (entry.opener_document_id, entry.document.identity())
        };
        let base = self
            .base_url_for_document(current_document_id)
            .or_else(|| self.base_url_for_document(opener_document_id));
        let LoadedChildDocument {
            document,
            csp_headers: headers,
            url: effective_url,
        } = self.load_iframe_document(requested, base.as_ref());
        let document_id = document.identity();
        let document_url = effective_url.unwrap_or_else(|| "about:blank".to_owned());
        let inherits_opener = requested.is_empty() || matches_about_blank_url(requested);
        let storage_origin = if inherits_opener {
            self.document_origins
                .get(&opener_document_id)
                .cloned()
                .flatten()
        } else {
            StorageOrigin::from_url(&document_url)
        };
        let security_origin = if inherits_opener {
            match self.document_security_origins.get(&opener_document_id) {
                Some(origin) => origin.clone(),
                None => self.new_opaque_security_origin()?,
            }
        } else {
            match &storage_origin {
                Some(origin) => DocumentSecurityOrigin::Tuple(origin.clone()),
                None => self.new_opaque_security_origin()?,
            }
        };
        let new_base = if inherits_opener {
            self.base_url_for_document(opener_document_id)
        } else {
            document_url.parse::<crate::http::Url>().ok()
        };
        let csp = if inherits_opener {
            self.document_csp
                .get(&opener_document_id)
                .cloned()
                .unwrap_or_default()
        } else {
            CspPolicy::from_headers_and_document(&headers, &document, &document_url)
        };

        self.register_tree(&document);
        self.update_document_target(&document, &document_url);
        let entry = self
            .auxiliary_contexts
            .get_mut(&id)
            .ok_or(JsHostError::AuxiliaryContextClosedDuringNavigation)?;
        let old_document = std::mem::replace(&mut entry.document, document);
        let _old_realm = entry.realm.take();
        entry.document_url = document_url.clone();
        self.retire_document_tree(&old_document);
        self.document_origins.insert(document_id, storage_origin);
        self.document_security_origins
            .insert(document_id, security_origin);
        self.document_urls.insert(document_id, document_url);
        if let Some(base) = new_base {
            self.document_base_urls.insert(document_id, base);
        }
        self.document_csp.insert(document_id, csp);
        self.document_sandbox
            .insert(document_id, IframeSandboxPolicy::default());
        self.document_styles.insert(
            document_id,
            DocumentStyleEntry {
                viewport_size: None,
                resolver: None,
                resources: Default::default(),
                web_fonts: Default::default(),
                dirty: true,
                needs_full_sample: true,
            },
        );
        self.web_lock_clients
            .insert(document_id, self.storage_manager.create_web_lock_client());
        Ok(document_id)
    }

    fn iframe_document_is_same_origin(&self, iframe: &NodeHandle, document: &NodeHandle) -> bool {
        let owner = owner_document_for_node(iframe).unwrap_or_else(|| self.document.clone());
        match (
            self.document_security_origins.get(&owner.identity()),
            self.document_security_origins.get(&document.identity()),
        ) {
            (Some(owner_origin), Some(document_origin)) => owner_origin == document_origin,
            _ => false,
        }
    }

    /// Fetches and constructs a sub-document from an iframe `src` or object
    /// `data` resource reference.
    ///
    /// Returns an `about:blank` skeleton for an empty/`about:blank` reference, a
    /// fetch failure, or an unsupported content type. HTML resources are parsed
    /// as HTML; XML MIME types, including SVG, are parsed as XML. See
    /// [`LoadedChildDocument`] for the committed URL and CSP header contract.
    fn load_iframe_document(
        &mut self,
        src: &str,
        base_url: Option<&crate::http::Url>,
    ) -> LoadedChildDocument {
        if src.is_empty() || matches_about_blank_url(src) {
            let url = if src.is_empty() { "about:blank" } else { src };
            return LoadedChildDocument::about_blank(url.to_owned());
        }
        self.fetch_child_resource(src, base_url)
            .map_or_else(LoadedChildDocument::fetch_failed, |resource| {
                resource.into_get_document()
            })
    }

    /// Resolves `src` to inline `data:` bytes or fetches it with a GET.
    /// Returns `None` when the reference cannot be resolved or the request
    /// fails.
    ///
    /// Unlike script loading (see `fetch_script_source`), a child document
    /// intentionally does NOT inspect the HTTP status code. A real browser
    /// renders even an error response's body into the frame, so the body is
    /// adopted regardless of status. The asymmetry with `fetch_script_source`
    /// (which requires 200) is deliberate.
    fn fetch_child_resource(
        &mut self,
        src: &str,
        base_url: Option<&crate::http::Url>,
    ) -> Option<FetchedChildResource> {
        let url = match resolve_resource_ref(src, base_url)? {
            ResolvedResource::Data { mime_type, data } => {
                return Some(FetchedChildResource {
                    mime_type,
                    body: data,
                    csp_headers: Vec::new(),
                    effective_url: src.to_owned(),
                });
            }
            ResolvedResource::Url(url) => url,
        };
        let mut request = crate::http::HttpRequest::get(&url).ok()?;
        if let Ok(site) = self.location_href.parse::<crate::http::Url>() {
            request.set_cookie_context(site, false);
        }
        let response = self.http_client.send(request).ok()?;
        let mut effective_url = response
            .effective_url()
            .map_or_else(|| url.to_string(), ToString::to_string);
        if let Some((_, fragment)) = src.split_once('#')
            && !effective_url.contains('#')
        {
            effective_url.push('#');
            effective_url.push_str(fragment);
        }
        Some(FetchedChildResource {
            mime_type: response.header("Content-Type").unwrap_or("").to_owned(),
            csp_headers: child_document::response_csp_headers(&response),
            body: response.body().to_vec(),
            effective_url,
        })
    }

    fn register_tree(&mut self, node: &NodeHandle) {
        self.register_tree_for_document(node, None);
    }

    fn register_tree_for_document(&mut self, node: &NodeHandle, creator: Option<usize>) {
        fn register(nodes: &mut HashMap<usize, NodeHandle>, node: &NodeHandle) {
            nodes.insert(node.identity(), node.clone());
            if let Some(content) = node.template_content() {
                register(nodes, &content);
            }
            if let Some(root) = node.shadow_root() {
                register(nodes, &root);
            }
            for child in node.child_nodes() {
                register(nodes, &child);
            }
        }
        register(&mut self.nodes, node);
        // Detached nodes still belong to the Document whose script created
        // them. Record that owner before their first JS wrapper is built, so
        // native access checks do not mistake them for foreign nodes.
        // register_tree is also called while HostState is mutably borrowed;
        // reading ACTIVE_HOST_STATE here would borrow it a second time.
        let owner = creator
            .or_else(|| {
                ACTIVE_HOST_STATE.with(|slot| {
                    let active_host = slot.borrow();
                    let active_document = ACTIVE_MODULE_DOCUMENT.with(|slot| slot.get())?;
                    active_host
                        .as_ref()
                        .is_some_and(|host| Rc::as_ptr(host) == active_document.host_state)
                        .then_some(active_document.document_id)
                })
            })
            .unwrap_or_else(|| self.document.identity());
        self.enroll_registered_tree(node, Some(owner));
    }

    fn collect_tree_ids(node: &NodeHandle, ids: &mut HashSet<usize>) {
        if !ids.insert(node.identity()) {
            return;
        }
        if let Some(content) = node.template_content() {
            Self::collect_tree_ids(&content, ids);
        }
        if let Some(root) = node.shadow_root() {
            Self::collect_tree_ids(&root, ids);
        }
        for child in node.child_nodes() {
            Self::collect_tree_ids(&child, ids);
        }
    }

    fn cancel_smooth_scrolls_in_subtree(&mut self, root: &NodeHandle) {
        let mut subtree_ids = HashSet::new();
        Self::collect_tree_ids(root, &mut subtree_ids);
        self.smooth_scrolls
            .retain(|animation| match animation.target {
                SmoothScrollTarget::Document(id) | SmoothScrollTarget::Element(id) => {
                    !subtree_ids.contains(&id)
                }
            });
    }

    /// Retires a popup Document without leaving callbacks, nested iframe
    /// contexts, or document-scoped native state alive after close/navigation.
    fn retire_document_tree(&mut self, document: &NodeHandle) {
        let document_id = document.identity();
        self.pointer_lock.retire_document(document_id);
        self.input_bridge.retire_document(document_id);
        self.form_state.retire_document(document_id);
        self.iframe_navigation.retire_document(document_id);
        self.form_validation.retire_document(document_id);
        if self.fullscreen_elements.contains_key(&document_id) {
            self.fully_exit_fullscreen(false);
        }
        self.invalidate_layout_metrics_cache();

        let mut tree_ids = HashSet::new();
        Self::collect_tree_ids(document, &mut tree_ids);
        self.smooth_scrolls
            .retain(|animation| match animation.target {
                SmoothScrollTarget::Document(id) | SmoothScrollTarget::Element(id) => {
                    !tree_ids.contains(&id)
                }
            });
        let nested_iframe_ids: Vec<_> = self
            .iframe_context_ids
            .keys()
            .copied()
            .filter(|id| tree_ids.contains(id))
            .collect();
        for iframe_id in nested_iframe_ids {
            self.destroy_iframe_context(iframe_id);
        }

        self.iframe_window_scrolls.remove(&document_id);
        self.observed_iframe_viewports.remove(&document_id);
        self.pending_window_resize_documents
            .retain(|id| *id != document_id);
        self.pending_visual_viewport_resize_documents
            .retain(|id| *id != document_id);
        self.pending_visual_viewport_scroll_documents
            .retain(|id| *id != document_id);
        tree_ids.extend(self.retired_document_node_ids(document_id));
        for id in &tree_ids {
            self.nodes.remove(id);
        }
        self.event_loop.cancel_tasks_for_document(document_id);
        self.document_styles.remove(&document_id);
        self.registered_custom_properties.remove(&document_id);
        self.font_loading.remove_document(document_id);
        self.write_parsers.remove(&document_id);
        self.written_script_queue
            .retain(|script| script.document.identity() != document_id);
        self.parser_inserted_scripts
            .retain(|id| !tree_ids.contains(id));
        if let Some(client_id) = self.web_lock_clients.remove(&document_id) {
            unregister_web_lock_client(&self.storage_manager, client_id);
        }
        self.document_origins.remove(&document_id);
        self.document_urls.remove(&document_id);
        self.clear_document_target(document_id);
        self.document_base_urls.remove(&document_id);
        if let Some(origin) = self.document_security_origins.remove(&document_id) {
            self.retired_document_security_origins
                .insert(document_id, origin);
        }
        self.document_csp.remove(&document_id);
        self.document_sandbox.remove(&document_id);
        #[cfg(test)]
        self.document_script_executions.remove(&document_id);
        self.csp_violations
            .retain(|violation| violation.document_id != document_id);
        self.csp_violation_keys
            .retain(|(id, _, _)| *id != document_id);
        if let Some(loader) = self.module_loader.as_ref().and_then(Weak::upgrade) {
            loader.clear_csp_context_for_document(document_id);
        }
        self.pending_resource_loads
            .retain(|id| !tree_ids.contains(id));
        self.event_loop.cancel_resource_loads_for_nodes(&tree_ids);
        self.discarded_node_ids.extend(tree_ids.iter().copied());
        self.unregister_tree(document);
        self.sweep_node_lifetimes();
        self.prune_document_sandbox();
    }

    /// Destroys one iframe's active document and every descendant browsing
    /// context. All document-scoped policy/cache state and queued resource
    /// tasks are removed in the same transition. DOM wrappers keep their old
    /// document data through GC leases; WindowProxy teardown is queued before
    /// a replacement document can be observed.
    fn retire_iframe_document(&mut self, iframe_id: usize) {
        let Some(previous) = self.iframe_documents.remove(&iframe_id) else {
            return;
        };
        self.pointer_lock
            .retire_document(previous.document.identity());
        self.input_bridge
            .retire_document(previous.document.identity());
        self.form_state
            .retire_document(previous.document.identity());
        self.iframe_navigation
            .retire_document(previous.document.identity());
        self.form_validation
            .retire_document(previous.document.identity());
        if self
            .fullscreen_elements
            .contains_key(&previous.document.identity())
        {
            self.fully_exit_fullscreen(false);
        }
        self.invalidate_layout_metrics_cache();

        let mut tree_ids = HashSet::new();
        Self::collect_tree_ids(&previous.document, &mut tree_ids);
        self.smooth_scrolls
            .retain(|animation| match animation.target {
                SmoothScrollTarget::Document(id) | SmoothScrollTarget::Element(id) => {
                    !tree_ids.contains(&id)
                }
            });
        let nested_iframe_ids: Vec<_> = self
            .iframe_documents
            .keys()
            .copied()
            .filter(|nested_id| tree_ids.contains(nested_id))
            .collect();
        for nested_id in nested_iframe_ids {
            self.destroy_iframe_context(nested_id);
        }

        let document_id = previous.document.identity();
        self.iframe_window_scrolls.remove(&document_id);
        self.observed_iframe_viewports.remove(&document_id);
        self.pending_window_resize_documents
            .retain(|id| *id != document_id);
        self.pending_visual_viewport_resize_documents
            .retain(|id| *id != document_id);
        self.pending_visual_viewport_scroll_documents
            .retain(|id| *id != document_id);
        tree_ids.extend(self.retired_document_node_ids(document_id));
        for id in &tree_ids {
            self.nodes.remove(id);
        }
        self.event_loop.cancel_tasks_for_document(document_id);
        self.document_styles.remove(&document_id);
        self.registered_custom_properties.remove(&document_id);
        self.font_loading.remove_document(document_id);
        self.write_parsers.remove(&document_id);
        self.written_script_queue
            .retain(|script| script.document.identity() != document_id);
        self.parser_inserted_scripts
            .retain(|id| !tree_ids.contains(id));
        if let Some(client_id) = self.web_lock_clients.remove(&document_id) {
            unregister_web_lock_client(&self.storage_manager, client_id);
        }
        self.document_origins.remove(&document_id);
        self.document_urls.remove(&document_id);
        self.clear_document_target(document_id);
        self.document_base_urls.remove(&document_id);
        if let Some(origin) = self.document_security_origins.remove(&document_id) {
            self.retired_document_security_origins
                .insert(document_id, origin);
        }
        self.document_csp.remove(&document_id);
        self.document_sandbox.remove(&document_id);
        #[cfg(test)]
        self.document_script_executions.remove(&document_id);
        self.csp_violations
            .retain(|violation| violation.document_id != document_id);
        self.csp_violation_keys
            .retain(|(violation_document_id, _, _)| *violation_document_id != document_id);
        if let Some(loader) = self.module_loader.as_ref().and_then(Weak::upgrade) {
            loader.clear_csp_context_for_document(document_id);
        }

        self.pending_resource_loads
            .retain(|node_id| !tree_ids.contains(node_id));
        self.event_loop.cancel_resource_loads_for_nodes(&tree_ids);
        self.discarded_node_ids.extend(tree_ids.iter().copied());
        self.unregister_tree(&previous.document);
        self.sweep_node_lifetimes();
        self.prune_document_sandbox();
    }

    fn destroy_iframe_context(&mut self, iframe_id: usize) {
        self.retire_iframe_document(iframe_id);
        self.pending_iframe_visits.remove(&iframe_id);
        self.iframe_context_ids.remove(&iframe_id);
        self.browsing_context_names.remove(&iframe_id);
    }

    /// Destroys every nested browsing context owned by iframe elements in a
    /// subtree that has left its owner Document.
    fn destroy_iframe_contexts_in_subtree(&mut self, root: &NodeHandle) {
        self.pointer_lock.subtree_removed(root);
        let mut subtree_ids = HashSet::new();
        Self::collect_tree_ids(root, &mut subtree_ids);
        let iframe_ids: Vec<_> = self
            .iframe_context_ids
            .keys()
            .copied()
            .filter(|iframe_id| subtree_ids.contains(iframe_id))
            .collect();
        for iframe_id in iframe_ids {
            self.destroy_iframe_context(iframe_id);
        }
        self.pending_resource_loads
            .retain(|node_id| !subtree_ids.contains(node_id));
        self.pending_iframe_visits
            .retain(|node_id, _| !subtree_ids.contains(node_id));
        self.event_loop
            .cancel_resource_loads_for_nodes(&subtree_ids);
    }

    /// Removes a retired tree from the strong registry. GC leases and the weak
    /// registry keep retained DOM references usable until their group dies.
    fn unregister_tree(&mut self, node: &NodeHandle) {
        self.nodes.remove(&node.identity());
        if self.retained_node(node.identity()).is_none() {
            self.adopted_stylesheets.remove(&node.identity());
            self.forget_unretained_node(node.identity());
        }
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

    fn prune_document_sandbox(&mut self) {
        let nodes = &self.nodes;
        self.document_sandbox
            .retain(|document_id, _| nodes.contains_key(document_id));
        #[cfg(test)]
        self.document_script_executions
            .retain(|document_id, _| nodes.contains_key(document_id));
    }

    fn get_node(&self, id: usize) -> Option<NodeHandle> {
        self.nodes
            .get(&id)
            .cloned()
            .or_else(|| self.retained_node(id))
    }

    /// Returns the URL base belonging to the Document that owns `node`.
    /// Resource references created inside an iframe must resolve against that
    /// browsing context's committed URL rather than the top-level page base.
    fn base_url_for_document(&self, document_id: usize) -> Option<crate::http::Url> {
        if document_id == self.document.identity() {
            return self.base_url.clone();
        }
        // about:blank/srcdoc keep their inherited HTTP base in this registry.
        // Their visible document URL cannot be parsed as an HTTP URL.
        self.document_base_urls.get(&document_id).cloned()
    }

    /// Network CSP belongs to the calling Window Realm, including srcdoc.
    /// JavaScript expandos cannot select another Document's policy.
    fn csp_document_for_context(&self, context: &Context) -> JsResult<NodeHandle> {
        let document_id = context
            .realm()
            .host_defined()
            .get::<ModuleDocumentId>()
            .map(|owner| owner.0)
            .unwrap_or_else(|| self.document.identity());
        if document_id != self.document.identity()
            && !self
                .iframe_documents
                .values()
                .any(|entry| entry.document.identity() == document_id)
            && !self
                .auxiliary_contexts
                .values()
                .any(|entry| entry.document.identity() == document_id)
        {
            return Err(JsNativeError::reference()
                .with_message("network document is no longer live")
                .into());
        }
        self.get_node(document_id)
            .filter(|node| node.node_type() == NodeType::Document)
            .ok_or_else(|| {
                JsNativeError::reference()
                    .with_message("network document is no longer live")
                    .into()
            })
    }

    fn csp_policy_for_document(&self, document: &NodeHandle) -> CspPolicy {
        self.document_csp
            .get(&document.identity())
            .cloned()
            .unwrap_or_default()
    }

    fn csp_policy_for_node(&self, node: &NodeHandle) -> CspPolicy {
        document_root_for_node(node)
            .map(|document| self.csp_policy_for_document(&document))
            .unwrap_or_default()
    }

    fn sandbox_policy_for_document(&self, document: &NodeHandle) -> IframeSandboxPolicy {
        self.document_sandbox
            .get(&document.identity())
            .copied()
            .unwrap_or_default()
    }

    fn sandbox_allows_scripts_for_node(&self, node: &NodeHandle) -> bool {
        document_root_for_node(node)
            .map(|document| self.sandbox_policy_for_document(&document).allow_scripts)
            .unwrap_or(true)
    }

    fn iframe_allows_fullscreen(&self, iframe: &NodeHandle, child_document: usize) -> bool {
        let owner_document = owner_document_for_node(iframe);
        let same_origin = owner_document.as_ref().is_some_and(|owner| {
            match (
                self.document_security_origins.get(&owner.identity()),
                self.document_security_origins.get(&child_document),
            ) {
                (Some(owner), Some(child)) => owner == child,
                _ => false,
            }
        });
        let allow = iframe.get_attribute("allow");
        if let Some(policy) = allow {
            if let Some(directive) = policy.split(';').find(|directive| {
                directive
                    .split_ascii_whitespace()
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("fullscreen"))
            }) {
                let tokens = directive
                    .split_ascii_whitespace()
                    .skip(1)
                    .collect::<Vec<_>>();
                if tokens
                    .iter()
                    .any(|token| token.eq_ignore_ascii_case("'none'"))
                {
                    return false;
                }
                return tokens.is_empty()
                    || tokens.iter().any(|token| {
                        *token == "*"
                            || token.eq_ignore_ascii_case("'src'")
                            || (same_origin && token.eq_ignore_ascii_case("'self'"))
                    });
            }
        }
        same_origin || iframe.get_attribute("allowfullscreen").is_some()
    }

    fn fullscreen_allowed_for_document(&self, document_id: usize) -> bool {
        if !self.fullscreen_supported || !self.document_is_active(document_id) {
            return false;
        }
        let mut current = document_id;
        let mut seen = HashSet::new();
        while current != self.document.identity() {
            if !seen.insert(current) {
                return false;
            }
            let Some((&iframe_id, _)) = self
                .iframe_documents
                .iter()
                .find(|(_, entry)| entry.document.identity() == current)
            else {
                return false;
            };
            let Some(iframe) = self.get_node(iframe_id) else {
                return false;
            };
            if !self.iframe_allows_fullscreen(&iframe, current) {
                return false;
            }
            let Some(owner) = owner_document_for_node(&iframe) else {
                return false;
            };
            current = owner.identity();
        }
        true
    }

    fn fullscreen_element(&self, document_id: usize) -> Option<NodeHandle> {
        self.fullscreen_elements
            .get(&document_id)
            .and_then(|stack| stack.last())
            .and_then(|id| self.get_node(*id))
            .filter(|node| node.is_fullscreen())
    }

    fn fullscreen_chain(&self, element: &NodeHandle) -> Option<Vec<(usize, NodeHandle)>> {
        let mut document = document_root_for_node(element)?;
        let mut chain = vec![(document.identity(), element.clone())];
        let mut seen = HashSet::new();
        while document.identity() != self.document.identity() {
            if !seen.insert(document.identity()) {
                return None;
            }
            let (&iframe_id, _) = self
                .iframe_documents
                .iter()
                .find(|(_, entry)| entry.document == document)?;
            let iframe = self.get_node(iframe_id)?;
            document = owner_document_for_node(&iframe)?;
            chain.push((document.identity(), iframe));
        }
        Some(chain)
    }

    fn request_fullscreen(&mut self, element: &NodeHandle) -> bool {
        let Some(document) = document_root_for_node(element) else {
            return false;
        };
        if !self.document_is_active(document.identity())
            || !self.fullscreen_allowed_for_document(document.identity())
            || (!self.fullscreen_host_active && !self.fullscreen_transition_allowed)
        {
            return false;
        }
        let Some(chain) = self.fullscreen_chain(element) else {
            return false;
        };
        for (document_id, node) in chain {
            let stack = self.fullscreen_elements.entry(document_id).or_default();
            stack.retain(|id| *id != node.identity());
            stack.push(node.identity());
            node.set_fullscreen(true);
            self.invalidate_style_cache_for_node(&node);
        }
        if !self.fullscreen_host_active {
            self.fullscreen_host_active = true;
            self.pending_fullscreen_transition = Some(FullscreenTransition::Enter);
        }
        true
    }

    fn fully_exit_fullscreen(&mut self, require_host_approval: bool) -> bool {
        if self.fullscreen_elements.is_empty() {
            return false;
        }
        if require_host_approval && !self.fullscreen_transition_allowed {
            return false;
        }
        let stacks = std::mem::take(&mut self.fullscreen_elements);
        for node_id in stacks.into_values().flatten() {
            if let Some(node) = self.get_node(node_id) {
                node.set_fullscreen(false);
                self.invalidate_style_cache_for_node(&node);
            }
        }
        if self.fullscreen_host_active {
            self.fullscreen_host_active = false;
            self.pending_fullscreen_transition = Some(FullscreenTransition::Exit);
        }
        true
    }

    fn fullscreen_subtree_will_be_removed(&mut self, root: &NodeHandle) -> bool {
        let contains_fullscreen = self
            .fullscreen_elements
            .values()
            .flatten()
            .filter_map(|id| self.get_node(*id))
            .any(|node| {
                let mut current = Some(node);
                while let Some(candidate) = current {
                    if candidate == *root {
                        return true;
                    }
                    current = candidate.parent_node().or_else(|| candidate.shadow_host());
                }
                false
            });
        contains_fullscreen && self.fully_exit_fullscreen(false)
    }

    fn record_csp_violation(
        &mut self,
        document: &NodeHandle,
        resource_type: ResourceType,
        blocked_uri: impl Into<String>,
    ) {
        let blocked_uri = blocked_uri.into();
        let document_id = document.identity();
        let directive = resource_type.directive().to_string();
        let key = (document_id, directive.clone(), blocked_uri.clone());
        if self.csp_violation_keys.contains(&key)
            || self.csp_violation_keys.len() >= MAX_CSP_VIOLATIONS
        {
            return;
        }
        self.csp_violation_keys.insert(key);
        self.csp_violations.push(CspViolation {
            document_id,
            effective_directive: directive,
            blocked_uri,
            resource_type: resource_type.label().to_string(),
            source: "enforced".to_string(),
        });
    }

    fn record_csp_violation_for_node(
        &mut self,
        node: &NodeHandle,
        resource_type: ResourceType,
        blocked_uri: impl Into<String>,
    ) {
        if let Some(document) = document_root_for_node(node) {
            self.record_csp_violation(&document, resource_type, blocked_uri);
        }
    }

    /// Refreshes the resolver's CSP-filtered inline-style set after a `style`
    /// attribute mutation without reparsing the document's stylesheets.  A
    /// normal inline-style edit is intentionally only a computed-style cache
    /// invalidation; this keeps the existing resolver-generation contract while
    /// still making a newly added/removed blocked attribute take effect.
    fn refresh_csp_inline_style_nodes(&mut self, node: &NodeHandle) {
        let Some(document) = document_root_for_node(node) else {
            return;
        };
        let blocked = !self
            .csp_policy_for_document(&document)
            .allows_inline(ResourceType::Style)
            && node.get_attribute("style").is_some();
        if blocked {
            self.record_csp_violation(
                &document,
                ResourceType::Style,
                format!("style-attribute:{}", node.identity()),
            );
        }
        if let Some(resolver) = self
            .document_styles
            .get_mut(&document.identity())
            .and_then(|entry| entry.resolver.as_mut())
        {
            resolver.set_blocked_inline_style_node(node.identity(), blocked);
        }
    }

    /// Marks the given document's cached style resolver as stale, creating its
    /// entry if it does not yet exist.
    ///
    /// `document` must be the root [`Document`] node of a browsing context (the
    /// top-level document or an iframe sub-document); it is keyed by its node
    /// identity. When it is the main document, the cached layout tree is also
    /// dropped, because layout is only maintained for the main document.
    fn mark_document_style_dirty(&mut self, document: &NodeHandle) {
        self.form_validation.invalidate();
        let document_id = document.identity();
        self.document_styles.entry(document_id).or_default().dirty = true;
        if document_id == self.document.identity() {
            self.style_generation = self.style_generation.saturating_add(1);
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
            self.invalidate_paint_cache();
        } else {
            self.invalidate_paint_cache();
        }
    }

    /// Invalidates values derived from the DOM without rebuilding the parsed
    /// stylesheet/rule-index portion of an existing resolver.
    fn invalidate_document_style_cache(&mut self, document: &NodeHandle) {
        self.form_validation.invalidate();
        self.invalidate_document_style_cache_keeping_validation(document);
    }

    /// Invalidates cascade-derived values for `document` while keeping the
    /// cached constraint-validation snapshot, for changes that cannot affect
    /// form validity.
    fn invalidate_document_style_cache_keeping_validation(&mut self, document: &NodeHandle) {
        let document_id = document.identity();
        if let Some(entry) = self.document_styles.get_mut(&document_id) {
            entry.needs_full_sample = true;
            if let Some(resolver) = entry.resolver.as_mut() {
                resolver.invalidate_style_cache();
            }
        }
        if document_id == self.document.identity() {
            self.style_generation = self.style_generation.saturating_add(1);
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
            self.invalidate_paint_cache();
        } else {
            self.invalidate_paint_cache();
        }
    }

    /// Invalidates the scroll-adjusted paint geometry while retaining the
    /// layout tree when the invalidation does not require reflow.
    fn invalidate_paint_cache(&mut self) {
        self.invalidate_layout_metrics_cache();
        self.paint_generation = self.paint_generation.saturating_add(1);
        self.adjusted_layout_cache = None;
    }

    fn invalidate_layout_metrics_cache(&mut self) {
        self.layout_metrics_generation = self.layout_metrics_generation.saturating_add(1);
    }

    /// Invalidates computed/selector results for a node mutation while keeping
    /// stylesheet parsing intact. Detached nodes affect no live document.
    fn invalidate_style_cache_for_node(&mut self, node: &NodeHandle) {
        self.form_validation.invalidate();
        self.invalidate_style_cache_for_node_keeping_validation(node);
    }

    /// Invalidates styles after a change to `node`'s inline `style` attribute.
    /// Constraint validity depends on form attributes, values and tree
    /// structure, never on inline style, so the validation snapshot stays
    /// fresh and the next CSSOM read does not rescan every form control.
    fn invalidate_inline_style_for_node(&mut self, node: &NodeHandle) {
        self.invalidate_style_cache_for_node_keeping_validation(node);
    }

    fn invalidate_style_cache_for_node_keeping_validation(&mut self, node: &NodeHandle) {
        if let Some(document) = document_root_for_node(node) {
            self.invalidate_document_style_cache_keeping_validation(&document);
        }
        if matches!(node.tag_name().as_deref(), Some("iframe" | "frame"))
            && let Some(child) = self.iframe_documents.get(&node.identity())
        {
            let child_document = child.document.clone();
            self.mark_document_style_dirty(&child_document);
        }
    }

    /// Composed paths cross shadow hosts; hover also crosses iframe boundaries.
    /// Store only node ids, so a detached subtree is not kept alive by input.
    fn user_action_path(&self, target_id: Option<usize>, cross_iframes: bool) -> Vec<usize> {
        let mut current = target_id
            .and_then(|id| self.get_node(id))
            .filter(|node| self.node_is_in_active_document(node));
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        while let Some(node) = current {
            if !seen.insert(node.identity()) {
                break;
            }
            if node.node_type() == NodeType::Element {
                path.push(node.identity());
            }
            current = node
                .assigned_slot()
                .or_else(|| node.parent_node())
                .or_else(|| node.shadow_host());
            if cross_iframes && current.is_none() && node.node_type() == NodeType::Document {
                current = self
                    .iframe_documents
                    .iter()
                    .find(|(_, entry)| entry.document.identity() == node.identity())
                    .and_then(|(iframe_id, _)| self.get_node(*iframe_id));
            }
        }
        path
    }

    /// Focus also matches shadow hosts whose shadow tree contains the focused
    /// element. Slotted light-DOM children do not focus their host.
    fn focus_subjects(&self, target_id: Option<usize>) -> Vec<usize> {
        let mut current = target_id
            .and_then(|id| self.get_node(id))
            .filter(|node| self.node_is_in_active_document(node));
        let mut subjects = Vec::new();
        let mut seen = HashSet::new();
        if let Some(node) = &current {
            subjects.push(node.identity());
        }
        while let Some(node) = current {
            if !seen.insert(node.identity()) {
                break;
            }
            if let Some(host) = node.shadow_host() {
                subjects.push(host.identity());
            }
            current = node.parent_node().or_else(|| node.shadow_host());
        }
        subjects
    }

    /// Changes dynamic selector flags once per input transition and invalidates
    /// the affected documents without reparsing their stylesheets.
    fn update_user_action_target(&mut self, kind: &str, target_id: Option<usize>, visible: bool) {
        let next = self.user_action_path(target_id, kind == "hover");
        let next_subjects = (kind == "focus").then(|| self.focus_subjects(target_id));
        let next_visible = if kind == "focus" && visible {
            next.first().copied()
        } else {
            None
        };
        let unchanged = match kind {
            "hover" => self.hover_path == next,
            "active" => self.active_path == next,
            "focus" => {
                self.focus_path == next
                    && next_subjects
                        .as_ref()
                        .is_some_and(|subjects| &self.focus_subjects == subjects)
                    && self.focus_visible_id == next_visible
            }
            _ => return,
        };
        if unchanged {
            return;
        }
        let old_subjects = next_subjects
            .as_ref()
            .map(|subjects| std::mem::replace(&mut self.focus_subjects, subjects.clone()));
        let old = match kind {
            "hover" => std::mem::replace(&mut self.hover_path, next.clone()),
            "active" => std::mem::replace(&mut self.active_path, next.clone()),
            "focus" => std::mem::replace(&mut self.focus_path, next.clone()),
            _ => return,
        };
        let flag = if kind == "focus" {
            "focus-within"
        } else {
            kind
        };
        let old_ids: HashSet<_> = old.iter().copied().collect();
        let next_ids: HashSet<_> = next.iter().copied().collect();
        let previous_visible = self.focus_visible_id;
        let mut affected = HashMap::new();
        let mut set_flag = |id, name, enabled| {
            if let Some(node) = self.get_node(id)
                && node.set_user_action_state(name, enabled)
                && let Some(document) = document_root_for_node(&node)
            {
                affected.insert(document.identity(), document);
            }
        };
        for id in old_ids.difference(&next_ids) {
            set_flag(*id, flag, false);
        }
        for id in next_ids.difference(&old_ids) {
            set_flag(*id, flag, true);
        }
        if kind == "focus" {
            let old_focus: HashSet<_> = old_subjects.unwrap_or_default().into_iter().collect();
            let new_focus: HashSet<_> = next_subjects.unwrap_or_default().into_iter().collect();
            for id in old_focus.difference(&new_focus) {
                set_flag(*id, "focus", false);
            }
            for id in new_focus.difference(&old_focus) {
                set_flag(*id, "focus", true);
            }
            if previous_visible != next_visible {
                if let Some(id) = previous_visible {
                    set_flag(id, "focus-visible", false);
                }
                if let Some(id) = next_visible {
                    set_flag(id, "focus-visible", true);
                }
            }
        }
        drop(set_flag);
        if kind == "focus" {
            self.focus_visible_id = next_visible;
        }
        for document in affected.into_values() {
            self.invalidate_document_style_cache(&document);
        }
    }

    /// Drops cached layout for a live main-document node without touching style caches.
    fn invalidate_layout_for_node(&mut self, node: &NodeHandle) {
        if document_root_for_node(node)
            .is_some_and(|document| document.identity() == self.document.identity())
        {
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
            self.invalidate_paint_cache();
        }
    }

    /// Marks the document that `node` currently lives in as stale.
    ///
    /// A detached node cannot affect a live document's style or layout. Its
    /// eventual insertion invalidates the target document in the tree-mutation
    /// path, so mutations made while detached do not need to drop any cache.
    fn mark_style_dirty_for_node(&mut self, node: &NodeHandle) {
        self.form_validation.invalidate();
        if let Some(document) = document_root_for_node(node) {
            self.mark_document_style_dirty(&document);
        }
        // The iframe element belongs to the parent document, but its rendered
        // content-box establishes the child document's viewport. A width/height
        // style or attribute mutation must therefore invalidate both caches.
        if matches!(node.tag_name().as_deref(), Some("iframe" | "frame"))
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
        self.style_generation = self.style_generation.saturating_add(1);
        for entry in self.document_styles.values_mut() {
            entry.dirty = true;
        }
        self.capture_scroll_offsets_before_layout();
        self.layout_root = None;
        self.invalidate_paint_cache();
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

    /// Rebuilds a document's author cascade, retaining its loaded resources.
    /// The same resolver feeds computed style, geometry, hit testing and paint.
    fn ensure_style_resolver(&mut self, document: &NodeHandle) {
        let document_id = document.identity();
        let viewport = self.viewport_for_document(document);
        let viewport_size = Some((viewport.width, viewport.height));
        let needs_rebuild = match self.document_styles.get(&document_id) {
            Some(entry) => {
                entry.dirty || entry.resolver.is_none() || entry.viewport_size != viewport_size
            }
            None => true,
        };
        if !needs_rebuild {
            let transition_time_ms = self.event_loop.rendering_time_ms();
            let live_animations = self.live_css_animations;
            let (time_changed, requires_layout) = self
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut())
                .map(|resolver| {
                    let changed = resolver.set_transition_time_ms(transition_time_ms);
                    let animation_changed =
                        live_animations && resolver.set_animation_time_ms(transition_time_ms);
                    (
                        changed || animation_changed,
                        // Keyframes can animate geometry as well as paint properties;
                        // conservatively rebuild layout when their sampled values change.
                        resolver.running_transitions_require_layout() || animation_changed,
                    )
                })
                .unwrap_or((false, false));
            if time_changed {
                if document_id == self.document.identity() {
                    self.style_generation = self.style_generation.saturating_add(1);
                    if requires_layout {
                        self.capture_scroll_offsets_before_layout();
                        self.layout_root = None;
                    }
                }
                self.invalidate_paint_cache();
            }
            return;
        }
        // Build the resolver as a local first so no `document_styles` borrow is
        // held while `self.viewport` / the document tree are read, then store it.
        let timeline = self
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
            .map(|resolver| {
                (
                    resolver.take_transition_timeline(),
                    resolver.take_animation_timeline(),
                )
            });
        let transition_time_ms = self.event_loop.rendering_time_ms();
        let mut resolver = StyleResolver::new();
        if let Some((transitions, animations)) = timeline {
            resolver.install_transition_timeline(transitions);
            resolver.install_animation_timeline(animations);
        }
        let _ = resolver.set_transition_time_ms(transition_time_ms);
        if self.live_css_animations {
            resolver.set_animation_time_ms(transition_time_ms);
        }
        resolver.set_viewport(viewport.width, viewport.height);
        let policy = self.csp_policy_for_document(document);
        let base = crate::paint::stylesheet::extract_document_base_url(
            document,
            self.base_url_for_document(document_id).as_ref(),
        );
        let mut resources = self
            .document_styles
            .get_mut(&document_id)
            .map(|entry| std::mem::take(&mut entry.resources))
            .unwrap_or_default();
        resources.set_error_reporter(self.error_reporter.clone());
        let mut web_fonts = crate::font::WebFontRegistry::new();
        let site_for_cookies = self.location_href.parse::<crate::http::Url>().ok();
        let (stylesheet_nodes, font_scope_parents) = collect_stylesheet_nodes(document);
        for (style_node, scope, implicit_scope_root) in stylesheet_nodes {
            let (css, blocked) = resources.load_node(
                &style_node,
                base.as_ref(),
                &policy,
                site_for_cookies.as_ref(),
                Arc::clone(&self.cookie_store),
            );
            for blocked_uri in blocked {
                self.record_csp_violation(document, ResourceType::Style, blocked_uri);
            }
            let (sheet, parse_failed) =
                crate::paint::stylesheet::parse_stylesheet_forgiving_with_status(&css);
            if parse_failed {
                report_safe_stylesheet_parse_failure(self.error_reporter.clone());
            }
            if let Some((scope, order)) = scope {
                resolver.add_scoped_stylesheet_in_order_with_implicit_scope_root(
                    Origin::Author,
                    sheet,
                    scope,
                    order,
                    implicit_scope_root,
                );
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
        let adopted_stylesheets = collect_adopted_stylesheets(&self.adopted_stylesheets, document);
        if !adopted_stylesheets.is_empty() && !policy.allows_inline(ResourceType::Style) {
            // Constructable stylesheets are author-provided style text just
            // like <style> elements, so style-src must gate them as well.
            self.record_csp_violation(document, ResourceType::Style, "adopted-stylesheets");
        } else {
            for (scope, css) in adopted_stylesheets {
                let (sheet, parse_failed) =
                    crate::paint::stylesheet::parse_stylesheet_forgiving_with_status(&css);
                if parse_failed {
                    report_safe_stylesheet_parse_failure(self.error_reporter.clone());
                }
                if let Some((scope_root, order)) = scope {
                    resolver.add_scoped_stylesheet_in_order(
                        Origin::Author,
                        sheet,
                        scope_root,
                        order,
                    );
                } else {
                    resolver.add_stylesheet(Origin::Author, sheet);
                }
            }
        }
        resolver.set_script_registered_custom_properties(
            self.registered_custom_properties
                .get(&document_id)
                .cloned()
                .unwrap_or_default(),
        );
        let active_font_rules = resolver.active_font_face_rules();
        let font_winners = resolver.resolved_font_face_rules();
        font_loading::sync_stylesheets(self, document, active_font_rules);
        self.font_loading
            .append_to(document_id, &mut web_fonts, &font_winners);
        for (scope_root, parent_scope_root) in font_scope_parents {
            web_fonts.register_scope_parent(scope_root, parent_scope_root);
        }
        let web_fonts = Arc::new(web_fonts);
        resolver.set_web_fonts(web_fonts.clone());
        let blocked_inline_styles = if policy.allows_inline(ResourceType::Style) {
            HashSet::new()
        } else {
            let mut blocked = HashSet::new();
            for element in collect_element_nodes(document) {
                if element.get_attribute("style").is_some() {
                    self.record_csp_violation(
                        document,
                        ResourceType::Style,
                        format!("style-attribute:{}", element.identity()),
                    );
                    blocked.insert(element.identity());
                }
            }
            blocked
        };
        resolver.set_blocked_inline_style_nodes(blocked_inline_styles);
        self.document_styles.insert(
            document_id,
            DocumentStyleEntry {
                viewport_size,
                resolver: Some(resolver),
                resources,
                web_fonts,
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

        if let Some(parent) = owner_document_for_node(&iframe)
            && parent.identity() != self.document.identity()
        {
            let parent_viewport = self.viewport_for_document(&parent);
            if let Some(root) = build_child_document_layout(self, &parent, parent_viewport)
                && let Some(owner) = find_layout_box(&root, &iframe)
            {
                return Rect {
                    x: 0.0,
                    y: 0.0,
                    width: owner.dimensions.content.width.max(0.0),
                    height: owner.dimensions.content.height.max(0.0),
                };
            }
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

    fn queue_document_once(queue: &mut Vec<usize>, document_id: usize) {
        if !queue.contains(&document_id) {
            queue.push(document_id);
        }
    }

    fn queue_window_resize(&mut self, document_id: usize) {
        Self::queue_document_once(&mut self.pending_window_resize_documents, document_id);
    }

    fn queue_visual_viewport_resize(&mut self, document_id: usize) {
        Self::queue_document_once(
            &mut self.pending_visual_viewport_resize_documents,
            document_id,
        );
    }

    fn queue_visual_viewport_scroll(&mut self, document_id: usize) {
        Self::queue_document_once(
            &mut self.pending_visual_viewport_scroll_documents,
            document_id,
        );
    }

    fn visual_viewport_for_document(&mut self, document: &NodeHandle) -> VisualViewportState {
        if document.identity() == self.document.identity() {
            return self.visual_viewport;
        }
        let viewport = self.viewport_for_document(document);
        VisualViewportState {
            width: viewport.width,
            height: viewport.height,
            offset_left: 0.0,
            offset_top: 0.0,
            scale: 1.0,
        }
    }

    /// Samples every child browsing context that owns a Realm. Resizing the
    /// iframe element changes both its Window and visual viewport; first
    /// observation establishes the baseline and must not synthesize an event.
    fn collect_iframe_viewport_resizes(&mut self) {
        let documents: Vec<_> = self
            .iframe_documents
            .values()
            .filter(|entry| entry.realm.is_some())
            .map(|entry| entry.document.clone())
            .collect();
        for document in documents {
            let document_id = document.identity();
            let viewport = self.viewport_for_document(&document);
            let next = (viewport.width, viewport.height);
            match self.observed_iframe_viewports.insert(document_id, next) {
                Some(previous) if previous != next => {
                    self.queue_window_resize(document_id);
                    self.queue_visual_viewport_resize(document_id);
                }
                _ => {}
            }
        }
    }

    fn window_scroll_for_document(&mut self, document_id: usize) -> (f32, f32) {
        if document_id == self.document.identity() {
            let current = self.window_scroll;
            if current != (0.0, 0.0) {
                self.set_window_scroll(current.0, current.1);
            }
            self.window_scroll
        } else {
            self.iframe_window_scrolls
                .get(&document_id)
                .copied()
                .unwrap_or((0.0, 0.0))
        }
    }

    fn set_window_scroll_for_document(&mut self, document_id: usize, x: f32, y: f32) -> bool {
        if document_id == self.document.identity() {
            return self.set_window_scroll(x, y);
        }
        if !self
            .iframe_documents
            .values()
            .any(|entry| entry.document.identity() == document_id)
        {
            return false;
        }
        // Child documents are not part of the top-level paint tree yet, so
        // their scrollable overflow is unavailable here. Retain the requested
        // positive offset instead of clamping it against the parent's extent.
        let next = (x.max(0.0), y.max(0.0));
        let current = self
            .iframe_window_scrolls
            .get(&document_id)
            .copied()
            .unwrap_or((0.0, 0.0));
        if current == next {
            return false;
        }
        self.iframe_window_scrolls.insert(document_id, next);
        self.invalidate_paint_cache();
        self.queue_scroll_target(document_id);
        true
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
        let base = crate::paint::stylesheet::extract_document_base_url(
            &document,
            self.base_url_for_document(document_id).as_ref(),
        );
        let image_site = self.location_href.parse::<crate::http::Url>().ok();
        let image_cookies = Arc::clone(&self.cookie_store);
        let animation_time = self.event_loop.rendering_time_ms() as u64;
        let layout_reporter = self.error_reporter.clone();
        let relevant_nodes = self
            .content_visibility_focus_nodes
            .union(&self.content_visibility_selection_nodes)
            .copied()
            .collect();
        let forced_in_layout = self.content_visibility_forced_nodes.clone();
        let content_visibility_input = crate::layout::ContentVisibilityLayoutInput {
            visible_rect: Rect {
                x: self.window_scroll.0,
                y: self.window_scroll.1,
                width: viewport.width,
                height: viewport.height,
            },
            forced_nodes: forced_in_layout.clone(),
            relevant_nodes,
            remembered_sizes: self.content_visibility_remembered_sizes.clone(),
        };
        // Compute into a local so the `document_styles` borrow is released
        // before assigning `self.layout_root` (a different field).
        let (layout, content_visibility_report) = self
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| {
                let resolver = entry.resolver.as_mut()?;
                Some(crate::layout::with_layout_fonts(
                    crate::paint::text::load_text_fonts(),
                    Some(entry.web_fonts.clone()),
                    || {
                        crate::layout::with_image_cookie_store(
                            image_cookies,
                            image_site,
                            document_id,
                            || {
                                crate::layout::with_image_base_url(base, || {
                                    crate::layout::with_image_animation_time(animation_time, || {
                                        crate::layout::with_error_reporter(layout_reporter, || {
                                            crate::layout::layout_tree_with_content_visibility(
                                                &document,
                                                resolver,
                                                viewport,
                                                content_visibility_input,
                                            )
                                        })
                                    })
                                })
                            },
                        )
                    },
                ))
            })
            .unwrap_or_default();
        self.content_visibility_auto_nodes = content_visibility_report.auto_nodes;
        self.content_visibility_skipped_nodes = content_visibility_report.skipped_nodes;
        self.content_visibility_remembered_sizes
            .retain(|node, _| self.nodes.contains_key(node));
        for node in &content_visibility_report.visited_nodes {
            if !content_visibility_report
                .auto_intrinsic_nodes
                .contains(node)
            {
                self.content_visibility_remembered_sizes.remove(node);
            }
        }
        self.content_visibility_remembered_sizes
            .extend(content_visibility_report.remembered_sizes);
        self.content_visibility_forced_in_layout = forced_in_layout;
        self.content_visibility_forced_nodes.clear();
        // This is a generation of rebuild attempts, not only successful trees:
        // a failed rebuild must not leave an older adjusted tree reusable.
        self.layout_generation = self.layout_generation.saturating_add(1);
        self.scroll_snap_cache.clear();
        self.invalidate_layout_metrics_cache();
        self.layout_root = layout;
        let previous_scroll_offsets = std::mem::take(&mut self.scroll_offsets_before_layout);
        let mut resnap_targets: Vec<_> = previous_scroll_offsets.keys().copied().collect();
        if self.window_scroll != (0.0, 0.0) {
            resnap_targets.push(document_id);
        }
        let mut clamped_targets = Vec::new();
        let mut paint_invalidated = false;
        if let Some(layout_root) = self.layout_root.as_ref() {
            for (node_id, previous) in previous_scroll_offsets {
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
                    self.scroll_generation = self.scroll_generation.saturating_add(1);
                    paint_invalidated = true;
                    clamped_targets.push(node_id);
                }
            }
        }
        if paint_invalidated {
            self.invalidate_paint_cache();
        }
        for node_id in clamped_targets {
            self.queue_scroll_target(node_id);
        }
        self.resnap_after_layout(resnap_targets);
    }

    fn content_visibility_ancestor_ids(node: &NodeHandle) -> HashSet<usize> {
        let mut result = HashSet::new();
        let mut current = Some(node.clone());
        while let Some(candidate) = current {
            result.insert(candidate.identity());
            current = candidate.assigned_slot().or_else(|| {
                candidate.parent_node().and_then(|parent| {
                    if parent.node_type() == NodeType::Element {
                        Some(parent)
                    } else {
                        parent.shadow_host()
                    }
                })
            });
        }
        result
    }

    fn replace_content_visibility_relevance(
        &mut self,
        focus: Option<HashSet<usize>>,
        selection: Option<HashSet<usize>>,
    ) {
        let mut changed = false;
        if let Some(focus) = focus
            && self.content_visibility_focus_nodes != focus
        {
            self.content_visibility_focus_nodes = focus;
            changed = true;
        }
        if let Some(selection) = selection
            && self.content_visibility_selection_nodes != selection
        {
            self.content_visibility_selection_nodes = selection;
            changed = true;
        }
        if changed {
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
            self.invalidate_paint_cache();
        }
    }

    fn set_content_visibility_focus(&mut self, node: Option<&NodeHandle>) {
        let relevant = node
            .filter(|node| self.node_is_in_active_document(node))
            .map(Self::content_visibility_ancestor_ids)
            .unwrap_or_default();
        self.replace_content_visibility_relevance(Some(relevant), None);
    }

    fn set_content_visibility_selection(
        &mut self,
        start: Option<&NodeHandle>,
        end: Option<&NodeHandle>,
    ) {
        let mut relevant = HashSet::new();
        for node in [start, end].into_iter().flatten() {
            if self.node_is_in_active_document(node) {
                relevant.extend(Self::content_visibility_ancestor_ids(node));
            }
        }
        self.replace_content_visibility_relevance(None, Some(relevant));
    }

    fn ensure_content_visibility_geometry(&mut self, node: &NodeHandle) {
        self.ensure_layout();
        let ancestors = Self::content_visibility_ancestor_ids(node);
        let skipped_ancestors = ancestors
            .iter()
            .copied()
            .filter(|identity| {
                *identity != node.identity()
                    && self.content_visibility_skipped_nodes.contains(identity)
            })
            .collect::<Vec<_>>();
        if skipped_ancestors
            .iter()
            .all(|identity| self.content_visibility_forced_in_layout.contains(identity))
        {
            return;
        }
        self.content_visibility_forced_nodes.extend(ancestors);
        self.capture_scroll_offsets_before_layout();
        self.layout_root = None;
        self.invalidate_paint_cache();
        self.ensure_layout();
    }

    fn content_visibility_skips_inner_text(&mut self, node: &NodeHandle) -> bool {
        self.ensure_layout();
        Self::content_visibility_ancestor_ids(node)
            .iter()
            .any(|identity| self.content_visibility_skipped_nodes.contains(identity))
    }

    fn retarget_content_visibility_hit(&self, node: NodeHandle) -> NodeHandle {
        let original = node.identity();
        let mut current = Some(node.clone());
        while let Some(candidate) = current {
            if candidate.identity() != original
                && self
                    .content_visibility_skipped_nodes
                    .contains(&candidate.identity())
            {
                return candidate;
            }
            current = candidate
                .assigned_slot()
                .or_else(|| candidate.parent_node())
                .or_else(|| candidate.shadow_host());
        }
        node
    }

    /// Builds paint-time geometry once per tracked style/layout/paint/scroll
    /// state and reuses it across CSSOM geometry queries and hit tests.
    fn ensure_adjusted_layout(&mut self) {
        self.ensure_layout();
        let current = (
            self.layout_generation,
            self.scroll_generation,
            self.style_generation,
            self.paint_generation,
        );
        if self.adjusted_layout_cache.as_ref().is_some_and(|cache| {
            (
                cache.layout_generation,
                cache.scroll_generation,
                cache.style_generation,
                cache.paint_generation,
            ) == current
        }) {
            return;
        }
        let Some(mut root) = self.layout_root.clone() else {
            self.adjusted_layout_cache = None;
            return;
        };
        let document_id = self.document.identity();
        let Some(resolver) = self
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
        else {
            self.adjusted_layout_cache = None;
            return;
        };
        crate::paint::apply_scroll_offsets(&mut root, resolver, self.viewport, self.window_scroll);
        self.adjusted_layout_cache = Some(AdjustedLayoutCache {
            layout_generation: current.0,
            scroll_generation: current.1,
            style_generation: current.2,
            paint_generation: current.3,
            root: Rc::new(root),
        });
        #[cfg(test)]
        {
            self.adjusted_layout_builds = self.adjusted_layout_builds.saturating_add(1);
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
        self.scroll_generation = self.scroll_generation.saturating_add(1);
        if !self.content_visibility_auto_nodes.is_empty() {
            self.capture_scroll_offsets_before_layout();
            self.layout_root = None;
        }
        self.invalidate_paint_cache();
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
        self.ensure_content_visibility_geometry(node);
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
        self.ensure_content_visibility_geometry(node);
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
            self.scroll_generation = self.scroll_generation.saturating_add(1);
            self.invalidate_paint_cache();
            self.queue_scroll_target(node.identity());
        }
        changed
    }

    fn cancel_smooth_scroll(&mut self, target: SmoothScrollTarget) {
        self.smooth_scrolls
            .retain(|animation| animation.target != target);
    }

    fn scroll_snap_geometry(
        &mut self,
        target: SmoothScrollTarget,
    ) -> Option<&scroll_snap::Geometry> {
        let node_id = match target {
            SmoothScrollTarget::Document(document_id) => {
                if document_id != self.document.identity() {
                    return None;
                }
                document_id
            }
            SmoothScrollTarget::Element(node_id) => node_id,
        };
        self.ensure_layout();
        if !self.scroll_snap_cache.contains_key(&node_id) {
            let root = self.layout_root.as_ref()?;
            let resolver = self
                .document_styles
                .get_mut(&self.document.identity())?
                .resolver
                .as_mut()?;
            let geometry = match target {
                SmoothScrollTarget::Document(_) => {
                    scroll_snap::Geometry::for_viewport(root, resolver, self.viewport)
                }
                SmoothScrollTarget::Element(node_id) => {
                    let node = self.nodes.get(&node_id)?;
                    let layout = find_layout_box(root, node)?;
                    scroll_snap::Geometry::for_element(layout, resolver)
                }
            };
            self.scroll_snap_cache.insert(node_id, geometry);
        }
        self.scroll_snap_cache
            .get(&node_id)
            .and_then(Option::as_ref)
    }

    fn snapped_scroll_endpoint(
        &mut self,
        target: SmoothScrollTarget,
        requested: (f32, f32),
        preserve: scroll_snap::Selection,
    ) -> (f32, f32) {
        let selected = self
            .scroll_snap_geometry(target)
            .map(|geometry| geometry.choose(requested, preserve));
        let node_id = match target {
            SmoothScrollTarget::Document(node_id) | SmoothScrollTarget::Element(node_id) => node_id,
        };
        if let Some((position, selection)) = selected {
            if selection.x.is_some() || selection.y.is_some() {
                self.scroll_snap_selection.insert(node_id, selection);
            } else {
                self.scroll_snap_selection.remove(&node_id);
            }
            position
        } else {
            self.scroll_snap_selection.remove(&node_id);
            requested
        }
    }

    fn resnap_after_layout(&mut self, additional_targets: Vec<usize>) {
        let mut selected = self.scroll_snap_selection.clone();
        for node_id in additional_targets {
            selected.entry(node_id).or_default();
        }
        for (node_id, preserve) in selected {
            if node_id == self.document.identity() {
                let target = SmoothScrollTarget::Document(node_id);
                let position = self.snapped_scroll_endpoint(target, self.window_scroll, preserve);
                self.set_window_scroll(position.0, position.1);
            } else if let Some(node) = self.nodes.get(&node_id).cloned() {
                let target = SmoothScrollTarget::Element(node_id);
                let current = node.scroll_offset();
                let position = self.snapped_scroll_endpoint(target, current, preserve);
                self.set_element_scroll(&node, position.0, position.1);
            } else {
                self.scroll_snap_selection.remove(&node_id);
            }
        }
    }

    fn scroll_document_to(&mut self, document_id: usize, x: f32, y: f32, smooth: bool) {
        let target = SmoothScrollTarget::Document(document_id);
        self.cancel_smooth_scroll(target);
        let (x, y) =
            self.snapped_scroll_endpoint(target, (x, y), scroll_snap::Selection::default());
        if !smooth {
            self.set_window_scroll_for_document(document_id, x, y);
            return;
        }
        let current = self.window_scroll_for_document(document_id);
        let end = if document_id == self.document.identity() {
            let (max_x, max_y) = self.window_scroll_extent();
            (x.clamp(0.0, max_x), y.clamp(0.0, max_y))
        } else {
            (x.max(0.0), y.max(0.0))
        };
        if current == end {
            return;
        }
        self.smooth_scrolls.push(SmoothScrollAnimation {
            target,
            start: current,
            end,
            started_ms: self.event_loop.rendering_time_ms(),
        });
    }

    fn scroll_element_to(&mut self, node: &NodeHandle, x: f32, y: f32, smooth: bool) {
        let target = SmoothScrollTarget::Element(node.identity());
        self.cancel_smooth_scroll(target);
        let (x, y) =
            self.snapped_scroll_endpoint(target, (x, y), scroll_snap::Selection::default());
        if !smooth {
            self.set_element_scroll(node, x, y);
            return;
        }
        self.ensure_content_visibility_geometry(node);
        let Some(layout) = self
            .layout_root
            .as_ref()
            .and_then(|root| find_layout_box(root, node))
        else {
            return;
        };
        if !layout.is_scroll_container() {
            return;
        }
        let start = layout.scroll_offset();
        let (max_x, max_y) = layout.max_scroll_offset();
        let end = (x.clamp(0.0, max_x), y.clamp(0.0, max_y));
        if start == end {
            return;
        }
        self.smooth_scrolls.push(SmoothScrollAnimation {
            target,
            start,
            end,
            started_ms: self.event_loop.rendering_time_ms(),
        });
    }

    fn sample_smooth_scrolls(&mut self) {
        let now = self.event_loop.rendering_time_ms();
        let animations = std::mem::take(&mut self.smooth_scrolls);
        for animation in animations {
            let progress =
                ((now - animation.started_ms) / SMOOTH_SCROLL_DURATION_MS).clamp(0.0, 1.0);
            // A fixed smoothstep curve keeps controlled-clock tests exact while
            // avoiding a visible velocity jump at either endpoint.
            let eased = progress * progress * (3.0 - 2.0 * progress);
            let position = (
                animation.start.0 + (animation.end.0 - animation.start.0) * eased as f32,
                animation.start.1 + (animation.end.1 - animation.start.1) * eased as f32,
            );
            let live = match animation.target {
                SmoothScrollTarget::Document(document_id) => {
                    if self.document_is_active(document_id) {
                        self.set_window_scroll_for_document(document_id, position.0, position.1);
                        true
                    } else {
                        false
                    }
                }
                SmoothScrollTarget::Element(node_id) => self
                    .get_node(node_id)
                    .filter(|node| document_root_for_node(node).is_some())
                    .is_some_and(|node| {
                        self.set_element_scroll(&node, position.0, position.1);
                        true
                    }),
            };
            if live && progress < 1.0 {
                self.smooth_scrolls.push(animation);
            }
        }
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
        #[cfg(test)]
        query_tests::record_root_step();
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

/// Finds the HTML element indicated by a URL fragment in this Document.
/// Literal IDs take precedence over decoded IDs, and IDs take precedence over
/// legacy `a[name]` anchors even when the anchor occurs earlier in tree order.
fn find_fragment_target(document: &NodeHandle, url: &str) -> Option<NodeHandle> {
    let (_, fragment) = url.split_once('#')?;
    if fragment.is_empty() {
        return None;
    }
    if let Some(target) = find_potential_fragment_target(document, fragment) {
        return Some(target);
    }
    let mut decoded = Vec::with_capacity(fragment.len());
    let bytes = fragment.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            decoded.push(((high << 4) | low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8_lossy(&decoded);
    let decoded = decoded.strip_prefix('\u{feff}').unwrap_or(&decoded);
    if decoded == fragment {
        return None;
    }
    find_potential_fragment_target(document, decoded)
}

fn find_potential_fragment_target(document: &NodeHandle, fragment: &str) -> Option<NodeHandle> {
    let mut pending = document.child_nodes();
    pending.reverse();
    let mut named_anchor = None;
    while let Some(node) = pending.pop() {
        if node.node_type() == NodeType::Element {
            if node.get_attribute("id").as_deref() == Some(fragment) {
                return Some(node);
            }
            if named_anchor.is_none()
                && node.is_html_element()
                && node.tag_name().as_deref() == Some("a")
                && node.get_attribute("name").as_deref() == Some(fragment)
            {
                named_anchor = Some(node.clone());
            }
        }
        pending.extend(node.child_nodes().into_iter().rev());
    }
    named_anchor
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
    /// Cooperative wall-clock limit per evaluation or callback (default: 5 seconds).
    ///
    /// Interpreter and JIT execution share the deadline, including asynchronous
    /// host suspension. A blocking native function is checked when it returns.
    /// Trusted DOM bootstrap runs before the page's limits are installed.
    pub timeout: std::time::Duration,
    /// Maximum loop iterations executed by one JavaScript evaluation.
    ///
    /// The Boa fork exposes this as a runtime limit. It is deliberately
    /// deterministic and applies to synchronous and asynchronous evaluation,
    /// preventing an infinite loop from monopolizing a page task.
    pub max_loop_iterations: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            timeout: std::time::Duration::from_secs(5),
            max_loop_iterations: 1_000_000,
        }
    }
}

/// Omoikane-owned snapshot of baseline-JIT counters for performance tooling.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct BaselineJitDiagnostics {
    /// Whether the baseline-JIT feature produced this snapshot.
    pub enabled: bool,
    /// Hot loops submitted to the emitter.
    pub compile_requests: u64,
    /// Requests that installed generated code.
    pub successful_compilations: u64,
    /// Requests rejected as unsupported or invalid.
    pub compile_rejections: u64,
    /// Nanoseconds spent verifying and emitting compile requests.
    pub total_compile_time_ns: u64,
    /// Executable bytes emitted by successful requests.
    pub generated_code_bytes: u64,
    /// Calls that entered generated machine code.
    pub compiled_entries: u64,
    /// Calls that resumed the interpreter.
    pub bailouts: u64,
    /// Native entries whose property guards matched.
    pub property_guard_hits: u64,
    /// Native entries rejected by a property guard.
    pub property_guard_misses: u64,
    /// Property-enabled entries that resumed the interpreter.
    pub property_bailouts: u64,
    /// Calls that entered a generated runtime-helper site.
    pub runtime_helper_entries: u64,
    /// Exceptions resolved through generated-frame metadata.
    pub exception_unwinds: u64,
    /// Catch/finally entries restored from generated-frame metadata.
    pub exception_handler_entries: u64,
    /// Generated loop slices that restored the interpreter for a budget or deadline poll.
    pub interrupt_deopts: u64,
    /// Generated entries that resumed the interpreter after a shape mismatch.
    pub shape_deopts: u64,
    /// Generated entries that resumed the interpreter after a type mismatch.
    pub type_deopts: u64,
    /// Generated entries that resumed the interpreter after an arithmetic guard failed.
    pub arithmetic_deopts: u64,
}

impl BaselineJitDiagnostics {
    /// Saturating aggregation used by multi-runtime performance reports.
    #[doc(hidden)]
    pub fn saturating_add_assign(&mut self, other: Self) {
        self.enabled |= other.enabled;
        self.compile_requests = self.compile_requests.saturating_add(other.compile_requests);
        self.successful_compilations = self
            .successful_compilations
            .saturating_add(other.successful_compilations);
        self.compile_rejections = self
            .compile_rejections
            .saturating_add(other.compile_rejections);
        self.total_compile_time_ns = self
            .total_compile_time_ns
            .saturating_add(other.total_compile_time_ns);
        self.generated_code_bytes = self
            .generated_code_bytes
            .saturating_add(other.generated_code_bytes);
        self.compiled_entries = self.compiled_entries.saturating_add(other.compiled_entries);
        self.bailouts = self.bailouts.saturating_add(other.bailouts);
        self.property_guard_hits = self
            .property_guard_hits
            .saturating_add(other.property_guard_hits);
        self.property_guard_misses = self
            .property_guard_misses
            .saturating_add(other.property_guard_misses);
        self.property_bailouts = self
            .property_bailouts
            .saturating_add(other.property_bailouts);
        self.runtime_helper_entries = self
            .runtime_helper_entries
            .saturating_add(other.runtime_helper_entries);
        self.exception_unwinds = self
            .exception_unwinds
            .saturating_add(other.exception_unwinds);
        self.exception_handler_entries = self
            .exception_handler_entries
            .saturating_add(other.exception_handler_entries);
        self.interrupt_deopts = self.interrupt_deopts.saturating_add(other.interrupt_deopts);
        self.shape_deopts = self.shape_deopts.saturating_add(other.shape_deopts);
        self.type_deopts = self.type_deopts.saturating_add(other.type_deopts);
        self.arithmetic_deopts = self
            .arithmetic_deopts
            .saturating_add(other.arithmetic_deopts);
    }
}

pub struct JsRuntime {
    // The provider must be dropped before `host_state` so it never traces a
    // freed host allocation during teardown.
    _host_roots_provider: RootProvider,
    context: Context,
    host_state: Rc<RefCell<HostState>>,
    module_loader: Rc<HttpModuleLoader>,
    sandbox: SandboxConfig,
}

/// Operation on the browser-owned page search session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindInPageAction {
    /// Start a new search with the supplied query.
    Start,
    /// Move to the following match, wrapping at the end.
    Next,
    /// Move to the preceding match, wrapping at the beginning.
    Previous,
    /// Return the current result after refreshing the document matches.
    Status,
    /// End the search and release its active selection.
    Stop,
}

impl FindInPageAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Status => "status",
            Self::Stop => "stop",
        }
    }
}

/// Current page-search result. The active ordinal is one-based, or zero for no match.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindInPageResult {
    /// Search text for the current session.
    pub query: String,
    /// Number of matches in the active document and its searchable subtrees.
    pub match_count: usize,
    /// One-based position of the active match, or zero if none exists.
    pub active_match_ordinal: usize,
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

/// A dynamically inserted (non-parser) script pending timer dispatch: the
/// script node, its `src`, its classified kind, its resolved base URL, and
/// the identity of the document that owns it.
type DynamicScriptInfo = (
    NodeHandle,
    String,
    ScriptKind,
    Option<crate::http::Url>,
    usize,
);

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

    pub(crate) fn sandbox_timeout(&self) -> Duration {
        self.sandbox.timeout
    }

    /// Attaches a best-effort reporter for errors observed by this runtime.
    /// The embedder retains ownership of the reporter and may flush it on exit.
    pub fn set_error_reporter(&mut self, reporter: Arc<ErrorReporter>, surface: ExecutionSurface) {
        let mut state = self.host_state.borrow_mut();
        state
            .http_client
            .set_error_reporter(Arc::clone(&reporter), surface);
        state.error_reporter = Some((reporter, surface));
    }

    pub(crate) fn error_reporter_destination(
        &self,
    ) -> Option<(Arc<ErrorReporter>, ExecutionSurface)> {
        self.host_state.borrow().error_reporter.clone()
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
        for (document_id, client_id) in host_state.borrow().web_lock_clients.clone() {
            register_web_lock_client(&host_state, document_id, client_id);
        }
        let module_loader = Rc::new(HttpModuleLoader {
            owner: Rc::downgrade(&host_state),
            ..HttpModuleLoader::default()
        });
        host_state.borrow_mut().module_loader = Some(Rc::downgrade(&module_loader));
        let context = Context::builder()
            .module_loader(module_loader.clone())
            .host_hooks(Rc::new(BrowserHostHooks))
            .build()?;
        context
            .realm()
            .host_defined_mut()
            .insert(ModuleDocumentId(document.identity()));
        context
            .realm()
            .host_defined_mut()
            .insert(realm_origin_snapshot(&host_state, document.identity())?);

        host_state.borrow_mut().main_realm = Some(context.realm().clone());

        let host_roots_provider =
            unsafe { RootProvider::register(std::ptr::NonNull::from(&*host_state.borrow())) };

        let mut runtime = Self {
            _host_roots_provider: host_roots_provider,
            context,
            host_state,
            module_loader,
            sandbox,
        };
        let bindings = register_host_bindings(&mut runtime.context, &runtime.host_state)?;
        {
            let _host = activate_host_state(Rc::clone(&runtime.host_state));
            evaluate_dom_bootstrap(&mut runtime.context, &runtime.host_state, &bindings)?;
        }
        // DOM bootstrap is runtime initialization rather than page code. Apply
        // the caller's budget only after it has completed so a deliberately
        // small limit (for example in a test or a short-lived page task) cannot
        // abort construction before the runtime is usable.
        runtime
            .context
            .runtime_limits_mut()
            .set_loop_iteration_limit(runtime.sandbox.max_loop_iterations);
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

    /// Searches the active page and moves the browser-owned active match.
    ///
    /// `query` is used only for [`FindInPageAction::Start`]. Other actions
    /// operate on the current search session. The result is refreshed against
    /// the current DOM so removed nodes are never returned as live matches.
    pub fn find_in_page(
        &mut self,
        action: FindInPageAction,
        query: &str,
    ) -> Result<FindInPageResult, FindInPageError> {
        let action = serde_json::to_string(action.as_str()).map_err(FindInPageError::Serialize)?;
        let query = serde_json::to_string(query).map_err(FindInPageError::Serialize)?;
        let script = format!("JSON.stringify(__omoikane_find_in_page({action}, {query}))");
        let value = self.eval(&script).map_err(FindInPageError::JavaScript)?;
        let payload = value
            .as_string()
            .ok_or(FindInPageError::MissingJsonString)?
            .to_std_string_escaped();
        serde_json::from_str(&payload).map_err(FindInPageError::Deserialize)
    }

    /// Creates (or returns) the Boa Realm used by an iframe's document.
    ///
    /// The DOM bootstrap is evaluated once in that Realm so wrappers, global
    /// constructors, and Promise jobs cannot accidentally share the parent's
    /// JavaScript global.  Same-origin frames receive a controlled reference
    /// to the top-level global for legacy `parent`/`top` access; opaque or
    /// sandboxed frames receive a null-prototype object instead.
    fn ensure_iframe_realm(&mut self, iframe_id: usize, document_id: usize) -> JsResult<Realm> {
        let host_state = Rc::clone(&self.host_state);
        self.with_active_host(|context| {
            ensure_iframe_realm(context, &host_state, iframe_id, document_id)
        })
    }

    fn ensure_auxiliary_realm(&mut self, id: u64) -> JsResult<Realm> {
        let host_state = Rc::clone(&self.host_state);
        self.with_active_host(|context| ensure_auxiliary_realm(context, &host_state, id))
    }

    /// Evaluates one iframe inline script in its child Realm and restores the
    /// caller's Realm even when evaluation or its microtask checkpoint fails.
    fn eval_iframe_script(
        &mut self,
        iframe_id: usize,
        document_id: usize,
        script_id: usize,
        source: &str,
    ) -> JsResult<()> {
        let realm = self.ensure_iframe_realm(iframe_id, document_id)?;
        let old_realm = self.context.enter_realm(realm);
        let script_node = self.host_state.borrow().get_node(script_id);
        self.host_state.borrow_mut().write_insertion_ref = script_node;
        let result = (|| {
            self.eval(&format!("__omoikane_set_current_script({script_id})"))?;
            self.eval(source)?;
            self.run_jobs()
        })();
        let _ = self.eval("__omoikane_set_current_script(null)");
        self.host_state.borrow_mut().write_insertion_ref = None;
        self.context.enter_realm(old_realm);
        result.map(|_| ())
    }

    /// Returns whether `realm` still belongs to the live iframe Document that
    /// created it. Navigation removes the `IframeDocument` entry, making any
    /// queued callback from the previous generation inert.
    fn iframe_realm_is_live(&self, document_id: usize, realm: &Realm) -> bool {
        let state = self.host_state.borrow();
        state.iframe_documents.values().any(|entry| {
            entry.document.identity() == document_id
                && entry.realm.as_ref().is_some_and(|active| active == realm)
        }) || state.auxiliary_contexts.values().any(|entry| {
            entry.document.identity() == document_id
                && entry.realm.as_ref().is_some_and(|active| active == realm)
        })
    }

    /// Returns the live child Realm for `document_id`, creating it when a
    /// resource task reaches a child Document before its first inline script.
    /// The top-level Document deliberately returns `None` because its current
    /// Context Realm is already the correct execution environment.
    fn realm_for_document(&mut self, document_id: usize) -> JsResult<Option<Realm>> {
        if document_id == self.document().identity() {
            return Ok(None);
        }
        let auxiliary_id = self
            .host_state
            .borrow()
            .auxiliary_contexts
            .iter()
            .find_map(|(id, entry)| (entry.document.identity() == document_id).then_some(*id));
        if let Some(id) = auxiliary_id {
            return self.ensure_auxiliary_realm(id).map(Some);
        }
        let iframe_id = self
            .host_state
            .borrow()
            .iframe_documents
            .iter()
            .find(|(_, entry)| entry.document.identity() == document_id)
            .map(|(iframe_id, _)| *iframe_id);
        let Some(iframe_id) = iframe_id else {
            return Ok(None);
        };
        let realm = self.ensure_iframe_realm(iframe_id, document_id)?;
        if self.iframe_realm_is_live(document_id, &realm) {
            Ok(Some(realm))
        } else {
            Ok(None)
        }
    }

    /// Looks up a child Realm without creating one. Resource events attached
    /// through an existing caller Realm must stay in that caller when the
    /// target Document has never executed child page code (notably an iframe
    /// moved into an inert sub-document).
    fn existing_realm_for_document(&self, document_id: usize) -> Option<Realm> {
        if document_id == self.document().identity() {
            return None;
        }
        self.host_state
            .borrow()
            .iframe_documents
            .values()
            .find(|entry| entry.document.identity() == document_id)
            .and_then(|entry| entry.realm.clone())
    }

    /// Executes a classic script in the Realm that owns its Document. This is
    /// used by dynamically inserted `<script src>` elements in child frames;
    /// evaluating the source through the top Context would install globals and
    /// callbacks on the wrong browsing context.
    fn eval_script_in_document_realm(
        &mut self,
        document_id: usize,
        script_id: usize,
        source: &str,
    ) -> JsResult<()> {
        let realm = self.realm_for_document(document_id)?;
        if document_id != self.document().identity() && realm.is_none() {
            return Err(JsNativeError::reference()
                .with_message("script document Realm is no longer live")
                .into());
        }
        let old_realm = realm.map(|realm| self.context.enter_realm(realm));
        let script_node = self.host_state.borrow().get_node(script_id);
        self.host_state.borrow_mut().write_insertion_ref = script_node;
        let result = (|| {
            self.eval(&format!("__omoikane_set_current_script({script_id})"))?;
            self.eval(source)?;
            self.run_jobs()
        })();
        let _ = self.eval("__omoikane_set_current_script(null)");
        self.host_state.borrow_mut().write_insertion_ref = None;
        if let Some(old_realm) = old_realm {
            self.context.enter_realm(old_realm);
        }
        result.map(|_| ())
    }

    /// Evaluates a module while the owning Document Realm is active. Module
    /// loader bookkeeping remains keyed by the native Document identity, so
    /// imports and CSP violations stay scoped to the child generation.
    fn eval_module_in_document_realm_timed(
        &mut self,
        document_id: usize,
        script_id: usize,
        source: &str,
        url: &str,
        document: NodeHandle,
    ) -> (
        Result<JsValue, JsEvaluationError>,
        std::time::Duration,
        std::time::Duration,
    ) {
        let realm = match self.realm_for_document(document_id) {
            Ok(realm) => realm,
            Err(error) => {
                return (
                    Err(JsEvaluationError::JavaScript(error)),
                    std::time::Duration::ZERO,
                    std::time::Duration::ZERO,
                );
            }
        };
        if document_id != self.document().identity() && realm.is_none() {
            return (
                Err(JsEvaluationError::DocumentRealmRetired),
                std::time::Duration::ZERO,
                std::time::Duration::ZERO,
            );
        }
        let old_realm = realm.map(|realm| self.context.enter_realm(realm));
        let script_node = self.host_state.borrow().get_node(script_id);
        self.host_state.borrow_mut().write_insertion_ref = script_node;
        let _ = self.eval(&format!("__omoikane_set_current_script({script_id})"));
        let result = self.eval_module_timed(source, url, document);
        let _ = self.eval("__omoikane_set_current_script(null)");
        self.host_state.borrow_mut().write_insertion_ref = None;
        if let Some(old_realm) = old_realm {
            self.context.enter_realm(old_realm);
        }
        result
    }

    /// Evaluates a host-generated dispatch script in the Realm that owns a
    /// Document. The top-level Document uses the currently active Realm;
    /// iframe Documents use their cached child Realm. This keeps resource and
    /// load/error listeners attached by a child from being looked up through
    /// the parent's wrapper cache.
    fn eval_in_document_realm(&mut self, document_id: usize, source: &str) -> JsResult<()> {
        let realm = self.existing_realm_for_document(document_id);
        let Some(realm) = realm else {
            // A child Document that has not run page code yet has no child
            // Realm. Its DOM wrappers can still be owned by the caller Realm
            // (for example when a parent script moves an iframe into that
            // document), so keep the historical caller-Realm dispatch path.
            self.eval(source)?;
            self.run_jobs()?;
            return Ok(());
        };
        if !self.iframe_realm_is_live(document_id, &realm) {
            return Ok(());
        }
        let old_realm = self.context.enter_realm(realm);
        let result = (|| {
            self.eval(source)?;
            self.run_jobs()
        })();
        self.context.enter_realm(old_realm);
        result
    }

    /// Returns the stable native identity of the explicitly focused element.
    ///
    /// `Document.activeElement` has a body/document-element fallback even when
    /// no element owns focus. The accessibility tree needs to distinguish that
    /// fallback from an actual focused control, so it reads the bootstrap's
    /// per-document focus slot directly.
    fn accessibility_node_identity(value: f64) -> Option<usize> {
        const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
        let maximum = MAX_SAFE_INTEGER.min(usize::MAX as f64);
        if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= maximum {
            Some(value as usize)
        } else {
            None
        }
    }

    pub(crate) fn accessibility_focused_node_identity(&mut self) -> Option<usize> {
        self.eval(
            "(() => { document.activeElement; return \
             document.hasFocus() && document.__focusedElementId != null \
               ? document.__focusedElementId : null; })()",
        )
        .ok()
        .and_then(|value| value.as_number())
        .and_then(Self::accessibility_node_identity)
    }

    /// Retains a CDP remote object in the host-side registry.
    pub(crate) fn retain_remote_object(&mut self, object_id: String, value: JsValue) {
        self.host_state
            .borrow_mut()
            .remote_objects
            .insert(object_id, value);
    }

    /// Returns a clone of a CDP remote object from the host-side registry.
    pub(crate) fn remote_object(&self, object_id: &str) -> Option<JsValue> {
        self.host_state
            .borrow()
            .remote_objects
            .get(object_id)
            .cloned()
    }

    /// Releases a CDP remote object. Unknown handles are intentionally
    /// idempotent, matching the CDP release operation's lifecycle semantics.
    pub(crate) fn release_remote_object(&mut self, object_id: &str) -> bool {
        self.host_state
            .borrow_mut()
            .remote_objects
            .remove(object_id)
            .is_some()
    }

    /// Resolves a Runtime remote object when it is a live DOM Node wrapper.
    pub(crate) fn node_for_remote_object_id(&mut self, object_id: &str) -> Option<NodeHandle> {
        let value = self.remote_object(object_id)?;
        let resolver = self
            .host_state
            .borrow()
            .canonical_node_identity_resolver
            .clone()?;
        let identity = self
            .with_active_host(|context| {
                let callable = resolver.as_callable().ok_or_else(|| {
                    JsError::from(
                        JsNativeError::typ()
                            .with_message("canonical node identity resolver is not callable"),
                    )
                })?;
                callable.call(&JsValue::undefined(), &[value], context)
            })
            .ok()?
            .as_number()
            .and_then(Self::accessibility_node_identity)?;
        self.host_state.borrow().get_node(identity)
    }

    /// Captures live form and disclosure state for accessibility consumers.
    ///
    /// Option selectedness is mirrored into the native DOM by a bootstrap-only
    /// binding, and `<details open>` is a reflected content attribute. Keeping
    /// this traversal native avoids exposing closed-shadow node identities or
    /// wrappers to page JavaScript.
    pub(crate) fn accessibility_snapshot_state(&mut self) -> AccessibilitySnapshotState {
        fn collect_options(node: &NodeHandle, options: &mut Vec<NodeHandle>) {
            for child in node.child_nodes() {
                if child.tag_name().as_deref() == Some("option") {
                    options.push(child);
                } else {
                    collect_options(&child, options);
                }
            }
        }

        fn collect(node: &NodeHandle, snapshot: &mut AccessibilitySnapshotState) {
            if node.tag_name().as_deref() == Some("select") {
                let mut options = Vec::new();
                collect_options(node, &mut options);
                let mut selected = options
                    .iter()
                    .filter(|option| option.selected())
                    .collect::<Vec<_>>();
                if node.get_attribute("multiple").is_none() && selected.is_empty() {
                    selected = options
                        .iter()
                        .find(|option| !is_actually_disabled(option))
                        .into_iter()
                        .collect();
                } else if node.get_attribute("multiple").is_none() && selected.len() > 1 {
                    selected = selected.split_off(selected.len() - 1);
                }
                snapshot
                    .selected_option_identities
                    .extend(selected.into_iter().map(|option| option.identity()));
            } else if node.tag_name().as_deref() == Some("details")
                && node.get_attribute("open").is_some()
            {
                snapshot.open_details_identities.insert(node.identity());
            }
            if let Some(root) = node.shadow_root() {
                collect(&root, snapshot);
            }
            for child in node.child_nodes() {
                collect(&child, snapshot);
            }
        }

        let mut snapshot = AccessibilitySnapshotState::default();
        collect(&self.document(), &mut snapshot);
        snapshot
    }

    /// Resolves whether an element participates in the rendered accessibility
    /// tree, including stylesheet-driven ancestor `display:none` and inherited
    /// `visibility` state. HTML/ARIA hidden attributes are handled by the
    /// accessibility builder so it can report the matching ignored reason.
    pub(crate) fn accessibility_render_state(
        &mut self,
        node: &NodeHandle,
    ) -> AccessibilityRenderState {
        if node.node_type() != NodeType::Element {
            return AccessibilityRenderState::Rendered;
        }
        let Some(document) = document_root_for_node(node) else {
            return AccessibilityRenderState::NotRendered;
        };
        let document_id = document.identity();
        let mut state = self.host_state.borrow_mut();
        state.ensure_style_resolver(&document);

        let mut current = Some(node.clone());
        while let Some(element) = current {
            let Some(resolver) = state
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut())
            else {
                return AccessibilityRenderState::NotRendered;
            };
            if matches!(
                resolver.computed_property(&element, "display"),
                Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("none")
            ) {
                return AccessibilityRenderState::NotRendered;
            }
            if element.identity() != node.identity()
                && matches!(
                    resolver.computed_property(&element, "content-visibility"),
                    Some(ComputedValue::Keyword(value))
                        if value.eq_ignore_ascii_case("hidden")
                )
            {
                return AccessibilityRenderState::NotRendered;
            }
            current = element.assigned_slot().or_else(|| {
                element.parent_node().and_then(|parent| {
                    if parent.node_type() == NodeType::Element {
                        Some(parent)
                    } else {
                        parent.shadow_host()
                    }
                })
            });
        }

        let Some(resolver) = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
        else {
            return AccessibilityRenderState::NotRendered;
        };
        match resolver.computed_property(node, "visibility") {
            Some(ComputedValue::Keyword(value))
                if value.eq_ignore_ascii_case("hidden")
                    || value.eq_ignore_ascii_case("collapse") =>
            {
                AccessibilityRenderState::NotVisible
            }
            _ => AccessibilityRenderState::Rendered,
        }
    }

    /// Cancels every worker when this global is replaced by navigation,
    /// reload, or disconnect. Dropping the map breaks the owner/worker Rc cycle
    /// after queued task ids have become inert.
    pub(crate) fn terminate_workers(&mut self) {
        let workers = std::mem::take(&mut self.host_state.borrow_mut().workers);
        for entry in workers.values() {
            let mut worker = entry.borrow_mut();
            if let Some(owner_object) = worker.owner_object.as_ref()
                && let Some(owner_object) = owner_object.as_object()
            {
                let _ = owner_object.set(
                    js_string!("__terminated"),
                    JsValue::from(true),
                    true,
                    &mut self.context,
                );
            }
            worker.terminated = true;
            worker.outgoing.clear();
            worker.runtime.host_state.borrow_mut().worker_terminated = true;
        }
        terminate_shared_worker_connections(&self.host_state);
        terminate_worklet_runtime(&self.host_state);
        self.host_state.borrow_mut().worker_owner_objects.clear();
    }

    fn advance_worker_clocks(&mut self, elapsed_ms: u64) {
        let workers: Vec<_> = self.host_state.borrow().workers.values().cloned().collect();
        for entry in workers {
            let worker = entry.borrow_mut();
            if worker.terminated || worker.runtime.host_state.borrow().worker_terminated {
                continue;
            }
            worker
                .runtime
                .host_state
                .borrow_mut()
                .event_loop
                .advance(elapsed_ms);
        }
    }

    fn run_worker_background_tasks(&mut self) {
        let worker_ids: Vec<_> = self.host_state.borrow().workers.keys().copied().collect();
        for worker_id in worker_ids {
            let Some(entry) = self.host_state.borrow_mut().workers.remove(&worker_id) else {
                continue;
            };
            let (owner_state, owner_realm, result, errors, terminated) = {
                let mut worker = entry.borrow_mut();
                if worker.terminated || worker.runtime.host_state.borrow().worker_terminated {
                    self.host_state
                        .borrow_mut()
                        .worker_owner_objects
                        .remove(&worker_id);
                    continue;
                }
                let result = worker.runtime.run_until_idle();
                let errors = worker.runtime.take_task_errors();
                let worker_state = worker.runtime.host_state.borrow();
                let owner_realm = worker_state.worker_owner_realm.clone();
                let terminated = worker_state.worker_terminated;
                (
                    Rc::clone(&worker.owner_state),
                    owner_realm,
                    result,
                    errors,
                    terminated,
                )
            };
            if !terminated {
                self.host_state
                    .borrow_mut()
                    .workers
                    .insert(worker_id, Rc::clone(&entry));
            } else {
                self.host_state
                    .borrow_mut()
                    .worker_owner_objects
                    .remove(&worker_id);
            }
            // A worker task failure is reported to the owner as an error event,
            // but must not abort the owner's event-loop pump. `run_timer_payload`
            // records callback failures in the worker runtime, while a rejected
            // microtask can be returned directly by `run_until_idle`.
            if let Err(error) = result {
                report_safe_worker_or_module_failure(
                    owner_state.borrow().error_reporter.clone(),
                    ErrorCategory::Worker,
                    "WORKER_RUNTIME_FAILED",
                    "execute",
                );
                owner_state.borrow_mut().event_loop.enqueue_worker_error(
                    worker_id,
                    None,
                    owner_realm.clone(),
                    error.to_string(),
                );
            }
            for error in errors {
                report_safe_worker_or_module_failure(
                    owner_state.borrow().error_reporter.clone(),
                    ErrorCategory::Worker,
                    "WORKER_RUNTIME_FAILED",
                    "execute",
                );
                owner_state.borrow_mut().event_loop.enqueue_worker_error(
                    worker_id,
                    None,
                    owner_realm.clone(),
                    error,
                );
            }
        }
    }

    /// Returns the virtual frame-scheduler timestamp used for rendering.
    pub(crate) fn rendering_time_ms(&self) -> u64 {
        self.host_state.borrow().event_loop.rendering_time_ms() as u64
    }

    /// Returns monotonic snapshots for the style, layout, and paint cache
    /// layers. A caller that retains derived output can compare this value
    /// before reusing it without reaching into the runtime's host state.
    pub fn render_generations(&self) -> RenderGenerations {
        let state = self.host_state.borrow();
        RenderGenerations {
            style: state.style_generation,
            layout: state.layout_generation,
            paint: state.paint_generation,
        }
    }

    /// Paints the current document with the layout used by CSSOM and input.
    pub(crate) fn paint_current_document(
        &mut self,
    ) -> Result<crate::paint::Canvas, crate::paint::PaintError> {
        let timeline = Arc::clone(&self.host_state.borrow().image_timeline);
        let canvas = crate::layout::with_image_animation_timeline(timeline, || {
            self.paint_document_at_current_image_time()
        })?;
        self.host_state.borrow_mut().visible_image_playbacks = canvas.image_playbacks();
        Ok(canvas)
    }

    fn paint_document_at_current_image_time(
        &mut self,
    ) -> Result<crate::paint::Canvas, crate::paint::PaintError> {
        self.eval("__omoikane_flush_stylesheets()")
            .map_err(|_| crate::paint::PaintError::InvalidImageBuffer)?;
        let mut state = self.host_state.borrow_mut();
        let start = Instant::now();
        state.ensure_adjusted_layout();
        let layout_time = start.elapsed();
        let document_id = state.document.identity();
        let visited_link_ids = state.visited_link_ids_for_paint(&state.document);
        let viewport = state.viewport;
        let base = crate::paint::stylesheet::extract_document_base_url(
            &state.document,
            state.base_url_for_document(document_id).as_ref(),
        );
        let image_site = state.location_href.parse::<crate::http::Url>().ok();
        let image_cookies = Arc::clone(&state.cookie_store);
        let animation_time = state.event_loop.rendering_time_ms() as u64;
        let state = &mut *state;
        let layout = state
            .adjusted_layout_cache
            .as_ref()
            .ok_or(crate::paint::PaintError::InvalidImageBuffer)?
            .root
            .clone();
        let snapshots = nested_rendering::collect_snapshots(state, &layout, 0)?;
        let entry = state
            .document_styles
            .get_mut(&document_id)
            .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
        let resolver = entry
            .resolver
            .as_mut()
            .ok_or(crate::paint::PaintError::InvalidImageBuffer)?;
        let start = Instant::now();
        let canvas =
            crate::layout::with_image_cookie_store(image_cookies, image_site, document_id, || {
                crate::layout::with_image_base_url(base, || {
                    crate::layout::with_image_animation_time(animation_time, || {
                        crate::paint::paint_layout_with_document_snapshots(
                            &layout,
                            resolver,
                            viewport,
                            crate::paint::text::load_text_fonts(),
                            Some(&entry.web_fonts),
                            visited_link_ids,
                            &snapshots,
                        )
                    })
                })
            });
        crate::paint::record_render_timings(&crate::paint::RenderTimings {
            layout: layout_time,
            paint: start.elapsed(),
            ..Default::default()
        });
        Ok(canvas)
    }

    /// Returns the topmost event-target element at viewport coordinates.
    /// The adjusted layout is shared with CSSOM geometry queries and rebuilt
    /// only when a tracked style, layout, paint, or scroll generation changes.
    pub(crate) fn hit_test(&mut self, x: f32, y: f32) -> Option<NodeHandle> {
        self.eval("__omoikane_flush_stylesheets()").ok()?;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let mut state = self.host_state.borrow_mut();
        state.ensure_adjusted_layout();
        let viewport = state.viewport;
        let document_id = state.document.identity();
        let state = &mut *state;
        let layout = &state.adjusted_layout_cache.as_ref()?.root;
        let target = {
            let resolver = state
                .document_styles
                .get_mut(&document_id)
                .and_then(|entry| entry.resolver.as_mut())?;
            crate::paint::hit_test_layout(layout, resolver, viewport, x, y)
        }?;
        let layout = layout.clone();
        let target = nested_rendering::hit_child(state, &layout, target, x, y, 0);
        Some(state.retarget_content_visibility_hit(target))
    }

    /// Converts embedder pointer coordinates to the target document's viewport
    /// and returns that document's scroll offset for MouseEvent page positions.
    pub(crate) fn input_position_for_node(
        &mut self,
        node: &NodeHandle,
        x: f64,
        y: f64,
    ) -> (f64, f64, f32, f32) {
        let mut state = self.host_state.borrow_mut();
        let document = owner_document_for_node(node).unwrap_or_else(|| state.document.clone());
        let origin = nested_rendering::document_origin(&mut state, &document);
        let scroll = state.window_scroll_for_document(document.identity());
        (x - origin.0, y - origin.1, scroll.0, scroll.1)
    }

    /// Sets the User-Agent exposed to scripts in this runtime.
    pub fn set_user_agent(&mut self, user_agent: impl Into<String>) {
        let user_agent = user_agent.into();
        self.host_state.borrow_mut().navigator_user_agent = user_agent.clone();
        if let Ok(quoted) = serde_json::to_string(&user_agent) {
            let _ = self.eval(&format!("globalThis.navigator.userAgent = {quoted};"));
        }
    }

    /// Clears hover when the pointer leaves the presentation surface.
    pub(crate) fn clear_pointer_hover(&mut self) {
        self.host_state
            .borrow_mut()
            .update_user_action_target("hover", None, false);
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
            let state = self.host_state.borrow();
            if state.viewport.width == width && state.viewport.height == height {
                return;
            }
        }
        {
            let mut state = self.host_state.borrow_mut();
            let previous_viewport = state.viewport;
            let previous_visual = state.visual_viewport;
            state.viewport = Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            };
            if state.visual_viewport_follows_layout {
                state.visual_viewport.width = width;
                state.visual_viewport.height = height;
            } else {
                state.visual_viewport.width = state.visual_viewport.width.min(width);
                state.visual_viewport.height = state.visual_viewport.height.min(height);
                state.visual_viewport.offset_left = state
                    .visual_viewport
                    .offset_left
                    .min((width - state.visual_viewport.width).max(0.0));
                state.visual_viewport.offset_top = state
                    .visual_viewport
                    .offset_top
                    .min((height - state.visual_viewport.height).max(0.0));
            }
            let document_id = state.document.identity();
            if previous_viewport.width != width || previous_viewport.height != height {
                state.queue_window_resize(document_id);
            }
            if previous_visual.width != state.visual_viewport.width
                || previous_visual.height != state.visual_viewport.height
                || previous_visual.scale != state.visual_viewport.scale
            {
                state.queue_visual_viewport_resize(document_id);
            }
            if previous_visual.offset_left != state.visual_viewport.offset_left
                || previous_visual.offset_top != state.visual_viewport.offset_top
            {
                state.queue_visual_viewport_scroll(document_id);
            }
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

    /// Updates the top-level visual viewport in CSS pixels.
    ///
    /// Embedders call this when pinch zoom or an on-screen keyboard changes the
    /// visible portion of the layout viewport. Width and height are clamped to
    /// the layout viewport, offsets are clamped to its remaining area, and an
    /// invalid scale is normalized to `1`. Observable `resize` and `scroll`
    /// events are coalesced until the next rendering opportunity.
    pub fn set_visual_viewport(
        &mut self,
        width: f32,
        height: f32,
        offset_left: f32,
        offset_top: f32,
        scale: f32,
    ) {
        let width = sanitize_viewport_dimension(width);
        let height = sanitize_viewport_dimension(height);
        let offset_left = sanitize_viewport_dimension(offset_left);
        let offset_top = sanitize_viewport_dimension(offset_top);
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let mut state = self.host_state.borrow_mut();
        let previous = state.visual_viewport;
        let width = width.min(state.viewport.width);
        let height = height.min(state.viewport.height);
        state.visual_viewport = VisualViewportState {
            width,
            height,
            offset_left: offset_left.min((state.viewport.width - width).max(0.0)),
            offset_top: offset_top.min((state.viewport.height - height).max(0.0)),
            scale,
        };
        state.visual_viewport_follows_layout = false;
        let document_id = state.document.identity();
        if previous.width != state.visual_viewport.width
            || previous.height != state.visual_viewport.height
            || previous.scale != state.visual_viewport.scale
        {
            state.queue_visual_viewport_resize(document_id);
        }
        if previous.offset_left != state.visual_viewport.offset_left
            || previous.offset_top != state.visual_viewport.offset_top
        {
            state.queue_visual_viewport_scroll(document_id);
        }
    }

    /// Sets the base URL used to resolve relative resource references such as
    /// `<iframe src="empty.html">`.
    ///
    /// [`execute_document_scripts`](Self::execute_document_scripts) sets this
    /// automatically from its `base_url` argument; call this directly when
    /// driving the runtime without running document scripts.
    pub fn set_base_url(&mut self, url: crate::http::Url) {
        self.host_state.borrow_mut().set_main_base_url(url);
    }

    /// Commits a same-Document URL change and refreshes its fragment target.
    /// The navigation owner calls this only after accepting the URL transition.
    pub fn commit_same_document_url(&mut self, url: &str) {
        self.commit_history_api_url(url);
        let mut state = self.host_state.borrow_mut();
        let document = state.document.clone();
        state.update_document_target(&document, url);
    }

    /// Updates the committed Document URL for history API changes without
    /// changing the fragment target. `pushState` and `replaceState` do not
    /// perform fragment navigation, even when their URL contains a new hash.
    pub(crate) fn commit_history_api_url(&mut self, url: &str) {
        let mut state = self.host_state.borrow_mut();
        let document = state.document.clone();
        state.location_href = url.to_owned();
        state
            .document_urls
            .insert(document.identity(), url.to_owned());
        if let Ok(base_url) = url.parse::<crate::http::Url>() {
            state.set_main_base_url(base_url);
        }
    }

    /// Uses the browsing session's Cookie store across this Document and its resources.
    pub(crate) fn set_shared_cookie_store(&mut self, store: Arc<Mutex<crate::http::CookieJar>>) {
        let mut state = self.host_state.borrow_mut();
        state
            .http_client
            .set_shared_cookie_store(Arc::clone(&store));
        state.cookie_store = store;
    }

    /// Sets the initial visibility before a new Document runs any page script.
    pub(crate) fn set_initial_visibility_hidden(&mut self, hidden: bool) {
        self.host_state.borrow_mut().page_hidden = hidden;
        let result = self.eval("__omoikane_sync_initial_visibility_entry()");
        self.record_error_from("initial visibility entry", result);
    }

    /// Updates this page and its iframe Documents, firing `visibilitychange`
    /// once on each live Document when the state actually changes.
    pub fn set_page_visibility(&mut self, hidden: bool) {
        let document_ids = {
            let mut state = self.host_state.borrow_mut();
            if state.page_hidden == hidden {
                return;
            }
            state.page_hidden = hidden;
            let mut ids = vec![state.document.identity()];
            ids.extend(
                state
                    .iframe_documents
                    .values()
                    .map(|entry| entry.document.identity()),
            );
            ids[1..].sort_unstable();
            ids
        };
        for document_id in document_ids {
            if !self
                .host_state
                .borrow()
                .document_urls
                .contains_key(&document_id)
            {
                continue;
            }
            let result = self.eval_in_document_realm(
                document_id,
                &format!("__omoikane_dispatch_visibilitychange({document_id})"),
            );
            self.record_error_from("visibilitychange", result);
        }
    }

    /// Installs the enforced CSP for the current Document before document
    /// scripts and style resolution run.  The policy is immutable for the
    /// lifetime of this browsing-context generation; a navigation constructs a
    /// fresh runtime and therefore cannot inherit the previous policy.
    pub(crate) fn install_csp_policy(&mut self, headers: &[String]) {
        let document = self.document();
        let base_url = self.host_state.borrow().location_href.clone();
        let policy = CspPolicy::from_headers_and_document(headers, &document, &base_url);
        self.host_state
            .borrow_mut()
            .document_csp
            .insert(document.identity(), policy);
    }

    fn sync_module_csp_violations(&mut self) {
        let violations = self.module_loader.take_csp_violations();
        if violations.is_empty() {
            return;
        }
        let mut state = self.host_state.borrow_mut();
        for (document_id, _directive, blocked_uri) in violations {
            // Module loading is the one CSP path that runs inside Boa's
            // ModuleLoader callback, so retain its blocked target here while
            // sharing the same per-document deduplication as native paths.
            if state.document_is_active(document_id)
                && let Some(document) = state.get_node(document_id)
            {
                state.record_csp_violation(&document, ResourceType::Script, blocked_uri);
            }
        }
    }

    /// Takes navigation requests queued by Location/History APIs in FIFO order.
    pub fn take_navigation_requests(&mut self) -> Vec<NavigationRequest> {
        self.host_state
            .borrow_mut()
            .navigation_requests
            .drain(..)
            .map(|(request, _)| request)
            .collect()
    }

    /// Takes browser-owned visit sources alongside queued navigation requests.
    pub(crate) fn take_navigation_requests_with_source(
        &mut self,
    ) -> Vec<(NavigationRequest, Option<VisitSource>)> {
        self.host_state
            .borrow_mut()
            .navigation_requests
            .drain(..)
            .collect()
    }

    /// Enables or disables Fullscreen API support for this presentation host.
    pub fn set_fullscreen_supported(&mut self, supported: bool) {
        let mut state = self.host_state.borrow_mut();
        state.fullscreen_supported = supported;
        if !supported {
            state.fully_exit_fullscreen(false);
        }
    }

    /// Controls whether the host accepts subsequent fullscreen transitions.
    ///
    /// Native frontends normally leave this enabled. Tests and embedders that
    /// cannot enter fullscreen can reject the request without exposing a
    /// private page-script hook.
    pub fn set_fullscreen_transition_allowed(&mut self, allowed: bool) {
        self.host_state.borrow_mut().fullscreen_transition_allowed = allowed;
    }

    /// Takes the most recent fullscreen window-state transition requested by
    /// page script or a browser close request.
    pub fn take_fullscreen_transition(&mut self) -> Option<FullscreenTransition> {
        self.host_state
            .borrow_mut()
            .pending_fullscreen_transition
            .take()
    }

    /// Fully exits fullscreen because the host ended the native presentation.
    pub fn exit_fullscreen_from_host(&mut self) -> JsResult<bool> {
        let target = self.eval("document.fullscreenElement")?;
        let exited = self.host_state.borrow_mut().fully_exit_fullscreen(false);
        if exited {
            self.call_function_with_value(
                "target => (target || document).dispatchEvent(\
                  new Event('fullscreenchange', { bubbles: true, composed: true }))",
                target,
            )?;
            self.run_jobs()?;
        }
        Ok(exited)
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
                JsError::from(JsNativeError::typ().with_message("CDP serializer is not callable"))
            })?;
            callable.call(&JsValue::undefined(), &[value], context)
        });
        // This helper is a synchronous execution boundary. If a serializer
        // getter/toJSON attempts to open a modal dialog, Boa rejects the
        // suspension; discard the corresponding host metadata on every exit.
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        result
    }

    /// Calls a JavaScript function with host-provided `this` and argument
    /// values.  CDP uses this boundary for remote object handles so page code
    /// never needs a global lookup to recover a retained object.
    pub(crate) fn call_function_with_arguments(
        &mut self,
        function_source: &str,
        this_value: JsValue,
        arguments: Vec<JsValue>,
    ) -> JsResult<JsValue> {
        let result = self.with_active_host(|context| {
            let function_source = format!("({function_source})");
            let function = context.eval(Source::from_bytes(&function_source))?;
            let callable = function.as_callable().ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ().with_message("CDP function declaration is not callable"),
                )
            })?;
            callable.call(&this_value, &arguments, context)
        });
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        result
    }

    /// Returns the console log buffer captured from `console.log`.
    pub fn console_logs(&self) -> Vec<String> {
        self.host_state.borrow().console_logs.clone()
    }

    /// Returns runtime-wide baseline-JIT counters for performance-gate tooling.
    #[cfg(feature = "baseline-jit")]
    #[doc(hidden)]
    pub fn baseline_jit_diagnostics(&self) -> BaselineJitDiagnostics {
        let diagnostics = self.context.arithmetic_jit_diagnostics();
        let exceptions = self.context.jit_exception_diagnostics();
        BaselineJitDiagnostics {
            enabled: true,
            compile_requests: diagnostics.compile_requests,
            successful_compilations: diagnostics.successful_compilations,
            compile_rejections: diagnostics.compile_rejections,
            total_compile_time_ns: diagnostics.total_compile_time_ns,
            generated_code_bytes: diagnostics.generated_code_bytes,
            compiled_entries: diagnostics.compiled_entries,
            bailouts: diagnostics.bailouts,
            property_guard_hits: diagnostics.property_guard_hits,
            property_guard_misses: diagnostics.property_guard_misses,
            property_bailouts: diagnostics.property_bailouts,
            runtime_helper_entries: exceptions.generated_entries,
            exception_unwinds: exceptions.exception_unwinds,
            exception_handler_entries: exceptions.handler_entries,
            interrupt_deopts: diagnostics.interrupt_deopts,
            shape_deopts: diagnostics.shape_deopts,
            type_deopts: diagnostics.type_deopts,
            arithmetic_deopts: diagnostics.arithmetic_deopts,
        }
    }

    /// Returns an owned snapshot of generated code, stack maps and deoptimization metadata.
    #[cfg(feature = "baseline-jit")]
    #[doc(hidden)]
    pub fn baseline_jit_debug_snapshot(&self) -> String {
        self.context.jit_debug_snapshot()
    }

    /// Selects generated entry or interpreter execution for differential verification.
    #[cfg(feature = "baseline-jit")]
    #[doc(hidden)]
    pub fn set_baseline_jit_enabled(&mut self, enabled: bool) {
        self.context.set_baseline_jit_enabled(enabled);
    }

    /// Evaluates JavaScript source code.
    ///
    /// Script errors are returned as `JsError`.
    ///
    /// The VM enforces the configured loop limit and wall-clock deadline in
    /// interpreter and generated execution. Native calls are checked when they
    /// return; a blocking host function is not preempted. Use [`Self::eval_async`]
    /// when host calls need to suspend or the owning page task may be cancelled.
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
        let deadline = execution_deadline(self.sandbox.timeout);
        let host_state = Rc::clone(&self.host_state);
        let future = ActiveHostFuture {
            future: Box::pin(async move {
                let mut context = self.context.enter_runtime_deadline(deadline);
                let script = Script::parse(Source::from_bytes(&source), None, &mut context)?;
                script.evaluate_async(&mut context).await
            }),
            host_state,
        };
        TimedJsFuture::new(future, deadline)
    }

    fn eval_async_for_document<'a>(
        &'a mut self,
        source: &str,
        document_id: usize,
    ) -> impl Future<Output = JsResult<JsValue>> + 'a {
        let host_state = Rc::clone(&self.host_state);
        ActiveDocumentFuture {
            future: Box::pin(self.eval_async(source)),
            host_state,
            document_id,
        }
    }

    fn run_jobs_for_document(&mut self, document_id: Option<usize>) -> JsResult<()> {
        let _document =
            document_id.map(|document_id| activate_module_document(&self.host_state, document_id));
        self.run_jobs()
    }

    /// Evaluates one module with asynchronous jobs and suspendable host calls.
    async fn eval_module_async(
        &mut self,
        source: &str,
        url: &str,
        document: NodeHandle,
    ) -> JsResult<JsValue> {
        let policy = self.host_state.borrow().csp_policy_for_document(&document);
        self.module_loader
            .set_csp_policy_for_module_graph(document.identity(), url, policy);
        let source = source.to_owned();
        let url = url.to_owned();
        let document_id = document.identity();
        let deadline = execution_deadline(self.sandbox.timeout);
        let host_state = Rc::clone(&self.host_state);
        let module_host_state = Rc::clone(&host_state);
        let future = ActiveHostFuture {
            future: Box::pin(async move {
                let mut context = self.context.enter_runtime_deadline(deadline);
                let module = Module::parse(
                    Source::from_reader(source.as_bytes(), Some(Path::new(&url))),
                    None,
                    &mut context,
                )?;
                let _module_document = activate_module_document(&module_host_state, document_id);
                module.load_link_evaluate_async(&mut context).await
            }),
            host_state: Rc::clone(&host_state),
        };
        let document_future = ActiveDocumentFuture {
            future: Box::pin(future),
            host_state,
            document_id,
        };
        TimedJsFuture::new(document_future, deadline).await
    }

    /// Collects document scripts in parser/defer order and moves the runtime
    /// into an owned page task.
    pub fn into_document_page_task(
        mut self,
        generation: u64,
        base_url: Option<crate::http::Url>,
    ) -> OwnedPageTask {
        if let Some(base) = &base_url {
            self.host_state.borrow_mut().set_main_base_url(base.clone());
        }
        if self
            .eval("__omoikane_install_window_named_properties()")
            .is_err()
        {
            self.record_document_script_failure("DOCUMENT_SCRIPT_INITIALIZATION_FAILED");
        }
        if self.eval("document.__readyState = 'loading'").is_err() {
            self.record_document_script_failure("DOCUMENT_SCRIPT_INITIALIZATION_FAILED");
        }
        let _ = self.wire_inline_event_handlers();
        let scripts = collect_script_elements(&self.document());
        let mut immediate = Vec::new();
        let mut deferred = Vec::new();
        for (script_index, script) in scripts.iter().enumerate() {
            let attrs = script.attributes().unwrap_or_default();
            let is_module = attrs
                .get("type")
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("module"));
            if !is_module
                && !is_executable_classic_script_type(attrs.get("type").map(String::as_str))
            {
                continue;
            }
            let src = attrs.get("src").cloned();
            let has_src = src.is_some();
            let policy = self.host_state.borrow().csp_policy_for_node(script);
            if let Some(src_url) = src.as_deref() {
                if !policy.allows_reference(ResourceType::Script, src_url) {
                    self.host_state.borrow_mut().record_csp_violation_for_node(
                        script,
                        ResourceType::Script,
                        src_url,
                    );
                    continue;
                }
            } else if !policy.allows_inline(ResourceType::Script) {
                self.host_state.borrow_mut().record_csp_violation_for_node(
                    script,
                    ResourceType::Script,
                    "inline",
                );
                continue;
            }
            let (source, label) = if let Some(src) = src {
                let fetched = {
                    let mut state = self.host_state.borrow_mut();
                    fetch_script_resource_with_client(
                        &src,
                        base_url.as_ref(),
                        &mut state.http_client,
                    )
                };
                let (effective_url, source, redirect_count) = match fetched {
                    Some((effective_url, source, redirect_count)) => {
                        (Some(effective_url), source, redirect_count)
                    }
                    None => {
                        let message = format!("failed to fetch script: {src}");
                        let quoted = serde_json::to_string(&message)
                            .expect("JavaScript error messages must serialize as JSON strings");
                        (None, format!("throw new Error({quoted})"), 0)
                    }
                };
                if let Some(effective_url) = effective_url
                    && !policy.allows_reference_after_redirects(
                        ResourceType::Script,
                        &effective_url,
                        redirect_count,
                    )
                {
                    self.host_state.borrow_mut().record_csp_violation_for_node(
                        script,
                        ResourceType::Script,
                        effective_url,
                    );
                    continue;
                }
                (source, src)
            } else {
                (
                    collect_text_content(script),
                    format!("inline-script-{}", script_index + 1),
                )
            };
            if source.trim().is_empty() {
                continue;
            }
            let script_node_id = Some(script.identity());
            let task_source = if is_module {
                PageTaskSource::Module {
                    source,
                    url: module_script_url(&label, base_url.as_ref(), !has_src),
                    script_node_id,
                }
            } else {
                PageTaskSource::Classic {
                    source,
                    label,
                    script_node_id,
                }
            };
            if is_module || (attrs.contains_key("defer") && has_src) {
                deferred.push(task_source);
            } else {
                immediate.push(task_source);
            }
        }
        immediate.extend(deferred);
        immediate.push(PageTaskSource::Classic {
            source: DOM_CONTENT_LOADED_SCRIPT.to_string(),
            label: "DOMContentLoaded".to_string(),
            script_node_id: None,
        });
        immediate.push(PageTaskSource::Classic {
            source: LOAD_SCRIPT.to_string(),
            label: "load".to_string(),
            script_node_id: None,
        });
        self.into_page_task(generation, immediate)
    }

    fn complete_page_task_with_error(
        mut self,
        generation: u64,
        error: PageTaskError,
    ) -> CompletedPageTask {
        let _ = self.eval("__omoikane_set_current_script(null)");
        self.host_state.borrow_mut().write_insertion_ref = None;
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        let cleanup_result = self.clear_posted_message_values();
        self.record_error_from("posted message cleanup", cleanup_result);
        CompletedPageTask {
            runtime: self,
            generation,
            result: Err(error),
        }
    }

    /// Moves this runtime into a FIFO page-script task that can outlive a
    /// single host pump iteration without storing a future that borrows `self`.
    pub fn into_page_task(
        mut self,
        generation: u64,
        sources: Vec<PageTaskSource>,
    ) -> OwnedPageTask {
        let controller = self.javascript_dialog_controller();
        let cancelled = Rc::new(Cell::new(false));
        let task_cancelled = Rc::clone(&cancelled);
        let future = Box::pin(async move {
            let mut errors = Vec::new();
            for source in sources {
                if task_cancelled.get() {
                    return self
                        .complete_page_task_with_error(generation, PageTaskError::Cancelled);
                }
                let (source, label, script_node_id, module_url) = match source {
                    PageTaskSource::Classic {
                        source,
                        label,
                        script_node_id,
                    } => (source, label, script_node_id, None),
                    PageTaskSource::Module {
                        source,
                        url,
                        script_node_id,
                    } => (source, url.clone(), script_node_id, Some(url)),
                };
                if let Some(node) =
                    script_node_id.and_then(|id| self.host_state.borrow().get_node(id))
                    && (module_url.is_some() || node.get_attribute("defer").is_some())
                {
                    if let Err(error) = self.run_written_scripts_before(&node) {
                        self.record_document_script_failure("DOCUMENT_SCRIPT_EVALUATION_FAILED");
                        errors.push(format!("[written scripts] {error}"));
                    }
                }
                if label == "DOMContentLoaded" && script_node_id.is_none() {
                    if let Err(error) = self.run_written_scripts(true) {
                        self.record_document_script_failure("DOCUMENT_SCRIPT_EVALUATION_FAILED");
                        errors.push(format!("[written scripts] {error}"));
                    }
                }
                if let Some(node_id) = script_node_id {
                    let node = self.host_state.borrow().get_node(node_id);
                    self.host_state.borrow_mut().write_insertion_ref = node;
                    let _ = self.eval(&format!("__omoikane_set_current_script({node_id})"));
                }
                let evaluation_result = {
                    let mut evaluation: Pin<Box<dyn Future<Output = JsResult<JsValue>> + '_>> =
                        if let Some(url) = module_url.as_deref() {
                            let module_document = script_node_id
                                .and_then(|node_id| self.host_state.borrow().get_node(node_id))
                                .and_then(|node| document_root_for_node(&node))
                                .unwrap_or_else(|| self.document());
                            Box::pin(self.eval_module_async(&source, url, module_document))
                        } else {
                            Box::pin(self.eval_async(&source))
                        };
                    std::future::poll_fn(|context| {
                        if task_cancelled.get() {
                            Poll::Ready(None)
                        } else {
                            evaluation.as_mut().poll(context).map(Some)
                        }
                    })
                    .await
                };
                if script_node_id.is_some() {
                    let _ = self.eval("__omoikane_set_current_script(null)");
                    self.host_state.borrow_mut().write_insertion_ref = None;
                }
                let Some(evaluation_result) = evaluation_result else {
                    return self
                        .complete_page_task_with_error(generation, PageTaskError::Cancelled);
                };
                if let Err(error) = evaluation_result {
                    if module_url.is_some() {
                        self.record_module_failure("DOCUMENT_MODULE_EVALUATION_FAILED", "execute");
                    } else {
                        self.record_document_script_failure("DOCUMENT_SCRIPT_EVALUATION_FAILED");
                    }
                    if is_wall_clock_timeout(&error) {
                        return self
                            .complete_page_task_with_error(generation, PageTaskError::TimedOut);
                    }
                    errors.push(format!("[script: {label}] {error}"));
                }
                if let Err(error) = self.run_jobs() {
                    self.record_document_script_failure("DOCUMENT_SCRIPT_JOBS_FAILED");
                    errors.push(format!("[script jobs: {label}] {error}"));
                }
            }
            self.sync_module_csp_violations();
            let page_work_result = {
                let mut page_work = Box::pin(async {
                    self.run_until_idle_async().await?;
                    self.run_animation_frame_async(0).await.map(|_| ())
                });
                std::future::poll_fn(|context| {
                    if task_cancelled.get() {
                        Poll::Ready(None)
                    } else {
                        page_work.as_mut().poll(context).map(Some)
                    }
                })
                .await
            };
            let Some(page_work_result) = page_work_result else {
                return self.complete_page_task_with_error(generation, PageTaskError::Cancelled);
            };
            if let Err(error) = page_work_result {
                if is_wall_clock_timeout(&error) {
                    return self.complete_page_task_with_error(generation, PageTaskError::TimedOut);
                }
                errors.push(format!("[page callbacks] {error}"));
            }
            CompletedPageTask {
                runtime: self,
                generation,
                result: Ok(errors),
            }
        });
        OwnedPageTask {
            future,
            controller,
            cancelled,
            generation,
        }
    }

    /// Evaluates JavaScript source code and retains any Boa error for the embedder.
    ///
    /// This does not catch Rust panics; it only returns JS-level errors.
    pub fn eval_safe(&mut self, source: &str) -> Result<JsValue, JsEvaluationError> {
        self.eval(source).map_err(JsEvaluationError::JavaScript)
    }

    fn eval_safe_timed(
        &mut self,
        source: &str,
    ) -> (
        Result<JsValue, JsEvaluationError>,
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
                        Err(JsEvaluationError::JavaScript(error)),
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
                    Err(JsEvaluationError::JavaScript(error)),
                    parse_elapsed,
                    compile_start.elapsed(),
                    std::time::Duration::ZERO,
                );
            }
            let compile_elapsed = compile_start.elapsed();

            let execute_start = std::time::Instant::now();
            let result = script
                .evaluate(context)
                .map_err(JsEvaluationError::JavaScript);
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
        document: NodeHandle,
    ) -> (
        Result<JsValue, JsEvaluationError>,
        std::time::Duration,
        std::time::Duration,
    ) {
        let policy = self.host_state.borrow().csp_policy_for_document(&document);
        self.module_loader
            .set_csp_policy_for_module_graph(document.identity(), url, policy);
        let parse_start = std::time::Instant::now();
        let module = match Module::parse(
            Source::from_reader(source.as_bytes(), Some(Path::new(url))),
            None,
            &mut self.context,
        ) {
            Ok(module) => module,
            Err(error) => {
                return (
                    Err(JsEvaluationError::JavaScript(error)),
                    parse_start.elapsed(),
                    std::time::Duration::ZERO,
                );
            }
        };
        let parse_elapsed = parse_start.elapsed();
        let execute_start = std::time::Instant::now();
        let _module_document = activate_module_document(&self.host_state, document.identity());
        let promise = self.with_active_host_value(|context| module.load_link_evaluate(context));
        let result = self
            .run_jobs()
            .map_err(JsEvaluationError::JavaScript)
            .and_then(|()| match promise.state() {
                PromiseState::Fulfilled(_) => Ok(JsValue::undefined()),
                PromiseState::Rejected(error) => Err(JsEvaluationError::ModuleRejected(error)),
                PromiseState::Pending => Err(JsEvaluationError::ModulePending),
            });
        // Module evaluation is also driven synchronously here. A modal-dialog
        // suspension therefore cannot outlive this call, even when evaluation
        // exits through the pending/error cases above.
        self.host_state.borrow_mut().pending_javascript_dialog = None;
        (result, parse_elapsed, execute_start.elapsed())
    }

    /// Runs pending promise jobs.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        let result = self.with_active_host(|context| context.run_jobs());
        if result.is_ok() && self.host_state.borrow().document_write_depth == 0 {
            self.run_written_scripts(false)?;
        }
        self.sync_module_csp_violations();
        self.flush_pointer_lock_notifications()?;
        result
    }

    /// Schedules a timeout task from Rust that evaluates `source` as code.
    pub fn set_timeout(&mut self, source: impl Into<String>, delay_ms: u64) -> u64 {
        self.host_state.borrow_mut().event_loop.schedule_timer(
            TimerPayload::Source(source.into()),
            delay_ms,
            false,
            None,
        )
    }

    /// Schedules an interval task from Rust that evaluates `source` as code.
    pub fn set_interval(&mut self, source: impl Into<String>, interval_ms: u64) -> u64 {
        self.host_state.borrow_mut().event_loop.schedule_timer(
            TimerPayload::Source(source.into()),
            interval_ms,
            true,
            None,
        )
    }

    /// Clears a previously scheduled timer.
    pub fn clear_timer(&mut self, id: u64) {
        self.host_state.borrow_mut().event_loop.clear_timer(id);
    }

    /// Notifies live `PermissionStatus` objects in this realm after an
    /// embedder-controlled permission transition.  The helper is intentionally
    /// best-effort: a runtime may be in the middle of teardown, in which case
    /// there are no page objects left to notify.
    fn notify_permission_change(&mut self, name: &str, state: &str) {
        let Ok(name) = serde_json::to_string(name) else {
            return;
        };
        let Ok(state) = serde_json::to_string(state) else {
            return;
        };
        let source = format!(
            "if (typeof globalThis.__omoikane_permission_changed === 'function') {{ globalThis.__omoikane_permission_changed({name}, {state}); }}"
        );
        let _ = self.eval(&source);
    }

    /// Sets the deterministic Notification permission used by this runtime's
    /// Window. This is an embedder/test hook and is not exposed to page JS.
    pub fn set_notification_permission(
        &mut self,
        permission: &str,
    ) -> Result<(), NotificationPermissionError> {
        if !matches!(permission, "default" | "granted" | "denied") {
            return Err(NotificationPermissionError::Invalid(permission.to_string()));
        }
        let changed = {
            let mut state = self.host_state.borrow_mut();
            if state.notification_permission == permission {
                false
            } else {
                state.notification_permission = permission.to_string();
                true
            }
        };
        if changed {
            let mapped = if permission == "default" {
                "prompt"
            } else {
                permission
            };
            self.notify_permission_change("notifications", mapped);
        }
        Ok(())
    }

    /// Sets the deterministic Async Clipboard permission used by this
    /// runtime. This is an embedder/test hook and is not exposed to page JS.
    pub fn set_clipboard_permission(&mut self, granted: bool) {
        let changed = {
            let mut state = self.host_state.borrow_mut();
            if state.clipboard_permission_granted == granted {
                false
            } else {
                state.clipboard_permission_granted = granted;
                true
            }
        };
        if changed {
            self.notify_permission_change(
                "clipboard-read",
                if granted { "granted" } else { "denied" },
            );
            self.notify_permission_change(
                "clipboard-write",
                if granted { "granted" } else { "denied" },
            );
        }
    }

    /// Advances the event loop clock and runs due macrotasks and pending jobs.
    ///
    /// Due timers fire in fire-time order (ties broken by registration order).
    /// Both string-source timers and function-callback timers are supported;
    /// callbacks re-scheduled from within a firing callback (e.g. an
    /// `setTimeout(update, delay)` chain) become due on subsequent ticks.
    pub fn tick(&mut self, elapsed_ms: u64) -> JsResult<()> {
        self.host_state.borrow_mut().event_loop.advance(elapsed_ms);
        self.advance_worker_clocks(elapsed_ms);
        self.advance_worklet_clocks(elapsed_ms);
        self.run_worker_background_tasks();
        self.run_worklet_background_tasks();
        self.run_shared_worker_background_tasks();
        self.run_until_idle()
    }

    /// Runs queued macrotasks and pending promise jobs until idle.
    ///
    /// Propagates the first error thrown by a timer callback or source string.
    pub fn run_until_idle(&mut self) -> JsResult<()> {
        if self.is_terminated_worker() {
            return Ok(());
        }
        // An embedder call may have completed a script task and left promise
        // jobs pending. Its checkpoint can itself enqueue host tasks.
        self.run_jobs()?;
        self.run_worker_background_tasks();
        self.run_worklet_background_tasks();
        self.run_shared_worker_background_tasks();
        loop {
            if self.is_terminated_worker() {
                break;
            }
            flush_web_lock_notifications();
            let task = { self.host_state.borrow_mut().event_loop.pop_task() };
            let Some((_, task)) = task else {
                break;
            };
            let task_document_id = match &task {
                Task::Timer {
                    owner_document_id, ..
                } => *owner_document_id,
                Task::WebLock { document_id, .. } => Some(*document_id),
                _ => None,
            };
            self.run_task(task)?;
            self.run_worker_background_tasks();
            self.run_worklet_background_tasks();
            self.run_shared_worker_background_tasks();
            if self.is_terminated_worker() {
                break;
            }
            // HTML performs a microtask checkpoint after every task, including
            // host-only navigation tasks that do not directly invoke script.
            self.run_jobs_for_document(task_document_id)?;
        }
        Ok(())
    }

    fn is_terminated_worker(&self) -> bool {
        let state = self.host_state.borrow();
        state.worker_id.is_some() && state.worker_terminated
    }

    async fn run_until_idle_async(&mut self) -> JsResult<()> {
        if self.is_terminated_worker() {
            return Ok(());
        }
        self.run_jobs()?;
        self.run_worklet_background_tasks();
        self.run_shared_worker_background_tasks();
        loop {
            if self.is_terminated_worker() {
                break;
            }
            flush_web_lock_notifications();
            let task = { self.host_state.borrow_mut().event_loop.pop_task() };
            let Some((_, task)) = task else { break };
            let task_document_id = match &task {
                Task::Timer {
                    owner_document_id, ..
                } => *owner_document_id,
                Task::WebLock { document_id, .. } => Some(*document_id),
                _ => None,
            };
            match task {
                Task::Timer {
                    payload: TimerPayload::Source(source),
                    owner_document_id: Some(document_id),
                } => {
                    let result = self.eval_async_for_document(&source, document_id).await;
                    if let Err(error) = result {
                        if is_wall_clock_timeout(&error) {
                            return Err(error);
                        }
                        self.record_error_from::<()>("timer", Err(error));
                    }
                }
                Task::Timer {
                    payload: TimerPayload::Source(source),
                    owner_document_id: None,
                } => {
                    let result = self.eval_async(&source).await;
                    if let Err(error) = result {
                        if is_wall_clock_timeout(&error) {
                            return Err(error);
                        }
                        self.record_error_from::<()>("timer", Err(error));
                    }
                }
                Task::Timer {
                    payload: TimerPayload::Callback { callback, args },
                    owner_document_id,
                } => {
                    if let Some(callable) = callback.as_callable() {
                        let this = JsValue::undefined();
                        let deadline = execution_deadline(self.sandbox.timeout);
                        let host_state = Rc::clone(&self.host_state);
                        let future = ActiveHostFuture {
                            future: Box::pin(async {
                                let mut context = self.context.enter_runtime_deadline(deadline);
                                callable.call_async(&this, &args, &mut context).await
                            }),
                            host_state: Rc::clone(&host_state),
                        };
                        let result = if let Some(document_id) = owner_document_id {
                            let document_future = ActiveDocumentFuture {
                                future: Box::pin(future),
                                host_state,
                                document_id,
                            };
                            TimedJsFuture::new(document_future, deadline).await
                        } else {
                            TimedJsFuture::new(future, deadline).await
                        };
                        if let Err(error) = result {
                            if is_wall_clock_timeout(&error) {
                                return Err(error);
                            }
                            self.record_error_from::<()>("timer callback", Err(error));
                        }
                    }
                }
                Task::Timer {
                    payload: TimerPayload::ResourceLoad { node_id },
                    ..
                } if self.is_dynamic_script_resource(node_id) => {
                    self.run_dynamic_script_resource_async(node_id).await?;
                }
                Task::PostedMessage { port, data } => {
                    let result = self.run_posted_message_async(port, data).await;
                    if let Err(error) = result {
                        if is_wall_clock_timeout(&error) {
                            return Err(error);
                        }
                        self.record_error_from::<()>("posted message", Err(error));
                    }
                }
                Task::BroadcastChannelMessage {
                    channel_id,
                    data,
                    origin,
                } => {
                    self.run_broadcast_channel_message(channel_id, data, origin)?;
                }
                task => self.run_task(task)?,
            }
            if self.is_terminated_worker() {
                break;
            }
            self.run_shared_worker_background_tasks();
            self.run_worklet_background_tasks();
            self.run_jobs_for_document(task_document_id)?;
        }
        Ok(())
    }

    fn is_dynamic_script_resource(&self, node_id: usize) -> bool {
        self.host_state
            .borrow()
            .get_node(node_id)
            .is_some_and(|node| {
                node.tag_name()
                    .is_some_and(|tag| tag.eq_ignore_ascii_case("script"))
                    && (node.get_attribute("src").is_some()
                        || self
                            .host_state
                            .borrow()
                            .parser_inserted_scripts
                            .contains(&node_id))
            })
    }

    async fn run_posted_message_async(&mut self, port: JsValue, data: JsValue) -> JsResult<()> {
        if let Err(error) = self.install_posted_message_values(port, data) {
            let cleanup_result = self.clear_posted_message_values();
            self.record_error_from("posted message cleanup", cleanup_result);
            return Err(error);
        }
        let result = self
            .eval_async(
                "if (!__omoikane_posted_message_port._closed) { \
                     if (typeof __omoikane_posted_message_port._acceptMessage === 'function') { \
                       __omoikane_posted_message_port._acceptMessage(__omoikane_posted_message_data); \
                     } else { \
                       __omoikane_posted_message_port.dispatchEvent(new MessageEvent('message', { \
                         data: __omoikane_posted_message_data, origin: '', source: null, ports: [] \
                       })); \
                     } \
                     }",
            )
            .await
            .map(|_| ());
        let cleanup_result = self.clear_posted_message_values();
        self.record_error_from("posted message cleanup", cleanup_result);
        result
    }

    fn run_window_posted_message(
        &mut self,
        target_document_id: usize,
        target_iframe: Option<(usize, u64)>,
        target_auxiliary_id: Option<u64>,
        source_document_id: usize,
        source_iframe_id: Option<usize>,
        source_auxiliary_id: Option<u64>,
        sender_security_origin: Option<DocumentSecurityOrigin>,
        origin: &str,
        target_origin: &str,
        wire: &str,
        ports: JsValue,
    ) -> JsResult<()> {
        let source = (|| {
            let state = self.host_state.borrow();
            if let Some(auxiliary_id) = target_auxiliary_id {
                if !state
                    .auxiliary_contexts
                    .get(&auxiliary_id)
                    .is_some_and(|entry| entry.document.identity() == target_document_id)
                {
                    return None;
                }
            } else if let Some((iframe_id, context_id)) = target_iframe {
                let iframe = state.get_node(iframe_id)?;
                if !state.node_is_in_active_document(&iframe)
                    || state.iframe_context_ids.get(&iframe_id) != Some(&context_id)
                    || !state
                        .iframe_documents
                        .get(&iframe_id)
                        .is_some_and(|entry| entry.document.identity() == target_document_id)
                {
                    return None;
                }
            } else if state.document.identity() != target_document_id {
                return None;
            }
            let current_origin = serialized_window_origin(
                state
                    .document_origins
                    .get(&target_document_id)
                    .and_then(Option::as_ref),
            );
            let matches_target = match target_origin {
                "*" => true,
                "/" if source_document_id == target_document_id => true,
                "/" => {
                    match (
                        sender_security_origin.as_ref(),
                        state.document_security_origins.get(&target_document_id),
                    ) {
                        (Some(sender), Some(target)) => sender == target,
                        _ => false,
                    }
                }
                explicit => explicit == current_origin,
            };
            if !matches_target {
                return None;
            }
            Some(if source_document_id == target_document_id {
                ("self", None)
            } else if let Some(auxiliary_id) = source_auxiliary_id {
                ("auxiliary", Some(auxiliary_id as usize))
            } else if target_auxiliary_id.is_some_and(|id| {
                state
                    .auxiliary_contexts
                    .get(&id)
                    .is_some_and(|entry| entry.opener_document_id == source_document_id)
            }) {
                ("opener", None)
            } else if target_iframe.is_some_and(|(iframe_id, _)| {
                state
                    .get_node(iframe_id)
                    .and_then(|node| owner_document_for_node(&node))
                    .is_some_and(|owner| owner.identity() == source_document_id)
            }) {
                ("parent", None)
            } else if source_iframe_id.is_some_and(|iframe_id| {
                state
                    .get_node(iframe_id)
                    .and_then(|node| owner_document_for_node(&node))
                    .is_some_and(|owner| owner.identity() == target_document_id)
            }) {
                ("iframe", source_iframe_id)
            } else {
                ("null", None)
            })
        })();
        let Some(source) = source else {
            return self.close_transferred_message_ports(ports);
        };
        let realm = if let Some(auxiliary_id) = target_auxiliary_id {
            self.ensure_auxiliary_realm(auxiliary_id)?
        } else if let Some((iframe_id, _)) = target_iframe {
            self.ensure_iframe_realm(iframe_id, target_document_id)?
        } else {
            let state = self.host_state.borrow();
            let Some(realm) = state.main_realm.clone() else {
                drop(state);
                return self.close_transferred_message_ports(ports);
            };
            realm
        };
        let previous = self.context.enter_realm(realm);
        let result = (|| {
            self.with_active_host(|context| {
                let global = context.global_object();
                let source_function =
                    global.get(js_string!("__omoikane_window_message_source"), context)?;
                let source_function = source_function.as_callable().ok_or_else(|| {
                    JsNativeError::typ()
                        .with_message("Window message source resolver is unavailable")
                })?;
                let source_value = source_function.call(
                    &global.clone().into(),
                    &[
                        JsValue::from(js_string!(source.0)),
                        source
                            .1
                            .map_or(JsValue::null(), |id| JsValue::from(id as f64)),
                    ],
                    context,
                )?;
                let receiver =
                    global.get(js_string!("__omoikane_receive_window_message"), context)?;
                let receiver = receiver.as_callable().ok_or_else(|| {
                    JsNativeError::typ().with_message("Window message receiver is unavailable")
                })?;
                receiver.call(
                    &global.clone().into(),
                    &[
                        JsValue::from(js_string!(wire)),
                        JsValue::from(js_string!(origin)),
                        source_value,
                        ports,
                    ],
                    context,
                )?;
                Ok(())
            })?;
            self.run_jobs()
        })();
        self.context.enter_realm(previous);
        result
    }

    fn close_transferred_message_ports(&mut self, ports: JsValue) -> JsResult<()> {
        self.with_active_host(|context| {
            let Some(ports) = ports.as_object() else {
                return Ok(());
            };
            let length = ports.get(js_string!("length"), context)?.to_u32(context)?;
            for index in 0..length {
                let port = ports.get(index, context)?;
                let Some(object) = port.as_object() else {
                    continue;
                };
                let close = object.get(js_string!("close"), context)?;
                if let Some(close) = close.as_callable() {
                    close.call(&port, &[], context)?;
                }
            }
            Ok(())
        })
    }

    fn run_auxiliary_navigation(
        &mut self,
        id: u64,
        url: &str,
        source: Option<VisitSource>,
    ) -> JsResult<()> {
        if !self
            .host_state
            .borrow()
            .auxiliary_contexts
            .contains_key(&id)
        {
            return Ok(());
        }
        let document_id = self
            .host_state
            .borrow_mut()
            .navigate_auxiliary_context(id, url)
            .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
        if let Some(source) = source {
            let state = self.host_state.borrow();
            if let Some(committed) = state.document_urls.get(&document_id)
                && StorageOrigin::from_url(committed).is_some()
            {
                state
                    .storage_manager
                    .record_page_navigation(url, committed, source);
            }
        }
        let realm = match self.ensure_auxiliary_realm(id) {
            Ok(realm) => realm,
            Err(error) => {
                if std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some() {
                    let opaque = error.to_opaque(&mut self.context);
                    let description = opaque
                        .to_string(&mut self.context)
                        .map(|value| value.to_std_string_escaped())
                        .unwrap_or_else(|_| "<unprintable>".to_owned());
                    eprintln!("[omoikane][auxiliary-realm-error] {description}");
                }
                return Err(error);
            }
        };
        let scripts = {
            let state = self.host_state.borrow();
            let document = state.get_node(document_id).ok_or_else(|| {
                JsNativeError::reference().with_message("popup Document disappeared")
            })?;
            collect_script_elements(&document)
        };
        let previous = self.context.enter_realm(realm);
        let result = (|| -> JsResult<()> {
            for script in scripts {
                let attributes = script.attributes().unwrap_or_default();
                if !is_executable_classic_script_type(attributes.get("type").map(String::as_str)) {
                    continue;
                }
                let policy = self.host_state.borrow().csp_policy_for_node(&script);
                let source = if let Some(src) = attributes.get("src") {
                    if !policy.allows_reference(ResourceType::Script, src) {
                        self.host_state.borrow_mut().record_csp_violation_for_node(
                            &script,
                            ResourceType::Script,
                            src,
                        );
                        continue;
                    }
                    let base = self.host_state.borrow().base_url_for_document(document_id);
                    let fetched = {
                        let mut state = self.host_state.borrow_mut();
                        fetch_script_resource_with_client(
                            src,
                            base.as_ref(),
                            &mut state.http_client,
                        )
                    };
                    let Some((effective_url, source, redirect_count)) = fetched else {
                        self.record_task_error(format!("[popup script: {src}] failed to fetch"));
                        continue;
                    };
                    if !policy.allows_reference_after_redirects(
                        ResourceType::Script,
                        &effective_url,
                        redirect_count,
                    ) {
                        self.host_state.borrow_mut().record_csp_violation_for_node(
                            &script,
                            ResourceType::Script,
                            effective_url,
                        );
                        continue;
                    }
                    source
                } else {
                    if !policy.allows_inline(ResourceType::Script) {
                        self.host_state.borrow_mut().record_csp_violation_for_node(
                            &script,
                            ResourceType::Script,
                            "inline",
                        );
                        continue;
                    }
                    collect_text_content(&script)
                };
                self.eval(&source)?;
            }
            self.eval(
                "document.__readyState = 'complete'; document.dispatchEvent(new Event('DOMContentLoaded')); window.dispatchEvent(new Event('load'));",
            )?;
            self.run_jobs()
        })();
        if let Err(error) = &result
            && std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some()
        {
            let opaque = error.to_opaque(&mut self.context);
            let description = opaque
                .to_string(&mut self.context)
                .map(|value| value.to_std_string_escaped())
                .unwrap_or_else(|_| "<unprintable>".to_owned());
            eprintln!("[omoikane][auxiliary-navigation-error] {description}");
        }
        self.context.enter_realm(previous);
        result
    }

    fn install_posted_message_values(&mut self, port: JsValue, data: JsValue) -> JsResult<()> {
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_posted_message_port"),
            port,
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_posted_message_data"),
            data,
            true,
            &mut self.context,
        )?;
        Ok(())
    }

    fn clear_posted_message_values(&mut self) -> JsResult<()> {
        let global = self.context.global_object();
        let port_result = global.set(
            js_string!("__omoikane_posted_message_port"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        let data_result = global.set(
            js_string!("__omoikane_posted_message_data"),
            JsValue::undefined(),
            true,
            &mut self.context,
        );
        port_result?;
        data_result?;
        Ok(())
    }

    async fn run_dynamic_script_resource_async(&mut self, node_id: usize) -> JsResult<()> {
        if self
            .host_state
            .borrow()
            .parser_inserted_scripts
            .contains(&node_id)
        {
            return self.run_written_script(node_id);
        }
        let Some((script_node, src, kind, base_url, document_id)) = ({
            let mut state = self.host_state.borrow_mut();
            state.pending_resource_loads.remove(&node_id);
            state.get_node(node_id).and_then(|node| {
                if !state.node_is_in_active_document(&node) {
                    return None;
                }
                let document_id = document_root_for_node(&node)?.identity();
                let src = node.get_attribute("src")?;
                Some((
                    node.clone(),
                    src,
                    ScriptKind::from_type_attribute(node.get_attribute("type").as_deref()),
                    state.base_url_for_document(document_id),
                    document_id,
                ))
            })
        }) else {
            return Ok(());
        };

        if kind == ScriptKind::NotExecutable {
            return Ok(());
        }
        let timing_name = resource_reference_timing_name(&src, base_url.as_ref());
        if !self
            .host_state
            .borrow()
            .sandbox_allows_scripts_for_node(&script_node)
        {
            let dispatch =
                dispatch_resource_timing_script("error", node_id, &timing_name, false, 0.0);
            let result = self.eval_async_for_document(&dispatch, document_id).await;
            if let Err(error) = result {
                if is_wall_clock_timeout(&error) {
                    return Err(error);
                }
                self.record_error_from::<()>(&src, Err(error));
            }
            return Ok(());
        }
        if !self
            .host_state
            .borrow()
            .csp_policy_for_node(&script_node)
            .allows_reference(ResourceType::Script, &src)
        {
            self.host_state.borrow_mut().record_csp_violation_for_node(
                &script_node,
                ResourceType::Script,
                &src,
            );
            return Ok(());
        }
        let fetch_start = std::time::Instant::now();
        let fetched = {
            let mut state = self.host_state.borrow_mut();
            fetch_script_resource_with_client(&src, base_url.as_ref(), &mut state.http_client)
        };
        let elapsed_ms = fetch_start.elapsed().as_secs_f64() * 1_000.0;
        let Some((effective_url, source, redirect_count)) = fetched else {
            self.record_task_error(format!("[dynamic script: {src}] failed to fetch"));
            let dispatch =
                dispatch_resource_timing_script("error", node_id, &timing_name, false, elapsed_ms);
            let result = self.eval_async_for_document(&dispatch, document_id).await;
            if let Err(error) = result {
                if is_wall_clock_timeout(&error) {
                    return Err(error);
                }
                self.record_error_from::<()>(&src, Err(error));
            }
            return Ok(());
        };
        let redirected = resource_reference_was_redirected(&src, &effective_url, base_url.as_ref());
        if !self
            .host_state
            .borrow()
            .csp_policy_for_node(&script_node)
            .allows_reference_after_redirects(ResourceType::Script, &effective_url, redirect_count)
        {
            self.host_state.borrow_mut().record_csp_violation_for_node(
                &script_node,
                ResourceType::Script,
                effective_url.clone(),
            );
            let dispatch = dispatch_resource_timing_script(
                "error",
                node_id,
                &effective_url,
                redirected,
                elapsed_ms,
            );
            let result = self.eval_async_for_document(&dispatch, document_id).await;
            if let Err(error) = result {
                if is_wall_clock_timeout(&error) {
                    return Err(error);
                }
                self.record_error_from::<()>(&src, Err(error));
            }
            return Ok(());
        }

        let marked = self
            .eval_async_for_document(
                &format!("__omoikane_set_current_script({node_id})"),
                document_id,
            )
            .await;
        if let Err(error) = marked {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            self.record_error_from::<()>(&src, Err(error));
        }
        let result = match kind {
            ScriptKind::Module => {
                let module_document =
                    document_root_for_node(&script_node).unwrap_or_else(|| self.document());
                self.eval_module_async(
                    &source,
                    &module_script_url(&src, base_url.as_ref(), false),
                    module_document,
                )
                .await
            }
            _ => self.eval_async_for_document(&source, document_id).await,
        };
        let _ = self.eval("__omoikane_set_current_script(null)");
        if let Err(error) = result {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            let context = script_source_context(&source);
            self.record_task_error(format!("[dynamic script: {src}; {context}] {error}"));
        }
        let dispatched = self
            .eval_async_for_document(
                &dispatch_resource_timing_script(
                    "load",
                    node_id,
                    &effective_url,
                    redirected,
                    elapsed_ms,
                ),
                document_id,
            )
            .await;
        if let Err(error) = dispatched {
            if is_wall_clock_timeout(&error) {
                return Err(error);
            }
            self.record_error_from::<()>("resource load", Err(error));
        }
        self.sync_module_csp_violations();
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
            if matches!(label, "timer" | "timer callback") {
                self.record_js_task_failure("JS_TIMER_CALLBACK_FAILED", "timer");
            }
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

    /// Returns true when geolocation callbacks are queued on their dedicated
    /// task source. `run_timers` also drains these host tasks so embedders can
    /// use one deterministic pump for all virtual-time work.
    pub fn has_pending_geolocation_tasks(&self) -> bool {
        self.host_state
            .borrow()
            .event_loop
            .has_pending_geolocation_tasks()
    }

    fn has_pending_css_transition_work(&self) -> bool {
        self.host_state
            .borrow()
            .document_styles
            .values()
            .any(|entry| {
                entry.dirty
                    || entry.needs_full_sample
                    || entry
                        .resolver
                        .as_ref()
                        .is_some_and(StyleResolver::has_running_transitions)
            })
    }

    /// Runs one rendering opportunity and invokes its animation-frame callbacks.
    /// `elapsed_ms` is a delta since the previous opportunity or initialization;
    /// callbacks receive the accumulated, absolute page timestamp.
    ///
    /// Pending macrotasks and promise jobs are drained before the frame starts.
    /// All callbacks present at the start of the frame receive the same
    /// monotonically increasing timestamp and run in registration order.
    /// Callbacks registered while the frame is running are retained for the
    /// next explicit call; [`run_jobs`](Self::run_jobs) alone never invokes
    /// animation-frame callbacks.
    pub fn run_animation_frame(&mut self, elapsed_ms: u64) -> JsResult<usize> {
        self.host_state.borrow_mut().event_loop.advance(elapsed_ms);
        self.advance_worker_clocks(elapsed_ms);
        self.advance_worklet_clocks(elapsed_ms);
        self.run_worker_background_tasks();
        self.run_worklet_background_tasks();
        self.run_until_idle()?;
        self.host_state.borrow_mut().sample_smooth_scrolls();
        self.host_state
            .borrow_mut()
            .collect_iframe_viewport_resizes();
        if self.has_pending_viewport_resize_steps() {
            self.flush_pending_viewport_resize_events()?;
        }
        if self.has_pending_scroll_steps() {
            self.flush_pending_scroll_events()?;
        }
        if self.has_pending_visual_viewport_scroll_steps() {
            self.flush_pending_visual_viewport_scroll_events()?;
        }

        if self.host_state.borrow().page_hidden {
            // Keep callbacks queued until the page has a rendering opportunity.
            return Ok(0);
        }

        let (timestamp, callback_ids) = self
            .host_state
            .borrow_mut()
            .event_loop
            .begin_animation_frame();
        let mut callbacks_run = 0;
        let mut first_error = None;
        let mut first_jobs_error = None;

        for id in callback_ids {
            let callback = self
                .host_state
                .borrow_mut()
                .event_loop
                .take_animation_frame_callback(id);
            let Some(callback) = callback else {
                continue;
            };

            let event_loop::AnimationFrameCallback {
                callback,
                realm,
                document_id,
            } = callback;
            if let (Some(realm), Some(document_id)) = (realm.as_ref(), document_id)
                && !self.iframe_realm_is_live(document_id, realm)
            {
                // Navigation replaced the child Document before its rendering
                // opportunity. Drop the stale callback with the old Realm.
                continue;
            }
            let old_realm = realm.map(|realm| self.context.enter_realm(realm));
            let _document = document_id
                .map(|document_id| activate_module_document(&self.host_state, document_id));
            let result = self.with_active_host(|context| {
                let callable = callback.as_callable().ok_or_else(|| {
                    JsError::from(
                        JsNativeError::typ()
                            .with_message("animation frame callback is not callable"),
                    )
                })?;
                callable.call(&JsValue::undefined(), &[JsValue::from(timestamp)], context)?;
                Ok(())
            });
            if let Some(old_realm) = old_realm {
                self.context.enter_realm(old_realm);
            }
            callbacks_run += 1;
            if let Err(error) = result {
                self.record_js_task_failure(
                    "JS_ANIMATION_FRAME_CALLBACK_FAILED",
                    "animation-frame",
                );
                // Browser callback exceptions are reported without preventing
                // the remaining callbacks in the same frame from running.
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            if document_id.is_some()
                && let Err(error) = self.run_jobs()
            {
                self.record_js_task_failure("JS_ANIMATION_FRAME_JOB_FAILED", "animation-frame");
                if first_jobs_error.is_none() {
                    first_jobs_error = Some(error);
                }
            }
        }

        // DOM mutations performed by frame callbacks queue observer delivery;
        // the microtask checkpoint completes that work before the embedder
        // proceeds to style/layout/paint.
        let jobs_result = self.run_jobs();
        if callbacks_run > 0 && jobs_result.is_err() {
            self.record_js_task_failure("JS_ANIMATION_FRAME_JOB_FAILED", "animation-frame");
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        if let Some(error) = first_jobs_error {
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

    async fn run_animation_frame_async(&mut self, elapsed_ms: u64) -> JsResult<usize> {
        self.host_state.borrow_mut().event_loop.advance(elapsed_ms);
        self.advance_worker_clocks(elapsed_ms);
        self.advance_worklet_clocks(elapsed_ms);
        self.run_worker_background_tasks();
        self.run_worklet_background_tasks();
        self.run_until_idle_async().await?;
        self.host_state.borrow_mut().sample_smooth_scrolls();
        self.host_state
            .borrow_mut()
            .collect_iframe_viewport_resizes();
        if self.has_pending_viewport_resize_steps() {
            self.flush_pending_viewport_resize_events()?;
        }
        if self.has_pending_scroll_steps() {
            self.flush_pending_scroll_events()?;
        }
        if self.has_pending_visual_viewport_scroll_steps() {
            self.flush_pending_visual_viewport_scroll_events()?;
        }
        if self.host_state.borrow().page_hidden {
            return Ok(0);
        }
        let (timestamp, callback_ids) = self
            .host_state
            .borrow_mut()
            .event_loop
            .begin_animation_frame();
        let mut callbacks_run = 0;
        let mut first_error = None;
        let mut first_jobs_error = None;
        for id in callback_ids {
            let callback = self
                .host_state
                .borrow_mut()
                .event_loop
                .take_animation_frame_callback(id);
            let Some(callback) = callback else { continue };
            let event_loop::AnimationFrameCallback {
                callback,
                realm,
                document_id,
            } = callback;
            if let (Some(realm), Some(document_id)) = (realm.as_ref(), document_id)
                && !self.iframe_realm_is_live(document_id, realm)
            {
                continue;
            }
            let old_realm = realm.map(|realm| self.context.enter_realm(realm));
            let result = if let Some(callable) = callback.as_callable() {
                let args = [JsValue::from(timestamp)];
                let this = JsValue::undefined();
                let deadline = execution_deadline(self.sandbox.timeout);
                let host_state = Rc::clone(&self.host_state);
                let future = ActiveHostFuture {
                    future: Box::pin(async {
                        let mut context = self.context.enter_runtime_deadline(deadline);
                        callable.call_async(&this, &args, &mut context).await
                    }),
                    host_state: Rc::clone(&host_state),
                };
                if let Some(document_id) = document_id {
                    let document_future = ActiveDocumentFuture {
                        future: Box::pin(future),
                        host_state,
                        document_id,
                    };
                    TimedJsFuture::new(document_future, deadline)
                        .await
                        .map(|_| ())
                } else {
                    TimedJsFuture::new(future, deadline).await.map(|_| ())
                }
            } else {
                Err(JsNativeError::typ()
                    .with_message("animation frame callback is not callable")
                    .into())
            };
            if let Some(old_realm) = old_realm {
                self.context.enter_realm(old_realm);
            }
            callbacks_run += 1;
            if let Err(error) = result {
                self.record_js_task_failure(
                    "JS_ANIMATION_FRAME_CALLBACK_FAILED",
                    "animation-frame",
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            if document_id.is_some()
                && let Err(error) = self.run_jobs_for_document(document_id)
            {
                self.record_js_task_failure("JS_ANIMATION_FRAME_JOB_FAILED", "animation-frame");
                if first_jobs_error.is_none() {
                    first_jobs_error = Some(error);
                }
            }
        }
        let jobs_result = self.run_jobs();
        if callbacks_run > 0 && jobs_result.is_err() {
            self.record_js_task_failure("JS_ANIMATION_FRAME_JOB_FAILED", "animation-frame");
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        if let Some(error) = first_jobs_error {
            return Err(error);
        }
        jobs_result?;
        self.update_css_transitions()?;
        self.run_until_idle_async().await?;
        Ok(callbacks_run)
    }

    /// Dispatches coalesced Window and VisualViewport events for this rendering
    /// opportunity. Window resize precedes VisualViewport resize for each
    /// browsing context, as required by CSSOM View's event ordering.
    fn flush_pending_viewport_resize_events(&mut self) -> JsResult<usize> {
        let (window_resize, visual_resize) = {
            let mut state = self.host_state.borrow_mut();
            (
                std::mem::take(&mut state.pending_window_resize_documents),
                std::mem::take(&mut state.pending_visual_viewport_resize_documents),
            )
        };
        let mut documents = Vec::new();
        for document_id in window_resize.iter().chain(&visual_resize).copied() {
            if !documents.contains(&document_id) {
                documents.push(document_id);
            }
        }
        for document_id in &documents {
            self.eval_in_document_realm(
                *document_id,
                &format!(
                    "__omoikane_dispatch_viewport_events({}, {}, {})",
                    window_resize.contains(document_id),
                    visual_resize.contains(document_id),
                    false,
                ),
            )?;
        }
        Ok(documents.len())
    }

    fn flush_pending_visual_viewport_scroll_events(&mut self) -> JsResult<usize> {
        let documents = std::mem::take(
            &mut self
                .host_state
                .borrow_mut()
                .pending_visual_viewport_scroll_documents,
        );
        for document_id in &documents {
            self.eval_in_document_realm(
                *document_id,
                "__omoikane_dispatch_viewport_events(false, false, true)",
            )?;
        }
        Ok(documents.len())
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
            let (owner_document_id, viewport) = {
                let state = self.host_state.borrow();
                let node = state.get_node(node_id);
                let viewport = node
                    .as_ref()
                    .is_some_and(|node| node.node_type() == NodeType::Document);
                let owner_document_id = node
                    .as_ref()
                    .and_then(|node| {
                        if viewport {
                            Some(node.identity())
                        } else {
                            document_root_for_node(node).map(|document| document.identity())
                        }
                    })
                    .unwrap_or(document_id);
                (owner_document_id, viewport)
            };
            self.eval_in_document_realm(
                owner_document_id,
                &format!("__omoikane_dispatch_scroll_event({node_id}, {viewport})"),
            )?;
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
            || !state.smooth_scrolls.is_empty()
    }

    fn has_pending_viewport_steps(&self) -> bool {
        self.has_pending_viewport_resize_steps() || self.has_pending_visual_viewport_scroll_steps()
    }

    fn has_pending_viewport_resize_steps(&self) -> bool {
        let state = self.host_state.borrow();
        !state.pending_window_resize_documents.is_empty()
            || !state.pending_visual_viewport_resize_documents.is_empty()
    }

    fn has_pending_visual_viewport_scroll_steps(&self) -> bool {
        !self
            .host_state
            .borrow()
            .pending_visual_viewport_scroll_documents
            .is_empty()
    }

    /// Drives a bounded number of rendering opportunities until no callback is pending.
    ///
    /// Callback errors are logged when script diagnostics are enabled and do
    /// not prevent later frames from settling, matching the render pipeline's
    /// best-effort timer pump.
    pub fn run_animation_frames(&mut self, max_frames: usize, frame_interval_ms: u64) -> usize {
        let mut callbacks_run = 0;
        for _ in 0..max_frames {
            if self.host_state.borrow().page_hidden
                && !self.has_pending_scroll_steps()
                && !self.has_pending_viewport_steps()
            {
                break;
            }
            if !self.has_pending_animation_frames()
                && !self.has_pending_scroll_steps()
                && !self.has_pending_viewport_steps()
            {
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
    /// bounds the total number of tasks executed (so a callback that
    /// continuously schedules timers or posted messages cannot explode).
    ///
    /// Unlike [`tick`](Self::tick), individual callback errors are swallowed so
    /// that one throwing timer does not halt the remaining pipeline work.
    ///
    /// Returns the number of timer tasks that were actually executed.
    pub fn run_timers(&mut self, max_virtual_ms: u64, step_ms: u64, max_tasks: usize) -> usize {
        let step = step_ms.max(1);
        let mut advanced: u64 = 0;
        let mut tasks_run: usize = 0;
        let mut tasks_processed: usize = 0;

        while advanced < max_virtual_ms && tasks_processed < max_tasks {
            if !self.has_pending_timers()
                && !self.has_pending_geolocation_tasks()
                && !self.has_pending_css_transition_work()
            {
                break;
            }
            self.host_state.borrow_mut().event_loop.advance(step);
            self.advance_worker_clocks(step);
            self.advance_worklet_clocks(step);
            self.run_worker_background_tasks();
            self.run_worklet_background_tasks();
            flush_web_lock_notifications();
            advanced = advanced.saturating_add(step);

            loop {
                if tasks_processed >= max_tasks {
                    break;
                }
                let Some((_, task)) = self.host_state.borrow_mut().event_loop.pop_task() else {
                    break;
                };
                tasks_processed += 1;
                let is_timer = matches!(task, Task::Timer { .. });
                let task_document_id = match &task {
                    Task::Timer {
                        owner_document_id, ..
                    } => *owner_document_id,
                    Task::WebLock { document_id, .. } => Some(*document_id),
                    Task::WindowPostedMessage {
                        target_document_id, ..
                    } => Some(*target_document_id),
                    _ => None,
                };
                {
                    // Swallow per-task JS errors: a single failing timer must
                    // not abort the whole pump during rendering. Diagnostics
                    // remain available on demand for complex app bootstraps.
                    let task_kind = match &task {
                        Task::Timer { payload, .. } => payload.kind(),
                        Task::Geolocation { .. } => "geolocation",
                        Task::WebLock { .. } => "web-lock",
                        Task::Navigation { .. } => "navigation",
                        Task::AuxiliaryNavigate { .. } => "auxiliary-navigation",
                        Task::PostedMessage { .. } => "posted-message",
                        Task::WindowPostedMessage { .. } => "window-posted-message",
                        Task::BroadcastChannelMessage { .. } => "broadcast-channel",
                        Task::WorkerMessage { .. }
                        | Task::WorkerOwnerMessage { .. }
                        | Task::WorkerError { .. }
                        | Task::SharedWorkerMessage { .. }
                        | Task::SharedWorkerOwnerMessage { .. } => "worker",
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
                    let jobs_result = self.run_jobs_for_document(task_document_id);
                    let jobs_elapsed = jobs_start.elapsed();
                    if let Err(error) = jobs_result
                        && std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some()
                    {
                        eprintln!("[omoikane][timer-job-error] {error}");
                    }
                    if std::env::var_os("OMOIKANE_LOG_TIMERS").is_some() {
                        eprintln!(
                            "[omoikane][timer] task={} kind={} callback_ms={:.3} jobs_ms={:.3}",
                            tasks_processed,
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
        if self.is_terminated_worker() {
            return Ok(());
        }
        match task {
            Task::Timer {
                payload,
                owner_document_id,
            } => {
                let _document = owner_document_id
                    .map(|document_id| activate_module_document(&self.host_state, document_id));
                self.run_timer_payload(payload)
            }
            Task::Geolocation { request_id } => self.run_geolocation_delivery(request_id, false),
            Task::WebLock {
                document_id,
                request_id,
                stolen,
            } => self.eval_in_document_realm(
                document_id,
                &format!(
                    "{}({:?})",
                    if stolen {
                        "__omoikane_web_lock_stolen"
                    } else {
                        "__omoikane_web_lock_granted"
                    },
                    request_id.to_string(),
                ),
            ),
            Task::Navigation { request, source } => {
                self.host_state
                    .borrow_mut()
                    .navigation_requests
                    .push_back((request, source));
                Ok(())
            }
            Task::PostedMessage { port, data } => {
                if let Err(error) = self.install_posted_message_values(port, data) {
                    self.record_task_error(format!("[posted message] {error}"));
                    let cleanup_result = self.clear_posted_message_values();
                    self.record_error_from("posted message cleanup", cleanup_result);
                    return Ok(());
                }
                let result = self.eval(
                    "if (!__omoikane_posted_message_port._closed) { \
                 if (typeof __omoikane_posted_message_port._acceptMessage === 'function') { \
                   __omoikane_posted_message_port._acceptMessage(__omoikane_posted_message_data); \
                 } else { \
                   __omoikane_posted_message_port.dispatchEvent(new MessageEvent('message', { \
                     data: __omoikane_posted_message_data, origin: '', source: null, ports: [] \
                   })); \
                 } \
                 }",
                );
                let cleanup_result = self.clear_posted_message_values();
                self.record_error_from("posted message cleanup", cleanup_result);
                self.record_error_from("posted message", result);
                Ok(())
            }
            Task::WindowPostedMessage {
                target_document_id,
                target_iframe,
                target_auxiliary_id,
                source_document_id,
                source_iframe_id,
                source_auxiliary_id,
                sender_security_origin,
                origin,
                target_origin,
                wire,
                ports,
            } => self.run_window_posted_message(
                target_document_id,
                target_iframe,
                target_auxiliary_id,
                source_document_id,
                source_iframe_id,
                source_auxiliary_id,
                sender_security_origin,
                &origin,
                &target_origin,
                &wire,
                ports,
            ),
            Task::AuxiliaryNavigate { id, url, source } => {
                self.run_auxiliary_navigation(id, &url, source)
            }
            Task::BroadcastChannelMessage {
                channel_id,
                data,
                origin,
            } => self.run_broadcast_channel_message(channel_id, data, origin),
            Task::WorkerMessage { worker_id, data } => self.run_worker_message(worker_id, data),
            Task::WorkerOwnerMessage {
                worker_id,
                owner,
                realm,
                data,
            } => self.run_worker_owner_message(worker_id, owner, realm, data),
            Task::WorkerError {
                worker_id,
                owner,
                realm,
                message,
            } => self.run_worker_error(worker_id, owner, realm, message),
            Task::SharedWorkerMessage {
                connection_id,
                data,
            } => self.run_shared_worker_message(connection_id, data),
            Task::SharedWorkerOwnerMessage {
                connection_id,
                port,
                data,
                origin,
            } => self.run_shared_worker_owner_message(connection_id, port, data, origin),
        }
    }

    fn run_worker_message(&mut self, worker_id: u64, data: String) -> JsResult<()> {
        let owner_origin = host_state_origin(&self.host_state.borrow());
        let entry = self.host_state.borrow_mut().workers.remove(&worker_id);
        let Some(entry) = entry else {
            return Ok(());
        };
        let mut worker = entry.borrow_mut();
        if worker.terminated || worker.runtime.host_state.borrow().worker_terminated {
            self.host_state
                .borrow_mut()
                .worker_owner_objects
                .remove(&worker_id);
            return Ok(());
        }
        let owner_state = Rc::clone(&worker.owner_state);
        let (errors, owner_realm, terminated) = {
            let runtime = &mut worker.runtime;
            let mut errors = Vec::new();
            if let Err(error) = runtime.install_worker_message_values(data, owner_origin) {
                errors.push(format!("[worker message setup] {error}"));
                if let Err(cleanup_error) = runtime.clear_worker_message_values() {
                    errors.push(format!("[worker message cleanup] {cleanup_error}"));
                }
            } else {
                if let Err(error) = runtime.eval(
                    "__omoikane_worker_message_data = __omoikane_decode_worker_message(__omoikane_worker_message_wire); self.dispatchEvent(new MessageEvent('message', { data: __omoikane_worker_message_data, origin: __omoikane_worker_message_origin, source: null, ports: [] }));",
                ) {
                    errors.push(format!("[worker message] {error}"));
                }
                if let Err(error) = runtime.clear_worker_message_values() {
                    errors.push(format!("[worker message cleanup] {error}"));
                }
                if !runtime.is_terminated_worker()
                    && let Err(error) = runtime.run_jobs()
                {
                    errors.push(format!("[worker message jobs] {error}"));
                }
            }
            let owner_realm = runtime.host_state.borrow().worker_owner_realm.clone();
            let terminated = runtime.is_terminated_worker();
            (errors, owner_realm, terminated)
        };
        for error in errors {
            owner_state.borrow_mut().event_loop.enqueue_worker_error(
                worker_id,
                None,
                owner_realm.clone(),
                error,
            );
        }
        drop(worker);
        if !terminated {
            self.host_state
                .borrow_mut()
                .workers
                .insert(worker_id, entry);
        } else {
            self.host_state
                .borrow_mut()
                .worker_owner_objects
                .remove(&worker_id);
        }
        Ok(())
    }

    fn run_worker_owner_message(
        &mut self,
        _worker_id: u64,
        owner_object: JsValue,
        owner_realm: Option<Realm>,
        data: String,
    ) -> JsResult<()> {
        let owner_origin = host_state_origin(&self.host_state.borrow());
        let old_realm = owner_realm.map(|realm| self.context.enter_realm(realm));
        if let Err(error) = self.install_worker_owner_values(owner_object, data) {
            self.record_task_error(format!("[worker message setup] {error}"));
            let cleanup = self.clear_worker_owner_values();
            self.record_error_from("worker message cleanup", cleanup);
            if let Some(old_realm) = old_realm {
                self.context.enter_realm(old_realm);
            }
            return Ok(());
        }
        let result = self.eval(
            &format!(
                "__omoikane_worker_owner_data = __omoikane_decode_worker_message(__omoikane_worker_owner_wire); if (!__omoikane_worker_owner.__terminated) {{ __omoikane_worker_owner.dispatchEvent(new MessageEvent('message', {{ data: __omoikane_worker_owner_data, origin: {owner_origin:?}, source: null, ports: [] }})); }}"
            ),
        );
        let cleanup = self.clear_worker_owner_values();
        self.record_error_from("worker message cleanup", cleanup);
        self.record_error_from("worker message", result);
        if let Some(old_realm) = old_realm {
            self.context.enter_realm(old_realm);
        }
        Ok(())
    }

    fn run_worker_error(
        &mut self,
        worker_id: u64,
        owner: Option<JsValue>,
        owner_realm: Option<Realm>,
        message: String,
    ) -> JsResult<()> {
        let owner_realm = owner_realm.or_else(|| {
            self.host_state
                .borrow()
                .workers
                .get(&worker_id)
                .and_then(|entry| {
                    entry
                        .borrow()
                        .runtime
                        .host_state
                        .borrow()
                        .worker_owner_realm
                        .clone()
                })
        });
        let owner_object = owner.or_else(|| {
            self.host_state
                .borrow()
                .workers
                .get(&worker_id)
                .and_then(|entry| entry.borrow().owner_object.clone())
        });
        let Some(owner_object) = owner_object else {
            return Ok(());
        };
        let old_realm = owner_realm.map(|realm| self.context.enter_realm(realm));
        if let Err(error) = self.install_worker_owner_values(owner_object, message) {
            self.record_task_error(format!("[worker error setup] {error}"));
            let cleanup = self.clear_worker_owner_values();
            self.record_error_from("worker error cleanup", cleanup);
            if let Some(old_realm) = old_realm {
                self.context.enter_realm(old_realm);
            }
            return Ok(());
        }
        let result = self.eval(
            "if (!__omoikane_worker_owner.__terminated) { const event = new Event('error'); event.message = String(__omoikane_worker_owner_wire); event.error = __omoikane_worker_owner_wire; __omoikane_worker_owner.dispatchEvent(event); }",
        );
        let cleanup = self.clear_worker_owner_values();
        self.record_error_from("worker error cleanup", cleanup);
        self.record_error_from("worker error", result);
        if let Some(old_realm) = old_realm {
            self.context.enter_realm(old_realm);
        }
        Ok(())
    }

    fn install_worker_message_values(&mut self, data: String, origin: String) -> JsResult<()> {
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_worker_message_wire"),
            JsValue::from(js_string!(data)),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_worker_message_origin"),
            JsValue::from(js_string!(origin)),
            true,
            &mut self.context,
        )?;
        Ok(())
    }

    fn clear_worker_message_values(&mut self) -> JsResult<()> {
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_worker_message_wire"),
            JsValue::undefined(),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_worker_message_origin"),
            JsValue::undefined(),
            true,
            &mut self.context,
        )?;
        Ok(())
    }

    fn install_worker_owner_values(&mut self, owner: JsValue, data: String) -> JsResult<()> {
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_worker_owner"),
            owner,
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_worker_owner_wire"),
            JsValue::from(js_string!(data)),
            true,
            &mut self.context,
        )?;
        Ok(())
    }

    fn clear_worker_owner_values(&mut self) -> JsResult<()> {
        let global = self.context.global_object();
        global.set(
            js_string!("__omoikane_worker_owner"),
            JsValue::undefined(),
            true,
            &mut self.context,
        )?;
        global.set(
            js_string!("__omoikane_worker_owner_wire"),
            JsValue::undefined(),
            true,
            &mut self.context,
        )?;
        Ok(())
    }

    /// Executes a single timer payload: evaluates a source string, or invokes a
    /// retained function callback with its bound extra arguments.
    fn run_timer_payload(&mut self, payload: TimerPayload) -> JsResult<()> {
        match payload {
            TimerPayload::Realm {
                payload,
                realm,
                document_id,
            } => {
                if !self.iframe_realm_is_live(document_id, &realm) {
                    return Ok(());
                }
                let old_realm = self.context.enter_realm(realm);
                let result = self.run_timer_payload(*payload);
                self.context.enter_realm(old_realm);
                result
            }
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
            TimerPayload::FormSubmission {
                node_id,
                request,
                visit_source,
            } => {
                let frame = self.host_state.borrow().get_node(node_id);
                let Some(frame) = frame else {
                    self.host_state
                        .borrow_mut()
                        .pending_iframe_visits
                        .remove(&node_id);
                    return Ok(());
                };
                let frame_is_active = self.host_state.borrow().node_is_in_active_document(&frame);
                if !frame_is_active {
                    self.host_state
                        .borrow_mut()
                        .pending_iframe_visits
                        .remove(&node_id);
                    return Ok(());
                }
                if let Some(visit_source) = visit_source {
                    self.host_state
                        .borrow_mut()
                        .pending_iframe_visits
                        .insert(node_id, visit_source);
                } else {
                    self.host_state
                        .borrow_mut()
                        .pending_iframe_visits
                        .remove(&node_id);
                }
                let loaded = self
                    .host_state
                    .borrow_mut()
                    .iframe_document_with_submission(&frame, Some(&request));
                if let Err(error) = loaded {
                    self.host_state
                        .borrow_mut()
                        .pending_iframe_visits
                        .remove(&node_id);
                    return Err(JsNativeError::typ().with_message(error.to_string()).into());
                }
                self.run_timer_payload(TimerPayload::ResourceLoad { node_id })
            }
            TimerPayload::ResourceLoad { node_id } => self.run_resource_load_timer(node_id),
            TimerPayload::GeolocationTimeout { request_id } => {
                self.run_geolocation_delivery(request_id, true)
            }
        }
    }

    /// Handles a `TimerPayload::ResourceLoad` timer for a script or iframe node:
    /// runs a parser-inserted script directly; otherwise gathers the node's
    /// pending initial iframe scripts and dynamic script, runs them, and
    /// dispatches the resulting `load` (or `error`) event.
    fn run_resource_load_timer(&mut self, node_id: usize) -> JsResult<()> {
        if self
            .host_state
            .borrow()
            .parser_inserted_scripts
            .contains(&node_id)
        {
            return self.run_written_script(node_id);
        }
        let Some((should_dispatch, initial_scripts, dynamic_script, resource_document_id)) =
            self.gather_resource_load_dispatch_state(node_id)?
        else {
            return Ok(());
        };
        let dispatch_document_id = dynamic_script
            .as_ref()
            .and_then(|(script, _, _, _, _)| document_root_for_node(script))
            .map(|document| document.identity())
            .unwrap_or(resource_document_id);
        let iframe_context = initial_scripts
            .first()
            .and_then(document_root_for_node)
            .map(|document| (node_id, document.identity()));
        if should_dispatch {
            let forgotten = self.eval("__omoikane_forget_discarded_node_wrappers()");
            self.record_error_from("iframe wrapper cleanup", forgotten);
        }
        self.run_initial_iframe_scripts(initial_scripts, iframe_context);
        // A script whose type Omoikane does not execute is not fetched and
        // does not load, so it must not go on to dispatch `load` either.
        let mut dispatch_load = should_dispatch && {
            let state = self.host_state.borrow();
            iframe_context.is_none_or(|(iframe_id, document_id)| {
                state
                    .iframe_documents
                    .get(&iframe_id)
                    .is_some_and(|entry| entry.document.identity() == document_id)
                    && state
                        .get_node(iframe_id)
                        .is_some_and(|frame| state.node_is_in_active_document(&frame))
            })
        };
        let mut dispatch_timing: Option<(String, bool, f64)> = None;
        if let Some(dynamic_script) = dynamic_script {
            let (updated_load, updated_timing) = self.run_dynamic_timer_script(
                node_id,
                dispatch_document_id,
                dynamic_script,
                dispatch_load,
            );
            dispatch_load = updated_load;
            dispatch_timing = updated_timing;
        }
        if dispatch_load {
            self.dispatch_resource_load_event(node_id, dispatch_document_id, dispatch_timing);
        }
        Ok(())
    }

    /// Gathers the initial iframe scripts and dynamic script pending on a
    /// resource-load timer's node, and whether the timer should still
    /// dispatch its `load`/`error` event. Returns `Ok(None)` when the node
    /// (or its document) is no longer live, matching this timer's original
    /// no-op early return.
    fn gather_resource_load_dispatch_state(
        &mut self,
        node_id: usize,
    ) -> JsResult<Option<(bool, Vec<NodeHandle>, Option<DynamicScriptInfo>, usize)>> {
        let mut state = self.host_state.borrow_mut();
        state.pending_resource_loads.remove(&node_id);
        let Some(node) = state.get_node(node_id) else {
            state.pending_iframe_visits.remove(&node_id);
            return Ok(None);
        };
        let resource_document_id = document_root_for_node(&node)
            .map(|document| document.identity())
            .unwrap_or_else(|| state.document.identity());
        if !state.node_is_in_active_document(&node) {
            state.pending_iframe_visits.remove(&node_id);
            Ok(Some((false, Vec::new(), None, resource_document_id)))
        } else {
            let mut initial_scripts: Vec<NodeHandle> = Vec::new();
            // A dynamically inserted external script is classified by
            // the same `type` gate the parsed-document path uses, so a
            // script runs the same way however it reached the tree.
            let dynamic_script = if node
                .tag_name()
                .is_some_and(|tag| tag.eq_ignore_ascii_case("script"))
            {
                let document_id = document_root_for_node(&node)
                    .map(|document| document.identity())
                    .unwrap_or_else(|| state.document.identity());
                node.get_attribute("src").map(|src| {
                    let kind =
                        ScriptKind::from_type_attribute(node.get_attribute("type").as_deref());
                    (
                        node.clone(),
                        src,
                        kind,
                        state.base_url_for_document(document_id),
                        document_id,
                    )
                })
            } else {
                None
            };
            if node.tag_name().is_some_and(|tag| is_nested_frame_tag(&tag)) {
                // A newly connected iframe starts a fresh navigation.
                // This also makes detach/reconnect reload rather than
                // merely replaying the old document's event.
                let previous_document = state
                    .iframe_documents
                    .get(&node_id)
                    .map(|entry| entry.document.identity());
                let document = match state.iframe_content_document(&node) {
                    Ok(document) => document,
                    Err(error) => {
                        state.pending_iframe_visits.remove(&node_id);
                        return Err(JsNativeError::error()
                            .with_message(error.to_string())
                            .into());
                    }
                };
                if previous_document == Some(document.identity()) {
                    // This queued load became a no-op. A plain
                    // contentDocument read must not clear a later
                    // form submission's pending visit source.
                    state.pending_iframe_visits.remove(&node_id);
                }
                let ids = state
                    .iframe_documents
                    .get_mut(&node_id)
                    .map(|entry| std::mem::take(&mut entry.initial_scripts))
                    .unwrap_or_default();
                initial_scripts = ids
                    .into_iter()
                    .filter_map(|id| state.get_node(id))
                    .filter(|script| document_root_for_node(script).as_ref() == Some(&document))
                    .filter(is_inline_classic_script)
                    .collect();
            }
            Ok(Some((
                true,
                initial_scripts,
                dynamic_script,
                resource_document_id,
            )))
        }
    }

    /// Runs each of an iframe's initial (parser-authored) scripts that are
    /// still live, in document order, tolerating a script that removes the
    /// frame or replaces its `Document` partway through.
    fn run_initial_iframe_scripts(
        &mut self,
        initial_scripts: Vec<NodeHandle>,
        iframe_context: Option<(usize, usize)>,
    ) {
        for script in initial_scripts {
            // Like top-level document scripts, one failing initial
            // script must not prevent later scripts or the iframe load
            // event from running.
            let (sandbox_allowed, csp_allowed) = {
                let mut state = self.host_state.borrow_mut();
                // Earlier scripts can remove the frame or replace its
                // Document. Never run the remaining old script nodes.
                if !state.node_is_in_active_document(&script)
                    || !state.started_inserted_scripts.insert(script.identity())
                {
                    continue;
                }
                (
                    state.sandbox_allows_scripts_for_node(&script),
                    state
                        .csp_policy_for_node(&script)
                        .allows_inline(ResourceType::Script),
                )
            };
            if !sandbox_allowed {
                continue;
            }
            if !csp_allowed {
                self.host_state.borrow_mut().record_csp_violation_for_node(
                    &script,
                    ResourceType::Script,
                    "inline",
                );
                continue;
            }
            #[cfg(test)]
            if let Some(document) = document_root_for_node(&script) {
                *self
                    .host_state
                    .borrow_mut()
                    .document_script_executions
                    .entry(document.identity())
                    .or_default() += 1;
            }
            let source = collect_text_content(&script);
            let result = if let Some((iframe_id, document_id)) = iframe_context {
                self.eval_iframe_script(iframe_id, document_id, script.identity(), &source)
            } else {
                self.eval(&source).and_then(|_| self.run_jobs())
            };
            self.record_error_from("iframe inline script", result);
        }
    }

    /// Fetches (or skips, per sandbox/CSP) and runs a dynamically inserted
    /// script for a resource-load timer, returning the updated
    /// dispatch-`load` flag and the resource-timing data for its `load` or
    /// `error` event.
    fn run_dynamic_timer_script(
        &mut self,
        node_id: usize,
        dispatch_document_id: usize,
        dynamic_script: DynamicScriptInfo,
        mut dispatch_load: bool,
    ) -> (bool, Option<(String, bool, f64)>) {
        let (script_node, src, kind, base_url, document_id) = dynamic_script;
        let mut dispatch_timing: Option<(String, bool, f64)> = None;
        let _script_document = activate_module_document(&self.host_state, document_id);
        let log_scripts = std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some();
        if kind == ScriptKind::NotExecutable {
            if log_scripts {
                eprintln!("[omoikane][script] skipped dynamic {src}");
            }
            dispatch_load = false;
        } else if !self
            .host_state
            .borrow()
            .sandbox_allows_scripts_for_node(&script_node)
        {
            if log_scripts {
                eprintln!("[omoikane][script] blocked dynamic {src} by iframe sandbox");
            }
            let timing_name = resource_reference_timing_name(&src, base_url.as_ref());
            let dispatched = self.eval_in_document_realm(
                dispatch_document_id,
                &dispatch_resource_timing_script("error", node_id, &timing_name, false, 0.0),
            );
            self.record_error_from(&src, dispatched);
            dispatch_load = false;
        } else if !self
            .host_state
            .borrow()
            .csp_policy_for_node(&script_node)
            .allows_reference(ResourceType::Script, &src)
        {
            self.host_state.borrow_mut().record_csp_violation_for_node(
                &script_node,
                ResourceType::Script,
                &src,
            );
            if log_scripts {
                eprintln!("[omoikane][script] blocked dynamic {src} by CSP");
            }
            dispatch_load = false;
        } else {
            if log_scripts {
                eprintln!("[omoikane][script] loading dynamic {src} kind={kind:?}");
            }
            let timing_name = resource_reference_timing_name(&src, base_url.as_ref());
            let fetch_start = std::time::Instant::now();
            let fetched = {
                let mut state = self.host_state.borrow_mut();
                fetch_script_resource_with_client(&src, base_url.as_ref(), &mut state.http_client)
            };
            let elapsed_ms = fetch_start.elapsed().as_secs_f64() * 1_000.0;
            match fetched {
                // Every failure below is the page's, not the engine's:
                // it is recorded and execution continues, exactly as
                // `execute_document_scripts` treats a parsed script.
                // Propagating instead would abort the event loop and,
                // through it, the whole navigation.
                None => {
                    // A script that never arrived did not load: it
                    // fires `error` instead, so a loader waiting on
                    // one of the two is not left with neither.
                    self.record_task_error(format!("[dynamic script: {src}] failed to fetch"));
                    dispatch_load = false;
                    let dispatched = self.eval_in_document_realm(
                        dispatch_document_id,
                        &dispatch_resource_timing_script(
                            "error",
                            node_id,
                            &timing_name,
                            false,
                            elapsed_ms,
                        ),
                    );
                    self.record_error_from(&src, dispatched);
                }
                Some((effective_url, _source, redirect_count))
                    if !self
                        .host_state
                        .borrow()
                        .csp_policy_for_node(&script_node)
                        .allows_reference_after_redirects(
                            ResourceType::Script,
                            &effective_url,
                            redirect_count,
                        ) =>
                {
                    let redirected =
                        resource_reference_was_redirected(&src, &effective_url, base_url.as_ref());
                    self.host_state.borrow_mut().record_csp_violation_for_node(
                        &script_node,
                        ResourceType::Script,
                        effective_url.clone(),
                    );
                    dispatch_load = false;
                    let dispatched = self.eval_in_document_realm(
                        dispatch_document_id,
                        &dispatch_resource_timing_script(
                            "error",
                            node_id,
                            &effective_url,
                            redirected,
                            elapsed_ms,
                        ),
                    );
                    self.record_error_from(&src, dispatched);
                }
                Some((effective_url, source, _redirect_count)) => {
                    let redirected =
                        resource_reference_was_redirected(&src, &effective_url, base_url.as_ref());
                    dispatch_timing = Some((effective_url, redirected, elapsed_ms));
                    let result = match kind {
                        ScriptKind::Module => {
                            self.eval_module_in_document_realm_timed(
                                dispatch_document_id,
                                script_node.identity(),
                                &source,
                                &module_script_url(&src, base_url.as_ref(), false),
                                document_root_for_node(&script_node)
                                    .unwrap_or_else(|| self.document()),
                            )
                            .0
                        }
                        _ => self
                            .eval_script_in_document_realm(
                                dispatch_document_id,
                                script_node.identity(),
                                &source,
                            )
                            .map(|_| JsValue::undefined())
                            .map_err(JsEvaluationError::JavaScript),
                    };
                    if let Err(error) = result {
                        let context = script_source_context(&source);
                        self.record_task_error(format!(
                            "[dynamic script: {src}; {context}] {error}"
                        ));
                    }
                    if log_scripts {
                        eprintln!("[omoikane][script] completed dynamic {src}");
                    }
                }
            }
        }
        (dispatch_load, dispatch_timing)
    }

    /// Dispatches a resource-load timer's `load` event: refreshes any
    /// iframe window names, fires an iframe window `load` listener when one
    /// is wired (or an inline `body`/`frameset` `onload` needs a realm), and
    /// dispatches the node's own resource-timing `load` event.
    fn dispatch_resource_load_event(
        &mut self,
        node_id: usize,
        dispatch_document_id: usize,
        dispatch_timing: Option<(String, bool, f64)>,
    ) {
        self.refresh_window_names_after_iframe_load(node_id, dispatch_document_id);
        let child_document_id = {
            let state = self.host_state.borrow();
            state
                .get_node(node_id)
                .filter(|node| {
                    node.tag_name()
                        .is_some_and(|name| is_nested_frame_tag(&name))
                })
                .and_then(|_| state.iframe_documents.get(&node_id))
                .and_then(|entry| {
                    // A Window load listener requires a live Realm. An inline
                    // body/frameset onload attribute is the one exception: it
                    // needs a Realm to be wired before dispatch.
                    let has_inline_load = || {
                        ["body", "frameset"].iter().any(|tag| {
                            entry
                                .document
                                .query_selector(tag)
                                .is_some_and(|node| node.get_attribute("onload").is_some())
                        })
                    };
                    (entry.realm.is_some() || has_inline_load())
                        .then_some(entry.document.identity())
                })
        };
        if let Some(document_id) = child_document_id {
            match self.realm_for_document(document_id) {
                Ok(_) => {
                    let dispatched = self.eval_in_document_realm(
                        document_id,
                        &format!("__omoikane_wire_inline_handlers(); {LOAD_SCRIPT}"),
                    );
                    self.record_error_from("iframe window load", dispatched);
                }
                Err(error) => self.record_task_error(format!("[iframe window load] {error}")),
            }
        }
        let (timing_name, redirected, elapsed_ms) =
            dispatch_timing.unwrap_or_else(|| (String::new(), false, 0.0));
        let dispatched = self.eval_in_document_realm(
            dispatch_document_id,
            &dispatch_resource_timing_script("load", node_id, &timing_name, redirected, elapsed_ms),
        );
        self.record_error_from("resource load", dispatched);
    }

    fn refresh_window_names_after_iframe_load(&mut self, node_id: usize, document_id: usize) {
        let is_iframe = self
            .host_state
            .borrow()
            .get_node(node_id)
            .is_some_and(|node| {
                node.tag_name()
                    .is_some_and(|name| is_nested_frame_tag(&name))
            });
        if !is_iframe {
            return;
        }
        let refreshed = self
            .eval_in_document_realm(document_id, "__omoikane_install_window_named_properties()");
        self.record_error_from("iframe named properties", refreshed);
    }

    /// Dispatches a `DOMContentLoaded` event on the document.
    ///
    /// Call this after the DOM tree is fully constructed (e.g., after parsing HTML
    /// and executing inline scripts). Listeners registered via
    /// `document.addEventListener('DOMContentLoaded', fn)` will be invoked.
    pub fn fire_dom_content_loaded(&mut self) -> JsResult<()> {
        self.run_written_scripts(true)?;
        self.eval(DOM_CONTENT_LOADED_SCRIPT)?;
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
        self.eval(LOAD_SCRIPT)?;
        self.run_jobs()
    }

    fn record_document_script_failure(&self, code: &'static str) {
        report_safe_javascript_failure(self.host_state.borrow().error_reporter.clone(), code, None);
    }

    fn record_module_failure(&self, code: &'static str, operation: &'static str) {
        report_safe_worker_or_module_failure(
            self.host_state.borrow().error_reporter.clone(),
            ErrorCategory::Module,
            code,
            operation,
        );
    }

    pub(crate) fn report_paint_failure(&self, code: &'static str) {
        let destination = self.host_state.borrow().error_reporter.clone();
        let Some((reporter, surface)) = destination else {
            return;
        };
        reporter.report(
            RawEvent::new(
                ErrorCategory::Paint,
                ErrorSeverity::Error,
                ErrorCode::new(code).expect("static code"),
                surface,
                "Paint failed",
                &[("operation", "render"), ("resource", "other")],
            )
            .sanitize(),
        );
    }

    fn record_js_task_failure(&self, code: &'static str, kind: &'static str) {
        report_safe_javascript_failure(
            self.host_state.borrow().error_reporter.clone(),
            code,
            Some(kind),
        );
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
            self.host_state.borrow_mut().set_main_base_url(base.clone());
        }
        if self
            .eval("__omoikane_install_window_named_properties()")
            .is_err()
        {
            self.record_document_script_failure("DOCUMENT_SCRIPT_INITIALIZATION_FAILED");
        }
        if self.eval("document.__readyState = 'loading'").is_err() {
            self.record_document_script_failure("DOCUMENT_SCRIPT_INITIALIZATION_FAILED");
        }

        let document = self.document();
        let scripts = collect_script_elements(&document);
        let mut errors = Vec::new();
        let mut deferred = Vec::new();
        let log_scripts = std::env::var_os("OMOIKANE_LOG_SCRIPTS").is_some();

        for (script_index, script) in scripts.iter().enumerate() {
            let Some((is_module, is_defer, has_src, source_code, script_label)) = self
                .fetch_or_inline_document_script_source(
                    script,
                    script_index,
                    base_url,
                    log_scripts,
                    &mut errors,
                )
            else {
                continue;
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
            self.run_document_script_now(
                script,
                &source_code,
                &script_label,
                log_scripts,
                &mut errors,
            );
        }

        // Execute deferred scripts. Each runs with its own insertion point set
        // to its <script> element, so a `document.write` from a deferred script
        // lands as that script's following siblings — the same treatment the
        // inline path applies above.
        self.run_deferred_document_scripts(deferred, log_scripts, &mut errors);

        self.sync_module_csp_violations();

        // Fire DOMContentLoaded
        if let Err(err) = self.fire_dom_content_loaded() {
            self.record_document_script_failure("DOCUMENT_SCRIPT_INITIALIZATION_FAILED");
            errors.push(format!("{err}"));
        }

        errors
    }
    /// Classifies one `<script>` element, enforces its CSP policy, and
    /// resolves its source: fetched for an external script, or its text
    /// content for an inline one. Returns `None` (after recording any CSP
    /// violation or fetch failure) when the script must not run at all.
    fn fetch_or_inline_document_script_source(
        &mut self,
        script: &NodeHandle,
        script_index: usize,
        base_url: Option<&crate::http::Url>,
        log_scripts: bool,
        errors: &mut Vec<String>,
    ) -> Option<(bool, bool, bool, String, String)> {
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
        if !is_module && !is_executable_classic_script_type(attrs.get("type").map(|s| s.as_str())) {
            if log_scripts {
                eprintln!(
                    "[omoikane][script] skipped type={:?} src={:?}",
                    attrs.get("type"),
                    attrs.get("src")
                );
            }
            return None;
        }

        let src = attrs.get("src").cloned();
        let has_src = src.is_some();
        // HTML spec: defer only applies to external (src) scripts.
        let is_defer = is_module || (attrs.contains_key("defer") && src.is_some());

        let policy = self.host_state.borrow().csp_policy_for_node(script);
        if let Some(src_url) = src.as_deref() {
            if !policy.allows_reference(ResourceType::Script, src_url) {
                self.host_state.borrow_mut().record_csp_violation_for_node(
                    script,
                    ResourceType::Script,
                    src_url,
                );
                return None;
            }
        } else if !policy.allows_inline(ResourceType::Script) {
            self.host_state.borrow_mut().record_csp_violation_for_node(
                script,
                ResourceType::Script,
                "inline",
            );
            return None;
        }

        let (source_code, script_label) = if let Some(src_url) = src {
            // External script: fetch
            let fetch_start = std::time::Instant::now();
            let fetched = {
                let mut state = self.host_state.borrow_mut();
                fetch_script_resource_with_client(&src_url, base_url, &mut state.http_client)
            };
            match fetched {
                Some((effective_url, code, redirect_count)) => {
                    if !policy.allows_reference_after_redirects(
                        ResourceType::Script,
                        &effective_url,
                        redirect_count,
                    ) {
                        self.host_state.borrow_mut().record_csp_violation_for_node(
                            script,
                            ResourceType::Script,
                            effective_url,
                        );
                        return None;
                    }
                    let redirected =
                        resource_reference_was_redirected(&src_url, &effective_url, base_url);
                    let elapsed_ms = fetch_start.elapsed().as_secs_f64() * 1_000.0;
                    let _ = self.eval(&format!(
                            "__omoikane_record_resource_timing({}, 'script', 200, false, {redirected}, {elapsed_ms})",
                            serde_json::to_string(&effective_url)
                                .unwrap_or_else(|_| "\"\"".to_string()),
                        ));
                    if log_scripts {
                        eprintln!(
                            "[omoikane][script] fetched {src_url} elapsed_ms={:.3}",
                            elapsed_ms,
                        );
                    }
                    (code, src_url.clone())
                }
                None => {
                    let timing_name = resource_reference_timing_name(&src_url, base_url);
                    let elapsed_ms = fetch_start.elapsed().as_secs_f64() * 1_000.0;
                    let _ = self.eval(&format!(
                            "__omoikane_record_resource_timing({}, 'script', 0, true, false, {elapsed_ms})",
                            serde_json::to_string(&timing_name)
                                .unwrap_or_else(|_| "\"\"".to_string()),
                        ));
                    errors.push(format!("failed to fetch script: {src_url}"));
                    return None;
                }
            }
        } else {
            // Inline script: collect text content
            (
                collect_text_content(script),
                format!("inline-script-{}", script_index + 1),
            )
        };
        Some((is_module, is_defer, has_src, source_code, script_label))
    }

    /// Runs one non-deferred document script immediately: wires up
    /// `document.write`'s insertion point and `document.currentScript`,
    /// evaluates the script and its jobs, and records any failure.
    fn run_document_script_now(
        &mut self,
        script: &NodeHandle,
        source_code: &str,
        script_label: &str,
        log_scripts: bool,
        errors: &mut Vec<String>,
    ) {
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
        let script_context = script_source_context(source_code);
        let (eval_result, parse_elapsed, compile_elapsed, execute_elapsed) =
            self.eval_safe_timed(source_code);
        if let Err(err) = eval_result {
            self.record_document_script_failure("DOCUMENT_SCRIPT_EVALUATION_FAILED");
            errors.push(format!("[script: {script_label}; {script_context}] {err}"));
        }
        let jobs_start = std::time::Instant::now();
        let jobs_result = self.run_jobs();
        let jobs_elapsed = jobs_start.elapsed();
        if let Err(err) = jobs_result {
            self.record_document_script_failure("DOCUMENT_SCRIPT_JOBS_FAILED");
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

    /// Runs every deferred (or module) document script, in order, each with
    /// its own `document.write` insertion point and `currentScript`, exactly
    /// as the immediate-execution path above.
    fn run_deferred_document_scripts(
        &mut self,
        deferred: Vec<(String, NodeHandle, String, Option<String>)>,
        log_scripts: bool,
        errors: &mut Vec<String>,
    ) {
        for (source_code, script, script_label, module_url) in deferred {
            if let Err(error) = self.run_written_scripts_before(&script) {
                self.record_document_script_failure("DOCUMENT_SCRIPT_EVALUATION_FAILED");
                errors.push(format!("[written scripts] {error}"));
            }
            if log_scripts {
                eprintln!("[omoikane][script] running deferred {script_label}");
            }
            self.host_state.borrow_mut().write_insertion_ref = Some(script.clone());
            let _ = self.eval(&format!(
                "__omoikane_set_current_script({})",
                script.identity()
            ));
            let script_context = script_source_context(&source_code);
            let is_module = module_url.is_some();
            let (result, parse_elapsed, compile_elapsed, execute_elapsed) =
                if let Some(module_url) = module_url {
                    let module_document =
                        document_root_for_node(&script).unwrap_or_else(|| self.document());
                    let (result, parse_elapsed, execute_elapsed) =
                        self.eval_module_timed(&source_code, &module_url, module_document);
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
                if is_module {
                    self.record_module_failure("DOCUMENT_MODULE_EVALUATION_FAILED", "execute");
                } else {
                    self.record_document_script_failure("DOCUMENT_SCRIPT_EVALUATION_FAILED");
                }
                errors.push(format!("[script: {script_label}; {script_context}] {err}"));
            }
            let jobs_start = std::time::Instant::now();
            let jobs_result = self.run_jobs();
            let jobs_elapsed = jobs_start.elapsed();
            if let Err(err) = jobs_result {
                self.record_document_script_failure("DOCUMENT_SCRIPT_JOBS_FAILED");
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
    }

    fn with_active_host<T>(&mut self, f: impl FnOnce(&mut Context) -> JsResult<T>) -> JsResult<T> {
        self.with_active_host_value(f)
    }

    fn with_active_host_value<T>(&mut self, f: impl FnOnce(&mut Context) -> T) -> T {
        let _guard = activate_host_state(Rc::clone(&self.host_state));
        self.host_state.borrow_mut().sweep_node_lifetimes();
        let deadline = execution_deadline(self.sandbox.timeout);
        let mut context = self.context.enter_runtime_deadline(deadline);
        f(&mut context)
    }
}

impl Drop for JsRuntime {
    fn drop(&mut self) {
        // WorkerRuntime keeps an owner/worker Rc cycle until the owner map is
        // cleared. Do that before the RootProvider and host state fields drop,
        // so a later collection cannot trace a stale worker realm.
        let _guard = activate_host_state(Rc::clone(&self.host_state));
        self.terminate_workers();

        let (storage_manager, clients) = {
            let mut state = self.host_state.borrow_mut();
            (
                state.storage_manager.clone(),
                std::mem::take(&mut state.web_lock_clients)
                    .into_values()
                    .collect::<Vec<_>>(),
            )
        };
        for client_id in clients {
            unregister_web_lock_client(&storage_manager, client_id);
        }
        flush_web_lock_notifications();

        // Iframe realms and queued callbacks are explicit Boa roots held by
        // HostState. Release them while the Context is still alive.  Dropping
        // these roots after Context teardown leaves Boa's generational
        // collector with realm/JsValue handles whose owner has already gone
        // away; this is observable when two independent runtimes are created
        // and collected in one process (the Acid3 harness does exactly that).
        let mut state = self.host_state.borrow_mut();
        for target in state.document_targets.drain().map(|(_, target)| target) {
            if let Some(target) = target.upgrade() {
                target.set_user_action_state("target", false);
            }
        }
        state.iframe_documents.clear();
        state.iframe_context_ids.clear();
        state.node_lifetimes = node_lifetime::NodeLifetimes::default();
        state.event_loop = EventLoop::default();
        state.pending_resource_loads.clear();
        state.worker_owner_realm = None;
        state.main_realm = None;
        state.write_parsers.clear();
        state.written_script_queue.clear();
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
    crate::dom::collect_descendant_text(node, crate::dom::TextTraversal::All)
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

/// Returns whether a fetched resource ended at a different URL than the one
/// requested by the document. Relative references are resolved against the
/// document base before comparing parsed HTTP(S) URLs, so a relative script
/// does not look redirected merely because the fetch helper returns an
/// absolute effective URL.
fn resource_reference_was_redirected(
    requested: &str,
    effective: &str,
    base_url: Option<&crate::http::Url>,
) -> bool {
    let requested = match resolve_resource_ref(requested, base_url) {
        Some(ResolvedResource::Url(url)) => url,
        _ => requested.to_string(),
    };
    match (
        requested.parse::<crate::http::Url>(),
        effective.parse::<crate::http::Url>(),
    ) {
        (Ok(requested), Ok(effective)) => requested != effective,
        _ => requested != effective,
    }
}

fn resource_reference_timing_name(requested: &str, base_url: Option<&crate::http::Url>) -> String {
    match resolve_resource_ref(requested, base_url) {
        Some(ResolvedResource::Url(url)) => url,
        _ => requested.to_string(),
    }
}

fn dispatch_resource_timing_script(
    event: &str,
    node_id: usize,
    url: &str,
    redirected: bool,
    elapsed_ms: f64,
) -> String {
    let safe_url = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".to_string());
    let safe_elapsed = if elapsed_ms.is_finite() && elapsed_ms >= 0.0 {
        elapsed_ms
    } else {
        0.0
    };
    format!(
        "__omoikane_dispatch_resource_{event}({node_id}, {safe_url}, {redirected}, {safe_elapsed})"
    )
}

fn same_origin_url(a: &crate::http::Url, b: &crate::http::Url) -> bool {
    a.scheme().eq_ignore_ascii_case(b.scheme())
        && a.host().eq_ignore_ascii_case(b.host())
        && a.port() == b.port()
}

fn host_state_origin(state: &HostState) -> String {
    state
        .base_url
        .as_ref()
        .map(|url| format!("{}://{}", url.scheme(), url.authority()))
        .unwrap_or_default()
}

fn resolve_worker_url(
    requested: &str,
    owner_url: &str,
    base_url: Option<&crate::http::Url>,
) -> JsResult<String> {
    if requested
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
    {
        return Ok(requested.to_string());
    }
    let owner = owner_url.parse::<crate::http::Url>().map_err(|_| {
        JsError::from(JsNativeError::error().with_message("Worker owner has no origin"))
    })?;
    let resolved = if requested
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || requested
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        requested.parse::<crate::http::Url>().map_err(|_| {
            JsError::from(JsNativeError::syntax().with_message("Invalid Worker URL"))
        })?
    } else {
        let base = base_url.ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("Worker URL cannot be resolved"))
        })?;
        crate::http::url::resolve_url(base, requested).map_err(|_| {
            JsError::from(JsNativeError::syntax().with_message("Invalid Worker URL"))
        })?
    };
    if !same_origin_url(&owner, &resolved) {
        return Err(JsNativeError::error()
            .with_message("Dedicated Worker must be same-origin")
            .into());
    }
    Ok(resolved.to_string())
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
    fetch_script_resource_with_client(src, base_url, &mut Client::new())
        .map(|(_, source, _)| source)
}

/// Fetches an external script and retains its effective URL and redirect count.
///
/// The effective URL matters for CSP: an initially permitted script may follow
/// a redirect into a source that the policy does not permit, in which case its
/// bytes must never reach the evaluator.
fn fetch_script_resource_with_client(
    src: &str,
    base_url: Option<&crate::http::Url>,
    client: &mut Client,
) -> Option<(String, String, usize)> {
    match resolve_resource_ref(src, base_url)? {
        // data: URI scripts are decoded inline without a network fetch. Only
        // JavaScript media types are executed as classic scripts; any other
        // media type is treated as a fetch failure (returns None), matching how
        // browsers refuse to run non-script `data:` sources.
        ResolvedResource::Data { mime_type, data } => {
            if !is_javascript_mime_type(&mime_type) {
                return None;
            }
            Some((src.to_string(), String::from_utf8(data).ok()?, 0))
        }
        ResolvedResource::Url(url) => {
            let resolved = url.parse::<crate::http::Url>().ok()?;
            let public_only = requires_public_fetch(&resolved, base_url);
            let mut request = crate::http::HttpRequest::new(crate::http::Method::Get, resolved);
            if public_only {
                request.require_public_ip();
            }
            if let Some(site) = base_url {
                request.set_cookie_context(site.clone(), false);
            }
            let response = client.send(request).ok()?;
            // Scripts require a successful (200) response; error pages are not
            // executed. This differs deliberately from iframe loading, which
            // adopts even an error response's body as the sub-document.
            if response.status_code() != 200 {
                client.report_resource_failure();
                return None;
            }
            let effective_url = response
                .effective_url()
                .map(ToString::to_string)
                .unwrap_or(url);
            let source = match std::str::from_utf8(response.body()) {
                Ok(source) => source.to_string(),
                Err(_) => {
                    client.report_resource_failure();
                    return None;
                }
            };
            Some((effective_url, source, response.redirect_count()))
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

/// Initializes the iframe Realm for both script execution and synchronous
/// same-origin WindowProxy access. Bootstrap itself queues no Promise jobs;
/// callers retain their existing microtask checkpoint instead of running the
/// parent's pending jobs during a nested property lookup.
fn ensure_iframe_realm(
    context: &mut Context,
    host_state: &Rc<RefCell<HostState>>,
    iframe_id: usize,
    document_id: usize,
) -> JsResult<Realm> {
    if let Some(realm) = host_state
        .borrow()
        .iframe_documents
        .get(&iframe_id)
        .filter(|entry| entry.document.identity() == document_id)
        .and_then(|entry| entry.realm.clone())
    {
        return Ok(realm);
    }

    let (document_url, same_origin, owner_document_id) = {
        let state = host_state.borrow();
        let entry = state
            .iframe_documents
            .get(&iframe_id)
            .filter(|entry| entry.document.identity() == document_id)
            .ok_or_else(|| {
                JsNativeError::reference().with_message("iframe document is no longer live")
            })?;
        let sandbox = state
            .document_sandbox
            .get(&document_id)
            .copied()
            .unwrap_or_default();
        let child_origin = state.document_origins.get(&document_id).cloned().flatten();
        let owner_document = owner_document_for_node(
            &state
                .get_node(iframe_id)
                .ok_or_else(|| JsNativeError::reference().with_message("iframe is detached"))?,
        )
        .ok_or_else(|| JsNativeError::reference().with_message("iframe owner is detached"))?;
        let owner_origin = state
            .document_origins
            .get(&owner_document.identity())
            .cloned()
            .flatten();
        // A non-sandboxed iframe is not automatically same-origin: the
        // effective child origin must match the embedding Document's
        // origin.  `about:blank` inherits that origin when it is loaded,
        // while opaque resources (for example `data:`) deliberately carry
        // `None` and can never match it.
        let same_origin = (!sandbox.active || sandbox.allow_same_origin)
            && child_origin.is_some()
            && child_origin == owner_origin;
        (
            entry.document_url.clone(),
            same_origin,
            owner_document.identity(),
        )
    };

    let caller_global = context.global_object();
    let top = caller_global.get(js_string!("top"), context)?;
    let top_global: JsValue = if top.is_object() {
        top
    } else {
        caller_global.into()
    };
    // A nested same-origin frame's `parent` is the owning browsing
    // context's global, not always the top-level global. Capture that
    // Realm's global before entering the new child Realm. The owning
    // Realm already exists whenever a nested frame is created by its
    // parent script; if it does not, retain the top-level fallback so a
    // detached/host-created frame still has a usable parent object.
    let parent_global =
        if same_origin && owner_document_id != host_state.borrow().document.identity() {
            let owner_realm = host_state
                .borrow()
                .iframe_documents
                .values()
                .find(|entry| entry.document.identity() == owner_document_id)
                .and_then(|entry| entry.realm.clone());
            if let Some(owner_realm) = owner_realm {
                let old_realm = context.enter_realm(owner_realm);
                let global: JsValue = context.global_object().into();
                context.enter_realm(old_realm);
                global
            } else {
                top_global.clone()
            }
        } else {
            top_global.clone()
        };
    let realm = context.create_realm()?;
    realm
        .host_defined_mut()
        .insert(ModuleDocumentId(document_id));
    realm
        .host_defined_mut()
        .insert(realm_origin_snapshot(host_state, document_id)?);
    let old_realm = context.enter_realm(realm.clone());
    let previous_resolver = host_state.borrow().canonical_node_identity_resolver.clone();
    let setup = (|| {
        let mut bindings = register_host_bindings(context, host_state)?;
        bindings.value(
            js_string!("__omoikane_document_id"),
            JsValue::from(document_id as f64),
            context,
        )?;
        bindings.value(
            js_string!("__omoikane_location_href"),
            JsValue::from(js_string!(document_url.as_str())),
            context,
        )?;
        bindings.value(
            js_string!("__omoikane_frame_element_id"),
            if same_origin {
                JsValue::from(iframe_id as f64)
            } else {
                JsValue::null()
            },
            context,
        )?;
        // A module has a private lexical environment; nested Script evaluation
        // would inherit the active bootstrap environment in Boa.
        evaluate_dom_bootstrap(context, host_state, &bindings)?;

        let (parent, top) = if same_origin {
            (parent_global, top_global.clone())
        } else {
            let factory = context
                .global_object()
                .get(js_string!("__omoikane_cross_origin_window"), context)?;
            let factory = factory.as_callable().ok_or_else(|| {
                JsNativeError::typ().with_message("cross-origin Window proxy is unavailable")
            })?;
            let create = |document_id, context: &mut Context| {
                factory.call(
                    &JsValue::undefined(),
                    &[JsValue::from(document_id as f64)],
                    context,
                )
            };
            let parent = create(owner_document_id, context)?;
            let top_document_id = host_state.borrow().document.identity();
            let top = if top_document_id == owner_document_id {
                parent.clone()
            } else {
                create(top_document_id, context)?
            };
            (parent, top)
        };
        let global = context.global_object();
        global.set(js_string!("parent"), parent.clone(), true, context)?;
        global.set(js_string!("top"), top, true, context)?;
        context.eval(Source::from_bytes(
            "__omoikane_install_window_named_properties()",
        ))?;
        Ok::<(), JsError>(())
    })();
    context.enter_realm(old_realm);
    host_state.borrow_mut().canonical_node_identity_resolver = previous_resolver;
    if let Err(error) = setup {
        host_state
            .borrow_mut()
            .input_bridge
            .retire_document(document_id);
        host_state
            .borrow_mut()
            .form_state
            .retire_document(document_id);
        host_state
            .borrow_mut()
            .iframe_navigation
            .retire_document(document_id);
        host_state
            .borrow_mut()
            .form_validation
            .retire_document(document_id);
        return Err(error);
    }

    let mut state = host_state.borrow_mut();
    let entry = state
        .iframe_documents
        .get_mut(&iframe_id)
        .filter(|entry| entry.document.identity() == document_id)
        .ok_or_else(|| JsNativeError::reference().with_message("iframe document was replaced"))?;
    entry.realm = Some(realm.clone());
    let document = entry.document.clone();
    drop(state);
    let viewport = host_state
        .borrow_mut()
        .visual_viewport_for_document(&document);
    host_state
        .borrow_mut()
        .observed_iframe_viewports
        .insert(document_id, (viewport.width, viewport.height));
    form_state::restore_pending_document(host_state, document_id, context)?;
    Ok(realm)
}

/// Initializes a popup without allocating an iframe node in its opener DOM.
fn ensure_auxiliary_realm(
    context: &mut Context,
    host_state: &Rc<RefCell<HostState>>,
    auxiliary_id: u64,
) -> JsResult<Realm> {
    let (document_id, document_url, opener_document_id, existing) = {
        let state = host_state.borrow();
        let entry = state.auxiliary_contexts.get(&auxiliary_id).ok_or_else(|| {
            JsNativeError::reference().with_message("auxiliary context is closed")
        })?;
        (
            entry.document.identity(),
            entry.document_url.clone(),
            entry.opener_document_id,
            entry.realm.clone(),
        )
    };
    if let Some(realm) = existing {
        return Ok(realm);
    }

    let same_origin = {
        let state = host_state.borrow();
        match (
            state.document_security_origins.get(&document_id),
            state.document_security_origins.get(&opener_document_id),
        ) {
            (Some(document), Some(opener)) => document == opener,
            _ => false,
        }
    };

    let opener = if context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .is_some_and(|owner| owner.0 == opener_document_id)
    {
        context.global_object().into()
    } else {
        let owner_realm = {
            let state = host_state.borrow();
            if opener_document_id == state.document.identity() {
                state.main_realm.clone()
            } else {
                state
                    .iframe_documents
                    .values()
                    .find(|entry| entry.document.identity() == opener_document_id)
                    .and_then(|entry| entry.realm.clone())
                    .or_else(|| {
                        state
                            .auxiliary_contexts
                            .values()
                            .find(|entry| entry.document.identity() == opener_document_id)
                            .and_then(|entry| entry.realm.clone())
                    })
            }
        };
        let owner_realm = owner_realm.ok_or_else(|| {
            JsNativeError::reference().with_message("popup opener is no longer live")
        })?;
        let previous = context.enter_realm(owner_realm);
        let global: JsValue = context.global_object().into();
        context.enter_realm(previous);
        global
    };
    let realm = context.create_realm()?;
    realm
        .host_defined_mut()
        .insert(ModuleDocumentId(document_id));
    realm
        .host_defined_mut()
        .insert(realm_origin_snapshot(host_state, document_id)?);
    let previous = context.enter_realm(realm.clone());
    let previous_resolver = host_state.borrow().canonical_node_identity_resolver.clone();
    let setup = (|| -> JsResult<()> {
        let mut bindings = register_host_bindings(context, host_state)?;
        bindings.value(
            js_string!("__omoikane_document_id"),
            JsValue::from(document_id as f64),
            context,
        )?;
        bindings.value(
            js_string!("__omoikane_location_href"),
            JsValue::from(js_string!(document_url.as_str())),
            context,
        )?;
        bindings.value(
            js_string!("__omoikane_frame_element_id"),
            JsValue::null(),
            context,
        )?;
        bindings.value(
            js_string!("__omoikane_auxiliary_context_id"),
            JsValue::from(auxiliary_id as f64),
            context,
        )?;
        evaluate_dom_bootstrap(context, host_state, &bindings)?;
        let opener = if same_origin {
            opener
        } else {
            let factory = context
                .global_object()
                .get(js_string!("__omoikane_cross_origin_window"), context)?;
            let factory = factory.as_callable().ok_or_else(|| {
                JsNativeError::typ().with_message("cross-origin opener proxy is unavailable")
            })?;
            factory.call(
                &JsValue::undefined(),
                &[JsValue::from(opener_document_id as f64)],
                context,
            )?
        };
        context
            .global_object()
            .set(js_string!("opener"), opener, true, context)?;
        Ok(())
    })();
    context.enter_realm(previous);
    host_state.borrow_mut().canonical_node_identity_resolver = previous_resolver;
    setup?;
    let mut state = host_state.borrow_mut();
    let entry = state
        .auxiliary_contexts
        .get_mut(&auxiliary_id)
        .ok_or_else(|| JsNativeError::reference().with_message("auxiliary context was closed"))?;
    entry.realm = Some(realm.clone());
    Ok(realm)
}

fn register_private_callable(
    context: &mut Context,
    bindings: &mut BootstrapBindings,
    name: JsString,
    length: usize,
    function: NativeFunction,
) -> JsResult<()> {
    bindings.callable(name, length, function, true, context)
}

fn register_private_builtin_callable(
    context: &mut Context,
    bindings: &mut BootstrapBindings,
    name: JsString,
    length: usize,
    function: NativeFunction,
) -> JsResult<()> {
    bindings.callable(name, length, function, false, context)
}

fn register_private_property(
    context: &mut Context,
    bindings: &mut BootstrapBindings,
    name: JsString,
    value: impl Into<JsValue>,
    _attributes: boa_engine::property::Attribute,
) -> JsResult<()> {
    bindings.value(name, value, context)
}

fn register_host_bindings(
    context: &mut Context,
    host_state: &Rc<RefCell<HostState>>,
) -> JsResult<BootstrapBindings> {
    let mut bindings = BootstrapBindings::new();
    let document_id = context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .expect("document realm has an identity")
        .0;
    host_state
        .borrow_mut()
        .bootstrap_bindings
        .insert(document_id, bindings.object.clone());
    font_loading::register(context, host_state, &mut bindings)?;
    pointer_lock::register(context, &mut bindings)?;
    input_bridge::register(context, &mut bindings)?;
    form_state::register(context, &mut bindings)?;
    iframe_navigation::register(context, &mut bindings)?;
    form_validation::register(context, &mut bindings)?;
    form_submission::register(context, &mut bindings)?;
    geolocation::register(context, &mut bindings)?;
    broadcast_channel::register(context, &mut bindings)?;
    web_locks::register(context, &mut bindings)?;
    cache_storage::register(context, &mut bindings)?;
    worklet::register(context, &mut bindings)?;
    shared_worker::register(context, &mut bindings)?;
    let state = host_state.borrow();
    register_private_property(
        context,
        &mut bindings,
        js_string!("__omoikane_document_id"),
        state.document.identity() as f64,
        boa_engine::property::Attribute::all(),
    )?;
    register_private_property(
        context,
        &mut bindings,
        js_string!("__omoikane_location_href"),
        js_string!(state.location_href.as_str()),
        boa_engine::property::Attribute::all(),
    )?;
    register_private_property(
        context,
        &mut bindings,
        js_string!("__omoikane_navigator_user_agent"),
        js_string!(state.navigator_user_agent.as_str()),
        boa_engine::property::Attribute::all(),
    )?;
    register_private_callable(
        context,
        &mut bindings,
        js_string!("__omoikane_retain_node"),
        2,
        NativeFunction::from_copy_closure(node_lifetime::retain_node_native),
    )?;
    register_private_callable(
        context,
        &mut bindings,
        js_string!("__omoikane_set_node_owner"),
        2,
        NativeFunction::from_copy_closure(node_lifetime::set_owner_native),
    )?;
    register_private_callable(
        context,
        &mut bindings,
        js_string!("__omoikane_collected_nodes"),
        1,
        NativeFunction::from_copy_closure(node_lifetime::collected_nodes_native),
    )?;
    register_private_property(
        context,
        &mut bindings,
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
            js_string!("__omoikane_register_canonical_node_identity"),
            1,
            NativeFunction::from_copy_closure(register_canonical_node_identity_native),
        ),
        (
            js_string!("__omoikane_performance_now"),
            0,
            NativeFunction::from_copy_closure(performance_now_native),
        ),
        (
            js_string!("__omoikane_event_loop_now"),
            0,
            NativeFunction::from_copy_closure(event_loop_now_native),
        ),
        (
            js_string!("__omoikane_notification_permission"),
            0,
            NativeFunction::from_copy_closure(notification_permission_native),
        ),
        (
            js_string!("__omoikane_notification_request_permission"),
            0,
            NativeFunction::from_copy_closure(notification_request_permission_native),
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
            js_string!("__omoikane_text_decoder_create"),
            3,
            NativeFunction::from_copy_closure(text_stream::create_native),
        ),
        (
            js_string!("__omoikane_text_decoder_decode"),
            3,
            NativeFunction::from_copy_closure(text_stream::decode_native),
        ),
        (
            js_string!("__omoikane_text_encoder_encode"),
            1,
            NativeFunction::from_copy_closure(text_stream::encode_native),
        ),
        (
            js_string!("__omoikane_compression_create"),
            2,
            NativeFunction::from_copy_closure(compression_stream::create_native),
        ),
        (
            js_string!("__omoikane_compression_write"),
            2,
            NativeFunction::from_copy_closure(compression_stream::write_native),
        ),
        (
            js_string!("__omoikane_compression_finish"),
            1,
            NativeFunction::from_copy_closure(compression_stream::finish_native),
        ),
        (
            js_string!("__omoikane_compression_abort"),
            1,
            NativeFunction::from_copy_closure(compression_stream::abort_native),
        ),
        (
            js_string!("__omoikane_crypto_hmac"),
            3,
            NativeFunction::from_copy_closure(crypto_hmac_native),
        ),
        (
            js_string!("__omoikane_is_secure_context"),
            0,
            NativeFunction::from_copy_closure(is_secure_context_native),
        ),
        (
            js_string!("__omoikane_clipboard_read_text"),
            0,
            NativeFunction::from_copy_closure(clipboard_read_text_native),
        ),
        (
            js_string!("__omoikane_clipboard_write_text"),
            1,
            NativeFunction::from_copy_closure(clipboard_write_text_native),
        ),
        (
            js_string!("__omoikane_clipboard_permission"),
            0,
            NativeFunction::from_copy_closure(clipboard_permission_native),
        ),
        (
            js_string!("__omoikane_storage_origin"),
            1,
            NativeFunction::from_copy_closure(storage_origin_native),
        ),
        (
            js_string!("__omoikane_document_cookie_get"),
            1,
            NativeFunction::from_copy_closure(document_cookie_get_native),
        ),
        (
            js_string!("__omoikane_document_cookie_set"),
            2,
            NativeFunction::from_copy_closure(document_cookie_set_native),
        ),
        (
            js_string!("__omoikane_document_visibility_state"),
            1,
            NativeFunction::from_copy_closure(document_visibility_state_native),
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
            js_string!("__omoikane_storage_manager"),
            2,
            NativeFunction::from_copy_closure(storage_manager_native),
        ),
        (
            js_string!("__omoikane_get_element_by_id"),
            2,
            NativeFunction::from_copy_closure(get_element_by_id_native),
        ),
        (
            js_string!("__omoikane_subtree_has_window_name"),
            1,
            NativeFunction::from_copy_closure(subtree_has_window_name_native),
        ),
        (
            js_string!("__omoikane_node_index"),
            1,
            NativeFunction::from_copy_closure(node_index_native),
        ),
        (
            js_string!("__omoikane_node_is_connected"),
            1,
            NativeFunction::from_copy_closure(node_is_connected_native),
        ),
        (
            js_string!("__omoikane_get_popover_open"),
            1,
            NativeFunction::from_copy_closure(get_popover_open_native),
        ),
        (
            js_string!("__omoikane_set_popover_open"),
            2,
            NativeFunction::from_copy_closure(set_popover_open_native),
        ),
        (
            js_string!("__omoikane_set_modal_dialog"),
            2,
            NativeFunction::from_copy_closure(set_modal_dialog_native),
        ),
        (
            js_string!("__omoikane_fullscreen_element"),
            1,
            NativeFunction::from_copy_closure(fullscreen_element_native),
        ),
        (
            js_string!("__omoikane_fullscreen_enabled"),
            1,
            NativeFunction::from_copy_closure(fullscreen_enabled_native),
        ),
        (
            js_string!("__omoikane_request_fullscreen"),
            1,
            NativeFunction::from_copy_closure(request_fullscreen_native),
        ),
        (
            js_string!("__omoikane_exit_fullscreen"),
            2,
            NativeFunction::from_copy_closure(exit_fullscreen_native),
        ),
        (
            js_string!("__omoikane_fullscreen_subtree_removed"),
            1,
            NativeFunction::from_copy_closure(fullscreen_subtree_removed_native),
        ),
        (
            js_string!("__omoikane_node_is_inclusive_descendant"),
            2,
            NativeFunction::from_copy_closure(node_is_inclusive_descendant_native),
        ),
        (
            js_string!("__omoikane_node_has_slot_ancestor"),
            1,
            NativeFunction::from_copy_closure(node_has_slot_ancestor_native),
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
            js_string!("__omoikane_create_element_ns"),
            2,
            NativeFunction::from_copy_closure(create_element_ns_native),
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
            js_string!("__omoikane_attribute_records"),
            1,
            NativeFunction::from_copy_closure(attribute_records_native),
        ),
        (
            js_string!("__omoikane_attribute_record_count"),
            1,
            NativeFunction::from_copy_closure(attribute_record_count_native),
        ),
        (
            js_string!("__omoikane_attribute_record_at"),
            2,
            NativeFunction::from_copy_closure(attribute_record_at_native),
        ),
        (
            js_string!("__omoikane_attribute_value_ns"),
            3,
            NativeFunction::from_copy_closure(attribute_value_ns_native),
        ),
        (
            js_string!("__omoikane_array_buffer_info"),
            1,
            NativeFunction::from_copy_closure(array_buffer_info_native),
        ),
        (
            js_string!("__omoikane_clone_array_buffer"),
            1,
            NativeFunction::from_copy_closure(clone_array_buffer_native),
        ),
        (
            js_string!("__omoikane_transfer_array_buffer"),
            1,
            NativeFunction::from_copy_closure(transfer_array_buffer_native),
        ),
        (
            js_string!("__omoikane_array_buffer_view_info"),
            1,
            NativeFunction::from_copy_closure(array_buffer_view_info_native),
        ),
        (
            js_string!("__omoikane_set_attribute"),
            3,
            NativeFunction::from_copy_closure(set_attribute_native),
        ),
        (
            js_string!("__omoikane_set_attribute_ns"),
            5,
            NativeFunction::from_copy_closure(set_attribute_ns_native),
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
            js_string!("__omoikane_get_option_selected"),
            1,
            NativeFunction::from_copy_closure(get_option_selected_native),
        ),
        (
            js_string!("__omoikane_set_option_selected"),
            2,
            NativeFunction::from_copy_closure(set_option_selected_native),
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
            js_string!("__omoikane_call_event_listener"),
            3,
            NativeFunction::from_copy_closure(call_event_listener_native),
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
            js_string!("__omoikane_font_loading"),
            3,
            NativeFunction::from_copy_closure(font_loading::native),
        ),
        (
            js_string!("__omoikane_queue_font_loading_task"),
            1,
            NativeFunction::from_copy_closure(font_loading::queue_task),
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
            js_string!("__omoikane_queue_dom_manipulation_task"),
            1,
            NativeFunction::from_copy_closure(queue_dom_manipulation_task_native),
        ),
        (
            js_string!("__omoikane_set_adopted_stylesheets"),
            2,
            NativeFunction::from_copy_closure(set_adopted_stylesheets_native),
        ),
        (
            js_string!("__omoikane_enqueue_posted_message"),
            2,
            NativeFunction::from_copy_closure(enqueue_posted_message_native),
        ),
        (
            js_string!("__omoikane_window_post_message"),
            6,
            NativeFunction::from_copy_closure(window_post_message_native),
        ),
        (
            js_string!("__omoikane_open_auxiliary_window"),
            2,
            NativeFunction::from_copy_closure(open_auxiliary_window_native),
        ),
        (
            js_string!("__omoikane_navigate_named_link_target"),
            3,
            NativeFunction::from_copy_closure(navigate_named_link_target_native),
        ),
        (
            js_string!("__omoikane_auxiliary_window_global"),
            1,
            NativeFunction::from_copy_closure(auxiliary_window_global_native),
        ),
        (
            js_string!("__omoikane_auxiliary_window_state"),
            1,
            NativeFunction::from_copy_closure(auxiliary_window_state_native),
        ),
        (
            js_string!("__omoikane_close_auxiliary_window"),
            1,
            NativeFunction::from_copy_closure(close_auxiliary_window_native),
        ),
        (
            js_string!("__omoikane_navigate_auxiliary_window"),
            2,
            NativeFunction::from_copy_closure(navigate_auxiliary_window_native),
        ),
        (
            js_string!("__omoikane_create_worker"),
            1,
            NativeFunction::from_copy_closure(create_worker_native),
        ),
        (
            js_string!("__omoikane_bind_worker_owner"),
            2,
            NativeFunction::from_copy_closure(bind_worker_owner_native),
        ),
        (
            js_string!("__omoikane_worker_post_message"),
            2,
            NativeFunction::from_copy_closure(worker_post_message_native),
        ),
        (
            js_string!("__omoikane_terminate_worker"),
            1,
            NativeFunction::from_copy_closure(terminate_worker_native),
        ),
        (
            js_string!("__omoikane_worker_owner_post_message"),
            2,
            NativeFunction::from_copy_closure(worker_owner_post_message_native),
        ),
        (
            js_string!("__omoikane_worker_close"),
            0,
            NativeFunction::from_copy_closure(worker_close_native),
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
            js_string!("__omoikane_canvas_png"),
            3,
            NativeFunction::from_copy_closure(canvas_png_native),
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
            js_string!("__omoikane_parse_contextual_fragment"),
            3,
            NativeFunction::from_copy_closure(parse_contextual_fragment_native),
        ),
        (
            js_string!("__omoikane_mark_inserted_script"),
            1,
            NativeFunction::from_copy_closure(mark_inserted_script_native),
        ),
        (
            js_string!("__omoikane_collect_inserted_scripts"),
            1,
            NativeFunction::from_copy_closure(collect_inserted_scripts_native),
        ),
        (
            js_string!("__omoikane_prepare_inserted_inline_script"),
            1,
            NativeFunction::from_copy_closure(prepare_inserted_inline_script_native),
        ),
        (
            js_string!("__omoikane_record_inserted_script_error"),
            2,
            NativeFunction::from_copy_closure(record_inserted_script_error_native),
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
            js_string!("__omoikane_set_user_action_target"),
            3,
            NativeFunction::from_copy_closure(set_user_action_target_native),
        ),
        (
            js_string!("__omoikane_node_type"),
            1,
            NativeFunction::from_copy_closure(node_type_native),
        ),
        (
            js_string!("__omoikane_node_is_html_element"),
            1,
            NativeFunction::from_copy_closure(node_is_html_element_native),
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
            js_string!("__omoikane_remove_attribute_ns"),
            2,
            NativeFunction::from_copy_closure(remove_attribute_ns_native),
        ),
        (
            js_string!("__omoikane_create_text_node"),
            1,
            NativeFunction::from_copy_closure(create_text_node_native),
        ),
        (
            js_string!("__omoikane_create_cdata_section"),
            1,
            NativeFunction::from_copy_closure(create_cdata_section_native),
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
            js_string!("__omoikane_parse_xml"),
            1,
            NativeFunction::from_copy_closure(parse_xml_native),
        ),
        (
            js_string!("__omoikane_serialize_xml"),
            1,
            NativeFunction::from_copy_closure(serialize_xml_native),
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
            js_string!("__omoikane_csp_violations"),
            1,
            NativeFunction::from_copy_closure(csp_violations_native),
        ),
        (
            js_string!("__omoikane_is_rendered_for_focus"),
            1,
            NativeFunction::from_copy_closure(is_rendered_for_focus_native),
        ),
        (
            js_string!("__omoikane_set_content_visibility_focus"),
            1,
            NativeFunction::from_copy_closure(set_content_visibility_focus_native),
        ),
        (
            js_string!("__omoikane_set_content_visibility_selection"),
            2,
            NativeFunction::from_copy_closure(set_content_visibility_selection_native),
        ),
        (
            js_string!("__omoikane_content_visibility_skips_inner_text"),
            1,
            NativeFunction::from_copy_closure(content_visibility_skips_inner_text_native),
        ),
        (
            js_string!("__omoikane_parser_form_owner"),
            1,
            NativeFunction::from_copy_closure(parser_form_owner_native),
        ),
        (
            js_string!("__omoikane_set_form_associated_custom"),
            2,
            NativeFunction::from_copy_closure(set_form_associated_custom_native),
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
            js_string!("__omoikane_expand_style_shorthand"),
            2,
            NativeFunction::from_copy_closure(expand_style_shorthand_native),
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
            js_string!("__omoikane_layout_metrics_generation"),
            0,
            NativeFunction::from_copy_closure(layout_metrics_generation_native),
        ),
        (
            js_string!("__omoikane_layout_metrics"),
            1,
            NativeFunction::from_copy_closure(layout_metrics_native),
        ),
        (
            js_string!("__omoikane_hit_test_point"),
            4,
            NativeFunction::from_copy_closure(hit_test_point_native),
        ),
        (
            js_string!("__omoikane_element_scroll_offset"),
            1,
            NativeFunction::from_copy_closure(element_scroll_offset_native),
        ),
        (
            js_string!("__omoikane_set_element_scroll"),
            4,
            NativeFunction::from_copy_closure(set_element_scroll_native),
        ),
        (
            js_string!("__omoikane_visual_viewport_state"),
            0,
            NativeFunction::from_copy_closure(visual_viewport_state_native),
        ),
        (
            js_string!("__omoikane_window_scroll_offset"),
            0,
            NativeFunction::from_copy_closure(window_scroll_offset_native),
        ),
        (
            js_string!("__omoikane_set_window_scroll"),
            3,
            NativeFunction::from_copy_closure(set_window_scroll_native),
        ),
        (
            js_string!("__omoikane_css_rule_count"),
            1,
            NativeFunction::from_copy_closure(css_rule_count_native),
        ),
        (
            js_string!("__omoikane_css_import_parts"),
            1,
            NativeFunction::from_copy_closure(css_import_parts_native),
        ),
        (
            js_string!("__omoikane_imported_stylesheet"),
            2,
            NativeFunction::from_copy_closure(imported_stylesheet_native),
        ),
        (
            js_string!("__omoikane_css_rule_sources"),
            1,
            NativeFunction::from_copy_closure(css_rule_sources_native),
        ),
        (
            js_string!("__omoikane_css_declarations"),
            1,
            NativeFunction::from_copy_closure(css_declarations_native),
        ),
        (
            js_string!("__omoikane_css_property_rule"),
            1,
            NativeFunction::from_copy_closure(css_property_rule_native),
        ),
        (
            js_string!("__omoikane_register_property"),
            4,
            NativeFunction::from_copy_closure(register_property_native),
        ),
        (
            js_string!("__omoikane_css_scope_rules_valid"),
            1,
            NativeFunction::from_copy_closure(css_scope_rules_valid_native),
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
            js_string!("__omoikane_document_close"),
            1,
            NativeFunction::from_copy_closure(document_close_native),
        ),
        (
            js_string!("__omoikane_iframe_global"),
            1,
            NativeFunction::from_copy_closure(iframe_global_native),
        ),
        (
            js_string!("__omoikane_dispatch_iframe_departure_native"),
            1,
            NativeFunction::from_copy_closure(dispatch_iframe_departure_native),
        ),
        (
            js_string!("__omoikane_existing_iframe_document"),
            1,
            NativeFunction::from_copy_closure(existing_iframe_document_native),
        ),
        (
            js_string!("__omoikane_iframe_content_document"),
            1,
            NativeFunction::from_copy_closure(iframe_content_document_native),
        ),
        (
            js_string!("__omoikane_iframe_context_state"),
            2,
            NativeFunction::from_copy_closure(iframe_context_state_native),
        ),
        (
            js_string!("__omoikane_iframe_force_navigation"),
            1,
            NativeFunction::from_copy_closure(iframe_force_navigation_native),
        ),
        (
            js_string!("__omoikane_take_discarded_node_ids"),
            0,
            NativeFunction::from_copy_closure(take_discarded_node_ids_native),
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
            js_string!("__omoikane_document_url"),
            1,
            NativeFunction::from_copy_closure(document_url_native),
        ),
        (
            js_string!("__omoikane_commit_history_api_url"),
            1,
            NativeFunction::from_copy_closure(commit_history_api_url_native),
        ),
        (
            js_string!("__omoikane_commit_fragment_url"),
            1,
            NativeFunction::from_copy_closure(commit_fragment_url_native),
        ),
        (
            js_string!("__omoikane_document_base_url"),
            1,
            NativeFunction::from_copy_closure(document_base_url_native),
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
            js_string!("__omoikane_parse_url"),
            2,
            NativeFunction::from_copy_closure(parse_url_native),
        ),
        (
            js_string!("__omoikane_schedule_navigation"),
            3,
            NativeFunction::from_copy_closure(schedule_navigation_native),
        ),
        (
            js_string!("__omoikane_submit_form"),
            6,
            NativeFunction::from_copy_closure(submit_form_native),
        ),
    ] {
        if name.to_std_string_escaped().starts_with("__omoikane_") {
            register_private_builtin_callable(context, &mut bindings, name, length, function)?;
        } else {
            context.register_global_builtin_callable(name, length, function)?;
        }
    }

    Ok(bindings)
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

/// HTML's "matches about:blank" predicate ignores the URL's query and
/// fragment while comparing the `about:blank` scheme/path pair.
fn matches_about_blank_url(value: &str) -> bool {
    let without_fragment = value
        .split_once('#')
        .map_or(value, |(before_fragment, _)| before_fragment);
    let without_query = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(before_query, _)| before_query);
    without_query.eq_ignore_ascii_case("about:blank")
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

fn cross_origin_access_error(context: &mut Context) -> JsResult<JsError> {
    let constructor = context
        .global_object()
        .get(js_string!("DOMException"), context)?;
    let constructor = constructor.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("DOMException constructor is unavailable")
    })?;
    let error = constructor.construct(
        &[
            JsValue::from(js_string!("Cross-origin access is denied")),
            JsValue::from(js_string!("SecurityError")),
        ],
        None,
        context,
    )?;
    Ok(JsError::from_opaque(error.into()))
}

fn caller_document_id(context: &Context) -> Option<usize> {
    // An explicit caller without a document identity must fail closed.
    if let Some(realm) = context.caller_realm() {
        return realm
            .host_defined()
            .get::<ModuleDocumentId>()
            .map(|id| id.0);
    }
    if let Some(realm) = context.active_script_or_module_realm() {
        return realm
            .host_defined()
            .get::<ModuleDocumentId>()
            .map(|id| id.0);
    }
    context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|id| id.0)
}

fn caller_document_origin(context: &Context, document_id: usize) -> Option<DocumentSecurityOrigin> {
    let from_realm = |realm: &Realm| {
        (realm.host_defined().get::<ModuleDocumentId>()?.0 == document_id)
            .then(|| {
                realm
                    .host_defined()
                    .get::<ModuleDocumentOrigin>()
                    .map(|origin| origin.0.clone())
            })
            .flatten()
    };
    if let Some(realm) = context.caller_realm() {
        return from_realm(&realm);
    }
    if let Some(realm) = context.active_script_or_module_realm() {
        return from_realm(&realm);
    }
    from_realm(&context.realm())
}

fn ensure_same_origin_document(context: &mut Context, target_document_id: usize) -> JsResult<()> {
    let caller = caller_document_id(context);
    let saved_caller_origin = caller.and_then(|id| caller_document_origin(context, id));
    let allowed = with_host_state(|host| {
        let state = host.borrow();
        Ok(caller.is_some_and(|caller| {
            let source_origin = state.document_security_origins.get(&caller).or_else(|| {
                (!state.document_is_active(caller))
                    .then_some(())
                    .and(saved_caller_origin.as_ref())
            });
            match (
                source_origin,
                state
                    .document_security_origins
                    .get(&target_document_id)
                    .or_else(|| {
                        state
                            .retired_document_security_origins
                            .get(&target_document_id)
                    }),
            ) {
                (Some(source), Some(target)) => source == target,
                _ => false,
            }
        }))
    })?;
    if allowed {
        Ok(())
    } else {
        Err(cross_origin_access_error(context)?)
    }
}

fn ensure_same_origin_node(context: &mut Context, node_id: usize) -> JsResult<()> {
    let target_document_id = with_host_state(|host| {
        let state = host.borrow();
        let Some(node) = state.get_node(node_id) else {
            return Ok(None);
        };
        Ok(document_root_for_node(&node)
            .or_else(|| state.node_lifetime_owner(node_id))
            .map(|document| document.identity()))
    })?;
    if let Some(document_id) = target_document_id {
        ensure_same_origin_document(context, document_id)
    } else {
        Err(cross_origin_access_error(context)?)
    }
}

fn register_canonical_node_identity_native(
    _this: &JsValue,
    args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let resolver = args.first().cloned().ok_or_else(|| {
        JsError::from(
            JsNativeError::typ().with_message("canonical node identity resolver is required"),
        )
    })?;
    if resolver.as_callable().is_none() {
        return Err(JsNativeError::typ()
            .with_message("canonical node identity resolver must be callable")
            .into());
    }
    with_host_state(|state| {
        state.borrow_mut().canonical_node_identity_resolver = Some(resolver);
        Ok(JsValue::undefined())
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
        state.borrow_mut().pending_javascript_dialog =
            Some(PendingJavaScriptDialog { dialog, suspension });
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

fn event_loop_now_native(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| Ok(JsValue::from(state.borrow().event_loop.now_ms() as f64)))
}

fn notification_permission_native(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| Ok(js_string!(state.borrow().notification_permission.as_str()).into()))
}

fn notification_request_permission_native(
    _this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let permission = with_host_state(|state| {
        let mut state = state.borrow_mut();
        if state.notification_permission == "default" {
            state.notification_permission = "denied".to_string();
        }
        Ok(state.notification_permission.clone())
    })?;
    Ok(js_string!(permission).into())
}

fn is_secure_context_url(url: &str) -> bool {
    let lower_url = url.to_ascii_lowercase();
    // Fragments are not part of an origin.  Strip them before the IPv6
    // fast-path and the lightweight URL parser so `http://[::1]#section`
    // behaves like the corresponding fragment-free loopback URL.
    let url_without_fragment = lower_url
        .split_once('#')
        .map_or(lower_url.as_str(), |(prefix, _)| prefix);
    if let Some(authority) = url_without_fragment.strip_prefix("http://") {
        let authority_end = authority.find(['/', '?']).unwrap_or(authority.len());
        let authority = &authority[..authority_end];
        if let Some(port) = authority.strip_prefix("[::1]") {
            let valid_port = match port.strip_prefix(':') {
                None => true,
                Some(value) => !value.is_empty() && value.parse::<u16>().is_ok(),
            };
            if valid_port {
                return true;
            }
        }
    }
    let Ok(url) = url_without_fragment.parse::<crate::http::Url>() else {
        return false;
    };
    if url.scheme().eq_ignore_ascii_case("https") {
        return true;
    }
    if !url.scheme().eq_ignore_ascii_case("http") {
        return false;
    }
    let host = url.host().to_ascii_lowercase();
    host == "localhost"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|address| address.octets()[0] == 127)
}

fn is_secure_context_parsed_url(url: &crate::http::Url) -> bool {
    if url.scheme().eq_ignore_ascii_case("https") {
        return true;
    }
    if !url.scheme().eq_ignore_ascii_case("http") {
        return false;
    }
    let host = url.host();
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn host_is_secure_context(state: &HostState) -> bool {
    if let Some(secure) = state.secure_context_override {
        return secure;
    }
    // The parsed base URL is the canonical origin used by the runtime. Avoid
    // formatting and reparsing it on every secure-context or clipboard check.
    if let Some(base_url) = state.base_url.as_ref()
        && !state.location_href.contains('#')
    {
        return is_secure_context_parsed_url(base_url);
    }
    is_secure_context_url(&state.location_href)
}

fn document_is_secure_context(state: &HostState, document_id: usize) -> bool {
    if let Some(secure) = state.secure_context_override {
        return secure;
    }
    if !host_is_secure_context(state) {
        return false;
    }
    state
        .document_urls
        .get(&document_id)
        .is_some_and(|url| is_secure_context_url(url))
        || state
            .document_base_urls
            .get(&document_id)
            .is_some_and(is_secure_context_parsed_url)
}

fn is_secure_context_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    with_host_state(|state| {
        let state = state.borrow();
        Ok(JsValue::from(host_is_secure_context(&state)))
    })
}

fn clipboard_read_text_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    with_host_state(|state| {
        let state = state.borrow();
        if !host_is_secure_context(&state) || !state.clipboard_permission_granted {
            return Ok(JsValue::null());
        }
        Ok(JsValue::from(js_string!(state.clipboard.read_text())))
    })
}

fn clipboard_permission_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    with_host_state(|state| Ok(JsValue::from(state.borrow().clipboard_permission_granted)))
}

fn clipboard_write_text_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let text = string_argument(args.first(), "", context)?;
    with_host_state(|state| {
        let state = state.borrow();
        if !host_is_secure_context(&state) || !state.clipboard_permission_granted {
            return Ok(JsValue::from(false));
        }
        state.clipboard.write_text(text);
        Ok(JsValue::from(true))
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
        JsError::from(
            JsNativeError::error()
                .with_message(format!("secure random generation failed: {error}")),
        )
    })?;
    let json = serde_json::to_string(&bytes)
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
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
        JsError::from(JsNativeError::typ().with_message(format!("invalid digest input: {error}")))
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
    let json = serde_json::to_string(&digest)
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
    Ok(js_string!(json).into())
}

/// Computes an HMAC over a caller-owned byte snapshot.
///
/// The Web Crypto wrapper keeps CryptoKey objects realm-local and only passes
/// the immutable key/data snapshots into this host hook.  Dispatching the
/// digest implementation here avoids exposing a Rust crypto object to Boa
/// while still making the operation's Promise/task boundary explicit in JS.
fn crypto_hmac_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    use hmac::{Hmac, Mac};

    let algorithm = string_argument(args.first(), "", context)?;
    let key_json = string_argument(args.get(1), "[]", context)?;
    let data_json = string_argument(args.get(2), "[]", context)?;
    let key: Vec<u8> = serde_json::from_str(&key_json).map_err(|error| {
        JsError::from(JsNativeError::typ().with_message(format!("invalid HMAC key data: {error}")))
    })?;
    let data: Vec<u8> = serde_json::from_str(&data_json).map_err(|error| {
        JsError::from(
            JsNativeError::typ().with_message(format!("invalid HMAC input data: {error}")),
        )
    })?;

    let digest = match algorithm.as_str() {
        "SHA-1" => {
            let mut mac = Hmac::<sha1::Sha1>::new_from_slice(&key).map_err(|error| {
                JsError::from(JsNativeError::error().with_message(error.to_string()))
            })?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        "SHA-256" => {
            let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key).map_err(|error| {
                JsError::from(JsNativeError::error().with_message(error.to_string()))
            })?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        "SHA-384" => {
            let mut mac = Hmac::<sha2::Sha384>::new_from_slice(&key).map_err(|error| {
                JsError::from(JsNativeError::error().with_message(error.to_string()))
            })?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        "SHA-512" => {
            let mut mac = Hmac::<sha2::Sha512>::new_from_slice(&key).map_err(|error| {
                JsError::from(JsNativeError::error().with_message(error.to_string()))
            })?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        _ => {
            return Err(JsError::from(
                JsNativeError::error().with_message("unsupported HMAC hash algorithm"),
            ));
        }
    };
    let json = serde_json::to_string(&digest)
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
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
    ensure_same_origin_document(context, document_id)?;
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

fn document_cookie_get_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        let state = state.borrow();
        let visible = state
            .document_origins
            .get(&document_id)
            .is_some_and(Option::is_some);
        let document_url = state
            .document_urls
            .get(&document_id)
            .and_then(|url| url.parse::<crate::http::Url>().ok());
        let text = if visible {
            document_url
                .map(|url| {
                    let site = state
                        .location_href
                        .parse::<crate::http::Url>()
                        .unwrap_or_else(|_| url.clone());
                    state
                        .cookie_store
                        .lock()
                        .unwrap()
                        .document_cookie(&url, &site)
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        Ok(JsValue::from(js_string!(text)))
    })
}

fn document_cookie_set_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let value = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let state = state.borrow();
        if !state
            .document_origins
            .get(&document_id)
            .is_some_and(Option::is_some)
        {
            return Ok(JsValue::undefined());
        }
        if let Some(url) = state
            .document_urls
            .get(&document_id)
            .and_then(|url| url.parse::<crate::http::Url>().ok())
        {
            let site = state
                .location_href
                .parse::<crate::http::Url>()
                .unwrap_or_else(|_| url.clone());
            state
                .cookie_store
                .lock()
                .unwrap()
                .add_from_document(&value, &url, &site);
        }
        Ok(JsValue::undefined())
    })
}

fn storage_origin_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
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

fn document_visibility_state_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        let state = state.borrow();
        let visible = state.document_urls.contains_key(&document_id) && !state.page_hidden;
        Ok(JsValue::from(js_string!(if visible {
            "visible"
        } else {
            "hidden"
        })))
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
    let key = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
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
    let key = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let value = args
        .get(3)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
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
    let key = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
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

fn storage_manager_native(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let document_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|host| {
        let state = host.borrow();
        let secure = document_is_secure_context(&state, document_id);
        if operation == "available" {
            return Ok(JsValue::from(secure));
        }
        if !secure {
            return Err(JsNativeError::error()
                .with_message("StorageManager requires a secure context")
                .into());
        }
        let origin = state
            .document_origins
            .get(&document_id)
            .cloned()
            .flatten()
            .ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ().with_message("StorageManager requires a tuple origin"),
                )
            })?;
        let manager = state.storage_manager.clone();
        drop(state);
        match operation.as_str() {
            "estimate" => {
                let estimate = manager.estimate(&origin);
                Ok(js_string!(
                    serde_json::json!({"usage": estimate.usage, "quota": estimate.quota})
                        .to_string()
                )
                .into())
            }
            "persisted" => Ok(JsValue::from(manager.persisted(&origin))),
            "persist" => manager
                .persist(&origin)
                .map(JsValue::from)
                .map_err(|error| {
                    JsNativeError::error()
                        .with_message(format!("could not persist storage permission: {error}"))
                        .into()
                }),
            "permission" => {
                let permission = if manager.persisted(&origin) {
                    "granted"
                } else if manager.persistence_policy() == StoragePersistencePolicy::Allow {
                    "prompt"
                } else {
                    "denied"
                };
                Ok(js_string!(permission).into())
            }
            _ => Err(JsNativeError::typ()
                .with_message(format!("unknown StorageManager operation: {operation}"))
                .into()),
        }
    })
}

// ---------------------------------------------------------------------------
// Computed style + layout metrics (issues 016-8, 044-2)
// ---------------------------------------------------------------------------

/// Collects author stylesheet owners in tree order, preserving shadow scopes.
type StylesheetNodeEntry = (NodeHandle, Option<(NodeHandle, usize)>, Option<NodeHandle>);
type FontScopeParent = (usize, Option<usize>);

fn collect_stylesheet_nodes(
    document: &NodeHandle,
) -> (Vec<StylesheetNodeEntry>, Vec<FontScopeParent>) {
    fn walk(
        node: &NodeHandle,
        scope: Option<&(NodeHandle, usize)>,
        next_scope_order: &mut usize,
        out: &mut Vec<StylesheetNodeEntry>,
        font_scope_parents: &mut Vec<FontScopeParent>,
    ) {
        if node.tag_name().as_deref() == Some("noscript") {
            return;
        }
        if matches!(node.tag_name().as_deref(), Some("style" | "link")) {
            let implicit_scope_root = node.parent_node().and_then(|parent| {
                if parent.node_type() == NodeType::Element {
                    Some(parent)
                } else {
                    scope.and_then(|(root, _)| root.shadow_host())
                }
            });
            out.push((node.clone(), scope.cloned(), implicit_scope_root));
        }
        if let Some(root) = node.shadow_root() {
            *next_scope_order += 1;
            let root_scope = (root.clone(), *next_scope_order);
            font_scope_parents.push((
                root.identity(),
                scope.map(|(parent_root, _)| parent_root.identity()),
            ));
            walk(
                &root,
                Some(&root_scope),
                next_scope_order,
                out,
                font_scope_parents,
            );
        }
        for child in node.child_nodes() {
            walk(&child, scope, next_scope_order, out, font_scope_parents);
        }
    }
    let mut out = Vec::new();
    let mut font_scope_parents = Vec::new();
    walk(document, None, &mut 0, &mut out, &mut font_scope_parents);
    (out, font_scope_parents)
}

/// Collects constructed stylesheets adopted by `document` and its shadow
/// roots. A shadow-root stylesheet is scoped to that root just like an inline
/// `<style>` inside the same tree; document-level sheets apply to the whole
/// browsing context.
fn collect_adopted_stylesheets(
    adopted: &HashMap<usize, Vec<String>>,
    document: &NodeHandle,
) -> Vec<(Option<(NodeHandle, usize)>, String)> {
    fn walk(
        node: &NodeHandle,
        scope: Option<&(NodeHandle, usize)>,
        next_scope_order: &mut usize,
        adopted: &HashMap<usize, Vec<String>>,
        out: &mut Vec<(Option<(NodeHandle, usize)>, String)>,
    ) {
        if let Some(stylesheets) = adopted.get(&node.identity()) {
            for css in stylesheets {
                if !css.trim().is_empty() {
                    out.push((scope.cloned(), css.clone()));
                }
            }
        }
        if let Some(root) = node.shadow_root() {
            *next_scope_order += 1;
            let root_scope = (root.clone(), *next_scope_order);
            walk(&root, Some(&root_scope), next_scope_order, adopted, out);
        }
        for child in node.child_nodes() {
            walk(&child, scope, next_scope_order, adopted, out);
        }
    }

    let mut out = Vec::new();
    let mut next_scope_order = 0;
    walk(document, None, &mut next_scope_order, adopted, &mut out);
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

/// Serializes colors without quantizing Color 4 channels or their alpha.
fn computed_color_to_css_string(color: &str) -> String {
    if let Some(parsed) = crate::paint::color4::CssColor::parse(color) {
        return parsed.serialize_computed();
    }
    if color.starts_with("rgba(") {
        return color.to_string();
    }
    crate::paint::color::parse_color(color).map_or_else(
        || color.to_string(),
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
    )
}

/// Serializes a single [`ComputedValue`] to its CSS string form.
fn computed_value_to_css_string(property_name: &str, value: &ComputedValue) -> String {
    match value {
        ComputedValue::Keyword(keyword) => keyword.clone(),
        ComputedValue::Color(color) => computed_color_to_css_string(color),
        ComputedValue::String(string) => string.clone(),
        ComputedValue::Px(px) => format!("{}px", format_css_number(*px)),
        ComputedValue::Percentage(pct) => format!("{}%", format_css_number(*pct)),
        ComputedValue::Number(number) => format_css_number(*number),
        value @ ComputedValue::LengthPercentage(_)
            if property_name.eq_ignore_ascii_case("text-decoration-thickness")
                || property_name.eq_ignore_ascii_case("text-underline-offset") =>
        {
            let Some((px, pct)) = value.linear_length_percentage_components() else {
                return value.css_text();
            };
            if px == 0.0 {
                format!("{}%", format_css_number(pct))
            } else if pct == 0.0 {
                format!("{}px", format_css_number(px))
            } else if px < 0.0 {
                format!(
                    "calc({}% - {}px)",
                    format_css_number(pct),
                    format_css_number(px.abs())
                )
            } else {
                format!(
                    "calc({}% + {}px)",
                    format_css_number(pct),
                    format_css_number(px)
                )
            }
        }
        value @ ComputedValue::LengthPercentage(_) => value.css_text(),
        value @ ComputedValue::Position { .. } => value.css_text(),
    }
}

/// Resolves property values that need the element's own context for CSSOM.
fn computed_style_property_to_css_string(
    property_name: &str,
    value: &ComputedValue,
    style: &ComputedStyle,
) -> String {
    if property_name == "text-shadow" {
        let current_color = style
            .get("color")
            .map(ComputedValue::css_text)
            .unwrap_or_else(|| "black".into());
        if let Some(resolved) =
            crate::css::style::text_shadow::resolved_css_text(&value.css_text(), &current_color)
        {
            return resolved;
        }
    }
    computed_value_to_css_string(property_name, value)
}

/// Serializes a resolved style to JSON with kebab-case CSS property names.
fn serialize_computed_style(style: &ComputedStyle) -> String {
    let mut json = String::from("{");
    let mut first = true;
    for (name, value) in style.properties() {
        if !first {
            json.push(',');
        }
        first = false;
        json.push('"');
        json.push_str(&escape_json_string(&name));
        json.push_str("\":\"");
        json.push_str(&escape_json_string(&computed_style_property_to_css_string(
            &name, &value, style,
        )));
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
    collect_matching_replaced_fragments(root, node, transform, (0.0, 0.0), fragments);
    for child in &root.children {
        if let Some(found) = find_layout_box_with_transform(child, node, transform, fragments) {
            return Some(found);
        }
    }
    None
}

struct InlineFragmentGeometry {
    rect: Rect,
    non_replaced: bool,
    style: ComputedStyle,
    transform: AffineTransform,
    scroll: (f32, f32),
}

fn collect_matching_replaced_fragments(
    root: &LayoutBox,
    node: &NodeHandle,
    transform: AffineTransform,
    scroll: (f32, f32),
    output: &mut Vec<InlineFragmentGeometry>,
) {
    for line in &root.lines {
        for fragment in &line.fragments {
            if &fragment.node != node {
                continue;
            }
            let style = match &fragment.content {
                InlineFragmentContent::InlineBox(style)
                | InlineFragmentContent::Image(_, style)
                | InlineFragmentContent::FormControl(style, _, _)
                | InlineFragmentContent::IconFormControl(style, _, _, _) => style,
                _ => continue,
            };
            output.push(InlineFragmentGeometry {
                rect: fragment.rect,
                non_replaced: matches!(fragment.content, InlineFragmentContent::InlineBox(_)),
                style: style.clone(),
                transform,
                scroll,
            });
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
///   box's inline line content and the border boxes or unclipped line content
///   of every overflowing descendant plus the container's end-edge padding.
///   A descendant that clips its overflow contributes its border box, while
///   absolutely positioned descendants whose containing block is outside
///   that clip can still extend this element's scrolling area.
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

    let client_rects = if layout.block_fragments.is_empty() {
        vec![Rect {
            x: border_x,
            y: border_y,
            width: border_width,
            height: border_height,
        }]
    } else {
        layout
            .block_fragments
            .iter()
            .map(|fragment| fragment.target)
            .collect()
    };
    let bounding = inline_rect_union(client_rects.iter().copied());
    let offset_rect = client_rects.first().copied().unwrap_or(Rect {
        x: border_x,
        y: border_y,
        width: border_width,
        height: border_height,
    });

    LayoutMetrics {
        x: bounding.x,
        y: bounding.y,
        width: bounding.width,
        height: bounding.height,
        content_x: content.x,
        content_y: content.y,
        content_width: content.width,
        content_height: content.height,
        offset_width: border_width,
        offset_height: border_height,
        offset_top: offset_rect.y,
        offset_left: offset_rect.x,
        client_width,
        client_height,
        client_top: border.top,
        client_left: border.left,
        scroll_width,
        scroll_height,
        client_rects,
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
    metrics.client_rects = metrics
        .client_rects
        .into_iter()
        .map(|rect| transform_rect(rect, transform))
        .collect();
    let transformed = inline_rect_union(metrics.client_rects.iter().copied());
    metrics.x = transformed.x;
    metrics.y = transformed.y;
    metrics.width = transformed.width;
    metrics.height = transformed.height;
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
    if corners
        .iter()
        .any(|point| !point.0.is_finite() || !point.1.is_finite())
    {
        // Keep CSSOM geometry anchored in layout space when projection has no
        // finite corner, avoiding a discontinuous jump to the origin.
        return rect;
    }
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

fn inline_rect_union(rects: impl Iterator<Item = Rect>) -> Rect {
    let mut first = None;
    let mut bounds: Option<Rect> = None;
    for rect in rects {
        first.get_or_insert(rect);
        if rect.width == 0.0 || rect.height == 0.0 {
            continue;
        }
        if let Some(r) = &mut bounds {
            let right = (r.x + r.width).max(rect.x + rect.width);
            let bottom = (r.y + r.height).max(rect.y + rect.height);
            r.x = r.x.min(rect.x);
            r.y = r.y.min(rect.y);
            r.width = right - r.x;
            r.height = bottom - r.y;
        } else {
            bounds = Some(rect);
        }
    }
    bounds.or(first).unwrap_or_default()
}

fn compute_replaced_fragment_metrics(fragments: Vec<InlineFragmentGeometry>) -> LayoutMetrics {
    let Some(first) = fragments.first() else {
        return LayoutMetrics::zero();
    };
    let layout_rect = first.rect;
    let non_replaced = first.non_replaced;
    let offset_rect = if non_replaced {
        inline_rect_union(fragments.iter().map(|f| f.rect))
    } else {
        layout_rect
    };
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
    let union = inline_rect_union(client_rects.iter().copied());
    LayoutMetrics {
        x: union.x,
        y: union.y,
        width: union.width,
        height: union.height,
        content_x: union.x + border.left + padding.left,
        content_y: union.y + border.top + padding.top,
        content_width,
        content_height,
        offset_width: offset_rect.width,
        offset_height: offset_rect.height,
        offset_top: layout_rect.y,
        offset_left: layout_rect.x,
        client_width: if non_replaced { 0.0 } else { client_width },
        client_height: if non_replaced { 0.0 } else { client_height },
        client_top: if non_replaced { 0.0 } else { border.top },
        client_left: if non_replaced { 0.0 } else { border.left },
        scroll_width: if non_replaced { 0.0 } else { client_width },
        scroll_height: if non_replaced { 0.0 } else { client_height },
        client_rects,
        has_box: true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LayoutUsedSize {
    content_width: f32,
    content_height: f32,
    border_width: f32,
    border_height: f32,
}

/// Returns the untransformed used box size for CSSOM resolved values. Ordinary
/// block boxes store it directly in the layout tree. Inline replaced elements
/// store it in an inline fragment; non-replaced inline fragments are excluded
/// because the CSS `width` and `height` properties do not apply to them.
fn layout_used_size(root: &LayoutBox, node: &NodeHandle) -> Option<LayoutUsedSize> {
    let mut fragments = Vec::new();
    if let Some((layout, _)) =
        find_layout_box_with_transform(root, node, AffineTransform::identity(), &mut fragments)
    {
        let content = layout.dimensions.content;
        let padding = layout.dimensions.padding;
        let border = layout.dimensions.border;
        return Some(LayoutUsedSize {
            content_width: content.width,
            content_height: content.height,
            border_width: content.width + padding.left + padding.right + border.left + border.right,
            border_height: content.height
                + padding.top
                + padding.bottom
                + border.top
                + border.bottom,
        });
    }

    let first = fragments.first()?;
    if first.non_replaced {
        return None;
    }
    let padding = edge_sizes(&first.style, "padding");
    let border = edge_sizes(&first.style, "border");
    Some(LayoutUsedSize {
        content_width: (first.rect.width
            - padding.left
            - padding.right
            - border.left
            - border.right)
            .max(0.0),
        content_height: (first.rect.height
            - padding.top
            - padding.bottom
            - border.top
            - border.bottom)
            .max(0.0),
        border_width: first.rect.width,
        border_height: first.rect.height,
    })
}

/// Resolves the layout tree belonging to `document` and returns `node`'s used
/// size. The top-level document reuses its cached layout. A child browsing
/// context is laid out against its own viewport, stylesheet resolver, resource
/// base and web-font registry instead of reading the main tree.
fn resolved_layout_size(
    state: &mut HostState,
    document: &NodeHandle,
    node: &NodeHandle,
) -> Option<LayoutUsedSize> {
    let document_id = document.identity();
    if document_id == state.document.identity() {
        state.ensure_layout();
        return state
            .layout_root
            .as_ref()
            .and_then(|root| layout_used_size(root, node));
    }

    let viewport = state.viewport_for_document(document);
    state.ensure_style_resolver(document);
    let base = crate::paint::stylesheet::extract_document_base_url(
        document,
        state.base_url_for_document(document_id).as_ref(),
    );
    let image_site = state.location_href.parse::<crate::http::Url>().ok();
    let image_cookies = Arc::clone(&state.cookie_store);
    let animation_time = state.event_loop.rendering_time_ms() as u64;
    let layout = state
        .document_styles
        .get_mut(&document_id)
        .and_then(|entry| {
            let resolver = entry.resolver.as_mut()?;
            crate::layout::with_layout_fonts(
                crate::paint::text::load_text_fonts(),
                Some(entry.web_fonts.clone()),
                || {
                    crate::layout::with_image_cookie_store(
                        image_cookies,
                        image_site,
                        document_id,
                        || {
                            crate::layout::with_image_base_url(base, || {
                                crate::layout::with_image_animation_time(animation_time, || {
                                    crate::layout::layout_tree(document, resolver, viewport)
                                })
                            })
                        },
                    )
                },
            )
        });
    layout
        .as_ref()
        .and_then(|root| layout_used_size(root, node))
}

fn resolved_width_applies(style: &ComputedStyle) -> bool {
    !matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(display))
            if display.eq_ignore_ascii_case("table-row")
                || display.eq_ignore_ascii_case("table-row-group")
    )
}

fn resolved_height_applies(style: &ComputedStyle) -> bool {
    !matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(display))
            if display.eq_ignore_ascii_case("table-column")
                || display.eq_ignore_ascii_case("table-column-group")
    )
}

/// `__omoikane_computed_style(nodeId)` -> JSON string of computed CSS
/// properties (kebab-case name to CSS string value). Forces a synchronous
/// style recompute if the DOM changed since the last query.
fn computed_style_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let flush_only = args.get(3).is_some_and(JsValue::to_boolean);
    let name = args
        .get(4)
        .and_then(JsValue::as_string)
        .map(|name| name.to_std_string_escaped());
    let needs_used_size = !flush_only && name.as_deref().is_none_or(cssom_property_uses_used_size);
    let style = resolve_native_computed_style(args, needs_used_size, context)?;
    // A synchronous flush samples transitions before event dispatch, but its
    // caller does not need to materialize a property map until a later read.
    if flush_only {
        return Ok(JsValue::undefined());
    }
    // A named CSSOM read needs only one serialized property. Resolve afresh as
    // above, but avoid allocating the complete JavaScript property map.
    if let Some(name) = name {
        return Ok(style.as_ref().map_or_else(JsValue::undefined, |style| {
            style.get(&name).map_or_else(JsValue::undefined, |value| {
                js_string!(computed_style_property_to_css_string(&name, value, style)).into()
            })
        }));
    }
    if args.get(2).is_some_and(JsValue::to_boolean) {
        // CSSOM uses an object directly, avoiding an escaped JSON round trip.
        let object = JsObject::with_object_proto(context.intrinsics());
        if let Some(style) = style {
            for (name, value) in style.properties() {
                let value = computed_style_property_to_css_string(&name, &value, &style);
                object.create_data_property_or_throw(
                    js_string!(name.as_str()),
                    js_string!(value.as_str()),
                    context,
                )?;
            }
        }
        return Ok(object.into());
    }
    let json = style
        .as_ref()
        .map_or_else(|| "{}".to_string(), serialize_computed_style);
    Ok(js_string!(json.as_str()).into())
}

/// Reports whether a computed CSSOM property reads a layout-resolved used
/// size. Only `width`, `height` and their logical aliases replace the cascaded
/// value with the box size, so other named reads can skip layout.
fn cssom_property_uses_used_size(name: &str) -> bool {
    matches!(name, "width" | "height" | "inline-size" | "block-size")
}

/// Resolves an owned CSSOM snapshot before constructing any JavaScript objects.
/// Layout runs only when `needs_used_size` asks for resolved `width`/`height`.
fn resolve_native_computed_style(
    args: &[JsValue],
    needs_used_size: bool,
    context: &mut Context,
) -> JsResult<Option<ComputedStyle>> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let pseudo = match args.get(1) {
        Some(value) if !value.is_null_or_undefined() => {
            let name = value.to_string(context)?.to_std_string_escaped();
            if name.starts_with(':') {
                match name.trim().to_ascii_lowercase().as_str() {
                    ":before" | "::before" => Some(PseudoElement::Before),
                    ":after" | "::after" => Some(PseudoElement::After),
                    _ => return Ok(None),
                }
            } else {
                None
            }
        }
        _ => None,
    };
    form_validation::flush(node_id, context)?;
    with_host_state(|state| {
        let Some(node) = state.borrow().get_node(node_id) else {
            return Ok(None);
        };
        if node.node_type() != NodeType::Element {
            return Ok(None);
        }
        // Use the owning document's cascade for iframe nodes; detached nodes
        // have no document and expose an empty declaration.
        let Some(document) = document_root_for_node(&node) else {
            return Ok(None);
        };
        let mut state = state.borrow_mut();
        let Some(mut style) =
            resolve_cssom_style(&mut state, &document, &node, pseudo, needs_used_size)
        else {
            return Ok(None);
        };
        let Some(resolver) = document_style_resolver(&mut state, &document) else {
            return Ok(None);
        };
        let auto_min_size = ["min-width", "min-height"].iter().any(|name| {
            matches!(style.get(name), Some(ComputedValue::Keyword(value)) if value == "auto")
        });
        let flex_or_grid_item = pseudo.is_none() && auto_min_size
            && node.parent_node().is_some_and(|parent| {
                matches!(resolver.computed_style(&parent).get("display"),
                    Some(ComputedValue::Keyword(display))
                        if matches!(display.as_str(), "flex" | "inline-flex" | "grid" | "inline-grid"))
            });
        style.populate_logical_cssom(flex_or_grid_item);
        Ok(Some(style))
    })
}

fn document_style_resolver<'a>(
    state: &'a mut HostState,
    document: &NodeHandle,
) -> Option<&'a mut StyleResolver> {
    state
        .document_styles
        .get_mut(&document.identity())
        .and_then(|entry| entry.resolver.as_mut())
}

/// Computes `node`'s style for CSSOM, laying out the document only when the
/// result depends on it: a resolved `width`/`height` was requested, or the
/// cascade uses `@container` rules or container-relative colors, which read
/// the container sizes captured by the latest layout.
fn resolve_cssom_style(
    state: &mut HostState,
    document: &NodeHandle,
    node: &NodeHandle,
    pseudo: Option<PseudoElement>,
    needs_used_size: bool,
) -> Option<ComputedStyle> {
    let compute = |state: &mut HostState| {
        let resolver = document_style_resolver(state, document)?;
        match pseudo {
            Some(pseudo) => resolver.computed_pseudo_style(node, pseudo),
            None => Some(resolver.computed_style(node)),
        }
    };
    if pseudo.is_none() && needs_used_size {
        let used_size = resolved_layout_size(state, document, node);
        let mut style = compute(state)?;
        if let Some(used_size) = used_size {
            apply_cssom_used_size(&mut style, used_size);
        }
        return Some(style);
    }
    state.ensure_style_resolver(document);
    let style = compute(state)?;
    let needs_containers = pseudo.is_none()
        && document_style_resolver(state, document)
            .is_some_and(|resolver| resolver.needs_container_contexts());
    if !needs_containers {
        return Some(style);
    }
    resolved_layout_size(state, document, node);
    compute(state)
}

/// Applies CSSOM's resolved width and height while retaining box sizing rules.
fn apply_cssom_used_size(style: &mut ComputedStyle, size: LayoutUsedSize) {
    let border_box = crate::layout::is_border_box(style);
    if resolved_width_applies(style) {
        style.set_resolved_px(
            "width",
            if border_box {
                size.border_width
            } else {
                size.content_width
            },
        );
    }
    if resolved_height_applies(style) {
        style.set_resolved_px(
            "height",
            if border_box {
                size.border_height
            } else {
                size.content_height
            },
        );
    }
}

/// Resolves an optional native node argument used by content-visibility hooks.
fn optional_node_argument(
    value: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<Option<NodeHandle>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    let node_id = parse_node_id(Some(value), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| Ok(state.borrow().get_node(node_id)))
}

fn set_content_visibility_focus_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node = optional_node_argument(args.first(), context)?;
    with_host_state(|state| {
        state
            .borrow_mut()
            .set_content_visibility_focus(node.as_ref());
        Ok(())
    })?;
    Ok(JsValue::undefined())
}

fn set_content_visibility_selection_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let start = optional_node_argument(args.first(), context)?;
    let end = optional_node_argument(args.get(1), context)?;
    with_host_state(|state| {
        state
            .borrow_mut()
            .set_content_visibility_selection(start.as_ref(), end.as_ref());
        Ok(())
    })?;
    Ok(JsValue::undefined())
}

fn content_visibility_skips_inner_text_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let skipped = node.is_some_and(|node| {
            state
                .borrow_mut()
                .content_visibility_skips_inner_text(&node)
        });
        Ok(JsValue::from(skipped))
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
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else {
            return Ok(JsValue::from(false));
        };
        if !state.borrow().node_is_in_active_document(&node) {
            return Ok(JsValue::from(false));
        }
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
            let Some(style) = style else {
                return Ok(JsValue::from(false));
            };
            if matches!(style.get("display"), Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("none"))
            {
                return Ok(JsValue::from(false));
            }
            if element.identity() != node.identity()
                && matches!(
                    style.get("content-visibility"),
                    Some(ComputedValue::Keyword(value))
                        if value.eq_ignore_ascii_case("hidden")
                )
            {
                return Ok(JsValue::from(false));
            }
            current = element.assigned_slot().or_else(|| {
                element.parent_node().and_then(|parent| {
                    if parent.node_type() == NodeType::Element {
                        Some(parent)
                    } else {
                        parent.shadow_host()
                    }
                })
            });
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

fn parser_form_owner_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let owner = state
            .borrow()
            .get_node(node_id)
            .and_then(|node| node.parser_form_owner());
        Ok(node_to_js_value(owner))
    })
}

fn set_form_associated_custom_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let associated = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::typ().with_message("Element required")))?;
        node.set_form_associated_custom(associated);
        state.borrow_mut().invalidate_style_cache_for_node(&node);
        Ok(JsValue::undefined())
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
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let disabled = state
            .borrow()
            .get_node(node_id)
            .is_some_and(|node| is_actually_disabled(&node));
        Ok(JsValue::from(disabled))
    })
}

/// Returns the epoch of the geometry exposed to CSSOM. Sampling transitions
/// before reading it also invalidates cached metrics when time advances without
/// a DOM mutation. BigInt preserves the existing u64 generation exactly.
fn layout_metrics_generation_native(
    _: &JsValue,
    _: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let document = state.document.clone();
        state.ensure_style_resolver(&document);
        Ok(boa_engine::JsBigInt::from(state.layout_metrics_generation).into())
    })
}

/// Returns one or all painted element IDs at a viewport point. `null` means
/// the point is outside the target document's viewport; an empty array means
/// it is inside but no painted element accepted pointer events there.
fn hit_test_point_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, document_id)?;
    let x = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as f32;
    let y = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as f32;
    let all = args.get(3).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let Some(document) = state.get_node(document_id) else {
            return Ok(JsValue::null());
        };
        if document.node_type() != NodeType::Document || !state.document_is_active(document_id) {
            return Ok(JsValue::null());
        }
        let viewport = state.viewport_for_document(&document);
        if !x.is_finite()
            || !y.is_finite()
            || x < 0.0
            || y < 0.0
            || x > viewport.width
            || y > viewport.height
        {
            return Ok(JsValue::null());
        }
        let hits = hit_test_document_point(&mut state, &document, viewport, x, y, all);
        let ids = hits
            .into_iter()
            .map(|node| state.retarget_content_visibility_hit(node))
            .map(|node| JsValue::from(node.identity() as f64))
            .collect::<Vec<_>>();
        drop(state);
        Ok(JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(ids, context),
        ))
    })
}

fn hit_test_document_point(
    state: &mut HostState,
    document: &NodeHandle,
    viewport: Rect,
    x: f32,
    y: f32,
    all: bool,
) -> Vec<NodeHandle> {
    let document_id = document.identity();
    if document_id == state.document.identity() {
        state.ensure_adjusted_layout();
        let Some(layout) = state
            .adjusted_layout_cache
            .as_ref()
            .map(|cache| &cache.root)
        else {
            return Vec::new();
        };
        let Some(resolver) = state
            .document_styles
            .get_mut(&document_id)
            .and_then(|entry| entry.resolver.as_mut())
        else {
            return Vec::new();
        };
        return if all {
            crate::paint::hit_test_layout_all(layout, resolver, viewport, x, y)
        } else {
            crate::paint::hit_test_layout(layout, resolver, viewport, x, y)
                .into_iter()
                .collect()
        };
    }

    let Some(mut layout) = build_child_document_layout(state, document, viewport) else {
        return Vec::new();
    };
    let scroll = state.window_scroll_for_document(document_id);
    let Some(resolver) = state
        .document_styles
        .get_mut(&document_id)
        .and_then(|entry| entry.resolver.as_mut())
    else {
        return Vec::new();
    };
    crate::paint::apply_scroll_offsets(&mut layout, resolver, viewport, scroll);
    if all {
        crate::paint::hit_test_layout_all(&layout, resolver, viewport, x, y)
    } else {
        crate::paint::hit_test_layout(&layout, resolver, viewport, x, y)
            .into_iter()
            .collect()
    }
}

/// Lays out an active child document in its own viewport. The main document's
/// cached tree contains its iframe box, but not the iframe's document boxes.
fn build_child_document_layout(
    state: &mut HostState,
    document: &NodeHandle,
    viewport: Rect,
) -> Option<LayoutBox> {
    let document_id = document.identity();
    state.ensure_style_resolver(document);
    let base = crate::paint::stylesheet::extract_document_base_url(
        document,
        state.base_url_for_document(document_id).as_ref(),
    );
    let image_site = state.location_href.parse::<crate::http::Url>().ok();
    let image_cookies = Arc::clone(&state.cookie_store);
    let animation_time = state.event_loop.rendering_time_ms() as u64;
    let entry = state.document_styles.get_mut(&document_id)?;
    let resolver = entry.resolver.as_mut()?;
    crate::layout::with_layout_fonts(
        crate::paint::text::load_text_fonts(),
        Some(entry.web_fonts.clone()),
        || {
            crate::layout::with_image_cookie_store(image_cookies, image_site, document_id, || {
                crate::layout::with_image_base_url(base, || {
                    crate::layout::with_image_animation_time(animation_time, || {
                        crate::layout::layout_tree(document, resolver, viewport)
                    })
                })
            })
        },
    )
}

fn metrics_in_layout(root: &LayoutBox, node: &NodeHandle) -> LayoutMetrics {
    let mut fragments = Vec::new();
    if let Some((layout, transform)) =
        find_layout_box_with_transform(root, node, AffineTransform::identity(), &mut fragments)
    {
        compute_transformed_layout_metrics(layout, transform)
    } else {
        compute_replaced_fragment_metrics(fragments)
    }
}

fn child_document_layout_metrics(
    state: &mut HostState,
    document: &NodeHandle,
    node: &NodeHandle,
    viewport: Rect,
) -> LayoutMetrics {
    let Some(layout) = build_child_document_layout(state, document, viewport) else {
        return LayoutMetrics::zero();
    };
    let mut metrics = metrics_in_layout(&layout, node);
    let scroll = state.window_scroll_for_document(document.identity());
    let Some(resolver) = state
        .document_styles
        .get_mut(&document.identity())
        .and_then(|entry| entry.resolver.as_mut())
    else {
        return metrics;
    };
    let mut painted_layout = layout.clone();
    crate::paint::apply_scroll_offsets(&mut painted_layout, resolver, viewport, scroll);
    let painted = metrics_in_layout(&painted_layout, node);
    if painted.has_box {
        metrics.x = painted.x;
        metrics.y = painted.y;
        metrics.width = painted.width;
        metrics.height = painted.height;
        metrics.client_rects = painted.client_rects;
    }
    metrics
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
    #[cfg(test)]
    TEST_LAYOUT_METRICS_CALLS.with(|count| count.set(count.get() + 1));
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    form_validation::flush(node_id, context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else {
            return Ok(js_string!(LayoutMetrics::zero().to_json().as_str()).into());
        };
        if !state.borrow().node_is_in_active_document(&node) {
            return Ok(js_string!(LayoutMetrics::zero().to_json().as_str()).into());
        }
        let is_root_element = node
            .parent_node()
            .is_some_and(|parent| parent.node_type() == NodeType::Document);
        let document = document_root_for_node(&node);
        let metrics = {
            let mut state = state.borrow_mut();
            let viewport = document
                .as_ref()
                .map(|document| state.viewport_for_document(document));
            state.ensure_content_visibility_geometry(&node);
            let current_scroll = state.window_scroll;
            state.set_window_scroll(current_scroll.0, current_scroll.1);
            let main_document_id = state.document.identity();
            let is_main_document = document
                .as_ref()
                .is_some_and(|document| document.identity() == main_document_id);
            // Client geometry comes from the same paint-time clone used by hit
            // testing and rendering. Besides ordinary scroll offsets this applies
            // `position: sticky` without changing document-coordinate layout.
            if is_main_document {
                state.ensure_adjusted_layout();
            }
            let mut metrics = if is_main_document {
                let mut metrics = LayoutMetrics::zero();
                if let Some(root) = state.layout_root.as_ref() {
                    let mut fragments = Vec::new();
                    if let Some((layout, transform)) = find_layout_box_with_transform(
                        root,
                        &node,
                        AffineTransform::identity(),
                        &mut fragments,
                    ) {
                        metrics = compute_transformed_layout_metrics(layout, transform);
                    } else {
                        metrics = compute_replaced_fragment_metrics(fragments);
                    }

                    if let Some(painted_root) = state
                        .adjusted_layout_cache
                        .as_ref()
                        .map(|cache| &cache.root)
                    {
                        let mut painted_fragments = Vec::new();
                        let painted = if let Some((layout, transform)) =
                            find_layout_box_with_transform(
                                &painted_root,
                                &node,
                                AffineTransform::identity(),
                                &mut painted_fragments,
                            ) {
                            compute_transformed_layout_metrics(layout, transform)
                        } else {
                            compute_replaced_fragment_metrics(painted_fragments)
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
                metrics
            } else {
                document.as_ref().zip(viewport).map_or_else(
                    LayoutMetrics::zero,
                    |(document, viewport)| {
                        child_document_layout_metrics(&mut state, document, &node, viewport)
                    },
                )
            };
            if is_root_element && let Some(viewport) = viewport {
                metrics.client_width = viewport.width;
                metrics.client_height = viewport.height;
                metrics.client_top = 0.0;
                metrics.client_left = 0.0;
                metrics.scroll_width = metrics.scroll_width.max(viewport.width);
                metrics.scroll_height = metrics.scroll_height.max(viewport.height);
            }
            metrics
        };
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
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let (x, y) = match node {
            Some(node) => {
                let mut state = state.borrow_mut();
                state.element_scroll_offset(&node)
            }
            None => (0.0, 0.0),
        };
        let json = format!("{{\"x\":{},\"y\":{}}}", json_number(x), json_number(y));
        Ok(js_string!(json).into())
    })
}

/// Sets or animates an element's scroll offset. Non-finite coordinates scroll
/// to zero, matching how browsers normalize them.
fn set_element_scroll_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let coordinate = |value: Option<&JsValue>, context: &mut Context| -> JsResult<f32> {
        let value = value.cloned().unwrap_or_default().to_number(context)? as f32;
        Ok(if value.is_finite() { value } else { 0.0 })
    };
    let x = coordinate(args.get(1), context)?;
    let y = coordinate(args.get(2), context)?;
    let smooth = args.get(3).and_then(JsValue::as_boolean).unwrap_or(false);
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else {
            return Ok(JsValue::undefined());
        };
        state.borrow_mut().scroll_element_to(&node, x, y, smooth);
        Ok(JsValue::undefined())
    })
}

fn context_document_id(context: &Context, state: &HostState) -> usize {
    context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|document| document.0)
        .unwrap_or_else(|| state.document.identity())
}

/// Returns the visual viewport for the Window Realm making the call.
fn visual_viewport_state_native(
    _: &JsValue,
    _: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let document_id = context_document_id(context, &state);
        let document = state
            .get_node(document_id)
            .filter(|node| node.node_type() == NodeType::Document)
            .unwrap_or_else(|| state.document.clone());
        let viewport = state.visual_viewport_for_document(&document);
        if document_id != state.document.identity() {
            let next = (viewport.width, viewport.height);
            if let Some(previous) = state.observed_iframe_viewports.insert(document_id, next)
                && previous != next
            {
                state.queue_window_resize(document_id);
                state.queue_visual_viewport_resize(document_id);
            }
        }
        let scroll = state.window_scroll_for_document(document_id);
        Ok(js_string!(viewport.json(scroll)).into())
    })
}

/// Returns the calling Window's scroll offset as a JSON object.
fn window_scroll_offset_native(
    _: &JsValue,
    _: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let (x, y) = {
            let mut state = state.borrow_mut();
            let document_id = context_document_id(context, &state);
            state.window_scroll_for_document(document_id)
        };
        let json = format!("{{\"x\":{},\"y\":{}}}", json_number(x), json_number(y));
        Ok(js_string!(json).into())
    })
}

/// Sets or animates the calling Window's scroll offset.
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
    let smooth = args.get(2).and_then(JsValue::as_boolean).unwrap_or(false);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let document_id = context_document_id(context, &state);
        state.scroll_document_to(document_id, x, y, smooth);
        Ok(JsValue::undefined())
    })
}

fn set_timeout_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    schedule_timer_from_js(args, context, false)
}

fn call_event_listener_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let listener = args.first().cloned().unwrap_or_default();
    let this = args.get(1).cloned().unwrap_or_default();
    let event = args.get(2).cloned().unwrap_or_default();
    let callback = if let Some(callback) = listener.as_callable() {
        callback.clone()
    } else {
        let Some(listener) = listener.as_object() else {
            return Ok(JsValue::undefined());
        };
        let handle_event = listener
            .get(js_string!("handleEvent"), context)
            .map_err(|error| {
                report_active_js_task_failure("JS_EVENT_LISTENER_FAILED", "event-listener");
                error
            })?;
        let Some(callback) = handle_event.as_callable() else {
            report_active_js_task_failure("JS_EVENT_LISTENER_FAILED", "event-listener");
            return Err(JsNativeError::typ()
                .with_message("event listener handleEvent is not callable")
                .into());
        };
        callback.clone()
    };
    context.call_with_native_continuation(
        &callback,
        &this,
        &[event],
        NativeCallContinuation::from_copy_closure_with_captures(
            |result, (), _| {
                if result.is_err() {
                    report_active_js_task_failure("JS_EVENT_LISTENER_FAILED", "event-listener");
                }
                result
            },
            (),
        ),
    )
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

/// Associates a host task with the active iframe Realm, if the current Boa
/// context is executing one. Top-level tasks retain the historical payload
/// shape; child tasks carry their Document identity so stale navigation tasks
/// can be ignored when they eventually reach the event loop.
fn bind_timer_payload_to_current_realm(context: &Context, payload: TimerPayload) -> TimerPayload {
    let realm = context.realm().clone();
    let document_id = ACTIVE_HOST_STATE.with(|slot| {
        let state = slot.borrow().clone()?;
        let state = state.borrow();
        state.iframe_documents.values().find_map(|entry| {
            entry
                .realm
                .as_ref()
                .is_some_and(|active| active == &realm)
                .then_some(entry.document.identity())
        })
    });
    match document_id {
        Some(document_id) => TimerPayload::Realm {
            payload: Box::new(payload),
            realm,
            document_id,
        },
        None => payload,
    }
}

/// Captures the active child browsing-context Realm, if the current host
/// invocation is running page code inside one. `Realm` is an explicit Boa
/// `Rooted<RealmInner>` handle, so retaining the returned clone keeps the
/// child intrinsics/global alive until the callback is consumed.
fn current_iframe_realm(context: &Context) -> Option<(Realm, usize)> {
    let realm = context.realm().clone();
    ACTIVE_HOST_STATE.with(|slot| {
        let state = slot.borrow().clone()?;
        let state = state.borrow();
        state.iframe_documents.values().find_map(|entry| {
            entry
                .realm
                .as_ref()
                .filter(|active| *active == &realm)
                .map(|active| (active.clone(), entry.document.identity()))
        })
    })
}

fn request_animation_frame_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("requestAnimationFrame callback must be callable")
            .into());
    }
    let binding = current_iframe_realm(context);
    // Resolve ownership from the executing Context Realm first. Inline child
    // scripts run in a cached iframe Realm even when no asynchronous module
    // guard is active; deriving the Document here keeps their timers scoped
    // without leaking a thread-local owner into unrelated host work. The
    // immutable Realm owner also identifies an already-retired child.
    let owner_document_id = context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|owner| owner.0)
        .or_else(active_document_id);
    with_host_state(|state| {
        let (realm, document_id) = binding
            .map(|(realm, document_id)| (Some(realm), Some(document_id)))
            .unwrap_or((None, owner_document_id));
        let owner_is_live =
            document_id.is_none_or(|document_id| state.borrow().document_is_active(document_id));
        let id = state
            .borrow_mut()
            .event_loop
            .schedule_animation_frame_with_realm(callback, realm, document_id);
        if !owner_is_live {
            state.borrow_mut().event_loop.cancel_animation_frame(id);
        }
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
        state.borrow_mut().event_loop.cancel_animation_frame(id);
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
    let payload = bind_timer_payload_to_current_realm(context, payload);

    // Resolve ownership from the executing Context Realm first. Promise jobs
    // run after the host task's module-document guard has been restored, so
    // relying on the thread-local owner alone would leave timers created by a
    // child callback unowned and uncancellable when its iframe is retired.
    // Read the immutable owner even after its active iframe entry is gone.
    let owner_document_id = context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|owner| owner.0)
        .or_else(active_document_id);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let owner_is_live =
            owner_document_id.is_none_or(|document_id| state.document_is_active(document_id));
        let id = state
            .event_loop
            .schedule_timer(payload, delay_ms, repeat, owner_document_id);
        if !owner_is_live {
            state.event_loop.clear_timer(id);
        }
        Ok(JsValue::from(id as f64))
    })
}

// Inspect the DOM without creating wrappers or invoking user-replaceable
// childNodes/getAttribute properties for every descendant. A stack keeps deep
// trees off both the JavaScript and Rust call stacks.
fn get_element_by_id_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let expected = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    // An empty id attribute does not give the element an ID.
    if expected.is_empty() {
        return Ok(JsValue::null());
    }
    with_host_state(|state| {
        let Some(root) = state.borrow().get_node(node_id) else {
            return Ok(JsValue::null());
        };
        let mut pending = root.child_nodes();
        pending.reverse();
        while let Some(node) = pending.pop() {
            if node.node_type() == NodeType::Element
                && node.get_attribute("id").as_deref() == Some(expected.as_str())
            {
                return Ok(node_to_js_value(Some(node)));
            }
            // Ordinary children exclude shadow trees, template contents and
            // iframe Documents. Reverse insertion preserves document order.
            pending.extend(node.child_nodes().into_iter().rev());
        }
        Ok(JsValue::null())
    })
}

fn subtree_has_window_name_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let Some(root) = state.borrow().get_node(node_id) else {
            return Ok(JsValue::from(false));
        };
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            match node.node_type() {
                NodeType::Element => {
                    let tag = node.local_name().unwrap_or_default();
                    let has_id = node.get_attribute("id").is_some_and(|id| !id.is_empty());
                    let has_name = matches!(tag.as_str(), "embed" | "form" | "img" | "object")
                        && node
                            .get_attribute("name")
                            .is_some_and(|name| !name.is_empty());
                    if has_id || matches!(tag.as_str(), "iframe" | "frame") || has_name {
                        return Ok(JsValue::from(true));
                    }
                    pending.extend(node.child_nodes());
                }
                NodeType::Document | NodeType::DocumentFragment => {
                    pending.extend(node.child_nodes());
                }
                _ => {}
            }
        }
        Ok(JsValue::from(false))
    })
}

fn node_index_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let index = state.borrow().get_node(node_id).and_then(|node| {
            node.parent_node()?
                .child_nodes()
                .iter()
                .position(|child| child.identity() == node_id)
        });
        Ok(JsValue::from(index.map_or(-1.0, |index| index as f64)))
    })
}

fn node_is_connected_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let connected = state
            .borrow()
            .get_node(node_id)
            .is_some_and(|node| document_root_for_node(&node).is_some());
        Ok(JsValue::from(connected))
    })
}

fn get_popover_open_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let open = state
            .borrow()
            .get_node(node_id)
            .is_some_and(|node| node.is_popover_open());
        Ok(JsValue::from(open))
    })
}

fn set_popover_open_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let open = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id).ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("Illegal invocation"))
        })?;
        let order = node.set_popover_open(open);
        state.borrow_mut().invalidate_style_cache_for_node(&node);
        Ok(JsValue::from(order.map_or(-1.0, |value| value as f64)))
    })
}

fn set_modal_dialog_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let modal = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id).ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("Illegal invocation"))
        })?;
        let order = node.set_modal_dialog(modal);
        state.borrow_mut().invalidate_style_cache_for_node(&node);
        Ok(JsValue::from(order.map_or(-1.0, |value| value as f64)))
    })
}

fn fullscreen_element_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .fullscreen_element(document_id)
            .map_or(
                JsValue::null(),
                |node| JsValue::from(node.identity() as f64),
            ))
    })
}

fn fullscreen_enabled_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        Ok(JsValue::from(
            state.borrow().fullscreen_allowed_for_document(document_id),
        ))
    })
}

fn request_fullscreen_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id).ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("Illegal invocation"))
        })?;
        Ok(JsValue::from(state.borrow_mut().request_fullscreen(&node)))
    })
}

fn exit_fullscreen_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    let require_host_approval = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        if state.fullscreen_element(document_id).is_none() {
            return Ok(JsValue::from(false));
        }
        Ok(JsValue::from(
            state.fully_exit_fullscreen(require_host_approval),
        ))
    })
}

fn fullscreen_subtree_removed_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id).ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("Illegal invocation"))
        })?;
        Ok(JsValue::from(
            state.borrow_mut().fullscreen_subtree_will_be_removed(&node),
        ))
    })
}

fn node_is_inclusive_descendant_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let ancestor_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_node(context, ancestor_id)?;
    with_host_state(|state| {
        let mut current = state.borrow().get_node(node_id);
        while let Some(node) = current {
            if node.identity() == ancestor_id {
                return Ok(JsValue::from(true));
            }
            // Range and traversal ancestry stops at ordinary tree roots;
            // unlike connectivity it must not cross a shadow host.
            current = node.parent_node();
        }
        Ok(JsValue::from(false))
    })
}

fn node_has_slot_ancestor_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let mut current = state.borrow().get_node(node_id);
        while let Some(node) = current {
            // This is a conservative filter. The bootstrap still applies the
            // exact HTML-slot, shadow-root and fallback-assignment checks.
            if node
                .local_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("slot"))
            {
                return Ok(JsValue::from(true));
            }
            current = node.parent_node();
        }
        Ok(JsValue::from(false))
    })
}

fn query_selector_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let selector = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let selectors = parse_dom_selector_list(&selector)?;
    if form_validation::selector_uses_validation(&selectors) {
        form_validation::flush(node_id, context)?;
    }
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
    let creator = caller_document_id(context);
    with_host_state(|state| {
        let node = NodeHandle::element(tag_name);
        let id = node.identity();
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
        Ok(JsValue::from(id as f64))
    })
}

fn create_element_ns_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
    let namespace = match args.first() {
        Some(value) if !value.is_null() && !value.is_undefined() => {
            Some(value.to_string(context)?.to_std_string_escaped())
        }
        _ => None,
    };
    let qualified_name = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let creator = caller_document_id(context);
    with_host_state(|state| {
        let node = if namespace.as_deref() == Some(HTML_NAMESPACE) {
            NodeHandle::html_element_ns(qualified_name, HTML_NAMESPACE)
        } else {
            NodeHandle::xml_element(qualified_name, namespace)
        };
        let id = node.identity();
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
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
    ensure_same_origin_node(context, parent_id)?;
    let child_id = parse_node_id(args.get(1), context)?;
    ensure_same_origin_node(context, child_id)?;
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
            if source_document.is_some() {
                state.destroy_iframe_contexts_in_subtree(&child);
            }
            // Inserting an already-connected iframe performs removing steps
            // first, even when the destination is the same Document. Its old
            // context is therefore destroyed above and a fresh resource load
            // is queued below. Scripts only re-run for an actual cross-document
            // move; iframe/object navigation applies to every insertion.
            if target_document.is_some() {
                state.schedule_connected_resource_loads(&child, source_document != target_document);
            }
        }
        Ok(JsValue::from(child.identity() as f64))
    })
}

fn parent_node_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        Ok(node_to_js_value(node.and_then(|node| node.parent_node())))
    })
}

/// Returns the node's current Document owner. Detached nodes retain the
/// registered owner from their most recent adoption, shared by all Realms.
/// A Document itself has no owner; an unregistered node may use the JS fallback.
fn owner_document_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let s = state.borrow();
        let Some(node) = s.get_node(node_id) else {
            return Ok(JsValue::null());
        };
        if node.node_type() == NodeType::Document {
            return Ok(JsValue::null());
        }
        Ok(node_to_js_value(
            owner_document_for_node(&node).or_else(|| s.node_lifetime_owner(node_id)),
        ))
    })
}

/// `__omoikane_document_owner_iframe(documentId)` — returns the node id of the
/// `<iframe>` element that owns the sub-browsing-context document `documentId`,
/// or `null` when it is the top-level (main) document or a reloaded/stale
/// document whose retained origin still matches the caller. Unknown IDs fail
/// the origin check with `SecurityError`.
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
    ensure_same_origin_document(context, document_id)?;
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
        // A retained but reloaded sub-document has no live owning iframe.
        Ok(JsValue::null())
    })
}

/// Returns the URL committed for a live top-level or nested Document. Unknown
/// and retired documents return `null` rather than inheriting the current
/// top-level Location.
fn document_url_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .document_urls
            .get(&document_id)
            .map(|url| JsValue::from(js_string!(url.as_str())))
            .unwrap_or_else(JsValue::null))
    })
}

/// Keeps the committed main Document URL in sync before `pushState` and
/// `replaceState` return to page script. The session owner records the history
/// entry separately after the script evaluation completes.
fn commit_history_api_url_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = args
        .first()
        .ok_or_else(|| JsNativeError::typ().with_message("History URL is required"))?
        .to_string(context)?
        .to_std_string_escaped();
    let parsed = url.parse::<crate::http::Url>().ok();
    let caller = caller_document_id(context);
    let allowed = with_host_state(|host| {
        let state = host.borrow();
        let document_id = state.document.identity();
        Ok(caller == Some(document_id)
            && (url == state.location_href
                || matches!(
                    (state.document_security_origins.get(&document_id), parsed.as_ref()),
                    (Some(DocumentSecurityOrigin::Tuple(origin)), Some(url))
                        if StorageOrigin::from_url(&url.to_string()).as_ref() == Some(origin)
                )))
    })?;
    if !allowed {
        return Err(cross_origin_access_error(context)?);
    }
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let document_id = state.document.identity();
        state.location_href = url.clone();
        state.document_urls.insert(document_id, url);
        if let Some(base_url) = parsed {
            state.set_main_base_url(base_url);
        }
        Ok(JsValue::undefined())
    })
}

/// Makes a same-Document fragment target visible to the current script before
/// the navigation owner later records its history entry and dispatches events.
fn commit_fragment_url_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = args
        .first()
        .ok_or_else(|| JsNativeError::typ().with_message("Fragment URL is required"))?
        .to_string(context)?
        .to_std_string_escaped();
    let caller = caller_document_id(context);
    let allowed = with_host_state(|host| Ok(caller == Some(host.borrow().document.identity())))?;
    if !allowed {
        return Err(cross_origin_access_error(context)?);
    }
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        let (current_base, current_fragment) = state
            .location_href
            .split_once('#')
            .unwrap_or((&state.location_href, ""));
        let (next_base, next_fragment) = url.split_once('#').unwrap_or((&url, ""));
        if current_base != next_base || current_fragment == next_fragment {
            return Ok(JsValue::from(false));
        }
        let document = state.document.clone();
        state.location_href = url.clone();
        state.document_urls.insert(document.identity(), url.clone());
        if let Ok(base_url) = url.parse::<crate::http::Url>() {
            state.set_main_base_url(base_url);
        }
        state.update_document_target(&document, &url);
        Ok(JsValue::from(true))
    })
}

/// Returns the effective HTTP(S) base URL for a live Document. A missing base
/// is represented as `null` so relative parsing in `data:`/unknown documents
/// cannot silently borrow the top-level Document's URL.
fn document_base_url_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .document_base_urls
            .get(&document_id)
            .map(|url| JsValue::from(js_string!(url.to_string())))
            .unwrap_or_else(JsValue::null))
    })
}

fn node_name_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let names: Vec<JsValue> = node
            .and_then(|node| node.attribute_records())
            .map(|records| {
                records
                    .into_iter()
                    .map(|(name, _, _, _)| js_string!(name.as_str()).into())
                    .collect()
            })
            .unwrap_or_default();
        Ok(boa_engine::JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(names, context),
        ))
    })
}

fn attribute_records_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let records = with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(node_id)
            .and_then(|node| node.attribute_records())
            .unwrap_or_default())
    })?;
    let rows: Vec<(JsValue, JsValue, JsValue, JsValue)> = records
        .into_iter()
        .map(|(qualified_name, namespace_uri, local_name, value)| {
            (
                js_string!(qualified_name.as_str()).into(),
                namespace_uri
                    .map(|namespace| js_string!(namespace.as_str()).into())
                    .unwrap_or_else(JsValue::null),
                js_string!(local_name.as_str()).into(),
                js_string!(value.as_str()).into(),
            )
        })
        .collect();
    // `TryIntoJs` roots each row array and the outer array while it appends
    // values. A plain Rust `Vec<JsValue>` is invisible to Boa's collector.
    rows.try_into_js(context)
}

fn attribute_record_count_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let count = state
            .borrow()
            .get_node(node_id)
            .and_then(|node| node.attribute_record_count())
            .unwrap_or(0);
        Ok(JsValue::from(count as f64))
    })
}

fn attribute_record_at_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let Some(index) = args
        .get(1)
        .and_then(JsValue::as_number)
        .filter(|index| index.is_finite() && *index >= 0.0)
        .map(|index| index as usize)
    else {
        return Ok(JsValue::null());
    };
    let record = with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(node_id)
            .and_then(|node| node.attribute_record_at(index)))
    })?;
    let Some((name, namespace, local_name, value)) = record else {
        return Ok(JsValue::null());
    };
    let row: (JsValue, JsValue, JsValue, JsValue) = (
        js_string!(name.as_str()).into(),
        namespace
            .map(|namespace| js_string!(namespace.as_str()).into())
            .unwrap_or_else(JsValue::null),
        js_string!(local_name.as_str()).into(),
        js_string!(value.as_str()).into(),
    );
    row.try_into_js(context)
}

fn attribute_value_ns_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let namespace = args
        .get(1)
        .filter(|value| !value.is_null_or_undefined())
        .map(|value| {
            value
                .to_string(context)
                .map(|value| value.to_std_string_escaped())
        })
        .transpose()?;
    let local_name = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        Ok(state
            .borrow()
            .get_node(node_id)
            .and_then(|node| node.attribute_value_ns(namespace.as_deref(), &local_name))
            .map(|value| js_string!(value.as_str()).into())
            .unwrap_or_else(JsValue::null))
    })
}

fn array_buffer_argument(args: &[JsValue]) -> JsResult<JsArrayBuffer> {
    let object = args
        .first()
        .and_then(JsValue::as_object)
        .ok_or_else(|| JsNativeError::typ().with_message("value is not an ArrayBuffer"))?;
    JsArrayBuffer::from_object(object)
}

fn array_buffer_info_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(object) = args.first().and_then(JsValue::as_object) else {
        return Ok(JsValue::null());
    };
    let Ok(buffer) = JsArrayBuffer::from_object(object) else {
        return Ok(JsValue::null());
    };
    let detached = buffer.data().is_none();
    let fixed = buffer.is_fixed_length();
    let max_byte_length = buffer.max_byte_length(context)?;
    Ok(JsArray::from_iter(
        [
            JsValue::from(detached),
            JsValue::from(fixed),
            JsValue::from(max_byte_length as f64),
        ],
        context,
    )
    .into())
}

fn clone_array_buffer_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let buffer = array_buffer_argument(args)?;
    let max_byte_length = (!buffer.is_fixed_length()).then(|| buffer.max_byte_length(context));
    let max_byte_length = max_byte_length.transpose()?;
    let data = buffer
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("ArrayBuffer is detached"))?;
    let data = AlignedVec::from_iter(0, data.iter().copied());
    let cloned = JsArrayBuffer::from_byte_block(data, context)?;
    Ok(match max_byte_length {
        Some(max) => cloned.with_max_byte_length(max as u64).into(),
        None => cloned.into(),
    })
}

fn transfer_array_buffer_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let buffer = array_buffer_argument(args)?;
    let max_byte_length = (!buffer.is_fixed_length()).then(|| buffer.max_byte_length(context));
    let max_byte_length = max_byte_length.transpose()?;
    let data = buffer.detach(&JsValue::undefined())?;
    let transferred = JsArrayBuffer::from_byte_block(data, context)?;
    Ok(match max_byte_length {
        Some(max) => transferred.with_max_byte_length(max as u64).into(),
        None => transferred.into(),
    })
}

fn typed_array_kind_name(kind: TypedArrayKind) -> &'static str {
    match kind {
        TypedArrayKind::Int8 => "Int8Array",
        TypedArrayKind::Uint8 => "Uint8Array",
        TypedArrayKind::Uint8Clamped => "Uint8ClampedArray",
        TypedArrayKind::Int16 => "Int16Array",
        TypedArrayKind::Uint16 => "Uint16Array",
        TypedArrayKind::Int32 => "Int32Array",
        TypedArrayKind::Uint32 => "Uint32Array",
        TypedArrayKind::BigInt64 => "BigInt64Array",
        TypedArrayKind::BigUint64 => "BigUint64Array",
        TypedArrayKind::Float16 => "Float16Array",
        TypedArrayKind::Float32 => "Float32Array",
        TypedArrayKind::Float64 => "Float64Array",
    }
}

fn array_buffer_view_info_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let Some(object) = args.first().and_then(JsValue::as_object) else {
        return Ok(JsValue::null());
    };
    if let Ok(view) = JsTypedArray::from_object(object.clone()) {
        let out_of_bounds = view.is_out_of_bounds();
        let buffer = view.buffer(context)?;
        let (offset, length) = if out_of_bounds {
            (JsValue::from(0), JsValue::null())
        } else {
            let length = if view.is_length_tracking() {
                JsValue::null()
            } else {
                JsValue::from(view.length(context)? as f64)
            };
            (JsValue::from(view.byte_offset(context)? as f64), length)
        };
        let kind = view
            .kind()
            .map(typed_array_kind_name)
            .ok_or_else(|| JsNativeError::typ().with_message("unknown TypedArray kind"))?;
        return Ok(JsArray::from_iter(
            [
                js_string!("typed").into(),
                JsString::from(kind).into(),
                buffer,
                offset,
                length,
                JsValue::from(out_of_bounds),
            ],
            context,
        )
        .into());
    }
    if let Ok(view) = JsDataView::from_object(object) {
        let out_of_bounds = view.is_out_of_bounds();
        let buffer = view.buffer(context)?;
        let (offset, length) = if out_of_bounds {
            (JsValue::from(0), JsValue::null())
        } else {
            let length = if view.is_length_tracking() {
                JsValue::null()
            } else {
                JsValue::from(view.byte_length(context)? as f64)
            };
            (JsValue::from(view.byte_offset(context)? as f64), length)
        };
        return Ok(JsArray::from_iter(
            [
                js_string!("data").into(),
                JsValue::null(),
                buffer,
                offset,
                length,
                JsValue::from(out_of_bounds),
            ],
            context,
        )
        .into());
    }
    Ok(JsValue::null())
}

fn get_attribute_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
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

/// Parses a complete `@import` rule through the same parser used by paint and
/// dynamic stylesheet expansion, returning its CSSOM-facing components.
fn css_import_parts_native(
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
    let Some(import) = crate::paint::stylesheet::parse_import_rule(&css) else {
        return Ok(JsValue::null());
    };
    let layer_name = match import.layer {
        Some(crate::paint::stylesheet::ImportLayer::Anonymous) => Some(String::new()),
        Some(crate::paint::stylesheet::ImportLayer::Named(name)) => Some(name),
        None => None,
    };
    let encoded = serde_json::json!({
        "href": import.href,
        "layerName": layer_name,
        "supportsText": import.supports,
        "mediaText": import.media.unwrap_or_default(),
    })
    .to_string();
    Ok(js_string!(encoded.as_str()).into())
}

/// Returns an already loaded imported stylesheet without initiating a fetch.
/// A non-matching `supports()` condition therefore remains `null` in CSSOM.
fn imported_stylesheet_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let href = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|host| {
        let Some(node) = host.borrow().get_node(node_id) else {
            return Ok(JsValue::null());
        };
        let Some(document) = document_root_for_node(&node) else {
            return Ok(JsValue::null());
        };
        let document_id = document.identity();
        let mut host = host.borrow_mut();
        host.ensure_style_resolver(&document);
        let base = crate::paint::stylesheet::extract_document_base_url(
            &document,
            host.base_url_for_document(document_id).as_ref(),
        );
        let Some((text, resolved_href)) =
            host.document_styles.get(&document_id).and_then(|entry| {
                entry
                    .resources
                    .cached_import(&href, base.as_ref(), base.as_ref())
            })
        else {
            return Ok(JsValue::null());
        };
        let encoded = serde_json::json!({"text": text, "href": resolved_href}).to_string();
        Ok(js_string!(encoded.as_str()).into())
    })
}

/// Parses CSS forgivingly and returns each accepted top-level rule as an
/// original source slice.
fn css_rule_sources_native(
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
    let rules = font_descriptors::accepted_rule_sources(&css);
    let rules = serde_json::to_string(&rules)
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
    Ok(js_string!(rules.as_str()).into())
}

/// Returns the validated descriptors of one `@property` rule, or `null` when
/// the rule does not establish a registration.
fn css_property_rule_native(
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
    let sheet = crate::paint::stylesheet::parse_stylesheet_forgiving(&css);
    let registration = match sheet.rules.as_slice() {
        [crate::css::Rule::At(rule)] => {
            crate::css::style::registered_custom_property_from_rule(rule)
        }
        _ => None,
    };
    let Some(registration) = registration else {
        return Ok(JsValue::null());
    };
    let initial_value = registration
        .initial_value
        .as_ref()
        .map(crate::css::serialize_specified_value);
    let encoded = serde_json::json!({
        "name": registration.name,
        "syntax": registration.syntax_text,
        "inherits": registration.inherits,
        "initialValue": initial_value,
    })
    .to_string();
    Ok(js_string!(encoded.as_str()).into())
}

/// Registers a custom property for the currently executing Document.
/// Returns a small status string so the JavaScript binding can create the
/// Web-exposed DOMException with the required name.
fn register_property_native(
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
    let syntax = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| js_string!("*").into())
        .to_string(context)?
        .to_std_string_escaped();
    let inherits = args.get(2).is_some_and(JsValue::to_boolean);
    let initial = args
        .get(3)
        .filter(|value| !value.is_null_or_undefined())
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_escaped());
    let Some(registration) = crate::css::style::parse_registered_custom_property(
        &name,
        &syntax,
        inherits,
        initial.as_deref(),
    ) else {
        return Ok(js_string!("invalid").into());
    };
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let document = state.csp_document_for_context(context)?;
        let document_id = document.identity();
        let registrations = state
            .registered_custom_properties
            .entry(document_id)
            .or_default();
        if registrations.contains_key(&name) {
            return Ok(js_string!("duplicate").into());
        }
        registrations.insert(name, registration);
        state.mark_document_style_dirty(&document);
        Ok(js_string!("ok").into())
    })
}

/// Returns source-preserving declarations for CSSOM descriptor blocks.
fn css_declarations_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let block = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let declarations = font_descriptors::declarations(&block)
        .into_iter()
        .map(|(name, value)| {
            let value = crate::paint::color4::CssColor::parse(&value)
                .map_or(value, |color| color.serialize());
            (name, value)
        })
        .collect::<Vec<_>>();
    let declarations = serde_json::to_string(&declarations)
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
    Ok(js_string!(declarations.as_str()).into())
}

/// Validates every `@scope` prelude in a stylesheet.  The regular stylesheet
/// parser intentionally keeps malformed at-rules so style resolution can
/// discard them forgivingly; CSSOM mutation APIs, however, must reject an
/// invalid scope rule with a SyntaxError before changing the sheet.
fn css_scope_rules_valid_native(
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
    // CSSOM mutation validation is concerned with @scope preludes.  Keep the
    // rest of the stylesheet forgiving so an unrelated malformed rule does
    // not get reported as an invalid scope prelude.
    let sheet = crate::paint::stylesheet::parse_stylesheet_forgiving(&css);
    let valid = scope_rules_valid(&sheet.rules);
    Ok(JsValue::from(valid))
}

fn scope_rules_valid(rules: &[crate::css::Rule]) -> bool {
    rules.iter().all(|rule| match rule {
        crate::css::Rule::At(at_rule) if at_rule.name.eq_ignore_ascii_case("scope") => {
            parse_scope_prelude(&at_rule.prelude).is_some()
                && at_rule.block.as_deref().is_some_and(scope_rules_valid)
        }
        crate::css::Rule::At(at_rule) => at_rule.block.as_deref().is_none_or(scope_rules_valid),
        _ => true,
    })
}

/// Evaluates the two-argument form of `CSS.supports()` against the same parser
/// and supported-property table used by style resolution.
fn css_supports_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
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
/// are canonicalized; strictly-validated CSS properties use native grammar
/// validation so invalid assignments are ignored while valid specified values
/// are preserved for CSSOM serialization.
fn normalize_style_value_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    #[cfg(test)]
    TEST_STYLE_NORMALIZATION_CALLS.with(|count| count.set(count.get() + 1));
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
    let normalized = cssom_normalization::normalize(&property, &value);
    Ok(normalized
        .map(|value| js_string!(value).into())
        .unwrap_or_else(JsValue::null))
}

fn expand_style_shorthand_native(
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
    let declarations = crate::css::parse_style_attribute(&format!("{property}: {value}"));
    let rows = declarations
        .into_iter()
        .map(|declaration| {
            [
                declaration.name,
                crate::css::serialize_specified_value(&declaration.value),
            ]
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_string(&rows)
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
    Ok(js_string!(encoded.as_str()).into())
}

fn take_transition_events_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    with_host_state(|state| {
        let events = {
            let mut state = state.borrow_mut();
            state
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
                .collect::<Vec<_>>()
        };
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
            let full_sample = state.document_styles.get(&document_id).is_none_or(|entry| {
                entry.dirty || entry.needs_full_sample || entry.resolver.is_none()
            });
            state.ensure_style_resolver(&document);
            let running_node_ids = state
                .document_styles
                .get(&document_id)
                .and_then(|entry| entry.resolver.as_ref())
                .map(|resolver| {
                    let mut ids = resolver.running_transition_node_ids();
                    ids.extend(resolver.animation_node_ids());
                    ids.sort_unstable();
                    ids.dedup();
                    ids
                })
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
            let active_node_ids = elements
                .iter()
                .map(NodeHandle::identity)
                .collect::<HashSet<_>>();
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
                resolver.retain_animation_nodes(&active_node_ids);
            }
            if full_sample && let Some(entry) = state.document_styles.get_mut(&document_id) {
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
fn match_media_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
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
                crate::css::evaluate_media_query(query, viewport.width, viewport.height, false)
            })
        });
        Ok(JsValue::from(matches))
    })
}

fn set_attribute_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
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
    let is_name_attribute = name.eq_ignore_ascii_case("name");
    let is_style_attribute = name.eq_ignore_ascii_case("style");
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        // Setting the resource attribute of a connected `<iframe>`/`<object>`
        // starts a fresh navigation; detect it before `name` is moved into
        // `set_attribute`. Any other attribute leaves navigation untouched.
        let resource_attr = node.tag_name().and_then(|tag| {
            if tag.eq_ignore_ascii_case("iframe") && name.eq_ignore_ascii_case("srcdoc") {
                Some("srcdoc")
            } else if (is_nested_frame_tag(&tag) || tag.eq_ignore_ascii_case("script"))
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
        if is_name_attribute {
            state.borrow_mut().refresh_iframe_context_name(&node);
        }
        // Any attribute may participate in a selector (id/class/attribute
        // selectors), so invalidate the element's live document. Detached
        // elements cannot affect it until the insertion path invalidates it.
        if is_style_attribute {
            state.borrow_mut().refresh_csp_inline_style_nodes(&node);
            state.borrow_mut().invalidate_inline_style_for_node(&node);
        } else if matches!(node.tag_name().as_deref(), Some("style" | "link" | "base")) {
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

fn set_attribute_ns_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let namespace = match args.get(1) {
        Some(value) if !value.is_null() && !value.is_undefined() => {
            Some(value.to_string(context)?.to_std_string_escaped())
        }
        _ => None,
    };
    let qualified_name = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let local_name = args
        .get(3)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let value = args
        .get(4)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let replace_qualified_name = args.get(5).and_then(JsValue::as_boolean).unwrap_or(false);
    let is_name_attribute = namespace.is_none() && qualified_name == "name";
    let is_style_attribute = namespace.is_none() && qualified_name == "style";
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let resource_attr = namespace.as_ref().is_none().then(|| {
            node.tag_name().and_then(|tag| {
                if tag.eq_ignore_ascii_case("iframe") && qualified_name == "srcdoc" {
                    Some("srcdoc")
                } else if (is_nested_frame_tag(&tag) || tag.eq_ignore_ascii_case("script"))
                    && qualified_name == "src"
                {
                    Some("src")
                } else if tag.eq_ignore_ascii_case("object") && qualified_name == "data" {
                    Some("data")
                } else {
                    None
                }
            })
        });
        if replace_qualified_name {
            node.replace_xml_attribute_ns(qualified_name, namespace, local_name, value);
        } else {
            node.set_xml_attribute_ns(qualified_name, namespace, local_name, value);
        }
        if is_name_attribute {
            state.borrow_mut().refresh_iframe_context_name(&node);
        }
        if is_style_attribute {
            state.borrow_mut().refresh_csp_inline_style_nodes(&node);
            state.borrow_mut().invalidate_inline_style_for_node(&node);
        } else if matches!(node.tag_name().as_deref(), Some("style" | "link" | "base")) {
            state.borrow_mut().mark_style_dirty_for_node(&node);
        } else {
            state.borrow_mut().invalidate_style_cache_for_node(&node);
        }
        if let Some(Some(resource_attr)) = resource_attr {
            state
                .borrow_mut()
                .schedule_resource_load_on_attribute_change(&node, resource_attr);
        }
        Ok(JsValue::undefined())
    })
}

fn get_checked_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
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
    ensure_same_origin_node(context, node_id)?;
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

fn get_option_selected_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id).ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("Illegal invocation"))
        })?;
        if node.tag_name().as_deref() != Some("option") {
            return Err(JsNativeError::typ()
                .with_message("Illegal invocation")
                .into());
        }
        Ok(JsValue::from(node.selected()))
    })
}

fn set_option_selected_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    let selected = args.get(1).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id).ok_or_else(|| {
            JsError::from(JsNativeError::typ().with_message("Illegal invocation"))
        })?;
        if node.tag_name().as_deref() != Some("option") {
            return Err(JsNativeError::typ()
                .with_message("Illegal invocation")
                .into());
        }
        node.set_selected(selected);
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
    ensure_same_origin_node(context, node_id)?;
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
fn body_bytes_argument(
    value: Option<&JsValue>,
    context: &mut Context,
) -> JsResult<Option<Vec<u8>>> {
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
                    JsNativeError::typ()
                        .with_message("request body is not backed by an ArrayBuffer"),
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
    context: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("file reading task callback must be callable")
            .into());
    }
    let payload = bind_timer_payload_to_current_realm(
        context,
        TimerPayload::Callback {
            callback,
            args: Vec::new(),
        },
    );
    with_host_state(|state| {
        state.borrow_mut().event_loop.enqueue_file_reading(payload);
        Ok(JsValue::undefined())
    })
}

/// Queues a callback on HTML's networking task source.
fn queue_networking_task_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("networking task callback must be callable")
            .into());
    }
    let payload = bind_timer_payload_to_current_realm(
        context,
        TimerPayload::Callback {
            callback,
            args: Vec::new(),
        },
    );
    with_host_state(|state| {
        state.borrow_mut().event_loop.enqueue_networking(payload);
        Ok(JsValue::undefined())
    })
}

/// Queues a callback on HTML's DOM manipulation task source.
fn queue_dom_manipulation_task_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let callback = args.first().cloned().unwrap_or_default();
    if !callback.is_callable() {
        return Err(JsNativeError::typ()
            .with_message("DOM manipulation task callback must be callable")
            .into());
    }
    let payload = bind_timer_payload_to_current_realm(
        context,
        TimerPayload::Callback {
            callback,
            args: Vec::new(),
        },
    );
    with_host_state(|state| {
        state
            .borrow_mut()
            .event_loop
            .enqueue_dom_manipulation(payload);
        Ok(JsValue::undefined())
    })
}

/// Replaces the parsed text snapshot for a Document/ShadowRoot's adopted
/// constructable stylesheets and invalidates that document's style resolver.
fn set_adopted_stylesheets_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let root_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, root_id)?;
    let encoded = string_argument(args.get(1), "[]", context)?;
    let stylesheets: Vec<String> = serde_json::from_str(&encoded).map_err(|_| {
        JsError::from(JsNativeError::typ().with_message("invalid adoptedStyleSheets payload"))
    })?;
    with_host_state(|state| {
        let mut host = state.borrow_mut();
        let root = host.get_node(root_id).ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("adoptedStyleSheets root not found"))
        })?;
        let valid_root = root.node_type() == NodeType::Document
            || (root.node_type() == NodeType::DocumentFragment && root.shadow_host().is_some());
        if !valid_root {
            return Err(JsNativeError::typ()
                .with_message("adoptedStyleSheets root must be a Document or ShadowRoot")
                .into());
        }
        // A ShadowRoot may be configured before its host is connected. Keep
        // the snapshot in that detached tree; style invalidation can start
        // once the host is inserted into a document.
        let document = document_root_for_node(&root);
        if stylesheets.is_empty() {
            host.adopted_stylesheets.remove(&root_id);
        } else {
            host.adopted_stylesheets.insert(root_id, stylesheets);
        }
        if let Some(document) = document {
            host.mark_document_style_dirty(&document);
        }
        Ok(JsValue::undefined())
    })
}

/// Queues a target port and cloned data on HTML's posted message task source.
fn enqueue_posted_message_native(
    _: &JsValue,
    args: &[JsValue],
    _: &mut Context,
) -> JsResult<JsValue> {
    let port = args.first().cloned().unwrap_or_default();
    let data = args.get(1).cloned().unwrap_or_default();
    with_host_state(|state| {
        state
            .borrow_mut()
            .event_loop
            .enqueue_posted_message(port, data);
        Ok(JsValue::undefined())
    })
}

/// Serializes a tuple origin without an explicit default port, as HTML does.
fn serialized_window_origin(origin: Option<&StorageOrigin>) -> String {
    origin
        .and_then(|origin| url::Url::parse(&origin.serialize()).ok())
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| "null".to_owned())
}

/// Web IDL reports Window.postMessage DOMExceptions from the target Window's
/// Realm, even though the value to clone belongs to the incumbent sender.
fn retarget_window_message_error(
    error: JsError,
    kind: &str,
    target_id: usize,
    expected_context: Option<u64>,
    incumbent: &Realm,
    context: &mut Context,
) -> JsError {
    let previous = context.enter_realm(incumbent.clone());
    let details = (|| -> JsResult<Option<(String, String)>> {
        let constructor = context
            .global_object()
            .get(js_string!("DOMException"), context)?;
        let value = error.to_opaque(context);
        if !value.instance_of(&constructor, context)? {
            return Ok(None);
        }
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let name = object
            .get(js_string!("name"), context)?
            .to_string(context)?
            .to_std_string_escaped();
        let message = object
            .get(js_string!("message"), context)?
            .to_string(context)?
            .to_std_string_escaped();
        Ok(Some((name, message)))
    })();
    context.enter_realm(previous);
    let Ok(Some((name, message))) = details else {
        return error;
    };

    let target_realm = with_host_state(|host| {
        if kind == "popup" {
            return ensure_auxiliary_realm(context, host, target_id as u64).map(Some);
        }
        let target = {
            let state = host.borrow();
            if kind == "iframe" {
                state
                    .iframe_context_ids
                    .get(&target_id)
                    .filter(|id| expected_context.is_none_or(|expected| **id == expected))
                    .and_then(|_| state.iframe_documents.get(&target_id))
                    .map(|entry| (target_id, entry.document.identity()))
            } else {
                state
                    .iframe_documents
                    .iter()
                    .find_map(|(iframe_id, entry)| {
                        (entry.document.identity() == target_id).then_some((*iframe_id, target_id))
                    })
            }
        };
        if let Some((iframe_id, document_id)) = target {
            return ensure_iframe_realm(context, host, iframe_id, document_id).map(Some);
        }
        let state = host.borrow();
        Ok((target_id == state.document.identity())
            .then(|| state.main_realm.clone())
            .flatten())
    });
    let Ok(Some(target_realm)) = target_realm else {
        return error;
    };
    let previous = context.enter_realm(target_realm);
    let recreated = (|| -> JsResult<JsValue> {
        let constructor = context
            .global_object()
            .get(js_string!("DOMException"), context)?;
        let constructor = constructor.as_object().ok_or_else(|| {
            JsNativeError::typ().with_message("DOMException constructor is unavailable")
        })?;
        Ok(constructor
            .construct(
                &[
                    JsValue::from(js_string!(message.as_str())),
                    JsValue::from(js_string!(name.as_str())),
                ],
                None,
                context,
            )?
            .into())
    })();
    context.enter_realm(previous);
    recreated.map_or(error, JsError::from_opaque)
}

/// Serializes a Window message in the incumbent script Realm and queues it
/// for the selected browsing context. A cross-Realm method call can execute
/// the target Window's function, so `context.realm()` alone is not the sender.
fn window_post_message_native(
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
    if kind != "document" && kind != "iframe" && kind != "popup" {
        return Err(JsNativeError::typ()
            .with_message("invalid Window message target")
            .into());
    }
    let target_id = parse_node_id(args.get(1), context)?;
    let expected_context = args
        .get(2)
        .filter(|value| !value.is_null_or_undefined())
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_escaped().parse::<u64>())
        .transpose()
        .map_err(|_| JsNativeError::typ().with_message("invalid iframe context id"))?;

    let incumbent = context
        .caller_realm()
        .or_else(|| context.active_script_or_module_realm())
        .unwrap_or_else(|| context.realm().clone());
    let sender_document_id = incumbent
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|document| document.0);
    let previous = context.enter_realm(incumbent.clone());
    let prepared = (|| -> JsResult<(String, String, String, JsValue)> {
        let global = context.global_object();
        let prepare = global.get(js_string!("__omoikane_prepare_window_message"), context)?;
        let callable = prepare.as_callable().ok_or_else(|| {
            JsNativeError::typ().with_message("Window message serializer is unavailable")
        })?;
        let prepared = callable.call(
            &global.clone().into(),
            &[
                args.get(3).cloned().unwrap_or_default(),
                args.get(4).cloned().unwrap_or_default(),
                args.get(5).cloned().unwrap_or_default(),
            ],
            context,
        )?;
        let result = prepared.as_object().ok_or_else(|| {
            JsNativeError::typ().with_message("invalid Window message serialization")
        })?;
        let value = |index, context: &mut Context| -> JsResult<String> {
            Ok(result
                .get(index, context)?
                .to_string(context)?
                .to_std_string_escaped())
        };
        Ok((
            value(0, context)?,
            value(1, context)?,
            value(2, context)?,
            result.get(3, context)?,
        ))
    })();
    context.enter_realm(previous);
    let (wire, _location_origin, target_origin, ports) = prepared.map_err(|error| {
        retarget_window_message_error(
            error,
            &kind,
            target_id,
            expected_context,
            &incumbent,
            context,
        )
    })?;

    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let sender_document_id = sender_document_id.unwrap_or_else(|| state.document.identity());
        let origin = serialized_window_origin(
            state
                .document_origins
                .get(&sender_document_id)
                .and_then(Option::as_ref),
        );
        let sender_security_origin = state
            .document_security_origins
            .get(&sender_document_id)
            .cloned();
        let source_iframe_id = state.iframe_documents.iter().find_map(|(id, entry)| {
            (entry.document.identity() == sender_document_id).then_some(*id)
        });
        let source_auxiliary_id = state.auxiliary_contexts.iter().find_map(|(id, entry)| {
            (entry.document.identity() == sender_document_id).then_some(*id)
        });
        let (target_document_id, target_iframe, target_auxiliary_id) = if kind == "popup" {
            let Some(entry) = state.auxiliary_contexts.get(&(target_id as u64)) else {
                return Ok(JsValue::undefined());
            };
            (entry.document.identity(), None, Some(target_id as u64))
        } else if kind == "iframe" {
            let Some(iframe) = state.get_node(target_id) else {
                return Ok(JsValue::undefined());
            };
            if !state.node_is_in_active_document(&iframe) {
                return Ok(JsValue::undefined());
            }
            let document = state
                .iframe_content_document(&iframe)
                .map_err(|error| JsNativeError::error().with_message(error.to_string()))?;
            let Some(context_id) = state.iframe_context_ids.get(&target_id).copied() else {
                return Ok(JsValue::undefined());
            };
            if expected_context.is_some_and(|expected| expected != context_id) {
                return Ok(JsValue::undefined());
            }
            (document.identity(), Some((target_id, context_id)), None)
        } else if target_id == state.document.identity() {
            (target_id, None, None)
        } else {
            let Some((iframe_id, _)) = state
                .iframe_documents
                .iter()
                .find(|(_, entry)| entry.document.identity() == target_id)
            else {
                return Ok(JsValue::undefined());
            };
            let Some(context_id) = state.iframe_context_ids.get(iframe_id).copied() else {
                return Ok(JsValue::undefined());
            };
            (target_id, Some((*iframe_id, context_id)), None)
        };
        state
            .event_loop
            .enqueue_window_posted_message(Task::WindowPostedMessage {
                target_document_id,
                target_iframe,
                target_auxiliary_id,
                source_document_id: sender_document_id,
                source_iframe_id,
                source_auxiliary_id,
                sender_security_origin,
                origin,
                target_origin,
                wire,
                ports,
            });
        Ok(JsValue::undefined())
    })
}

/// Creates or reuses a named auxiliary browsing context. Its initial
/// `about:blank` Document is independent of the opener's DOM tree.
fn open_auxiliary_window_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let name = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| js_string!("_blank").into())
        .to_string(context)?
        .to_std_string_escaped();
    let opener_document_id = caller_document_id(context);
    with_host_state(|host| {
        let id = {
            let mut state = host.borrow_mut();
            let opener_document_id =
                opener_document_id.unwrap_or_else(|| state.document.identity());
            state
                .open_auxiliary_context(opener_document_id, &name)
                .map_err(|error| JsNativeError::error().with_message(error.to_string()))?
        };
        ensure_auxiliary_realm(context, host, id)?;
        if !url.is_empty() && !matches_about_blank_url(&url) {
            let mut state = host.borrow_mut();
            let source = opener_document_id
                .and_then(|document_id| state.visit_source_for_document(document_id));
            let resolved_url = opener_document_id
                .and_then(|document_id| state.base_url_for_document(document_id))
                .map(|base| resolve_url_reference(&url, Some(&base)))
                .unwrap_or(url);
            state
                .event_loop
                .enqueue_auxiliary_navigation(id, resolved_url, source);
        }
        Ok(JsValue::from(id as f64))
    })
}

/// Navigates a named iframe without exposing a cross-origin owner Document or
/// Node identity to the initiating page.
fn navigate_named_link_target_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let link_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, link_id)?;
    let destination = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let target = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|host| {
        let (frame_id, document_id, callback, document_url, event_state, visit_source) = {
            let mut state = host.borrow_mut();
            let Some(link) = state.get_node(link_id) else {
                return Ok(JsValue::from(false));
            };
            let frame = match state.resolve_named_link_target(&link, &target) {
                form_submission::NamedLinkTarget::NotFound => return Ok(JsValue::null()),
                form_submission::NamedLinkTarget::Blocked => return Ok(JsValue::from(false)),
                form_submission::NamedLinkTarget::Frame(frame) => frame,
            };
            let visit_source = owner_document_for_node(&link)
                .and_then(|source| state.visit_source_for_document(source.identity()));
            let document = state
                .iframe_content_document(&frame)
                .map_err(|error| JsNativeError::typ().with_message(error.to_string()))?;
            let owner = owner_document_for_node(&frame)
                .ok_or_else(|| JsNativeError::typ().with_message("iframe has no owner Document"))?;
            let callback = state
                .iframe_navigation
                .owner(owner.identity())
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("iframe owner is no longer active")
                })?;
            let document_url = state
                .document_urls
                .get(&document.identity())
                .cloned()
                .ok_or_else(|| JsNativeError::typ().with_message("iframe URL is unavailable"))?;
            let event_state = state.shared_event_state_for_node(document.identity());
            (
                frame.identity(),
                document.identity(),
                callback,
                document_url,
                event_state,
                visit_source,
            )
        };
        let previous_active = {
            let mut state = host.borrow_mut();
            state.pending_iframe_visits.remove(&frame_id);
            if let Some(source) = &visit_source {
                state.pending_iframe_visits.insert(frame_id, source.clone());
            }
            state.active_child_navigation_frame.replace(frame_id)
        };
        let forwarded = [
            JsValue::from(frame_id as f64),
            JsValue::from(document_id as f64),
            js_string!("assign").into(),
            js_string!(destination.as_str()).into(),
            JsValue::undefined(),
            js_string!(document_url.as_str()).into(),
            event_state.map_or_else(JsValue::null, JsValue::from),
        ];
        let result = callback
            .as_callable()
            .expect("registered iframe navigation handler")
            .call(&JsValue::undefined(), &forwarded, context);
        {
            let mut state = host.borrow_mut();
            state.active_child_navigation_frame = previous_active;
            if result.is_ok() && state.pending_resource_loads.contains(&frame_id) {
                if let Some(source) = visit_source {
                    state.pending_iframe_visits.insert(frame_id, source);
                } else {
                    state.pending_iframe_visits.remove(&frame_id);
                }
            } else {
                state.pending_iframe_visits.remove(&frame_id);
            }
        }
        result.map(|_| JsValue::from(true))
    })
}

fn navigate_auxiliary_window_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)? as u64;
    let url = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let caller = caller_document_id(context);
    with_host_state(|host| {
        let mut state = host.borrow_mut();
        if state.auxiliary_contexts.contains_key(&id) {
            let source =
                caller.and_then(|document_id| state.visit_source_for_document(document_id));
            let resolved_url = caller
                .and_then(|document_id| state.base_url_for_document(document_id))
                .map(|base| resolve_url_reference(&url, Some(&base)))
                .unwrap_or(url);
            state
                .event_loop
                .enqueue_auxiliary_navigation(id, resolved_url, source);
        }
        Ok(JsValue::undefined())
    })
}

fn auxiliary_window_state_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)? as u64;
    let caller_document_id = context
        .realm()
        .host_defined()
        .get::<ModuleDocumentId>()
        .map(|owner| owner.0);
    with_host_state(|host| {
        let state = host.borrow();
        let Some(entry) = state.auxiliary_contexts.get(&id) else {
            return Ok(JsValue::from(js_string!("closed")));
        };
        let caller = caller_document_id.unwrap_or_else(|| state.document.identity());
        let same_origin = match (
            state.document_security_origins.get(&caller),
            state
                .document_security_origins
                .get(&entry.document.identity()),
        ) {
            (Some(caller), Some(target)) => caller == target,
            _ => false,
        };
        Ok(JsValue::from(js_string!(if same_origin {
            "same"
        } else {
            "cross"
        })))
    })
}

fn auxiliary_window_global_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)? as u64;
    let state = auxiliary_window_state_native(&JsValue::undefined(), args, context)?;
    if state
        .as_string()
        .is_none_or(|state| state.to_std_string_escaped() != "same")
    {
        return Ok(JsValue::null());
    }
    with_host_state(|host| {
        let realm = ensure_auxiliary_realm(context, host, id)?;
        let previous = context.enter_realm(realm);
        let global: JsValue = context.global_object().into();
        context.enter_realm(previous);
        Ok(global)
    })
}

fn close_auxiliary_window_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)? as u64;
    with_host_state(|host| {
        let realm = host
            .borrow()
            .auxiliary_contexts
            .get(&id)
            .and_then(|entry| entry.realm.clone());
        if let Some(realm) = realm {
            let previous = context.enter_realm(realm);
            let _ = (|| -> JsResult<()> {
                let global = context.global_object();
                let close =
                    global.get(js_string!("__omoikane_close_auxiliary_document"), context)?;
                if let Some(callable) = close.as_callable() {
                    callable.call(&global.into(), &[], context)?;
                }
                Ok(())
            })();
            context.enter_realm(previous);
        }
        host.borrow_mut().close_auxiliary_context(id);
        Ok(JsValue::undefined())
    })
}

fn create_worker_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = string_argument(args.first(), "", context)?;
    with_host_state(|state| {
        create_worker_for_owner_state(Rc::clone(state), &url)
            .map(|id| JsValue::from(js_string!(id.to_string())))
    })
}

fn worker_id_argument(args: &[JsValue], context: &mut Context) -> JsResult<u64> {
    let value = args.first().cloned().unwrap_or_default();
    if let Some(string) = value.as_string() {
        return string.to_std_string_escaped().parse::<u64>().map_err(|_| {
            JsNativeError::typ()
                .with_message("invalid worker id")
                .into()
        });
    }
    let number = value.to_number(context)?;
    if !number.is_finite()
        || number < 0.0
        || number.fract() != 0.0
        || number > 9_007_199_254_740_991.0
    {
        return Err(JsNativeError::typ()
            .with_message("invalid worker id")
            .into());
    }
    Ok(number as u64)
}

fn create_worker_for_owner_state(
    owner_state: Rc<RefCell<HostState>>,
    requested_url: &str,
) -> JsResult<u64> {
    let active_owner_document_id = active_document_id();
    let (
        owner_url,
        base_url,
        storage,
        session_id,
        user_agent,
        worker_id,
        owner_origin,
        owner_security_origin,
        owner_secure_context,
    ) = {
        let mut state = owner_state.borrow_mut();
        let owner_document_id =
            active_owner_document_id.unwrap_or_else(|| state.document.identity());
        let id = state.next_worker_id;
        state.next_worker_id = state.next_worker_id.saturating_add(1);
        (
            state
                .base_url
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| state.location_href.clone()),
            state.base_url.clone(),
            state.storage_manager.clone(),
            state.storage_session_id,
            state.navigator_user_agent.clone(),
            id,
            state
                .document_origins
                .get(&owner_document_id)
                .cloned()
                .flatten(),
            state
                .document_security_origins
                .get(&owner_document_id)
                .cloned(),
            document_is_secure_context(&state, owner_document_id),
        )
    };
    let worker_url = resolve_worker_url(requested_url, &owner_url, base_url.as_ref())?;
    let source = {
        let mut state = owner_state.borrow_mut();
        fetch_script_resource_with_client(requested_url, base_url.as_ref(), &mut state.http_client)
            .map(|(_, source, _)| source)
    };
    let mut worker_runtime = JsRuntime::with_document_url_and_storage(
        blank_html_document(),
        &owner_url,
        storage,
        session_id,
    )?;
    worker_runtime.set_user_agent(user_agent);
    let worker_state = Rc::clone(&worker_runtime.host_state);
    {
        let mut state = worker_state.borrow_mut();
        let document_id = state.document.identity();
        state.location_href = worker_url.clone();
        state.base_url = worker_url.parse::<crate::http::Url>().ok();
        state.document_urls.insert(document_id, worker_url.clone());
        if let Some(base_url) = state.base_url.clone() {
            state.document_base_urls.insert(document_id, base_url);
        } else {
            state.document_base_urls.remove(&document_id);
        }
        state.document_origins.insert(document_id, owner_origin);
        if let Some(origin) = owner_security_origin {
            state.document_security_origins.insert(document_id, origin);
        }
        state.secure_context_override = Some(owner_secure_context);
        state.worker_owner = Some(Rc::clone(&owner_state));
        state.worker_id = Some(worker_id);
        state.worker_terminated = false;
        state.worker_owner_bound = false;
    }
    worker_runtime.eval(&format!(
        "__omoikane_install_worker_global({worker_url:?}, {worker_id:?})"
    ))?;
    let entry = Rc::new(RefCell::new(WorkerRuntime {
        runtime: worker_runtime,
        owner_state: Rc::clone(&owner_state),
        owner_object: None,
        outgoing: VecDeque::new(),
        startup_error: None,
        terminated: false,
    }));
    owner_state
        .borrow_mut()
        .workers
        .insert(worker_id, Rc::clone(&entry));
    let source_loaded = source.is_some();
    let startup_error = match source {
        Some(source) => {
            // Keep the entry out of the owner map while evaluating worker
            // startup. This lets self.close() remove the entry immediately
            // without recursively borrowing the entry currently executing.
            let entry_for_eval = owner_state
                .borrow_mut()
                .workers
                .remove(&worker_id)
                .expect("new worker entry must be present");
            let result = entry_for_eval.borrow_mut().runtime.eval(&source);
            let terminated = result.is_err()
                || entry_for_eval
                    .borrow()
                    .runtime
                    .host_state
                    .borrow()
                    .worker_terminated;
            if result.is_err() {
                entry_for_eval
                    .borrow()
                    .runtime
                    .host_state
                    .borrow_mut()
                    .worker_terminated = true;
            }
            entry_for_eval.borrow_mut().terminated = terminated;
            // Keep a startup-closed entry until the owner endpoint binds. Any
            // postMessage queued before close() must still be flushed to that
            // endpoint; bind_worker_owner_native removes the terminated entry
            // immediately after that flush.
            owner_state
                .borrow_mut()
                .workers
                .insert(worker_id, Rc::clone(&entry_for_eval));
            result.err().map(|error| error.to_string())
        }
        None => {
            entry.borrow_mut().terminated = true;
            entry
                .borrow()
                .runtime
                .host_state
                .borrow_mut()
                .worker_terminated = true;
            Some(format!("failed to fetch Worker script: {requested_url}"))
        }
    };
    if startup_error.is_some() {
        report_safe_worker_or_module_failure(
            owner_state.borrow().error_reporter.clone(),
            ErrorCategory::Worker,
            if source_loaded {
                "WORKER_STARTUP_FAILED"
            } else {
                "WORKER_FETCH_FAILED"
            },
            if source_loaded { "execute" } else { "fetch" },
        );
    }
    {
        let mut worker = entry.borrow_mut();
        worker.startup_error = startup_error;
        let queued = std::mem::take(
            &mut worker
                .runtime
                .host_state
                .borrow_mut()
                .worker_startup_outgoing,
        );
        worker.outgoing.extend(queued);
    }
    Ok(worker_id)
}

fn bind_worker_owner_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    let owner_object = args.get(1).cloned().unwrap_or_default();
    let owner_realm = Some(context.realm().clone());
    with_host_state(|state| {
        let entry = state.borrow().workers.get(&id).cloned();
        let Some(entry) = entry else {
            return Ok(JsValue::undefined());
        };
        state
            .borrow_mut()
            .worker_owner_objects
            .insert(id, owner_object.clone());
        let mut worker = entry.borrow_mut();
        worker.owner_object = Some(owner_object.clone());
        {
            let mut worker_state = worker.runtime.host_state.borrow_mut();
            worker_state.worker_owner_bound = true;
            worker_state.worker_owner_object = Some(owner_object.clone());
            worker_state.worker_owner_realm = owner_realm.clone();
        }
        let startup_queued = std::mem::take(
            &mut worker
                .runtime
                .host_state
                .borrow_mut()
                .worker_startup_outgoing,
        );
        worker.outgoing.extend(startup_queued);
        let queued = std::mem::take(&mut worker.outgoing);
        for data in queued {
            state.borrow_mut().event_loop.enqueue_worker_owner_message(
                id,
                owner_object.clone(),
                owner_realm.clone(),
                data,
            );
        }
        if let Some(message) = worker.startup_error.take() {
            state.borrow_mut().event_loop.enqueue_worker_error(
                id,
                Some(owner_object.clone()),
                owner_realm.clone(),
                message,
            );
        }
        if worker.terminated {
            let mut state = state.borrow_mut();
            state.workers.remove(&id);
            state.worker_owner_objects.remove(&id);
        }
        Ok(JsValue::undefined())
    })
}

fn worker_post_message_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    let data = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let entry = state.borrow().workers.get(&id).cloned();
        let Some(entry) = entry else {
            return Ok(JsValue::undefined());
        };
        let worker = entry.borrow();
        if worker.terminated || worker.runtime.host_state.borrow().worker_terminated {
            return Ok(JsValue::undefined());
        }
        state
            .borrow_mut()
            .event_loop
            .enqueue_worker_message(id, data);
        Ok(JsValue::undefined())
    })
}

fn terminate_worker_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = worker_id_argument(args, context)?;
    with_host_state(|state| {
        // Removing the entry breaks the owner/worker Rc cycle immediately. Any
        // already queued tasks retain only the id and become harmless no-ops
        // when the owner map lookup below fails.
        let entry = state.borrow_mut().workers.remove(&id);
        state.borrow_mut().worker_owner_objects.remove(&id);
        if let Some(entry) = entry {
            let mut worker = entry.borrow_mut();
            worker.terminated = true;
            worker.outgoing.clear();
            worker.runtime.host_state.borrow_mut().worker_terminated = true;
        }
        Ok(JsValue::undefined())
    })
}

fn worker_owner_post_message_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let data = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let (owner, id, owner_bound, owner_object, owner_realm) = {
            let state_ref = state.borrow();
            (
                state_ref.worker_owner.clone(),
                state_ref.worker_id,
                state_ref.worker_owner_bound,
                state_ref.worker_owner_object.clone(),
                state_ref.worker_owner_realm.clone(),
            )
        };
        let Some(owner) = owner else {
            return Ok(JsValue::undefined());
        };
        let Some(id) = id else {
            return Ok(JsValue::undefined());
        };
        if state.borrow().worker_terminated {
            return Ok(JsValue::undefined());
        }
        if !owner_bound {
            state.borrow_mut().worker_startup_outgoing.push_back(data);
            return Ok(JsValue::undefined());
        }
        if state.borrow().worker_terminated {
            return Ok(JsValue::undefined());
        }
        let Some(owner_object) = owner_object else {
            return Ok(JsValue::undefined());
        };
        owner.borrow_mut().event_loop.enqueue_worker_owner_message(
            id,
            owner_object,
            owner_realm,
            data,
        );
        Ok(JsValue::undefined())
    })
}

fn worker_close_native(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    with_host_state(|state| {
        let (owner, worker_id, owner_bound) = {
            let state_ref = state.borrow();
            (
                state_ref.worker_owner.clone(),
                state_ref.worker_id,
                state_ref.worker_owner_bound,
            )
        };
        state.borrow_mut().worker_terminated = true;
        if let (Some(owner), Some(worker_id)) = (owner.clone(), worker_id)
            && owner_bound
        {
            let entry = {
                let mut owner_state = owner.borrow_mut();
                owner_state.worker_owner_objects.remove(&worker_id);
                owner_state.workers.remove(&worker_id)
            };
            if let Some(entry) = entry {
                let mut worker = entry.borrow_mut();
                worker.terminated = true;
                worker.outgoing.clear();
                worker.runtime.host_state.borrow_mut().worker_terminated = true;
            }
        } else if let (Some(owner), Some(worker_id)) = (owner, worker_id)
            && let Some(entry) = owner.borrow().workers.get(&worker_id).cloned()
        {
            entry.borrow_mut().terminated = true;
        }
        Ok(JsValue::undefined())
    })
}

fn canvas_commit_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    ensure_same_origin_node(context, id)?;
    let width = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as u32;
    let height = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as u32;
    let pixels = body_bytes_argument(args.get(3), context)?.unwrap_or_default();
    with_host_state(|state| {
        let Some(node) = state.borrow().get_node(id) else {
            return Ok(JsValue::from(false));
        };
        Ok(JsValue::from(crate::canvas::commit(
            &node, width, height, pixels,
        )))
    })
}

fn canvas_data_url_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    ensure_same_origin_node(context, id)?;
    Ok(js_string!(crate::canvas::png_data_url(id).unwrap_or_else(|| "data:,".into())).into())
}

fn canvas_png_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let width_number = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)?;
    let height_number = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)?;
    if !width_number.is_finite()
        || !height_number.is_finite()
        || width_number.fract() != 0.0
        || height_number.fract() != 0.0
        || !(0.0..=32_768.0).contains(&width_number)
        || !(0.0..=32_768.0).contains(&height_number)
    {
        return Ok(JsValue::null());
    }
    let width = width_number as u32;
    let height = height_number as u32;
    let pixels = body_bytes_argument(args.get(2), context)?.unwrap_or_default();
    let Some(expected_len) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return Ok(JsValue::null());
    };
    if pixels.len() != expected_len {
        return Ok(JsValue::null());
    }
    Ok(crate::canvas::png_data_url_from_rgba(width, height, pixels)
        .map(|url| js_string!(url).into())
        .unwrap_or_else(JsValue::null))
}

fn canvas_image_source_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    ensure_same_origin_node(context, id)?;
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

/// Cross-origin WebSockets may reach only public addresses, like other
/// cross-origin subresources. The `ws:` URL is compared with the calling
/// document as its `http:` equivalent, so a page may still reach its own host.
fn websocket_address_policy(
    url: &str,
    document_url: Option<&crate::http::Url>,
) -> crate::realtime::WebSocketAddressPolicy {
    match crate::realtime::websocket_http_url(url) {
        Ok(http_url) if !requires_public_fetch(&http_url, document_url) => {
            crate::realtime::WebSocketAddressPolicy::Any
        }
        _ => crate::realtime::WebSocketAddressPolicy::PublicOnly,
    }
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
    let payload = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let document = state.csp_document_for_context(context)?;
        if !state
            .csp_policy_for_document(&document)
            .allows_reference(ResourceType::Connect, &url)
        {
            state.record_csp_violation(&document, ResourceType::Connect, &url);
            return Err(JsNativeError::error()
                .with_message("WebSocket connection blocked by CSP connect-src")
                .into());
        }
        let origin = state
            .base_url
            .as_ref()
            .map(|url| format!("{}://{}", url.scheme(), url.authority()));
        let address_policy = websocket_address_policy(
            &url,
            state.base_url_for_document(document.identity()).as_ref(),
        );
        let client = crate::realtime::WebSocketClient::connect_with_policy(
            &url,
            &protocols,
            origin.as_deref(),
            address_policy,
        )
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
        let protocol = client.protocol().to_string();
        let mut reader = client.try_clone().map_err(|error| {
            JsError::from(JsNativeError::error().with_message(error.to_string()))
        })?;
        let (sender, incoming) = channel();
        thread::spawn(move || {
            loop {
                match reader.read_message() {
                    Ok(message) => {
                        let closed =
                            matches!(message, crate::realtime::WebSocketMessage::Close { .. });
                        if sender.send(WebSocketReadResult::Message(message)).is_err() || closed {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(WebSocketReadResult::Error(error.to_string()));
                        break;
                    }
                }
            }
        });
        let id = state.next_websocket_id;
        state.next_websocket_id = state.next_websocket_id.saturating_add(1);
        state
            .websocket_clients
            .insert(id, WebSocketConnection { client, incoming });
        Ok(serde_json::json!({"id": id, "protocol": protocol}).to_string())
    })?;
    Ok(js_string!(payload).into())
}

fn websocket_send_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as u64;
    let encoded = string_argument(args.get(1), "", context)?;
    let binary = args.get(2).is_some_and(JsValue::to_boolean);
    let payload = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| {
            JsError::from(JsNativeError::typ().with_message("invalid WebSocket payload"))
        })?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let connection = state.websocket_clients.get_mut(&id).ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("WebSocket is not connected"))
        })?;
        connection.client.send(payload, binary).map_err(|error| {
            JsError::from(JsNativeError::error().with_message(error.to_string()))
        })?;
        Ok(JsValue::undefined())
    })
}

fn websocket_poll_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as u64;
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
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as u64;
    let code = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| JsValue::from(1000))
        .to_number(context)? as u16;
    let reason = string_argument(args.get(2), "", context)?;
    with_host_state(|state| {
        let mut connection = state
            .borrow_mut()
            .websocket_clients
            .remove(&id)
            .ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("WebSocket is not connected"))
            })?;
        connection.client.close(code, &reason).map_err(|error| {
            JsError::from(JsNativeError::error().with_message(error.to_string()))
        })?;
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
    let body = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let parsed = url
            .parse::<crate::http::Url>()
            .map_err(|error| JsError::from(JsNativeError::typ().with_message(error.to_string())))?;
        let document = state.csp_document_for_context(context)?;
        if !state
            .csp_policy_for_document(&document)
            .allows_reference(ResourceType::Connect, &url)
        {
            state.record_csp_violation(&document, ResourceType::Connect, &url);
            return Err(JsNativeError::error()
                .with_message("EventSource connection blocked by CSP connect-src")
                .into());
        }
        let origin = state
            .base_url
            .as_ref()
            .map(CorsOrigin::from_url)
            .unwrap_or_else(CorsOrigin::opaque);
        let mut request = HttpRequest::new(Method::Get, parsed);
        if let Ok(site) = state.location_href.parse::<crate::http::Url>() {
            request.set_cookie_context(site, false);
        }
        request.set_header("Accept", "text/event-stream");
        if !last_event_id.is_empty() {
            request.set_header("Last-Event-ID", last_event_id);
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
            RequestMode::Cors,
            if with_credentials {
                CredentialsMode::Include
            } else {
                CredentialsMode::SameOrigin
            },
            RedirectMode::Follow,
            cors_preflight_cache,
        )
        .map_err(|error| JsError::from(JsNativeError::error().with_message(error.to_string())))?;
        if let Some(effective_url) = fetched.response.effective_url()
            && !state
                .csp_policy_for_document(&document)
                .allows_url_after_redirects(
                    ResourceType::Connect,
                    effective_url,
                    fetched.response.redirect_count(),
                )
        {
            let blocked_uri = effective_url.to_string();
            state.record_csp_violation(&document, ResourceType::Connect, &blocked_uri);
            return Err(JsNativeError::error()
                .with_message("EventSource redirect blocked by CSP connect-src")
                .into());
        }
        if fetched.response.status_code() != 200 {
            return Err(JsNativeError::error()
                .with_message("EventSource response must be HTTP 200")
                .into());
        }
        let content_type = fetched.response.header("content-type").unwrap_or_default();
        if !content_type
            .to_ascii_lowercase()
            .starts_with("text/event-stream")
        {
            return Err(JsNativeError::error()
                .with_message("EventSource response must be text/event-stream")
                .into());
        }
        Ok(String::from_utf8_lossy(fetched.response.body()).into_owned())
    })?;
    Ok(js_string!(body).into())
}

fn fetch_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    #[cfg(test)]
    if let Some(response) = TEST_FETCH_RESPONSE_OVERRIDE.with(|override_response| {
        let mut override_response = override_response.borrow_mut();
        if override_response
            .as_ref()
            .is_some_and(|(target, _)| target == &url)
        {
            override_response.take().map(|(_, response)| response)
        } else {
            None
        }
    }) {
        return Ok(js_string!(response.as_str()).into());
    }
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
        JsError::from(
            JsNativeError::typ().with_message(format!("invalid request headers: {error}")),
        )
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
    let timeout = if let Some(value) = args.get(7) {
        let milliseconds = value.to_number(context)?;
        if milliseconds.is_finite() && milliseconds > 0.0 {
            Some(Duration::from_millis(
                milliseconds.ceil().min(u64::MAX as f64) as u64,
            ))
        } else {
            None
        }
    } else {
        None
    };

    let payload = with_host_state(|state| {
        let mut state = state.borrow_mut();
        let parsed_url = url
            .parse::<crate::http::Url>()
            .map_err(|error| JsError::from(JsNativeError::typ().with_message(error.to_string())))?;
        let normalized_url = parsed_url.to_string();
        let document = state.csp_document_for_context(context)?;
        if !state
            .csp_policy_for_document(&document)
            .allows_reference(ResourceType::Connect, &normalized_url)
        {
            state.record_csp_violation(&document, ResourceType::Connect, &normalized_url);
            return Err(JsNativeError::error()
                .with_message("fetch blocked by CSP connect-src")
                .into());
        }
        let origin = state
            .base_url
            .as_ref()
            .map(CorsOrigin::from_url)
            .unwrap_or_else(CorsOrigin::opaque);
        let mut request = HttpRequest::new(method, parsed_url);
        if let Ok(site) = state.location_href.parse::<crate::http::Url>() {
            request.set_cookie_context(site, false);
        }
        for (name, value) in headers {
            if !crate::http::is_valid_header(&name, &value) {
                return Err(JsNativeError::typ()
                    .with_message("invalid request header")
                    .into());
            }
            if crate::http::is_forbidden_request_header(&name, &value) {
                continue;
            }
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
        let fetched = match crate::http::cors::fetch_with_timeout(
            http_client,
            request,
            &origin,
            mode,
            credentials,
            redirect_mode,
            cors_preflight_cache,
            timeout,
        ) {
            Ok(fetched) => fetched,
            Err(crate::http::cors::CorsError::Timeout) if timeout.is_some() => {
                return Ok(r#"{"__omoikane_timeout":true}"#.to_string());
            }
            Err(error) => {
                return Err(
                    JsError::from(JsNativeError::error().with_message(error.to_string())).into(),
                );
            }
        };
        if let Some(effective_url) = fetched.response.effective_url()
            && !state
                .csp_policy_for_document(&document)
                .allows_url_after_redirects(
                    ResourceType::Connect,
                    effective_url,
                    fetched.response.redirect_count(),
                )
        {
            let blocked_uri = effective_url.to_string();
            state.record_csp_violation(&document, ResourceType::Connect, &blocked_uri);
            return Err(JsNativeError::error()
                .with_message("fetch redirect blocked by CSP connect-src")
                .into());
        }
        let response = fetched.response;
        let opaque = matches!(
            fetched.response_type,
            ResponseType::Opaque | ResponseType::OpaqueRedirect
        );
        // Fetch creates a null body for HEAD responses and for status codes
        // whose semantics forbid a body. A successful response with an empty
        // payload (for example `200 Content-Length: 0`) still has an empty
        // ReadableStream, so presence cannot be inferred from byte length.
        let body_present = !opaque
            && !matches!(method, Method::Head)
            && !matches!(response.status_code(), 100..=199 | 204 | 205 | 304);
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
        let decoded_body = body_present.then(|| String::from_utf8_lossy(response.body()));
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
            "bodyPresent": body_present,
        })
        .to_string();
        Ok(payload)
    })?;
    Ok(js_string!(payload.as_str()).into())
}

/// `__omoikane_csp_violations(documentId)` exposes the enforced violations
/// retained by the current Document generation.  It is intentionally a
/// snapshot rather than a consuming queue so repeated DOM inspection remains
/// deterministic and navigation replacement naturally drops the old state.
fn csp_violations_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let document_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, document_id)?;
    with_host_state(|state| {
        let violations = state
            .borrow()
            .csp_violations
            .iter()
            .filter(|violation| violation.document_id == document_id)
            .cloned()
            .collect::<Vec<_>>();
        Ok(
            js_string!(serde_json::to_string(&violations).unwrap_or_else(|_| "[]".to_string()))
                .into(),
        )
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
    ensure_same_origin_node(context, id)?;
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
    crate::dom::collect_descendant_text(node, crate::dom::TextTraversal::Containers)
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
    ensure_same_origin_node(context, id)?;
    let text = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let creator = caller_document_id(context);
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
            let removed_children = node.child_nodes();
            for child in &removed_children {
                let _ = node.remove_child(&child);
            }
            {
                let mut state = state.borrow_mut();
                for child in &removed_children {
                    state.cancel_smooth_scrolls_in_subtree(child);
                    state.destroy_iframe_contexts_in_subtree(child);
                }
            }
            // Add single text node
            if !text.is_empty() {
                let text_node = NodeHandle::text(&text);
                node.append_child(text_node.clone());
                state
                    .borrow_mut()
                    .register_tree_for_document(&text_node, creator);
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
        let removed_children = target.child_nodes();
        for child in &removed_children {
            let _ = target.remove_child(&child);
        }
        {
            let mut state = state.borrow_mut();
            for child in &removed_children {
                state.destroy_iframe_contexts_in_subtree(child);
            }
        }
        if !html.is_empty() {
            let parsed = crate::html::TreeBuilder::parse_fragment(&html, &node).fragment();
            for child in parsed.child_nodes() {
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

fn parse_contextual_fragment_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let context_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, context_id)?;
    let source = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let xml = args.get(2).cloned().unwrap_or_default().to_boolean();
    let context_node = with_host_state(|state| {
        state.borrow().get_node(context_id).ok_or_else(|| {
            JsError::from(JsNativeError::error().with_message("context node not found"))
        })
    })?;
    let fragment = if xml {
        crate::xml::parse_fragment(source.as_bytes(), &context_node).map_err(|error| {
            JsError::from(JsNativeError::syntax().with_message(error.to_string()))
        })?
    } else {
        crate::html::TreeBuilder::parse_fragment(&source, &context_node).fragment()
    };
    let id = fragment.identity();
    let creator = caller_document_id(context);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        state.register_tree_for_document(&fragment, creator);
        if !xml {
            mark_inserted_scripts_in_tree(&mut state, &fragment);
        }
        Ok(())
    })?;
    Ok(JsValue::from(id as f64))
}

/// Marks an HTML script as created by an API whose insertion may prepare it.
/// Parser and `innerHTML` paths never call this, which keeps their scripts
/// inert when later moved.
fn mark_inserted_script_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        if state
            .get_node(id)
            .is_some_and(|node| is_html_script_node(&node))
        {
            state.runnable_inserted_scripts.insert(id);
        }
        Ok(JsValue::undefined())
    })
}

fn is_html_script_node(node: &NodeHandle) -> bool {
    node.tag_name()
        .is_some_and(|tag| tag.eq_ignore_ascii_case("script"))
        && node
            .namespace_uri()
            .as_deref()
            .is_none_or(|namespace| namespace == "http://www.w3.org/1999/xhtml")
}

fn mark_inserted_scripts_in_tree(state: &mut HostState, root: &NodeHandle) {
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if is_html_script_node(&node) {
            state.runnable_inserted_scripts.insert(node.identity());
        }
        pending.extend(node.child_nodes().into_iter().rev());
    }
}

/// Returns only pending script node ids from an inserted subtree. The common
/// case has no pending scripts and exits without walking or creating wrappers.
fn collect_inserted_scripts_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let root_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, root_id)?;
    with_host_state(|state| {
        let state = state.borrow();
        let mut ids = Vec::new();
        if !state.runnable_inserted_scripts.is_empty()
            && let Some(root) = state.get_node(root_id)
        {
            let mut pending = vec![root];
            while let Some(node) = pending.pop() {
                if state.runnable_inserted_scripts.contains(&node.identity()) {
                    ids.push(JsValue::from(node.identity() as f64));
                }
                pending.extend(node.child_nodes().into_iter().rev());
            }
        }
        Ok(JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(ids, context),
        ))
    })
}

/// Prepares one dynamically inserted inline classic script.
///
/// `undefined` means the node has not reached an active document and may be
/// retried. `null` means preparation completed without classic inline source
/// (external/non-classic/blocked/already-started). A string is source that the
/// bootstrap evaluates synchronously in the owning Document's Window.
fn prepare_inserted_inline_script_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        if !state.runnable_inserted_scripts.contains(&id)
            || state.started_inserted_scripts.contains(&id)
        {
            return Ok(JsValue::null());
        }
        let Some(node) = state.get_node(id) else {
            return Ok(JsValue::null());
        };
        if !state.node_is_in_active_document(&node) {
            return Ok(JsValue::undefined());
        }
        state.runnable_inserted_scripts.remove(&id);
        state.started_inserted_scripts.insert(id);
        if !is_inline_classic_script(&node) {
            return Ok(JsValue::null());
        }
        if !state.sandbox_allows_scripts_for_node(&node) {
            return Ok(JsValue::null());
        }
        if !state
            .csp_policy_for_node(&node)
            .allows_inline(ResourceType::Script)
        {
            state.record_csp_violation_for_node(&node, ResourceType::Script, "inline");
            return Ok(JsValue::null());
        }
        Ok(JsValue::from(js_string!(collect_text_content(&node))))
    })
}

/// Records a page exception from synchronous dynamic inline script execution.
/// Script failures do not make the surrounding DOM insertion throw.
fn record_inserted_script_error_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let message = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        if state.task_errors.len() < MAX_TASK_ERRORS {
            state
                .task_errors
                .push(format!("[dynamic inline script {id}] {message}"));
        } else {
            state.suppressed_task_errors = state.suppressed_task_errors.saturating_add(1);
        }
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
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let (node, children) = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("node not found"))
            })?;
            let children = node.child_nodes();
            (node, children)
        };
        state.borrow_mut().register_query_results(&node, &children);
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, parent_id)?;
    let child_id = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    ensure_same_origin_node(context, child_id)?;
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
        {
            let mut state = state.borrow_mut();
            state.cancel_smooth_scrolls_in_subtree(&child);
            state.destroy_iframe_contexts_in_subtree(&child);
            if let Some(document) = &parent_document {
                state.mark_document_style_dirty(document);
            }
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
    ensure_same_origin_node(context, parent_id)?;
    let new_id = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    ensure_same_origin_node(context, new_id)?;
    let ref_value = args.get(2).cloned().unwrap_or_default();
    let ref_id = if ref_value.is_null_or_undefined() {
        None
    } else {
        let id = ref_value.to_number(context)? as usize;
        ensure_same_origin_node(context, id)?;
        Some(id)
    };
    with_host_state(|state| {
        let ref_node = ref_id.and_then(|id| state.borrow().get_node(id));
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
            if source_document.is_some() {
                state.destroy_iframe_contexts_in_subtree(&new_node);
            }
            // See `append_child_native`: insertion into a live document creates
            // a fresh iframe/object context, including an in-document reorder.
            if target_document.is_some() {
                state.schedule_connected_resource_loads(
                    &new_node,
                    source_document != target_document,
                );
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
    ensure_same_origin_node(context, parent_id)?;
    let selector = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let selectors = parse_dom_selector_list(&selector)?;
    if form_validation::selector_uses_validation(&selectors) {
        form_validation::flush(parent_id, context)?;
    }
    with_host_state(|state| {
        let (parent, results) = {
            let s = state.borrow();
            let parent = s.get_node(parent_id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("node not found"))
            })?;
            let results = query_all_matching_descendants(&parent, &selectors);
            (parent, results)
        };
        state.borrow_mut().register_query_results(&parent, &results);
        let ids: Vec<JsValue> = results
            .iter()
            .map(|n| JsValue::from(n.identity() as f64))
            .collect();
        Ok(boa_engine::JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(ids, context),
        ))
    })
}

fn set_user_action_target_native(
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
    if !matches!(kind.as_str(), "hover" | "active" | "focus") {
        return Err(JsNativeError::typ()
            .with_message("invalid user-action state")
            .into());
    }
    let target_id = args
        .get(1)
        .filter(|value| !value.is_null_or_undefined())
        .map(|value| parse_node_id(Some(value), context))
        .transpose()?;
    if let Some(target_id) = target_id {
        ensure_same_origin_node(context, target_id)?;
    }
    let visible = args.get(2).is_some_and(JsValue::to_boolean);
    with_host_state(|state| {
        state
            .borrow_mut()
            .update_user_action_target(&kind, target_id, visible);
        Ok(JsValue::undefined())
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
    ensure_same_origin_node(context, node_id)?;
    let selector = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let selectors = parse_dom_selector_list(&selector)?;
    if form_validation::selector_uses_validation(&selectors) {
        form_validation::flush(node_id, context)?;
    }
    let scope_id = args
        .get(2)
        .map(|value| parse_node_id(Some(value), context))
        .transpose()?;
    if let Some(scope_id) = scope_id {
        ensure_same_origin_node(context, scope_id)?;
    }
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(node_id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let scope = scope_id
            .and_then(|id| state.get_node(id))
            .unwrap_or_else(|| node.clone());
        let mut cache = SelectorMatchCache::default();
        Ok(JsValue::from(selectors.iter().any(|selector| {
            matches_selector_boundary_cached(&node, selector, None, &mut cache, Some(&scope))
        })))
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
    let scope = query_scope_root(node);
    find_matching_descendant(
        node,
        selectors,
        scope.as_ref(),
        &mut SelectorMatchCache::default(),
    )
}

fn query_scope_root(node: &NodeHandle) -> Option<NodeHandle> {
    if node.node_type() == NodeType::Document {
        node.child_nodes()
            .into_iter()
            .find(|child| child.node_type() == NodeType::Element)
    } else if node.node_type() == NodeType::Element {
        Some(node.clone())
    } else {
        None
    }
}

fn matches_dom_query_selector(
    node: &NodeHandle,
    selector: &Selector,
    cache: &mut SelectorMatchCache,
    scope: Option<&NodeHandle>,
) -> bool {
    if let Some(scope) = scope {
        matches_selector_boundary_cached(node, selector, None, cache, Some(scope))
    } else {
        matches_selector_cached(node, selector, cache)
    }
}

fn find_matching_descendant(
    node: &NodeHandle,
    selectors: &[Selector],
    scope: Option<&NodeHandle>,
    cache: &mut SelectorMatchCache,
) -> Option<NodeHandle> {
    for child in node.child_nodes() {
        if child.node_type() == NodeType::Element
            && selectors
                .iter()
                .any(|selector| matches_dom_query_selector(&child, selector, cache, scope))
        {
            return Some(child);
        }
        if let Some(found) = find_matching_descendant(&child, selectors, scope, cache) {
            return Some(found);
        }
    }
    None
}

fn query_all_matching_descendants(node: &NodeHandle, selectors: &[Selector]) -> Vec<NodeHandle> {
    let mut results = Vec::new();
    let scope = query_scope_root(node);
    collect_matching_descendants(
        node,
        selectors,
        scope.as_ref(),
        &mut results,
        &mut SelectorMatchCache::default(),
    );
    results
}

fn collect_matching_descendants(
    node: &NodeHandle,
    selectors: &[Selector],
    scope: Option<&NodeHandle>,
    results: &mut Vec<NodeHandle>,
    cache: &mut SelectorMatchCache,
) {
    for child in node.child_nodes() {
        if child.node_type() == NodeType::Element
            && selectors
                .iter()
                .any(|selector| matches_dom_query_selector(&child, selector, cache, scope))
        {
            results.push(child.clone());
        }
        collect_matching_descendants(&child, selectors, scope, results, cache);
    }
}

fn node_type_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_number(context)? as usize;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let node_type = match node.node_type() {
            crate::dom::NodeType::Element => 1,
            crate::dom::NodeType::Text if node.is_cdata_section() => 4,
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

fn node_is_html_element_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        Ok(JsValue::from(node.is_html_element()))
    })
}

fn clone_node_impl(node: &NodeHandle, deep: bool) -> NodeHandle {
    let clone = match node.node_type() {
        crate::dom::NodeType::Element => {
            let tag = node.tag_name().unwrap_or_default();
            let el = if node.is_html_element() {
                match node.namespace_uri() {
                    Some(namespace) => NodeHandle::html_element_ns(&tag, namespace),
                    None => NodeHandle::element(&tag),
                }
            } else {
                NodeHandle::xml_element(&tag, node.namespace_uri())
            };
            if let Some(attributes) = node.attribute_records() {
                for (qualified_name, namespace, local_name, value) in attributes {
                    if node.is_html_element() && namespace.is_none() {
                        el.set_attribute(qualified_name, value);
                    } else {
                        el.set_xml_attribute_ns(qualified_name, namespace, local_name, value);
                    }
                }
            }
            el
        }
        crate::dom::NodeType::Text if node.is_cdata_section() => {
            NodeHandle::cdata_section(node.data().unwrap_or_default())
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
    ensure_same_origin_node(context, id)?;
    let deep = args.get(1).cloned().unwrap_or_default().to_boolean();
    let creator = caller_document_id(context);
    with_host_state(|state| {
        let (node, clone) = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("node not found"))
            })?;
            let clone = clone_node_impl(&node, deep);
            (node, clone)
        };
        let clone_id = clone.identity() as f64;
        let mut state = state.borrow_mut();
        if node.node_type() == NodeType::Document
            && let Some(origin) = state
                .document_security_origins
                .get(&node.identity())
                .cloned()
        {
            state
                .document_security_origins
                .insert(clone.identity(), origin);
        }
        state.register_tree_for_document(&clone, creator);
        copy_inserted_script_state(&mut state, &node, &clone, deep);
        Ok(JsValue::from(clone_id))
    })
}

fn copy_inserted_script_state(
    state: &mut HostState,
    source: &NodeHandle,
    target: &NodeHandle,
    deep: bool,
) {
    if state.runnable_inserted_scripts.contains(&source.identity()) {
        state.runnable_inserted_scripts.insert(target.identity());
        if state.started_inserted_scripts.contains(&source.identity()) {
            state.started_inserted_scripts.insert(target.identity());
        }
    }
    if !deep {
        return;
    }
    if let (Some(source_content), Some(target_content)) =
        (source.template_content(), target.template_content())
    {
        copy_inserted_script_state(state, &source_content, &target_content, true);
    }
    for (source_child, target_child) in source.child_nodes().iter().zip(target.child_nodes()) {
        copy_inserted_script_state(state, source_child, &target_child, true);
    }
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
    ensure_same_origin_node(context, id)?;
    let name = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let is_style_attribute = name.eq_ignore_ascii_case("style");
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let resource_attr = node.tag_name().and_then(|tag| {
            if tag.eq_ignore_ascii_case("iframe") && name.eq_ignore_ascii_case("srcdoc") {
                Some("srcdoc")
            } else if (is_nested_frame_tag(&tag) || tag.eq_ignore_ascii_case("script"))
                && name.eq_ignore_ascii_case("src")
            {
                Some("src")
            } else if tag.eq_ignore_ascii_case("object") && name.eq_ignore_ascii_case("data") {
                Some("data")
            } else {
                None
            }
        });
        let removed_name =
            name.eq_ignore_ascii_case("name") && node.get_attribute("name").is_some();
        node.remove_attribute(&name);
        if removed_name {
            state.borrow_mut().refresh_iframe_context_name(&node);
        }
        // Any attribute may participate in a selector, so invalidate the
        // element's live document. Detached elements affect no document yet.
        if is_style_attribute {
            state.borrow_mut().refresh_csp_inline_style_nodes(&node);
            state.borrow_mut().invalidate_inline_style_for_node(&node);
        } else if matches!(node.tag_name().as_deref(), Some("style" | "link" | "base")) {
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

fn remove_attribute_ns_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let namespace = match args.get(1) {
        Some(value) if !value.is_null() && !value.is_undefined() => {
            Some(value.to_string(context)?.to_std_string_escaped())
        }
        _ => None,
    };
    let local_name = args
        .get(2)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let qualified_name = args
        .get(3)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let is_style_attribute = namespace.is_none() && qualified_name == "style";
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let resource_attr = namespace.as_ref().is_none().then(|| {
            node.tag_name().and_then(|tag| {
                if tag.eq_ignore_ascii_case("iframe") && qualified_name == "srcdoc" {
                    Some("srcdoc")
                } else if (is_nested_frame_tag(&tag) || tag.eq_ignore_ascii_case("script"))
                    && qualified_name == "src"
                {
                    Some("src")
                } else if tag.eq_ignore_ascii_case("object") && qualified_name == "data" {
                    Some("data")
                } else {
                    None
                }
            })
        });
        let removed_name = qualified_name == "name" && node.get_attribute("name").is_some();
        node.remove_xml_attribute_ns(namespace.as_deref(), &local_name);
        if removed_name {
            state.borrow_mut().refresh_iframe_context_name(&node);
        }
        if is_style_attribute {
            state.borrow_mut().refresh_csp_inline_style_nodes(&node);
            state.borrow_mut().invalidate_inline_style_for_node(&node);
        } else if matches!(node.tag_name().as_deref(), Some("style" | "link" | "base")) {
            state.borrow_mut().mark_style_dirty_for_node(&node);
        } else {
            state.borrow_mut().invalidate_style_cache_for_node(&node);
        }
        if let Some(Some(resource_attr)) = resource_attr {
            state
                .borrow_mut()
                .schedule_resource_load_on_attribute_change(&node, resource_attr);
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
    let creator = caller_document_id(context);
    with_host_state(|state| {
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
        Ok(JsValue::from(id))
    })
}

fn create_cdata_section_native(
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
    let node = NodeHandle::cdata_section(text);
    let id = node.identity() as f64;
    let creator = caller_document_id(context);
    with_host_state(|state| {
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
        Ok(JsValue::from(id))
    })
}

fn create_document_fragment_native(
    _: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node = NodeHandle::document_fragment();
    let id = node.identity() as f64;
    let creator = caller_document_id(context);
    with_host_state(|state| {
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
        Ok(JsValue::from(id))
    })
}

fn template_content_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
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

fn attach_shadow_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    let mode = if args.get(1).is_some_and(JsValue::to_boolean) {
        ShadowRootMode::Closed
    } else {
        ShadowRootMode::Open
    };
    let creator = caller_document_id(context);
    with_host_state(|state| {
        let host = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let Some(root) = host.attach_shadow(mode) else {
            return Ok(JsValue::null());
        };
        let root_id = root.identity();
        state
            .borrow_mut()
            .register_tree_for_document(&root, creator);
        Ok(JsValue::from(root_id as f64))
    })
}

fn shadow_root_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let root = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.shadow_root());
        Ok(node_to_js_value(root))
    })
}

fn shadow_host_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let host = state
            .borrow()
            .get_node(id)
            .and_then(|node| node.shadow_host());
        Ok(node_to_js_value(host))
    })
}

fn shadow_mode_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
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

fn assigned_slot_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
    ensure_same_origin_node(context, id)?;
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
    context: &mut Context,
) -> JsResult<JsValue> {
    let document = NodeHandle::document();
    let id = document.identity();
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let creator = context_document_id(context, &state);
        if let Some(origin) = state.document_security_origins.get(&creator).cloned() {
            state.document_security_origins.insert(id, origin);
        }
        state.register_tree(&document);
        state.document_styles.insert(
            id,
            DocumentStyleEntry {
                viewport_size: None,
                resolver: None,
                resources: Default::default(),
                web_fonts: Default::default(),
                dirty: true,
                needs_full_sample: true,
            },
        );
        Ok(JsValue::from(id as f64))
    })
}

/// Parses an XML-family document using the host XML parser and enrolls the
/// complete detached tree in this runtime's node/style registries.  A null
/// result means the input is not well-formed; DOMParser turns that into the
/// script-visible `parsererror` document instead of exposing parser internals.
fn parse_xml_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let source = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let Ok(document) = crate::xml::parse(source.as_bytes()) else {
        return Ok(JsValue::null());
    };
    let id = document.identity();
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let creator = context_document_id(context, &state);
        if let Some(origin) = state.document_security_origins.get(&creator).cloned() {
            state.document_security_origins.insert(id, origin);
        }
        state.register_tree(&document);
        state.document_styles.insert(
            id,
            DocumentStyleEntry {
                viewport_size: None,
                resolver: None,
                resources: Default::default(),
                web_fonts: Default::default(),
                dirty: true,
                needs_full_sample: true,
            },
        );
        Ok(JsValue::from(id as f64))
    })
}

/// Serializes a live native DOM node without invoking replaceable JavaScript
/// accessors on the node or any of its descendants.
fn serialize_xml_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let serialized = crate::xml::serialize(&node);
        Ok(js_string!(serialized.as_str()).into())
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
    let creator = caller_document_id(context);
    with_host_state(|state| {
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
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
    let creator = caller_document_id(context);
    with_host_state(|state| {
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
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
    let creator = caller_document_id(context);
    with_host_state(|state| {
        state
            .borrow_mut()
            .register_tree_for_document(&node, creator);
        Ok(JsValue::from(id))
    })
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
/// [`is_inline_classic_script`], [`ScriptKind::from_type_attribute`], and
/// `JsRuntime::execute_document_scripts` gate on this
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
/// Reset only this Document's parser state. Opening an independent child
/// document must preserve any outer script's insertion reference.
fn document_reset_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, id)?;
    with_host_state(|state| {
        let (node, is_main_document) = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("document node not found"))
            })?;
            let is_main_document = node == s.document;
            (node, is_main_document)
        };
        if state
            .borrow()
            .write_parsers
            .get(&id)
            .is_some_and(|parser| parser.borrow().is_executing())
        {
            return Ok(JsValue::from(false));
        }
        let removed_children = node.child_nodes();
        let removed_any = !removed_children.is_empty();
        {
            let mut state = state.borrow_mut();
            for child in &removed_children {
                state.destroy_iframe_contexts_in_subtree(child);
            }
        }
        for child in removed_children {
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
        state.borrow_mut().write_parsers.insert(
            id,
            Rc::new(RefCell::new(document_write::WriteState::new(
                node.clone(),
                None,
                true,
            ))),
        );
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
    let caller_document = caller_document_id(context);
    with_host_state(|state| {
        let state = state.borrow();
        let base = caller_document
            .and_then(|document_id| state.base_url_for_document(document_id))
            .or_else(|| {
                caller_document
                    .is_none()
                    .then(|| state.base_url.clone())
                    .flatten()
            });
        let resolved = resolve_url_reference(&reference, base.as_ref());
        Ok(js_string!(resolved.as_str()).into())
    })
}

/// Supplies the JS URL class with the same WHATWG parser used by HTTP requests.
fn parse_url_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let input = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let base = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let parsed = url::Url::parse(&base)
        .and_then(|base| base.join(&input))
        .map_err(|error| JsNativeError::typ().with_message(format!("invalid URL: {error}")))?;
    let hostname = parsed.host_str().unwrap_or("");
    let host = match parsed.port() {
        Some(port) => format!("{hostname}:{port}"),
        None => hostname.to_owned(),
    };
    let search = parsed
        .query()
        .filter(|query| !query.is_empty())
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let hash = parsed
        .fragment()
        .filter(|fragment| !fragment.is_empty())
        .map(|fragment| format!("#{fragment}"))
        .unwrap_or_default();
    let data = serde_json::json!({
        "protocol": format!("{}:", parsed.scheme()),
        "host": host,
        "hostname": hostname,
        "username": parsed.username(),
        "password": parsed.password().unwrap_or(""),
        "port": parsed.port().map(|port| port.to_string()).unwrap_or_default(),
        "pathname": parsed.path(),
        "search": search,
        "hash": hash,
        "origin": parsed.origin().ascii_serialization(),
        "href": parsed.as_str(),
    });
    Ok(js_string!(data.to_string()).into())
}

/// Resolve an IDL URL reference while retaining its fragment and raw fallback.
fn resolve_url_reference(reference: &str, base: Option<&crate::http::Url>) -> String {
    match base {
        Some(base) => {
            // Preserve any fragment: resolve only the part before the first
            // `#`, then re-attach `#fragment` to the resolved output.
            let (without_fragment, fragment) = match reference.split_once('#') {
                Some((before, after)) => (before, Some(after)),
                None => (reference, None),
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
                None => reference.to_owned(),
            }
        }
        None => reference.to_owned(),
    }
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
    let caller = caller_document_id(context);
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let source = caller.and_then(|document| state.visit_source_for_document(document));
        state
            .event_loop
            .enqueue_navigation_from_source(request, source);
        Ok(JsValue::undefined())
    })
}

fn submit_form_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
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
    let target = string_arg(4, context)?;
    let form_id = parse_node_id(args.get(5), context)?;
    ensure_same_origin_node(context, form_id)?;
    let request = form_submission::Submission {
        url,
        method,
        body,
        content_type,
    };
    let frame = with_host_state(|state| {
        let mut state = state.borrow_mut();
        if let Some(form) = state.get_node(form_id) {
            return state.queue_form_submission(&form, &target, request);
        }
        Ok(None)
    })?;
    if let Some(frame_id) = frame {
        let callback = with_host_state(|state| {
            let state = state.borrow();
            let owner = state
                .get_node(frame_id)
                .as_ref()
                .and_then(owner_document_for_node)
                .map(|document| document.identity());
            Ok(owner.and_then(|id| state.iframe_navigation.owner(id)))
        })?;
        if let Some(callback) = callback {
            callback
                .as_callable()
                .expect("registered navigation handler")
                .call(
                    &JsValue::undefined(),
                    &[
                        JsValue::from(frame_id as f64),
                        JsValue::null(),
                        js_string!("prepare").into(),
                    ],
                    context,
                )?;
        }
    }
    Ok(frame.map_or_else(JsValue::null, |id| JsValue::from(id as f64)))
}

/// Incrementally parses written input, executing each completed classic
/// script before the following tokens become visible to page code.
fn document_write_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let target_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, target_id)?;
    let text = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| document_write::write(state, target_id, &text, false, context))
}

fn document_close_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let target_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_document(context, target_id)?;
    with_host_state(|state| document_write::write(state, target_id, "", true, context))
}

/// `__omoikane_iframe_content_document(iframeId)` — returns the node id of the
/// same-origin sub-browsing-context document owned by an `<iframe>` element,
/// loading it on first access. Cross-origin, opaque, detached, and unknown
/// contexts return `null`.
fn iframe_content_document_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, node_id)?;
    with_host_state(|state| {
        let iframe = state.borrow().get_node(node_id);
        match iframe {
            Some(iframe) if state.borrow().node_is_in_active_document(&iframe) => {
                let document = state
                    .borrow_mut()
                    .iframe_content_document(&iframe)
                    .map_err(|error| {
                        JsError::from(JsNativeError::error().with_message(error.to_string()))
                    })?;
                let exposed = {
                    let state = state.borrow();
                    state
                        .sandbox_policy_for_document(&document)
                        .exposes_document_to_parent()
                        && state.iframe_document_is_same_origin(&iframe, &document)
                };
                if exposed {
                    Ok(JsValue::from(document.identity() as f64))
                } else {
                    Ok(JsValue::null())
                }
            }
            Some(_) => Ok(JsValue::null()),
            None => Ok(JsValue::null()),
        }
    })
}

/// Returns the live same-origin iframe global for the private WindowProxy
/// forwarding path. A false second argument only looks up an existing Realm,
/// so event-listener access does not bootstrap a scriptless child Document.
/// Origin and sandbox checks also apply at this native boundary.
fn iframe_global_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let document = iframe_content_document_native(&JsValue::undefined(), args, context)?;
    if document.is_null() {
        return Ok(JsValue::null());
    }
    let iframe_id = parse_node_id(args.first(), context)?;
    let document_id = document.to_number(context)? as usize;
    let create_if_missing = args.get(1).is_none_or(JsValue::to_boolean);
    with_host_state(|state| {
        let realm = if create_if_missing {
            ensure_iframe_realm(context, state, iframe_id, document_id)?
        } else {
            let Some(realm) = state
                .borrow()
                .iframe_documents
                .get(&iframe_id)
                .and_then(|entry| entry.realm.clone())
            else {
                return Ok(JsValue::null());
            };
            realm
        };
        let previous = context.enter_realm(realm);
        let global = context.global_object();
        context.enter_realm(previous);
        Ok(global.into())
    })
}

/// Dispatches lifecycle events in the departing iframe's existing Realm.
/// A scriptless Document has no Realm to dispatch through yet; callers use
/// their same-origin wrapper without creating a full Realm just for departure.
fn dispatch_iframe_departure_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let iframe_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, iframe_id)?;
    with_host_state(|state| {
        let realm = {
            let state = state.borrow();
            let Some(entry) = state.iframe_documents.get(&iframe_id) else {
                return Ok(JsValue::from(false));
            };
            entry.realm.clone()
        };
        let realm = match realm {
            Some(realm) => realm,
            None => return Ok(JsValue::from(false)),
        };
        let previous = context.enter_realm(realm);
        let result = context.eval(Source::from_bytes("__omoikane_dispatch_iframe_departure()"));
        context.enter_realm(previous);
        result.map(|_| JsValue::from(true))
    })
}

/// Private visibility-dispatch lookup. Unlike `contentDocument`, this neither
/// starts an iframe load nor applies script-origin access checks.
fn existing_iframe_document_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let iframe_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, iframe_id)?;
    with_host_state(|state| {
        Ok(state
            .borrow()
            .iframe_documents
            .get(&iframe_id)
            .map(|entry| JsValue::from(entry.document.identity() as f64))
            .unwrap_or_else(JsValue::null))
    })
}

/// Returns `same:<context>:<generation>`, `cross:<context>:<generation>`, or
/// `closed` for a nested WindowProxy. `expectedContext` pins an already-created
/// proxy to its browsing context so detach/reconnect cannot revive it.
fn iframe_context_state_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let iframe_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, iframe_id)?;
    let expected_context = args
        .get(1)
        .filter(|value| !value.is_null_or_undefined())
        .map(|value| value.to_string(context))
        .transpose()?
        .map(|value| value.to_std_string_escaped().parse::<u64>())
        .transpose()
        .map_err(|_| {
            JsError::from(JsNativeError::typ().with_message("invalid iframe browsing context id"))
        })?
        .unwrap_or_default();
    with_host_state(|state| {
        let Some(iframe) = state.borrow().get_node(iframe_id) else {
            return Ok(js_string!("closed").into());
        };
        if !state.borrow().node_is_in_active_document(&iframe) {
            return Ok(js_string!("closed").into());
        }
        let document = state
            .borrow_mut()
            .iframe_content_document(&iframe)
            .map_err(|error| {
                JsError::from(JsNativeError::error().with_message(error.to_string()))
            })?;
        let state = state.borrow();
        let Some(context_id) = state.iframe_context_ids.get(&iframe_id).copied() else {
            return Ok(js_string!("closed").into());
        };
        if expected_context != 0 && expected_context != context_id {
            return Ok(js_string!("closed").into());
        }
        let generation = state
            .iframe_documents
            .get(&iframe_id)
            .map(|entry| entry.generation)
            .unwrap_or_default();
        let access = if state.iframe_document_is_same_origin(&iframe, &document)
            && state
                .sandbox_policy_for_document(&document)
                .exposes_document_to_parent()
        {
            "same"
        } else {
            "cross"
        };
        Ok(js_string!(format!("{access}:{context_id}:{generation}")).into())
    })
}

/// Forces the next load of a connected iframe to create a fresh Document and
/// Window generation even when its effective `src`/`srcdoc` is unchanged.
/// The browsing-context id is preserved, so existing WindowProxy objects are
/// retargeted rather than retired. Used by Location reload and history travel.
fn iframe_force_navigation_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let iframe_id = parse_node_id(args.first(), context)?;
    ensure_same_origin_node(context, iframe_id)?;
    with_host_state(|state| {
        let iframe = state.borrow().get_node(iframe_id);
        let Some(iframe) = iframe else {
            return Ok(JsValue::from(false));
        };
        if !state.borrow().node_is_in_active_document(&iframe) {
            return Ok(JsValue::from(false));
        }
        let mut state = state.borrow_mut();
        state.retire_iframe_document(iframe_id);
        state.schedule_connected_resource_loads(&iframe, true);
        Ok(JsValue::from(true))
    })
}

/// Drains identities whose browsing-context behavior was retired. Their
/// monotonic DOM identities remain valid while JavaScript retains the nodes.
fn take_discarded_node_ids_native(
    _: &JsValue,
    _: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    with_host_state(|state| {
        let ids = std::mem::take(&mut state.borrow_mut().discarded_node_ids)
            .into_iter()
            .map(|id| JsValue::from(id as f64));
        Ok(JsValue::from(
            boa_engine::object::builtins::JsArray::from_iter(ids, context),
        ))
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod inline_geometry_tests;

#[cfg(test)]
mod frameset_tests;
#[cfg(test)]
mod ua_display_tests;
#[cfg(test)]
mod visited_link_tests;
