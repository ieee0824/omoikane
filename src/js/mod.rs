//! JavaScript engine embedding and DOM/Web API bindings.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use boa_engine::native_function::NativeFunction;
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, Source, js_string};

use crate::dom::{Node, NodeHandle};
use crate::http::Client;

thread_local! {
    static ACTIVE_HOST_STATE: RefCell<Option<Rc<RefCell<HostState>>>> = const { RefCell::new(None) };
}

const DOM_BOOTSTRAP: &str = include_str!("dom_bootstrap.js");


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
    Callback { callback: JsValue, args: Vec<JsValue> },
}

#[derive(Debug, Clone)]
struct TimerTask {
    id: u64,
    payload: TimerPayload,
    next_run_at: u64,
    interval_ms: u64,
    repeat: bool,
}

#[derive(Debug, Default)]
struct EventLoopState {
    next_timer_id: u64,
    now_ms: u64,
    macrotasks: VecDeque<TimerPayload>,
    timers: Vec<TimerTask>,
}

impl EventLoopState {
    fn schedule_timer(&mut self, payload: TimerPayload, delay_ms: u64, repeat: bool) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id += 1;
        self.timers.push(TimerTask {
            id,
            payload,
            next_run_at: self.now_ms.saturating_add(delay_ms),
            interval_ms: delay_ms,
            repeat,
        });
        id
    }

    fn clear_timer(&mut self, id: u64) {
        self.timers.retain(|timer| timer.id != id);
    }

    fn advance(&mut self, elapsed_ms: u64) {
        self.now_ms = self.now_ms.saturating_add(elapsed_ms);

        // Collect every timer that is now due, remembering its original fire
        // time and id so we can enqueue them in fire-time order (ties broken by
        // registration order, since ids increase monotonically).
        let mut ready: Vec<(u64, u64, TimerPayload)> = Vec::new();
        for timer in &mut self.timers {
            if timer.next_run_at <= self.now_ms {
                ready.push((timer.next_run_at, timer.id, timer.payload.clone()));
                if timer.repeat {
                    timer.next_run_at = self.now_ms.saturating_add(timer.interval_ms);
                }
            }
        }

        ready.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        self.timers
            .retain(|timer| timer.repeat || timer.next_run_at > self.now_ms);

        for (_, _, payload) in ready {
            self.macrotasks.push_back(payload);
        }
    }

    fn drain_macrotasks(&mut self) -> Vec<TimerPayload> {
        self.macrotasks.drain(..).collect()
    }

    fn has_pending_timers(&self) -> bool {
        !self.timers.is_empty()
    }
}

#[derive(Debug)]
struct HostState {
    event_loop: EventLoopState,
    document: NodeHandle,
    nodes: HashMap<usize, NodeHandle>,
    console_logs: Vec<String>,
    location_href: String,
    navigator_user_agent: String,
    http_client: Client,
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
}

impl HostState {
    fn new(document: NodeHandle) -> Self {
        let mut state = Self {
            event_loop: EventLoopState::default(),
            document: document.clone(),
            nodes: HashMap::new(),
            console_logs: Vec::new(),
            location_href: "http://localhost/".to_string(),
            navigator_user_agent: "Omoikane/0.1".to_string(),
            http_client: Client::new(),
            write_insertion_ref: None,
        };
        state.register_tree(&document);
        state
    }

    fn register_tree(&mut self, node: &NodeHandle) {
        self.nodes.insert(node.identity(), node.clone());
        for child in node.child_nodes() {
            self.register_tree(&child);
        }
    }

