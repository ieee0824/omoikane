//! Block layout primitives.
//!
//! The layout phase consumes DOM nodes together with computed styles and
//! produces a tree of rectangular block boxes.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use crate::css::{
    AffineTransform, ContainerContext, ComputedStyle, ComputedValue, PseudoElement, StyleResolver,
    TransformReferenceBox, parse_perspective_with_origin, parse_transform_with_origin,
};
use crate::dom::{Node, NodeHandle, NodeType};
use crate::font::{Font, FontFamilyKey, FontStyle, FontWeight, WebFontRegistry};
use crate::http::{Client, Url};
use crate::paint::Image;
use rusqlite::{Connection, params};

mod flex;
mod grid;
mod inline;
mod table;

use flex::{flex_direction, is_flex_container, layout_flex_container};
use grid::{is_grid_container, layout_grid_container};
use inline::{
    InlineSegmentContent,
    font_metrics, generated_inline_segments,
    layout_inline_nodes, layout_vertical_inline_nodes, line_height, measure_text_width, normalize_text,
    resolve_image_rendered_size,
    text_align, vertical_align, white_space,
};
use table::{
    collect_table_entries, is_table_container_element, layout_table_container,
    table_border_spacing,
};

pub(crate) use inline::{canonical_image_asset_reference, decode_or_fetch_image_asset};
pub(crate) use inline::element_inline_image;

// Re-exports used by tests
#[cfg(test)]
pub(crate) use crate::paint::{DataUri, parse_data_uri};
#[cfg(test)]
pub(crate) use inline::split_words_preserving_spaces_cjk;
#[cfg(test)]
pub(crate) use inline::split_chars;
#[cfg(test)]
pub(crate) use inline::split_words_no_cjk_break;

// Thread-local cache for fetched images and fonts to avoid redundant loads
thread_local! {
    static IMAGE_CACHE: RefCell<HashMap<String, Option<Image>>> = RefCell::new(HashMap::new());
    static IMAGE_ANIMATION_CACHE: RefCell<HashMap<String, crate::paint::ImageAnimation>> = RefCell::new(HashMap::new());
    static IMAGE_ANIMATION_TIME_MS: Cell<u64> = const { Cell::new(0) };
    static HTTP_CLIENT: RefCell<Client> = RefCell::new(Client::new());
    static LAYOUT_FONTS: RefCell<Option<LayoutFontContext>> = const { RefCell::new(None) };
    static IMAGE_BASE_URL: RefCell<Option<Url>> = const { RefCell::new(None) };
    static HTML_TAG_SQLITE_CONNECTIONS: RefCell<HashMap<String, Connection>> = RefCell::new(HashMap::new());
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
        let previous = cell.replace(Some(LayoutFontContext { system_fonts, web_fonts }));
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
        "html" | "head" | "body" | "div" | "span" | "section" | "article"
            | "aside" | "main" | "nav" | "header" | "footer"
            | "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
            | "br" | "strong" | "em" | "b" | "i" | "u" | "s" | "a" | "pre" | "code"
            | "ul" | "ol" | "li"
            | "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
            | "img" | "object" | "svg" | "form" | "input"
            | "button" | "textarea" | "select" | "option" | "iframe" | "label"
            | "style" | "link" | "meta" | "title" | "script" | "noscript"
            | "font" | "blockquote" | "hr" | "address"
            | "dl" | "dt" | "dd" | "figure" | "figcaption"
            | "sup" | "sub" | "small" | "mark" | "abbr" | "cite" | "q"
            | "center" | "nobr" | "wbr"
            | "details" | "summary" | "dialog" | "time" | "progress" | "meter"
            | "video" | "audio" | "canvas" | "source" | "picture"
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
    /// CSS `font-weight` value (raw string, e.g. `"bold"`, `"400"`), pre-normalized to lowercase.
    /// Used by the paint stage to select the appropriate web font variant.
    pub font_weight: Option<String>,
    /// CSS `font-style` value (raw string, e.g. `"italic"`, `"normal"`), pre-normalized to lowercase.
    /// Used by the paint stage to select the appropriate web font variant.
    pub font_style: Option<String>,
    /// CSS `font-family` value (raw string, first family in list).
    /// Used by the paint stage to look up web font variants.
    pub font_family: Option<String>,
    /// Resolved writing mode for the inline fragment.  Keeping this small
    /// inherited value on the fragment lets paint use the same flow direction
    /// as layout even when a nested inline overrides its ancestor.
    pub writing_mode: Option<String>,
    /// Resolved inline base direction (`ltr` or `rtl`).
    pub direction: Option<String>,
    /// Resolved CSS bidi mode used while constructing the line paragraph.
    pub unicode_bidi: Option<String>,
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

        let extract_str = |key: &str| -> Option<String> {
            match style.get(key) {
                Some(ComputedValue::Keyword(s)) => Some(s.clone()),
                Some(ComputedValue::Color(s)) => Some(s.clone()),
                Some(ComputedValue::String(s)) => Some(s.clone()),
                _ => None,
            }
        };

        let normalize_lower = |key: &str| -> Option<String> {
            extract_str(key).map(|s| s.to_ascii_lowercase())
        };

        // Extract the first font-family name from the CSS value.
        // The value may be quoted (e.g. `"My Font"`) or unquoted (e.g. `sans-serif`).
        let font_family = extract_str("font-family").map(|s| {
            // Take the first comma-separated entry and strip quotes
            let first = s.split(',').next().unwrap_or(&s).trim().to_string();
            first.trim_matches('"').trim_matches('\'').to_string()
        });

        Self {
            color: extract_str("color"),
            text_transform: normalize_lower("text-transform"),
            text_decoration_line: normalize_lower("text-decoration-line"),
            text_decoration_color: extract_str("text-decoration-color"),
            font_weight: normalize_lower("font-weight"),
            font_style: normalize_lower("font-style"),
            font_family,
            writing_mode: normalize_lower("writing-mode"),
            direction: normalize_lower("direction"),
            unicode_bidi: normalize_lower("unicode-bidi"),
            resolved_bidi_level: None,
        }
    }
}

/// A laid out fragment of inline text.
#[derive(Debug, Clone, PartialEq)]
pub struct InlineFragment {
    pub node: NodeHandle,
    pub content: InlineFragmentContent,
    pub rect: Rect,
    pub metrics: FontMetrics,
    pub vertical_align: VerticalAlign,
    /// Minimal style information extracted from the element's `ComputedStyle`.
    /// Used by the paint stage to apply per-fragment `text-transform`,
    /// `text-decoration`, and `color` rather than inheriting from the
    /// containing block's style.
    pub style: FragmentStyle,
}

/// A laid out inline fragment payload.
#[derive(Debug, Clone, PartialEq)]
pub enum InlineFragmentContent {
    Text(String),
    Image(Image, ComputedStyle),
    GeneratedBox(ComputedStyle),
    FormControl(ComputedStyle, String, Option<TextControlPaintState>),
    IconFormControl(ComputedStyle, Image, f32, f32),
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
}

