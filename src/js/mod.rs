//! JavaScript engine embedding and DOM/Web API bindings.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use boa_engine::native_function::NativeFunction;
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, Source, js_string};

use crate::css::{
    ComputedStyle, ComputedValue, Origin, Selector, StyleResolver, matches_selector,
    parse_selector_list,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::http::Client;
use crate::layout::{LayoutBox, Overflow, Rect};

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
    /// A connected iframe/object resource load, followed by `load` dispatch.
    ResourceLoad { node_id: usize },
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
        !self.timers.is_empty() || !self.macrotasks.is_empty()
    }
}

struct HostState {
    event_loop: EventLoopState,
    document: NodeHandle,
    nodes: HashMap<usize, NodeHandle>,
    console_logs: Vec<String>,
    location_href: String,
    navigator_user_agent: String,
    http_client: Client,
    /// Viewport used when resolving computed styles and running layout for the
    /// `getComputedStyle` / layout-metrics bindings (issues 016-8 and 044-2).
    viewport: Rect,
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
    fn new(document: NodeHandle) -> Self {
        // Seed the main document's style entry immediately so its identity is a
        // known key from the start; iframe sub-document entries are created when
        // their content document is first loaded (see `iframe_content_document`).
        let mut document_styles = HashMap::new();
        document_styles.insert(
            document.identity(),
            DocumentStyleEntry {
                resolver: None,
                dirty: true,
            },
        );
        let mut state = Self {
            event_loop: EventLoopState::default(),
            document: document.clone(),
            nodes: HashMap::new(),
            console_logs: Vec::new(),
            location_href: "http://localhost/".to_string(),
            navigator_user_agent: "Omoikane/0.1".to_string(),
            http_client: Client::new(),
            viewport: Rect {
                x: 0.0,
                y: 0.0,
                width: DEFAULT_VIEWPORT_WIDTH,
                height: DEFAULT_VIEWPORT_HEIGHT,
            },
            document_styles,
            layout_root: None,
            write_insertion_ref: None,
            base_url: None,
            iframe_documents: HashMap::new(),
            pending_resource_loads: HashSet::new(),
        };
        state.register_tree(&document);
        state
    }

    /// Queue loads for iframe and data-bearing object descendants when a
    /// detached subtree first becomes connected to a document.
    fn schedule_connected_resource_loads(&mut self, root: &NodeHandle) {
        fn visit(state: &mut HostState, node: &NodeHandle) {
            let tag = node.tag_name().unwrap_or_default();
            let is_resource = tag.eq_ignore_ascii_case("iframe")
                || (tag.eq_ignore_ascii_case("object")
                    && node
                        .attributes()
                        .is_some_and(|attrs| attrs.contains_key("data")));
            if is_resource && state.pending_resource_loads.insert(node.identity()) {
                state
                    .event_loop
                    .macrotasks
                    .push_back(TimerPayload::ResourceLoad {
                        node_id: node.identity(),
                    });
            }
            for child in node.child_nodes() {
                visit(state, &child);
            }
        }
        visit(self, root);
    }

    /// Returns the sub-browsing-context document for `iframe`, loading it on the
    /// first access and reloading it whenever the element's `src` changes.
    ///
    /// The returned document's whole node tree is registered so it can be
    /// traversed and mutated through the DOM primitives exactly like the
    /// top-level document. An empty or `about:blank` `src` yields an empty HTML
    /// skeleton (`<html><head></head><body></body></html>`); any other `src` is
    /// fetched and, only if it is served with an HTML content type, parsed as
    /// HTML (otherwise the sub-document stays an empty skeleton so non-HTML
    /// resources are never mined for markup).
    fn iframe_content_document(&mut self, iframe: &NodeHandle) -> NodeHandle {
        let src = iframe
            .attributes()
            .and_then(|attrs| attrs.get("src").cloned())
            .unwrap_or_default()
            .trim()
            .to_string();
        let iframe_id = iframe.identity();

        if let Some(entry) = self.iframe_documents.get(&iframe_id) {
            if entry.loaded_src == src {
                return entry.document.clone();
            }
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
            self.unregister_tree(&previous.document);
        }

        let document = self.load_iframe_document(&src);
        self.register_tree(&document);
        // Seed a dirty style cache entry for the freshly loaded sub-document so
        // its resolver is built from its own `<style>` rules on first query.
        self.document_styles.insert(
            document.identity(),
            DocumentStyleEntry {
                resolver: None,
                dirty: true,
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

    /// Fetches and constructs the sub-document for an iframe `src`.
    ///
    /// Returns an `about:blank` skeleton for an empty/`about:blank` `src`, for a
    /// fetch failure, and for any resource whose content type is not an HTML
    /// type. Only HTML resources (`text/html`, `application/xhtml+xml`) are
    /// parsed into a real DOM tree.
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
                Some(ResolvedResource::Url(url)) => {
                    self.http_client.get(&url).ok().map(|resp| {
                        let mime = resp.header("Content-Type").unwrap_or("").to_string();
                        (mime, resp.body().to_vec())
                    })
                }
                None => None,
            };

        match fetched {
            Some((mime, body)) if is_html_mime_type(&mime) => {
                let html = String::from_utf8_lossy(&body);
                crate::html::TreeBuilder::parse(&html).document()
            }
            // Non-HTML content types (image/png, text/plain, XML, SVG, ...) are
            // never parsed as HTML: the sub-document stays an empty skeleton so
            // a page cannot mine markup out of a non-HTML resource. Acid3 tests
            // 14 and 15 depend on this (a PNG/text file must not yield a <p>).
            _ => blank_html_document(),
        }
    }

    fn register_tree(&mut self, node: &NodeHandle) {
        self.nodes.insert(node.identity(), node.clone());
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
            self.layout_root = None;
        }
    }

