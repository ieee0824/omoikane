//! Block layout primitives.
//!
//! The layout phase consumes DOM nodes together with computed styles and
//! produces a tree of rectangular block boxes.

pub(crate) mod frameset;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use crate::css::style::PropertyId;
use crate::css::{
    AffineTransform, ComputedDirection, ComputedFloat, ComputedPosition, ComputedStyle,
    ComputedValue, ComputedWritingMode, ContainerContext, CssToken, Matrix4, PseudoElement,
    StyleResolver, TransformReferenceBox, parse_composed_transform, parse_perspective_matrix,
    tokenize,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::error_reporting::{
    ErrorCategory, ErrorCode, ErrorReporter, ErrorSeverity, ExecutionSurface, RawEvent,
};
use crate::font::{
    Font, FontFamilyKey, FontStyle, FontVariantKey, FontWeight, LayoutFontMetrics, WebFontRegistry,
};
use crate::http::corp::EmbedderPolicyValue;
use crate::http::cors::{CorpViolation, Origin as CorsOrigin};
use crate::http::{Client, CookieJar, Url};
use crate::paint::Image;
use rusqlite::{Connection, params};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_segmentation::UnicodeSegmentation;
use unicode_vo::{Orientation, char_orientation};

mod counters;
mod flex;
mod grid;
mod inline;
mod intrinsic;
mod margins;
mod multicol;
mod paged;
mod profile;
mod shapes;
mod table;

pub use paged::{
    PageContentFragment, PageContinuation, PageMarginFragment, PagedLayout, PagedPage,
    layout_paged_tree,
};

use flex::{is_flex_container, layout_flex_container};
use grid::{is_grid_container, layout_grid_container};
use inline::{
    InlineSegmentContent, font_metrics, generated_inline_segments, layout_inline_nodes,
    layout_inline_nodes_with_line_constraints, layout_vertical_inline_nodes, line_height,
    measure_text_width, normalize_text, resolve_image_rendered_size, text_align, vertical_align,
    white_space,
};
use table::{
    collect_table_entries, is_table_container_element, layout_table_container, table_border_spacing,
};

pub(crate) use inline::element_inline_image;
pub(crate) use inline::{
    canonical_image_asset_reference, decode_or_fetch_image_asset,
    decode_or_fetch_image_asset_with_pending_dependencies,
};
mod image_security;
pub(crate) use image_security::{
    ImageDocumentState, ImagePolicyNotice, poll_async_image_effects, poll_async_images,
    with_image_layout_dependency_capture, with_image_security_policy,
};

// Re-exports used by tests
#[cfg(test)]
pub(crate) use crate::paint::{DataUri, parse_data_uri};
#[cfg(test)]
pub(crate) use inline::split_chars;
#[cfg(test)]
pub(crate) use inline::split_words_no_cjk_break;
#[cfg(test)]
pub(crate) use inline::split_words_preserving_spaces_cjk;

type ImageCacheOwner = (
    Arc<Mutex<CookieJar>>,
    crate::http::CookieRequestContext,
    usize,
    CorsOrigin,
    EmbedderPolicyValue,
    EmbedderPolicyValue,
);

// Thread-local cache for fetched images and fonts to avoid redundant loads
thread_local! {
    static IMAGE_CACHE: RefCell<HashMap<String, Option<Image>>> = RefCell::new(HashMap::new());
    static IMAGE_ANIMATION_CACHE: RefCell<HashMap<String, Arc<crate::paint::ImageAnimation>>> = RefCell::new(HashMap::new());
    static IMAGE_ANIMATION_TIME_MS: Cell<u64> = const { Cell::new(0) };
    static HTTP_CLIENT: RefCell<Client> = RefCell::new(Client::new());
    static LAYOUT_FONTS: RefCell<Option<LayoutFontContext>> = const { RefCell::new(None) };
    static IMAGE_BASE_URL: RefCell<Option<Url>> = const { RefCell::new(None) };
    static IMAGE_SELECTION_ENVIRONMENT: Cell<(f32, f32, f32)> = const { Cell::new((800.0, 600.0, 1.0)) };
    static IMAGE_COOKIE_CONTEXT: RefCell<Option<ImageRequestContext>> = const { RefCell::new(None) };
    static IMAGE_CACHE_OWNER: RefCell<Option<ImageCacheOwner>> = const { RefCell::new(None) };
    static HTML_TAG_SQLITE_CONNECTIONS: RefCell<HashMap<String, Connection>> = RefCell::new(HashMap::new());
    static CONTENT_VISIBILITY_LAYOUT: RefCell<Option<ContentVisibilityLayoutSession>> = const { RefCell::new(None) };
    static LAYOUT_ERROR_REPORTER: RefCell<Option<(Arc<ErrorReporter>, ExecutionSurface)>> = const { RefCell::new(None) };
}

/// Immutable requesting environment for images discovered during one document traversal.
#[derive(Clone)]
struct ImageRequestContext {
    store: Arc<Mutex<CookieJar>>,
    cookie_context: crate::http::CookieRequestContext,
    document_id: usize,
    origin: CorsOrigin,
    embedder_policy: EmbedderPolicyValue,
    report_only_embedder_policy: EmbedderPolicyValue,
    coep_reports: Rc<RefCell<Vec<CorpViolation>>>,
}

/// Returns the document owning the current traversal's image requests.
fn image_document_id() -> usize {
    IMAGE_COOKIE_CONTEXT.with(|cell| {
        cell.borrow()
            .as_ref()
            .map_or(0, |context| context.document_id)
    })
}

/// Runs layout with an optional destination for recoverable layout failures.
pub(crate) fn with_error_reporter<T>(
    destination: Option<(Arc<ErrorReporter>, ExecutionSurface)>,
    f: impl FnOnce() -> T,
) -> T {
    struct ReporterGuard(Option<(Arc<ErrorReporter>, ExecutionSurface)>);

    impl Drop for ReporterGuard {
        fn drop(&mut self) {
            LAYOUT_ERROR_REPORTER.with(|cell| {
                cell.replace(self.0.take());
            });
        }
    }

    LAYOUT_ERROR_REPORTER.with(|cell| {
        let previous = cell.replace(destination);
        let _guard = ReporterGuard(previous);
        f()
    })
}

fn report_inline_image_decode_failure() {
    LAYOUT_ERROR_REPORTER.with(|cell| {
        let destination = cell.borrow();
        let Some((reporter, surface)) = destination.as_ref() else {
            return;
        };
        reporter.report(
            RawEvent::new(
                ErrorCategory::Layout,
                ErrorSeverity::Warning,
                ErrorCode::new("LAYOUT_INLINE_IMAGE_DECODE_FAILED").expect("static code"),
                *surface,
                "Layout failed",
                &[("operation", "decode"), ("resource", "image")],
            )
            .sanitize(),
        );
    });
}

#[derive(Clone)]
struct ContentVisibilityLayoutSession {
    input: ContentVisibilityLayoutInput,
    report: Rc<RefCell<ContentVisibilityLayoutReport>>,
}

/// Runtime inputs needed to decide whether `content-visibility: auto` is close
/// enough to the viewport to lay out its contents.
#[derive(Debug, Clone, Default)]
pub(crate) struct ContentVisibilityLayoutInput {
    pub(crate) visible_rect: Rect,
    pub(crate) forced_nodes: HashSet<usize>,
    pub(crate) relevant_nodes: HashSet<usize>,
    pub(crate) remembered_sizes: HashMap<usize, (f32, f32)>,
}

/// Observable results of one content-visibility-aware layout pass.
#[derive(Debug, Clone, Default)]
pub(crate) struct ContentVisibilityLayoutReport {
    pub(crate) auto_nodes: HashSet<usize>,
    pub(crate) skipped_nodes: HashSet<usize>,
    pub(crate) visited_nodes: HashSet<usize>,
    pub(crate) auto_intrinsic_nodes: HashSet<usize>,
    pub(crate) remembered_sizes: HashMap<usize, (f32, f32)>,
}

struct ContentVisibilityLayoutScope {
    previous: Option<ContentVisibilityLayoutSession>,
}

impl ContentVisibilityLayoutScope {
    fn new(session: ContentVisibilityLayoutSession) -> Self {
        let previous = CONTENT_VISIBILITY_LAYOUT.with(|current| current.replace(Some(session)));
        Self { previous }
    }
}

impl Drop for ContentVisibilityLayoutScope {
    fn drop(&mut self) {
        CONTENT_VISIBILITY_LAYOUT.with(|current| {
            current.replace(self.previous.take());
        });
    }
}

/// Runs image resolution at a deterministic frame-scheduler timestamp.
pub(crate) fn with_image_animation_time<T>(time_ms: u64, f: impl FnOnce() -> T) -> T {
    IMAGE_ANIMATION_TIME_MS.with(|cell| {
        struct Restore<'a> {
            cell: &'a Cell<u64>,
            previous: u64,
        }

        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.cell.set(self.previous);
            }
        }

        let restore = Restore {
            cell,
            previous: cell.replace(time_ms),
        };
        let result = f();
        drop(restore);
        result
    })
}

struct LayoutFontContext {
    system_fonts: Vec<Arc<Font>>,
    web_fonts: Option<Arc<WebFontRegistry>>,
    exact_metrics: bool,
    metrics_cache:
        HashMap<(Option<FontFamilyKey>, Option<usize>, FontVariantKey, u32), LayoutFontMetrics>,
}

/// Runs `f` with the given fonts installed as the thread-local layout font
/// context, restoring the previous context afterwards — including when `f`
/// panics, so a caught panic cannot leak fonts into later renders.
pub(crate) fn with_layout_fonts<T>(
    system_fonts: Vec<Arc<Font>>,
    web_fonts: Option<Arc<WebFontRegistry>>,
    f: impl FnOnce() -> T,
) -> T {
    struct LayoutFontsGuard(Option<LayoutFontContext>);

    impl Drop for LayoutFontsGuard {
        fn drop(&mut self) {
            LAYOUT_FONTS.with(|cell| {
                cell.replace(self.0.take());
            });
        }
    }

    LAYOUT_FONTS.with(|cell| {
        let previous = cell.replace(Some(LayoutFontContext {
            system_fonts,
            web_fonts,
            exact_metrics: true,
            metrics_cache: HashMap::new(),
        }));
        let _guard = LayoutFontsGuard(previous);
        f()
    })
}

static UNSUPPORTED_HTML_CONFIG: OnceLock<UnsupportedHtmlConfig> = OnceLock::new();
static UNSUPPORTED_HTML_LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static HTML_SQLITE_LOG_ERRORS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
const MAX_HTML_LOG_KEYS: usize = 4096;
const MAX_HTML_SQLITE_ERRORS: usize = 1024;

#[derive(Debug, Clone)]
struct UnsupportedHtmlConfig {
    logging_enabled: bool,
    sqlite_path: Option<String>,
}

fn unsupported_html_config() -> &'static UnsupportedHtmlConfig {
    UNSUPPORTED_HTML_CONFIG.get_or_init(|| UnsupportedHtmlConfig {
        logging_enabled: std::env::var("OMOIKANE_LOG_UNSUPPORTED_HTML")
            .map(|v| {
                let v = v.trim();
                v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
            })
            .unwrap_or(false),
        sqlite_path: std::env::var("OMOIKANE_UNSUPPORTED_HTML_SQLITE")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
    })
}

fn is_supported_html_tag(tag: &str) -> bool {
    matches!(
        tag,
        "html"
            | "head"
            | "body"
            | "div"
            | "span"
            | "section"
            | "article"
            | "aside"
            | "main"
            | "nav"
            | "header"
            | "footer"
            | "p"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "br"
            | "strong"
            | "em"
            | "b"
            | "i"
            | "u"
            | "s"
            | "a"
            | "pre"
            | "code"
            | "ul"
            | "ol"
            | "li"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "td"
            | "th"
            | "img"
            | "object"
            | "svg"
            | "form"
            | "input"
            | "button"
            | "textarea"
            | "select"
            | "option"
            | "iframe"
            | "label"
            | "style"
            | "link"
            | "meta"
            | "title"
            | "script"
            | "noscript"
            | "font"
            | "blockquote"
            | "hr"
            | "address"
            | "dl"
            | "dt"
            | "dd"
            | "figure"
            | "figcaption"
            | "sup"
            | "sub"
            | "small"
            | "mark"
            | "abbr"
            | "cite"
            | "q"
            | "center"
            | "nobr"
            | "wbr"
            | "details"
            | "summary"
            | "dialog"
            | "time"
            | "progress"
            | "meter"
            | "video"
            | "audio"
            | "canvas"
            | "source"
            | "picture"
    )
}

fn log_unsupported_html_tag(tag: &str, parent_tag: Option<&str>) {
    let config = unsupported_html_config();
    if !config.logging_enabled && config.sqlite_path.is_none() {
        return;
    }
    if is_supported_html_tag(tag) {
        return;
    }

    if let Some(path) = config.sqlite_path.as_deref() {
        persist_unsupported_html_to_sqlite(path, tag, parent_tag);
    }

    if config.logging_enabled {
        let key = format!("{tag}:{}", parent_tag.unwrap_or(""));
        let logged = UNSUPPORTED_HTML_LOGGED.get_or_init(|| Mutex::new(HashSet::new()));
        let mut logged = logged.lock().expect("html log lock poisoned");
        if logged.len() >= MAX_HTML_LOG_KEYS {
            logged.clear();
        }
        if logged.insert(key) {
            eprintln!(
                "[omoikane][unsupported-html] <{tag}> (parent: {})",
                parent_tag.unwrap_or("none")
            );
        }
    }
}

#[cfg(test)]
fn close_html_sqlite_connection_for_path(path: &str) {
    HTML_TAG_SQLITE_CONNECTIONS.with(|c| {
        c.borrow_mut().remove(path);
    });
}

fn persist_unsupported_html_to_sqlite(path: &str, tag: &str, parent_tag: Option<&str>) {
    let result: Result<(), rusqlite::Error> = HTML_TAG_SQLITE_CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        if !connections.contains_key(path) {
            let conn = Connection::open(path)?;
            conn.busy_timeout(std::time::Duration::from_millis(5000))?;
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 CREATE TABLE IF NOT EXISTS unsupported_html_log (
                     tag TEXT NOT NULL,
                     parent_tag TEXT NOT NULL DEFAULT '',
                     first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                     last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                     occurrences INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY (tag, parent_tag)
                 );
                 CREATE INDEX IF NOT EXISTS idx_unsupported_html_log_occurrences
                 ON unsupported_html_log (occurrences DESC);",
            )?;
            connections.insert(path.to_string(), conn);
        }
        let conn = connections.get(path).expect("connection must exist");
        conn.execute(
            "INSERT INTO unsupported_html_log (tag, parent_tag, occurrences)
             VALUES (?1, ?2, 1)
             ON CONFLICT(tag, parent_tag) DO UPDATE SET
               occurrences = unsupported_html_log.occurrences + 1,
               last_seen_at = CURRENT_TIMESTAMP",
            params![tag, parent_tag.unwrap_or("")],
        )?;
        Ok(())
    });
    if let Err(error) = result {
        let key = format!("{error}");
        let errors = HTML_SQLITE_LOG_ERRORS.get_or_init(|| Mutex::new(HashSet::new()));
        let mut errors = errors.lock().expect("html sqlite error lock poisoned");
        if errors.len() < MAX_HTML_SQLITE_ERRORS && errors.insert(key.clone()) {
            eprintln!("[omoikane][unsupported-html][sqlite-error] {key}");
        }
    }
}

/// Drops every cached image that was decoded from a `blob:` URL.
///
/// Object URLs belong to the Document that minted them, so their decoded images
/// must not survive into the next one: a cache hit would otherwise resolve a URL
/// whose store entry is gone, and the pixels would be retained for the rest of
/// the process. Called alongside [`crate::data::clear_blob_urls`] when a new
/// global is created.
pub(crate) fn forget_blob_url_images() {
    IMAGE_CACHE.with(|cache| {
        cache.borrow_mut().retain(|url, _| {
            !url.get(..5)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("blob:"))
        });
    });
}

/// Runs layout/image resolution with a temporary base URL used for relative image sources.
pub fn with_image_base_url<T>(base_url: Option<Url>, f: impl FnOnce() -> T) -> T {
    struct ImageBaseUrlGuard(Option<Url>);

    impl Drop for ImageBaseUrlGuard {
        fn drop(&mut self) {
            IMAGE_BASE_URL.with(|cell| {
                cell.replace(self.0.take());
            });
        }
    }

    IMAGE_BASE_URL.with(|cell| {
        let previous = cell.replace(base_url);
        let _guard = ImageBaseUrlGuard(previous);
        f()
    })
}

/// Captures the current image base URL at an effect boundary for explicit URL resolution.
pub(crate) fn current_image_base_url() -> Option<Url> {
    IMAGE_BASE_URL.with(|cell| cell.borrow().clone())
}

fn with_image_selection_environment<T>(
    viewport_width: f32,
    viewport_height: f32,
    device_pixel_ratio: f32,
    f: impl FnOnce() -> T,
) -> T {
    struct Restore((f32, f32, f32));

    impl Drop for Restore {
        fn drop(&mut self) {
            IMAGE_SELECTION_ENVIRONMENT.with(|cell| cell.set(self.0));
        }
    }

    IMAGE_SELECTION_ENVIRONMENT.with(|cell| {
        let previous = cell.replace((
            viewport_width.max(0.0),
            viewport_height.max(0.0),
            device_pixel_ratio.max(f32::EPSILON),
        ));
        let _restore = Restore(previous);
        f()
    })
}

fn current_image_selection_environment() -> (f32, f32, f32) {
    IMAGE_SELECTION_ENVIRONMENT.with(Cell::get)
}

/// Runs image resolution with the browsing session's Cookie store and site.
/// Cached responses are scoped to the Document that selected those cookies.
pub(crate) fn with_image_cookie_store<T>(
    store: Arc<Mutex<CookieJar>>,
    cookie_context: crate::http::CookieRequestContext,
    document_id: usize,
    f: impl FnOnce() -> T,
) -> T {
    with_image_cookie_store_reporting(
        store,
        cookie_context,
        document_id,
        current_image_base_url()
            .as_ref()
            .map(CorsOrigin::from_url)
            .unwrap_or_else(CorsOrigin::opaque),
        EmbedderPolicyValue::UnsafeNone,
        EmbedderPolicyValue::UnsafeNone,
        f,
    )
    .0
}

/// Runs image resolution and returns COEP reports generated by image fetches.
pub(crate) fn with_image_cookie_store_reporting<T>(
    store: Arc<Mutex<CookieJar>>,
    cookie_context: crate::http::CookieRequestContext,
    document_id: usize,
    origin: CorsOrigin,
    embedder_policy: EmbedderPolicyValue,
    report_only_embedder_policy: EmbedderPolicyValue,
    f: impl FnOnce() -> T,
) -> (T, Vec<CorpViolation>) {
    struct ImageCookieContextGuard(Option<ImageRequestContext>);

    impl Drop for ImageCookieContextGuard {
        fn drop(&mut self) {
            IMAGE_COOKIE_CONTEXT.with(|cell| {
                cell.replace(self.0.take());
            });
        }
    }

    let coep_reports = Rc::new(RefCell::new(Vec::new()));
    let result = IMAGE_COOKIE_CONTEXT.with(|cell| {
        let previous = cell.replace(Some(ImageRequestContext {
            store,
            cookie_context,
            document_id,
            origin,
            embedder_policy,
            report_only_embedder_policy,
            coep_reports: Rc::clone(&coep_reports),
        }));
        let _guard = ImageCookieContextGuard(previous);
        f()
    });
    let reports = std::mem::take(&mut *coep_reports.borrow_mut());
    (result, reports)
}

fn ensure_image_cache_context() {
    let current = IMAGE_COOKIE_CONTEXT.with(|cell| {
        cell.borrow().as_ref().map(|context| {
            (
                Arc::clone(&context.store),
                context.cookie_context.clone(),
                context.document_id,
                context.origin.clone(),
                context.embedder_policy,
                context.report_only_embedder_policy,
            )
        })
    });
    let owner_changed = IMAGE_CACHE_OWNER.with(|owner| {
        let mut owner = owner.borrow_mut();
        let same = match (owner.as_ref(), current.as_ref()) {
            (Some(previous), Some(current)) => {
                image_security::same_image_cache_owner(previous, current)
            }
            (None, None) => true,
            _ => false,
        };
        if !same {
            IMAGE_CACHE.with(|cache| cache.borrow_mut().clear());
            IMAGE_ANIMATION_CACHE.with(|cache| cache.borrow_mut().clear());
            *owner = current.clone();
        }
        !same
    });
    if owner_changed {
        image_security::synchronize_layout_dependency_owner(current.as_ref());
    }
}

/// A rectangle in layout space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Edge sizes for the CSS box model.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeSizes {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeSizes {
    fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// CSS box dimensions for a single layout box.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxDimensions {
    pub content: Rect,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub margin: EdgeSizes,
}

impl BoxDimensions {
    fn padding_box(&self) -> Rect {
        Rect {
            x: self.content.x - self.padding.left,
            y: self.content.y - self.padding.top,
            width: self.content.width + self.padding.horizontal(),
            height: self.content.height + self.padding.vertical(),
        }
    }

    pub fn border_box(&self) -> Rect {
        Rect {
            x: self.content.x - self.padding.left - self.border.left,
            y: self.content.y - self.padding.top - self.border.top,
            width: self.content.width
                + self.padding.left
                + self.padding.right
                + self.border.left
                + self.border.right,
            height: self.content.height
                + self.padding.top
                + self.padding.bottom
                + self.border.top
                + self.border.bottom,
        }
    }

    /// Returns the total width including padding, border, and margin.
    pub fn total_width(&self) -> f32 {
        self.content.width
            + self.padding.horizontal()
            + self.border.horizontal()
            + self.margin.horizontal()
    }

    /// Returns the total height including padding, border, and margin.
    pub fn total_height(&self) -> f32 {
        self.content.height
            + self.padding.vertical()
            + self.border.vertical()
            + self.margin.vertical()
    }
}

/// Visibility state for a laid out box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
}

/// Overflow behavior tracked by the layout tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overflow {
    #[default]
    Visible,
    /// Clips both axes without establishing a scroll container (`clip`).
    Clip,
    /// Clips only the horizontal axis (`overflow-x: clip; overflow-y: visible`).
    ClipX,
    /// Clips only the vertical axis (`overflow-x: visible; overflow-y: clip`).
    ClipY,
    /// The box clips its overflow and establishes a scroll container, so it can
    /// be scrolled programmatically (`hidden`, `scroll` and `auto`). Only user
    /// scrolling distinguishes them, and there is no scrollbar UI yet.
    Hidden,
}

impl Overflow {
    pub(crate) fn clips_x(self) -> bool {
        matches!(self, Self::Clip | Self::ClipX | Self::Hidden)
    }

    pub(crate) fn clips_y(self) -> bool {
        matches!(self, Self::Clip | Self::ClipY | Self::Hidden)
    }

    pub(crate) fn clips_overflow(self) -> bool {
        self.clips_x() || self.clips_y()
    }
}

/// Minimal style information carried by each inline fragment.
///
/// Rather than cloning the full `ComputedStyle` (a `BTreeMap`) for every
/// fragment produced from a text node, we extract only the properties needed
/// by the paint stage.  This reduces per-fragment memory allocation when a
/// text run is split into many pieces (e.g. word-wrapped lines).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FragmentStyle {
    /// The pseudo-element that supplied generated content, when applicable.
    /// Paint uses this to resolve private color overlays without altering layout.
    pub(crate) pseudo: Option<PseudoElement>,
    /// Computed CSS `visibility` for this fragment. Hidden fragments continue
    /// to participate in layout and CSSOM geometry, but paint and hit testing
    /// skip them independently from their containing block.
    pub visibility: Visibility,
    /// CSS `color` value (raw keyword or color string) from the computed style.
    /// May come from an explicit declaration, inheritance, or an initial value.
    pub color: Option<String>,
    /// CSS `text-transform` keyword from the computed style (pre-normalized to lowercase).
    /// May reflect an explicitly set value or one inherited from an ancestor.
    pub text_transform: Option<String>,
    /// CSS `text-decoration-line` keyword from the computed style (pre-normalized to lowercase).
    /// May include values originating from explicit declarations or initial defaults.
    pub text_decoration_line: Option<String>,
    /// CSS `text-decoration-color` value (raw string) from the computed style.
    pub text_decoration_color: Option<String>,
    /// Decorations originating on this box or propagated through its ancestors.
    pub(crate) text_decorations: Arc<[crate::css::style::PropagatedTextDecoration]>,
    /// CSS `font-weight` value (raw string, e.g. `"bold"`, `"400"`), pre-normalized to lowercase.
    /// Used by the paint stage to select the appropriate web font variant.
    pub font_weight: Option<String>,
    /// CSS `font-style` value (raw string, e.g. `"italic"`, `"normal"`), pre-normalized to lowercase.
    /// Used by the paint stage to select the appropriate web font variant.
    pub font_style: Option<String>,
    /// CSS `font-stretch` value used for face matching.
    pub font_stretch: Option<String>,
    /// CSS `font-family` value (raw string, first family in list).
    /// Used by the paint stage to look up web font variants.
    pub font_family: Option<String>,
    /// Tree scope captured by the `font-family` declaration.
    pub(crate) font_scope_root: Option<usize>,
    /// Typed OpenType feature and variation settings shared with shaping.
    pub(crate) shaping_options: Arc<crate::font::ShapingOptions>,
    /// Synthetic face permissions consumed after font matching at paint time.
    pub(crate) font_synthesis: crate::font::FontSynthesis,
    /// Resolved CPAL palette selection and entry overrides shared with paint.
    pub(crate) font_palette: Arc<crate::font::FontPalette>,
    /// Resolved writing mode for the inline fragment.  Keeping this small
    /// inherited value on the fragment lets paint use the same flow direction
    /// as layout even when a nested inline overrides its ancestor.
    pub writing_mode: Option<String>,
    /// Typed CSS `text-orientation`, validated by the style system and
    /// converted once when the fragment crosses into layout and paint.
    pub(crate) text_orientation: TextOrientation,
    /// Typed CSS `text-combine-upright` consumed by vertical inline layout.
    pub(crate) text_combine_upright: TextCombineUpright,
    /// Typed CSS `text-autospace` controls shared by measurement and paint.
    pub(crate) text_autospace: TextAutospace,
    /// Typed CSS `text-spacing-trim` controls for contextual CJK punctuation.
    pub(crate) text_spacing_trim: TextSpacingTrim,
    /// Typed CSS emphasis mark and placement consumed directly by paint.
    pub(crate) text_emphasis: TextEmphasis,
    pub(crate) text_emphasis_color: Option<String>,
    /// Whether the inherited document language is French. Resolved once from
    /// the DOM language boundary for language-specific `text-autospace` rules.
    pub(crate) french_content_language: bool,
    /// Resolved inline base direction (`ltr` or `rtl`).
    pub direction: Option<String>,
    /// Resolved CSS bidi mode used while constructing the line paragraph.
    pub unicode_bidi: Option<String>,
    /// Typed CSS Text break controls consumed by inline layout. Values are
    /// validated by the style system and converted once at this boundary.
    pub(crate) hyphens: Hyphens,
    pub(crate) hyphenate_character: HyphenateCharacter,
    pub(crate) line_break: LineBreak,
    pub(crate) text_wrap_style: TextWrapStyle,
    /// UAX#9 embedding level resolved for this fragment as part of its line.
    /// Paint uses this instead of resolving adjacent fragments independently.
    pub resolved_bidi_level: Option<u8>,
}