/// Approximate font metrics used by inline layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub font_size: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub average_advance: f32,
    /// Extra spacing to add between each character (CSS `letter-spacing`).
    pub letter_spacing: f32,
    pub(crate) font_family: Option<FontFamilyKey>,
    pub(crate) font_weight: FontWeight,
    pub(crate) font_style: FontStyle,
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
            font_family: None,
            font_weight: FontWeight::default(),
            font_style: FontStyle::default(),
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
enum PositionScheme {
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
    match style.get("writing-mode") {
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value))
            if value.eq_ignore_ascii_case("vertical-rl")
                || value.eq_ignore_ascii_case("sideways-rl") => WritingMode::VerticalRl,
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value))
            if value.eq_ignore_ascii_case("vertical-lr")
                || value.eq_ignore_ascii_case("sideways-lr") => WritingMode::VerticalLr,
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
    matches!(
        style.get("direction"),
        Some(ComputedValue::Keyword(value) | ComputedValue::String(value))
            if value.eq_ignore_ascii_case("rtl")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatSide {
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
struct FloatRegion {
    outer: Rect,
    side: FloatSide,
}

#[derive(Debug, Clone, Copy, Default)]
struct FloatOffsets {
    left: f32,
    right: f32,
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
    pub dimensions: BoxDimensions,
    pub visibility: Visibility,
    pub overflow: Overflow,
    pub z_index: i32,
    /// Paint-time CSS transform in the document's absolute coordinate space.
    /// It does not participate in normal-flow layout sizing or placement.
    pub transform: AffineTransform,
    /// Whether this subtree can need paint-time scroll/sticky translation even
    /// when all currently stored scroll offsets are zero.
    pub(crate) needs_scroll_translation: bool,
    /// Pre-translation scroll geometry consumed by paint-only features such as
    /// `background-attachment: local`. Kept separate from CSSOM layout data.
    pub(crate) paint_scroll: Option<PaintScrollGeometry>,
    pub lines: Vec<LineBox>,
    pub children: Vec<LayoutBox>,
    /// List marker for `display: list-item` elements.
    pub marker: Option<ListMarker>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PaintScrollGeometry {
    pub offset: (f32, f32),
    pub overflow_size: (f32, f32),
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
    /// never smaller than the padding box itself.
    ///
    /// Layout coordinates are absolute (see `layout_document` / `layout_element`),
    /// so descendant border-box edges compare directly against this box's
    /// padding-box edges. Traversal stops at a descendant that clips its own
    /// overflow: that descendant scrolls its own content, so what it clips is not
    /// part of this box's scrolling area.
    pub(crate) fn scrollable_overflow(&self) -> (f32, f32) {
        let content = self.dimensions.content;
        let padding = self.dimensions.padding;
        let client_width = content.width + padding.horizontal();
        let client_height = content.height + padding.vertical();
        let padding_right_edge = content.x + content.width + padding.right;
        let padding_bottom_edge = content.y + content.height + padding.bottom;
        let mut max_right = padding_right_edge;
        let mut max_bottom = padding_bottom_edge;
        expand_scrollable_overflow(&self.children, &mut max_right, &mut max_bottom);
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
        let max_x =
            (overflow_size.0 - (content.width + padding.left + padding.right)).max(0.0);
        let max_y =
            (overflow_size.1 - (content.height + padding.top + padding.bottom)).max(0.0);
        PaintScrollGeometry {
            offset: (
                stored_x.clamp(0.0, max_x),
                stored_y.clamp(0.0, max_y),
            ),
            overflow_size,
        }
    }
}

/// Expands `max_right` / `max_bottom` to enclose the border boxes of `boxes` and
/// their descendants. A clipped axis stops contributing below that box while
/// the other axis can continue through the same subtree.
fn expand_scrollable_overflow(boxes: &[LayoutBox], max_right: &mut f32, max_bottom: &mut f32) {
    expand_scrollable_overflow_axes(boxes, max_right, max_bottom, true, true);
}

fn expand_scrollable_overflow_axes(
    boxes: &[LayoutBox],
    max_right: &mut f32,
    max_bottom: &mut f32,
    include_x: bool,
    include_y: bool,
) {
    if !include_x && !include_y {
        return;
    }
    for child in boxes {
        let content = child.dimensions.content;
        let padding = child.dimensions.padding;
        let border = child.dimensions.border;
        if include_x {
            *max_right = max_right.max(content.x + content.width + padding.right + border.right);
        }
        if include_y {
            *max_bottom = max_bottom.max(content.y + content.height + padding.bottom + border.bottom);
        }
        expand_scrollable_overflow_axes(
            &child.children,
            max_right,
            max_bottom,
            include_x && !child.overflow.clips_x(),
            include_y && !child.overflow.clips_y(),
        );
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
/// Nodes with `display: none` are omitted from the result. Non-element nodes do
/// not currently produce layout boxes.
pub fn layout_tree(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
) -> Option<LayoutBox> {
    let mut layout = layout_node(node, resolver, containing_block, containing_block, None)?;
    if resolver.has_container_queries() {
        for _ in 0..4 {
            let mut contexts = HashMap::new();
            collect_container_contexts(&layout, resolver, &mut contexts);
            if !resolver.set_container_contexts(contexts) {
                break;
            }
            layout = layout_node(node, resolver, containing_block, containing_block, None)?;
        }
    }
    populate_layout_transforms(
        &mut layout,
        resolver,
        resolver.root_font_size(),
        AffineTransform::identity(),
    );
    Some(layout)
}

fn populate_layout_transforms(
    layout: &mut LayoutBox,
    resolver: &mut StyleResolver,
    root_font_size: f32,
    parent_perspective: AffineTransform,
) {
    let style = resolver.computed_style(&layout.node);
    let transform = computed_keyword(&style, "transform").unwrap_or("none");
    let origin = computed_keyword(&style, "transform-origin").unwrap_or("50% 50%");
    let perspective_box = layout.dimensions.border_box();
    let font_size = match style.get("font-size") {
        Some(ComputedValue::Px(value)) => *value,
        _ => 16.0,
    };
    let local_transform = parse_transform_with_origin(
        transform,
        origin,
        TransformReferenceBox {
            x: perspective_box.x,
            y: perspective_box.y,
            width: perspective_box.width,
            height: perspective_box.height,
            font_size,
            root_font_size,
        },
    )
    .unwrap_or_default();
    // `perspective` is a parent effect: it projects the immediate child
    // coordinate plane while leaving the parent's own border box untouched.
    // Keeping it in the child's paint-time matrix preserves normal-flow
    // geometry and composes with the child's transform in CSS order.
    layout.transform = parent_perspective.multiply(local_transform);
    let perspective = computed_keyword(&style, "perspective").unwrap_or("none");
    let perspective_origin = computed_keyword(&style, "perspective-origin").unwrap_or("50% 50%");
    let border_box = layout.dimensions.border_box();
    let perspective_matrix = parse_perspective_with_origin(
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
    .unwrap_or_default();
    for child in &mut layout.children {
        populate_layout_transforms(child, resolver, root_font_size, perspective_matrix);
    }
    layout.needs_scroll_translation = matches!(
        style.get("position"),
        Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("sticky")
    ) || layout.children.iter().any(|child| child.needs_scroll_translation);
}

fn computed_keyword<'a>(style: &'a ComputedStyle, property: &str) -> Option<&'a str> {
    match style.get(property) {
        Some(ComputedValue::Keyword(value)) | Some(ComputedValue::String(value)) => Some(value),
        _ => None,
    }
}

/// Whether `contain` activates one of the core containment axes.
pub(crate) fn has_containment(style: &ComputedStyle, keyword: &str) -> bool {
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
    let style = resolver.computed_style(&layout.node);
    let container_type = match style.get("container-type") {
        Some(ComputedValue::Keyword(value)) => value.to_ascii_lowercase(),
        _ => "normal".to_string(),
    };
    if container_type == "inline-size" || container_type == "size" {
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
            },
        );
    }
    for child in &layout.children {
        collect_container_contexts(child, resolver, contexts);
    }
}

fn layout_node(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
) -> Option<LayoutBox> {
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
        ),
        _ => None,
    }
}