    /// Marks the document that `node` currently lives in as stale.
    ///
    /// If `node` has no live document root (a detached, freshly created node),
    /// every cached document is conservatively marked dirty: the node may later
    /// be inserted into any document, and over-invalidating only costs a rebuild
    /// while under-invalidating would serve stale styles.
    fn mark_style_dirty_for_node(&mut self, node: &NodeHandle) {
        match document_root_for_node(node) {
            Some(document) => self.mark_document_style_dirty(&document),
            None => self.mark_all_document_styles_dirty(),
        }
        // The iframe element belongs to the parent document, but its rendered
        // content-box establishes the child document's viewport. A width/height
        // style or attribute mutation must therefore invalidate both caches.
        if node.tag_name().as_deref() == Some("iframe") {
            if let Some(child) = self.iframe_documents.get(&node.identity()) {
                let child_document = child.document.clone();
                self.mark_document_style_dirty(&child_document);
            }
        }
    }

    /// Marks every cached document's style resolver as stale and drops the main
    /// document's layout tree.
    ///
    /// Used when a change affects all documents at once — a viewport change
    /// (every resolver shares the same viewport for `vw`/`vh` resolution) — and
    /// as the conservative fallback when a mutated node's owning document cannot
    /// be determined.
    fn mark_all_document_styles_dirty(&mut self) {
        for entry in self.document_styles.values_mut() {
            entry.dirty = true;
        }
        self.layout_root = None;
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
            return;
        }
        // Build the resolver as a local first so no `document_styles` borrow is
        // held while `self.viewport` / the document tree are read, then store it.
        let viewport = self.viewport_for_document(document);
        let mut resolver = StyleResolver::new();
        resolver.set_viewport(viewport.width, viewport.height);
        for css in collect_inline_stylesheets(document) {
            let sheet = crate::paint::stylesheet::parse_stylesheet_forgiving(&css);
            resolver.add_stylesheet(Origin::Author, sheet);
        }
        self.document_styles.insert(
            document_id,
            DocumentStyleEntry {
                resolver: Some(resolver),
                dirty: false,
            },
        );
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
            attrs.get(name)
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
        self.layout_root = layout;
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
    while let Some(parent) = current.parent_node() {
        current = parent;
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
        // Parsed resource elements are already connected before the JS wrapper
        // exists. Queue their loads only after bootstrap so dispatch can wrap
        // the target element when the macrotask runs.
        runtime
            .host_state
            .borrow_mut()
            .schedule_connected_resource_loads(&document);
        Ok(runtime)
    }

