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


#[derive(Debug, Clone, PartialEq, Eq)]
struct TimerTask {
    id: u64,
    source: String,
    next_run_at: u64,
    interval_ms: u64,
    repeat: bool,
}

#[derive(Debug, Default)]
struct EventLoopState {
    next_timer_id: u64,
    now_ms: u64,
    macrotasks: VecDeque<String>,
    timers: Vec<TimerTask>,
}

impl EventLoopState {
    fn schedule_timer(&mut self, source: String, delay_ms: u64, repeat: bool) -> u64 {
        let id = self.next_timer_id;
        self.next_timer_id += 1;
        self.timers.push(TimerTask {
            id,
            source,
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

        let mut ready = Vec::new();
        for timer in &mut self.timers {
            if timer.next_run_at <= self.now_ms {
                ready.push((
                    timer.id,
                    timer.source.clone(),
                    timer.repeat,
                    timer.interval_ms,
                ));
                if timer.repeat {
                    timer.next_run_at = self.now_ms.saturating_add(timer.interval_ms);
                }
            }
        }

        self.timers
            .retain(|timer| timer.repeat || timer.next_run_at > self.now_ms);

        for (_, source, _, _) in ready {
            self.macrotasks.push_back(source);
        }
    }

    fn drain_macrotasks(&mut self) -> Vec<String> {
        self.macrotasks.drain(..).collect()
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

    /// Schedules a timeout task from Rust.
    pub fn set_timeout(&mut self, source: impl Into<String>, delay_ms: u64) -> u64 {
        self.host_state
            .borrow_mut()
            .event_loop
            .schedule_timer(source.into(), delay_ms, false)
    }

    /// Schedules an interval task from Rust.
    pub fn set_interval(&mut self, source: impl Into<String>, interval_ms: u64) -> u64 {
        self.host_state
            .borrow_mut()
            .event_loop
            .schedule_timer(source.into(), interval_ms, true)
    }

    /// Clears a previously scheduled timer.
    pub fn clear_timer(&mut self, id: u64) {
        self.host_state.borrow_mut().event_loop.clear_timer(id);
    }

    /// Advances the event loop clock and runs due macrotasks and pending jobs.
    pub fn tick(&mut self, elapsed_ms: u64) -> JsResult<()> {
        self.host_state.borrow_mut().event_loop.advance(elapsed_ms);
        self.run_until_idle()
    }

    /// Runs queued macrotasks and pending promise jobs until idle.
    pub fn run_until_idle(&mut self) -> JsResult<()> {
        loop {
            let tasks = self.host_state.borrow_mut().event_loop.drain_macrotasks();
            if tasks.is_empty() {
                break;
            }

            for task in tasks {
                self.eval(&task)?;
                self.run_jobs()?;
            }
        }

        self.run_jobs()
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
                deferred.push(source_code);
                continue;
            }

            // Execute immediately
            if let Err(err) = self.eval_safe(&source_code) {
                errors.push(err);
            }
            if let Err(err) = self.run_jobs() {
                errors.push(format!("{err}"));
            }
        }

        // Execute deferred scripts
        for source_code in deferred {
            if let Err(err) = self.eval_safe(&source_code) {
                errors.push(err);
            }
            if let Err(err) = self.run_jobs() {
                errors.push(format!("{err}"));
            }
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

/// Fetches an external script source via HTTP.
fn fetch_script_source(src: &str, base_url: Option<&crate::http::Url>) -> Option<String> {
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
    let source = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    let delay_ms = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_u32(context)
        .unwrap_or(0) as u64;

    with_host_state(|state| {
        let id = state
            .borrow_mut()
            .event_loop
            .schedule_timer(source, delay_ms, repeat);
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
}