fn layout_document(
    node: &NodeHandle,
    resolver: &mut StyleResolver,
    containing_block: Rect,
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
) -> Option<LayoutBox> {
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
            && is_out_of_flow_positioned(style) {
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

    let document_box = BoxDimensions {
        content: dimensions.content,
        ..BoxDimensions::default()
    };
    for (child, style, static_position) in positioned_children {
        if let Some(positioned) = layout_positioned_child(
            &child,
            resolver,
            &style,
            positioned_ancestor.unwrap_or(document_box),
            static_position,
            viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index(&mut children);

    Some(LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: Visibility::Visible,
        overflow: Overflow::Visible,
        z_index: 0,
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        paint_scroll: None,
        lines: Vec::new(),
        children,
        marker: None,
    })
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
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    float_regions: &[FloatRegion],
    cursor_y: &mut f32,
    x: f32,
    width: f32,
    lines: &mut Vec<LineBox>,
) {
    if pending.is_empty() || all_whitespace_only(pending) {
        pending.clear();
        return;
    }
    let offsets = active_float_offsets(float_regions, *cursor_y, x, width);
    let inline_lines = layout_inline_nodes(
        pending,
        resolver,
        x + offsets.left,
        *cursor_y,
        (width - offsets.left - offsets.right).max(0.0),
        text_align(style),
        line_height(style),
        direction_is_rtl(style),
    );
    if let Some(last_line) = inline_lines.last() {
        *cursor_y = last_line.rect.y + last_line.rect.height;
    }
    lines.extend(inline_lines);
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
                *cursor_y, child_margin_top, collapse_delta, float_regions, FloatSide::Left,
            );
        }
        ClearSide::Right => {
            *cursor_y = clear_cursor_y_for_side(
                *cursor_y, child_margin_top, collapse_delta, float_regions, FloatSide::Right,
            );
        }
        ClearSide::Both => {
            *cursor_y = clear_cursor_y_for_side(
                *cursor_y, child_margin_top, collapse_delta, float_regions, FloatSide::Left,
            );
            *cursor_y = clear_cursor_y_for_side(
                *cursor_y, child_margin_top, collapse_delta, float_regions, FloatSide::Right,
            );
        }
        ClearSide::None => {}
    }
}