    /// Returns the current DOM document.
    pub fn document(&self) -> NodeHandle {
        self.host_state.borrow().document.clone()
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
             globalThis.screen.availWidth = {w}; globalThis.screen.availHeight = {h}; }}"
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
            TimerPayload::ResourceLoad { node_id } => {
                let should_dispatch = {
                    let mut state = self.host_state.borrow_mut();
                    state.pending_resource_loads.remove(&node_id);
                    let Some(node) = state.get_node(node_id) else {
                        return Ok(());
                    };
                    if document_root_for_node(&node).is_none() {
                        false
                    } else {
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
                            state.iframe_content_document(&node);
                            // JS wrappers are cached by native identity, so keep
                            // the old Rc alive until its replacement has been
                            // allocated and cannot reuse the same address.
                            drop(previous);
                        }
                        true
                    }
                };
                if should_dispatch {
                    self.eval(&format!("__omoikane_dispatch_resource_load({node_id})"))?;
                    self.run_jobs()?;
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
        // Record the base URL so relative resource references discovered later
        // (e.g. an `<iframe src="empty.html">` whose contentDocument is accessed
        // during the timer loop) can be resolved.
        if let Some(base) = base_url {
            self.host_state.borrow_mut().base_url = Some(base.clone());
        }

        let document = self.document();
        let scripts = collect_script_elements(&document);
        let mut errors = Vec::new();
        let mut deferred = Vec::new();

        for script in &scripts {
            let attrs = script.attributes().unwrap_or_default();

            // Skip <script> types Omoikane does not run as classic scripts
            // (`type="module"` and non-JavaScript types). Shares the type gate
            // with `is_inline_classic_script` so a script executes identically
            // whether it was parsed normally or inserted via `document.write`.
            if !is_executable_classic_script_type(attrs.get("type").map(|s| s.as_str())) {
                continue;
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
            let mut client = Client::new();
            let response = client.get(&url).ok()?;
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
    drop(state);

    for (name, length, function) in [
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
            js_string!("__omoikane_layout_metrics"),
            1,
            NativeFunction::from_copy_closure(layout_metrics_native),
        ),
        (
            js_string!("__omoikane_validate_inline_css"),
            2,
            NativeFunction::from_copy_closure(validate_inline_css_native),
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
    matches!(essence.as_str(), "text/html" | "application/xhtml+xml")
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

// ---------------------------------------------------------------------------
// Computed style + layout metrics (issues 016-8, 044-2)
// ---------------------------------------------------------------------------

/// Recursively collects the text content of every inline `<style>` element in
/// the document tree, returning one CSS string per `<style>` element.
///
/// Only inline styles are gathered; linked stylesheets are not fetched here to
/// keep computed-style resolution synchronous and side-effect free.
fn collect_inline_stylesheets(document: &NodeHandle) -> Vec<String> {
    fn walk(node: &NodeHandle, out: &mut Vec<String>) {
        if node.node_type() == NodeType::Element
            && node
                .tag_name()
                .as_deref()
                .is_some_and(|tag| tag.eq_ignore_ascii_case("style"))
        {
            let css = collect_text_recursive(node);
            if !css.trim().is_empty() {
                out.push(css);
            }
        }
        for child in node.child_nodes() {
            walk(&child, out);
        }
    }
    let mut out = Vec::new();
    walk(document, &mut out);
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
        ComputedValue::Color(color) => color.clone(),
        ComputedValue::String(string) => string.clone(),
        ComputedValue::Px(px) => format!("{}px", format_css_number(*px)),
        ComputedValue::Percentage(pct) => format!("{}%", format_css_number(*pct)),
        ComputedValue::Number(number) => format_css_number(*number),
        ComputedValue::CalcPxPercent(px, pct) => {
            format!("calc({}px + {}%)", format_css_number(*px), format_css_number(*pct))
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

/// The eight geometry values `getBoundingClientRect()` exposes plus the derived
/// `offset*` / `client*` / `scroll*` metrics, all in CSS pixels.
struct LayoutMetrics {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
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
\"offsetWidth\":{ow},\"offsetHeight\":{oh},\"offsetTop\":{ot},\"offsetLeft\":{ol},\
\"clientWidth\":{cw},\"clientHeight\":{ch},\"clientTop\":{ct},\"clientLeft\":{cl},\
\"scrollWidth\":{sw},\"scrollHeight\":{sh},\"scrollTop\":0,\"scrollLeft\":0,\
\"hasBox\":{has_box}}}",
            x = json_number(self.x),
            y = json_number(self.y),
            w = json_number(self.width),
            h = json_number(self.height),
            right = json_number(self.x + self.width),
            bottom = json_number(self.y + self.height),
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
        )
    }
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
///   border boxes of every overflowing descendant (not just the direct
///   children). Traversal stops at any descendant that clips its overflow
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

    // scrollWidth/scrollHeight: the padding box grown to contain descendants.
    // Layout coordinates are absolute (see `layout_document`/`layout_element`),
    // so descendant border-box edges are comparable directly with this box's
    // padding-box edges without accumulating per-level offsets.
    let padding_right_edge = content.x + content.width + padding.right;
    let padding_bottom_edge = content.y + content.height + padding.bottom;
    let padding_left_edge = content.x - padding.left;
    let padding_top_edge = content.y - padding.top;
    let mut max_right = padding_right_edge;
    let mut max_bottom = padding_bottom_edge;
    expand_scroll_bounds(&layout.children, &mut max_right, &mut max_bottom);
    let scroll_width = (max_right - padding_left_edge).max(client_width);
    let scroll_height = (max_bottom - padding_top_edge).max(client_height);

    LayoutMetrics {
        x: border_x,
        y: border_y,
        width: border_width,
        height: border_height,
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
        has_box: true,
    }
}

/// Expands `max_right`/`max_bottom` to enclose the border boxes of every box in
/// `boxes` and (recursively) their descendants, for `scrollWidth`/`scrollHeight`.
///
/// Layout coordinates are absolute, so each descendant's border-box right/bottom
/// edge is compared directly against the running maxima — no per-level offset
/// accumulation is required. Traversal does not descend into a box that clips its
/// overflow ([`Overflow::Hidden`]): the clipping box still contributes its own
/// border box, but its clipped content is not part of an ancestor's scrollable
/// area, matching how a scroll container establishes its own scroll region.
fn expand_scroll_bounds(boxes: &[LayoutBox], max_right: &mut f32, max_bottom: &mut f32) {
    for child in boxes {
        let child_content = child.dimensions.content;
        let child_padding = child.dimensions.padding;
        let child_border = child.dimensions.border;
        let child_right =
            child_content.x + child_content.width + child_padding.right + child_border.right;
        let child_bottom =
            child_content.y + child_content.height + child_padding.bottom + child_border.bottom;
        *max_right = max_right.max(child_right);
        *max_bottom = max_bottom.max(child_bottom);
        // A descendant that clips its overflow bounds its own subtree; deeper
        // content cannot spill into this element's scrollable area.
        if child.overflow == Overflow::Visible {
            expand_scroll_bounds(&child.children, max_right, max_bottom);
        }
    }
}

/// `__omoikane_computed_style(nodeId)` -> JSON string of computed CSS
/// properties (kebab-case name to CSS string value). Forces a synchronous
/// style recompute if the DOM changed since the last query.
fn computed_style_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
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

/// `__omoikane_layout_metrics(nodeId)` -> JSON string of geometry metrics for
/// the element (see [`compute_layout_metrics`]). Forces a synchronous reflow if
/// the DOM changed since the last query. Elements that produce no box (e.g.
/// `display: none`) report all-zero metrics.
fn layout_metrics_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node_id = parse_node_id(args.first(), context)?;
    with_host_state(|state| {
        let node = state.borrow().get_node(node_id);
        let Some(node) = node else {
            return Ok(js_string!(LayoutMetrics::zero().to_json().as_str()).into());
        };
        let mut state = state.borrow_mut();
        state.ensure_layout();
        let metrics = state
            .layout_root
            .as_ref()
            .and_then(|root| find_layout_box(root, &node))
            .map(compute_layout_metrics)
            .unwrap_or_else(LayoutMetrics::zero);
        Ok(js_string!(metrics.to_json().as_str()).into())
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
        state.borrow_mut().nodes.insert(id, node);
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
                state.schedule_connected_resource_loads(&child);
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
fn owner_document_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
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

/// `__omoikane_validate_inline_css(name, value)` -> the value string to apply
/// for an inline-style declaration, or `null` when the declaration is invalid
/// and must be dropped (so the cascaded value is retained). This keeps the
/// `getComputedStyle` inline override in step with the cascade's per-property
/// value validation (issue 051): validated properties (e.g. `cursor`) are
/// checked and normalized; every other property echoes its raw value unchanged.
fn validate_inline_css_native(
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
    let value = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    Ok(
        match crate::css::style::validate_inline_declaration(&name, &value) {
            crate::css::style::InlineDeclarationValidation::Unvalidated => {
                js_string!(value.as_str()).into()
            }
            crate::css::style::InlineDeclarationValidation::Valid(normalized) => {
                js_string!(normalized.as_str()).into()
            }
            crate::css::style::InlineDeclarationValidation::Invalid => JsValue::null(),
        },
    )
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
        // Any attribute may participate in a selector (id/class/attribute
        // selectors), so invalidate the element's document unconditionally. A
        // detached element falls back to invalidating every document.
        state.borrow_mut().mark_style_dirty_for_node(&node);
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
        state.borrow_mut().mark_style_dirty_for_node(&node);
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
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
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
        // The node stays attached to its document (only its text changes), so
        // its document root is unchanged; invalidate that document's resolver.
        // This covers the common `<style>.textContent = "..."` case, where the
        // stylesheet text of the owning document must be re-collected.
        state.borrow_mut().mark_style_dirty_for_node(&node);
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

fn set_inner_html_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let html = args.get(1).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    with_host_state(|state| {
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
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
        // The node stays attached to its document; invalidate that document's
        // resolver so a `<style>` inside the new markup is picked up.
        state.borrow_mut().mark_style_dirty_for_node(&node);
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
        let (parent, child) = {
            let state = state.borrow();
            let parent = state.get_node(parent_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("parent not found")))?;
            let child = state.get_node(child_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("child not found")))?;
            (parent, child)
        };
        // Save the parent's document *before* removing `child`: afterwards the
        // detached child has no document root, so the affected document could no
        // longer be found from it. The parent keeps its place in the tree.
        let parent_document = document_root_for_node(&parent);
        parent.remove_child(&child).map_err(|e| JsError::from(JsNativeError::error().with_message(e.to_string())))?;
        if let Some(document) = &parent_document {
            state.borrow_mut().mark_document_style_dirty(document);
        }
        Ok(JsValue::undefined())
    })
}

fn insert_before_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let new_id = args.get(1).cloned().unwrap_or_default().to_number(context)? as usize;
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
            let parent = state.get_node(parent_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("parent not found")))?;
            let new_node = state.get_node(new_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("new node not found")))?;
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
                state.schedule_connected_resource_loads(&new_node);
            }
        }
        Ok(JsValue::undefined())
    })
}