impl FragmentStyle {
    /// Build a `FragmentStyle` from a full `ComputedStyle`.
    /// Keywords for `text-transform` and `text-decoration-line` are pre-normalized
    /// to lowercase to avoid per-paint allocation.
    pub fn from_computed(style: &ComputedStyle) -> Self {
        use crate::css::ComputedValue;
        use crate::css::style::PropertyId;

        let extract_str = |key: PropertyId| -> Option<String> {
            match style.get_by_id(key) {
                Some(ComputedValue::Keyword(s)) => Some(s.clone()),
                Some(ComputedValue::Color(s)) => Some(s.clone()),
                Some(ComputedValue::String(s)) => Some(s.clone()),
                _ => None,
            }
        };

        let normalize_lower = |key: PropertyId| -> Option<String> {
            extract_str(key).map(|s| s.to_ascii_lowercase())
        };

        // Extract the first font-family name from the CSS value.
        // The value may be quoted (e.g. `"My Font"`) or unquoted (e.g. `sans-serif`).
        let font_family = extract_str(PropertyId::FontFamily).map(|s| {
            // Take the first comma-separated entry and strip quotes
            let first = s.split(',').next().unwrap_or(&s).trim().to_string();
            first.trim_matches('"').trim_matches('\'').to_string()
        });

        Self {
            pseudo: None,
            visibility: visibility(style),
            color: extract_str(PropertyId::Color),
            text_transform: normalize_lower(PropertyId::TextTransform),
            text_decoration_line: normalize_lower(PropertyId::TextDecorationLine),
            text_decoration_color: extract_str(PropertyId::TextDecorationColor),
            text_decorations: Arc::clone(style.text_decorations()),
            font_weight: normalize_lower(PropertyId::FontWeight),
            font_style: normalize_lower(PropertyId::FontStyle),
            font_stretch: match style.get_by_id(PropertyId::FontStretch) {
                Some(ComputedValue::Percentage(value)) => Some(format!("{value}%")),
                _ => normalize_lower(PropertyId::FontStretch),
            },
            font_family,
            font_scope_root: style.font_family_scope_root(),
            shaping_options: Arc::clone(style.shaping_options()),
            font_synthesis: style.font_synthesis(),
            font_palette: Arc::clone(style.font_palette()),
            writing_mode: normalize_lower(PropertyId::WritingMode),
            text_orientation: match normalize_lower(PropertyId::TextOrientation).as_deref() {
                Some("upright") => TextOrientation::Upright,
                Some("sideways") => TextOrientation::Sideways,
                _ => TextOrientation::Mixed,
            },
            text_combine_upright: match normalize_lower(PropertyId::TextCombineUpright).as_deref() {
                Some("all") => TextCombineUpright::All,
                Some(value) => value
                    .strip_prefix("digits ")
                    .and_then(|count| count.parse::<u8>().ok())
                    .filter(|count| (2..=4).contains(count))
                    .map_or(TextCombineUpright::None, TextCombineUpright::Digits),
                _ => TextCombineUpright::None,
            },
            text_autospace: TextAutospace::from_computed(
                normalize_lower(PropertyId::TextAutospace).as_deref(),
            ),
            text_spacing_trim: TextSpacingTrim::from_computed(
                normalize_lower(PropertyId::TextSpacingTrim).as_deref(),
            ),
            text_emphasis: TextEmphasis::from_computed(
                extract_str(PropertyId::TextEmphasisStyle).as_deref(),
                normalize_lower(PropertyId::TextEmphasisPosition).as_deref(),
            ),
            text_emphasis_color: extract_str(PropertyId::TextEmphasisColor),
            french_content_language: false,
            direction: normalize_lower(PropertyId::Direction),
            unicode_bidi: normalize_lower(PropertyId::UnicodeBidi),
            hyphens: match normalize_lower(PropertyId::Hyphens).as_deref() {
                Some("none") => Hyphens::None,
                Some("auto") => Hyphens::Auto,
                _ => Hyphens::Manual,
            },
            hyphenate_character: match style.get_by_id(PropertyId::HyphenateCharacter) {
                Some(ComputedValue::String(value)) => HyphenateCharacter::String(value.clone()),
                _ => HyphenateCharacter::Auto,
            },
            line_break: match normalize_lower(PropertyId::LineBreak).as_deref() {
                Some("loose") => LineBreak::Loose,
                Some("normal") => LineBreak::Normal,
                Some("strict") => LineBreak::Strict,
                Some("anywhere") => LineBreak::Anywhere,
                _ => LineBreak::Auto,
            },
            text_wrap_style: match normalize_lower(PropertyId::TextWrapStyle).as_deref() {
                Some("balance") => TextWrapStyle::Balance,
                Some("stable") => TextWrapStyle::Stable,
                Some("pretty") => TextWrapStyle::Pretty,
                _ => TextWrapStyle::Auto,
            },
            resolved_bidi_level: None,
        }
    }
}

/// Paint-ready CSS `text-emphasis` value. Unknown strings are normalized at
/// the computed-style boundary so paint never reparses CSS tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextEmphasis {
    pub(crate) mark: Option<String>,
    pub(crate) over: bool,
    pub(crate) right: bool,
}

impl Default for TextEmphasis {
    fn default() -> Self {
        Self {
            mark: None,
            over: true,
            right: true,
        }
    }
}

impl TextEmphasis {
    fn from_computed(style: Option<&str>, position: Option<&str>) -> Self {
        let style = style.unwrap_or("none");
        let mark = match style {
            "none" => None,
            "filled dot" => Some("\u{2022}".into()),
            "open dot" => Some("\u{25e6}".into()),
            "filled circle" => Some("\u{25cf}".into()),
            "open circle" => Some("\u{25cb}".into()),
            "filled double-circle" => Some("\u{25c9}".into()),
            "open double-circle" => Some("\u{25ce}".into()),
            "filled triangle" => Some("\u{25b2}".into()),
            "open triangle" => Some("\u{25b3}".into()),
            "filled sesame" => Some("\u{fe45}".into()),
            "open sesame" => Some("\u{fe46}".into()),
            custom => Some(custom.into()),
        };
        let position = position.unwrap_or("over right");
        Self {
            mark,
            over: !position
                .split_ascii_whitespace()
                .any(|part| part == "under"),
            right: !position.split_ascii_whitespace().any(|part| part == "left"),
        }
    }
}

/// Glyph orientation requested inside vertical writing modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TextOrientation {
    #[default]
    Mixed,
    Upright,
    Sideways,
}

/// Runs eligible for horizontal typesetting inside one vertical em cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TextCombineUpright {
    #[default]
    None,
    All,
    Digits(u8),
}

/// Character-class switches and whitespace handling for `text-autospace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextAutospace {
    pub(crate) ideograph_alpha: bool,
    pub(crate) ideograph_numeric: bool,
    pub(crate) punctuation: bool,
    pub(crate) replace: bool,
}

impl Default for TextAutospace {
    fn default() -> Self {
        Self {
            ideograph_alpha: true,
            ideograph_numeric: true,
            punctuation: false,
            replace: false,
        }
    }
}