    fn get_node(&self, id: usize) -> Option<NodeHandle> {
        self.nodes.get(&id).cloned()
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
            .field("sandbox", &format_args!("timeout={:?}", self.sandbox.timeout))
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

    /// Creates a JavaScript runtime with custom sandbox configuration.
    pub fn with_document_and_sandbox(
        document: NodeHandle,
        sandbox: SandboxConfig,
    ) -> JsResult<Self> {
        let host_state = Rc::new(RefCell::new(HostState::new(document.clone())));
        let mut context = Context::default();

        register_host_bindings(&mut context, &host_state)?;

        let mut runtime = Self {
            context,
            host_state,
            sandbox,
        };
        runtime.eval(DOM_BOOTSTRAP)?;
        Ok(runtime)
    }

    /// Returns the current DOM document.
    pub fn document(&self) -> NodeHandle {
        self.host_state.borrow().document.clone()
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
        self.with_active_host(|context| context.eval(Source::from_bytes(source)))
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

    /// Runs pending promise jobs.
    pub fn run_jobs(&mut self) -> JsResult<()> {
        self.with_active_host(|context| context.run_jobs())
    }

    /// Schedules a timeout task from Rust that evaluates `source` as code.
    pub fn set_timeout(&mut self, source: impl Into<String>, delay_ms: u64) -> u64 {
        self.host_state
            .borrow_mut()
            .event_loop
            .schedule_timer(TimerPayload::Source(source.into()), delay_ms, false)
    }

    /// Schedules an interval task from Rust that evaluates `source` as code.
    pub fn set_interval(&mut self, source: impl Into<String>, interval_ms: u64) -> u64 {
        self.host_state
            .borrow_mut()
            .event_loop
            .schedule_timer(TimerPayload::Source(source.into()), interval_ms, true)
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
        loop {
            let tasks = self.host_state.borrow_mut().event_loop.drain_macrotasks();
            if tasks.is_empty() {
                break;
            }

            for task in tasks {
                self.run_timer_payload(task)?;
                self.run_jobs()?;
            }
        }

        self.run_jobs()
    }

    /// Returns true if any timers are still scheduled (pending or repeating).
    pub fn has_pending_timers(&self) -> bool {
        self.host_state.borrow().event_loop.has_pending_timers()
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
            if !self.has_pending_timers() {
                break;
            }
            self.host_state.borrow_mut().event_loop.advance(step);
            advanced = advanced.saturating_add(step);

            loop {
                let tasks = self.host_state.borrow_mut().event_loop.drain_macrotasks();
                if tasks.is_empty() {
                    break;
                }
                let mut hit_cap = false;
                for task in tasks {
                    if tasks_run >= max_tasks {
                        hit_cap = true;
                        break;
                    }
                    // Swallow per-task JS errors: a single failing timer must
                    // not abort the whole pump during rendering.
                    let _ = self.run_timer_payload(task);
                    let _ = self.run_jobs();
                    tasks_run += 1;
                }
                if hit_cap {
                    break;
                }
            }
        }

        tasks_run
    }

    /// Executes a single timer payload: evaluates a source string, or invokes a
    /// retained function callback with its bound extra arguments.
    fn run_timer_payload(&mut self, payload: TimerPayload) -> JsResult<()> {
        match payload {
            TimerPayload::Source(source) => {
                self.eval(&source)?;
                Ok(())
            }
            TimerPayload::Callback { callback, args } => self.with_active_host(|context| {
                if let Some(callable) = callback.as_callable() {
                    callable.call(&JsValue::undefined(), &args, context)?;
                }
                Ok(())
            }),
        }
    }

    /// Dispatches a `DOMContentLoaded` event on the document.
    ///
    /// Call this after the DOM tree is fully constructed (e.g., after parsing HTML
    /// and executing inline scripts). Listeners registered via
    /// `document.addEventListener('DOMContentLoaded', fn)` will be invoked.
    pub fn fire_dom_content_loaded(&mut self) -> JsResult<()> {
        self.eval("document.dispatchEvent(new Event('DOMContentLoaded'))")?;
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
        self.eval("window.dispatchEvent(new Event('load', { bubbles: false }))")?;
        self.run_jobs()
    }

    /// Collects and executes all `<script>` elements in the document.
    ///
    /// - Inline scripts: text content is executed directly.
    /// - External scripts (`src` attribute): fetched via HTTP and executed.
    /// - `type` attribute must be absent, empty, or `text/javascript` / `module` (module is skipped).
    /// - `defer` scripts are collected and executed after all inline/sync scripts.
    /// - After all scripts, `DOMContentLoaded` is fired.
    ///
    /// Errors in individual scripts are logged but do not stop execution of remaining scripts.
    pub fn execute_document_scripts(&mut self, base_url: Option<&crate::http::Url>) -> Vec<String> {
        let document = self.document();
        let scripts = collect_script_elements(&document);
        let mut errors = Vec::new();
        let mut deferred = Vec::new();

        for script in &scripts {
            let attrs = script.attributes().unwrap_or_default();
            let script_type = attrs.get("type").map(|s| s.to_ascii_lowercase());

            // Skip non-JavaScript types
            if let Some(ref t) = script_type {
                // Strip MIME parameters (e.g., "text/javascript; charset=utf-8" → "text/javascript")
                let mime = t.split(';').next().unwrap_or("").trim();
                if !mime.is_empty()
                    && mime != "text/javascript"
                    && mime != "application/javascript"
                    && mime != "module"
                {
                    continue;
                }
                if mime == "module" {
                    continue;
                }
            }

            let src = attrs.get("src").cloned();
            // HTML spec: defer only applies to external (src) scripts.
            let is_defer = attrs.get("defer").is_some() && src.is_some();

            let source_code = if let Some(src_url) = src {
                // External script: fetch
                match fetch_script_source(&src_url, base_url) {
                    Some(code) => code,
                    None => {
                        errors.push(format!("failed to fetch script: {src_url}"));
                        continue;
                    }
                }
            } else {
                // Inline script: collect text content
                collect_text_content(script)
            };

            if source_code.trim().is_empty() {
                continue;
            }

            if is_defer {
                // Keep the <script> node alongside its source so the deferred
                // execution loop below can point `document.write`'s insertion
                // reference at this script (exactly like the inline path), rather
                // than letting a deferred write() fall back to appending at
                // <body>.
                deferred.push((source_code, script.clone()));
                continue;
            }

            // Point `document.write`'s insertion reference at this script so any
            // content it writes lands as the script's following siblings (the
            // HTML tokenizer inserts written text at the "insertion point",
            // i.e. right where the running <script> sits in the tree).
            self.host_state.borrow_mut().write_insertion_ref = Some(script.clone());

            // Execute immediately
            if let Err(err) = self.eval_safe(&source_code) {
                errors.push(err);
            }
            if let Err(err) = self.run_jobs() {
                errors.push(format!("{err}"));
            }

            // The insertion point is only defined while a script is running.
            self.host_state.borrow_mut().write_insertion_ref = None;
        }

        // Execute deferred scripts. Each runs with its own insertion point set
        // to its <script> element, so a `document.write` from a deferred script
        // lands as that script's following siblings — the same treatment the
        // inline path applies above.
        for (source_code, script) in deferred {
            self.host_state.borrow_mut().write_insertion_ref = Some(script.clone());
            if let Err(err) = self.eval_safe(&source_code) {
                errors.push(err);
            }
            if let Err(err) = self.run_jobs() {
                errors.push(format!("{err}"));
            }
            self.host_state.borrow_mut().write_insertion_ref = None;
        }

        // Fire DOMContentLoaded
        if let Err(err) = self.fire_dom_content_loaded() {
            errors.push(format!("{err}"));
        }

        errors
    }

    fn with_active_host<T>(&mut self, f: impl FnOnce(&mut Context) -> JsResult<T>) -> JsResult<T> {
        let host_state = Rc::clone(&self.host_state);
        ACTIVE_HOST_STATE.with(|slot| {
            let previous = slot.replace(Some(host_state));
            let result = f(&mut self.context);
            slot.replace(previous);
            result
        })
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
fn fetch_script_source(src: &str, base_url: Option<&crate::http::Url>) -> Option<String> {
    // data: URI scripts are decoded inline without a network fetch.
    if src.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:")) {
        let parsed = crate::http::parse_data_uri(src)?;
        if !is_javascript_mime_type(&parsed.mime_type) {
            return None;
        }
        return String::from_utf8(parsed.data).ok();
    }

    let url = if src.starts_with("http://") || src.starts_with("https://") {
        src.to_string()
    } else if let Some(base) = base_url {
        crate::http::url::resolve_url(base, src).ok()?.to_string()
    } else {
        return None;
    };

    let mut client = Client::new();
    let response = client.get(&url).ok()?;
    if response.status_code() != 200 {
        return None;
    }
    std::str::from_utf8(response.body()).ok().map(|s| s.to_string())
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
    drop(state);

    for (name, length, function) in [
        (
            js_string!("__omoikane_get_element_by_id"),
            1,
            NativeFunction::from_copy_closure(get_element_by_id_native),
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
            js_string!("__omoikane_fetch"),
            1,
            NativeFunction::from_copy_closure(fetch_native),
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
            js_string!("__omoikane_create_comment"),
            1,
            NativeFunction::from_copy_closure(create_comment_native),
        ),
        (
            js_string!("__omoikane_document_write"),
            1,
            NativeFunction::from_copy_closure(document_write_native),
        ),
        (
            js_string!("__omoikane_document_reset"),
            1,
            NativeFunction::from_copy_closure(document_reset_native),
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

fn get_element_by_id_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let document = state.borrow().document.clone();
        Ok(node_to_js_value(document.query_selector(&format!("#{id}"))))
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
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        Ok(node_to_js_value(
            node.and_then(|node| node.query_selector(&selector)),
        ))
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
        state.borrow_mut().nodes.insert(id, node);
        Ok(JsValue::from(id as f64))
    })
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
        parent.append_child(child.clone());
        state.borrow_mut().register_tree(&child);
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

fn attribute_names_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
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
        .to_std_string_escaped()
        .to_ascii_lowercase();
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let value = node
            .and_then(|node| node.attributes())
            .and_then(|attributes| attributes.get(&name).cloned());
        Ok(match value {
            Some(value) => js_string!(value.as_str()).into(),
            None => JsValue::null(),
        })
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
        node.set_attribute(name, value);
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

fn fetch_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let url = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    with_host_state(|state| {
        let mut state = state.borrow_mut();
        let response = state.http_client.get(&url).map_err(|error| {
            JsError::from(JsNativeError::error().with_message(error.to_string()))
        })?;
        let body_text = String::from_utf8_lossy(response.body()).to_string();
        let payload = format!(
            r#"{{"status":{},"ok":{},"url":"{}","bodyText":"{}"}}"#,
            response.status_code(),
            if (200..300).contains(&response.status_code()) {
                "true"
            } else {
                "false"
            },
            escape_json_string(&url),
            escape_json_string(&body_text),
        );
        Ok(js_string!(payload.as_str()).into())
    })
}

fn escape_json_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// ── Additional DOM native bindings ──────────────────────────────────────────

fn get_text_content_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        match node.node_type() {
            // DocumentType returns null per DOM spec
            crate::dom::NodeType::DocumentType => Ok(JsValue::null()),
            // Text and Comment return their data
            crate::dom::NodeType::Text | crate::dom::NodeType::Comment => {
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
            crate::dom::NodeType::Comment | crate::dom::NodeType::DocumentType => {}
            _ => {
                text.push_str(&collect_text_recursive(&child));
            }
        }
    }
    text
}

fn set_text_content_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let text = args.get(1).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        // For text/comment leaf nodes, update data directly
        if matches!(node.node_type(), crate::dom::NodeType::Text | crate::dom::NodeType::Comment) {
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
        Ok(JsValue::undefined())
    })
}

