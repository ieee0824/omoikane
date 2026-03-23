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

const DOM_BOOTSTRAP: &str = r#"
(() => {
  const cache = new Map();

  function wrapNode(id) {
    if (id === null || id === undefined) {
      return null;
    }
    if (cache.has(id)) {
      return cache.get(id);
    }
    const node = id === __omoikane_document_id ? new Document(id) : new Node(id);
    cache.set(id, node);
    return node;
  }

  function invokeListeners(node, event, capture, phase) {
    const listeners = node.__listeners.get(event.type) || [];
    for (const entry of listeners) {
      if (!!entry.capture === capture) {
        event.currentTarget = node;
        event.eventPhase = phase;
        entry.listener.call(node, event);
        if (event.__stopped) {
          return true;
        }
      }
    }
    return false;
  }

  class Event {
    constructor(type, init = {}) {
      this.type = String(type);
      this.bubbles = init.bubbles ?? true;
      this.cancelable = init.cancelable ?? false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.__stopped = false;
    }

    stopPropagation() {
      this.__stopped = true;
    }
  }

  class Node {
    constructor(id) {
      this.__id = id;
      this.__listeners = new Map();
    }

    appendChild(child) {
      __omoikane_append_child(this.__id, child.__id);
      return child;
    }

    querySelector(selector) {
      const id = __omoikane_query_selector(this.__id, String(selector));
      return wrapNode(id);
    }

    addEventListener(type, listener, options = false) {
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const key = String(type);
      const list = this.__listeners.get(key) ?? [];
      // Deduplicate: same listener+capture is only registered once (DOM spec).
      if (!list.some(entry => entry.listener === listener && !!entry.capture === capture)) {
        list.push({ listener, capture });
      }
      this.__listeners.set(key, list);
    }

    removeEventListener(type, listener, options = false) {
      const capture = typeof options === "boolean" ? options : !!options.capture;
      const key = String(type);
      const list = this.__listeners.get(key);
      if (!list) return;
      const index = list.findIndex(entry => entry.listener === listener && !!entry.capture === capture);
      if (index !== -1) list.splice(index, 1);
    }

    dispatchEvent(event) {
      const dispatchEvent = event instanceof Event ? event : new Event(event);
      dispatchEvent.target = this;

      const path = [];
      let current = this;
      while (current) {
        path.push(current);
        current = current.parentNode;
      }

      for (let i = path.length - 1; i >= 1; i -= 1) {
        if (invokeListeners(path[i], dispatchEvent, true, 1)) {
          return false;
        }
      }

      if (invokeListeners(this, dispatchEvent, true, 2)) {
        return false;
      }
      if (invokeListeners(this, dispatchEvent, false, 2)) {
        return false;
      }

      if (dispatchEvent.bubbles) {
        for (let i = 1; i < path.length; i += 1) {
          if (invokeListeners(path[i], dispatchEvent, false, 3)) {
            return false;
          }
        }
      }

      return true;
    }

    get parentNode() {
      return wrapNode(__omoikane_parent_node(this.__id));
    }

    get nodeName() {
      return __omoikane_node_name(this.__id);
    }

    get id() {
      return __omoikane_get_attribute(this.__id, "id");
    }

    set id(value) {
      __omoikane_set_attribute(this.__id, "id", String(value));
    }

    getAttribute(name) {
      return __omoikane_get_attribute(this.__id, String(name));
    }

    setAttribute(name, value) {
      __omoikane_set_attribute(this.__id, String(name), String(value));
    }

    get className() {
      return __omoikane_get_attribute(this.__id, "class") || "";
    }

    set className(value) {
      __omoikane_set_attribute(this.__id, "class", String(value));
    }

    get classList() {
      const node = this;
      return {
        add(...classes) {
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          for (const cls of classes) current.add(cls);
          node.className = [...current].join(" ");
        },
        remove(...classes) {
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          for (const cls of classes) current.delete(cls);
          node.className = [...current].join(" ");
        },
        toggle(cls, force) {
          const current = new Set((node.className || "").split(/\s+/).filter(Boolean));
          const has = current.has(cls);
          if (force === undefined) {
            has ? current.delete(cls) : current.add(cls);
          } else if (force) {
            current.add(cls);
          } else {
            current.delete(cls);
          }
          node.className = [...current].join(" ");
          return current.has(cls);
        },
        contains(cls) {
          return (node.className || "").split(/\s+/).filter(Boolean).includes(cls);
        },
        get length() {
          return (node.className || "").split(/\s+/).filter(Boolean).length;
        },
        item(index) {
          return (node.className || "").split(/\s+/).filter(Boolean)[index] || null;
        },
      };
    }

    get style() {
      const node = this;
      return new Proxy({}, {
        get(target, prop) {
          if (typeof prop !== "string") return undefined;
          const kebab = prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
          const styleAttr = __omoikane_get_attribute(node.__id, "style") || "";
          const escaped = kebab.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
          const match = styleAttr.match(new RegExp("(?:^|;\\s*)" + escaped + "\\s*:\\s*([^;]+)"));
          return match ? match[1].trim() : "";
        },
        set(target, prop, value) {
          if (typeof prop !== "string") return true;
          const kebab = prop.replace(/[A-Z]/g, m => "-" + m.toLowerCase());
          const shouldRemove = value === null || value === undefined || value === "";
          const strValue = shouldRemove ? "" : String(value);
          let styleAttr = __omoikane_get_attribute(node.__id, "style") || "";
          const escaped = kebab.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
          const regex = new RegExp("(^|;\\s*)" + escaped + "\\s*:[^;]+(;|$)");
          if (regex.test(styleAttr)) {
            if (shouldRemove) {
              styleAttr = styleAttr.replace(regex, "$1");
            } else {
              styleAttr = styleAttr.replace(regex, (m, pre) => pre + kebab + ": " + strValue + ";");
            }
          } else if (!shouldRemove) {
            styleAttr = (styleAttr ? styleAttr.replace(/;?\s*$/, "; ") : "") + kebab + ": " + strValue + ";";
          }
          styleAttr = styleAttr.replace(/^[;\s]+/, "").replace(/[;\s]+$/, "").replace(/;\s*;+/g, ";");
          __omoikane_set_attribute(node.__id, "style", styleAttr.trim());
          return true;
        },
      });
    }
  }

  class Document extends Node {
    getElementById(id) {
      return wrapNode(__omoikane_get_element_by_id(String(id)));
    }

    createElement(tag) {
      return wrapNode(__omoikane_create_element(String(tag)));
    }
  }

  globalThis.Node = Node;
  globalThis.Document = Document;
  globalThis.Event = Event;
  globalThis.document = wrapNode(__omoikane_document_id);
  globalThis.window = globalThis;
  globalThis.location = { href: __omoikane_location_href };
  globalThis.navigator = { userAgent: __omoikane_navigator_user_agent };
  globalThis.console = {
    log: (...args) => __omoikane_console_log(...args),
  };
  globalThis.fetch = function(url) {
    return Promise.resolve(__omoikane_fetch(String(url))).then(raw => {
      const data = JSON.parse(String(raw));
      return {
        status: data.status,
        ok: data.ok,
        url: data.url,
        text() {
          return Promise.resolve(data.bodyText);
        },
      };
    });
  };
})();
"#;

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
}