impl TextAutospace {
    fn from_computed(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        if value == "no-autospace" {
            return Self {
                ideograph_alpha: false,
                ideograph_numeric: false,
                punctuation: false,
                replace: false,
            };
        }
        if matches!(value, "normal" | "auto") {
            return Self::default();
        }
        let has = |keyword| value.split_ascii_whitespace().any(|part| part == keyword);
        let only_mode = matches!(value, "insert" | "replace");
        Self {
            ideograph_alpha: only_mode || has("ideograph-alpha"),
            ideograph_numeric: only_mode || has("ideograph-numeric"),
            punctuation: has("punctuation"),
            replace: has("replace"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextSpacingTrim {
    pub(crate) start: TextSpacingTrimStart,
    pub(crate) adjacent: TextSpacingTrimAdjacent,
    pub(crate) end: TextSpacingTrimEnd,
    pub(crate) trim_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextSpacingTrimStart {
    Trim,
    Space,
    SpaceFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextSpacingTrimAdjacent {
    Trim,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextSpacingTrimEnd {
    Trim,
    Space,
    Allow,
}

impl Default for TextSpacingTrim {
    fn default() -> Self {
        Self {
            start: TextSpacingTrimStart::Space,
            adjacent: TextSpacingTrimAdjacent::Trim,
            end: TextSpacingTrimEnd::Allow,
            trim_all: false,
        }
    }
}

impl TextSpacingTrim {
    fn from_computed(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self::default();
        };
        match value {
            "normal" => return Self::default(),
            "auto" | "trim-both" => {
                return Self {
                    start: TextSpacingTrimStart::Trim,
                    adjacent: TextSpacingTrimAdjacent::Trim,
                    end: TextSpacingTrimEnd::Trim,
                    trim_all: false,
                };
            }
            "space-all" => {
                return Self {
                    start: TextSpacingTrimStart::Space,
                    adjacent: TextSpacingTrimAdjacent::Space,
                    end: TextSpacingTrimEnd::Space,
                    trim_all: false,
                };
            }
            "trim-all" => {
                return Self {
                    start: TextSpacingTrimStart::Trim,
                    adjacent: TextSpacingTrimAdjacent::Trim,
                    end: TextSpacingTrimEnd::Trim,
                    trim_all: true,
                };
            }
            _ => {}
        }
        let has = |keyword| value.split_ascii_whitespace().any(|part| part == keyword);
        Self {
            start: if has("space-start") {
                TextSpacingTrimStart::Space
            } else if has("space-first") {
                TextSpacingTrimStart::SpaceFirst
            } else {
                TextSpacingTrimStart::Trim
            },
            adjacent: if has("space-adjacent") {
                TextSpacingTrimAdjacent::Space
            } else {
                TextSpacingTrimAdjacent::Trim
            },
            end: if has("trim-end") {
                TextSpacingTrimEnd::Trim
            } else if has("space-end") {
                TextSpacingTrimEnd::Space
            } else {
                TextSpacingTrimEnd::Allow
            },
            trim_all: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FullwidthPunctuationClass {
    Opening,
    Closing,
    Middle,
}

pub(crate) fn fullwidth_punctuation_class(
    character: char,
    vertical: bool,
) -> Option<FullwidthPunctuationClass> {
    use FullwidthPunctuationClass::{Closing, Middle, Opening};
    match character {
        '\u{2018}' | '\u{201c}' | '\u{3008}' | '\u{300a}' | '\u{300c}' | '\u{300e}'
        | '\u{3010}' | '\u{3014}' | '\u{3016}' | '\u{3018}' | '\u{301a}' | '\u{301d}'
        | '\u{ff08}' | '\u{ff3b}' | '\u{ff5b}' | '\u{ff5f}' => Some(Opening),
        '\u{2019}' | '\u{201d}' | '\u{3009}' | '\u{300b}' | '\u{300d}' | '\u{300f}'
        | '\u{3011}' | '\u{3015}' | '\u{3017}' | '\u{3019}' | '\u{301b}' | '\u{301e}'
        | '\u{301f}' | '\u{ff09}' | '\u{ff3d}' | '\u{ff5d}' | '\u{ff60}' | '\u{3001}'
        | '\u{3002}' | '\u{ff0c}' | '\u{ff0e}' => Some(Closing),
        '\u{00b7}' | '\u{2027}' | '\u{30fb}' | '\u{ff1a}' => Some(Middle),
        '\u{ff1b}' if !vertical => Some(Middle),
        _ => None,
    }
}

pub(crate) fn opening_punctuation_trims_after(
    previous: char,
    previous_size: f32,
    opening_size: f32,
    vertical: bool,
) -> bool {
    match fullwidth_punctuation_class(previous, vertical) {
        Some(FullwidthPunctuationClass::Opening | FullwidthPunctuationClass::Middle) => true,
        Some(FullwidthPunctuationClass::Closing) => previous_size >= opening_size,
        None => {
            previous == '\u{3000}'
                || get_general_category(previous) == GeneralCategory::OpenPunctuation
        }
    }
}

pub(crate) fn closing_punctuation_trims_before(
    next: char,
    closing_size: f32,
    next_size: f32,
    vertical: bool,
) -> bool {
    match fullwidth_punctuation_class(next, vertical) {
        Some(FullwidthPunctuationClass::Closing | FullwidthPunctuationClass::Middle) => true,
        Some(FullwidthPunctuationClass::Opening) => next_size > closing_size,
        None => {
            next == '\u{3000}' || get_general_category(next) == GeneralCategory::ClosePunctuation
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PunctuationTrimBoundary {
    pub(crate) byte_offset: usize,
    pub(crate) leading: bool,
    pub(crate) trailing: bool,
}

pub(crate) fn text_spacing_trim_boundaries(
    text: &str,
    value: TextSpacingTrim,
    vertical: bool,
) -> Vec<PunctuationTrimBoundary> {
    let graphemes = text.grapheme_indices(true).collect::<Vec<_>>();
    graphemes
        .iter()
        .enumerate()
        .filter_map(|(index, (offset, grapheme))| {
            let class = fullwidth_punctuation_class(grapheme.chars().next()?, vertical)?;
            let previous = index
                .checked_sub(1)
                .and_then(|index| graphemes[index].1.chars().next());
            let next = graphemes
                .get(index + 1)
                .and_then(|(_, grapheme)| grapheme.chars().next());
            let leading = (value.trim_all
                && matches!(
                    class,
                    FullwidthPunctuationClass::Opening | FullwidthPunctuationClass::Middle
                ))
                || (class == FullwidthPunctuationClass::Opening
                    && value.adjacent == TextSpacingTrimAdjacent::Trim
                    && previous.is_some_and(|character| {
                        opening_punctuation_trims_after(character, 1.0, 1.0, vertical)
                    }));
            let trailing = (value.trim_all && class == FullwidthPunctuationClass::Closing)
                || (class == FullwidthPunctuationClass::Closing
                    && value.adjacent == TextSpacingTrimAdjacent::Trim
                    && next.is_some_and(|character| {
                        closing_punctuation_trims_before(character, 1.0, 1.0, vertical)
                    }));
            (leading || trailing).then_some(PunctuationTrimBoundary {
                byte_offset: *offset,
                leading,
                trailing,
            })
        })
        .collect()
}

pub(crate) fn text_spacing_trim_shaping_options<'a>(
    text: &str,
    value: TextSpacingTrim,
    vertical: bool,
    base: &'a crate::font::ShapingOptions,
) -> std::borrow::Cow<'a, crate::font::ShapingOptions> {
    text_spacing_trim_shaping_options_with_edges(
        text,
        value,
        vertical,
        PunctuationTrim::default(),
        base,
    )
}

pub(crate) fn text_spacing_trim_shaping_options_with_edges<'a>(
    text: &str,
    value: TextSpacingTrim,
    vertical: bool,
    edges: PunctuationTrim,
    base: &'a crate::font::ShapingOptions,
) -> std::borrow::Cow<'a, crate::font::ShapingOptions> {
    let offsets = text_spacing_trim_offsets_with_edges(text, value, vertical, edges);
    if offsets.is_empty() {
        return std::borrow::Cow::Borrowed(base);
    }
    let mut options = base.clone();
    let tag = if vertical { *b"vhal" } else { *b"halt" };
    for offset in offsets {
        let Some(character) = text[offset..].chars().next() else {
            continue;
        };
        let feature =
            crate::font::OpenTypeFeature::for_range(tag, 1, offset..offset + character.len_utf8());
        if !options.features.contains(&feature) {
            options.features.push(feature);
        }
    }
    std::borrow::Cow::Owned(options)
}

pub(crate) fn text_spacing_trim_offsets_with_edges(
    text: &str,
    value: TextSpacingTrim,
    vertical: bool,
    edges: PunctuationTrim,
) -> Vec<usize> {
    let mut offsets = text_spacing_trim_boundaries(text, value, vertical)
        .into_iter()
        .map(|boundary| boundary.byte_offset)
        .collect::<Vec<_>>();
    if edges.first
        && let Some((offset, _)) = text.grapheme_indices(true).next()
    {
        offsets.push(offset);
    }
    if edges.last
        && let Some((offset, _)) = text.grapheme_indices(true).next_back()
    {
        offsets.push(offset);
    }
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// Returns whether a prepared fragment is one combined upright unit.
pub(crate) fn text_is_combined_upright(text: &str, style: &FragmentStyle) -> bool {
    if text.is_empty()
        || !matches!(
            style.writing_mode.as_deref(),
            Some("vertical-rl" | "vertical-lr" | "sideways-rl" | "sideways-lr")
        )
    {
        return false;
    }
    match style.text_combine_upright {
        TextCombineUpright::None => false,
        TextCombineUpright::All => !text.contains(['\n', '\r']),
        TextCombineUpright::Digits(maximum) => {
            let count = text.len();
            (2..=maximum as usize).contains(&count)
                && text.bytes().all(|byte| byte.is_ascii_digit())
        }
    }
}

fn is_autospace_ideograph(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x3041..=0x30FF
            | 0x31C0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    ) && !matches!(
        ch,
        '、' | '。' | '・' | '「' | '」' | '『' | '』' | '（' | '）'
    )
}

fn is_east_asian_wide_or_fullwidth(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x1100..=0x11FF
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE6F
            | 0xFF01..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD
    )
}

fn is_upright_autospace_character(
    ch: char,
    vertical_writing: bool,
    sideways_writing: bool,
    orientation: TextOrientation,
) -> bool {
    if !vertical_writing || sideways_writing || orientation == TextOrientation::Sideways {
        return false;
    }
    orientation == TextOrientation::Upright
        || matches!(
            char_orientation(ch),
            Orientation::Upright | Orientation::TransformedOrUpright
        )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AutospaceClass {
    Ideograph,
    Letter,
    Numeral,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrenchAutospace {
    NarrowNoBreak,
    NoBreak,
}

impl FrenchAutospace {
    pub(crate) fn character(self) -> char {
        match self {
            Self::NarrowNoBreak => '\u{202f}',
            Self::NoBreak => '\u{00a0}',
        }
    }
}

pub(crate) fn fragment_french_autospace_pair(
    left: char,
    right: char,
    style: &FragmentStyle,
) -> Option<FrenchAutospace> {
    french_autospace_pair(
        left,
        right,
        style.french_content_language,
        style.text_autospace.punctuation,
    )
}

fn french_autospace_pair(
    left: char,
    right: char,
    french_content_language: bool,
    punctuation: bool,
) -> Option<FrenchAutospace> {
    if !french_content_language || !punctuation {
        return None;
    }
    if left.is_whitespace() || right.is_whitespace() {
        return None;
    }
    match (left, right) {
        ('«', _) | (_, '»' | ':') => Some(FrenchAutospace::NoBreak),
        (_, ';' | '?' | '!') => Some(FrenchAutospace::NarrowNoBreak),
        _ => None,
    }
}

pub(crate) fn fragment_french_autospace_boundaries(
    text: &str,
    style: &FragmentStyle,
) -> Vec<(usize, FrenchAutospace)> {
    let graphemes = text.grapheme_indices(true).collect::<Vec<_>>();
    graphemes
        .windows(2)
        .filter_map(|pair| {
            let (right_offset, right) = pair[1];
            fragment_french_autospace_pair(pair[0].1.chars().next()?, right.chars().next()?, style)
                .map(|spacing| (right_offset, spacing))
        })
        .collect()
}

pub(crate) fn font_metrics_french_autospace_boundaries(
    text: &str,
    metrics: &FontMetrics,
) -> Vec<(usize, FrenchAutospace)> {
    let graphemes = text.grapheme_indices(true).collect::<Vec<_>>();
    graphemes
        .windows(2)
        .filter_map(|pair| {
            let (right_offset, right) = pair[1];
            french_autospace_pair(
                pair[0].1.chars().next()?,
                right.chars().next()?,
                metrics.french_content_language,
                metrics.text_autospace.punctuation,
            )
            .map(|spacing| (right_offset, spacing))
        })
        .collect()
}

fn autospace_class(
    ch: char,
    vertical_writing: bool,
    sideways_writing: bool,
    orientation: TextOrientation,
) -> AutospaceClass {
    if is_autospace_ideograph(ch) {
        AutospaceClass::Ideograph
    } else if is_upright_autospace_character(ch, vertical_writing, sideways_writing, orientation)
        || is_east_asian_wide_or_fullwidth(ch)
    {
        AutospaceClass::Other
    } else if ch.is_alphabetic() || matches!(ch as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF) {
        AutospaceClass::Letter
    } else if ch.is_numeric() {
        AutospaceClass::Numeral
    } else {
        AutospaceClass::Other
    }
}

fn autospace_pair_applies(
    left: AutospaceClass,
    right: AutospaceClass,
    value: TextAutospace,
) -> bool {
    matches!(
        (left, right),
        (AutospaceClass::Ideograph, AutospaceClass::Letter)
            | (AutospaceClass::Letter, AutospaceClass::Ideograph)
    ) && value.ideograph_alpha
        || matches!(
            (left, right),
            (AutospaceClass::Ideograph, AutospaceClass::Numeral)
                | (AutospaceClass::Numeral, AutospaceClass::Ideograph)
        ) && value.ideograph_numeric
}

fn autospace_boundary_offsets(
    text: &str,
    value: TextAutospace,
    vertical_writing: bool,
    sideways_writing: bool,
    orientation: TextOrientation,
    combined: bool,
) -> Vec<usize> {
    if combined || (!value.ideograph_alpha && !value.ideograph_numeric) {
        return Vec::new();
    }
    let graphemes = text.grapheme_indices(true).collect::<Vec<_>>();
    graphemes
        .windows(2)
        .filter_map(|pair| {
            let (right_offset, right) = pair[1];
            let left = pair[0].1.chars().next()?;
            let right = right.chars().next()?;
            autospace_pair_applies(
                autospace_class(left, vertical_writing, sideways_writing, orientation),
                autospace_class(right, vertical_writing, sideways_writing, orientation),
                value,
            )
            .then_some(right_offset)
        })
        .collect()
}

pub(crate) fn fragment_autospace_boundary_offsets(text: &str, style: &FragmentStyle) -> Vec<usize> {
    let writing_mode = style.writing_mode.as_deref();
    autospace_boundary_offsets(
        text,
        style.text_autospace,
        writing_mode.is_some_and(|value| value != "horizontal-tb"),
        writing_mode.is_some_and(|value| value.starts_with("sideways-")),
        style.text_orientation,
        text_is_combined_upright(text, style),
    )
}

pub(crate) fn fragment_autospace_pair_applies(
    left: char,
    right: char,
    style: &FragmentStyle,
) -> bool {
    let writing_mode = style.writing_mode.as_deref();
    autospace_pair_applies(
        autospace_class(
            left,
            writing_mode.is_some_and(|value| value != "horizontal-tb"),
            writing_mode.is_some_and(|value| value.starts_with("sideways-")),
            style.text_orientation,
        ),
        autospace_class(
            right,
            writing_mode.is_some_and(|value| value != "horizontal-tb"),
            writing_mode.is_some_and(|value| value.starts_with("sideways-")),
            style.text_orientation,
        ),
        style.text_autospace,
    )
}

pub(crate) fn font_metrics_autospace_advance(text: &str, metrics: &FontMetrics) -> f32 {
    let combined = metrics.vertical_writing
        && match metrics.text_combine_upright {
            TextCombineUpright::All => true,
            TextCombineUpright::Digits(maximum) => {
                (2..=maximum as usize).contains(&text.len())
                    && text.bytes().all(|byte| byte.is_ascii_digit())
            }
            TextCombineUpright::None => false,
        };
    autospace_boundary_offsets(
        text,
        metrics.text_autospace,
        metrics.vertical_writing,
        metrics.sideways_writing,
        metrics.text_orientation,
        combined,
    )
    .len() as f32
        * metrics.font_size
        * 0.125
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Hyphens {
    None,
    #[default]
    Manual,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum HyphenateCharacter {
    #[default]
    Auto,
    String(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LineBreak {
    #[default]
    Auto,
    Loose,
    Normal,
    Strict,
    Anywhere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TextWrapStyle {
    #[default]
    Auto,
    Balance,
    Stable,
    Pretty,
}

/// A laid out fragment of inline text.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineFragment {
    pub node: NodeHandle,
    pub content: InlineFragmentContent,
    /// Source Text nodes and UTF-16 ranges for this fragment's rendered text.
    /// Generated content and non-text payloads have no DOM text source.
    pub text_source: Vec<InlineTextSource>,
    pub rect: Rect,
    /// Paint-free spacing appended on the physical inline-end side by
    /// `text-autospace`. Kept separate so bidi reordering can move it to the
    /// boundary between the visually adjacent characters.
    pub(crate) autospace_after: f32,
    /// Contextual CJK punctuation compaction selected after line breaking.
    pub(crate) punctuation_trim: PunctuationTrim,
    pub metrics: FontMetrics,
    pub vertical_align: VerticalAlign,
    /// Position after this fragment in its formatting context's normalized
    /// inline stream. Print pagination uses it to resume after a line break.
    pub source_end_token: usize,
    /// Minimal style information extracted from the element's `ComputedStyle`.
    /// Used by the paint stage to apply per-fragment `text-transform`,
    /// `text-decoration`, and `color` rather than inheriting from the
    /// containing block's style.
    pub style: FragmentStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PunctuationTrim {
    pub(crate) first: bool,
    pub(crate) last: bool,
}

/// An owned mapping from a rendered text span to its original DOM Text node.
/// Adjacent Text nodes can share one shaped fragment without losing identity.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineTextSource {
    /// Original Text node; this can differ from the fragment's paint owner.
    pub node: NodeHandle,
    /// UTF-8 byte range in the fragment's rendered text.
    pub text_range: std::ops::Range<usize>,
    /// UTF-16 code-unit range in the original Text node.
    pub dom_range: std::ops::Range<usize>,
    /// Whether rendered UTF-16 positions map directly within `dom_range`.
    /// False denotes collapsed whitespace or a length-changing case transform.
    pub direct: bool,
}

/// A laid out inline fragment payload.
#[derive(Debug, Clone, PartialEq)]
pub enum InlineFragmentContent {
    Text(String),
    /// Paint-free inline-axis spacing used for non-replaced inline margins.
    InlineSpacing(bool),
    /// Temporary start/end edge marker retained while lines are wrapped.
    InlineEdge(ComputedStyle, bool),
    /// The border area of a non-replaced inline element on one line.
    InlineBox(ComputedStyle),
    /// Placement marker for an atomic inline-level box stored in the owning
    /// [`LayoutBox`]'s `children`. The child owns its formatting context; this
    /// marker preserves its position in the parent line. Its value is the
    /// baseline offset from the atomic box's margin-box top.
    AtomicInline(f32),
    Image(Image, ComputedStyle),
    GeneratedBox(ComputedStyle),
    FormControl(ComputedStyle, String, Option<TextControlPaintState>),
    ValueIndicator(ComputedStyle, ValueIndicatorPaintState),
    IconFormControl(ComputedStyle, Image, f32, f32),
}

/// Normalized native-paint state for a `<progress>` or `<meter>` element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ValueIndicatorPaintState {
    /// `None` represents an indeterminate progress bar.
    Progress(Option<f32>),
    Meter {
        fraction: f32,
        region: MeterRegion,
    },
}

/// The qualitative region used by the native meter appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterRegion {
    Optimum,
    Suboptimal,
    EvenLessGood,
}

/// Selection/caret state carried from a live text control into paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextControlPaintState {
    pub selection_start: usize,
    pub selection_end: usize,
    pub focused: bool,
}

/// A single line box inside a block formatting context.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    pub rect: Rect,
    pub baseline: f32,
    pub fragments: Vec<InlineFragment>,
    /// Paint-only replacement fragments for CSS `text-overflow: ellipsis`.
    /// The original fragments remain available for intrinsic/scroll geometry.
    pub text_overflow: Option<TextOverflowPaint>,
}

/// A grapheme-safe inline paint sequence ending in an ellipsis marker.
#[derive(Debug, Clone, PartialEq)]
pub struct TextOverflowPaint {
    pub fragments: Vec<InlineFragment>,
}

/// Approximate font metrics used by inline layout.
#[derive(Debug, Clone, PartialEq)]
pub struct FontMetrics {
    pub font_size: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub average_advance: f32,
    /// Extra spacing to add between each character (CSS `letter-spacing`).
    pub letter_spacing: f32,
    /// Extra advance applied to each collapsible word separator (CSS `word-spacing`).
    pub word_spacing: f32,
    pub(crate) text_autospace: TextAutospace,
    pub(crate) text_spacing_trim: TextSpacingTrim,
    pub(crate) french_content_language: bool,
    pub(crate) text_orientation: TextOrientation,
    pub(crate) text_combine_upright: TextCombineUpright,
    pub(crate) vertical_writing: bool,
    pub(crate) sideways_writing: bool,
    pub(crate) font_family: Option<FontFamilyKey>,
    pub(crate) font_scope_root: Option<usize>,
    pub(crate) font_weight: FontWeight,
    pub(crate) font_style: FontStyle,
    pub(crate) font_style_angle: i32,
    pub(crate) font_stretch: crate::font::FontStretch,
    pub(crate) shaping_options: Arc<crate::font::ShapingOptions>,
}

impl FontMetrics {
    /// Creates approximate metrics from a CSS font size.
    pub fn from_font_size(font_size: f32) -> Self {
        Self {
            font_size,
            ascent: font_size * 0.8,
            descent: font_size * 0.2,
            line_gap: font_size * 0.2,
            average_advance: font_size * 0.6,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            text_autospace: TextAutospace::default(),
            text_spacing_trim: TextSpacingTrim::default(),
            french_content_language: false,
            text_orientation: TextOrientation::default(),
            text_combine_upright: TextCombineUpright::default(),
            vertical_writing: false,
            sideways_writing: false,
            font_family: None,
            font_scope_root: None,
            font_weight: FontWeight::default(),
            font_style: FontStyle::default(),
            font_style_angle: 0,
            font_stretch: crate::font::FontStretch::default(),
            shaping_options: Arc::new(crate::font::ShapingOptions::default()),
        }
    }
}

/// Minimal `vertical-align` values supported by the inline layout engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VerticalAlign {
    Baseline,
    Top,
    Middle,
    Bottom,
    Length(f32),
}

/// Supported flex directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

/// Supported flex wrapping modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
}

/// Minimal justify-content values supported by the flex layout engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    FlexStart,
    Center,
    FlexEnd,
    SpaceBetween,
    SpaceAround,
}

/// Minimal align-items / align-self values supported by the flex layout engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    Stretch,
    FlexStart,
    Center,
    FlexEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PositionScheme {
    Static,
    Relative,
    Sticky,
    Absolute,
    Fixed,
}

/// The writing-mode values which affect the physical flow axes.  Sideways
/// modes currently share the corresponding vertical flow direction; glyph
/// orientation is handled by the paint path as a follow-up concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WritingMode {
    HorizontalTb,
    VerticalRl,
    VerticalLr,
}

fn writing_mode(style: &ComputedStyle) -> WritingMode {
    match style.writing_mode() {
        Some(ComputedWritingMode::VerticalRl | ComputedWritingMode::SidewaysRl) => {
            WritingMode::VerticalRl
        }
        Some(ComputedWritingMode::VerticalLr | ComputedWritingMode::SidewaysLr) => {
            WritingMode::VerticalLr
        }
        _ => WritingMode::HorizontalTb,
    }
}

fn is_vertical_writing(style: &ComputedStyle) -> bool {
    !matches!(writing_mode(style), WritingMode::HorizontalTb)
}

fn is_vertical_rl(style: &ComputedStyle) -> bool {
    matches!(writing_mode(style), WritingMode::VerticalRl)
}

fn direction_is_rtl(style: &ComputedStyle) -> bool {
    style.direction() == Some(ComputedDirection::Rtl)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatSide {
    None,
    Left,
    Right,
}

#[derive(Debug, Clone)]
struct FloatRegion {
    outer: Rect,
    side: FloatSide,
    shape: Option<shapes::ShapeOutside>,
}

#[derive(Debug, Clone, Copy, Default)]
struct FloatOffsets {
    left: f32,
    right: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct VerticalFloatOffsets {
    top: f32,
    bottom: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClearSide {
    None,
    Left,
    Right,
    Both,
}

/// A list marker attached to a `display: list-item` box.
#[derive(Debug, Clone, PartialEq)]
pub struct ListMarker {
    /// The rendered marker string (e.g. "bullet", "circle", "square", "1.", ...).
    pub text: String,
    /// Font size in px to use when rendering the marker.
    pub font_size: f32,
    /// Whether the marker is positioned outside the content box (`outside`)
    /// or inside the content flow (`inside`).
    pub outside: bool,
    /// The x/y position of the marker (content box origin for outside markers,
    /// inline cursor for inside markers).
    pub x: f32,
    pub y: f32,
}

/// A block layout box derived from a DOM node.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutBox {
    pub node: NodeHandle,
    /// The generated pseudo-element represented by this box. `None` denotes
    /// the principal box for `node`.
    pub(crate) pseudo: Option<PseudoElement>,
    pub dimensions: BoxDimensions,
    pub visibility: Visibility,
    pub overflow: Overflow,
    /// Positioning mode needed when a descendant escapes an intermediate clip.
    pub(crate) position_scheme: PositionScheme,
    /// Whether this box establishes a containing block for fixed descendants.
    pub(crate) fixed_containing_block: bool,
    pub z_index: i32,
    /// Paint-time CSS transform in the document's absolute coordinate space.
    /// It does not participate in normal-flow layout sizing or placement.
    pub transform: AffineTransform,
    /// Whether this subtree can need paint-time scroll/sticky translation even
    /// when all currently stored scroll offsets are zero.
    pub(crate) needs_scroll_translation: bool,
    /// Cached after layout to skip clipped subtrees without positioned boxes.
    /// `None` keeps hand-built or in-progress layout boxes conservative.
    pub(crate) has_out_of_flow_descendants: Option<bool>,
    /// Whether this box's contents are skipped by `content-visibility` for the
    /// current layout pass. Geometry queries can force descendants into the
    /// layout tree without making those descendants paintable.
    pub(crate) content_visibility_contents_skipped: bool,
    /// Pre-translation scroll geometry consumed by paint-only features such as
    /// `background-attachment: local`. Kept separate from CSSOM layout data.
    pub(crate) paint_scroll: Option<PaintScrollGeometry>,
    /// Physical fragments produced when this block is split across columns.
    /// `source` remains in the ordinary-flow coordinate space, while `target`
    /// is the fragment's final border-box geometry in document order.
    pub(crate) block_fragments: Vec<BlockFragment>,
    /// Used multi-column overflow and the physical centers of column rules.
    pub(crate) multicol: Option<MultiColumnLayout>,
    pub lines: Vec<LineBox>,
    pub children: Vec<LayoutBox>,
    /// List marker for `display: list-item` elements.
    pub marker: Option<ListMarker>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlockFragment {
    /// Source-space slice replayed into this fragmentainer.
    pub(crate) source: Rect,
    /// Fragment border box exposed through CSSOM. It may have zero block size
    /// when only overflow continues into a later fragmentainer.
    pub(crate) target: Rect,
    /// Final fragmentainer clip used while replaying source-space painting.
    pub(crate) clip: Rect,
    /// Whether this box owns the paint replay. Descendant entries propagated
    /// for CSSOM geometry are painted by their nearest owning ancestor.
    pub(crate) owns_paint: bool,
    /// Content-area destination for `box-decoration-break: clone`. `None`
    /// denotes the ordinary `slice` mapping where source and target are equal
    /// sized border-box slices.
    pub(crate) clone_content_target: Option<Rect>,
}

impl BlockFragment {
    pub(crate) fn translation(self) -> (f32, f32) {
        let target = self.clone_content_target.unwrap_or(self.clip);
        (target.x - self.source.x, target.y - self.source.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PaintScrollGeometry {
    pub offset: (f32, f32),
    pub overflow_size: (f32, f32),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MultiColumnLayout {
    pub(crate) overflow: Rect,
    pub(crate) rules: Vec<Rect>,
}

/// Returns the style that produced a layout box. Generated boxes use their
/// pseudo-element style while principal boxes use the originating DOM node.
pub(crate) fn layout_box_style(layout: &LayoutBox, resolver: &mut StyleResolver) -> ComputedStyle {
    layout
        .pseudo
        .and_then(|pseudo| resolver.computed_pseudo_style(&layout.node, pseudo))
        .unwrap_or_else(|| resolver.computed_style(&layout.node))
}

impl LayoutBox {
    /// Returns the box width including padding, border, and margins.
    pub fn total_width(&self) -> f32 {
        self.dimensions.total_width()
    }

    /// Returns the box height including padding, border, and margins.
    pub fn total_height(&self) -> f32 {
        self.dimensions.total_height()
    }

    /// Returns the size of this box's scrolling area (`scrollWidth` /
    /// `scrollHeight`): its padding box grown to contain every descendant box,
    /// including this box's end-edge padding after overflowing content, and
    /// never smaller than the padding box itself. Inline line boxes are included
    /// because text fragments are stored directly on their containing box rather
    /// than as descendant [`LayoutBox`] values.
    ///
    /// Layout coordinates are absolute (see `layout_document` / `layout_element`),
    /// so descendant border-box edges compare directly against this box's
    /// padding-box edges. A descendant's clip bounds its ordinary content,
    /// but not an absolutely positioned descendant whose containing block is
    /// outside that clip.
    pub(crate) fn scrollable_overflow(&self) -> (f32, f32) {
        let content = self.dimensions.content;
        let padding = self.dimensions.padding;
        let client_width = content.width + padding.horizontal();
        let client_height = content.height + padding.vertical();
        let padding_right_edge = content.x + content.width + padding.right;
        let padding_bottom_edge = content.y + content.height + padding.bottom;
        let mut max_right = padding_right_edge;
        let mut max_bottom = padding_bottom_edge;
        if let Some(multicol) = &self.multicol {
            max_right = max_right.max(multicol.overflow.x + multicol.overflow.width);
            max_bottom = max_bottom.max(multicol.overflow.y + multicol.overflow.height);
        }
        expand_line_overflow(&self.lines, &mut max_right, &mut max_bottom, true, true);
        let absolute_cb = (self.position_scheme != PositionScheme::Static
            || self.fixed_containing_block)
            .then_some((true, true));
        let fixed_cb = self.fixed_containing_block.then_some((true, true));
        expand_scrollable_overflow(
            &self.children,
            &mut max_right,
            &mut max_bottom,
            absolute_cb,
            fixed_cb,
        );
        // Once descendant content crosses the padding-box end edge, the
        // scrollable overflow region includes the box's end padding after that
        // content. Content which still fits leaves the padding box unchanged.
        if max_right > padding_right_edge {
            max_right += padding.right;
        }
        if max_bottom > padding_bottom_edge {
            max_bottom += padding.bottom;
        }
        (
            (max_right - (content.x - padding.left)).max(client_width),
            (max_bottom - (content.y - padding.top)).max(client_height),
        )
    }

    /// Returns the largest scroll offsets this box can reach, i.e. its
    /// scrolling area minus its visible padding box. Both are non-negative, and
    /// zero for a box whose content fits.
    pub(crate) fn max_scroll_offset(&self) -> (f32, f32) {
        let content = self.dimensions.content;
        let padding = self.dimensions.padding;
        let (scroll_width, scroll_height) = self.scrollable_overflow();
        (
            (scroll_width - (content.width + padding.horizontal())).max(0.0),
            (scroll_height - (content.height + padding.vertical())).max(0.0),
        )
    }

    /// Whether this box clips its overflow and can therefore be scrolled.
    pub(crate) fn is_scroll_container(&self) -> bool {
        self.overflow == Overflow::Hidden
    }

    /// Returns the scroll offset actually in effect for this box: the offset
    /// stored on its element, clamped to [`Self::max_scroll_offset`].
    ///
    /// A box that does not establish a scroll container reports `(0.0, 0.0)`
    /// while leaving the stored offset alone, so it comes back if the element
    /// becomes scrollable again.
    pub(crate) fn scroll_offset(&self) -> (f32, f32) {
        if !self.is_scroll_container() {
            return (0.0, 0.0);
        }
        let (x, y) = self.node.scroll_offset();
        if (x, y) == (0.0, 0.0) {
            return (0.0, 0.0);
        }
        let (max_x, max_y) = self.max_scroll_offset();
        (x.clamp(0.0, max_x), y.clamp(0.0, max_y))
    }

    /// Computes the paint-time scroll snapshot with a single overflow walk.
    pub(crate) fn paint_scroll_geometry(&self) -> PaintScrollGeometry {
        let overflow_size = self.scrollable_overflow();
        let (stored_x, stored_y) = self.node.scroll_offset();
        let content = self.dimensions.content;
        let padding = self.dimensions.padding;
        let max_x = (overflow_size.0 - (content.width + padding.left + padding.right)).max(0.0);
        let max_y = (overflow_size.1 - (content.height + padding.top + padding.bottom)).max(0.0);
        PaintScrollGeometry {
            offset: (stored_x.clamp(0.0, max_x), stored_y.clamp(0.0, max_y)),
            overflow_size,
        }
    }
}

/// Expands `max_right` / `max_bottom` to enclose eligible descendant boxes.
/// Each axis follows its own clip chain; positioned descendants use the chain
/// rooted at their containing block.
fn expand_scrollable_overflow(
    boxes: &[LayoutBox],
    max_right: &mut f32,
    max_bottom: &mut f32,
    absolute_cb: Option<(bool, bool)>,
    fixed_cb: Option<(bool, bool)>,
) {
    expand_scrollable_overflow_axes(
        boxes,
        max_right,
        max_bottom,
        true,
        true,
        absolute_cb,
        fixed_cb,
    );
}

fn expand_scrollable_overflow_axes(
    boxes: &[LayoutBox],
    max_right: &mut f32,
    max_bottom: &mut f32,
    include_x: bool,
    include_y: bool,
    absolute_cb: Option<(bool, bool)>,
    fixed_cb: Option<(bool, bool)>,
) {
    if !include_x
        && !include_y
        && absolute_cb.is_some_and(|(x, y)| !x && !y)
        && fixed_cb.is_none_or(|(x, y)| !x && !y)
    {
        return;
    }
    for child in boxes {
        // Absolute positioning uses its containing block's clip chain. Clips
        // between that block and this child do not apply. Fixed descendants
        // anchored to the viewport never enlarge the document's scroll area.
        let (include_x, include_y) = match child.position_scheme {
            PositionScheme::Absolute => absolute_cb.unwrap_or((true, true)),
            PositionScheme::Fixed => fixed_cb.unwrap_or((false, false)),
            _ => (include_x, include_y),
        };
        if child
            .block_fragments
            .iter()
            .any(|fragment| fragment.owns_paint)
        {
            for fragment in child
                .block_fragments
                .iter()
                .filter(|fragment| fragment.owns_paint)
            {
                if include_x {
                    *max_right = max_right.max(fragment.clip.x + fragment.clip.width);
                }
                if include_y {
                    *max_bottom = max_bottom.max(fragment.clip.y + fragment.clip.height);
                }
            }
            // Fragmented descendants are painted through each fragmentainer's
            // clip. Their source-space boxes must not inflate the multicol
            // container's scrollable area after they have been redistributed.
            continue;
        }
        let content = child.dimensions.content;
        let padding = child.dimensions.padding;
        let border = child.dimensions.border;
        if include_x {
            *max_right = max_right.max(content.x + content.width + padding.right + border.right);
        }
        if include_y {
            *max_bottom =
                max_bottom.max(content.y + content.height + padding.bottom + border.bottom);
        }
        let include_child_x = include_x && !child.overflow.clips_x();
        let include_child_y = include_y && !child.overflow.clips_y();
        let absolute_cb =
            if child.position_scheme != PositionScheme::Static || child.fixed_containing_block {
                Some((include_child_x, include_child_y))
            } else {
                absolute_cb
            };
        let fixed_cb = if child.fixed_containing_block {
            Some((include_child_x, include_child_y))
        } else {
            fixed_cb
        };
        expand_line_overflow(
            &child.lines,
            max_right,
            max_bottom,
            include_child_x,
            include_child_y,
        );
        if include_child_x || include_child_y || child.has_out_of_flow_descendants != Some(false) {
            expand_scrollable_overflow_axes(
                &child.children,
                max_right,
                max_bottom,
                include_child_x,
                include_child_y,
                absolute_cb,
                fixed_cb,
            );
        }
    }
}

fn expand_line_overflow(
    lines: &[LineBox],
    max_right: &mut f32,
    max_bottom: &mut f32,
    include_x: bool,
    include_y: bool,
) {
    for line in lines {
        if include_x {
            *max_right = max_right.max(line.rect.x + line.rect.width);
        }
        if include_y {
            *max_bottom = max_bottom.max(line.rect.y + line.rect.height);
        }
    }
}

impl InlineFragment {
    /// Returns the text payload when this fragment represents text.
    pub fn text(&self) -> Option<&str> {
        match &self.content {
            InlineFragmentContent::Text(text) => Some(text.as_str()),
            _ => None,
        }
    }
}

/// Lays out a DOM subtree as block boxes inside `containing_block`.
///
/// Nodes with `display: none` are omitted from the result. Text nodes do
/// not produce standalone layout roots; text runs inside a flex container
/// can produce anonymous flex item boxes.
pub fn layout_tree(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
) -> Option<LayoutBox> {
    layout_tree_with_content_visibility(
        node,
        resolver,
        containing_block,
        ContentVisibilityLayoutInput {
            visible_rect: containing_block,
            ..ContentVisibilityLayoutInput::default()
        },
    )
    .0
}

/// Lays out a tree while tracking content-visibility decisions for the runtime.
pub(crate) fn layout_tree_with_content_visibility(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    input: ContentVisibilityLayoutInput,
) -> (Option<LayoutBox>, ContentVisibilityLayoutReport) {
    with_image_selection_environment(containing_block.width, containing_block.height, 1.0, || {
        layout_tree_with_content_visibility_inner(node, resolver, containing_block, input)
    })
}

fn layout_tree_with_content_visibility_inner(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    input: ContentVisibilityLayoutInput,
) -> (Option<LayoutBox>, ContentVisibilityLayoutReport) {
    let _phase_pass = profile::Pass::begin();
    let _tree_phase = profile::Scope::begin(profile::Phase::Tree);
    let report = Rc::new(RefCell::new(ContentVisibilityLayoutReport::default()));
    let scope = ContentVisibilityLayoutScope::new(ContentVisibilityLayoutSession {
        input,
        report: report.clone(),
    });
    let _margin_scope = margins::Scope::new();
    counters::prepare_counter_values(node, resolver);
    let Some(mut layout) = intrinsic::with_pass(|| {
        layout_node(
            node,
            resolver,
            containing_block,
            LayoutViewport::new(containing_block),
            None,
        )
    }) else {
        drop(scope);
        let snapshot = report.borrow().clone();
        return (None, snapshot);
    };
    if resolver.needs_container_contexts() {
        for _ in 0..4 {
            let mut contexts = HashMap::new();
            collect_container_contexts(&layout, resolver, &mut contexts);
            if !resolver.set_container_contexts(contexts) {
                break;
            }
            *report.borrow_mut() = ContentVisibilityLayoutReport::default();
            counters::prepare_counter_values(node, resolver);
            let Some(next_layout) = intrinsic::with_pass(|| {
                layout_node(
                    node,
                    resolver,
                    containing_block,
                    LayoutViewport::new(containing_block),
                    None,
                )
            }) else {
                drop(scope);
                let snapshot = report.borrow().clone();
                return (None, snapshot);
            };
            layout = next_layout;
        }
    }
    mark_content_visibility_skipped(&mut layout, &report.borrow().skipped_nodes);
    populate_layout_transforms(
        &mut layout,
        resolver,
        resolver.root_font_size(),
        Matrix4::identity(),
    );
    collect_content_visibility_remembered_sizes(&layout, &report, resolver, false);
    drop(scope);
    let report = report.borrow().clone();
    (Some(layout), report)
}

fn mark_content_visibility_skipped(layout: &mut LayoutBox, skipped_nodes: &HashSet<usize>) {
    layout.content_visibility_contents_skipped = skipped_nodes.contains(&layout.node.identity());
    for child in &mut layout.children {
        mark_content_visibility_skipped(child, skipped_nodes);
    }
}

fn collect_content_visibility_remembered_sizes(
    layout: &LayoutBox,
    report: &Rc<RefCell<ContentVisibilityLayoutReport>>,
    resolver: &mut StyleResolver,
    ancestor_skipped: bool,
) {
    let node_id = layout.node.identity();
    let style = layout_box_style(layout, resolver);
    let has_auto_intrinsic_size = ["contain-intrinsic-width", "contain-intrinsic-height"]
        .iter()
        .any(|property| {
            matches!(style.get(property), Some(ComputedValue::Keyword(value))
                if value.eq_ignore_ascii_case("auto")
                    || value.to_ascii_lowercase().starts_with("auto "))
        });
    let skipped = ancestor_skipped || report.borrow().skipped_nodes.contains(&node_id);
    {
        let mut report = report.borrow_mut();
        report.visited_nodes.insert(node_id);
        if has_auto_intrinsic_size {
            report.auto_intrinsic_nodes.insert(node_id);
        }
    }
    if has_auto_intrinsic_size && !skipped && !has_containment(&style, "size") {
        report.borrow_mut().remembered_sizes.insert(
            node_id,
            (
                layout.dimensions.content.width.max(0.0),
                layout.dimensions.content.height.max(0.0),
            ),
        );
    }
    for child in &layout.children {
        collect_content_visibility_remembered_sizes(child, report, resolver, skipped);
    }
}

fn populate_layout_transforms(
    layout: &mut LayoutBox,
    resolver: &mut StyleResolver,
    root_font_size: f32,
    parent_perspective: Matrix4,
) {
    let style = layout_box_style(layout, resolver);
    let perspective_box = layout.dimensions.border_box();
    let font_size = match style.get_by_id(PropertyId::FontSize) {
        Some(ComputedValue::Px(value)) => *value,
        _ => 16.0,
    };
    let local_transform = parse_composed_transform(
        &style,
        TransformReferenceBox {
            x: perspective_box.x,
            y: perspective_box.y,
            width: perspective_box.width,
            height: perspective_box.height,
            font_size,
            root_font_size,
        },
    )
    .unwrap_or_else(Matrix4::identity);
    // `perspective` is a parent effect: it projects the immediate child
    // coordinate plane while leaving the parent's own border box untouched.
    // Keeping it in the child's paint-time matrix preserves normal-flow
    // geometry and composes with the child's transform in CSS order.
    layout.transform = parent_perspective.multiply(local_transform).to_projective();
    let perspective = computed_keyword(&style, "perspective").unwrap_or("none");
    let perspective_origin = computed_keyword(&style, "perspective-origin").unwrap_or("50% 50%");
    let border_box = layout.dimensions.border_box();
    let perspective_matrix = parse_perspective_matrix(
        perspective,
        perspective_origin,
        TransformReferenceBox {
            x: border_box.x,
            y: border_box.y,
            width: border_box.width,
            height: border_box.height,
            font_size,
            root_font_size,
        },
    )
    .unwrap_or_else(Matrix4::identity);
    for child in &mut layout.children {
        populate_layout_transforms(child, resolver, root_font_size, perspective_matrix);
    }
    layout.needs_scroll_translation = matches!(
        style.get_by_id(PropertyId::Position),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("sticky")
    ) || layout
        .children
        .iter()
        .any(|child| child.needs_scroll_translation);
    layout.has_out_of_flow_descendants = Some(layout.children.iter().any(|child| {
        matches!(
            child.position_scheme,
            PositionScheme::Absolute | PositionScheme::Fixed
        ) || child.has_out_of_flow_descendants.unwrap_or(true)
    }));
}

fn computed_keyword<'a>(style: &'a ComputedStyle, property: &str) -> Option<&'a str> {
    match style.get(property) {
        Some(ComputedValue::Keyword(value)) | Some(ComputedValue::String(value)) => Some(value),
        _ => None,
    }
}

/// Whether `contain` activates one of the core containment axes.
pub(crate) fn has_containment(style: &ComputedStyle, keyword: &str) -> bool {
    let implied_by_content_visibility = match content_visibility_mode(style) {
        ContentVisibilityMode::Hidden => {
            matches!(keyword, "layout" | "style" | "paint" | "size")
        }
        ContentVisibilityMode::Auto => matches!(keyword, "layout" | "style" | "paint"),
        ContentVisibilityMode::Visible => false,
    };
    if implied_by_content_visibility {
        return true;
    }
    let Some(value) = computed_keyword(style, "contain") else {
        return false;
    };
    value.eq_ignore_ascii_case("strict")
        || (value.eq_ignore_ascii_case("content") && keyword != "size")
        || value
            .split_ascii_whitespace()
            .any(|value| value.eq_ignore_ascii_case(keyword))
}

fn has_inline_size_containment(style: &ComputedStyle) -> bool {
    has_containment(style, "size")
        || matches!(computed_keyword(style, "container-type"), Some(value)
            if value.eq_ignore_ascii_case("size") || value.eq_ignore_ascii_case("inline-size"))
}

pub(crate) fn has_block_size_containment(style: &ComputedStyle) -> bool {
    has_containment(style, "size")
        || matches!(computed_keyword(style, "container-type"), Some(value)
            if value.eq_ignore_ascii_case("size"))
}

fn collect_container_contexts(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    contexts: &mut HashMap<usize, ContainerContext>,
) {
    let style = layout_box_style(layout, resolver);
    let container_type = match style.get("container-type") {
        Some(ComputedValue::Keyword(value)) => value.to_ascii_lowercase(),
        _ => "normal".to_string(),
    };
    if layout.node.node_type() == crate::dom::NodeType::Element {
        let names = match style.get("container-name") {
            Some(ComputedValue::Keyword(value)) if !value.eq_ignore_ascii_case("none") => {
                value.split_whitespace().map(str::to_string).collect()
            }
            _ => Vec::new(),
        };
        contexts.insert(
            layout.node.identity(),
            ContainerContext {
                width: layout.dimensions.content.width,
                height: layout.dimensions.content.height,
                container_type,
                names,
                units: resolver.container_unit_context(&style),
                style: resolver.container_style_snapshot(&style),
            },
        );
    }
    for child in &layout.children {
        collect_container_contexts(child, resolver, contexts);
    }
}

/// A flex-assigned content height. A used size can remain indefinite for
/// descendant percentages, for example with only a container min-height.
#[derive(Debug, Clone, Copy)]
struct UsedHeight {
    value: f32,
    definite: bool,
}

impl UsedHeight {
    fn percentage_basis(self) -> Option<f32> {
        self.definite.then_some(self.value)
    }
}

// Keep the viewport separate from the nearest fixed-position containing block.
// This context follows the layout traversal, including through static wrappers.
#[derive(Clone, Copy)]
struct LayoutViewport {
    rect: Rect,
    fixed: Option<Rect>,
}

impl LayoutViewport {
    fn new(rect: Rect) -> Self {
        Self { rect, fixed: None }
    }

    fn after_sizing(
        self,
        style: &ComputedStyle,
        dimensions: BoxDimensions,
        children: &mut [LayoutBox],
        resolver: &mut StyleResolver,
    ) -> Self {
        let final_context = self.for_descendants(style, dimensions);
        // Fixed boxes do not contribute to normal-flow sizing. An auto-sized
        // containing block can therefore resolve their final percentage sizes
        // and bottom/right insets once its in-flow children have been laid out.
        // Definite, unchanged containing blocks require no additional walk.
        if final_context.fixed != self.fixed {
            relayout_fixed_descendants(children, resolver, final_context);
        }
        final_context
    }

    fn for_descendants(self, style: &ComputedStyle, dimensions: BoxDimensions) -> Self {
        if establishes_fixed_containing_block(style) {
            Self {
                fixed: Some(dimensions.padding_box()),
                ..self
            }
        } else {
            self
        }
    }
}

fn layout_node(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
) -> Option<LayoutBox> {
    layout_node_with_subgrid(
        node,
        resolver,
        containing_block,
        viewport,
        positioned_ancestor,
        None,
    )
}

fn layout_node_with_subgrid(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    subgrid: Option<grid::SubgridContext>,
) -> Option<LayoutBox> {
    if node.top_layer_order().is_some() {
        return None;
    }
    match node.node_type() {
        NodeType::Document => layout_document(
            node,
            resolver,
            containing_block,
            viewport,
            positioned_ancestor,
        ),
        NodeType::Element => layout_element(
            node,
            resolver,
            containing_block,
            viewport,
            positioned_ancestor,
            subgrid,
            None,
        ),
        _ => None,
    }
}

#[derive(Clone)]
enum LayoutSource {
    Node(NodeHandle),
    Generated {
        host: NodeHandle,
        pseudo: PseudoElement,
    },
}

impl LayoutSource {
    fn from_layout_box(layout: &LayoutBox) -> Self {
        layout.pseudo.map_or_else(
            || Self::Node(layout.node.clone()),
            |pseudo| Self::Generated {
                host: layout.node.clone(),
                pseudo,
            },
        )
    }

    fn node(&self) -> &NodeHandle {
        match self {
            Self::Node(node) => node,
            Self::Generated { host, .. } => host,
        }
    }

    fn style(&self, resolver: &mut StyleResolver) -> Option<ComputedStyle> {
        match self {
            Self::Node(node) if node.node_type() == NodeType::Element => {
                Some(resolver.computed_style(node))
            }
            Self::Generated { host, pseudo } => generated_pseudo_style(host, resolver, *pseudo),
            Self::Node(_) => None,
        }
    }

    fn layout(
        &self,
        resolver: &mut StyleResolver,
        containing_block: Rect,
        viewport: LayoutViewport,
        positioned_ancestor: Option<BoxDimensions>,
    ) -> Option<LayoutBox> {
        match self {
            Self::Node(node) => layout_node(
                node,
                resolver,
                containing_block,
                viewport,
                positioned_ancestor,
            ),
            Self::Generated { host, pseudo } => {
                let style = generated_pseudo_style(host, resolver, *pseudo)?;
                Some(layout_generated_pseudo_box(
                    host,
                    *pseudo,
                    resolver,
                    &style,
                    containing_block,
                    viewport,
                    positioned_ancestor,
                ))
            }
        }
    }
}

fn block_flow_sources(node: &NodeHandle, resolver: &mut StyleResolver) -> Vec<LayoutSource> {
    let mut sources = Vec::new();
    if generated_block_pseudo_style(node, resolver, PseudoElement::Before).is_some() {
        sources.push(LayoutSource::Generated {
            host: node.clone(),
            pseudo: PseudoElement::Before,
        });
    }
    sources.extend(
        node.layout_child_nodes()
            .into_iter()
            .map(LayoutSource::Node),
    );
    if generated_block_pseudo_style(node, resolver, PseudoElement::After).is_some() {
        sources.push(LayoutSource::Generated {
            host: node.clone(),
            pseudo: PseudoElement::After,
        });
    }
    sources
}

fn layout_document(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
) -> Option<LayoutBox> {
    if let Some(frameset) = node.query_selector("frameset") {
        return Some(frameset::layout_frameset_document(
            node,
            &frameset,
            resolver,
            containing_block,
        ));
    }
    let mut children = Vec::new();
    let mut positioned_children = Vec::new();
    let mut cursor_y = containing_block.y;
    let mut previous_margin_bottom: Option<f32> = None;

    for child in node.layout_child_nodes() {
        let child_style = match child.node_type() {
            NodeType::Element => Some(resolver.computed_style(&child)),
            _ => None,
        };
        let child_margin_top = child_style
            .as_ref()
            .map(|style| edge_sizes(style, "margin").top)
            .unwrap_or(0.0);
        let collapse_delta = previous_margin_bottom
            .map(|margin_bottom| {
                margin_bottom + child_margin_top - collapse_margins(margin_bottom, child_margin_top)
            })
            .unwrap_or(0.0);
        let child_containing = Rect {
            x: containing_block.x,
            y: cursor_y - collapse_delta,
            width: containing_block.width,
            height: containing_block.height,
        };
        if let Some(style) = &child_style
            && is_out_of_flow_positioned(style)
        {
            positioned_children.push((child, style.clone(), child_containing));
            continue;
        }

        if let Some(layout_child) = layout_node(
            &child,
            resolver,
            child_containing,
            viewport,
            positioned_ancestor,
        ) {
            cursor_y += layout_child.total_height();
            previous_margin_bottom = Some(layout_child.dimensions.margin.bottom);
            children.push(layout_child);
        }
    }

    let dimensions = BoxDimensions {
        content: Rect {
            x: containing_block.x,
            y: containing_block.y,
            width: containing_block.width,
            height: cursor_y - containing_block.y,
        },
        ..BoxDimensions::default()
    };

    let initial_containing_block = BoxDimensions {
        content: viewport.rect,
        ..BoxDimensions::default()
    };
    for (child, style, static_position) in positioned_children {
        if let Some(positioned) = layout_positioned_child(
            &child,
            resolver,
            &style,
            positioned_ancestor.unwrap_or(initial_containing_block),
            static_position,
            viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index_in_tree_order(node, &mut children);

    let mut top_layer = Vec::new();
    collect_top_layer_nodes(node, &mut top_layer);
    top_layer.sort_by_key(|element| element.top_layer_order().unwrap_or_default());
    for element in top_layer {
        let style = resolver.computed_style(&element);
        if let Some(mut layout) = layout_positioned_child_mode(
            &element,
            resolver,
            &style,
            initial_containing_block,
            viewport.rect,
            LayoutViewport::new(viewport.rect),
            true,
        ) {
            layout.z_index = i32::MAX;
            children.push(layout);
        }
    }

    Some(LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions,
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        position_scheme: PositionScheme::Static,
        fixed_containing_block: false,
        z_index: 0,
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines: Vec::new(),
        children,
        marker: None,
    })
}

fn collect_top_layer_nodes(node: &NodeHandle, result: &mut Vec<NodeHandle>) {
    if node.top_layer_order().is_some() {
        result.push(node.clone());
    }
    for child in node.child_nodes() {
        collect_top_layer_nodes(&child, result);
    }
    if let Some(shadow_root) = node.shadow_root() {
        collect_top_layer_nodes(&shadow_root, result);
    }
}

/// Returns `true` when all nodes are whitespace-only text.
fn all_whitespace_only(nodes: &[NodeHandle]) -> bool {
    nodes.iter().all(|n| {
        n.node_type() == NodeType::Text
            && n.data()
                .map(|t| {
                    t.bytes()
                        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0C'))
                })
                .unwrap_or(true)
    })
}

/// Flushes pending inline nodes into line boxes, advancing cursor_y.
/// Clears `pending` after processing.
fn flush_pending_inline_nodes(
    pending: &mut Vec<NodeHandle>,
    generated_owner: &NodeHandle,
    include_before: bool,
    include_after: bool,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    float_regions: &[FloatRegion],
    cursor_y: &mut f32,
    x: f32,
    width: f32,
    containing_height: f32,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    lines: &mut Vec<LineBox>,
    children: &mut Vec<LayoutBox>,
) {
    if !include_before && !include_after && (pending.is_empty() || all_whitespace_only(pending)) {
        pending.clear();
        return;
    }
    let line_constraints = |line_y: f32, line_height: f32| {
        let offsets = float_offsets_for_line(float_regions, line_y, line_height, x, width);
        (
            x + offsets.left,
            (width - offsets.left - offsets.right).max(0.0),
        )
    };
    let inline::InlineLayoutResult {
        lines: inline_lines,
        atomic_boxes,
    } = layout_inline_nodes_with_line_constraints(
        pending,
        resolver,
        x,
        *cursor_y,
        width,
        text_align(style),
        line_height(style),
        direction_is_rtl(style),
        (text_overflow_is_ellipsis(style) && overflow(style).clips_x()).then_some(style),
        containing_height,
        viewport,
        positioned_ancestor,
        true,
        Some(generated_owner),
        include_before,
        include_after,
        &line_constraints,
    );
    if let Some(last_line) = inline_lines.last() {
        *cursor_y = last_line.rect.y + last_line.rect.height;
    }
    lines.extend(inline_lines);
    children.extend(atomic_boxes);
    pending.clear();
}

/// Applies CSS `clear` to cursor_y, pushing past any interfering floats.
fn apply_clear(
    cursor_y: &mut f32,
    child_style: &ComputedStyle,
    child_margin_top: f32,
    collapse_delta: f32,
    float_regions: &[FloatRegion],
) {
    match clear_side(child_style) {
        ClearSide::Left => {
            *cursor_y = clear_cursor_y_for_side(
                *cursor_y,
                child_margin_top,
                collapse_delta,
                float_regions,
                FloatSide::Left,
            );
        }
        ClearSide::Right => {
            *cursor_y = clear_cursor_y_for_side(
                *cursor_y,
                child_margin_top,
                collapse_delta,
                float_regions,
                FloatSide::Right,
            );
        }
        ClearSide::Both => {
            *cursor_y = clear_cursor_y_for_side(
                *cursor_y,
                child_margin_top,
                collapse_delta,
                float_regions,
                FloatSide::Left,
            );
            *cursor_y = clear_cursor_y_for_side(
                *cursor_y,
                child_margin_top,
                collapse_delta,
                float_regions,
                FloatSide::Right,
            );
        }
        ClearSide::None => {}
    }
}

/// Lays out a floated child, finding a suitable vertical position and
/// registering the float region. Returns `true` when the child was
/// consumed (always; provided for control flow clarity).
fn layout_float_child(
    source: &LayoutSource,
    child_style: &ComputedStyle,
    resolver: &mut StyleResolver,
    side: FloatSide,
    child_y: f32,
    x: f32,
    width: f32,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    float_regions: &mut Vec<FloatRegion>,
    children: &mut Vec<LayoutBox>,
) {
    let available = (width
        - active_float_offsets(float_regions, child_y, x, width).left
        - active_float_offsets(float_regions, child_y, x, width).right)
        .max(0.0);
    let float_width = resolved_length(child_style, "width", available)
        .unwrap_or_else(|| shrink_to_fit_width(source.node(), resolver, width));
    let mut float_y = child_y;
    loop {
        let offsets = active_float_offsets(float_regions, float_y, x, width);
        let float_available_width = (width - offsets.left - offsets.right).max(0.0);
        let float_containing = Rect {
            x: x + offsets.left,
            y: float_y,
            width: float_available_width.max(float_width),
            height: 0.0,
        };
        if let Some(mut layout_child) =
            source.layout(resolver, float_containing, viewport, positioned_ancestor)
        {
            // Float placement uses the margin box. In particular, a negative
            // margin can make a specified-width float fit beside an earlier
            // float (a common legacy two-column layout technique).
            if layout_child.total_width() <= float_available_width + 0.5 {
                if resolved_length(child_style, "width", float_available_width).is_none() {
                    layout_child.dimensions.content.width = float_width;
                }
                let outer_y = float_containing.y;
                let outer_x = match side {
                    FloatSide::Left => x + offsets.left,
                    FloatSide::Right => x + width - offsets.right - layout_child.total_width(),
                    FloatSide::None => x + offsets.left,
                };
                translate_layout_box_to_outer(&mut layout_child, outer_x, outer_y, resolver);
                let shape =
                    shapes::ShapeOutside::from_style(child_style, layout_child.dimensions, width);
                float_regions.push(FloatRegion {
                    outer: Rect {
                        x: outer_x,
                        y: outer_y,
                        width: layout_child.total_width(),
                        height: layout_child.total_height(),
                    },
                    side,
                    shape,
                });
                children.push(layout_child);
                break;
            }
        }
        let Some(next_y) = next_float_boundary_after(float_regions, float_y) else {
            break;
        };
        if next_y <= float_y {
            break;
        }
        float_y = next_y;
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_vertical_float_child(
    source: &LayoutSource,
    child_style: &ComputedStyle,
    resolver: &mut StyleResolver,
    side: FloatSide,
    cursor_x: &mut f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    vertical_rl: bool,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    float_regions: &mut Vec<FloatRegion>,
    children: &mut Vec<LayoutBox>,
) {
    loop {
        let remaining_width = if vertical_rl {
            (*cursor_x - x).max(0.0)
        } else {
            (x + width - *cursor_x).max(0.0)
        };
        let provisional_x = if vertical_rl {
            *cursor_x - remaining_width
        } else {
            *cursor_x
        };
        let containing = Rect {
            x: provisional_x,
            y,
            width: remaining_width,
            height,
        };
        let Some(mut layout_child) =
            source.layout(resolver, containing, viewport, positioned_ancestor)
        else {
            return;
        };
        let outer_width = layout_child.total_width();
        let outer_height = layout_child.total_height();
        let outer_x = if vertical_rl {
            *cursor_x - outer_width
        } else {
            *cursor_x
        };
        let offsets = active_vertical_float_offsets(float_regions, outer_x, outer_width, y, height);
        let available_height = (height - offsets.top - offsets.bottom).max(0.0);
        if outer_height > available_height + 0.5
            && let Some(next_x) = next_vertical_float_boundary(
                float_regions,
                *cursor_x,
                outer_x,
                outer_width,
                vertical_rl,
            )
        {
            if next_x == *cursor_x {
                return;
            }
            *cursor_x = next_x;
            continue;
        }

        let outer_y = match side {
            FloatSide::Left => y + offsets.top,
            FloatSide::Right => (y + height - offsets.bottom - outer_height).max(y),
            FloatSide::None => y + offsets.top,
        };
        translate_layout_box_to_outer(&mut layout_child, outer_x, outer_y, resolver);
        let shape = shapes::ShapeOutside::from_style(child_style, layout_child.dimensions, height);
        float_regions.push(FloatRegion {
            outer: Rect {
                x: outer_x,
                y: outer_y,
                width: outer_width,
                height: outer_height,
            },
            side,
            shape,
        });
        children.push(layout_child);
        return;
    }
}

/// Computes the containing block for a child element, accounting for float offsets.
fn child_containing_rect(
    child_style: &ComputedStyle,
    child_y: f32,
    offsets: &FloatOffsets,
    x: f32,
    width: f32,
    height: f32,
) -> Rect {
    let has_explicit_width = explicit_length(child_style, "width").is_some();
    Rect {
        x: if has_explicit_width {
            x
        } else {
            x + offsets.left
        },
        y: child_y,
        width: if has_explicit_width {
            width
        } else {
            (width - offsets.left - offsets.right).max(0.0)
        },
        height,
    }
}

/// Re-distributes auto margins for shrink-to-fit table containers.
fn redistribute_auto_margins_for_table(
    style: &ComputedStyle,
    width: f32,
    padding: &EdgeSizes,
    border: &EdgeSizes,
    margin: &mut EdgeSizes,
    containing_width: f32,
) {
    if float_side(style) != FloatSide::None {
        return;
    }
    let outer = width + padding.horizontal() + border.horizontal();
    let remaining = (containing_width - outer).max(0.0);
    let left_auto = margin_start_is_auto(style);
    let right_auto = margin_end_is_auto(style);
    match (left_auto, right_auto) {
        (true, true) => {
            margin.left = remaining / 2.0;
            margin.right = remaining / 2.0;
        }
        (true, false) => margin.left = (remaining - margin.right).max(0.0),
        (false, true) => margin.right = (remaining - margin.left).max(0.0),
        _ => {}
    }
}

fn layout_element(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    subgrid: Option<grid::SubgridContext>,
    used_height: Option<UsedHeight>,
) -> Option<LayoutBox> {
    let _phase = profile::Scope::begin(profile::Phase::Element);
    layout_element_with_cell(
        node,
        resolver,
        containing_block,
        viewport,
        positioned_ancestor,
        subgrid,
        used_height,
        None,
        containing_block.width,
        containing_block.width,
    )
}

/// Lays out an allocated item while retaining its actual percentage basis.
fn layout_element_with_percentage_basis(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: LayoutViewport,
    subgrid: Option<grid::SubgridContext>,
    used_height: Option<UsedHeight>,
    percentage_basis: f32,
) -> Option<LayoutBox> {
    layout_element_with_cell(
        node,
        resolver,
        containing_block,
        viewport,
        None,
        subgrid,
        used_height,
        None,
        percentage_basis,
        containing_block.width,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentVisibilityMode {
    Visible,
    Auto,
    Hidden,
}

fn content_visibility_mode(style: &ComputedStyle) -> ContentVisibilityMode {
    match style.get("content-visibility") {
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("auto") => {
            ContentVisibilityMode::Auto
        }
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("hidden") => {
            ContentVisibilityMode::Hidden
        }
        _ => ContentVisibilityMode::Visible,
    }
}

fn content_visibility_layout_is_skipped(
    node: &NodeHandle,
    style: &ComputedStyle,
    content_rect: Rect,
    padding: EdgeSizes,
    border: EdgeSizes,
) -> bool {
    let mode = content_visibility_mode(style);
    if mode == ContentVisibilityMode::Visible {
        return false;
    }
    CONTENT_VISIBILITY_LAYOUT.with(|current| {
        let current = current.borrow();
        let Some(session) = current.as_ref() else {
            return mode == ContentVisibilityMode::Hidden;
        };
        let node_id = node.identity();
        if mode == ContentVisibilityMode::Auto {
            session.report.borrow_mut().auto_nodes.insert(node_id);
        }
        let relevant =
            mode == ContentVisibilityMode::Auto && session.input.relevant_nodes.contains(&node_id);
        let semantic_skip = match mode {
            ContentVisibilityMode::Visible => false,
            ContentVisibilityMode::Hidden => true,
            ContentVisibilityMode::Auto => {
                !relevant
                    && !content_visibility_box_is_close(
                        content_rect,
                        padding,
                        border,
                        session.input.visible_rect,
                    )
            }
        };
        if semantic_skip {
            session.report.borrow_mut().skipped_nodes.insert(node_id);
        }
        semantic_skip && !session.input.forced_nodes.contains(&node_id)
    })
}

fn content_visibility_box_is_close(
    content_rect: Rect,
    padding: EdgeSizes,
    border: EdgeSizes,
    visible_rect: Rect,
) -> bool {
    let box_rect = Rect {
        x: content_rect.x - padding.left - border.left,
        y: content_rect.y - padding.top - border.top,
        width: content_rect.width + padding.horizontal() + border.horizontal(),
        height: content_rect.height + padding.vertical() + border.vertical(),
    };
    // CSS Containment recommends a user-agent margin of 50% of the viewport.
    let proximity = Rect {
        x: visible_rect.x - visible_rect.width * 0.5,
        y: visible_rect.y - visible_rect.height * 0.5,
        width: visible_rect.width * 2.0,
        height: visible_rect.height * 2.0,
    };
    box_rect.x <= proximity.x + proximity.width
        && box_rect.x + box_rect.width >= proximity.x
        && box_rect.y <= proximity.y + proximity.height
        && box_rect.y + box_rect.height >= proximity.y
}

fn contain_intrinsic_axis_size(
    node: &NodeHandle,
    style: &ComputedStyle,
    property: &str,
    remembered_axis: usize,
) -> f32 {
    let mut use_remembered = false;
    let fallback = match style.get(property) {
        Some(ComputedValue::Px(value)) => Some(*value),
        Some(ComputedValue::Number(value)) if *value == 0.0 => Some(0.0),
        Some(computed @ ComputedValue::LengthPercentage(_))
            if computed
                .linear_length_percentage_components()
                .is_some_and(|(_, percentage)| percentage == 0.0) =>
        {
            computed.resolve_length_percentage(0.0)
        }
        Some(ComputedValue::Keyword(value)) => {
            let value = value.trim().to_ascii_lowercase();
            if let Some(fallback) = value.strip_prefix("auto ") {
                use_remembered = true;
                if fallback == "none" {
                    None
                } else {
                    fallback
                        .strip_suffix("px")
                        .and_then(|value| value.trim().parse::<f32>().ok())
                }
            } else {
                None
            }
        }
        _ => None,
    }
    .filter(|value| value.is_finite() && *value >= 0.0);

    if use_remembered
        && let Some(size) = CONTENT_VISIBILITY_LAYOUT.with(|current| {
            current.borrow().as_ref().and_then(|session| {
                session
                    .input
                    .remembered_sizes
                    .get(&node.identity())
                    .copied()
            })
        })
    {
        return if remembered_axis == 0 { size.0 } else { size.1 };
    }
    fallback.unwrap_or(0.0)
}

#[allow(clippy::too_many_arguments)]
fn skipped_content_visibility_box(
    node: &NodeHandle,
    style: &ComputedStyle,
    containing_height: f32,
    x: f32,
    y: f32,
    width: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: EdgeSizes,
    used_height: Option<UsedHeight>,
    resolver: &mut StyleResolver,
) -> LayoutBox {
    let width = if resolved_length(style, "width", width).is_none()
        && (matches!(computed_keyword(style, "display"), Some(value)
            if value.eq_ignore_ascii_case("inline-block"))
            || float_side(style) != FloatSide::None)
    {
        contain_intrinsic_axis_size(node, style, "contain-intrinsic-width", 0)
    } else {
        width
    };
    let placeholder = contain_intrinsic_axis_size(node, style, "contain-intrinsic-height", 1);
    let content_height = used_height.map(|height| height.value).unwrap_or_else(|| {
        let explicit = resolved_length(style, "height", containing_height)
            .map(|height| border_box_adjust_height(style, height, &padding, &border));
        clamp_content_height(
            style,
            explicit.unwrap_or(placeholder),
            containing_height,
            padding,
            border,
        )
    });
    let mut layout = LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height: content_height,
            },
            padding,
            border,
            margin,
        },
        visibility: visibility(style),
        overflow: overflow(style),
        position_scheme: position_scheme(style),
        fixed_containing_block: establishes_fixed_containing_block(style),
        z_index: z_index(style),
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines: Vec::new(),
        children: Vec::new(),
        marker: None,
    };
    apply_relative_offset(&mut layout, style, resolver);
    layout
}

fn pseudo_generates_content(style: &ComputedStyle) -> bool {
    match style.get("content") {
        None => false,
        Some(ComputedValue::Keyword(value)) => {
            !value.eq_ignore_ascii_case("none") && !value.eq_ignore_ascii_case("normal")
        }
        Some(_) => true,
    }
}

fn generated_inline_pseudo_exists(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    pseudo: PseudoElement,
) -> bool {
    resolver
        .computed_pseudo_style(node, pseudo)
        .is_some_and(|style| {
            !style.is_display_none()
                && pseudo_generates_content(&style)
                && !is_out_of_flow_positioned(&style)
                && float_side(&style) == FloatSide::None
                && matches!(
                    style.get("display"),
                    Some(ComputedValue::Keyword(value))
                        if value.eq_ignore_ascii_case("inline")
                            || value.eq_ignore_ascii_case("inline-block")
                )
        })
}

fn generated_pseudo_style(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    pseudo: PseudoElement,
) -> Option<ComputedStyle> {
    let style = resolver.computed_pseudo_style(node, pseudo)?;
    if style.is_display_none() || !pseudo_generates_content(&style) {
        return None;
    }
    Some(style)
}

fn generated_block_pseudo_style(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    pseudo: PseudoElement,
) -> Option<ComputedStyle> {
    let style = generated_pseudo_style(node, resolver, pseudo)?;
    (is_out_of_flow_positioned(&style)
        || float_side(&style) != FloatSide::None
        || matches!(
            style.get("display"),
            Some(ComputedValue::Keyword(value))
                if matches!(
                    value.to_ascii_lowercase().as_str(),
                    "block" | "flow-root" | "flex" | "grid" | "table" | "list-item"
                )
        ))
    .then_some(style)
}

#[allow(clippy::too_many_arguments)]
fn layout_generated_pseudo_box(
    node: &NodeHandle,
    pseudo: PseudoElement,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    containing_block: Rect,
    _viewport: LayoutViewport,
    _positioned_ancestor: Option<BoxDimensions>,
) -> LayoutBox {
    let padding = edge_sizes_with_basis(style, "padding", containing_block.width);
    let border = edge_sizes(style, "border");
    let mut margin = edge_sizes_with_basis(style, "margin", containing_block.width);
    let width = compute_width(
        node,
        resolver,
        style,
        containing_block.width,
        containing_block.width,
        padding,
        border,
        &mut margin,
    );
    let x = containing_block.x + margin.left + border.left + padding.left;
    let y = containing_block.y + margin.top + border.top + padding.top;
    let lines = inline::layout_block_pseudo_content(node, resolver, pseudo, style, x, y, width);
    let cursor_y = lines
        .last()
        .map(|line| line.rect.y + line.rect.height)
        .unwrap_or(y);
    let content_height = resolve_content_height(
        node,
        style,
        containing_block.height,
        padding,
        border,
        y,
        cursor_y,
    );
    let mut layout = LayoutBox {
        node: node.clone(),
        pseudo: Some(pseudo),
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height: content_height,
            },
            padding,
            border,
            margin,
        },
        visibility: visibility(style),
        overflow: overflow(style),
        position_scheme: position_scheme(style),
        fixed_containing_block: establishes_fixed_containing_block(style),
        z_index: z_index(style),
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines,
        children: Vec::new(),
        marker: None,
    };
    apply_relative_offset(&mut layout, style, resolver);
    layout
}

// Table tracks assign the cell's border-box width, independently of its width
// hint. Collapsed borders contribute half of the shared edge to that box.
fn layout_element_with_cell(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    subgrid: Option<grid::SubgridContext>,
    used_height: Option<UsedHeight>,
    table_cell: Option<EdgeSizes>,
    percentage_basis: f32,
    width_percentage_basis: f32,
) -> Option<LayoutBox> {
    margins::forget(node);
    if is_non_rendered_html_element(node) {
        return None;
    }

    let style = resolver.computed_style(node);
    if style.is_display_none() {
        return None;
    }

    let config = unsupported_html_config();
    if (config.logging_enabled || config.sqlite_path.is_some())
        && let Some(tag) = node.tag_name()
    {
        let parent_tag = node.parent_node().and_then(|p| p.tag_name());
        log_unsupported_html_tag(&tag, parent_tag.as_deref());
    }

    let metrics = resolve_cell_box_metrics(
        node,
        resolver,
        &style,
        containing_block,
        table_cell,
        used_height,
        percentage_basis,
        width_percentage_basis,
    );
    let CellBoxMetrics {
        padding,
        border,
        margin,
        width,
        used_height,
        x,
        y,
    } = metrics;

    let viewport =
        cell_fixed_containing_viewport(node, &style, containing_block, viewport, metrics);

    if let Some(skipped) =
        skip_cell_for_content_visibility(node, &style, containing_block, metrics, resolver)
    {
        return Some(skipped);
    }

    if let Some(layout) = layout_replaced_media_cell(
        node,
        resolver,
        &style,
        containing_block,
        viewport,
        positioned_ancestor,
        metrics,
    ) {
        return Some(layout);
    }

    if let Some(layout) = layout_form_control_cell(
        node,
        resolver,
        &style,
        containing_block,
        viewport,
        positioned_ancestor,
        metrics,
    ) {
        return Some(layout);
    }

    if is_table_container_element(node, &style) {
        return layout_table_cell_container(
            node,
            resolver,
            style,
            containing_block,
            viewport,
            metrics,
        );
    }

    if is_flex_container(&style) {
        return layout_flex_container(
            node,
            resolver,
            style,
            margin,
            padding,
            border,
            x,
            y,
            width,
            viewport,
            containing_block.height,
            used_height,
        );
    }
    if is_grid_container(&style) {
        return layout_grid_container(
            node,
            resolver,
            style,
            margin,
            padding,
            border,
            x,
            y,
            width,
            containing_block.height,
            viewport,
            subgrid,
            used_height,
        );
    }

    let (dimensions, children, lines, multicol, line_clamped) = layout_block_cell_body(
        node,
        resolver,
        &style,
        containing_block,
        viewport,
        positioned_ancestor,
        metrics,
    );

    let marker = build_list_marker(node, &style, dimensions.content.x, dimensions.content.y);
    let mut layout = LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions,
        visibility: visibility(&style),
        overflow: if line_clamped {
            Overflow::Clip
        } else {
            overflow(&style)
        },
        position_scheme: position_scheme(&style),
        fixed_containing_block: establishes_fixed_containing_block(&style),
        z_index: z_index(&style),
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol,
        lines,
        children,
        marker,
    };
    apply_relative_offset(&mut layout, &style, resolver);
    Some(layout)
}

/// The padding/border/margin edges, used width and height, and content
/// origin resolved for a table-cell-aware element, before any of the
/// element-specific layout paths below run.
#[derive(Clone, Copy)]
struct CellBoxMetrics {
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: EdgeSizes,
    width: f32,
    used_height: Option<UsedHeight>,
    x: f32,
    y: f32,
}

/// Resolves this cell's padding/border/margin edges, used width, and content
/// origin from its computed style and the table/float context that
/// `layout_element_with_cell` was called with. Table tracks assign the
/// cell's border-box width directly, overriding both the computed width and
/// margins. `percentage_basis` applies to margin/padding; width and min/max
/// constraints use `width_percentage_basis` so allocated flex/grid widths do
/// not accidentally resolve again against their original container.
fn resolve_cell_box_metrics(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    containing_block: Rect,
    table_cell: Option<EdgeSizes>,
    used_height: Option<UsedHeight>,
    percentage_basis: f32,
    width_percentage_basis: f32,
) -> CellBoxMetrics {
    let padding = edge_sizes_with_basis(style, "padding", percentage_basis);
    let border = table_cell.unwrap_or_else(|| edge_sizes(style, "border"));
    let mut margin = if table_cell.is_some() {
        EdgeSizes::default()
    } else {
        edge_sizes_with_basis(style, "margin", percentage_basis)
    };
    let used_height = used_height.map(|height| UsedHeight {
        value: clamp_content_height(
            style,
            height.value,
            containing_block.height,
            padding,
            border,
        ),
        ..height
    });

    let mut width = compute_width(
        node,
        resolver,
        style,
        containing_block.width,
        width_percentage_basis,
        padding,
        border,
        &mut margin,
    );
    if table_cell.is_some() {
        margin = EdgeSizes::default();
        width = (containing_block.width - padding.horizontal() - border.horizontal()).max(0.0);
    }
    if float_side(style) != FloatSide::None
        && resolved_length(style, "width", containing_block.width).is_none()
    {
        width = shrink_to_fit_width(node, resolver, containing_block.width);
    }
    let x = containing_block.x + margin.left + border.left + padding.left;
    let y = containing_block.y + margin.top + border.top + padding.top;

    CellBoxMetrics {
        padding,
        border,
        margin,
        width,
        used_height,
        x,
        y,
    }
}

/// Narrows the viewport to this box's own dimensions when it establishes a
/// containing block for fixed-position descendants.
fn cell_fixed_containing_viewport(
    node: &NodeHandle,
    style: &ComputedStyle,
    containing_block: Rect,
    viewport: LayoutViewport,
    metrics: CellBoxMetrics,
) -> LayoutViewport {
    let CellBoxMetrics {
        padding,
        border,
        margin,
        x,
        y,
        width,
        used_height,
    } = metrics;
    if !establishes_fixed_containing_block(style) {
        return viewport;
    }
    viewport.for_descendants(
        style,
        BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height: used_height.map(|height| height.value).unwrap_or_else(|| {
                    resolve_content_height(
                        node,
                        style,
                        containing_block.height,
                        padding,
                        border,
                        y,
                        y,
                    )
                }),
            },
            padding,
            border,
            margin,
        },
    )
}

/// Returns the placeholder layout box for this element if content-visibility
/// causes its subtree layout to be skipped.
fn skip_cell_for_content_visibility(
    node: &NodeHandle,
    style: &ComputedStyle,
    containing_block: Rect,
    metrics: CellBoxMetrics,
    resolver: &mut StyleResolver,
) -> Option<LayoutBox> {
    let CellBoxMetrics {
        padding,
        border,
        margin,
        x,
        y,
        width,
        used_height,
    } = metrics;
    if content_visibility_mode(style) == ContentVisibilityMode::Visible {
        return None;
    }
    let skipped = skipped_content_visibility_box(
        node,
        style,
        containing_block.height,
        x,
        y,
        width,
        padding,
        border,
        margin,
        used_height,
        resolver,
    );
    content_visibility_layout_is_skipped(node, style, skipped.dimensions.content, padding, border)
        .then_some(skipped)
}

fn is_replaced_media_element(node: &NodeHandle) -> bool {
    node.with_tag_name(|tag| {
        matches!(
            tag,
            Some("img" | "picture" | "video" | "canvas" | "svg" | "object")
        )
    })
}

fn is_form_control_layout_element(node: &NodeHandle) -> bool {
    node.with_tag_name(|tag| {
        matches!(
            tag,
            Some("input" | "textarea" | "select" | "button" | "progress" | "meter")
        )
    })
}

fn has_replaced_layout_semantics(node: &NodeHandle) -> bool {
    // Keep the engine's media/form-control sizing paths and the remaining
    // replaced-like defaults out of the non-replaced absolute equation.
    // This predicate is tag based so image decode state cannot change sizing.
    is_replaced_media_element(node)
        || is_form_control_layout_element(node)
        || node.with_tag_name(|tag| matches!(tag, Some("audio" | "embed" | "frame" | "iframe")))
}

/// Lays out `img`/`picture`/`video`/`canvas`/`svg`/`object` elements that
/// participate as block or flex/grid items, so they still paint their image
/// payload even though the parent isn't collecting inline image fragments.
/// Previously only inline formatting collected image fragments, so
/// `display: block` SVGs (a common Tailwind reset) had a box but rendered
/// none of their graphics. Returns `None` when this element isn't a
/// replaced-media element, or has no image fragments to paint, so the
/// caller can fall through to the next layout path.
fn layout_replaced_media_cell(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    metrics: CellBoxMetrics,
) -> Option<LayoutBox> {
    let CellBoxMetrics {
        padding,
        border,
        margin,
        x,
        y,
        width,
        used_height,
    } = metrics;
    let is_positioned_img = node.has_tag_name("img") && is_out_of_flow_positioned(style);
    if is_positioned_img || !is_replaced_media_element(node) {
        return None;
    }

    let mut lines = layout_inline_nodes(
        std::slice::from_ref(node),
        resolver,
        x,
        y,
        width,
        text_align(style),
        0.0,
        direction_is_rtl(style),
        None,
        containing_block.height,
        viewport,
        positioned_ancestor,
        false,
    )
    .lines;
    if !lines.iter().any(|line| {
        line.fragments
            .iter()
            .any(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
    }) {
        lines.clear();
    }
    if lines.is_empty() {
        return None;
    }

    let percentage_width = matches!(style.get("width"), Some(ComputedValue::Percentage(_)));
    for line in &mut lines {
        for fragment in &mut line.fragments {
            if matches!(fragment.content, InlineFragmentContent::Image(_, _))
                && fragment.rect.width > 0.0
                && width > 0.0
                && (percentage_width || fragment.rect.width > width)
            {
                let scale = width / fragment.rect.width;
                fragment.rect.width = width;
                fragment.rect.height *= scale;
            }
        }
        if let Some(height) = line
            .fragments
            .iter()
            .map(|fragment| fragment.rect.height)
            .reduce(f32::max)
        {
            line.rect.width = width;
            line.rect.height = height;
            line.baseline = height;
        }
    }
    let cursor_y = lines
        .last()
        .map(|line| line.rect.y + line.rect.height)
        .unwrap_or(y);
    let content_height = used_height.map(|height| height.value).unwrap_or_else(|| {
        resolve_content_height(
            node,
            style,
            containing_block.height,
            padding,
            border,
            y,
            cursor_y,
        )
    });
    let mut layout = LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions: BoxDimensions {
            content: Rect {
                x,
                y,
                width,
                height: content_height,
            },
            padding,
            border,
            margin,
        },
        visibility: visibility(style),
        overflow: overflow(style),
        position_scheme: position_scheme(style),
        fixed_containing_block: establishes_fixed_containing_block(style),
        z_index: z_index(style),
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines,
        children: Vec::new(),
        marker: None,
    };
    apply_relative_offset(&mut layout, style, resolver);
    Some(layout)
}

/// Blockification must retain a form control's value/selection fragment,
/// even when the parent's inline formatting context never visits it.
/// Returns `None` when this element isn't a form control, or has no
/// rendered fragment, so the caller can fall through to the next layout
/// path.
fn layout_form_control_cell(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    metrics: CellBoxMetrics,
) -> Option<LayoutBox> {
    let CellBoxMetrics {
        padding,
        border,
        margin,
        x,
        y,
        width,
        used_height,
    } = metrics;
    if !is_form_control_layout_element(node) {
        return None;
    }

    let mut lines = layout_inline_nodes(
        std::slice::from_ref(node),
        resolver,
        x - padding.left - border.left,
        y - padding.top - border.top,
        width + padding.left + padding.right + border.left + border.right,
        inline::TextAlign::Left,
        0.0,
        direction_is_rtl(style),
        None,
        containing_block.height,
        viewport,
        positioned_ancestor,
        false,
    )
    .lines;
    let fragment = lines.first().and_then(|line| line.fragments.first())?;
    let intrinsic_height =
        (fragment.rect.height - padding.top - padding.bottom - border.top - border.bottom).max(0.0);
    let content_height = used_height.map(|height| height.value).unwrap_or_else(|| {
        resolve_content_height(
            node,
            style,
            containing_block.height,
            padding,
            border,
            y,
            y + intrinsic_height,
        )
    });
    let dimensions = BoxDimensions {
        content: Rect {
            x,
            y,
            width,
            height: content_height,
        },
        padding,
        border,
        margin,
    };
    let border_box = dimensions.border_box();
    for line in &mut lines {
        line.rect = border_box;
        for fragment in &mut line.fragments {
            fragment.rect = border_box;
        }
    }
    let mut layout = LayoutBox {
        node: node.clone(),
        pseudo: None,
        dimensions,
        visibility: visibility(style),
        overflow: overflow(style),
        position_scheme: position_scheme(style),
        fixed_containing_block: establishes_fixed_containing_block(style),
        z_index: z_index(style),
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        has_out_of_flow_descendants: None,
        content_visibility_contents_skipped: false,
        paint_scroll: None,
        block_fragments: Vec::new(),
        multicol: None,
        lines,
        children: Vec::new(),
        marker: None,
    };
    apply_relative_offset(&mut layout, style, resolver);
    Some(layout)
}

/// Lays out this element as a table wrapper/table box, shrinking the box to
/// fit its column tracks first when no explicit width was specified.
fn layout_table_cell_container(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: ComputedStyle,
    containing_block: Rect,
    viewport: LayoutViewport,
    metrics: CellBoxMetrics,
) -> Option<LayoutBox> {
    let CellBoxMetrics {
        padding,
        border,
        mut margin,
        y,
        mut width,
        used_height,
        ..
    } = metrics;
    let is_shrink_to_fit = resolved_length(&style, "width", containing_block.width).is_none();
    if is_shrink_to_fit {
        width = shrink_to_fit_width(node, resolver, containing_block.width);
        redistribute_auto_margins_for_table(
            &style,
            width,
            &padding,
            &border,
            &mut margin,
            containing_block.width,
        );
    }
    let x = containing_block.x + margin.left + border.left + padding.left;
    layout_table_container(
        node,
        resolver,
        style,
        margin,
        padding,
        border,
        x,
        y,
        width,
        viewport,
        is_shrink_to_fit,
        used_height,
    )
}

/// Lays out this element's ordinary block-level children, applies the
/// margin-collapsing correction now that the box's own margins are known,
/// resolves out-of-flow positioned descendants against the final
/// dimensions, and z-orders the resulting children. Returns the resolved
/// dimensions together with the final children, lines, and multicol
/// metadata.
fn layout_block_cell_body(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    containing_block: Rect,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    metrics: CellBoxMetrics,
) -> (
    BoxDimensions,
    Vec<LayoutBox>,
    Vec<LineBox>,
    Option<MultiColumnLayout>,
    bool,
) {
    let CellBoxMetrics {
        padding,
        border,
        mut margin,
        x,
        y,
        width,
        used_height,
    } = metrics;
    let BlockChildrenResult {
        mut children,
        mut lines,
        cursor_y,
        float_bottom,
        mut positioned_children,
        margin_info,
        child_shifts,
        multicol,
    } = layout_block_children(
        node,
        resolver,
        style,
        padding,
        border,
        margin,
        x,
        y,
        width,
        containing_block.height,
        viewport,
        positioned_ancestor,
        used_height,
    );

    let margin_delta = margin_info.map_or(0.0, |info| {
        let delta = info.top.value() - margin.top;
        margin.top = info.top.value();
        margin.bottom = info.bottom.value();
        margins::remember(node, info);
        delta
    });
    let mut shifts = child_shifts.into_iter().peekable();
    for (index, child) in children.iter_mut().enumerate() {
        let correction = if shifts
            .peek()
            .is_some_and(|(child_index, _)| *child_index == index)
        {
            shifts.next().unwrap().1
        } else {
            0.0
        };
        margins::shift_flow(
            child,
            correction + margin_delta,
            resolver,
            establishes_positioned_containing_block(style),
            establishes_fixed_containing_block(style),
        );
    }
    margins::shift_lines(&mut lines, margin_delta);
    for (_, _, static_position) in &mut positioned_children {
        static_position.y += margin_delta;
    }
    let y = y + margin_delta;
    let mut effective_cursor_y = cursor_y.max(float_bottom) + margin_delta;
    let line_clamped =
        line_clamp_point(node, style, &lines, &children, resolver).is_some_and(|point| {
            effective_cursor_y = effective_cursor_y.min(point.block_end);
            let paint = line_clamp_paint_context(
                node,
                style,
                Rect {
                    x,
                    y,
                    width,
                    height: resolved_length(style, "height", containing_block.height)
                        .map(|height| border_box_adjust_height(style, height, &padding, &border))
                        .or_else(|| {
                            used_height
                                .map(|height| height.value)
                                .filter(|height| *height > 0.0)
                        })
                        .unwrap_or(containing_block.height),
                },
                &children,
                &point.target,
                resolver,
            );
            if let Some(marker) = block_ellipsis_text(&paint.style)
                && let Some(line) = line_clamp_target_mut(&mut lines, &mut children, &point.target)
            {
                inline::apply_block_ellipsis(
                    line,
                    &paint.owner,
                    paint.start,
                    paint.width,
                    direction_is_rtl(&paint.style),
                    &paint.style,
                    &marker,
                );
            }
            true
        });
    let content_height = used_height.map(|height| height.value).unwrap_or_else(|| {
        resolve_content_height(
            node,
            style,
            containing_block.height,
            padding,
            border,
            y,
            effective_cursor_y,
        )
    });

    let dimensions = BoxDimensions {
        content: Rect {
            x,
            y,
            width,
            height: content_height,
        },
        padding,
        border,
        margin,
    };

    let viewport = viewport.after_sizing(style, dimensions, &mut children, resolver);

    // Resolve positioned children using the final dimensions (content_height is
    // now known, which is required for absolute positioning relative to this box).
    let next_pos_ancestor = if establishes_positioned_containing_block(style) {
        Some(dimensions)
    } else {
        positioned_ancestor
    };
    for (source, cs, static_position) in positioned_children {
        if let Some(positioned) = layout_positioned_source(
            &source,
            resolver,
            &cs,
            next_pos_ancestor.unwrap_or(BoxDimensions {
                content: viewport.rect,
                ..BoxDimensions::default()
            }),
            static_position,
            viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index_in_tree_order(node, &mut children);

    (dimensions, children, lines, multicol, line_clamped)
}

fn line_clamp_point(
    node: &NodeHandle,
    style: &ComputedStyle,
    lines: &[LineBox],
    children: &[LayoutBox],
    resolver: &mut StyleResolver,
) -> Option<LineClampCandidate> {
    let max_lines = line_clamp_max_lines(style)?;
    let mut ranks = HashMap::new();
    let mut next_rank = 0;
    collect_descendant_ranks(node, &mut ranks, &mut next_rank);
    let mut line_ends = lines
        .iter()
        .enumerate()
        .map(|(index, line)| line_clamp_candidate(line, node, &ranks, LineClampTarget::Root(index)))
        .collect::<Vec<_>>();
    for (index, child) in children.iter().enumerate() {
        collect_in_flow_line_ends(child, resolver, &ranks, &mut vec![index], &mut line_ends);
    }
    line_ends.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.source_end.cmp(&right.source_end))
    });
    (line_ends.len() > max_lines).then(|| line_ends[max_lines - 1].clone())
}

#[derive(Clone)]
struct LineClampCandidate {
    order: usize,
    source_end: usize,
    block_end: f32,
    target: LineClampTarget,
}

#[derive(Clone)]
enum LineClampTarget {
    Root(usize),
    Descendant { path: Vec<usize>, line: usize },
}

fn collect_descendant_ranks(
    node: &NodeHandle,
    ranks: &mut HashMap<usize, usize>,
    next: &mut usize,
) {
    ranks.insert(node.identity(), *next);
    *next += 1;
    for child in node.layout_child_nodes() {
        collect_descendant_ranks(&child, ranks, next);
    }
}

fn line_clamp_candidate(
    line: &LineBox,
    fallback_owner: &NodeHandle,
    ranks: &HashMap<usize, usize>,
    target: LineClampTarget,
) -> LineClampCandidate {
    let order = line
        .fragments
        .iter()
        .flat_map(|fragment| {
            fragment
                .text_source
                .iter()
                .map(|source| &source.node)
                .chain(std::iter::once(&fragment.node))
        })
        .filter_map(|node| ranks.get(&node.identity()).copied())
        .min()
        .or_else(|| ranks.get(&fallback_owner.identity()).copied())
        .unwrap_or(usize::MAX);
    LineClampCandidate {
        order,
        source_end: line
            .fragments
            .iter()
            .map(|fragment| fragment.source_end_token)
            .max()
            .unwrap_or_default(),
        block_end: line.rect.y + line.rect.height,
        target,
    }
}

fn line_clamp_max_lines(style: &ComputedStyle) -> Option<usize> {
    let Some(ComputedValue::Keyword(continuation)) = style.get_by_id(PropertyId::Continue) else {
        return None;
    };
    if !continuation.eq_ignore_ascii_case("collapse")
        && !continuation.eq_ignore_ascii_case("-webkit-legacy")
    {
        return None;
    }
    let Some(ComputedValue::Keyword(max_lines)) = style.get_by_id(PropertyId::MaxLines) else {
        return None;
    };
    max_lines
        .split_ascii_whitespace()
        .find_map(|component| component.parse::<usize>().ok())
        .filter(|lines| *lines > 0)
}

fn block_ellipsis_text(style: &ComputedStyle) -> Option<String> {
    let Some(ComputedValue::Keyword(value)) = style.get_by_id(PropertyId::BlockEllipsis) else {
        return None;
    };
    if value.eq_ignore_ascii_case("no-ellipsis") {
        return None;
    }
    if value.eq_ignore_ascii_case("ellipsis") {
        return Some("…".to_string());
    }
    let tokens = tokenize(value).ok()?;
    match tokens.as_slice() {
        [CssToken::String(value)] => Some(value.clone()),
        _ => None,
    }
}

fn line_clamp_target_mut<'a>(
    lines: &'a mut [LineBox],
    children: &'a mut [LayoutBox],
    target: &LineClampTarget,
) -> Option<&'a mut LineBox> {
    match target {
        LineClampTarget::Root(index) => lines.get_mut(*index),
        LineClampTarget::Descendant { path, line } => {
            let mut layout = children.get_mut(*path.first()?)?;
            for index in &path[1..] {
                layout = layout.children.get_mut(*index)?;
            }
            layout.lines.get_mut(*line)
        }
    }
}

struct LineClampPaintContext {
    owner: NodeHandle,
    style: ComputedStyle,
    start: f32,
    width: f32,
}

fn line_clamp_paint_context(
    node: &NodeHandle,
    style: &ComputedStyle,
    content: Rect,
    children: &[LayoutBox],
    target: &LineClampTarget,
    resolver: &mut StyleResolver,
) -> LineClampPaintContext {
    let root_context = || {
        let vertical = is_vertical_writing(style);
        LineClampPaintContext {
            owner: node.clone(),
            style: style.clone(),
            start: if vertical { content.y } else { content.x },
            width: if vertical {
                content.height
            } else {
                content.width
            },
        }
    };
    let LineClampTarget::Descendant { path, .. } = target else {
        return root_context();
    };
    let Some(mut layout) = path.first().and_then(|index| children.get(*index)) else {
        return root_context();
    };
    for index in &path[1..] {
        let Some(child) = layout.children.get(*index) else {
            return root_context();
        };
        layout = child;
    }
    let target_style = layout_box_style(layout, resolver);
    let vertical = is_vertical_writing(&target_style);
    LineClampPaintContext {
        owner: layout.node.clone(),
        style: target_style,
        start: if vertical {
            layout.dimensions.content.y
        } else {
            layout.dimensions.content.x
        },
        width: if vertical {
            layout.dimensions.content.height
        } else {
            layout.dimensions.content.width
        },
    }
}

fn collect_in_flow_line_ends(
    layout: &LayoutBox,
    resolver: &mut StyleResolver,
    ranks: &HashMap<usize, usize>,
    path: &mut Vec<usize>,
    line_ends: &mut Vec<LineClampCandidate>,
) {
    if matches!(
        layout.position_scheme,
        PositionScheme::Absolute | PositionScheme::Fixed
    ) {
        return;
    }
    let style = layout_box_style(layout, resolver);
    if establishes_independent_line_formatting_context(&style) {
        return;
    }
    line_ends.extend(layout.lines.iter().enumerate().map(|(index, line)| {
        line_clamp_candidate(
            line,
            &layout.node,
            ranks,
            LineClampTarget::Descendant {
                path: path.clone(),
                line: index,
            },
        )
    }));
    for (index, child) in layout.children.iter().enumerate() {
        path.push(index);
        collect_in_flow_line_ends(child, resolver, ranks, path, line_ends);
        path.pop();
    }
}

fn establishes_independent_line_formatting_context(style: &ComputedStyle) -> bool {
    float_side(style) != FloatSide::None
        || is_out_of_flow_positioned(style)
        || overflow(style) != Overflow::Visible
        || is_flex_container(style)
        || is_grid_container(style)
        || has_containment(style, "layout")
        || matches!(
            computed_keyword(style, "display"),
            Some(display) if display.eq_ignore_ascii_case("flow-root")
                || display.eq_ignore_ascii_case("inline-block")
                || display.eq_ignore_ascii_case("table")
        )
}

struct BlockChildrenResult {
    children: Vec<LayoutBox>,
    lines: Vec<LineBox>,
    cursor_y: f32,
    float_bottom: f32,
    positioned_children: Vec<(LayoutSource, ComputedStyle, Rect)>,
    margin_info: Option<margins::Info>,
    child_shifts: Vec<(usize, f32)>,
    multicol: Option<MultiColumnLayout>,
}

/// Lays out block-level children, returning in-flow children, lines,
/// cursor position, float bottom, and deferred positioned children.
fn layout_block_children(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: EdgeSizes,
    x: f32,
    y: f32,
    width: f32,
    containing_height: f32,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    used_height: Option<UsedHeight>,
) -> BlockChildrenResult {
    if multicol::is_multicol_container(style) {
        return multicol::layout_multicol_children(
            node,
            resolver,
            style,
            padding,
            border,
            margin,
            x,
            y,
            width,
            containing_height,
            viewport,
            positioned_ancestor,
            used_height,
        );
    }
    if is_vertical_writing(style) {
        return layout_vertical_block_children(
            node,
            resolver,
            style,
            padding,
            border,
            margin,
            x,
            y,
            width,
            containing_height,
            viewport,
            positioned_ancestor,
            used_height,
        );
    }

    margins::layout_children(
        node,
        resolver,
        style,
        padding,
        border,
        margin,
        x,
        y,
        width,
        containing_height,
        viewport,
        positioned_ancestor,
        used_height,
    )
}

/// Block formatting context for vertical writing modes.
///
/// In vertical writing the inline axis is physical y and the block axis is
/// physical x.  The normal horizontal implementation remains the source of
/// truth for margins, positioning and nested horizontal subtrees; this path
/// only changes the sibling cursor and delegates inline content to the
/// transposed inline formatter.
fn layout_vertical_block_children(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: EdgeSizes,
    x: f32,
    y: f32,
    width: f32,
    containing_height: f32,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    used_height: Option<UsedHeight>,
) -> BlockChildrenResult {
    let vertical_rl = is_vertical_rl(style);
    let mut children = Vec::new();
    let mut positioned_children = Vec::new();
    let mut lines = Vec::new();
    let mut pending_inline_nodes = Vec::new();
    let mut include_inline_before =
        generated_inline_pseudo_exists(node, resolver, PseudoElement::Before);
    let include_inline_after = generated_inline_pseudo_exists(node, resolver, PseudoElement::After);
    let mut float_regions = Vec::new();
    let mut cursor_x = if vertical_rl { x + width } else { x };
    let mut inline_bottom = y;
    // An auto-height root often enters layout with a zero containing height.
    // Vertical inline layout still needs a finite line-breaking basis; use an
    // explicit height when present, otherwise a generous unbounded basis so
    // content can establish the auto height naturally.
    let available_inline_height = used_height
        .and_then(UsedHeight::percentage_basis)
        .or_else(|| {
            resolved_length(style, "height", containing_height)
                .map(|height| border_box_adjust_height(style, height, &padding, &border))
        })
        .or_else(|| (containing_height > 0.0).then_some(containing_height))
        .unwrap_or(1_000_000.0);

    for source in block_flow_sources(node, resolver) {
        let child = source.node().clone();
        if child.node_type() == NodeType::Comment {
            continue;
        }
        if matches!(&source, LayoutSource::Node(_)) && is_inline_child(&child, resolver) {
            pending_inline_nodes.push(child);
            continue;
        }

        flush_pending_vertical_inline_nodes(
            &mut pending_inline_nodes,
            node,
            std::mem::take(&mut include_inline_before),
            false,
            resolver,
            style,
            &float_regions,
            x,
            y,
            width,
            available_inline_height,
            viewport,
            positioned_ancestor,
            &mut cursor_x,
            vertical_rl,
            &mut lines,
            &mut inline_bottom,
            &mut children,
        );

        let child_style = source.style(resolver);
        let Some(child_style) = child_style else {
            continue;
        };

        apply_vertical_clear(&mut cursor_x, &child_style, &float_regions, vertical_rl);

        // Positioned descendants are resolved after the containing block's
        // final dimensions are known, just as in the horizontal path.
        let estimated_width = resolved_length(&child_style, "width", width)
            .unwrap_or_else(|| line_height(&child_style))
            .max(0.01)
            .min(width);
        let provisional_x = if vertical_rl {
            cursor_x - estimated_width
        } else {
            cursor_x
        };
        let offsets = active_vertical_float_offsets(
            &float_regions,
            provisional_x,
            estimated_width,
            y,
            available_inline_height,
        );
        let has_explicit_height = explicit_length(&child_style, "height").is_some();
        let child_containing = Rect {
            x: provisional_x,
            y: if has_explicit_height {
                y
            } else {
                y + offsets.top
            },
            width: if vertical_rl {
                (cursor_x - x).max(0.0)
            } else {
                (x + width - cursor_x).max(0.0)
            },
            height: if has_explicit_height {
                available_inline_height
            } else {
                (available_inline_height - offsets.top - offsets.bottom).max(0.0)
            },
        };
        if is_out_of_flow_positioned(&child_style) {
            positioned_children.push((source, child_style, child_containing));
            continue;
        }

        let next_pos_ancestor = if establishes_positioned_containing_block(style) {
            Some(BoxDimensions {
                content: Rect {
                    x,
                    y,
                    width,
                    height: 0.0,
                },
                padding,
                border,
                margin,
            })
        } else {
            positioned_ancestor
        };
        let side = float_side(&child_style);
        if side != FloatSide::None {
            layout_vertical_float_child(
                &source,
                &child_style,
                resolver,
                side,
                &mut cursor_x,
                x,
                y,
                width,
                available_inline_height,
                vertical_rl,
                viewport,
                next_pos_ancestor,
                &mut float_regions,
                &mut children,
            );
            continue;
        }
        let Some(mut layout_child) =
            source.layout(resolver, child_containing, viewport, next_pos_ancestor)
        else {
            continue;
        };

        let outer_x = if vertical_rl {
            cursor_x - layout_child.total_width()
        } else {
            cursor_x
        };
        translate_layout_box_to_outer(&mut layout_child, outer_x, child_containing.y, resolver);
        if vertical_rl {
            cursor_x = outer_x;
        } else {
            cursor_x = outer_x + layout_child.total_width();
        }
        inline_bottom = inline_bottom.max(child_containing.y + layout_child.total_height());
        children.push(layout_child);
    }

    flush_pending_vertical_inline_nodes(
        &mut pending_inline_nodes,
        node,
        include_inline_before,
        include_inline_after,
        resolver,
        style,
        &float_regions,
        x,
        y,
        width,
        available_inline_height,
        viewport,
        positioned_ancestor,
        &mut cursor_x,
        vertical_rl,
        &mut lines,
        &mut inline_bottom,
        &mut children,
    );
    sort_children_by_z_index_in_tree_order(node, &mut children);

    BlockChildrenResult {
        children,
        lines,
        // The vertical formatter uses cursor_y as the inline-axis extent so
        // the existing height resolver can still apply explicit/min/max
        // height declarations without a second sizing pipeline.
        cursor_y: inline_bottom,
        float_bottom: float_regions
            .iter()
            .map(|region| region.outer.y + region.outer.height)
            .fold(inline_bottom, f32::max),
        positioned_children,
        margin_info: None,
        child_shifts: Vec::new(),
        multicol: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn flush_pending_vertical_inline_nodes(
    pending: &mut Vec<NodeHandle>,
    generated_owner: &NodeHandle,
    include_before: bool,
    include_after: bool,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    float_regions: &[FloatRegion],
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    viewport: LayoutViewport,
    positioned_ancestor: Option<BoxDimensions>,
    cursor_x: &mut f32,
    vertical_rl: bool,
    lines: &mut Vec<LineBox>,
    inline_bottom: &mut f32,
    children: &mut Vec<LayoutBox>,
) {
    if !include_before && !include_after && (pending.is_empty() || all_whitespace_only(pending)) {
        pending.clear();
        return;
    }

    let (region_x, region_width) = if vertical_rl {
        (x, (*cursor_x - x).max(0.0))
    } else {
        (*cursor_x, (x + width - *cursor_x).max(0.0))
    };
    let inline::InlineLayoutResult {
        lines: inline_lines,
        atomic_boxes,
    } = {
        let column_constraints = |column_x: f32, column_width: f32| {
            let offsets =
                vertical_float_offsets_for_column(float_regions, column_x, column_width, y, height);
            (
                y + offsets.top,
                (height - offsets.top - offsets.bottom).max(0.0),
            )
        };
        layout_vertical_inline_nodes(
            pending,
            resolver,
            region_x,
            y,
            region_width,
            height,
            text_align(style),
            line_height(style),
            vertical_rl,
            direction_is_rtl(style),
            (text_overflow_is_ellipsis(style) && overflow(style).clips_y()).then_some(style),
            width,
            viewport,
            positioned_ancestor,
            Some(generated_owner),
            include_before,
            include_after,
            Some(&column_constraints),
        )
    };
    if let Some(last_line) = inline_lines
        .iter()
        .map(|line| line.rect.y + line.rect.height)
        .reduce(f32::max)
    {
        *inline_bottom = (*inline_bottom).max(last_line);
    }
    let used_width = inline_lines.iter().map(|line| line.rect.width).sum::<f32>();
    if vertical_rl {
        *cursor_x = (*cursor_x - used_width).max(x);
    } else {
        *cursor_x = (*cursor_x + used_width).min(x + width);
    }
    lines.extend(inline_lines);
    children.extend(atomic_boxes);
    pending.clear();
}

/// Resolves the content height for a block element, considering explicit height,
/// min/max constraints, border-box mode, and auto height from content.
///
/// `cursor_y` is the bottom edge of all content (including floats).
fn resolve_content_height(
    node: &NodeHandle,
    style: &ComputedStyle,
    containing_height: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    y: f32,
    cursor_y: f32,
) -> f32 {
    let border_box = is_border_box(style);
    let pb_vertical = padding.vertical() + border.vertical();
    let auto_height = if has_block_size_containment(style) {
        contain_intrinsic_axis_size(node, style, "contain-intrinsic-height", 1)
    } else if node.has_tag_name("iframe") {
        150.0
    } else {
        (cursor_y - y).max(0.0)
    };
    let height = resolved_length(style, "height", containing_height)
        .map(|h| {
            if border_box {
                (h - pb_vertical).max(0.0)
            } else {
                h
            }
        })
        .unwrap_or(auto_height);
    clamp_content_height(style, height, containing_height, padding, border)
}

/// Applies min/max-height in content-box coordinates to a resolved height.
fn clamp_content_height(
    style: &ComputedStyle,
    mut height: f32,
    containing_height: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
) -> f32 {
    let border_box = is_border_box(style);
    let pb_vertical = padding.vertical() + border.vertical();
    let (min_h, max_h) =
        normalized_min_max_lengths(style, "min-height", "max-height", containing_height);
    if let Some(min_h) = min_h {
        let content_min = if border_box {
            (min_h - pb_vertical).max(0.0)
        } else {
            min_h
        };
        height = height.max(content_min);
    }
    if let Some(max_h) = max_h {
        let content_max = if border_box {
            (max_h - pb_vertical).max(0.0)
        } else {
            max_h
        };
        height = height.min(content_max);
    }
    height
}

// ── Width computation ───────────────────────────────────────────────────────

/// Returns `true` when `box-sizing: border-box` is set on the element.
pub(crate) fn is_border_box(style: &ComputedStyle) -> bool {
    matches!(
        style.get("box-sizing"),
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("border-box")
    )
}

/// Convert a specified height value from border-box to content-box if needed.
/// Returns the content height after subtracting padding and border for border-box elements.
pub(crate) fn border_box_adjust_height(
    style: &ComputedStyle,
    specified: f32,
    padding: &EdgeSizes,
    border: &EdgeSizes,
) -> f32 {
    border_box_adjust_length(
        style,
        specified,
        padding.top + border.top,
        padding.bottom + border.bottom,
    )
}

/// Converts a length that applies to the box selected by `box-sizing` into a
/// content-box length. Callers supply the decorations on the relevant axis.
pub(crate) fn border_box_adjust_length(
    style: &ComputedStyle,
    specified: f32,
    start_decoration: f32,
    end_decoration: f32,
) -> f32 {
    if is_border_box(style) {
        (specified - start_decoration - end_decoration).max(0.0)
    } else {
        specified
    }
}

#[derive(Default)]
struct IntrinsicContentWidths {
    minimum: Option<f32>,
    maximum: Option<f32>,
}

impl IntrinsicContentWidths {
    fn minimum(
        &mut self,
        node: &NodeHandle,
        resolver: &mut StyleResolver,
        decorations: f32,
    ) -> f32 {
        *self.minimum.get_or_insert_with(|| {
            (minimum_content_width_for_sizing(node, resolver) - decorations).max(0.0)
        })
    }

    fn maximum(
        &mut self,
        node: &NodeHandle,
        resolver: &mut StyleResolver,
        decorations: f32,
    ) -> f32 {
        *self.maximum.get_or_insert_with(|| {
            (intrinsic_width_for_sizing(node, resolver) - decorations).max(0.0)
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn resolved_content_width(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    property: &str,
    containing_width: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    stretch_fit_width: f32,
    intrinsic: &mut IntrinsicContentWidths,
) -> Option<f32> {
    if let Some(width) = resolved_length(style, property, containing_width) {
        return Some(border_box_adjust_length(
            style,
            width,
            padding.left + border.left,
            padding.right + border.right,
        ));
    }

    let Some(ComputedValue::Keyword(keyword)) = style.get(property) else {
        return None;
    };
    let decorations = padding.horizontal() + border.horizontal();
    if keyword.eq_ignore_ascii_case("min-content") {
        Some(intrinsic.minimum(node, resolver, decorations))
    } else if keyword.eq_ignore_ascii_case("max-content") {
        Some(intrinsic.maximum(node, resolver, decorations))
    } else if keyword.eq_ignore_ascii_case("fit-content") {
        let minimum = intrinsic.minimum(node, resolver, decorations);
        let maximum = intrinsic.maximum(node, resolver, decorations);
        Some(maximum.min(minimum.max(stretch_fit_width)))
    } else if keyword.eq_ignore_ascii_case("stretch") {
        Some(stretch_fit_width)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_width(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    available_width: f32,
    percentage_basis: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: &mut EdgeSizes,
) -> f32 {
    let pb_horizontal = padding.horizontal() + border.horizontal();
    let stretch_fit_width = (available_width - pb_horizontal - margin.horizontal()).max(0.0);
    let mut intrinsic = IntrinsicContentWidths::default();

    // `specified_width` is the value given to the `width` property.
    // For border-box, that value already includes padding + border, so we
    // convert it to a content-box width immediately.
    let specified_width = resolved_content_width(
        node,
        resolver,
        style,
        "width",
        percentage_basis,
        padding,
        border,
        stretch_fit_width,
        &mut intrinsic,
    );
    let margin_left_auto = margin_start_is_auto(style);
    let margin_right_auto = margin_end_is_auto(style);

    let mut width = if let Some(width) = specified_width {
        let remaining = (available_width - width - pb_horizontal).max(0.0);

        match (margin_left_auto, margin_right_auto) {
            (true, true) => {
                margin.left = remaining / 2.0;
                margin.right = remaining / 2.0;
            }
            (true, false) => {
                margin.left = (remaining - margin.right).max(0.0);
            }
            (false, true) => {
                margin.right = (remaining - margin.left).max(0.0);
            }
            (false, false) => {}
        }

        width
    } else {
        if margin_left_auto {
            margin.left = 0.0;
        }
        if margin_right_auto {
            margin.right = 0.0;
        }

        if node.has_tag_name("iframe") {
            300.0
        } else {
            (available_width - pb_horizontal - margin.horizontal()).max(0.0)
        }
    };

    // For border-box, min-width / max-width also refer to the outer (border)
    // box, so subtract padding + border before comparing.
    let min_width = resolved_content_width(
        node,
        resolver,
        style,
        "min-width",
        percentage_basis,
        padding,
        border,
        stretch_fit_width,
        &mut intrinsic,
    );
    let max_width = resolved_content_width(
        node,
        resolver,
        style,
        "max-width",
        percentage_basis,
        padding,
        border,
        stretch_fit_width,
        &mut intrinsic,
    );
    let (min_width, max_width) = match (min_width, max_width) {
        (Some(minimum), Some(maximum)) if minimum > maximum => (Some(minimum), Some(minimum)),
        pair => pair,
    };
    if let Some(min_width) = min_width {
        width = width.max(min_width);
    }
    if let Some(max_width) = max_width {
        width = width.min(max_width);
    }

    if margin_left_auto || margin_right_auto {
        let remaining = (available_width - width - pb_horizontal).max(0.0);
        match (margin_left_auto, margin_right_auto) {
            (true, true) => {
                margin.left = remaining / 2.0;
                margin.right = remaining / 2.0;
            }
            (true, false) => {
                margin.left = (remaining - margin.right).max(0.0);
            }
            (false, true) => {
                margin.right = (remaining - margin.left).max(0.0);
            }
            (false, false) => {}
        }
    }

    width
}

fn normalized_min_max_lengths(
    style: &ComputedStyle,
    min_name: &str,
    max_name: &str,
    containing_length: f32,
) -> (Option<f32>, Option<f32>) {
    let min = resolved_length(style, min_name, containing_length);
    let max = resolved_length(style, max_name, containing_length);
    match (min, max) {
        (Some(min), Some(max)) if min > max => (Some(min), Some(min)),
        pair => pair,
    }
}

// ── Float helpers ───────────────────────────────────────────────────────────

fn active_float_offsets(regions: &[FloatRegion], y: f32, x: f32, width: f32) -> FloatOffsets {
    let mut offsets = FloatOffsets::default();
    for region in regions {
        if y < region.outer.y || y >= region.outer.y + region.outer.height {
            continue;
        }
        match region.side {
            FloatSide::Left => {
                offsets.left = offsets
                    .left
                    .max((region.outer.x + region.outer.width - x).max(0.0));
            }
            FloatSide::Right => {
                let right_edge = x + width;
                offsets.right = offsets.right.max((right_edge - region.outer.x).max(0.0));
            }
            FloatSide::None => {}
        }
    }
    offsets
}

/// Computes float exclusions for one complete line band. A `shape-outside`
/// changes this exclusion only; float placement and `clear` continue to use
/// the rectangular margin box through `active_float_offsets`.
fn float_offsets_for_line(
    regions: &[FloatRegion],
    y: f32,
    line_height: f32,
    x: f32,
    width: f32,
) -> FloatOffsets {
    let mut offsets = FloatOffsets::default();
    let band_bottom = y + line_height.max(0.01);
    for region in regions {
        let bounds = if let Some(shape) = &region.shape {
            shape.horizontal_bounds(y, line_height)
        } else if band_bottom > region.outer.y && y < region.outer.y + region.outer.height {
            Some((region.outer.x, region.outer.x + region.outer.width))
        } else {
            None
        };
        let Some((min_x, max_x)) = bounds else {
            continue;
        };
        match region.side {
            FloatSide::Left => {
                offsets.left = offsets.left.max((max_x - x).clamp(0.0, width));
            }
            FloatSide::Right => {
                offsets.right = offsets.right.max((x + width - min_x).clamp(0.0, width));
            }
            FloatSide::None => {}
        }
    }
    offsets
}

fn active_vertical_float_offsets(
    regions: &[FloatRegion],
    x: f32,
    column_width: f32,
    y: f32,
    height: f32,
) -> VerticalFloatOffsets {
    let mut offsets = VerticalFloatOffsets::default();
    let column_right = x + column_width.max(0.01);
    for region in regions {
        if column_right <= region.outer.x || x >= region.outer.x + region.outer.width {
            continue;
        }
        match region.side {
            FloatSide::Left => {
                offsets.top = offsets
                    .top
                    .max((region.outer.y + region.outer.height - y).clamp(0.0, height));
            }
            FloatSide::Right => {
                offsets.bottom = offsets
                    .bottom
                    .max((y + height - region.outer.y).clamp(0.0, height));
            }
            FloatSide::None => {}
        }
    }
    offsets
}

/// Computes float exclusions for one vertical line (column) band. Rectangle
/// placement and `clear` use [`active_vertical_float_offsets`]; a
/// `shape-outside` only changes the inline-axis space available to text.
fn vertical_float_offsets_for_column(
    regions: &[FloatRegion],
    x: f32,
    column_width: f32,
    y: f32,
    height: f32,
) -> VerticalFloatOffsets {
    let mut offsets = VerticalFloatOffsets::default();
    for region in regions {
        let bounds = if let Some(shape) = &region.shape {
            shape.vertical_bounds(x, column_width)
        } else if x + column_width.max(0.01) > region.outer.x
            && x < region.outer.x + region.outer.width
        {
            Some((region.outer.y, region.outer.y + region.outer.height))
        } else {
            None
        };
        let Some((min_y, max_y)) = bounds else {
            continue;
        };
        match region.side {
            FloatSide::Left => {
                offsets.top = offsets.top.max((max_y - y).clamp(0.0, height));
            }
            FloatSide::Right => {
                offsets.bottom = offsets.bottom.max((y + height - min_y).clamp(0.0, height));
            }
            FloatSide::None => {}
        }
    }
    offsets
}

fn vertical_clear_cursor_for_side(
    cursor_x: f32,
    regions: &[FloatRegion],
    side: FloatSide,
    vertical_rl: bool,
) -> f32 {
    regions
        .iter()
        .filter(|region| region.side == side)
        .fold(cursor_x, |cursor, region| {
            if vertical_rl {
                cursor.min(region.outer.x)
            } else {
                cursor.max(region.outer.x + region.outer.width)
            }
        })
}

fn apply_vertical_clear(
    cursor_x: &mut f32,
    child_style: &ComputedStyle,
    regions: &[FloatRegion],
    vertical_rl: bool,
) {
    match clear_side(child_style) {
        ClearSide::Left => {
            *cursor_x =
                vertical_clear_cursor_for_side(*cursor_x, regions, FloatSide::Left, vertical_rl);
        }
        ClearSide::Right => {
            *cursor_x =
                vertical_clear_cursor_for_side(*cursor_x, regions, FloatSide::Right, vertical_rl);
        }
        ClearSide::Both => {
            *cursor_x =
                vertical_clear_cursor_for_side(*cursor_x, regions, FloatSide::Left, vertical_rl);
            *cursor_x =
                vertical_clear_cursor_for_side(*cursor_x, regions, FloatSide::Right, vertical_rl);
        }
        ClearSide::None => {}
    }
}

fn next_vertical_float_boundary(
    regions: &[FloatRegion],
    cursor_x: f32,
    band_x: f32,
    band_width: f32,
    vertical_rl: bool,
) -> Option<f32> {
    let band_right = band_x + band_width;
    let boundaries = regions.iter().filter_map(|region| {
        let overlaps = band_right > region.outer.x && band_x < region.outer.x + region.outer.width;
        if !overlaps {
            return None;
        }
        let boundary = if vertical_rl {
            region.outer.x
        } else {
            region.outer.x + region.outer.width
        };
        ((vertical_rl && boundary < cursor_x) || (!vertical_rl && boundary > cursor_x))
            .then_some(boundary)
    });
    if vertical_rl {
        boundaries.max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    } else {
        boundaries.min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
    }
}

fn next_float_boundary_after(regions: &[FloatRegion], y: f32) -> Option<f32> {
    regions
        .iter()
        .filter_map(|region| {
            let bottom = region.outer.y + region.outer.height;
            (bottom > y).then_some(bottom)
        })
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

fn clear_cursor_y_for_side(
    cursor_y: f32,
    child_margin_top: f32,
    collapse_delta: f32,
    regions: &[FloatRegion],
    side: FloatSide,
) -> f32 {
    let border_edge_top = cursor_y + child_margin_top - collapse_delta;
    let interfering_bottom = regions
        .iter()
        .filter(|region| match side {
            FloatSide::Left => region.side == FloatSide::Left,
            FloatSide::Right => region.side == FloatSide::Right,
            FloatSide::None => false,
        })
        .filter(|region| region.outer.y + region.outer.height > border_edge_top)
        .map(|region| region.outer.y + region.outer.height)
        .fold(border_edge_top, f32::max);
    cursor_y.max(interfering_bottom + collapse_delta - child_margin_top)
}

// ── Edge sizes / length resolution ──────────────────────────────────────────

pub(crate) fn edge_sizes(style: &ComputedStyle, prefix: &str) -> EdgeSizes {
    edge_sizes_using_basis(style, prefix, None)
}

/// Resolves margin and padding percentages against an explicit containing width.
/// A zero width is a definite basis; intrinsic callers retain the no-basis path.
pub(crate) fn edge_sizes_with_basis(
    style: &ComputedStyle,
    prefix: &str,
    containing_width: f32,
) -> EdgeSizes {
    edge_sizes_using_basis(style, prefix, Some(containing_width))
}

fn edge_sizes_using_basis(style: &ComputedStyle, prefix: &str, basis: Option<f32>) -> EdgeSizes {
    let ids = match prefix {
        "margin" => [
            PropertyId::MarginTop,
            PropertyId::MarginRight,
            PropertyId::MarginBottom,
            PropertyId::MarginLeft,
        ],
        "padding" => [
            PropertyId::PaddingTop,
            PropertyId::PaddingRight,
            PropertyId::PaddingBottom,
            PropertyId::PaddingLeft,
        ],
        "border" => [
            PropertyId::BorderTopWidth,
            PropertyId::BorderRightWidth,
            PropertyId::BorderBottomWidth,
            PropertyId::BorderLeftWidth,
        ],
        _ => return generic_edge_sizes_using_basis(style, prefix, basis),
    };
    let length = |value: Option<&ComputedValue>| {
        edge_length_value(value, basis).map(|value| {
            if prefix == "padding" {
                value.max(0.0)
            } else {
                value
            }
        })
    };
    let shorthand_name = if prefix == "border" {
        "border-width"
    } else {
        prefix
    };
    let shorthand = length(style.get(shorthand_name)).unwrap_or(0.0);
    let aliases = ["border-top", "border-right", "border-bottom", "border-left"];
    let sides = std::array::from_fn::<_, 4, _>(|index| {
        length(style.get_by_id(ids[index]))
            .or_else(|| {
                if prefix == "border" {
                    length(style.get(aliases[index]))
                } else {
                    None
                }
            })
            .unwrap_or(shorthand)
    });
    EdgeSizes {
        top: sides[0],
        right: sides[1],
        bottom: sides[2],
        left: sides[3],
    }
}

/// Resolves one already-looked-up value with the existing definite-basis policy.
fn edge_length_value(value: Option<&ComputedValue>, basis: Option<f32>) -> Option<f32> {
    let value = value?;
    match value {
        ComputedValue::Px(value) => Some(*value),
        ComputedValue::Number(value) if *value == 0.0 => Some(0.0),
        _ => basis.and_then(|basis| value.resolve_length_percentage(basis)),
    }
}

/// Retains the string-keyed path for other edge families and legacy names.
fn generic_edge_sizes_using_basis(
    style: &ComputedStyle,
    prefix: &str,
    basis: Option<f32>,
) -> EdgeSizes {
    let length = |property: &str| {
        explicit_length(style, property)
            .or_else(|| {
                basis.and_then(|basis| style.get(property)?.resolve_length_percentage(basis))
            })
            .map(|value| {
                if prefix == "padding" {
                    value.max(0.0)
                } else {
                    value
                }
            })
    };
    let shorthand_property = match prefix {
        "border" => "border-width",
        _ => prefix,
    };
    let side_property = match prefix {
        "border" => "border-{}-width",
        _ => "{prefix}-{}",
    };
    let shorthand = length(shorthand_property).unwrap_or(0.0);
    let physical = |side: &str| {
        length(
            &side_property
                .replace("{}", side)
                .replace("{prefix}", prefix),
        )
        .or_else(|| length(&format!("{prefix}-{side}")))
    };

    // The cascade already mapped logical declarations to physical sides in
    // priority order. Re-reading their logical spellings here would let an
    // earlier declaration override a later physical declaration.
    EdgeSizes {
        top: physical("top").unwrap_or(shorthand),
        right: physical("right").unwrap_or(shorthand),
        bottom: physical("bottom").unwrap_or(shorthand),
        left: physical("left").unwrap_or(shorthand),
    }
}

fn explicit_length(style: &ComputedStyle, property: &str) -> Option<f32> {
    match style.get(property) {
        Some(ComputedValue::Px(value)) => Some(*value),
        // CSS 2.1: unitless numbers are only valid as lengths when the value is 0
        Some(ComputedValue::Number(value)) if *value == 0.0 => Some(0.0),
        _ => None,
    }
}

fn resolved_length(style: &ComputedStyle, property: &str, basis: f32) -> Option<f32> {
    // Project the borrowed property into an owned used scalar while the lookup
    // is live. The old explicit/percentage/math chain searched the same style
    // map up to three times for one property.
    let resolved = match style.get(property) {
        Some(ComputedValue::Px(value)) => Some(*value),
        Some(ComputedValue::Number(value)) if *value == 0.0 => Some(0.0),
        Some(value) if basis > 0.0 => value.resolve_length_percentage(basis),
        _ => None,
    };
    if matches!(
        property,
        "width" | "height" | "min-width" | "min-height" | "max-width" | "max-height"
    ) {
        resolved.map(|value| value.max(0.0))
    } else {
        resolved
    }
}

fn is_auto(value: Option<&ComputedValue>) -> bool {
    matches!(value, Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("auto"))
}

fn margin_start_is_auto(style: &ComputedStyle) -> bool {
    is_auto(style.get("margin-left"))
}

fn margin_end_is_auto(style: &ComputedStyle) -> bool {
    is_auto(style.get("margin-right"))
}

// ── Margin collapsing ───────────────────────────────────────────────────────

fn collapse_margins(first: f32, second: f32) -> f32 {
    if first >= 0.0 && second >= 0.0 {
        first.max(second)
    } else if first <= 0.0 && second <= 0.0 {
        first.min(second)
    } else {
        first + second
    }
}

// ── Positioning helpers ─────────────────────────────────────────────────────

fn is_out_of_flow_positioned(style: &ComputedStyle) -> bool {
    matches!(
        position_scheme(style),
        PositionScheme::Absolute | PositionScheme::Fixed
    )
}

pub(crate) fn establishes_positioned_containing_block(style: &ComputedStyle) -> bool {
    matches!(
        position_scheme(style),
        PositionScheme::Relative
            | PositionScheme::Sticky
            | PositionScheme::Absolute
            | PositionScheme::Fixed
    ) || establishes_fixed_containing_block(style)
}

/// Whether this box anchors fixed descendants instead of the viewport.
/// Identity transforms still establish a containing block.
pub(crate) fn establishes_fixed_containing_block(style: &ComputedStyle) -> bool {
    const TRANSFORM_PROPERTIES: &[(PropertyId, &str)] = &[
        (PropertyId::Transform, "transform"),
        (PropertyId::Translate, "translate"),
        (PropertyId::Rotate, "rotate"),
        (PropertyId::Scale, "scale"),
        (PropertyId::Perspective, "perspective"),
    ];
    let keyword = |property| match style.get_by_id(property) {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value)) => Some(value.as_str()),
        _ => None,
    };

    TRANSFORM_PROPERTIES.iter().any(|(property, _)| {
        keyword(*property).is_some_and(|value| !value.eq_ignore_ascii_case("none"))
    }) || keyword(PropertyId::WillChange).is_some_and(|features| {
        features.split(',').any(|feature| {
            TRANSFORM_PROPERTIES
                .iter()
                .any(|(_, name)| feature.trim().eq_ignore_ascii_case(name))
        })
    }) || has_containment(style, "layout")
        || has_containment(style, "paint")
}

pub(super) fn position_scheme(style: &ComputedStyle) -> PositionScheme {
    match style.position() {
        Some(ComputedPosition::Relative) => PositionScheme::Relative,
        Some(ComputedPosition::Sticky) => PositionScheme::Sticky,
        Some(ComputedPosition::Absolute) => PositionScheme::Absolute,
        Some(ComputedPosition::Fixed) => PositionScheme::Fixed,
        _ => PositionScheme::Static,
    }
}

fn float_side(style: &ComputedStyle) -> FloatSide {
    match style.float() {
        Some(ComputedFloat::Left) => FloatSide::Left,
        Some(ComputedFloat::Right) => FloatSide::Right,
        _ => FloatSide::None,
    }
}

fn clear_side(style: &ComputedStyle) -> ClearSide {
    match style.get("clear") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("left") => {
            ClearSide::Left
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("right") => {
            ClearSide::Right
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("both") => {
            ClearSide::Both
        }
        _ => ClearSide::None,
    }
}

// ── Shrink-to-fit / intrinsic width ─────────────────────────────────────────

fn shrink_to_fit_width(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    available_width: f32,
) -> f32 {
    let outer = intrinsic_width(node, resolver);
    let style = resolver.computed_style(node);
    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    (outer - padding.horizontal() - border.horizontal())
        .max(0.0)
        .min(available_width)
}

fn shrink_to_fit_layout_width(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    available_width: f32,
) -> f32 {
    let style = resolver.computed_style(node);
    let padding = edge_sizes_with_basis(&style, "padding", available_width);
    let border = edge_sizes(&style, "border");
    let mut margin = edge_sizes_with_basis(&style, "margin", available_width);
    if margin_start_is_auto(&style) {
        margin.left = 0.0;
    }
    if margin_end_is_auto(&style) {
        margin.right = 0.0;
    }

    shrink_to_fit_width(node, resolver, available_width)
        + padding.horizontal()
        + border.horizontal()
        + margin.horizontal()
}

fn used_content_width(layout: &LayoutBox) -> f32 {
    let content_left = layout.dimensions.content.x;
    let line_width = layout
        .lines
        .iter()
        .map(|line| (line.rect.x + line.rect.width - content_left).max(0.0))
        .fold(0.0, f32::max);
    let child_width = layout
        .children
        .iter()
        .map(|child| {
            let outer_right = child.dimensions.content.x
                + child.dimensions.content.width
                + child.dimensions.padding.right
                + child.dimensions.border.right
                + child.dimensions.margin.right;
            (outer_right - content_left).max(0.0)
        })
        .fold(0.0, f32::max);

    line_width.max(child_width)
}

fn auto_width_from_layout(
    layout: &LayoutBox,
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    available_width: f32,
) -> f32 {
    if is_vertical_writing(&resolver.computed_style(node)) {
        // Columns consume physical width, whereas intrinsic_width measures the
        // horizontal text advance (the inline axis before transposition). Sum
        // the flow's column widths, independent of provisional right alignment.
        let columns = layout.lines.iter().map(|line| line.rect.width).sum::<f32>();
        let blocks = layout
            .children
            .iter()
            .filter_map(|child| {
                let style = layout_box_style(child, resolver);
                (!is_out_of_flow_positioned(&style) && !is_inline_child(&child.node, resolver))
                    .then(|| child.total_width())
            })
            .sum::<f32>();
        return (columns + blocks).max(0.0);
    }
    used_content_width(layout)
        .max(shrink_to_fit_width(node, resolver, available_width))
        .min(available_width)
        .max(0.0)
}

/// Returns the outer width (content + padding + border) that `node` needs.
/// Used by parent elements to determine how wide their content area must be.
/// Returns true if the node or any descendant is an image element.
#[allow(dead_code)]
fn cell_contains_image_recursive(node: &NodeHandle) -> bool {
    if element_inline_image(node).is_some() {
        return true;
    }
    for child in node.layout_child_nodes() {
        if cell_contains_image_recursive(&child) {
            return true;
        }
    }
    false
}

/// Returns the minimum content width for a node.
/// For text, this is the width of the longest unbreakable word.
/// For elements with explicit width, returns that width + padding/border.
/// This is used for table column sizing where columns should shrink as much as possible.
pub(super) fn minimum_content_width(node: &NodeHandle, resolver: &mut StyleResolver) -> f32 {
    minimum_content_width_inner(node, resolver, false)
}

fn minimum_content_width_for_sizing(node: &NodeHandle, resolver: &mut StyleResolver) -> f32 {
    minimum_content_width_inner(node, resolver, true)
}

fn minimum_content_width_inner(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    ignore_own_width: bool,
) -> f32 {
    // With no wrapping opportunity a text node's min-content contribution is
    // exactly its max-content contribution. Reuse that pass entry instead of
    // normalizing and shaping the same text once for each intrinsic kind.
    if node.node_type() == NodeType::Text {
        let parent_style = node
            .parent_node()
            .map(|parent| resolver.computed_style(&parent))
            .unwrap_or_default();
        if !white_space(&parent_style).allows_wrapping() {
            return intrinsic_width_inner(node, resolver, ignore_own_width);
        }
    }
    intrinsic::measure(
        node.identity(),
        intrinsic::Kind::Minimum,
        ignore_own_width,
        || minimum_content_width_inner_uncached(node, resolver, ignore_own_width),
    )
}

fn minimum_content_width_inner_uncached(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    ignore_own_width: bool,
) -> f32 {
    match node.node_type() {
        NodeType::Text => node
            .data()
            .map(|text| {
                let parent_style = node
                    .parent_node()
                    .map(|parent| resolver.computed_style(&parent))
                    .unwrap_or_default();
                let metrics = font_metrics(&parent_style);
                // Find the width of the longest unbreakable unit (word or CJK character).
                use crate::layout::inline::for_each_word_preserving_spaces_cjk;
                let white_space_mode = white_space(&parent_style);
                let normalized = normalize_text(&text, white_space_mode);
                if !white_space_mode.allows_wrapping() {
                    return measure_text_width(&normalized, &metrics);
                }
                let mut minimum = 0.0f32;
                for_each_word_preserving_spaces_cjk(&normalized, |word| {
                    minimum = minimum.max(measure_text_width(word, &metrics));
                });
                minimum
            })
            .unwrap_or(0.0),
        NodeType::Element => {
            let style = resolver.computed_style(node);
            if style.is_display_none() || is_non_rendered_html_element(node) {
                return 0.0;
            }
            let padding = edge_sizes(&style, "padding");
            let border = edge_sizes(&style, "border");
            // Elements with explicit width use that as minimum.
            if !ignore_own_width && let Some(width) = explicit_length(&style, "width") {
                let margin = edge_sizes(&style, "margin");
                return width + padding.horizontal() + border.horizontal() + margin.horizontal();
            }
            if has_inline_size_containment(&style) {
                let margin = if ignore_own_width {
                    0.0
                } else {
                    edge_sizes(&style, "margin").horizontal()
                };
                return contain_intrinsic_axis_size(node, &style, "contain-intrinsic-width", 0)
                    + padding.horizontal()
                    + border.horizontal()
                    + margin;
            }
            if node.has_tag_name("iframe") {
                return 300.0 + padding.horizontal() + border.horizontal();
            }
            // For images, use rendered size.
            if let Some((image_node, image)) = element_inline_image(node) {
                let image_style = resolver.computed_style(&image_node);
                let (rendered_width, _) =
                    resolve_image_rendered_size(&image_node, &image, &image_style);
                let img_padding = edge_sizes(&image_style, "padding");
                let img_border = edge_sizes(&image_style, "border");
                return rendered_width + img_padding.horizontal() + img_border.horizontal();
            }
            // Recurse: minimum of children's minimum widths
            let mut min_width = 0.0f32;
            for child in node.layout_child_nodes() {
                min_width = min_width.max(minimum_content_width_inner(&child, resolver, false));
            }
            min_width + padding.horizontal() + border.horizontal()
        }
        _ => 0.0,
    }
}

fn intrinsic_width(node: &NodeHandle, resolver: &mut StyleResolver) -> f32 {
    intrinsic_width_inner(node, resolver, false)
}

fn intrinsic_width_for_sizing(node: &NodeHandle, resolver: &mut StyleResolver) -> f32 {
    intrinsic_width_inner(node, resolver, true)
}

fn intrinsic_width_inner(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    ignore_own_width: bool,
) -> f32 {
    intrinsic::measure(
        node.identity(),
        intrinsic::Kind::Maximum,
        ignore_own_width,
        || intrinsic_width_inner_uncached(node, resolver, ignore_own_width),
    )
}

fn intrinsic_width_inner_uncached(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    ignore_own_width: bool,
) -> f32 {
    match node.node_type() {
        NodeType::Text => node
            .data()
            .map(|text| {
                let parent_style = node
                    .parent_node()
                    .map(|parent| resolver.computed_style(&parent))
                    .unwrap_or_default();
                let metrics = font_metrics(&parent_style);
                measure_text_width(&normalize_text(&text, white_space(&parent_style)), &metrics)
            })
            .unwrap_or(0.0),
        NodeType::Element => {
            let style = resolver.computed_style(node);
            if style.is_display_none() || is_non_rendered_html_element(node) {
                return 0.0;
            }
            let padding = edge_sizes(&style, "padding");
            let border = edge_sizes(&style, "border");
            if !ignore_own_width && let Some(width) = explicit_length(&style, "width") {
                let margin = edge_sizes(&style, "margin");
                return width + padding.horizontal() + border.horizontal() + margin.horizontal();
            }
            if has_inline_size_containment(&style) {
                let margin = if ignore_own_width {
                    0.0
                } else {
                    edge_sizes(&style, "margin").horizontal()
                };
                return contain_intrinsic_axis_size(node, &style, "contain-intrinsic-width", 0)
                    + padding.horizontal()
                    + border.horizontal()
                    + margin;
            }
            if node.has_tag_name("iframe") {
                return 300.0 + padding.horizontal() + border.horizontal();
            }
            if let Some((image_node, image)) = element_inline_image(node) {
                let image_style = resolver.computed_style(&image_node);
                let img_padding = edge_sizes(&image_style, "padding");
                let img_border = edge_sizes(&image_style, "border");
                let (rendered_width, _) =
                    resolve_image_rendered_size(&image_node, &image, &image_style);
                return rendered_width
                    + img_padding.left
                    + img_padding.right
                    + img_border.left
                    + img_border.right;
            }
            if is_flex_container(&style) {
                return flex::intrinsic_content_width(node, resolver, &style)
                    + padding.horizontal()
                    + border.horizontal();
            }
            // Content width = max of children's outer widths
            let mut content_width: f32 = 0.0;
            if is_table_container_element(node, &style) {
                let entries = collect_table_entries(node, resolver);
                let spacing = table_border_spacing(&style);
                for entry in &entries {
                    let row_width: f32 = entry
                        .cells
                        .iter()
                        .map(|cell| intrinsic_width(cell, resolver))
                        .sum::<f32>()
                        + spacing * (entry.cells.len().max(1) as f32 + 1.0);
                    content_width = content_width.max(row_width);
                }
            } else {
                let mut inline_run_width = 0.0f32;
                for child in node.layout_child_nodes() {
                    let child_width = intrinsic_width_inner(&child, resolver, false);
                    if is_inline_child(&child, resolver) {
                        inline_run_width += child_width;
                    } else {
                        content_width = content_width.max(inline_run_width);
                        inline_run_width = 0.0;
                        content_width = content_width.max(child_width);
                    }
                }
                content_width = content_width.max(inline_run_width);
            }
            let mut width = content_width;
            if width == 0.0 {
                width = generated_inline_segments(node, resolver, PseudoElement::Before)
                    .into_iter()
                    .chain(generated_inline_segments(
                        node,
                        resolver,
                        PseudoElement::After,
                    ))
                    .map(|segment| match segment.content {
                        InlineSegmentContent::Text(text) => {
                            measure_text_width(&text, &segment.metrics)
                        }
                        InlineSegmentContent::InlineEdge(style, start) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            if start {
                                padding.left + border.left
                            } else {
                                padding.right + border.right
                            }
                        }
                        InlineSegmentContent::AtomicInline(width, _, _) => width,
                        InlineSegmentContent::Image(_, style, rendered_width, _) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            rendered_width
                                + padding.left
                                + padding.right
                                + border.left
                                + border.right
                        }
                        InlineSegmentContent::FormControl(style, _, _, width, _) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            width + padding.left + padding.right + border.left + border.right
                        }
                        InlineSegmentContent::ValueIndicator(style, _, width, _) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            width + padding.left + padding.right + border.left + border.right
                        }
                        InlineSegmentContent::IconFormControl(style, _, width, _, _, _) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            width + padding.left + padding.right + border.left + border.right
                        }
                        InlineSegmentContent::GeneratedBox(style) => {
                            let padding = edge_sizes(&style, "padding");
                            let border = edge_sizes(&style, "border");
                            explicit_length(&style, "width").unwrap_or(0.0)
                                + padding.left
                                + padding.right
                                + border.left
                                + border.right
                        }
                    })
                    .fold(0.0, f32::max);
            }
            // Outer width = content + own padding + border
            width + padding.horizontal() + border.horizontal()
        }
        _ => 0.0,
    }
}

// ── Z-index / relative offset / transform ───────────────────────────────────

fn z_index(style: &ComputedStyle) -> i32 {
    match style.get("z-index") {
        Some(ComputedValue::Number(value)) => *value as i32,
        Some(ComputedValue::Px(value)) => *value as i32,
        _ => 0,
    }
}

fn apply_relative_offset(
    layout: &mut LayoutBox,
    style: &ComputedStyle,
    resolver: &mut StyleResolver,
) {
    if position_scheme(style) != PositionScheme::Relative {
        return;
    }

    let dx = explicit_length(style, "left").unwrap_or(0.0)
        - explicit_length(style, "right").unwrap_or(0.0);
    let dy = explicit_length(style, "top").unwrap_or(0.0)
        - explicit_length(style, "bottom").unwrap_or(0.0);

    if dx != 0.0 || dy != 0.0 {
        translate_layout_box(layout, dx, dy, resolver);
    }
}

// ── Positioned child layout ─────────────────────────────────────────────────

fn layout_positioned_source(
    source: &LayoutSource,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    parent_box: BoxDimensions,
    containing_block: Rect,
    viewport: LayoutViewport,
) -> Option<LayoutBox> {
    if let LayoutSource::Node(node) = source {
        return layout_positioned_child(
            node,
            resolver,
            style,
            parent_box,
            containing_block,
            viewport,
        );
    }

    let origin = match position_scheme(style) {
        PositionScheme::Fixed => viewport.fixed.unwrap_or(viewport.rect),
        PositionScheme::Absolute => parent_box.padding_box(),
        PositionScheme::Static | PositionScheme::Relative | PositionScheme::Sticky => {
            containing_block
        }
    };
    let (left, right, top, bottom) = positioned_insets(style, origin);
    let mut layout = source.layout(
        resolver,
        Rect {
            x: origin.x,
            y: origin.y,
            width: origin.width,
            height: origin.height,
        },
        viewport,
        Some(parent_box),
    )?;
    let outer_width = layout.total_width();
    let outer_height = layout.total_height();
    let outer_x = left.map_or_else(
        || {
            right.map_or(containing_block.x, |right| {
                origin.x + origin.width - outer_width - right
            })
        },
        |left| origin.x + left,
    );
    let outer_y = top.map_or_else(
        || {
            bottom.map_or(containing_block.y, |bottom| {
                origin.y + origin.height - outer_height - bottom
            })
        },
        |top| origin.y + top,
    );
    translate_layout_box_to_outer(&mut layout, outer_x, outer_y, resolver);
    Some(layout)
}

fn layout_positioned_child(
    child: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    parent_box: BoxDimensions,
    containing_block: Rect,
    viewport: LayoutViewport,
) -> Option<LayoutBox> {
    layout_positioned_child_mode(
        child,
        resolver,
        style,
        parent_box,
        containing_block,
        viewport,
        false,
    )
}

fn positioned_insets(
    style: &ComputedStyle,
    origin: Rect,
) -> (Option<f32>, Option<f32>, Option<f32>, Option<f32>) {
    (
        resolved_length(style, "left", origin.width),
        resolved_length(style, "right", origin.width),
        resolved_length(style, "top", origin.height),
        resolved_length(style, "bottom", origin.height),
    )
}

fn positioned_auto_content_width(
    child: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    layout: &LayoutBox,
    available_width: f32,
) -> f32 {
    let mut width = auto_width_from_layout(layout, child, resolver, available_width);
    if style.get("min-width").is_none() && style.get("max-width").is_none() {
        return width;
    }
    let decorations =
        layout.dimensions.padding.horizontal() + layout.dimensions.border.horizontal();
    let stretch = (available_width - decorations - layout.dimensions.margin.horizontal()).max(0.0);
    let mut intrinsic = IntrinsicContentWidths::default();
    let minimum = resolved_content_width(
        child,
        resolver,
        style,
        "min-width",
        available_width,
        layout.dimensions.padding,
        layout.dimensions.border,
        stretch,
        &mut intrinsic,
    );
    let maximum = resolved_content_width(
        child,
        resolver,
        style,
        "max-width",
        available_width,
        layout.dimensions.padding,
        layout.dimensions.border,
        stretch,
        &mut intrinsic,
    );
    if let Some(minimum) = minimum {
        width = width.max(minimum);
    }
    if let Some(maximum) = maximum {
        width = width.min(maximum.max(minimum.unwrap_or(0.0)));
    }
    width
}

#[derive(Clone, Copy, Default)]
struct PositionedAutoStretch {
    available_width: Option<f32>,
    content_height: Option<f32>,
}

fn positioned_size_is_auto(style: &ComputedStyle, property: &str) -> bool {
    style.get(property).is_none() || is_auto(style.get(property))
}

fn positioned_self_alignment_is_normal(style: &ComputedStyle, property: &str) -> bool {
    style.get(property).is_none_or(|value| {
        matches!(value, ComputedValue::Keyword(keyword) if keyword.eq_ignore_ascii_case("normal"))
    })
}

fn has_preferred_aspect_ratio(style: &ComputedStyle) -> bool {
    style.get("aspect-ratio").is_some_and(|value| {
        !matches!(value, ComputedValue::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto"))
    })
}

#[allow(clippy::too_many_arguments)]
fn positioned_auto_stretch(
    child: &NodeHandle,
    style: &ComputedStyle,
    position: PositionScheme,
    origin: Rect,
    left: Option<f32>,
    right: Option<f32>,
    top: Option<f32>,
    bottom: Option<f32>,
) -> PositionedAutoStretch {
    if position != PositionScheme::Absolute
        || has_replaced_layout_semantics(child)
        || is_table_container_element(child, style)
        || has_preferred_aspect_ratio(style)
        || !positioned_self_alignment_is_normal(style, "justify-self")
        || !positioned_self_alignment_is_normal(style, "align-self")
    {
        return PositionedAutoStretch::default();
    }

    // Preserve the signed inset span until margins and decorations are
    // removed, then clamp the final content size. compute_width performs the
    // horizontal equation below; the vertical path mirrors it directly.
    // Negative margins can therefore leave content space when insets overlap.
    let available_width = left
        .zip(right)
        .filter(|_| positioned_size_is_auto(style, "width"))
        .map(|(left, right)| origin.width - left - right)
        .filter(|span| span.is_finite());
    let content_height = top
        .zip(bottom)
        .filter(|_| positioned_size_is_auto(style, "height"))
        .map(|(top, bottom)| origin.height - top - bottom)
        .filter(|span| span.is_finite())
        .map(|span| {
            let margin = edge_sizes_with_basis(style, "margin", origin.width);
            let padding = edge_sizes_with_basis(style, "padding", origin.width);
            let border = edge_sizes(style, "border");
            (span - margin.vertical() - padding.vertical() - border.vertical()).max(0.0)
        });

    PositionedAutoStretch {
        available_width,
        content_height,
    }
}

fn layout_positioned_child_mode(
    child: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    parent_box: BoxDimensions,
    containing_block: Rect,
    viewport: LayoutViewport,
    top_layer_root: bool,
) -> Option<LayoutBox> {
    let position = position_scheme(style);
    let origin = match position {
        PositionScheme::Fixed => viewport.fixed.unwrap_or(viewport.rect),
        PositionScheme::Absolute => parent_box.padding_box(),
        PositionScheme::Static => containing_block,
        PositionScheme::Relative => containing_block,
        PositionScheme::Sticky => containing_block,
    };

    let (left, right, top, bottom) = positioned_insets(style, origin);
    let stretch = positioned_auto_stretch(child, style, position, origin, left, right, top, bottom);
    let static_outer = containing_block;
    let specified_width = resolved_length(style, "width", origin.width);
    let child_width = stretch.available_width.unwrap_or_else(|| {
        if specified_width.is_none() {
            shrink_to_fit_layout_width(child, resolver, origin.width)
        } else {
            origin.width
        }
    });
    let child_containing = Rect {
        x: origin.x,
        y: origin.y,
        width: child_width,
        height: origin.height,
    };
    let width_percentage_basis = if stretch.available_width.is_some() {
        origin.width
    } else {
        child_containing.width
    };
    if !top_layer_root && child.top_layer_order().is_some() {
        return None;
    }
    // Shrink-to-fit changes the allocated width, not the containing block
    // against which percentage margins and padding are resolved. Ordinary
    // width constraints keep their allocated-width basis; opposing-inset auto
    // sizing alone resolves min/max percentages against the original block.
    let mut layout_child = layout_element_with_cell(
        child,
        resolver,
        child_containing,
        viewport,
        Some(parent_box),
        None,
        stretch.content_height.map(|value| UsedHeight {
            value,
            definite: true,
        }),
        None,
        origin.width,
        width_percentage_basis,
    )?;
    if specified_width.is_none() && stretch.available_width.is_none() {
        let auto_width =
            positioned_auto_content_width(child, resolver, style, &layout_child, origin.width);
        if (auto_width - layout_child.dimensions.content.width).abs() > 0.5 {
            let outside_width = if is_vertical_writing(style) {
                layout_child.dimensions.padding.horizontal()
                    + layout_child.dimensions.border.horizontal()
                    + layout_child.dimensions.margin.horizontal()
            } else {
                0.0
            };
            let relayout_containing = Rect {
                width: auto_width + outside_width,
                ..child_containing
            };
            layout_child = layout_element_with_cell(
                child,
                resolver,
                relayout_containing,
                viewport,
                Some(parent_box),
                None,
                stretch.content_height.map(|value| UsedHeight {
                    value,
                    definite: true,
                }),
                None,
                origin.width,
                relayout_containing.width,
            )?;
        }
        layout_child.dimensions.content.width =
            positioned_auto_content_width(child, resolver, style, &layout_child, origin.width);
    }
    let outer_width = layout_child.total_width();
    let outer_height = layout_child.total_height();
    let outer_x = if let Some(left) = left {
        origin.x + left
    } else if let Some(right) = right {
        origin.x + origin.width - outer_width - right
    } else {
        static_outer.x
    };
    let outer_y = if let Some(top) = top {
        origin.y + top
    } else if let Some(bottom) = bottom {
        origin.y + origin.height - outer_height - bottom
    } else {
        static_outer.y
    };
    translate_layout_box_to_outer(&mut layout_child, outer_x, outer_y, resolver);
    layout_child.z_index = z_index(style);
    Some(layout_child)
}

fn relayout_fixed_descendants(
    children: &mut [LayoutBox],
    resolver: &mut StyleResolver,
    viewport: LayoutViewport,
) {
    for child in children {
        let style = layout_box_style(child, resolver);
        if position_scheme(&style) == PositionScheme::Fixed {
            let dimensions = child.dimensions;
            // Preserve the already-placed hypothetical position on auto axes.
            let static_position = Rect {
                x: dimensions.content.x
                    - dimensions.padding.left
                    - dimensions.border.left
                    - dimensions.margin.left,
                y: dimensions.content.y
                    - dimensions.padding.top
                    - dimensions.border.top
                    - dimensions.margin.top,
                width: child.total_width(),
                height: child.total_height(),
            };
            let source = LayoutSource::from_layout_box(child);
            if let Some(resolved) = layout_positioned_source(
                &source,
                resolver,
                &style,
                BoxDimensions {
                    content: viewport.rect,
                    ..BoxDimensions::default()
                },
                static_position,
                viewport,
            ) {
                *child = resolved;
            }
            // The replacement has already laid out its own descendants with
            // the final inherited context; do not lay them out a second time.
        } else if !establishes_fixed_containing_block(&style) {
            relayout_fixed_descendants(&mut child.children, resolver, viewport);
        }
    }
}

// Positioned block children are laid out after in-flow children. Restore tree
// order for equal z-index instead of using that implementation's append order.
// Flex keeps its separate order-modified sequence and uses the stable z sort.
fn sort_children_by_z_index_in_tree_order(node: &NodeHandle, children: &mut [LayoutBox]) {
    let ranks: HashMap<_, _> = node
        .layout_child_nodes()
        .iter()
        .enumerate()
        .map(|(index, child)| (child.identity(), index + 1))
        .collect();
    children.sort_by_cached_key(|child| {
        let rank = match child.pseudo {
            Some(PseudoElement::Before) if child.node == *node => 0,
            Some(PseudoElement::After) if child.node == *node => usize::MAX,
            _ => ranks
                .get(&child.node.identity())
                .copied()
                .unwrap_or(usize::MAX - 1),
        };
        (child.z_index, rank)
    });
}

fn sort_children_by_z_index(children: &mut [LayoutBox]) {
    children.sort_by_key(|child| child.z_index);
}

// ── Box translation helpers ─────────────────────────────────────────────────

fn translate_layout_box_to_outer(
    layout: &mut LayoutBox,
    outer_x: f32,
    outer_y: f32,
    resolver: &mut StyleResolver,
) {
    let current_outer_x = layout.dimensions.content.x
        - layout.dimensions.padding.left
        - layout.dimensions.border.left
        - layout.dimensions.margin.left;
    let current_outer_y = layout.dimensions.content.y
        - layout.dimensions.padding.top
        - layout.dimensions.border.top
        - layout.dimensions.margin.top;
    translate_layout_box(
        layout,
        outer_x - current_outer_x,
        outer_y - current_outer_y,
        resolver,
    );
}

pub(crate) fn translate_layout_box(
    layout: &mut LayoutBox,
    dx: f32,
    dy: f32,
    resolver: &mut StyleResolver,
) {
    if (dx, dy) == (0.0, 0.0) {
        return;
    }
    let style = layout_box_style(layout, resolver);
    layout.dimensions.content.x += dx;
    layout.dimensions.content.y += dy;
    translate_block_fragments(layout, dx, dy);
    translate_contents_in_context(
        layout,
        dx,
        dy,
        resolver,
        establishes_positioned_containing_block(&style),
        establishes_fixed_containing_block(&style),
    );
}

// Moving content within a box (e.g. table-cell vertical-align) does not move
// the box's containing block. Auto insets still follow their static position.
fn translate_layout_contents(
    layout: &mut LayoutBox,
    dx: f32,
    dy: f32,
    resolver: &mut StyleResolver,
) {
    translate_contents_in_context(layout, dx, dy, resolver, false, false);
}

fn translate_inherited_box(
    layout: &mut LayoutBox,
    mut dx: f32,
    mut dy: f32,
    resolver: &mut StyleResolver,
    absolute_moves: bool,
    fixed_moves: bool,
) {
    if (dx, dy) == (0.0, 0.0) {
        return;
    }
    let style = layout_box_style(layout, resolver);
    let outside = match position_scheme(&style) {
        PositionScheme::Fixed => !fixed_moves,
        PositionScheme::Absolute => !absolute_moves,
        _ => false,
    };
    if outside {
        let (left, right, top, bottom) = positioned_insets(
            &style,
            Rect {
                width: 1.0,
                height: 1.0,
                ..Rect::default()
            },
        );
        if left.is_some() || right.is_some() {
            dx = 0.0;
        }
        if top.is_some() || bottom.is_some() {
            dy = 0.0;
        }
    }
    if (dx, dy) == (0.0, 0.0) {
        return;
    }
    layout.dimensions.content.x += dx;
    layout.dimensions.content.y += dy;
    translate_block_fragments(layout, dx, dy);
    translate_contents_in_context(
        layout,
        dx,
        dy,
        resolver,
        absolute_moves || establishes_positioned_containing_block(&style),
        fixed_moves || establishes_fixed_containing_block(&style),
    );
}

fn translate_block_fragments(layout: &mut LayoutBox, dx: f32, dy: f32) {
    for fragment in &mut layout.block_fragments {
        fragment.source.x += dx;
        fragment.source.y += dy;
        fragment.target.x += dx;
        fragment.target.y += dy;
        fragment.clip.x += dx;
        fragment.clip.y += dy;
        if let Some(target) = &mut fragment.clone_content_target {
            target.x += dx;
            target.y += dy;
        }
    }
}

fn translate_contents_in_context(
    layout: &mut LayoutBox,
    dx: f32,
    dy: f32,
    resolver: &mut StyleResolver,
    absolute_moves: bool,
    fixed_moves: bool,
) {
    if (dx, dy) == (0.0, 0.0) {
        return;
    }
    if let Some(multicol) = &mut layout.multicol {
        multicol.overflow.x += dx;
        multicol.overflow.y += dy;
        for rule in &mut multicol.rules {
            rule.x += dx;
            rule.y += dy;
        }
    }
    for line in &mut layout.lines {
        line.rect.x += dx;
        line.rect.y += dy;
        line.baseline += dy;
        for fragment in &mut line.fragments {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
        }
    }
    for child in &mut layout.children {
        translate_inherited_box(child, dx, dy, resolver, absolute_moves, fixed_moves);
    }
    if let Some(marker) = &mut layout.marker {
        marker.x += dx;
        marker.y += dy;
    }
}

// ── Inline child detection ──────────────────────────────────────────────────

fn is_inline_child(node: &NodeHandle, resolver: &mut StyleResolver) -> bool {
    match node.node_type() {
        NodeType::Text => true,
        NodeType::Element => {
            if is_non_rendered_html_element(node) {
                return false;
            }
            let style = resolver.computed_style(node);
            if float_side(&style) != FloatSide::None || is_out_of_flow_positioned(&style) {
                return false;
            }
            if let Some(ComputedValue::Keyword(keyword)) = style.get_by_id(PropertyId::Display) {
                return keyword.eq_ignore_ascii_case("inline")
                    || keyword.eq_ignore_ascii_case("inline-block");
            }
            node.tag_name()
                .map(|tag| {
                    matches!(
                        tag.as_str(),
                        "span"
                            | "a"
                            | "em"
                            | "strong"
                            | "b"
                            | "i"
                            | "img"
                            | "object"
                            | "svg"
                            | "input"
                            | "button"
                            | "textarea"
                            | "select"
                            | "time"
                            | "progress"
                            | "meter"
                            | "video"
                            | "audio"
                            | "canvas"
                            | "picture"
                    )
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

// ── Display / visibility / overflow ─────────────────────────────────────────

// Source elements supply media resources rather than rendered content. UA
// display defaults for other elements are resolved in the normal cascade so
// author display declarations can expose them.
fn is_non_rendered_html_element(node: &NodeHandle) -> bool {
    node.has_tag_name("source")
}

fn visibility(style: &ComputedStyle) -> Visibility {
    match style.get_by_id(PropertyId::Visibility) {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("hidden") => {
            Visibility::Hidden
        }
        _ => Visibility::Visible,
    }
}

fn text_overflow_is_ellipsis(style: &ComputedStyle) -> bool {
    matches!(
        style.get_by_id(PropertyId::TextOverflow),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("ellipsis")
    )
}

fn overflow(style: &ComputedStyle) -> Overflow {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Axis {
        Visible,
        Clip,
        Scroll,
    }

    fn axis(style: &ComputedStyle, property: PropertyId) -> Axis {
        match style.get_by_id(property) {
            Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("clip") => {
                Axis::Clip
            }
            Some(ComputedValue::Keyword(keyword)) if overflow_keyword_scrolls(keyword) => {
                Axis::Scroll
            }
            _ => Axis::Visible,
        }
    }

    let mut x = axis(style, PropertyId::OverflowX);
    let mut y = axis(style, PropertyId::OverflowY);
    // CSS Overflow computes visible to auto and clip to hidden when the other
    // axis is neither visible nor clip. Thus any scrollable axis makes both
    // axes part of the scroll container; clip/visible is the special pair that
    // remains independently clipped.
    if x == Axis::Scroll {
        y = Axis::Scroll;
    }
    if y == Axis::Scroll {
        x = Axis::Scroll;
    }
    match (x, y) {
        (Axis::Visible, Axis::Visible) => Overflow::Visible,
        (Axis::Clip, Axis::Visible) => Overflow::ClipX,
        (Axis::Visible, Axis::Clip) => Overflow::ClipY,
        (Axis::Clip, Axis::Clip) => Overflow::Clip,
        _ => Overflow::Hidden,
    }
}

/// Whether an `overflow` keyword makes the box clip and scroll its content.
///
/// `hidden`, `scroll` and `auto` all do; `visible` and `clip` do not. A
/// two-value `overflow` sets both axes at once, and a non-`visible` axis forces
/// the other one to become scrollable, so one scrollable token is enough.
fn overflow_keyword_scrolls(keyword: &str) -> bool {
    keyword
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
        .any(|token| {
            token.eq_ignore_ascii_case("hidden")
                || token.eq_ignore_ascii_case("scroll")
                || token.eq_ignore_ascii_case("auto")
        })
}

// ── List marker ─────────────────────────────────────────────────────────────

/// Returns a `ListMarker` for a `display: list-item` element, or `None` if no
/// marker should be rendered (e.g. `list-style-type: none`).
fn build_list_marker(
    node: &NodeHandle,
    style: &ComputedStyle,
    content_x: f32,
    content_y: f32,
) -> Option<ListMarker> {
    // Only `display: list-item` generates a marker.
    let display_is_list_item = matches!(
        style.get_by_id(PropertyId::Display),
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("list-item")
    );
    if !display_is_list_item {
        return None;
    }

    // Determine list-style-type.
    let list_style_type = match style.get_by_id(PropertyId::ListStyleType) {
        Some(ComputedValue::Keyword(kw)) => kw.to_ascii_lowercase(),
        _ => "disc".to_string(),
    };

    if list_style_type == "none" {
        return None;
    }

    let font_size = match style.get_by_id(PropertyId::FontSize) {
        Some(ComputedValue::Px(px)) => *px,
        _ => 16.0,
    };

    let outside = !matches!(
        style.get_by_id(PropertyId::ListStylePosition),
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("inside")
    );

    // Build the marker string.
    let text = match list_style_type.as_str() {
        "disc" => "\u{2022}".to_string(),   // bullet
        "circle" => "\u{25e6}".to_string(), // circle
        "square" => "\u{25a0}".to_string(), // square
        "decimal" => {
            let ordinal = list_item_ordinal(node);
            format!("{ordinal}.")
        }
        "lower-roman" => {
            let ordinal = list_item_ordinal(node);
            format!("{}.", to_roman_lower(ordinal))
        }
        "upper-roman" => {
            let ordinal = list_item_ordinal(node);
            format!("{}.", to_roman_upper(ordinal))
        }
        "lower-alpha" | "lower-latin" => {
            let ordinal = list_item_ordinal(node);
            format!("{}.", to_alpha_lower(ordinal))
        }
        "upper-alpha" | "upper-latin" => {
            let ordinal = list_item_ordinal(node);
            format!("{}.", to_alpha_upper(ordinal))
        }
        _ => "\u{2022}".to_string(), // fallback to disc
    };

    let (x, y) = if outside {
        (content_x - font_size, content_y)
    } else {
        (content_x, content_y)
    };

    Some(ListMarker {
        text,
        font_size,
        outside,
        x,
        y,
    })
}

/// Returns the 1-based ordinal position of `node` among its `li` siblings.
fn list_item_ordinal(node: &NodeHandle) -> usize {
    let Some(parent) = node.parent_node() else {
        return 1;
    };
    let mut count = 0usize;
    for sibling in parent.layout_child_nodes() {
        if sibling.node_type() == NodeType::Element
            && sibling
                .tag_name()
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("li"))
                .unwrap_or(false)
        {
            count += 1;
        }
        if sibling.identity() == node.identity() {
            break;
        }
    }
    count.max(1)
}

/// Converts a number to lowercase Roman numerals.
pub(super) fn to_roman_lower(n: usize) -> String {
    to_roman_inner(n).to_ascii_lowercase()
}

/// Converts a number to uppercase Roman numerals.
pub(super) fn to_roman_upper(n: usize) -> String {
    to_roman_inner(n)
}

fn to_roman_inner(mut n: usize) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const VALUES: &[(usize, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut result = String::new();
    for &(value, symbol) in VALUES {
        while n >= value {
            result.push_str(symbol);
            n -= value;
        }
    }
    result
}

/// Converts a 1-based number to a lowercase Latin letter (a, b, ..., z, aa, ...).
pub(super) fn to_alpha_lower(n: usize) -> String {
    to_alpha_inner(n).to_ascii_lowercase()
}

/// Converts a 1-based number to an uppercase Latin letter (A, B, ..., Z, AA, ...).
pub(super) fn to_alpha_upper(n: usize) -> String {
    to_alpha_inner(n)
}

fn to_alpha_inner(mut n: usize) -> String {
    if n == 0 {
        return "a".to_string();
    }
    let mut result = String::new();
    while n > 0 {
        n -= 1;
        result.insert(
            0,
            char::from_u32(b'A' as u32 + (n % 26) as u32).unwrap_or('A'),
        );
        n /= 26;
    }
    result
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod flex_reflow_tests;

#[cfg(test)]
mod grid_stretch_tests;

#[cfg(test)]
mod flex_text_tests;

#[cfg(test)]
mod margin_collapse_tests;

#[cfg(test)]
mod inline_box_tests;

mod image_animation;
pub(crate) use image_animation::{image_animation_time_ms, with_image_animation_timeline};

#[cfg(test)]
mod edge_lookup_tests;