fn get_inner_html_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let html = serialize_inner_html(&node);
        Ok(js_string!(html.as_str()).into())
    })
}

fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_html_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

fn serialize_inner_html(node: &NodeHandle) -> String {
    let mut html = String::new();
    for child in node.child_nodes() {
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
        crate::dom::NodeType::DocumentType => {
            if let Some(name) = node.data() {
                html.push_str("<!DOCTYPE ");
                html.push_str(&name);
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

fn set_inner_html_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let html = args.get(1).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        for child in node.child_nodes() {
            let _ = node.remove_child(&child);
        }
        if !html.is_empty() {
            // Parse as fragment: wrap in body context and extract children
            let parsed = crate::html::TreeBuilder::parse(&format!("<body>{html}</body>")).document();
            let body = parsed.query_selector("body");
            let source = body.as_ref().map(|b| b.child_nodes()).unwrap_or_default();
            for child in source {
                node.append_child(child);
            }
        }
        Ok(JsValue::undefined())
    })
}

fn child_node_ids_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let children = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
            node.child_nodes()
        };
        {
            let mut s = state.borrow_mut();
            for child in &children {
                s.register_tree(child);
            }
        }
        let ids: Vec<JsValue> = children.iter().map(|c| JsValue::from(c.identity() as f64)).collect();
        Ok(boa_engine::JsValue::from(boa_engine::object::builtins::JsArray::from_iter(ids, context)))
    })
}

fn next_sibling_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
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

fn previous_sibling_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let parent = match node.parent_node() {
            Some(p) => p,
            None => return Ok(JsValue::null()),
        };
        let siblings = parent.child_nodes();
        let mut prev: Option<&NodeHandle> = None;
        for sibling in &siblings {
            if sibling.identity() == id {
                return Ok(prev.map(|p| JsValue::from(p.identity() as f64)).unwrap_or(JsValue::null()));
            }
            prev = Some(sibling);
        }
        Ok(JsValue::null())
    })
}

fn remove_child_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let child_id = args.get(1).cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let parent = state.get_node(parent_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("parent not found")))?;
        let child = state.get_node(child_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("child not found")))?;
        parent.remove_child(&child).map_err(|e| JsError::from(JsNativeError::error().with_message(e.to_string())))?;
        Ok(JsValue::undefined())
    })
}

fn insert_before_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let new_id = args.get(1).cloned().unwrap_or_default().to_number(context)? as usize;
    let ref_value = args.get(2).cloned().unwrap_or_default();
    with_host_state(|state| {
        let state = state.borrow();
        let parent = state.get_node(parent_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("parent not found")))?;
        let new_node = state.get_node(new_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("new node not found")))?;
        if ref_value.is_null() || ref_value.is_undefined() {
            parent.append_child(new_node);
        } else {
            let ref_id = ref_value.to_number(context)? as usize;
            let ref_node = state.get_node(ref_id);
            if let Some(ref_node) = ref_node {
                let _ = parent.insert_before(new_node.clone(), &ref_node);
            } else {
                parent.append_child(new_node);
            }
        }
        Ok(JsValue::undefined())
    })
}

fn query_selector_all_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let selector = args.get(1).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    with_host_state(|state| {
        let results = {
            let s = state.borrow();
            let parent = s.get_node(parent_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
            query_selector_all_recursive(&parent, &selector)
        };
        {
            let mut s = state.borrow_mut();
            for node in &results {
                s.register_tree(node);
            }
        }
        let ids: Vec<JsValue> = results.iter().map(|n| JsValue::from(n.identity() as f64)).collect();
        Ok(boa_engine::JsValue::from(boa_engine::object::builtins::JsArray::from_iter(ids, context)))
    })
}

/// Simple querySelectorAll: supports tag name, .class, #id selectors.
fn query_selector_all_recursive(node: &NodeHandle, selector: &str) -> Vec<NodeHandle> {
    let mut results = Vec::new();
    let selector = selector.trim();
    for child in node.child_nodes() {
        if matches_simple_selector(&child, selector) {
            results.push(child.clone());
        }
        results.extend(query_selector_all_recursive(&child, selector));
    }
    results
}

fn matches_simple_selector(node: &NodeHandle, selector: &str) -> bool {
    if node.node_type() != crate::dom::NodeType::Element {
        return false;
    }
    if let Some(cls) = selector.strip_prefix('.') {
        let class_attr = node.attributes().and_then(|a| a.get("class").cloned()).unwrap_or_default();
        return class_attr.split_whitespace().any(|c| c == cls);
    }
    if let Some(id) = selector.strip_prefix('#') {
        let id_attr = node.attributes().and_then(|a| a.get("id").cloned()).unwrap_or_default();
        return id_attr == id;
    }
    match node.tag_name() {
        Some(tag) => tag.eq_ignore_ascii_case(selector),
        None => false,
    }
}