/// Lays out a floated child, finding a suitable vertical position and
/// registering the float region. Returns `true` when the child was
/// consumed (always; provided for control flow clarity).
fn layout_float_child(
    child: &NodeHandle,
    child_style: &ComputedStyle,
    resolver: &mut StyleResolver,
    side: FloatSide,
    child_y: f32,
    x: f32,
    width: f32,
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
    float_regions: &mut Vec<FloatRegion>,
    children: &mut Vec<LayoutBox>,
) {
    let available = (width
        - active_float_offsets(float_regions, child_y, x, width).left
        - active_float_offsets(float_regions, child_y, x, width).right)
        .max(0.0);
    let float_width = resolved_length(child_style, "width", available)
        .unwrap_or_else(|| shrink_to_fit_width(child, resolver, width));
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
        if let Some(mut layout_child) = layout_node(
            child, resolver, float_containing, viewport, positioned_ancestor,
        ) {
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
                translate_layout_box_to_outer(&mut layout_child, outer_x, outer_y);
                float_regions.push(FloatRegion {
                    outer: Rect {
                        x: outer_x,
                        y: outer_y,
                        width: layout_child.total_width(),
                        height: layout_child.total_height(),
                    },
                    side,
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

/// Advances cursor_y after laying out a block child, handling margin collapse.
fn update_cursor_after_child(
    layout_child: &LayoutBox,
    cursor_y: &mut f32,
    previous_margin_bottom: &mut Option<f32>,
    effective_collapse_delta: f32,
    collapse_delta: f32,
) {
    if is_empty_for_margin_collapse(layout_child) {
        let prev = previous_margin_bottom.unwrap_or(0.0);
        let empty_collapsed = collapse_through_empty(layout_child);
        let combined = collapse_margins(prev, empty_collapsed);
        *cursor_y += combined - prev - (effective_collapse_delta - collapse_delta);
        *previous_margin_bottom = Some(combined);
    } else {
        *cursor_y += layout_child.total_height() - effective_collapse_delta;
        *previous_margin_bottom = Some(layout_child.dimensions.margin.bottom);
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
        x: if has_explicit_width { x } else { x + offsets.left },
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
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
) -> Option<LayoutBox> {
    if is_non_rendered_html_element(node) {
        return None;
    }

    let style = resolver.computed_style(node);
    if is_display_none(&style) {
        return None;
    }

    let config = unsupported_html_config();
    if (config.logging_enabled || config.sqlite_path.is_some())
        && let Some(tag) = node.tag_name() {
            let parent_tag = node.parent_node().and_then(|p| p.tag_name());
            log_unsupported_html_tag(&tag, parent_tag.as_deref());
        }

    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    let mut margin = edge_sizes(&style, "margin");

    let mut width = compute_width(&style, containing_block.width, padding, border, &mut margin);
    if float_side(&style) != FloatSide::None
        && resolved_length(&style, "width", containing_block.width).is_none()
    {
        width = shrink_to_fit_width(node, resolver, containing_block.width);
    }
    let x = containing_block.x + margin.left + border.left + padding.left;
    let y = containing_block.y + margin.top + border.top + padding.top;

    // Replaced elements that participate as block or flex/grid items still
    // paint their image payload. Previously only inline formatting collected
    // image fragments, so `display: block` SVGs (a common Tailwind reset) had
    // a box but rendered none of their graphics.
    let is_positioned_img = node.tag_name().as_deref() == Some("img")
        && is_out_of_flow_positioned(&style);
    if !is_positioned_img && matches!(
        node.tag_name().as_deref(),
        Some("img" | "picture" | "video" | "canvas" | "svg" | "object")
    ) {
        let mut lines = layout_inline_nodes(
            std::slice::from_ref(node),
            resolver,
            x,
            y,
            width,
            text_align(&style),
            0.0,
            direction_is_rtl(&style),
        );
        if !lines.iter().any(|line| {
            line.fragments
                .iter()
                .any(|fragment| matches!(fragment.content, InlineFragmentContent::Image(_, _)))
        }) {
            lines.clear();
        }
        if !lines.is_empty() {
            let percentage_width =
                matches!(style.get("width"), Some(ComputedValue::Percentage(_)));
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
            let content_height = resolve_content_height(
                &style,
                containing_block.height,
                padding,
                border,
                y,
                cursor_y,
            );
            let mut layout = LayoutBox {
                node: node.clone(),
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
                visibility: visibility(&style),
                overflow: overflow(&style),
                z_index: z_index(&style),
                transform: AffineTransform::identity(),
                needs_scroll_translation: false,
                paint_scroll: None,
                lines,
                children: Vec::new(),
                marker: None,
            };
            apply_relative_offset(&mut layout, &style);
            return Some(layout);
        }
    }

    if is_table_container_element(node, &style) {
        let is_shrink_to_fit = resolved_length(&style, "width", containing_block.width).is_none();
        if is_shrink_to_fit {
            width = shrink_to_fit_width(node, resolver, containing_block.width);
            redistribute_auto_margins_for_table(
                &style, width, &padding, &border, &mut margin, containing_block.width,
            );
        }
        let x = containing_block.x + margin.left + border.left + padding.left;
        return layout_table_container(
            node, resolver, style, margin, padding, border, x, y, width, viewport,
            is_shrink_to_fit,
        );
    }

    if is_flex_container(&style) {
        return layout_flex_container(
            node, resolver, style, margin, padding, border, x, y, width, viewport,
        );
    }
    if is_grid_container(&style) {
        return layout_grid_container(
            node, resolver, style, margin, padding, border, x, y, width,
            containing_block.height, viewport,
        );
    }

    let BlockChildrenResult {
        mut children, lines, cursor_y, float_bottom, positioned_children,
    } = layout_block_children(
        node, resolver, &style, padding, border, margin,
        x, y, width, containing_block.height, viewport, positioned_ancestor,
    );

    let effective_cursor_y = cursor_y.max(float_bottom);
    let content_height = resolve_content_height(
        &style, containing_block.height, padding, border, y, effective_cursor_y,
    );

    let dimensions = BoxDimensions {
        content: Rect { x, y, width, height: content_height },
        padding, border, margin,
    };

    // Resolve positioned children using the final dimensions (content_height is
    // now known, which is required for absolute positioning relative to this box).
    let next_pos_ancestor = if establishes_positioned_containing_block(&style) {
        Some(dimensions)
    } else {
        positioned_ancestor
    };
    for (child, cs, static_position) in positioned_children {
        if let Some(positioned) = layout_positioned_child(
            &child, resolver, &cs,
            next_pos_ancestor.unwrap_or(dimensions),
            static_position, viewport,
        ) {
            children.push(positioned);
        }
    }
    sort_children_by_z_index(&mut children);

    let marker = build_list_marker(node, &style, x, y);
    let mut layout = LayoutBox {
        node: node.clone(),
        dimensions,
        visibility: visibility(&style),
        overflow: overflow(&style),
        z_index: z_index(&style),
        transform: AffineTransform::identity(),
        needs_scroll_translation: false,
        paint_scroll: None,
        lines,
        children,
        marker,
    };
    apply_relative_offset(&mut layout, &style);
    Some(layout)
}

struct BlockChildrenResult {
    children: Vec<LayoutBox>,
    lines: Vec<LineBox>,
    cursor_y: f32,
    float_bottom: f32,
    positioned_children: Vec<(NodeHandle, ComputedStyle, Rect)>,
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
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
) -> BlockChildrenResult {
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
        );
    }

    let child_height_basis = resolved_length(style, "height", containing_height)
        .map(|height| border_box_adjust_height(style, height, &padding, &border))
        .unwrap_or(0.0);
    let mut children = Vec::new();
    let mut positioned_children = Vec::new();
    let mut lines = Vec::new();
    let mut cursor_y = y;
    let mut previous_margin_bottom: Option<f32> = None;
    let mut pending_inline_nodes = Vec::new();
    let mut float_regions = Vec::new();

    for child in node.layout_child_nodes() {
        // Comments do not generate boxes and do not interrupt an inline
        // formatting context. Keeping pending text together also preserves
        // word-boundary behavior across framework hydration comments.
        if child.node_type() == NodeType::Comment {
            continue;
        }
        if is_inline_child(&child, resolver) {
            pending_inline_nodes.push(child);
            continue;
        }

        flush_pending_inline_nodes(
            &mut pending_inline_nodes, resolver, style,
            &float_regions, &mut cursor_y, x, width, &mut lines,
        );

        let child_style = match child.node_type() {
            NodeType::Element => Some(resolver.computed_style(&child)),
            _ => None,
        };
        let child_margin_top = child_style
            .as_ref()
            .map(|s| edge_sizes(s, "margin").top)
            .unwrap_or(0.0);
        let collapse_delta = previous_margin_bottom
            .map(|mb| mb + child_margin_top - collapse_margins(mb, child_margin_top))
            .unwrap_or(0.0);

        if let Some(cs) = &child_style {
            apply_clear(&mut cursor_y, cs, child_margin_top, collapse_delta, &float_regions);
        }

        if let Some(child_style) = &child_style {
            let parent_top_collapse = previous_margin_bottom.is_none()
                && lines.is_empty()
                && pending_inline_nodes.is_empty()
                && !has_containment(style, "layout")
                && border.top == 0.0
                && padding.top == 0.0
                && clear_side(child_style) == ClearSide::None
                && !is_out_of_flow_positioned(child_style)
                && float_side(child_style) == FloatSide::None;
            let effective_collapse_delta = if parent_top_collapse {
                collapse_delta + child_margin_top
            } else {
                collapse_delta
            };
            let child_y = cursor_y - effective_collapse_delta;
            let offsets = active_float_offsets(&float_regions, child_y, x, width);
            let child_containing = child_containing_rect(
                child_style,
                child_y,
                &offsets,
                x,
                width,
                child_height_basis,
            );

            if is_out_of_flow_positioned(child_style) {
                positioned_children.push((child, child_style.clone(), child_containing));
                continue;
            }

            let side = float_side(child_style);
            if side != FloatSide::None {
                layout_float_child(
                    &child, child_style, resolver, side, child_y, x, width,
                    viewport, positioned_ancestor, &mut float_regions, &mut children,
                );
                continue;
            }

            let next_pos_ancestor = if establishes_positioned_containing_block(style) {
                Some(BoxDimensions {
                    content: Rect { x, y, width, height: 0.0 },
                    padding, border, margin,
                })
            } else {
                positioned_ancestor
            };
            if let Some(layout_child) = layout_node(
                &child, resolver, child_containing, viewport, next_pos_ancestor,
            ) {
                update_cursor_after_child(
                    &layout_child, &mut cursor_y, &mut previous_margin_bottom,
                    effective_collapse_delta, collapse_delta,
                );
                children.push(layout_child);
            }
            continue;
        }

        // Non-element child (comment, etc.) — fallback path
        let child_containing = Rect {
            x, y: cursor_y - collapse_delta, width, height: 0.0,
        };
        if let Some(layout_child) = layout_node(
            &child, resolver, child_containing, viewport, positioned_ancestor,
        ) {
            update_cursor_after_child(
                &layout_child, &mut cursor_y, &mut previous_margin_bottom,
                collapse_delta, collapse_delta,
            );
            children.push(layout_child);
        }
    }

    flush_pending_inline_nodes(
        &mut pending_inline_nodes, resolver, style,
        &float_regions, &mut cursor_y, x, width, &mut lines,
    );

    let float_bottom = float_regions
        .iter()
        .map(|region| region.outer.y + region.outer.height)
        .fold(y, f32::max);

    sort_children_by_z_index(&mut children);

    BlockChildrenResult {
        children,
        lines,
        cursor_y,
        float_bottom,
        positioned_children,
    }
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
    viewport: Rect,
    positioned_ancestor: Option<BoxDimensions>,
) -> BlockChildrenResult {
    let vertical_rl = is_vertical_rl(style);
    let mut children = Vec::new();
    let mut positioned_children = Vec::new();
    let mut lines = Vec::new();
    let mut pending_inline_nodes = Vec::new();
    let mut cursor_x = if vertical_rl { x + width } else { x };
    let mut inline_bottom = y;
    // An auto-height root often enters layout with a zero containing height.
    // Vertical inline layout still needs a finite line-breaking basis; use an
    // explicit height when present, otherwise a generous unbounded basis so
    // content can establish the auto height naturally.
    let available_inline_height = resolved_length(style, "height", containing_height)
        .or_else(|| (containing_height > 0.0).then_some(containing_height))
        .unwrap_or(1_000_000.0);

    for child in node.layout_child_nodes() {
        if child.node_type() == NodeType::Comment {
            continue;
        }
        if is_inline_child(&child, resolver) {
            pending_inline_nodes.push(child);
            continue;
        }

        flush_pending_vertical_inline_nodes(
            &mut pending_inline_nodes,
            resolver,
            style,
            x,
            y,
            width,
            available_inline_height,
            &mut cursor_x,
            vertical_rl,
            &mut lines,
            &mut inline_bottom,
        );

        let child_style = match child.node_type() {
            NodeType::Element => Some(resolver.computed_style(&child)),
            _ => None,
        };
        let Some(child_style) = child_style else {
            continue;
        };

        // Positioned descendants are resolved after the containing block's
        // final dimensions are known, just as in the horizontal path.
        let provisional_x = if vertical_rl {
            cursor_x - width
        } else {
            cursor_x
        };
        let child_containing = Rect {
            x: provisional_x,
            y,
            width,
            height: available_inline_height,
        };
        if is_out_of_flow_positioned(&child_style) {
            positioned_children.push((child, child_style, child_containing));
            continue;
        }

        let next_pos_ancestor = if establishes_positioned_containing_block(style) {
            Some(BoxDimensions {
                content: Rect { x, y, width, height: 0.0 },
                padding,
                border,
                margin,
            })
        } else {
            positioned_ancestor
        };
        let Some(mut layout_child) = layout_node(
            &child,
            resolver,
            child_containing,
            viewport,
            next_pos_ancestor,
        ) else {
            continue;
        };

        let outer_x = if vertical_rl {
            cursor_x - layout_child.total_width()
        } else {
            cursor_x
        };
        translate_layout_box_to_outer(&mut layout_child, outer_x, y);
        if vertical_rl {
            cursor_x = outer_x;
        } else {
            cursor_x = outer_x + layout_child.total_width();
        }
        inline_bottom = inline_bottom.max(y + layout_child.total_height());
        children.push(layout_child);
    }

    flush_pending_vertical_inline_nodes(
        &mut pending_inline_nodes,
        resolver,
        style,
        x,
        y,
        width,
        available_inline_height,
        &mut cursor_x,
        vertical_rl,
        &mut lines,
        &mut inline_bottom,
    );
    sort_children_by_z_index(&mut children);

    BlockChildrenResult {
        children,
        lines,
        // The vertical formatter uses cursor_y as the inline-axis extent so
        // the existing height resolver can still apply explicit/min/max
        // height declarations without a second sizing pipeline.
        cursor_y: inline_bottom,
        float_bottom: inline_bottom,
        positioned_children,
    }
}

#[allow(clippy::too_many_arguments)]
fn flush_pending_vertical_inline_nodes(
    pending: &mut Vec<NodeHandle>,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    cursor_x: &mut f32,
    vertical_rl: bool,
    lines: &mut Vec<LineBox>,
    inline_bottom: &mut f32,
) {
    if pending.is_empty() || all_whitespace_only(pending) {
        pending.clear();
        return;
    }

    let (region_x, region_width) = if vertical_rl {
        (x, (*cursor_x - x).max(0.0))
    } else {
        (*cursor_x, (x + width - *cursor_x).max(0.0))
    };
    let inline_lines = layout_vertical_inline_nodes(
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
    );
    if let Some(last_line) = inline_lines
        .iter()
        .map(|line| line.rect.y + line.rect.height)
        .reduce(f32::max)
    {
        *inline_bottom = (*inline_bottom).max(last_line);
    }
    let used_width = inline_lines
        .iter()
        .map(|line| line.rect.width)
        .sum::<f32>();
    if vertical_rl {
        *cursor_x = (*cursor_x - used_width).max(x);
    } else {
        *cursor_x = (*cursor_x + used_width).min(x + width);
    }
    lines.extend(inline_lines);
    pending.clear();
}

/// Resolves the content height for a block element, considering explicit height,
/// min/max constraints, border-box mode, and auto height from content.
///
/// `cursor_y` is the bottom edge of all content (including floats).
fn resolve_content_height(
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
        0.0
    } else {
        (cursor_y - y).max(0.0)
    };
    let mut height = resolved_length(style, "height", containing_height)
        .map(|h| if border_box { (h - pb_vertical).max(0.0) } else { h })
        .unwrap_or(auto_height);
    let (min_h, max_h) =
        normalized_min_max_lengths(style, "min-height", "max-height", containing_height);
    if let Some(min_h) = min_h {
        let content_min = if border_box { (min_h - pb_vertical).max(0.0) } else { min_h };
        height = height.max(content_min);
    }
    if let Some(max_h) = max_h {
        let content_max = if border_box { (max_h - pb_vertical).max(0.0) } else { max_h };
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

fn compute_width(
    style: &ComputedStyle,
    containing_width: f32,
    padding: EdgeSizes,
    border: EdgeSizes,
    margin: &mut EdgeSizes,
) -> f32 {
    let pb_horizontal = padding.horizontal() + border.horizontal();

    // `specified_width` is the value given to the `width` property.
    // For border-box, that value already includes padding + border, so we
    // convert it to a content-box width immediately.
    let specified_width = resolved_length(style, "width", containing_width)
        .map(|w| border_box_adjust_length(style, w, padding.left + border.left, padding.right + border.right));
    let margin_left_auto = margin_start_is_auto(style);
    let margin_right_auto = margin_end_is_auto(style);

    let mut width = if let Some(width) = specified_width {
        let remaining =
            (containing_width - width - pb_horizontal).max(0.0);

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

        (containing_width - pb_horizontal - margin.horizontal()).max(0.0)
    };

    // For border-box, min-width / max-width also refer to the outer (border)
    // box, so subtract padding + border before comparing.
    let (min_width, max_width) =
        normalized_min_max_lengths(style, "min-width", "max-width", containing_width);
    if let Some(min_width) = min_width {
        let content_min = border_box_adjust_length(
            style,
            min_width,
            padding.left + border.left,
            padding.right + border.right,
        );
        width = width.max(content_min);
    }
    if let Some(max_width) = max_width {
        let content_max = border_box_adjust_length(
            style,
            max_width,
            padding.left + border.left,
            padding.right + border.right,
        );
        width = width.min(content_max);
    }

    if margin_left_auto || margin_right_auto {
        let remaining =
            (containing_width - width - pb_horizontal).max(0.0);
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
    let shorthand_property = match prefix {
        "border" => "border-width",
        _ => prefix,
    };
    let side_property = match prefix {
        "border" => "border-{}-width",
        _ => "{prefix}-{}",
    };
    let shorthand = explicit_length(style, shorthand_property).unwrap_or(0.0);
    let physical = |side: &str| {
        explicit_length(
            style,
            &side_property
                .replace("{}", side)
                .replace("{prefix}", prefix),
        )
        .or_else(|| explicit_length(style, &format!("{prefix}-{side}")))
    };

    // Logical properties are resolved against the element's own writing mode,
    // not the horizontal defaults used by the CSS cascade's physical aliases.
    // Prefer them for vertical boxes so `padding-inline-start` maps to the
    // inline (top/bottom) axis and `padding-block-start` maps to the column
    // (left/right) axis.  Horizontal boxes retain the historical physical
    // precedence, which also preserves the CSS cascade tests for a later
    // `padding-left` overriding an earlier logical declaration.
    if is_vertical_writing(style) {
        let inline_start = logical_side_length(style, prefix, "inline", true);
        let inline_end = logical_side_length(style, prefix, "inline", false);
        let block_start = logical_side_length(style, prefix, "block", true);
        let block_end = logical_side_length(style, prefix, "block", false);
        let rtl = direction_is_rtl(style);
        let top = if rtl { inline_end } else { inline_start };
        let bottom = if rtl { inline_start } else { inline_end };
        let (left, right) = if matches!(writing_mode(style), WritingMode::VerticalRl) {
            (block_end, block_start)
        } else {
            (block_start, block_end)
        };
        return EdgeSizes {
            top: top.or_else(|| physical("top")).unwrap_or(shorthand),
            right: right.or_else(|| physical("right")).unwrap_or(shorthand),
            bottom: bottom.or_else(|| physical("bottom")).unwrap_or(shorthand),
            left: left.or_else(|| physical("left")).unwrap_or(shorthand),
        };
    }

    let rtl = direction_is_rtl(style);
    let inline_start = logical_side_length(style, prefix, "inline", true);
    let inline_end = logical_side_length(style, prefix, "inline", false);
    let block_start = logical_side_length(style, prefix, "block", true);
    let block_end = logical_side_length(style, prefix, "block", false);
    let left_logical = if rtl { inline_end } else { inline_start };
    let right_logical = if rtl { inline_start } else { inline_end };
    EdgeSizes {
        top: physical("top").or(block_start).unwrap_or(shorthand),
        right: physical("right").or(right_logical).unwrap_or(shorthand),
        bottom: physical("bottom").or(block_end).unwrap_or(shorthand),
        left: physical("left").or(left_logical).unwrap_or(shorthand),
    }
}

fn logical_side_length(
    style: &ComputedStyle,
    prefix: &str,
    axis: &str,
    start: bool,
) -> Option<f32> {
    let side = if start { "start" } else { "end" };
    explicit_length(style, &format!("{prefix}-{axis}-{side}"))
}

fn explicit_length(style: &ComputedStyle, property: &str) -> Option<f32> {
    match style.get(property) {
        Some(ComputedValue::Px(value)) => Some(*value),
        // CSS 2.1: unitless numbers are only valid as lengths when the value is 0
        Some(ComputedValue::Number(value)) if *value == 0.0 => Some(0.0),
        _ => None,
    }
}

fn percentage_length(style: &ComputedStyle, property: &str) -> Option<f32> {
    match style.get(property) {
        Some(ComputedValue::Percentage(value)) => Some(*value),
        _ => None,
    }
}

fn resolved_length(style: &ComputedStyle, property: &str, basis: f32) -> Option<f32> {
    let resolved = explicit_length(style, property)
        .or_else(|| {
            percentage_length(style, property).and_then(|percent| {
                if basis > 0.0 {
                    Some(basis * (percent / 100.0))
                } else {
                    None
                }
            })
        })
        .or_else(|| {
            // Resolve calc(px + %) using the provided basis.
            // Only resolve when basis is known (> 0); otherwise leave unresolved.
            match style.get(property) {
                Some(ComputedValue::CalcPxPercent(px, pct)) if basis > 0.0 => {
                    Some(px + basis * (pct / 100.0))
                }
                _ => None,
            }
        });
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
    if is_vertical_writing(style) {
        is_auto(style.get("margin-left")) || is_auto(style.get("margin-block-start"))
    } else {
        is_auto(style.get("margin-left"))
            || is_auto(style.get("margin-inline-start"))
    }
}

fn margin_end_is_auto(style: &ComputedStyle) -> bool {
    if is_vertical_writing(style) {
        is_auto(style.get("margin-right")) || is_auto(style.get("margin-block-end"))
    } else {
        is_auto(style.get("margin-right")) || is_auto(style.get("margin-inline-end"))
    }
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

/// CSS 2.1 section 8.3.1: An element is "empty" for margin collapsing when it has
/// zero height, zero vertical border/padding, no line boxes, and all
/// children (if any) are themselves empty for margin collapsing.
fn is_empty_for_margin_collapse(layout: &LayoutBox) -> bool {
    layout.dimensions.content.height == 0.0
        && layout.dimensions.padding.top == 0.0
        && layout.dimensions.padding.bottom == 0.0
        && layout.dimensions.border.top == 0.0
        && layout.dimensions.border.bottom == 0.0
        && layout.lines.is_empty()
        && layout
            .children
            .iter()
            .all(is_empty_for_margin_collapse)
}

/// Collapse all margins through an empty element and its empty descendants.
/// Returns the single collapsed margin value that represents the entire chain.
fn collapse_through_empty(layout: &LayoutBox) -> f32 {
    let mut result = collapse_margins(
        layout.dimensions.margin.top,
        layout.dimensions.margin.bottom,
    );
    for child in &layout.children {
        result = collapse_margins(result, collapse_through_empty(child));
    }
    result
}

// ── Positioning helpers ─────────────────────────────────────────────────────

fn is_out_of_flow_positioned(style: &ComputedStyle) -> bool {
    matches!(
        position_scheme(style),
        PositionScheme::Absolute | PositionScheme::Fixed
    )
}

fn establishes_positioned_containing_block(style: &ComputedStyle) -> bool {
    matches!(
        position_scheme(style),
        PositionScheme::Relative | PositionScheme::Sticky | PositionScheme::Absolute | PositionScheme::Fixed
    ) || has_containment(style, "layout") || has_containment(style, "paint")
}

fn position_scheme(style: &ComputedStyle) -> PositionScheme {
    match style.get("position") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("relative") => {
            PositionScheme::Relative
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("sticky") => {
            PositionScheme::Sticky
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("absolute") => {
            PositionScheme::Absolute
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("fixed") => {
            PositionScheme::Fixed
        }
        _ => PositionScheme::Static,
    }
}

fn float_side(style: &ComputedStyle) -> FloatSide {
    match style.get("float") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("left") => {
            FloatSide::Left
        }
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("right") => {
            FloatSide::Right
        }
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
    let padding = edge_sizes(&style, "padding");
    let border = edge_sizes(&style, "border");
    let mut margin = edge_sizes(&style, "margin");
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
                use crate::layout::inline::split_words_preserving_spaces_cjk;
                let white_space_mode = white_space(&parent_style);
                let normalized = normalize_text(&text, white_space_mode);
                if !white_space_mode.allows_wrapping() {
                    return measure_text_width(&normalized, metrics);
                }
                split_words_preserving_spaces_cjk(&normalized)
                    .into_iter()
                    .map(|word| measure_text_width(&word, metrics))
                    .fold(0.0f32, f32::max)
            })
            .unwrap_or(0.0),
        NodeType::Element => {
            let style = resolver.computed_style(node);
            if is_display_none(&style) || is_non_rendered_html_element(node) {
                return 0.0;
            }
            let padding = edge_sizes(&style, "padding");
            let border = edge_sizes(&style, "border");
            // Elements with explicit width use that as minimum.
            if let Some(width) = explicit_length(&style, "width") {
                let margin = edge_sizes(&style, "margin");
                return width + padding.horizontal() + border.horizontal() + margin.horizontal();
            }
            if has_inline_size_containment(&style) {
                let margin = edge_sizes(&style, "margin");
                return padding.horizontal() + border.horizontal() + margin.horizontal();
            }
            // For images, use rendered size.
            if let Some((image_node, image)) = element_inline_image(node) {
                let image_style = resolver.computed_style(&image_node);
                let (rendered_width, _) = resolve_image_rendered_size(&image_node, &image, &image_style);
                let img_padding = edge_sizes(&image_style, "padding");
                let img_border = edge_sizes(&image_style, "border");
                return rendered_width + img_padding.horizontal() + img_border.horizontal();
            }
            // Recurse: minimum of children's minimum widths
            let mut min_width = 0.0f32;
            for child in node.layout_child_nodes() {
                min_width = min_width.max(minimum_content_width(&child, resolver));
            }
            min_width + padding.horizontal() + border.horizontal()
        }
        _ => 0.0,
    }
}

fn intrinsic_width(node: &NodeHandle, resolver: &mut StyleResolver) -> f32 {
    match node.node_type() {
        NodeType::Text => node
            .data()
            .map(|text| {
                let parent_style = node
                    .parent_node()
                    .map(|parent| resolver.computed_style(&parent))
                    .unwrap_or_default();
                measure_text_width(
                    &normalize_text(&text, white_space(&parent_style)),
                    font_metrics(&parent_style),
                )
            })
            .unwrap_or(0.0),
        NodeType::Element => {
            let style = resolver.computed_style(node);
            if is_display_none(&style) || is_non_rendered_html_element(node) {
                return 0.0;
            }
            let padding = edge_sizes(&style, "padding");
            let border = edge_sizes(&style, "border");
            if let Some(width) = explicit_length(&style, "width") {
                let margin = edge_sizes(&style, "margin");
                return width + padding.horizontal() + border.horizontal() + margin.horizontal();
            }
            if has_inline_size_containment(&style) {
                let margin = edge_sizes(&style, "margin");
                return padding.horizontal() + border.horizontal() + margin.horizontal();
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
                let direction = flex_direction(&style);
                let mut content_width = 0.0f32;
                for child in node.layout_child_nodes() {
                    if child.node_type() != NodeType::Element {
                        continue;
                    }
                    let child_style = resolver.computed_style(&child);
                    if is_display_none(&child_style) {
                        continue;
                    }
                    let child_width = intrinsic_width(&child, resolver);
                    match direction {
                        FlexDirection::Row => content_width += child_width,
                        FlexDirection::Column => content_width = content_width.max(child_width),
                    }
                }
                return content_width + padding.horizontal() + border.horizontal();
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
                    let child_width = intrinsic_width(&child, resolver);
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
                            measure_text_width(&text, segment.metrics)
                        }
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

fn apply_relative_offset(layout: &mut LayoutBox, style: &ComputedStyle) {
    if position_scheme(style) != PositionScheme::Relative {
        return;
    }

    let dx = explicit_length(style, "left").unwrap_or(0.0)
        - explicit_length(style, "right").unwrap_or(0.0);
    let dy = explicit_length(style, "top").unwrap_or(0.0)
        - explicit_length(style, "bottom").unwrap_or(0.0);

    if dx != 0.0 || dy != 0.0 {
        translate_layout_box(layout, dx, dy);
    }
}

// ── Positioned child layout ─────────────────────────────────────────────────

fn layout_positioned_child(
    child: &NodeHandle,
    resolver: &mut StyleResolver,
    style: &ComputedStyle,
    parent_box: BoxDimensions,
    containing_block: Rect,
    viewport: Rect,
) -> Option<LayoutBox> {
    let position = position_scheme(style);
    let origin = match position {
        PositionScheme::Fixed => viewport,
        PositionScheme::Absolute => parent_box.content,
        PositionScheme::Static => containing_block,
        PositionScheme::Relative => containing_block,
        PositionScheme::Sticky => containing_block,
    };

    let rtl = direction_is_rtl(style);
    let (left_logical, right_logical, top_logical, bottom_logical) = if is_vertical_writing(style) {
        // In vertical writing the inline axis is physical y.  Direction
        // reverses inline start/end, while writing-mode chooses the physical
        // block start/end edge for the x axis.
        let inline_start = resolved_length(style, "inset-inline-start", origin.height);
        let inline_end = resolved_length(style, "inset-inline-end", origin.height);
        let block_start = resolved_length(style, "inset-block-start", origin.width);
        let block_end = resolved_length(style, "inset-block-end", origin.width);
        let (top, bottom) = if rtl {
            (inline_end, inline_start)
        } else {
            (inline_start, inline_end)
        };
        let (left, right) = if matches!(writing_mode(style), WritingMode::VerticalRl) {
            (block_end, block_start)
        } else {
            (block_start, block_end)
        };
        (left, right, top, bottom)
    } else {
        let inline_start = resolved_length(style, "inset-inline-start", origin.width);
        let inline_end = resolved_length(style, "inset-inline-end", origin.width);
        let (left, right) = if rtl {
            (inline_end, inline_start)
        } else {
            (inline_start, inline_end)
        };
        (
            left,
            right,
            resolved_length(style, "inset-block-start", origin.height),
            resolved_length(style, "inset-block-end", origin.height),
        )
    };
    let left = resolved_length(style, "left", origin.width).or(left_logical);
    let right = resolved_length(style, "right", origin.width).or(right_logical);
    let top = resolved_length(style, "top", origin.height).or(top_logical);
    let bottom = resolved_length(style, "bottom", origin.height).or(bottom_logical);
    let static_outer = containing_block;
    let specified_width = resolved_length(style, "width", origin.width);
    let child_width = if specified_width.is_none() {
        shrink_to_fit_layout_width(child, resolver, origin.width)
    } else {
        origin.width
    };
    let child_containing = Rect {
        x: origin.x,
        y: origin.y,
        width: child_width,
        height: origin.height,
    };
    let mut layout_child = layout_node(
        child,
        resolver,
        child_containing,
        viewport,
        Some(parent_box),
    )?;
    if specified_width.is_none() {
        let auto_width = auto_width_from_layout(&layout_child, child, resolver, origin.width);
        if (auto_width - layout_child.dimensions.content.width).abs() > 0.5 {
            let relayout_containing = Rect {
                width: auto_width,
                ..child_containing
            };
            layout_child = layout_node(
                child,
                resolver,
                relayout_containing,
                viewport,
                Some(parent_box),
            )?;
        }
        layout_child.dimensions.content.width =
            auto_width_from_layout(&layout_child, child, resolver, origin.width);
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
    translate_layout_box_to_outer(&mut layout_child, outer_x, outer_y);
    layout_child.z_index = z_index(style);
    Some(layout_child)
}

fn sort_children_by_z_index(children: &mut [LayoutBox]) {
    children.sort_by_key(|child| child.z_index);
}

// ── Box translation helpers ─────────────────────────────────────────────────

fn translate_layout_box_to_outer(layout: &mut LayoutBox, outer_x: f32, outer_y: f32) {
    let current_outer_x = layout.dimensions.content.x
        - layout.dimensions.padding.left
        - layout.dimensions.border.left
        - layout.dimensions.margin.left;
    let current_outer_y = layout.dimensions.content.y
        - layout.dimensions.padding.top
        - layout.dimensions.border.top
        - layout.dimensions.margin.top;
    translate_layout_box(layout, outer_x - current_outer_x, outer_y - current_outer_y);
}

fn translate_layout_box(layout: &mut LayoutBox, dx: f32, dy: f32) {
    layout.dimensions.content.x += dx;
    layout.dimensions.content.y += dy;
    translate_layout_contents(layout, dx, dy);
}

fn translate_layout_contents(layout: &mut LayoutBox, dx: f32, dy: f32) {
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
        translate_layout_box(child, dx, dy);
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
            if let Some(ComputedValue::Keyword(keyword)) = style.get("display") {
                return keyword.eq_ignore_ascii_case("inline")
                    || keyword.eq_ignore_ascii_case("inline-block");
            }
            node.tag_name()
                .map(|tag| {
                    matches!(
                        tag.as_str(),
                        "span" | "a" | "em" | "strong" | "b" | "i" | "img" | "object" | "svg"
                            | "input" | "button" | "textarea" | "select"
                            | "time" | "progress" | "meter"
                            | "video" | "audio" | "canvas" | "picture"
                    )
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

// ── Display / visibility / overflow ─────────────────────────────────────────

fn is_non_rendered_html_element(node: &NodeHandle) -> bool {
    matches!(
        node.tag_name().as_deref(),
        Some(
            "head" | "title" | "meta" | "style" | "script" | "link" | "noscript" | "source"
        )
    )
}

fn is_display_none(style: &ComputedStyle) -> bool {
    matches!(
        style.get("display"),
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("none")
    )
}

fn visibility(style: &ComputedStyle) -> Visibility {
    match style.get("visibility") {
        Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("hidden") => {
            Visibility::Hidden
        }
        _ => Visibility::Visible,
    }
}

fn overflow(style: &ComputedStyle) -> Overflow {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Axis {
        Visible,
        Clip,
        Scroll,
    }

    fn axis(style: &ComputedStyle, property: &str) -> Axis {
        match style.get(property) {
            Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("clip") => {
                Axis::Clip
            }
            Some(ComputedValue::Keyword(keyword)) if overflow_keyword_scrolls(keyword) => {
                Axis::Scroll
            }
            _ => Axis::Visible,
        }
    }

    let mut x = axis(style, "overflow-x");
    let mut y = axis(style, "overflow-y");
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
        style.get("display"),
        Some(ComputedValue::Keyword(kw)) if kw.eq_ignore_ascii_case("list-item")
    );
    if !display_is_list_item {
        return None;
    }

    // Determine list-style-type.
    let list_style_type = match style.get("list-style-type") {
        Some(ComputedValue::Keyword(kw)) => kw.to_ascii_lowercase(),
        _ => "disc".to_string(),
    };

    if list_style_type == "none" {
        return None;
    }

    let font_size = match style.get("font-size") {
        Some(ComputedValue::Px(px)) => *px,
        _ => 16.0,
    };

    let outside = !matches!(
        style.get("list-style-position"),
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

    Some(ListMarker { text, font_size, outside, x, y })
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
fn to_roman_lower(n: usize) -> String {
    to_roman_inner(n).to_ascii_lowercase()
}

/// Converts a number to uppercase Roman numerals.
fn to_roman_upper(n: usize) -> String {
    to_roman_inner(n)
}

fn to_roman_inner(mut n: usize) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const VALUES: &[(usize, &str)] = &[
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"),
        (100, "C"),  (90, "XC"),  (50, "L"),  (40, "XL"),
        (10, "X"),   (9, "IX"),   (5, "V"),   (4, "IV"),
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
fn to_alpha_lower(n: usize) -> String {
    to_alpha_inner(n).to_ascii_lowercase()
}

/// Converts a 1-based number to an uppercase Latin letter (A, B, ..., Z, AA, ...).
fn to_alpha_upper(n: usize) -> String {
    to_alpha_inner(n)
}

fn to_alpha_inner(mut n: usize) -> String {
    if n == 0 {
        return "a".to_string();
    }
    let mut result = String::new();
    while n > 0 {
        n -= 1;
        result.insert(0, char::from_u32(b'A' as u32 + (n % 26) as u32).unwrap_or('A'));
        n /= 26;
    }
    result
}

#[cfg(test)]
mod tests;