fn query_selector_all_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let parent_id = args.first().cloned().unwrap_or_default().to_number(context)? as usize;
    let selector = args.get(1).cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
    let selectors = parse_dom_selector_list(&selector)?;
    with_host_state(|state| {
        let results = {
            let s = state.borrow();
            let parent = s.get_node(parent_id).ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
            query_all_matching_descendants(&parent, &selectors)
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

fn parse_dom_selector_list(selector: &str) -> JsResult<Vec<Selector>> {
    parse_selector_list(selector).map_err(|error| {
        JsError::from(JsNativeError::syntax().with_message(error.to_string()))
    })
}

fn query_first_matching_descendant(node: &NodeHandle, selectors: &[Selector]) -> Option<NodeHandle> {
    for child in node.child_nodes() {
        if child.node_type() == NodeType::Element
            && selectors.iter().any(|selector| matches_selector(&child, selector))
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
            && selectors.iter().any(|selector| matches_selector(&child, selector))
        {
            results.push(child.clone());
        }
        results.extend(query_all_matching_descendants(&child, selectors));
    }
    results
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
            NodeHandle::document_type(
                node.data().unwrap_or_default(),
                node.public_id().unwrap_or_default(),
                node.system_id().unwrap_or_default(),
            )
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
        let node = state
            .borrow()
            .get_node(id)
            .ok_or_else(|| JsError::from(JsNativeError::error().with_message("node not found")))?;
        node.remove_attribute(&name);
        // Any attribute may participate in a selector, so invalidate the
        // element's document (or every document if it is detached).
        state.borrow_mut().mark_style_dirty_for_node(&node);
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
/// `text/ecmascript` — are treated as non-executable.
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
            let mime = t.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            mime.is_empty() || mime == "text/javascript" || mime == "application/javascript"
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
    if attrs.get("src").is_some() {
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
        let (parent, reference_child, is_main, target_doc) = {
            let s = state.borrow();
            // The document being written to. `document.write` passes its id; an
            // unresolved id falls back to the top-level document.
            let target_doc = s
                .get_node(target_id)
                .unwrap_or_else(|| s.document.clone());
            let is_main = target_doc == s.document;
            // Fallback target when there is no active insertion point: the
            // target document's <body>, or — after document.open() emptied it so
            // no <body> exists — the document node itself, so the written nodes
            // become the (fresh) document's children.
            let fallback_parent = || {
                target_doc
                    .query_selector("body")
                    .unwrap_or_else(|| target_doc.clone())
            };
            let (parent, reference_child) = if is_main {
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
                // sub-document's <body> (or the document node itself right after
                // document.open() emptied it). The main document's insertion
                // point is left untouched.
                (Some(fallback_parent()), None)
            };
            (parent, reference_child, is_main, target_doc)
        };

        let Some(parent) = parent else {
            // No insertion target at all; nothing sensible to do.
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
                s.schedule_connected_resource_loads(child);
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
            if is_main {
                if let Some(last) = last_inserted {
                    if s.write_insertion_ref.is_some() {
                        s.write_insertion_ref = Some(last);
                    }
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

        assert_eq!(result, "0,0", "unreachable traversal objects must be swept from the registry");
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
        runtime
            .eval(
                r#"
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
            eval_str(&mut runtime, "document.querySelector('div').style.backgroundColor"),
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
            eval_str(&mut runtime, "document.querySelector('div').style.getPropertyValue('color')"),
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
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
            .eval(
                "document.getElementById('target').style.setProperty('white-space', 'pre-wrap')",
            )
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
        let style_attr = body.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
        let style_attr = div.attributes().unwrap().get("style").cloned().unwrap_or_default();
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
    fn get_element_by_id_does_not_parse_id_as_a_selector() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse(r#"<html><body><div id="plain"></div></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert!(runtime
            .eval("document.getElementById('plain\\0suffix') === null")
            .unwrap()
            .as_boolean()
            .unwrap());
        assert!(runtime
            .eval("document.getElementById('plain').id === 'plain'")
            .unwrap()
            .as_boolean()
            .unwrap());
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
                  return [text.checked, div.checked, box.checked,
                          document.querySelector(":checked") === box].join("|");
                })()
                "#,
            )
            .unwrap()
            .to_string(&mut runtime.context)
            .unwrap()
            .to_std_string_escaped();
        assert_eq!(result, "false|false|true|true");
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
        let doc = TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#)
            .document();
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
            "blue",
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
            eval_str(&mut runtime, "getComputedStyle(globalThis.__t, '').whiteSpace"),
            "pre-wrap",
            "the <style> rule applies before document.open()"
        );

        // Empty the document via document.open(); the <style> is now gone.
        runtime.eval("document.open();").unwrap();

        assert_ne!(
            eval_str(&mut runtime, "getComputedStyle(globalThis.__t, '').whiteSpace"),
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

        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().width"), 100.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().height"), 50.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().left"), 0.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().top"), 0.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().right"), 100.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().bottom"), 50.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').offsetWidth"), 100.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').offsetHeight"), 50.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').offsetLeft"), 0.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').offsetTop"), 0.0);
    }

    #[test]
    fn client_metrics_account_for_padding_and_border() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 50px; padding: 10px; border: 5px solid black; }
        </style></head><body><div id="box"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        // clientWidth/Height = content + padding (no border).
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').clientWidth"), 120.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').clientHeight"), 70.0);
        // offsetWidth/Height = content + padding + border.
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').offsetWidth"), 130.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').offsetHeight"), 80.0);
        // clientTop/Left = border widths.
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').clientTop"), 5.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').clientLeft"), 5.0);
        // Border box starts at the origin.
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().left"), 0.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').getBoundingClientRect().top"), 0.0);
    }

    #[test]
    fn layout_metrics_force_reflow_after_class_change() {
        let html = r#"<html><head><style>
            * { margin: 0; padding: 0; }
            #box { width: 100px; height: 50px; }
            #box.wide { width: 250px; }
        </style></head><body><div id="box"></div></body></html>"#;
        let mut runtime = runtime_from_html(html);

        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').offsetWidth"), 100.0);

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
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').clientHeight"), 50.0);
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('box').scrollHeight"),
            200.0,
            "scrollHeight must enclose the overflowing child"
        );
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
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').clientWidth"), 100.0);
        assert_eq!(eval_num(&mut runtime, "document.getElementById('box').clientHeight"), 100.0);
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
            eval_num(&mut runtime, "document.getElementById('zero').getClientRects().length"),
            1.0,
            "a zero-sized rendered box must still return one client rect"
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('gone').getClientRects().length"),
            0.0,
            "an element that generates no box must return an empty client-rect list"
        );
        // The zero-sized box's single rect reports zero width/height.
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('zero').getClientRects()[0].width"),
            0.0
        );
        assert_eq!(
            eval_num(&mut runtime, "document.getElementById('zero').getClientRects()[0].height"),
            0.0
        );
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

    // ── DOM Traversal / Range ───────────────────────────────────────────────

    #[test]
    fn node_iterator_honors_mask_filter_exceptions_and_live_removal() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><div id='r'>a<i>b</i><b>c</b></div></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{
          var r=document.getElementById('r'), seen=[];
          var it=document.createNodeIterator(r, NodeFilter.SHOW_ELEMENT, function(n) {
            if (n.tagName === 'I') return NodeFilter.FILTER_REJECT;
            return NodeFilter.FILTER_ACCEPT;
          });
          var n; while(n=it.nextNode()) seen.push(n.tagName);
          return seen.join(',')})()"#), "DIV,B");
        assert_eq!(eval_str(&mut runtime, r#"(()=>{
          var r=document.getElementById('r');
          try { document.createNodeIterator(r, NodeFilter.SHOW_ALL, function(){throw 'filter-error'}).nextNode(); return 'miss' }
          catch(e) { return String(e) }})()"#), "filter-error");
        assert_eq!(eval_str(&mut runtime, r#"(()=>{
          var x=document.createElement('div'); var a=document.createElement('a'); var b=document.createElement('b');
          x.appendChild(a); x.appendChild(b); var live=document.createNodeIterator(x); live.nextNode(); live.nextNode();
          x.removeChild(a); return live.nextNode().tagName})()"#), "B");
    }

    #[test]
    fn tree_walker_distinguishes_reject_from_skip_and_stays_in_root() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><div id='r'><section><i></i></section><b></b></div></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{
          var r=document.getElementById('r');
          var w=document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT, function(n) {
            return n.tagName==='SECTION' ? NodeFilter.FILTER_SKIP : NodeFilter.FILTER_ACCEPT;
          }); var out=[],n; while(n=w.nextNode()) out.push(n.tagName); return out.join(',')})()"#), "I,B");
        assert_eq!(eval_str(&mut runtime, r#"(()=>{
          var r=document.getElementById('r'); var w=document.createTreeWalker(r, NodeFilter.SHOW_ELEMENT, function(n) {
            return n.tagName==='SECTION' ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT;
          }); var out=[],n; while(n=w.nextNode()) out.push(n.tagName); return out.join(',')})()"#), "B");
    }

    #[test]
    fn range_boundaries_clone_string_and_legacy_exception_are_explicit() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><p id='p'>Hello <em>World</em>!</p></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var p=document.getElementById('p'),r=document.createRange();
          r.selectNodeContents(p); var c=r.cloneContents();
          return [r.toString(),c.nodeType,c.childNodes.length,r.collapsed,r.commonAncestorContainer===p].join('|')})()"#), "Hello World!|11|3|false|true");
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var p=document.getElementById('p'),r=document.createRange();
          r.setStart(p.firstChild,2); r.setEnd(p.firstChild,5); return [r.toString(),r.cloneRange().toString()].join('|')})()"#), "llo|llo");
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var r=document.createRange();try{r.setEndBefore(document);return 'none'}catch(e){return e.name+'|'+e.code+'|'+e.INVALID_NODE_TYPE_ERR}})()"#), "InvalidNodeTypeError|24|24");
    }

    #[test]
    fn range_extract_returns_fragment_with_partial_ancestor_clones() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><h1>Hello <em>Wonderful</em> Kitty</h1><p>How are you?</p></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var h=document.querySelector('h1'),em=document.querySelector('em'),p=document.querySelector('p');
          var r=document.createRange();r.setStart(em.firstChild,6);r.setEnd(p,0);var f=r.extractContents();
          return [f.nodeType,f.childNodes.length,f.firstChild.tagName,f.firstChild.firstChild.tagName,f.firstChild.firstChild.textContent,f.firstChild.lastChild.textContent,f.lastChild.tagName,p.childNodes.length].join('|')})()"#), "11|2|H1|EM|ful| Kitty|P|1");
    }

    #[test]
    fn range_insert_splits_text_and_keeps_inserted_node_in_selection() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><p id='p'>12345</p></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var p=document.getElementById('p'),t1=p.firstChild,t2=document.createTextNode('ABCDE');p.appendChild(t2);
          var r=document.createRange();r.setStart(t1,2);r.setEnd(t1,3);r.insertNode(t2);
          return [p.childNodes.length,p.childNodes[0].data,p.childNodes[1].data,p.childNodes[2].data,r.toString()].join('|')})()"#), "3|12|ABCDE|345|ABCDE3");
    }

    #[test]
    fn range_live_boundaries_follow_removed_subtree() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body><p id='p'>12345</p></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var p=document.getElementById('p'),b=document.body,r=document.createRange();
          r.setEnd(b,1);r.setStart(p.firstChild,2);b.removeChild(p);
          return [r.collapsed,r.startContainer===b,r.startOffset,r.endContainer===b,r.endOffset].join('|')})()"#), "true|true|0|true|0");
    }

    #[test]
    fn range_surround_reports_hierarchy_and_partial_character_data_errors() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><body></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var c=document.createComment('11111');document.appendChild(c);var r=document.createRange();r.selectNode(c);
          try{r.surroundContents(document.createElement('a'));return 'none'}catch(e){document.removeChild(c);return e.name+'|'+e.code}})()"#), "HierarchyRequestError|3");
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var b=document.body,c1=document.createComment('111'),c2=document.createComment('222');b.appendChild(c1);b.appendChild(c2);
          var r=document.createRange();r.setStart(c1,1);r.setEnd(c2,1);try{r.surroundContents(document.createElement('a'));return 'none'}catch(e){return e.name+'|'+e.code}})()"#), "InvalidStateError|11");
    }

    #[test]
    fn acid3_traversal_filter_mutation_and_tree_regrafting_regressions() {
        use crate::html::TreeBuilder;
        let doc = TreeBuilder::parse("<html><head><title></title></head><body></body></html>").document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var b=document.body;for(var k=0;k<5;k++){var s=document.createElement('section');s.title=k;b.appendChild(s)}
          var count=0,it=document.createNodeIterator(b,0xffffffff,function(){if(count>3&&count<12)b.appendChild(b.firstChild);count++;return count%2===0?1:2});
          var out=[],n;while(n=it.nextNode())out.push(n.title);return out.join(',')})()"#), "0,2,4,1,3,0,2");
        assert_eq!(eval_str(&mut runtime, r#"(()=>{var b=document.body;b.textContent='';var p=document.createElement('p');b.appendChild(p);var w=document.createTreeWalker(b);
          w.lastChild();w.previousNode();document.documentElement.removeChild(b);var a=w.lastChild()===p,z=w.nextNode()===null;
          document.documentElement.appendChild(p);var title=w.previousNode();p.appendChild(b);return [a,z,title.tagName,w.nextNode()===p,w.nextNode()===b,w.previousNode()===null].join('|')})()"#), "true|true|TITLE|true|true|true");
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
        runtime.tick(0).expect("zero-delay resource tasks should run");
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

        assert_eq!(runtime.eval("propertyLoads + attributeLoads + listenerLoads").unwrap().as_number(), Some(0.0));
        pump_zero_delay_tasks(&mut runtime);
        assert_eq!(runtime.eval("propertyLoads").unwrap().as_number(), Some(1.0));
        assert_eq!(runtime.eval("attributeLoads").unwrap().as_number(), Some(1.0));
        assert_eq!(runtime.eval("listenerLoads").unwrap().as_number(), Some(1.0));
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

    #[test]
    fn missing_title_reflects_as_empty_string() {
        let mut runtime = JsRuntime::new().unwrap();
        assert_eq!(eval_string_value(&mut runtime, "document.createElement('p').title").as_deref(), Some(""));
    }

    #[test]
    fn is_html_mime_type_matches_html_essences_only() {
        assert!(is_html_mime_type("text/html"));
        assert!(is_html_mime_type("text/html; charset=utf-8"));
        assert!(is_html_mime_type("APPLICATION/XHTML+XML"));
        assert!(!is_html_mime_type("image/png"));
        assert!(!is_html_mime_type("text/plain; charset=utf-8"));
        assert!(!is_html_mime_type("application/xml"));
        assert!(!is_html_mime_type("image/svg+xml"));
        assert!(!is_html_mime_type(""));
    }

    #[test]
    fn blank_html_document_has_html_head_and_body_but_no_content() {
        let doc = blank_html_document();
        assert_eq!(doc.node_type(), crate::dom::NodeType::Document);
        assert_eq!(
            doc.query_selector("html").and_then(|h| h.tag_name()).as_deref(),
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
                .eval("document.getElementById('f').contentDocument.getElementsByTagName('p').length")
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
                .eval("document.getElementById('f').contentDocument.getElementsByTagName('i').length")
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
                .eval("document.getElementById('f').contentDocument.getElementsByTagName('p').length")
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

    /// A resource served as `image/png` must never be parsed as HTML, even when
    /// its bytes look like markup (Acid3 test 14).
    #[test]
    fn iframe_png_src_is_not_parsed_as_html() {
        use crate::html::TreeBuilder;
        let port = spawn_static_http_server("image/png", r#"<html><body><p>FAIL</p></body></html>"#);
        let doc = TreeBuilder::parse(&format!(
            r#"<html><body><iframe id="f" src="http://127.0.0.1:{port}/empty.png"></iframe></body></html>"#
        ))
        .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        assert_eq!(
            runtime
                .eval("document.getElementById('f').contentDocument.getElementsByTagName('p').length")
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
                .eval("document.getElementById('f').contentDocument.getElementsByTagName('p').length")
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
        let port = spawn_static_http_server("text/html", r#"<html><body><p id="rel">yes</p></body></html>"#);
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f" src="sub.html"></iframe></body></html>"#)
                .document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();
        let base: crate::http::Url = format!("http://127.0.0.1:{port}/index.html").parse().unwrap();
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

    /// Changing an iframe's `src` reloads its document on the next access.
    #[test]
    fn iframe_changing_src_reloads_content_document() {
        use crate::html::TreeBuilder;
        let port =
            spawn_static_http_server("text/html", r#"<html><body><p id="x">loaded</p></body></html>"#);
        let doc =
            TreeBuilder::parse(r#"<html><body><iframe id="f"></iframe></body></html>"#).document();
        let mut runtime = JsRuntime::with_document(doc).unwrap();

        // No src yet: an empty about:blank skeleton with no <p>.
        assert_eq!(
            runtime
                .eval("document.getElementById('f').contentDocument.getElementsByTagName('p').length")
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
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
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"), "1");
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#t { z-index: 6; position: absolute; }';
                   d.body.appendChild(s);
                   var t = d.createElement('div'); t.id = 't'; d.body.appendChild(t);"#,
            )
            .unwrap();

        assert_eq!(eval_str(&mut runtime, "getComputedStyle(t, '').zIndex"), "6");

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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
        runtime
            .eval(
                r#"var d = document.getElementById('f').contentDocument;
                   var s = d.createElement('style');
                   s.textContent = '#old { z-index: 1; position: absolute; }';
                   d.body.appendChild(s);
                   var o = d.createElement('div'); o.id = 'old'; d.body.appendChild(o);"#,
            )
            .unwrap();
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(o, '').zIndex"), "1");

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
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(subT, '').zIndex"), "");

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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
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
        assert_eq!(eval_str(&mut runtime, "getComputedStyle(t, '').width"), "400px");

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
            .eval(
                "document.getElementById('f').setAttribute('class', 'large')",
            )
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
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
            .eval(
                r#"host.innerHTML = '<style>#t { z-index: 8; position: absolute; }</style>';"#,
            )
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="f"></iframe></body></html>"#,
        );
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="selectors"></iframe></body></html>"#,
        );
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
        let mut runtime = runtime_from_html(
            r#"<html><body><iframe id="selectors"></iframe></body></html>"#,
        );
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
        assert_eq!(
            result,
            "1,1,2;1,0,1;1,0;1,1,1,2,0;1,2,0,3,2,0;1,4,3,2,0"
        );
    }
}