fn node_type_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        let node_type = match node.node_type() {
            crate::dom::NodeType::Element => 1,
            crate::dom::NodeType::Text => 3,
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
        crate::dom::NodeType::Text => {
            NodeHandle::text(&node.data().unwrap_or_default())
        }
        crate::dom::NodeType::Comment => {
            NodeHandle::comment(&node.data().unwrap_or_default())
        }
        crate::dom::NodeType::Document => NodeHandle::document(),
        crate::dom::NodeType::DocumentFragment => NodeHandle::document_fragment(),
        crate::dom::NodeType::DocumentType => {
            NodeHandle::document_type(&node.data().unwrap_or_default())
        }
    };
    if deep {
        for child in node.child_nodes() {
            clone.append_child(clone_node_impl(&child, true));
        }
    }
    clone
}

fn clone_node_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let deep = args.get(1).cloned().unwrap_or_default().to_boolean();
    with_host_state(|state| {
        let clone = {
            let s = state.borrow();
            let node = s.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
            clone_node_impl(&node, deep)
        };
        let clone_id = clone.identity() as f64;
        state.borrow_mut().register_tree(&clone);
        Ok(JsValue::from(clone_id))
    })
}

fn remove_attribute_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let name = args.get(1).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    with_host_state(|state| {
        let state = state.borrow();
        let node = state.get_node(id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        node.remove_attribute(&name);
        Ok(JsValue::undefined())
    })
}

fn create_text_node_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = args.first().cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    let node = NodeHandle::text(&text);
    let id = node.identity() as f64;
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(node.identity(), node);
        Ok(JsValue::from(id))
    })
}

fn create_document_fragment_native(_: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let node = NodeHandle::document_fragment();
    let id = node.identity() as f64;
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(node.identity(), node);
        Ok(JsValue::from(id))
    })
}

fn create_comment_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let data = args.first().cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    let node = NodeHandle::comment(&data);
    let id = node.identity() as f64;
    with_host_state(|state| {
        state.borrow_mut().nodes.insert(node.identity(), node);
        Ok(JsValue::from(id))
    })
}

/// Inserts `child` into `parent` before `reference` when given, otherwise
/// appends it. If `insert_before` fails — for example the reference node is no
/// longer a child of `parent` — this falls back to appending, guaranteeing the
/// child always lands somewhere in `parent`'s subtree rather than being
/// silently dropped. Callers rely on this: `document.write` registers each
/// inserted node and advances its insertion point, both of which are only valid
/// for nodes that are actually in the tree.
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

/// Returns whether `node` is an inline classic `<script>` — one that
/// `document.write` should execute synchronously.
///
/// A script qualifies only when it has no `src` attribute (external scripts
/// carry no inline code to run) and its `type` is absent, empty, or a
/// JavaScript MIME type (so `type="module"` and non-JS types are excluded).
/// This mirrors the classic-script conditions in `execute_document_scripts`.
fn is_inline_classic_script(node: &NodeHandle) -> bool {
    if node.tag_name().as_deref() != Some("script") {
        return false;
    }
    let attrs = node.attributes().unwrap_or_default();
    if attrs.get("src").is_some() {
        return false;
    }
    match attrs.get("type") {
        None => true,
        Some(t) => {
            // Strip any MIME parameters (e.g. "text/javascript; charset=utf-8").
            let mime = t.split(';').next().unwrap_or("").trim();
            mime.is_empty() || is_javascript_mime_type(mime)
        }
    }
}

/// Backs `document.open()`'s reset semantics: removes every child of the given
/// document node so a following `document.write` builds fresh content into an
/// empty document (HTML's "document open steps" replace the document with an
/// empty one). Works for any Document node id, so it will apply equally to
/// sub-documents once iframe `contentDocument` lands (016-9); today it is
/// exercised on the main document.
fn document_reset_native(
    _: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = {
            let s = state.borrow();
            s.get_node(id).ok_or_else(|| {
                JsError::from(JsNativeError::error().with_message("document node not found"))
            })?
        };
        for child in node.child_nodes() {
            let _ = node.remove_child(&child);
        }
        // The emptied document has no insertion point; a following write()
        // appends into the now-childless document node.
        state.borrow_mut().write_insertion_ref = None;
        Ok(JsValue::undefined())
    })
}

/// Backs `document.write` / `document.writeln`.
///
/// Tokenizes the argument as an HTML fragment (in `<body>` context) and splices
/// the resulting nodes into the live tree at the current insertion point:
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
fn document_write_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();

    // Parse the fragment into a throwaway document and lift out the body's
    // children. Parsing per-call matches the existing innerHTML path; it means
    // a single write() call must contain balanced-enough markup (Acid3 writes
    // its whole fragment in one call), which is the common case.
    let parsed_children: Vec<NodeHandle> = if text.is_empty() {
        Vec::new()
    } else {
        let parsed = crate::html::TreeBuilder::parse(&format!("<body>{text}</body>")).document();
        parsed
            .query_selector("body")
            .map(|body| body.child_nodes())
            .unwrap_or_default()
    };

    with_host_state(|state| {
        // Resolve the parent element and the sibling to insert before.
        let (parent, reference_child) = {
            let s = state.borrow();
            // Fallback target when there is no active insertion point: the
            // <body>, or — after document.open() emptied the document so no
            // <body> exists — the document node itself, so the written nodes
            // become the (fresh) document's children.
            let fallback_parent = || {
                s.document
                    .query_selector("body")
                    .unwrap_or_else(|| s.document.clone())
            };
            match s.write_insertion_ref.clone() {
                // Active insertion point: insert right after the reference node
                // (i.e. before the reference node's next sibling).
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
        };

        let Some(parent) = parent else {
            // No insertion target at all; nothing sensible to do.
            return Ok(JsValue::from(
                boa_engine::object::builtins::JsArray::from_iter(Vec::<JsValue>::new(), context),
            ));
        };

        // Splice the parsed nodes in, preserving order. `insert_or_append`
        // guarantees every child actually lands in the tree even if
        // `insert_before` fails (e.g. a stale reference), so registering the
        // node and advancing the insertion point below stay consistent — we
        // never register or advance to a node that isn't in the tree.
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
            }
            if let Some(last) = last_inserted {
                if s.write_insertion_ref.is_some() {
                    s.write_insertion_ref = Some(last);
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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
                 child.dispatchEvent(new Event('click'));",
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
    fn exposes_window_console_and_navigator() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("console.log(window.location.href, navigator.userAgent);")
            .unwrap();

        let logs = runtime.console_logs();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("http://localhost/"));
        assert!(logs[0].contains("Omoikane/0.1"));
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
    fn eval_safe_catches_syntax_error() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime.eval_safe("this is not valid javascript }{");
        assert!(result.is_err(), "eval_safe should return Err for syntax errors");
    }

    #[test]
    fn eval_safe_catches_runtime_error() {
        let mut runtime = JsRuntime::new().unwrap();
        let result = runtime.eval_safe("undefinedFunction()");
        assert!(result.is_err(), "eval_safe should return Err for runtime errors");
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
        assert!(result.is_err(), "accessing 'process' should throw ReferenceError");
        let result = runtime.eval_safe("require");
        assert!(result.is_err(), "accessing 'require' should throw ReferenceError");
    }

    #[test]
    fn classname_getter_and_setter() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const el = document.querySelector("div");
            el.className = "foo bar";
        "#).unwrap();

        let result = runtime.eval("document.querySelector('div').className")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "foo bar");
    }

    #[test]
    fn classlist_add_remove_contains() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const el = document.querySelector("div");
            el.classList.add("alpha", "beta");
        "#).unwrap();

        let has_alpha = runtime.eval("document.querySelector('div').classList.contains('alpha')")
            .unwrap().as_boolean().unwrap();
        assert!(has_alpha, "classList should contain 'alpha'");

        runtime.eval("document.querySelector('div').classList.remove('alpha')").unwrap();
        let has_alpha = runtime.eval("document.querySelector('div').classList.contains('alpha')")
            .unwrap().as_boolean().unwrap();
        assert!(!has_alpha, "classList should not contain 'alpha' after remove");

        let has_beta = runtime.eval("document.querySelector('div').classList.contains('beta')")
            .unwrap().as_boolean().unwrap();
        assert!(has_beta, "classList should still contain 'beta'");
    }

    #[test]
    fn classlist_toggle() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime.eval(r#"
            const el = document.querySelector("div");
            el.classList.toggle("active");
        "#).unwrap().as_boolean().unwrap();
        assert!(result, "toggle should return true when adding");

        let result = runtime.eval("document.querySelector('div').classList.toggle('active')")
            .unwrap().as_boolean().unwrap();
        assert!(!result, "toggle should return false when removing");
    }

    #[test]
    fn style_getter_and_setter() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const el = document.querySelector("div");
            el.style.backgroundColor = "red";
            el.style.fontSize = "16px";
        "#).unwrap();

        let bg = runtime.eval("document.querySelector('div').style.backgroundColor")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(bg, "red");

        let fs = runtime.eval("document.querySelector('div').style.fontSize")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(fs, "16px");

        // Verify the style attribute on the DOM node
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
        assert!(style_attr.contains("background-color: red"), "style attr: {style_attr}");
        assert!(style_attr.contains("font-size: 16px"), "style attr: {style_attr}");
    }

    #[test]
    fn get_set_attribute_round_trip() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const el = document.querySelector("div");
            el.setAttribute("data-value", "42");
        "#).unwrap();

        let result = runtime.eval("document.querySelector('div').getAttribute('data-value')")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "42");

        let missing = runtime.eval("document.querySelector('div').getAttribute('nonexistent')")
            .unwrap();
        assert!(missing.is_null(), "getAttribute for missing attr should return null");
    }

    #[test]
    fn classlist_length_and_item() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const el = document.querySelector("div");
            el.classList.add("a", "b", "c");
        "#).unwrap();

        let len = runtime.eval("document.querySelector('div').classList.length")
            .unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(len, 3.0);

        let item0 = runtime.eval("document.querySelector('div').classList.item(0)")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(item0, "a");

        let item_oob = runtime.eval("document.querySelector('div').classList.item(99)")
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
        let result = runtime.eval(r#"
            const el = document.querySelector("div");
            el.classList.toggle("x", true);
        "#).unwrap().as_boolean().unwrap();
        assert!(result, "toggle(cls, true) should return true");

        // force=true when already present keeps it
        let result = runtime.eval("document.querySelector('div').classList.toggle('x', true)")
            .unwrap().as_boolean().unwrap();
        assert!(result, "toggle(cls, true) when present should return true");

        // force=false always removes
        let result = runtime.eval("document.querySelector('div').classList.toggle('x', false)")
            .unwrap().as_boolean().unwrap();
        assert!(!result, "toggle(cls, false) should return false");

        let has = runtime.eval("document.querySelector('div').classList.contains('x')")
            .unwrap().as_boolean().unwrap();
        assert!(!has, "x should be removed after toggle(x, false)");
    }

    #[test]
    fn style_value_zero_is_preserved() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const el = document.querySelector("div");
            el.style.marginTop = 0;
        "#).unwrap();

        let result = runtime.eval("document.querySelector('div').style.marginTop")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "0", "style value 0 should be preserved, not removed");
    }

    #[test]
    fn remove_event_listener_stops_callback() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            let count = 0;
            const el = document.querySelector("div");
            function handler() { count++; }
            el.addEventListener("click", handler);
            el.dispatchEvent(new Event("click"));
            el.removeEventListener("click", handler);
            el.dispatchEvent(new Event("click"));
        "#).unwrap();

        let count = runtime.eval("count").unwrap()
            .to_number(&mut runtime.context).unwrap();
        assert_eq!(count, 1.0, "handler should fire once before removal, not after");
    }

    #[test]
    fn fire_dom_content_loaded_invokes_listeners() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime.eval(r#"
            let loaded = false;
            document.addEventListener("DOMContentLoaded", () => { loaded = true; });
        "#).unwrap();

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

        runtime.eval(r#"
            let eventFired = "";
            document.addEventListener("myevent", (e) => { eventFired = e.type; });
        "#).unwrap();

        runtime.fire_document_event("myevent").unwrap();

        let result = runtime.eval("eventFired").unwrap()
            .as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "myevent");
    }

    #[test]
    fn fire_document_event_escapes_special_chars() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        runtime.eval(r#"
            let special = "";
            document.addEventListener("te'st", (e) => { special = e.type; });
        "#).unwrap();

        runtime.fire_document_event("te'st").unwrap();

        let result = runtime.eval("special").unwrap()
            .as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "te'st");
    }

    #[test]
    fn add_event_listener_deduplicates() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            let count = 0;
            const el = document.querySelector("div");
            function handler() { count++; }
            el.addEventListener("click", handler);
            el.addEventListener("click", handler);
            el.addEventListener("click", handler);
            el.dispatchEvent(new Event("click"));
        "#).unwrap();

        let count = runtime.eval("count").unwrap()
            .to_number(&mut runtime.context).unwrap();
        assert_eq!(count, 1.0, "duplicate addEventListener should only fire once");
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
        assert!(result, "DOMContentLoaded should fire after execute_document_scripts");
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

        let result = runtime.eval("order").unwrap()
            .as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "first,second,", "defer on inline should be ignored; both run in order");
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

        let count = runtime.eval("globalThis.loadCount").unwrap().as_number().unwrap();
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
        assert_eq!(ty, "click", "onclick handler should receive the event argument");
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
        assert_eq!(value, "hello|world|world|true|5", "Text.data get/set must map to character data");
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
        assert_eq!(value, "note|changed|true", "Comment.data get/set must map to character data");
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
        assert!(is_undefined, "Element nodes must not expose CharacterData.data");
    }

    #[test]
    fn document_default_view_is_global() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let same = runtime
            .eval("document.defaultView === globalThis && document.defaultView === window")
            .unwrap()
            .as_boolean()
            .unwrap();
        assert!(same, "document.defaultView must be the global window object");
    }

    #[test]
    fn node_type_constants_are_exposed() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        // On the Node constructor.
        assert_eq!(runtime.eval("Node.ELEMENT_NODE").unwrap().as_number().unwrap(), 1.0);
        assert_eq!(runtime.eval("Node.TEXT_NODE").unwrap().as_number().unwrap(), 3.0);
        assert_eq!(runtime.eval("Node.COMMENT_NODE").unwrap().as_number().unwrap(), 8.0);
        assert_eq!(runtime.eval("Node.DOCUMENT_NODE").unwrap().as_number().unwrap(), 9.0);
        assert_eq!(runtime.eval("Node.DOCUMENT_TYPE_NODE").unwrap().as_number().unwrap(), 10.0);
        assert_eq!(runtime.eval("Node.DOCUMENT_FRAGMENT_NODE").unwrap().as_number().unwrap(), 11.0);
        assert_eq!(runtime.eval("Node.NOTATION_NODE").unwrap().as_number().unwrap(), 12.0);
        // On instances via the prototype (as Acid3 test 19 checks).
        assert_eq!(
            runtime.eval("document.DOCUMENT_FRAGMENT_NODE").unwrap().as_number().unwrap(),
            11.0,
            "document must inherit DOCUMENT_FRAGMENT_NODE"
        );
        assert_eq!(
            runtime.eval("document.createTextNode('').ELEMENT_NODE").unwrap().as_number().unwrap(),
            1.0,
            "text node must inherit ELEMENT_NODE"
        );
    }

    #[test]
    fn local_name_lowercases_elements_and_is_null_for_others() {
        let mut runtime = JsRuntime::with_document(default_document()).unwrap();
        let el = runtime
            .eval("document.createElement('DIV').localName")
            .unwrap()
            .as_string()
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(el, "div", "element localName must be the lower-cased tag name");
        // Non-element nodes (text, comment, document) have null localName.
        assert!(runtime.eval("document.createTextNode('x').localName === null").unwrap().as_boolean().unwrap());
        assert!(runtime.eval("document.createComment('x').localName === null").unwrap().as_boolean().unwrap());
        assert!(runtime.eval("document.localName === null").unwrap().as_boolean().unwrap());
    }

    // --- 016-5: data: URI scripts ---

    #[test]
    fn fetch_script_source_decodes_acid3_data_uri_vectors() {
        // The five Acid3 (test 97) vectors and the JS source each must yield.
        let cases = [
            ("data:text/javascript,d1%20%3D%20'one'%3B", "d1 = 'one';"),
            ("data:text/javascript;base64,ZDIgPSAndHdvJzs%3D", "d2 = 'two';"),
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
        runtime.eval(r#"
            var observed = false;
            var intersecting = false;
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => {
                observed = true;
                intersecting = entries[0].isIntersecting;
            });
            observer.observe(el);
        "#).unwrap();
        runtime.run_jobs().unwrap();

        let observed = runtime.eval("observed").unwrap().as_boolean().unwrap();
        assert!(observed, "IntersectionObserver callback should fire after observe()");

        let intersecting = runtime.eval("intersecting").unwrap().as_boolean().unwrap();
        assert!(intersecting, "entry.isIntersecting should be true in headless mode");
    }

    #[test]
    fn intersection_observer_reobserve_fires_callback_again() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            var count = 0;
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => { count++; });
            observer.observe(el);
            observer.unobserve(el);
            observer.observe(el);
        "#).unwrap();
        runtime.run_jobs().unwrap();

        let count = runtime.eval("count").unwrap()
            .to_number(&mut runtime.context).unwrap();
        // observe → unobserve → observe: callback fires for each observe (2 times)
        assert_eq!(count, 2.0, "callback should fire for each observe() call");
    }

    #[test]
    fn intersection_observer_disconnect_clears_targets() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            var count = 0;
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => { count++; });
            observer.observe(el);
            observer.disconnect();
            observer.observe(el);
        "#).unwrap();
        runtime.run_jobs().unwrap();

        let count = runtime.eval("count").unwrap()
            .to_number(&mut runtime.context).unwrap();
        assert_eq!(count, 2.0, "disconnect then re-observe should fire callback again");
    }

    #[test]
    fn intersection_observer_classlist_add_pattern() {
        // Simulate the common pattern: IO + classList.add('on') for fade-in
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        div.set_attribute("class", "fade");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const el = document.querySelector("div");
            const observer = new IntersectionObserver((entries) => {
                entries.forEach(entry => {
                    if (entry.isIntersecting) {
                        entry.target.classList.add("on");
                    }
                });
            });
            observer.observe(el);
        "#).unwrap();
        runtime.run_jobs().unwrap();

        let has_on = runtime.eval("document.querySelector('div').classList.contains('on')")
            .unwrap().as_boolean().unwrap();
        assert!(has_on, "IO should add 'on' class via classList.add");

        // Verify the DOM attribute
        let class_attr = div.attributes().unwrap().get("class").cloned().unwrap_or_default();
        assert!(class_attr.contains("on"), "DOM class attr should contain 'on': {class_attr}");
    }

    #[test]
    fn text_content_getter_and_setter() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let text = NodeHandle::text("Hello world");
        div.append_child(text);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime.eval("document.querySelector('div').textContent")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "Hello world");

        runtime.eval("document.querySelector('div').textContent = 'Changed'").unwrap();
        let result = runtime.eval("document.querySelector('div').textContent")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "Changed");
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
        let len = runtime.eval("document.querySelector('div').childNodes.length")
            .unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(len, 2.0);
    }

    #[test]
    fn document_body_and_create_text_node() {
        use crate::html::TreeBuilder;
        let html = "<html><body></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let has_body = runtime.eval("document.body !== null")
            .unwrap().as_boolean().unwrap();
        assert!(has_body, "document.body should not be null");

        runtime.eval(r#"
            const t = document.createTextNode("Hello");
            document.body.appendChild(t);
        "#).unwrap();

        let result = runtime.eval("document.body.textContent")
            .unwrap().as_string().unwrap().to_std_string_escaped();
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
        let len = runtime.eval("document.querySelectorAll('span').length")
            .unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(len, 2.0);
    }

    #[test]
    fn node_type_returns_correct_values() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div>text</div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let div_type = runtime.eval("document.querySelector('div').nodeType")
            .unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(div_type, 1.0, "element nodeType should be 1");

        let doc_type = runtime.eval("document.nodeType")
            .unwrap().to_number(&mut runtime.context).unwrap();
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
        let shallow_children = runtime.eval(r#"
            const el = document.querySelector('div');
            const shallow = el.cloneNode(false);
            shallow.childNodes.length;
        "#).unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(shallow_children, 0.0, "shallow clone should have no children");

        let shallow_class = runtime.eval("shallow.className")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(shallow_class, "original", "shallow clone should preserve attributes");

        // Deep clone should copy children too
        let deep_children = runtime.eval(r#"
            const deep = el.cloneNode(true);
            deep.childNodes.length;
        "#).unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(deep_children, 1.0, "deep clone should have children");
    }

    #[test]
    fn remove_attribute_removes_from_dom() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        div.set_attribute("data-value", "42");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let has = runtime.eval("document.querySelector('div').hasAttribute('data-value')")
            .unwrap().as_boolean().unwrap();
        assert!(has, "attribute should exist before removal");

        runtime.eval("document.querySelector('div').removeAttribute('data-value')").unwrap();

        let has = runtime.eval("document.querySelector('div').hasAttribute('data-value')")
            .unwrap().as_boolean().unwrap();
        assert!(!has, "attribute should be removed");
    }

    #[test]
    fn create_document_fragment_has_correct_node_type() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let node_type = runtime.eval("document.createDocumentFragment().nodeType")
            .unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(node_type, 11.0, "DocumentFragment nodeType should be 11");
    }

    #[test]
    fn document_fragment_can_hold_children() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div id='target'></div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            const frag = document.createDocumentFragment();
            frag.appendChild(document.createElement('p'));
            frag.appendChild(document.createElement('span'));
            document.getElementById('target').appendChild(frag);
        "#).unwrap();

        let children = runtime.eval("document.getElementById('target').childNodes.length")
            .unwrap().to_number(&mut runtime.context).unwrap();
        // Fragment itself is appended (not its children individually) since we don't have
        // special fragment append semantics yet, but the fragment node holds the children.
        assert!(children > 0.0, "target should have children after appending fragment");
    }

    #[test]
    fn inner_html_round_trip() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div id='box'><span class=\"a\">Hello</span></div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let inner = runtime.eval("document.getElementById('box').innerHTML")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert!(inner.contains("<span"), "innerHTML should contain span tag");
        assert!(inner.contains("Hello"), "innerHTML should contain text");

        // Set and re-read
        runtime.eval(r#"document.getElementById('box').innerHTML = '<b>Bold</b>'"#).unwrap();
        let inner = runtime.eval("document.getElementById('box').innerHTML")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert!(inner.contains("<b>"), "innerHTML should contain b tag after set");
        assert!(inner.contains("Bold"), "innerHTML should contain text after set");
    }

    #[test]
    fn inner_html_escapes_text() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let text = NodeHandle::text("<script>alert(1)</script>");
        div.append_child(text);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let inner = runtime.eval("document.querySelector('div').innerHTML")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert!(inner.contains("&lt;script&gt;"), "innerHTML should escape angle brackets in text: {inner}");
        assert!(!inner.contains("<script>"), "innerHTML should not contain raw script tag");
    }

    #[test]
    fn text_content_null_sets_empty() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        let text = NodeHandle::text("Hello");
        div.append_child(text);
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval("document.querySelector('div').textContent = null").unwrap();
        let result = runtime.eval("document.querySelector('div').textContent")
            .unwrap().as_string().unwrap().to_std_string_escaped();
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
        runtime.eval("document.querySelector('div').innerHTML = null").unwrap();
        let result = runtime.eval("document.querySelector('div').innerHTML")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "", "innerHTML = null should produce empty string");
    }

    #[test]
    fn text_content_excludes_comments() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div>Hello<!-- comment -->World</div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime.eval("document.querySelector('div').textContent")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "HelloWorld", "textContent should not include comment data");
    }

    #[test]
    fn document_fragment_appends_children_not_self() {
        use crate::html::TreeBuilder;
        let html = "<html><body><div id='target'></div></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc.clone()).unwrap();
        runtime.eval(r#"
            const frag = document.createDocumentFragment();
            const p = document.createElement('p');
            const span = document.createElement('span');
            frag.appendChild(p);
            frag.appendChild(span);
            document.getElementById('target').appendChild(frag);
        "#).unwrap();

        // Fragment's children should be directly under target
        let target = doc.query_selector("#target").unwrap();
        let children = target.child_nodes();
        let tags: Vec<_> = children.iter().filter_map(|c| c.tag_name()).collect();
        assert_eq!(tags, vec!["p", "span"], "fragment children should be appended directly");
    }

    #[test]
    fn owner_document_is_null_for_document() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let is_null = runtime.eval("document.ownerDocument === null")
            .unwrap().as_boolean().unwrap();
        assert!(is_null, "document.ownerDocument should be null");

        runtime.eval("const el = document.createElement('div')").unwrap();
        let is_doc = runtime.eval("el.ownerDocument === document")
            .unwrap().as_boolean().unwrap();
        assert!(is_doc, "element.ownerDocument should be document");
    }

    #[test]
    fn tag_name_undefined_for_non_elements() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let is_undef = runtime.eval("document.createTextNode('x').tagName === undefined")
            .unwrap().as_boolean().unwrap();
        assert!(is_undef, "text node tagName should be undefined");

        let tag = runtime.eval("document.createElement('div').tagName")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(tag, "DIV", "element tagName should be uppercase tag");
    }

    #[test]
    fn comment_text_content_returns_data() {
        use crate::html::TreeBuilder;
        let html = "<html><body><!-- hello --></body></html>";
        let doc = TreeBuilder::parse(html).document();

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        // Comment nodes are in the body's childNodes
        let result = runtime.eval(r#"
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
        "#).unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result.trim(), "hello", "comment textContent should return its data");
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

        let count = runtime.eval("count").unwrap()
            .to_number(&mut runtime.context).unwrap();
        assert_eq!(count, 1.0, "stopImmediatePropagation should prevent later listeners");

        let prevented = runtime.eval("prevented").unwrap()
            .as_boolean().unwrap();
        assert!(prevented, "preventDefault should set defaultPrevented");

        let dispatch_return = runtime.eval("dispatchReturn").unwrap()
            .as_boolean().unwrap();
        assert!(!dispatch_return, "dispatchEvent should return false when default prevented");
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
        runtime.eval(r#"
            let count = 0;
            let parentFired = false;
            const el = document.querySelector("div");
            el.addEventListener("click", (e) => { count++; e.stopPropagation(); });
            el.addEventListener("click", () => { count++; });
            document.querySelector("body").addEventListener("click", () => { parentFired = true; });
            el.dispatchEvent(new Event("click", { bubbles: true }));
        "#).unwrap();

        let count = runtime.eval("count").unwrap()
            .to_number(&mut runtime.context).unwrap();
        assert_eq!(count, 2.0, "stopPropagation should NOT prevent other listeners on same node");

        let parent_fired = runtime.eval("parentFired").unwrap()
            .as_boolean().unwrap();
        assert!(!parent_fired, "stopPropagation should prevent bubbling to parent");
    }

    #[test]
    fn custom_event_has_detail() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            let detail = null;
            document.addEventListener("custom", (e) => { detail = e.detail; });
            document.dispatchEvent(new CustomEvent("custom", { detail: 42 }));
        "#).unwrap();

        let detail = runtime.eval("detail").unwrap()
            .to_number(&mut runtime.context).unwrap();
        assert_eq!(detail, 42.0, "CustomEvent detail should be accessible");
    }

    #[test]
    fn dataset_proxy_read_write() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        div.set_attribute("data-foo", "bar");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let result = runtime.eval("document.querySelector('div').dataset.foo")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "bar");

        runtime.eval("document.querySelector('div').dataset.baz = 'qux'").unwrap();
        let attr = div.attributes().unwrap().get("data-baz").cloned();
        assert_eq!(attr.as_deref(), Some("qux"), "dataset setter should set data- attribute");
    }

    #[test]
    fn is_connected_property() {
        let doc = NodeHandle::document();
        let div = NodeHandle::element("div");
        doc.append_child(div.clone());

        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let connected = runtime.eval("document.querySelector('div').isConnected")
            .unwrap().as_boolean().unwrap();
        assert!(connected, "element in document should be connected");

        let disconnected = runtime.eval("document.createElement('span').isConnected")
            .unwrap().as_boolean().unwrap();
        assert!(!disconnected, "orphan element should not be connected");
    }

    #[test]
    fn document_create_comment() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let node_type = runtime.eval("document.createComment('test').nodeType")
            .unwrap().to_number(&mut runtime.context).unwrap();
        assert_eq!(node_type, 8.0, "comment nodeType should be 8");
    }

    #[test]
    fn document_ready_state() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        let state = runtime.eval("document.readyState")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(state, "complete");
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
        let matches = runtime.eval("document.querySelector('span').matches('span')")
            .unwrap().as_boolean().unwrap();
        assert!(matches, "span should match 'span'");

        let closest = runtime.eval("document.querySelector('span').closest('.wrapper').tagName")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(closest, "DIV");
    }

    #[test]
    fn local_storage_stub() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval(r#"
            localStorage.setItem("key", "value");
        "#).unwrap();

        let result = runtime.eval("localStorage.getItem('key')")
            .unwrap().as_string().unwrap().to_std_string_escaped();
        assert_eq!(result, "value");

        runtime.eval("localStorage.removeItem('key')").unwrap();
        let removed = runtime.eval("localStorage.getItem('key')").unwrap();
        assert!(removed.is_null(), "removed item should return null");
    }

    #[test]
    fn match_media_returns_object() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let matches = runtime.eval("matchMedia('(min-width: 768px)').matches")
            .unwrap().as_boolean().unwrap();
        assert!(!matches, "matchMedia stub should return matches=false");
    }

    #[test]
    fn request_animation_frame_calls_callback() {
        let doc = NodeHandle::document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        runtime.eval("globalThis.rafCalled = false; requestAnimationFrame(() => { globalThis.rafCalled = true; });").unwrap();
        runtime.run_jobs().unwrap();

        let called = runtime.eval("rafCalled").unwrap().as_boolean().unwrap();
        assert!(called, "requestAnimationFrame callback should be called");
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
        runtime.eval(r#"
            const parent = document.querySelector('div');
            const oldChild = document.querySelector('#old');
            const newChild = document.createElement('b');
            newChild.id = 'new';
            parent.replaceChild(newChild, oldChild);
        "#).unwrap();

        let found_new = runtime.eval("document.querySelector('#new') !== null")
            .unwrap().as_boolean().unwrap();
        assert!(found_new, "new child should be in DOM");

        let found_old = runtime.eval("document.querySelector('#old')")
            .unwrap();
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
        assert_eq!(runtime.eval("globalThis.count").unwrap().as_number(), Some(1.0));
        runtime.tick(5).unwrap();
        assert_eq!(runtime.eval("globalThis.count").unwrap().as_number(), Some(2.0));

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
        assert_eq!(runtime.eval("globalThis.marker").unwrap().as_number(), Some(0.0));
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
            target.attributes().unwrap_or_default().get("data-done").map(|s| s.as_str()),
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
        assert_eq!(tasks, 10, "virtual-time budget should bound interval firings");
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
        assert!(matches, "String.prototype.substr must handle negative start");
    }

    #[test]
    fn run_timers_caps_infinite_interval_by_task_count() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .eval("globalThis.ticks = 0; setInterval(function () { globalThis.ticks += 1; }, 1);")
            .unwrap();

        // Effectively unlimited virtual time, but the task cap stops it.
        let tasks = runtime.run_timers(u64::MAX, 1, 50);
        assert_eq!(tasks, 50, "task-count cap should bound total timer executions");
        assert_eq!(
            runtime.eval("globalThis.ticks").unwrap().as_number(),
            Some(50.0),
            "infinite interval must stop at the task-count cap"
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
            vec![
                "script".to_string(),
                "div".to_string(),
                "p".to_string()
            ],
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
            runtime.eval("globalThis.__written_ran").unwrap().as_number(),
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
                && n.data().as_deref().map(|d| d.contains('\n')).unwrap_or(false)
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
            selectors.attributes().unwrap_or_default().get("style").map(|s| s.as_str()),
            Some("height: 100px"),
            "the written iframe must be mutable through the DOM API"
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

        // Module: type="module".
        let module = NodeHandle::element("script");
        module.set_attribute("type", "module");
        assert!(!is_inline_classic_script(&module));

        // Non-JS type.
        let non_js = NodeHandle::element("script");
        non_js.set_attribute("type", "text/plain");
        assert!(!is_inline_classic_script(&non_js));

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
            ext.attributes().unwrap_or_default().get("src").map(|s| s.as_str()),
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
    /// freshly written content (the prior content is erased by open()).
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
        // Only the written content remains, reachable by id.
        let fresh = doc
            .query_selector("#fresh")
            .expect("the written content must be present after open/write/close");
        assert_eq!(fresh.tag_name().as_deref(), Some("div"));
        let tags: Vec<String> = doc
            .child_nodes()
            .iter()
            .filter_map(|n| n.tag_name())
            .collect();
        assert_eq!(
            tags,
            vec!["div".to_string()],
            "the document must contain only the freshly written <div>"
        );
    }
}
