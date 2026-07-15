//! CSS cascade and computed style resolution.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::dom::{Node, NodeHandle, NodeType};
use rusqlite::{Connection, params};

use super::{
    Declaration, MediaQuery, PseudoElement, Rule, Specificity, Stylesheet, Value,
    evaluate_media_query, matches_selector_with_pseudo, parse_media_query_list, specificity,
};

/// CSS origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    UserAgent,
    User,
    Author,
}

/// A property value after computation.
#[derive(Debug, Clone, PartialEq)]
pub enum ComputedValue {
    Keyword(String),
    Px(f32),
    Percentage(f32),
    Color(String),
    String(String),
    Number(f32),
    /// `calc()` expression with mixed px and percentage: `px_value + percent_value% of basis`.
    /// Resolved at layout time using `resolved_length(basis)`.
    CalcPxPercent(f32, f32),
}

/// Resolved computed style for a node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComputedStyle {
    properties: BTreeMap<String, ComputedValue>,
}

impl ComputedStyle {
    /// Returns a computed property.
    pub fn get(&self, name: &str) -> Option<&ComputedValue> {
        self.properties.get(name)
    }

    /// Returns all computed properties.
    pub fn properties(&self) -> &BTreeMap<String, ComputedValue> {
        &self.properties
    }
}

/// A stylesheet together with its cascade origin.
#[derive(Debug, Clone)]
pub struct StylesheetInput {
    pub origin: Origin,
    pub stylesheet: Stylesheet,
}

/// Context used when converting CSS values to computed px values.
#[derive(Debug, Clone, Copy)]
struct ResolutionContext {
    /// The parent element's computed font-size in px (used for `em` units).
    parent_font_size: f32,
    /// The root element's computed font-size in px (used for `rem` units).
    root_font_size: f32,
    /// Viewport width in px (used for `vw`, `vmin`, `vmax`).
    viewport_width: f32,
    /// Viewport height in px (used for `vh`, `vmin`, `vmax`).
    viewport_height: f32,
}

impl Default for ResolutionContext {
    fn default() -> Self {
        Self {
            parent_font_size: 16.0,
            root_font_size: 16.0,
            viewport_width: 0.0,
            viewport_height: 0.0,
        }
    }
}

/// Computes styles and caches results per node.
#[derive(Debug, Default)]
pub struct StyleResolver {
    stylesheets: Vec<StylesheetInput>,
    cache: HashMap<usize, ComputedStyle>,
    pseudo_cache: HashMap<(usize, PseudoElement), ComputedStyle>,
    /// Root element's computed font-size in px (for `rem` unit resolution).
    root_font_size: f32,
    /// `true` when `root_font_size` was explicitly set via `set_root_font_size()`,
    /// preventing auto-update from the computed root element style.
    root_font_size_explicit: bool,
    /// Viewport width in px (for `vw`, `vmin`, `vmax` resolution).
    viewport_width: f32,
    /// Viewport height in px (for `vh`, `vmin`, `vmax` resolution).
    viewport_height: f32,
    /// `true` when the system is in dark mode (affects `prefers-color-scheme` evaluation).
    color_scheme_dark: bool,
    /// Cache of parsed media query lists keyed by the normalized (trimmed) prelude string.
    ///
    /// Avoids re-parsing the same `@media` prelude string for every node that
    /// is matched against the stylesheet.  The cache is intentionally separate
    /// from the per-node `cache` so it survives `cache.clear()` calls (e.g.
    /// after `set_color_scheme_dark`).  The parsed `Vec<MediaQuery>` is stable:
    /// it depends only on the prelude text, not on viewport dimensions or
    /// color-scheme settings.
    media_query_cache: HashMap<String, Vec<MediaQuery>>,
    /// Parsed `@keyframes` rules keyed by animation name.
    keyframes: HashMap<String, Vec<KeyframeStep>>,
}

#[derive(Debug, Clone)]
struct KeyframeStep {
    offset: f32,
    declarations: Vec<Declaration>,
}

/// Deterministic post-load instant used for static screenshots.
const STATIC_ANIMATION_TIME_SECONDS: f32 = 1.2;

static UNSUPPORTED_CSS_LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static UNSUPPORTED_CSS_CONFIG: OnceLock<UnsupportedCssConfig> = OnceLock::new();
static SQLITE_LOG_ERRORS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static UNSUPPORTED_CSS_TOP_N_LAST_DIGEST: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
const MAX_UNSUPPORTED_LOG_KEYS: usize = 4096;
const MAX_UNSUPPORTED_LOG_VALUE_LEN: usize = 256;
const MAX_SQLITE_LOG_ERRORS: usize = 1024;
const DEFAULT_UNSUPPORTED_CSS_TOP_N: usize = 20;

thread_local! {
    static SQLITE_CONNECTIONS: RefCell<HashMap<String, Connection>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone)]
struct UnsupportedCssConfig {
    logging_enabled: bool,
    sqlite_path: Option<String>,
    top_n: Option<usize>,
}

impl StyleResolver {
    /// Creates a new style resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of distinct `@media` prelude strings currently held
    /// in the parse cache.
    ///
    /// Primarily useful for testing and diagnostic purposes.
    #[cfg(test)]
    pub(crate) fn media_query_cache_len(&self) -> usize {
        self.media_query_cache.len()
    }

    /// Sets the root element's computed font-size in px.
    ///
    /// This value is used to resolve `rem` units. Defaults to 16px when not set.
    /// Calling this explicitly prevents the resolver from auto-deriving the root
    /// font size from the computed style of the root element.
    pub fn set_root_font_size(&mut self, px: f32) {
        self.root_font_size = px;
        self.root_font_size_explicit = true;
        self.cache.clear();
        self.pseudo_cache.clear();
    }

    /// Sets the viewport dimensions in px.
    ///
    /// These values are used to resolve `vw`, `vh`, `vmin`, and `vmax` units.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport_width = width;
        self.viewport_height = height;
        self.cache.clear();
        self.pseudo_cache.clear();
    }

    /// Sets whether the system is in dark mode.
    ///
    /// When `true`, `@media (prefers-color-scheme: dark)` queries match and
    /// `@media (prefers-color-scheme: light)` queries do not.  Defaults to
    /// `false` (light mode).  Clears the style cache so that subsequent calls
    /// to [`StyleResolver::computed_style`] reflect the new scheme.
    pub fn set_color_scheme_dark(&mut self, dark: bool) {
        self.color_scheme_dark = dark;
        self.cache.clear();
        self.pseudo_cache.clear();
    }

    /// Adds a stylesheet with its origin.
    pub fn add_stylesheet(&mut self, origin: Origin, stylesheet: Stylesheet) {
        // Extract @keyframes rules before storing the stylesheet.
        collect_keyframes(&stylesheet.rules, &mut self.keyframes);
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.cache.clear();
        self.pseudo_cache.clear();
    }

    /// Resolves computed style for `node`, using the cache when possible.
    pub fn computed_style(&mut self, node: &NodeHandle) -> ComputedStyle {
        let key = node.identity();
        if let Some(style) = self.cache.get(&key) {
            return style.clone();
        }

        let inherited = node
            .parent_node()
            .map(|parent| self.computed_style(&parent));
        let style = self.compute_style(node, inherited.as_ref());

        // Auto-update root_font_size from the root element's computed font-size so that
        // `rem` units in descendant elements resolve correctly even without an explicit
        // set_root_font_size() call. Skip if the caller already provided an explicit value.
        if !self.root_font_size_explicit {
            let is_root = node.node_type() == NodeType::Element
                && node
                    .tag_name()
                    .as_deref()
                    .map(|t| t.eq_ignore_ascii_case("html"))
                    .unwrap_or(false)
                && node
                    .parent_node()
                    .map(|p| p.node_type() == NodeType::Document)
                    .unwrap_or(false);
            if is_root
                && let Some(ComputedValue::Px(px)) = style.get("font-size") {
                    self.root_font_size = *px;
                }
        }

        self.cache.insert(key, style.clone());
        style
    }

    /// Resolves computed style for a pseudo-element attached to `node`.
    pub fn computed_pseudo_style(
        &mut self,
        node: &NodeHandle,
        pseudo: PseudoElement,
    ) -> Option<ComputedStyle> {
        let key = (node.identity(), pseudo);
        if let Some(style) = self.pseudo_cache.get(&key) {
            return Some(style.clone());
        }

        let parent_style = self.computed_style(node);
        let style = self.compute_style_with_pseudo(node, Some(&parent_style), Some(pseudo));
        if style.properties.is_empty() {
            return None;
        }

        self.pseudo_cache.insert(key, style.clone());
        Some(style)
    }

    fn compute_style(
        &mut self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
    ) -> ComputedStyle {
        self.compute_style_with_pseudo(node, parent_style, None)
    }

    fn compute_style_with_pseudo(
        &mut self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
        pseudo: Option<PseudoElement>,
    ) -> ComputedStyle {
        let mut candidates = Vec::new();
        let mut source_order = 0usize;
        let viewport_width = self.viewport_width;
        let viewport_height = self.viewport_height;
        let color_scheme_dark = self.color_scheme_dark;

        for input in &self.stylesheets {
            collect_rule_candidates(
                node,
                &input.stylesheet.rules,
                input.origin,
                pseudo,
                &mut source_order,
                &mut candidates,
                viewport_width,
                viewport_height,
                color_scheme_dark,
                &mut self.media_query_cache,
            );
        }

        if pseudo.is_none()
            && node.node_type() == NodeType::Element
            && let Some(inline_style) = node.get_attribute("style")
        {
            for declaration in super::parse_style_attribute(&inline_style) {
                candidates.push(Candidate {
                    name: canonical_property_name(&declaration.name).to_string(),
                    prefixed_alias: is_prefixed_property_alias(&declaration.name),
                    value: declaration.value,
                    important: declaration.important,
                    origin: Origin::Author,
                    inline: true,
                    specificity: Specificity {
                        ids: 0,
                        classes: 0,
                        elements: 0,
                    },
                    source_order,
                });
                source_order += 1;
            }
        }

        candidates.sort_by(|left, right| {
            cascade_rank(left)
                .cmp(&cascade_rank(right))
                .then(right.prefixed_alias.cmp(&left.prefixed_alias))
                .then(left.inline.cmp(&right.inline))
                .then(left.specificity.cmp(&right.specificity))
                .then(left.source_order.cmp(&right.source_order))
        });

        let mut properties: BTreeMap<String, ComputedValue> = BTreeMap::new();
        let mut custom_properties = inherited_custom_properties(parent_style);
        for candidate in &candidates {
            if candidate.name.starts_with("--") {
                custom_properties.insert(candidate.name.clone(), candidate.value.clone());
            }
        }

        // Effective root font-size for rem resolution: use the resolver's configured value,
        // falling back to the CSS default of 16px.
        let root_font_size = if self.root_font_size > 0.0 {
            self.root_font_size
        } else {
            16.0
        };

        let mut important_properties = HashSet::new();

        // Process font-size first so that em units in other properties
        // resolve against the element's own computed font-size.
        if let Some(fs_candidate) = candidates.iter().rfind(|c| c.name == "font-size")
            && let Some(resolved_value) =
                resolve_value_with_custom_properties(&fs_candidate.value, &custom_properties)
            {
                let parent_fs = parent_style
                    .and_then(|ps| ps.get("font-size"))
                    .and_then(|v| match v {
                        ComputedValue::Px(px) => Some(*px),
                        _ => None,
                    })
                    .unwrap_or(16.0);
                let ctx = ResolutionContext {
                    parent_font_size: parent_fs,
                    root_font_size,
                    viewport_width: self.viewport_width,
                    viewport_height: self.viewport_height,
                };
                let computed = compute_value(&resolved_value, "font-size", ctx);
                // Resolve font-size keywords "smaller" / "larger" relative to parent.
                let resolved = match &computed {
                    ComputedValue::Keyword(kw) if kw.eq_ignore_ascii_case("smaller") => {
                        ComputedValue::Px(parent_fs * 0.833)
                    }
                    ComputedValue::Keyword(kw) if kw.eq_ignore_ascii_case("larger") => {
                        ComputedValue::Px(parent_fs * 1.2)
                    }
                    other => other.clone(),
                };
                properties.insert("font-size".to_string(), resolved);
                if fs_candidate.important {
                    important_properties.insert("font-size".to_string());
                }
            }

        // For the root element, update root_font_size from its computed font-size
        // so that rem-based properties on the root itself resolve correctly.
        let mut root_font_size = root_font_size;
        if !self.root_font_size_explicit {
            let is_root = node
                .tag_name()
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("html"));
            if is_root
                && let Some(ComputedValue::Px(px)) = properties.get("font-size") {
                    root_font_size = *px;
                }
        }

        for candidate in candidates {
            if candidate.name == "font-size" {
                continue; // already processed above
            }
            log_unsupported_css_if_enabled(&candidate.name, &candidate.value);
            let Some(resolved_value) =
                resolve_value_with_custom_properties(&candidate.value, &custom_properties)
            else {
                continue;
            };
            // Per-property value validation runs before the value enters the
            // cascade so an invalid declaration is dropped entirely (CSS error
            // handling), never overriding an earlier valid declaration.
            match validate_declaration(&candidate.name, &resolved_value) {
                DeclarationValidation::Valid(computed) => {
                    properties.insert(candidate.name.to_ascii_lowercase(), computed);
                    if candidate.important {
                        important_properties.insert(candidate.name.to_ascii_lowercase());
                    }
                    continue;
                }
                DeclarationValidation::Invalid => continue,
                DeclarationValidation::Unvalidated => {}
            }
            let font_size = inherited_font_size(parent_style, &properties);
            let ctx = ResolutionContext {
                parent_font_size: font_size,
                root_font_size,
                viewport_width: self.viewport_width,
                viewport_height: self.viewport_height,
            };
            if candidate.name == "gap" || candidate.name == "grid-gap" {
                if let Some((row_gap, column_gap)) =
                    compute_gap_shorthand(&resolved_value, ctx)
                {
                    insert_computed_property(&mut properties, "row-gap", row_gap);
                    insert_computed_property(&mut properties, "column-gap", column_gap);
                    if candidate.important {
                        important_properties.insert("row-gap".to_string());
                        important_properties.insert("column-gap".to_string());
                    }
                }
                continue;
            }
            if candidate.name == "grid-row-gap" || candidate.name == "grid-column-gap" {
                let target = if candidate.name == "grid-row-gap" { "row-gap" } else { "column-gap" };
                let computed = compute_value(&resolved_value, target, ctx);
                insert_computed_property(&mut properties, target, computed);
                if candidate.important {
                    important_properties.insert(target.to_string());
                }
                continue;
            }
            let computed = compute_value(&resolved_value, &candidate.name, ctx);
            insert_computed_property(&mut properties, &candidate.name, computed);
            if candidate.important {
                important_properties.insert(candidate.name.to_ascii_lowercase());
            }
        }

        apply_ua_defaults(node, &mut properties, pseudo, parent_style);
        apply_presentational_hints(node, &mut properties, pseudo);
        resolve_current_color_on_color_property(&mut properties, parent_style);
        resolve_explicit_inherit(&mut properties, parent_style);
        apply_inheritance(&mut properties, parent_style);
        apply_initial_values(&mut properties);
        zero_border_width_for_none_style(&mut properties);
        self.apply_animation_snapshot(&mut properties, parent_style, &important_properties);

        ComputedStyle { properties }
    }

    /// Applies a deterministic animation snapshot. Completed forwards/both
    /// animations keep their final state; running infinite animations are
    /// sampled at a fixed post-load instant so screenshots remain stable.
    fn apply_animation_snapshot(
        &self,
        properties: &mut BTreeMap<String, ComputedValue>,
        _parent_style: Option<&ComputedStyle>,
        important_properties: &HashSet<String>,
    ) {
        let anim_name = match properties.get("animation-name") {
            Some(ComputedValue::Keyword(name)) => name.clone(),
            _ => return,
        };
        if anim_name.eq_ignore_ascii_case("none") || anim_name.is_empty() {
            return;
        }
        let Some(steps) = self.keyframes.get(&anim_name) else { return; };

        let fill_mode = match properties.get("animation-fill-mode") {
            Some(ComputedValue::Keyword(value)) => value.to_ascii_lowercase(),
            _ => "none".to_string(),
        };
        let infinite = matches!(
            properties.get("animation-iteration-count"),
            Some(ComputedValue::Keyword(value)) if value.eq_ignore_ascii_case("infinite")
        );
        let declarations = if fill_mode == "forwards" || fill_mode == "both" {
            steps.last().map(|step| &step.declarations)
        } else if infinite {
            let duration = animation_seconds(properties.get("animation-duration")).unwrap_or(0.0);
            let delay = animation_seconds(properties.get("animation-delay")).unwrap_or(0.0);
            if duration <= 0.0 || STATIC_ANIMATION_TIME_SECONDS < delay {
                None
            } else {
                let progress =
                    ((STATIC_ANIMATION_TIME_SECONDS - delay) / duration).rem_euclid(1.0);
                steps
                    .iter()
                    .rev()
                    .find(|step| step.offset <= progress)
                    .map(|step| &step.declarations)
            }
        } else {
            None
        };
        let Some(declarations) = declarations else { return; };

        let element_font_size = properties
            .get("font-size")
            .and_then(|value| match value {
                ComputedValue::Px(px) => Some(*px),
                _ => None,
            })
            .unwrap_or(16.0);
        let ctx = ResolutionContext {
            parent_font_size: element_font_size,
            root_font_size: self.root_font_size,
            viewport_width: self.viewport_width,
            viewport_height: self.viewport_height,
        };
        let custom_properties: BTreeMap<String, Value> = properties
            .iter()
            .filter(|(name, _)| name.starts_with("--"))
            .map(|(name, value)| (name.clone(), computed_value_to_value(value)))
            .collect();

        let standard_properties: HashSet<&str> = declarations
            .iter()
            .filter(|declaration| !is_prefixed_property_alias(&declaration.name))
            .map(|declaration| canonical_property_name(&declaration.name))
            .collect();
        for declaration in declarations {
            let property_name = canonical_property_name(&declaration.name);
            if is_prefixed_property_alias(&declaration.name)
                && standard_properties.contains(property_name)
            {
                continue;
            }
            if important_properties.contains(property_name) {
                continue;
            }
            let resolved =
                resolve_value_with_custom_properties(&declaration.value, &custom_properties)
                    .unwrap_or_else(|| declaration.value.clone());
            let computed = compute_value(&resolved, property_name, ctx);
            insert_computed_property(properties, property_name, computed);
        }
    }
}

fn animation_seconds(value: Option<&ComputedValue>) -> Option<f32> {
    match value {
        Some(ComputedValue::Number(value)) => Some(*value),
        _ => None,
    }
}

fn compute_gap_shorthand(
    value: &Value,
    ctx: ResolutionContext,
) -> Option<(ComputedValue, ComputedValue)> {
    match value {
        Value::List(values) => match values.as_slice() {
            [single] => {
                let computed = compute_value(single, "row-gap", ctx);
                if should_skip_computed_property("row-gap", &computed) {
                    None
                } else {
                    Some((computed.clone(), computed))
                }
            }
            [row, column] => {
                let row_gap = compute_value(row, "row-gap", ctx);
                let column_gap = compute_value(column, "column-gap", ctx);
                if should_skip_computed_property("row-gap", &row_gap)
                    || should_skip_computed_property("column-gap", &column_gap)
                {
                    None
                } else {
                    Some((row_gap, column_gap))
                }
            }
            _ => None,
        },
        _ => {
            let computed = compute_value(value, "row-gap", ctx);
            if should_skip_computed_property("row-gap", &computed) {
                None
            } else {
                Some((computed.clone(), computed))
            }
        }
    }
}

fn insert_computed_property(
    properties: &mut BTreeMap<String, ComputedValue>,
    name: &str,
    computed: ComputedValue,
) {
    if should_skip_computed_property(name, &computed) {
        return;
    }
    properties.insert(name.to_string(), computed);
}

fn should_skip_computed_property(name: &str, computed: &ComputedValue) -> bool {
    // CSS 2.1: non-zero unitless numbers are invalid for length properties;
    // skip them so they don't override valid length values in the cascade.
    if matches!(computed, ComputedValue::Number(n) if *n != 0.0) && is_length_property(name) {
        return true;
    }

    // Enumerated properties: a keyword outside the property's valid set is an
    // invalid declaration and must be discarded by the cascade so it cannot
    // override an earlier valid declaration of the same property. Acid3 test 0
    // relies on `white-space: pre-wrap; white-space: x-bogus;` keeping the
    // `pre-wrap` value (the invalid `x-bogus` declaration is dropped).
    if let ComputedValue::Keyword(keyword) = computed
        && let Some(valid) = enumerated_keyword_set(name) {
            let lower = keyword.to_ascii_lowercase();
            // CSS-wide keywords are resolved in a later pass; never drop them.
            // `revert-layer` (CSS Cascade 5) is a CSS-wide keyword too and must
            // not be discarded by the enumerated-value validation.
            let is_css_wide = matches!(
                lower.as_str(),
                "inherit" | "initial" | "unset" | "revert" | "revert-layer"
            );
            if !is_css_wide && !valid.iter().any(|candidate| *candidate == lower) {
                return true;
            }
        }

    false
}

/// Returns the set of valid keyword values for an enumerated CSS property, or
/// `None` for properties that are not validated here.
///
/// Only properties whose invalid values Omoikane must actively discard during
/// the cascade are listed. Keeping the set small avoids accidentally dropping a
/// valid value that a property accepts but that is not enumerated here.
fn enumerated_keyword_set(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "white-space" => Some(&["normal", "pre", "nowrap", "pre-wrap", "pre-line", "break-spaces"]),
        _ => None,
    }
}

/// Outcome of validating a single declaration's value against a property's
/// grammar. Properties without a dedicated grammar report [`Self::Unvalidated`]
/// and fall through to the generic compute path unchanged, so introducing a new
/// validated property cannot alter the handling of existing ones.
enum DeclarationValidation {
    /// The declaration is valid; use this normalized computed value.
    Valid(ComputedValue),
    /// The declaration is invalid and must be dropped by the cascade (CSS error
    /// handling: an invalid declaration is ignored, so it neither applies nor
    /// blocks an earlier/later valid declaration of the same property).
    Invalid,
    /// The property has no dedicated grammar validation here.
    Unvalidated,
}

/// Validates a resolved declaration value against the property's grammar.
///
/// This is the single extension point for per-property value validation. Only
/// `cursor` is validated today; other properties fall through unchanged. To add
/// a property, match its name here and return [`DeclarationValidation::Valid`] /
/// [`DeclarationValidation::Invalid`].
fn validate_declaration(name: &str, value: &Value) -> DeclarationValidation {
    if name.eq_ignore_ascii_case("cursor") {
        return match compute_cursor_value(value) {
            Some(computed) => DeclarationValidation::Valid(computed),
            None => DeclarationValidation::Invalid,
        };
    }
    DeclarationValidation::Unvalidated
}

/// A CSS-wide keyword (CSS Cascade). These are valid for every property and are
/// resolved (or left as-is) by later passes, so property grammars must never
/// reject them.
fn is_css_wide_keyword(lowercased: &str) -> bool {
    matches!(
        lowercased,
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    )
}

/// Valid `cursor` keyword set: the CSS 2.1 values plus the CSS UI Level 3 / 4
/// additions. Includes every keyword exercised by Acid3 test 47 and the common
/// CSS3 extras (`grab`/`grabbing`, `zoom-in`/`zoom-out`).
fn is_valid_cursor_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "auto"
            | "default"
            | "none"
            | "context-menu"
            | "help"
            | "pointer"
            | "progress"
            | "wait"
            | "cell"
            | "crosshair"
            | "text"
            | "vertical-text"
            | "alias"
            | "copy"
            | "move"
            | "no-drop"
            | "not-allowed"
            | "grab"
            | "grabbing"
            | "e-resize"
            | "n-resize"
            | "ne-resize"
            | "nw-resize"
            | "s-resize"
            | "se-resize"
            | "sw-resize"
            | "w-resize"
            | "ew-resize"
            | "ns-resize"
            | "nesw-resize"
            | "nwse-resize"
            | "col-resize"
            | "row-resize"
            | "all-scroll"
            | "zoom-in"
            | "zoom-out"
    )
}

/// Validates a `cursor` declaration value and returns its normalized computed
/// value, or `None` if the value is invalid.
///
/// Per the CSS UI `cursor` grammar `[ <url> [ <x> <y> ]? , ]* <keyword>`, the
/// value is a comma-separated list of `url()` groups followed by a mandatory
/// trailing keyword. Each group is a `url()` reference optionally followed by a
/// hotspot coordinate **pair** `<x> <y>` (never a lone coordinate). Coordinates
/// may only appear directly after a `url()`. Any grammar violation — a
/// coordinate with no preceding `url()`, an odd number of coordinates, or an
/// unexpected token — makes the whole declaration invalid (returns `None`, so
/// the cascade falls back to the initial value `auto`). The trailing keyword is
/// validated against the supported set and normalized to lowercase; CSS-wide
/// keywords pass through for resolution by later cascade passes.
///
/// The CSS parser discards commas from the token stream, so the group structure
/// is reconstructed positionally: each `url()` starts a new group and consumes
/// the run of coordinate tokens that immediately follows it. Serialization
/// re-inserts the group-separating commas (`url(a), url(b) 1 2, pointer`).
fn compute_cursor_value(value: &Value) -> Option<ComputedValue> {
    match value {
        Value::Keyword(keyword) => {
            let lower = keyword.to_ascii_lowercase();
            if is_css_wide_keyword(&lower) || is_valid_cursor_keyword(&lower) {
                Some(ComputedValue::Keyword(lower))
            } else {
                None
            }
        }
        Value::List(items) => {
            // The final component must be a valid, non-CSS-wide cursor keyword.
            let (last, leading) = items.split_last()?;
            let Value::Keyword(keyword) = last else {
                return None;
            };
            let lower = keyword.to_ascii_lowercase();
            if !is_valid_cursor_keyword(&lower) {
                return None;
            }
            if leading.is_empty() {
                return Some(ComputedValue::Keyword(lower));
            }
            // Parse the leading `url()` groups positionally. Each group must
            // start with a `url()` reference (the parser renders these as
            // `Keyword("url(...)")` or, defensively, a `url` function) and may
            // be followed by exactly zero or two coordinate tokens.
            let mut groups: Vec<String> = Vec::new();
            let mut index = 0;
            while index < leading.len() {
                let is_url = match &leading[index] {
                    Value::Keyword(k) => k.to_ascii_lowercase().starts_with("url("),
                    Value::Function { name, .. } => name.eq_ignore_ascii_case("url"),
                    _ => false,
                };
                if !is_url {
                    // A coordinate (or anything else) with no preceding `url()`.
                    return None;
                }
                let url = render_value(&leading[index]);
                index += 1;

                // Consume the coordinate run that follows this `url()`.
                let mut coords: Vec<String> = Vec::new();
                while index < leading.len() {
                    match &leading[index] {
                        Value::Number(_) | Value::Length(_, _) => {
                            coords.push(render_value(&leading[index]));
                            index += 1;
                        }
                        _ => break,
                    }
                }
                // Coordinates are only valid as an `<x> <y>` pair.
                if coords.len() != 2 && !coords.is_empty() {
                    return None;
                }

                if coords.is_empty() {
                    groups.push(url);
                } else {
                    groups.push(format!("{url} {}", coords.join(" ")));
                }
            }

            let prefix = groups.join(", ");
            Some(ComputedValue::Keyword(format!("{prefix}, {lower}")))
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    name: String,
    prefixed_alias: bool,
    value: Value,
    important: bool,
    origin: Origin,
    inline: bool,
    specificity: Specificity,
    source_order: usize,
}

fn collect_rule_candidates(
    node: &NodeHandle,
    rules: &[Rule],
    origin: Origin,
    pseudo: Option<PseudoElement>,
    source_order: &mut usize,
    out: &mut Vec<Candidate>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_cache: &mut HashMap<String, Vec<MediaQuery>>,
) {
    if node.node_type() != NodeType::Element {
        return;
    }

    for rule in rules {
        match rule {
            Rule::Style(style_rule) => {
                let matching_specificity = style_rule
                    .selectors
                    .iter()
                    .filter(|selector| matches_selector_with_pseudo(node, selector, pseudo))
                    .map(specificity)
                    .max();

                if let Some(specificity) = matching_specificity {
                    for declaration in &style_rule.declarations {
                        out.push(Candidate {
                            name: canonical_property_name(&declaration.name).to_string(),
                            prefixed_alias: is_prefixed_property_alias(&declaration.name),
                            value: declaration.value.clone(),
                            important: declaration.important,
                            origin,
                            inline: false,
                            specificity,
                            source_order: *source_order,
                        });
                        *source_order += 1;
                    }
                } else {
                    *source_order += style_rule.declarations.len();
                }
            }
            Rule::At(at_rule) => {
                if let Some(block) = &at_rule.block {
                    // Evaluate @media queries before descending into the block.
                    let should_apply = if at_rule.name == "media" {
                        media_query_matches(
                            &at_rule.prelude,
                            viewport_width,
                            viewport_height,
                            color_scheme_dark,
                            media_cache,
                        )
                    } else if at_rule.name.eq_ignore_ascii_case("keyframes")
                        || at_rule.name.eq_ignore_ascii_case("-webkit-keyframes")
                    {
                        // @keyframes rules are handled separately; skip them in cascade.
                        false
                    } else {
                        // Other at-rules (e.g. @supports) are passed through.
                        true
                    };
                    if should_apply {
                        collect_rule_candidates(
                            node,
                            block,
                            origin,
                            pseudo,
                            source_order,
                            out,
                            viewport_width,
                            viewport_height,
                            color_scheme_dark,
                            media_cache,
                        );
                    } else {
                        // Count the rules inside for correct source_order numbering.
                        *source_order += count_declarations(block);
                    }
                } else {
                    *source_order += at_rule.declarations.len();
                }
            }
            // @font-face rules are handled by the font loading layer, not style resolution.
            Rule::FontFace(_) => {}
        }
    }
}

/// Returns `true` when at least one query in a comma-separated media query list
/// matches the given viewport.  Falls back to `false` when the list cannot be
/// parsed (conservative, forward-compatible behaviour).
///
/// The `cache` parameter is a mutable reference to a parse-result cache keyed
/// by the normalized (trimmed) prelude string.  On a cache miss the prelude is
/// parsed and the result is stored so that subsequent calls with the same string
/// skip parsing.
fn media_query_matches(
    prelude: &str,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    cache: &mut HashMap<String, Vec<MediaQuery>>,
) -> bool {
    let prelude = prelude.trim();
    if prelude.is_empty() {
        return true;
    }
    let queries = cache
        .entry(prelude.to_owned())
        .or_insert_with(|| parse_media_query_list(prelude).unwrap_or_default());
    queries
        .iter()
        .any(|q| evaluate_media_query(q, viewport_width, viewport_height, color_scheme_dark))
}

/// Counts the total number of declarations inside a rule list (used for
/// source_order bookkeeping when a block is skipped due to a non-matching
/// media query).
fn count_declarations(rules: &[Rule]) -> usize {
    rules.iter().map(|r| match r {
        Rule::Style(s) => s.declarations.len(),
        Rule::At(a) => {
            a.declarations.len()
                + a.block.as_deref().map(count_declarations).unwrap_or(0)
        }
        Rule::FontFace(_) => 0,
    }).sum()
}

fn is_length_property(name: &str) -> bool {
    matches!(
        name,
        "width"
            | "height"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "margin-top"
            | "margin-inline-start"
            | "margin-inline-end"
            | "margin-block-start"
            | "margin-block-end"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding-top"
            | "padding-inline-start"
            | "padding-inline-end"
            | "padding-block-start"
            | "padding-block-end"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "border-spacing"
            | "flex-basis"
            | "outline-width"
            | "outline-offset"
    )
}

/// Extracts all `@keyframes` steps from a stylesheet.
fn collect_keyframes(rules: &[Rule], keyframes: &mut HashMap<String, Vec<KeyframeStep>>) {
    for rule in rules {
        match rule {
            Rule::At(at_rule)
                if at_rule.name.eq_ignore_ascii_case("keyframes")
                    || at_rule.name.eq_ignore_ascii_case("-webkit-keyframes") =>
            {
                let animation_name = at_rule.prelude.trim().to_string();
                if animation_name.is_empty() {
                    continue;
                }
                let raw_block = at_rule
                    .declarations
                    .iter()
                    .find(|declaration| declaration.name == "__keyframes_block")
                    .and_then(|declaration| match &declaration.value {
                        Value::Keyword(text) => Some(text.clone()),
                        _ => None,
                    });
                if let Some(block_text) = raw_block {
                    let steps = parse_keyframe_steps(&block_text);
                    if !steps.is_empty() {
                        keyframes.insert(animation_name, steps);
                    }
                }
            }
            Rule::At(at_rule) if at_rule.block.is_some() => {
                collect_keyframes(at_rule.block.as_ref().unwrap(), keyframes);
            }
            _ => {}
        }
    }
}

fn parse_keyframe_steps(block_text: &str) -> Vec<KeyframeStep> {
    let mut steps = Vec::new();
    let mut position = 0;
    let chars: Vec<char> = block_text.chars().collect();

    while position < chars.len() {
        while position < chars.len() && chars[position].is_ascii_whitespace() {
            position += 1;
        }
        if position >= chars.len() {
            break;
        }
        let selector_start = position;
        while position < chars.len() && chars[position] != '{' {
            position += 1;
        }
        if position >= chars.len() {
            break;
        }
        let selector: String = chars[selector_start..position].iter().collect();
        position += 1;

        let declaration_start = position;
        let mut depth = 1;
        while position < chars.len() && depth > 0 {
            match chars[position] {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                position += 1;
            }
        }
        let declaration_text: String = chars[declaration_start..position].iter().collect();
        if position < chars.len() {
            position += 1;
        }

        let offsets: Vec<f32> = selector
            .split(',')
            .filter_map(|part| match part.trim().to_ascii_lowercase().as_str() {
                "from" => Some(0.0),
                "to" => Some(1.0),
                percentage => percentage
                    .strip_suffix('%')
                    .and_then(|number| number.trim().parse::<f32>().ok())
                    .map(|number| (number / 100.0).clamp(0.0, 1.0)),
            })
            .collect();
        if offsets.is_empty() {
            continue;
        }

        let fake_rule = format!("x {{ {declaration_text} }}");
        let Ok(stylesheet) = super::parse_stylesheet(&fake_rule) else { continue; };
        let Some(declarations) = stylesheet.rules.iter().find_map(|rule| match rule {
            Rule::Style(style_rule) => Some(style_rule.declarations.clone()),
            _ => None,
        }) else {
            continue;
        };
        for offset in offsets {
            steps.push(KeyframeStep {
                offset,
                declarations: declarations.clone(),
            });
        }
    }

    steps.sort_by(|left, right| left.offset.total_cmp(&right.offset));
    steps
}

fn cascade_rank(candidate: &Candidate) -> (u8, u8) {
    let importance = if candidate.important { 1 } else { 0 };
    let origin = match (candidate.important, candidate.origin) {
        (true, Origin::UserAgent) => 5,
        (true, Origin::User) => 4,
        (true, Origin::Author) => 3,
        (false, Origin::Author) => 2,
        (false, Origin::User) => 1,
        (false, Origin::UserAgent) => 0,
    };
    (importance, origin)
}

fn log_unsupported_css_if_enabled(property: &str, value: &Value) {
    if should_ignore_unsupported_css_logging(property) || is_supported_property(property) {
        return;
    }

    let config = unsupported_css_config();
    if !config.logging_enabled && config.sqlite_path.is_none() {
        return;
    }

    let rendered_value = sanitize_unsupported_css_log_value(&render_value(value));
    if let Some(path) = config.sqlite_path.as_deref() {
        persist_unsupported_css_to_sqlite(path, property, &rendered_value);
        if let Some(top_n) = config.top_n {
            emit_unsupported_css_top_n_summary_if_updated(path, top_n);
        }
    }

    if config.logging_enabled {
        let key = unsupported_css_dedup_key(property, &rendered_value);
        let logged = UNSUPPORTED_CSS_LOGGED.get_or_init(|| Mutex::new(HashSet::new()));
        let mut logged = logged.lock().expect("unsupported css log lock poisoned");
        if logged.len() >= MAX_UNSUPPORTED_LOG_KEYS {
            logged.clear();
        }
        if logged.insert(key) {
            let value = truncate_log_value(&rendered_value, MAX_UNSUPPORTED_LOG_VALUE_LEN);
            eprintln!("[omoikane][unsupported-css] {property}={value}");
        }
    }
}

fn unsupported_css_config() -> &'static UnsupportedCssConfig {
    UNSUPPORTED_CSS_CONFIG.get_or_init(|| UnsupportedCssConfig {
        logging_enabled: env_flag_true("OMOIKANE_LOG_UNSUPPORTED_CSS"),
        sqlite_path: std::env::var("OMOIKANE_UNSUPPORTED_CSS_SQLITE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        top_n: std::env::var("OMOIKANE_UNSUPPORTED_CSS_TOP_N")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
            .or_else(|| {
                if env_flag_true("OMOIKANE_LOG_UNSUPPORTED_CSS_TOP_N") {
                    Some(DEFAULT_UNSUPPORTED_CSS_TOP_N)
                } else {
                    None
                }
            }),
    })
}

fn env_flag_true(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn ensure_unsupported_css_sqlite_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS unsupported_css_log (
            property TEXT NOT NULL,
            value TEXT NOT NULL,
            first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            occurrences INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (property, value)
        );
        CREATE INDEX IF NOT EXISTS idx_unsupported_css_log_occurrences
        ON unsupported_css_log (occurrences DESC);",
    )?;
    Ok(())
}

fn persist_unsupported_css_to_sqlite(path: &str, property: &str, value: &str) {
    let result: Result<(), rusqlite::Error> = SQLITE_CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        if !connections.contains_key(path) {
            let mut conn = Connection::open(path)?;
            configure_sqlite_connection(&mut conn)?;
            ensure_unsupported_css_sqlite_schema(&conn)?;
            connections.insert(path.to_string(), conn);
        }

        let conn = connections
            .get_mut(path)
            .expect("sqlite connection must exist after initialization");
        conn.execute(
            "INSERT INTO unsupported_css_log (property, value, occurrences)
             VALUES (?1, ?2, 1)
             ON CONFLICT(property, value) DO UPDATE SET
               occurrences = unsupported_css_log.occurrences + 1,
               last_seen_at = CURRENT_TIMESTAMP",
            params![property, value],
        )?;
        Ok(())
    });

    if let Err(error) = result {
        log_sqlite_error(&error);
    }
}

fn emit_unsupported_css_top_n_summary_if_updated(path: &str, top_n: usize) {
    let rows = SQLITE_CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        let Some(conn) = connections.get_mut(path) else {
            return Ok(Vec::new());
        };
        query_unsupported_css_top_n(conn, top_n)
    });
    let Ok(rows) = rows else {
        if let Err(error) = rows {
            log_sqlite_error(&error);
        }
        return;
    };
    if rows.is_empty() {
        return;
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    top_n.hash(&mut hasher);
    for (property, value, occurrences) in &rows {
        property.hash(&mut hasher);
        value.hash(&mut hasher);
        occurrences.hash(&mut hasher);
    }
    let digest = hasher.finish();
    let key = format!("{path}#{top_n}");
    let map = UNSUPPORTED_CSS_TOP_N_LAST_DIGEST.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map
        .lock()
        .expect("unsupported css top-n digest lock poisoned");
    if map.get(&key).copied() == Some(digest) {
        return;
    }
    map.insert(key, digest);

    eprintln!("[omoikane][unsupported-css][top-n] top {top_n} candidates (site/url anonymized)");
    for (index, (property, value, occurrences)) in rows.iter().enumerate() {
        let value = truncate_log_value(value, MAX_UNSUPPORTED_LOG_VALUE_LEN);
        eprintln!(
            "[omoikane][unsupported-css][top-n] {}. {}={} (count={})",
            index + 1,
            property,
            value,
            occurrences
        );
    }
}

fn query_unsupported_css_top_n(
    conn: &Connection,
    top_n: usize,
) -> Result<Vec<(String, String, i64)>, rusqlite::Error> {
    let limit = i64::try_from(top_n).unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(
        "SELECT property, value, occurrences
         FROM unsupported_css_log
         ORDER BY occurrences DESC, property ASC, value ASC
         LIMIT ?1",
    )?;
    stmt.query_map(params![limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?
    .collect::<Result<Vec<_>, _>>()
}

fn configure_sqlite_connection(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    conn.busy_timeout(Duration::from_millis(5000))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;",
    )?;
    Ok(())
}

#[cfg(test)]
fn close_sqlite_connection_for_path(path: &str) {
    SQLITE_CONNECTIONS.with(|connections| {
        connections.borrow_mut().remove(path);
    });
}

fn log_sqlite_error(error: &rusqlite::Error) {
    let error_key = format!("{error}");
    let errors = SQLITE_LOG_ERRORS.get_or_init(|| Mutex::new(HashSet::new()));
    let mut errors = errors.lock().expect("sqlite css log error lock poisoned");
    if errors.contains(&error_key) {
        return;
    }
    if errors.len() >= MAX_SQLITE_LOG_ERRORS {
        return;
    }
    errors.insert(error_key.clone());
    eprintln!("[omoikane][unsupported-css][sqlite-error] {error_key}");
}

fn should_ignore_unsupported_css_logging(property: &str) -> bool {
    property.starts_with("--")
}

fn unsupported_css_dedup_key(property: &str, value: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{property}#{}#{}", value.len(), hasher.finish())
}

fn sanitize_unsupported_css_log_value(value: &str) -> String {
    const URL_PREFIXES: [&str; 6] = ["http://", "https://", "ws://", "wss://", "ftp://", "data:"];
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0usize;

    while cursor < value.len() {
        let tail = &value[cursor..];
        let mut matched_prefix = false;
        for prefix in URL_PREFIXES {
            if tail.len() >= prefix.len() && tail[..prefix.len()].eq_ignore_ascii_case(prefix) {
                matched_prefix = true;
                out.push_str("[redacted-url]");
                let mut consumed = 0usize;
                for (offset, ch) in tail.char_indices() {
                    if offset > 0 && is_url_terminator(ch) {
                        break;
                    }
                    consumed = offset + ch.len_utf8();
                }
                cursor += consumed.max(prefix.len());
                break;
            }
        }
        if matched_prefix {
            continue;
        }

        let ch = tail
            .chars()
            .next()
            .expect("tail must have at least one char");
        out.push(ch);
        cursor += ch.len_utf8();
    }

    out
}

fn is_url_terminator(ch: char) -> bool {
    ch.is_ascii_whitespace() || matches!(ch, '"' | '\'' | ')' | '(' | '<' | '>')
}

fn truncate_log_value(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }
    let mut out = value.chars().take(max_len).collect::<String>();
    out.push_str("...");
    out
}

fn is_supported_property(name: &str) -> bool {
    matches!(
        name,
        "align-items"
            | "align-content"
            | "align-self"
            | "animation"
            | "animation-delay"
            | "animation-direction"
            | "animation-duration"
            | "animation-fill-mode"
            | "animation-iteration-count"
            | "animation-name"
            | "animation-play-state"
            | "animation-timing-function"
            | "background-attachment"
            | "background-color"
            | "background-image"
            | "background-position-x"
            | "background-position-y"
            | "background-repeat"
            | "background-size"
            | "border-bottom-color"
            | "border-bottom-style"
            | "border-bottom-width"
            | "border-bottom-left-radius"
            | "border-bottom-right-radius"
            | "border-top-left-radius"
            | "border-top-right-radius"
            | "border-collapse"
            | "border-left-color"
            | "border-left-style"
            | "border-left-width"
            | "border-right-color"
            | "border-right-style"
            | "border-right-width"
            | "border-spacing"
            | "border-style"
            | "border-top-color"
            | "border-top-style"
            | "border-top-width"
            | "bottom"
            | "box-sizing"
            | "clear"
            | "clip-path"
            | "-webkit-clip-path"
            | "color"
            | "content"
            | "cursor"
            | "display"
            | "flex-basis"
            | "flex-direction"
            | "flex-grow"
            | "flex-shrink"
            | "flex-wrap"
            | "float"
            | "font-family"
            | "font-size"
            | "font-style"
            | "font-weight"
            | "gap"
            | "grid-gap"
            | "grid-row-gap"
            | "grid-column-gap"
            | "grid-template-columns"
            | "grid-template-rows"
            | "grid-template-areas"
            | "grid-template"
            | "grid-area"
            | "grid-column"
            | "grid-column-start"
            | "grid-column-end"
            | "grid-row"
            | "grid-row-start"
            | "grid-row-end"
            | "height"
            | "justify-content"
            | "justify-items"
            | "justify-self"
            | "place-content"
            | "place-items"
            | "place-self"
            | "left"
            | "line-height"
            | "margin-bottom"
            | "margin-left"
            | "margin-right"
            | "margin-top"
            | "margin-inline-start"
            | "margin-inline-end"
            | "margin-block-start"
            | "margin-block-end"
            | "max-height"
            | "max-width"
            | "min-height"
            | "min-width"
            | "column-gap"
            | "outline-color"
            | "outline-offset"
            | "outline-style"
            | "outline-width"
            | "overflow"
            | "overflow-x"
            | "overflow-y"
            | "padding-bottom"
            | "padding-left"
            | "padding-right"
            | "padding-top"
            | "padding-inline-start"
            | "padding-inline-end"
            | "padding-block-start"
            | "padding-block-end"
            | "position"
            | "right"
            | "row-gap"
            | "transform"
            | "transform-origin"
            | "text-align"
            | "text-decoration-line"
            | "text-decoration-color"
            | "text-decoration-style"
            | "text-transform"
            | "letter-spacing"
            | "word-spacing"
            | "top"
            | "vertical-align"
            | "visibility"
            | "white-space"
            | "width"
            | "word-break"
            | "overflow-wrap"
            | "word-wrap"
            | "z-index"
            | "box-shadow"
            | "opacity"
            | "list-style-type"
            | "list-style-position"
            | "list-style-image"
            | "mask"
            | "mask-image"
            | "mask-position"
            | "mask-position-x"
            | "mask-position-y"
            | "mask-repeat"
            | "mask-size"
            | "-webkit-mask"
            | "-webkit-mask-image"
            | "-webkit-mask-position"
            | "-webkit-mask-position-x"
            | "-webkit-mask-position-y"
            | "-webkit-mask-repeat"
            | "-webkit-mask-size"
    )
}

fn resolve_time_seconds(value: &Value) -> Option<f32> {
    match value {
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("s") => Some(*number),
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("ms") => Some(*number / 1000.0),
        Value::Number(number) if *number == 0.0 => Some(0.0),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            resolve_time_calc(arguments.first()?)
        }
        Value::List(_) => resolve_time_calc(value),
        _ => None,
    }
}

fn resolve_time_calc(value: &Value) -> Option<f32> {
    let Value::List(values) = value else {
        return resolve_time_seconds(value);
    };
    let mut total = 0.0;
    let mut sign = 1.0;
    let mut expects_value = true;
    for value in values {
        match value {
            Value::Keyword(operator) if operator == "+" || operator == "-" => {
                if expects_value {
                    return None;
                }
                sign = if operator == "-" { -1.0 } else { 1.0 };
                expects_value = true;
            }
            value if expects_value => {
                total += sign * resolve_time_seconds(value)?;
                expects_value = false;
            }
            _ => return None,
        }
    }
    (!expects_value).then_some(total)
}

fn compute_value(value: &Value, property_name: &str, ctx: ResolutionContext) -> ComputedValue {
    if property_name.eq_ignore_ascii_case("animation-duration")
        || property_name.eq_ignore_ascii_case("animation-delay")
    {
        return resolve_time_seconds(value)
            .map(ComputedValue::Number)
            .unwrap_or_else(|| ComputedValue::Keyword(render_value(value)));
    }
    if property_name.eq_ignore_ascii_case("clip-path") {
        return ComputedValue::Keyword(render_clip_path_value(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("grid-template-areas") {
        return ComputedValue::Keyword(render_grid_template_areas(value));
    }
    if property_name.eq_ignore_ascii_case("grid-template-columns")
        || property_name.eq_ignore_ascii_case("grid-template-rows")
    {
        return ComputedValue::Keyword(render_grid_track_value(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("grid-column-start")
        || property_name.eq_ignore_ascii_case("grid-column-end")
        || property_name.eq_ignore_ascii_case("grid-row-start")
        || property_name.eq_ignore_ascii_case("grid-row-end")
    {
        return ComputedValue::Keyword(render_value(value));
    }
    match value {
        Value::Keyword(keyword) => {
            // CSS-wide keywords must remain as Keyword for inherit/initial resolution.
            let lower = keyword.to_ascii_lowercase();
            if matches!(lower.as_str(), "inherit" | "initial" | "unset" | "revert") {
                ComputedValue::Keyword(keyword.clone())
            } else if is_color_keyword(keyword)
                || property_name.ends_with("color")
                || property_name == "color"
            {
                ComputedValue::Color(keyword.clone())
            } else {
                ComputedValue::Keyword(keyword.clone())
            }
        }
        Value::Length(number, unit) => {
            let px = resolve_length_to_px(*number, unit, ctx).unwrap_or(*number);
            ComputedValue::Px(px)
        }
        Value::Percentage(percent) => {
            if property_name == "font-size" {
                let px = ctx.parent_font_size * (*percent / 100.0);
                ComputedValue::Px(px)
            } else {
                ComputedValue::Percentage(*percent)
            }
        }
        Value::Color(color) => ComputedValue::Color(color.clone()),
        Value::String(value) => ComputedValue::String(value.clone()),
        Value::Number(value) => ComputedValue::Number(*value),
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") =>
        {
            if let Some(hex) = compute_rgb_function(arguments) {
                ComputedValue::Color(hex)
            } else {
                ComputedValue::Keyword(render_value(value))
            }
        }
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") =>
        {
            if let Some(hex) = compute_hsl_function(arguments) {
                ComputedValue::Color(hex)
            } else {
                ComputedValue::Keyword(render_value(value))
            }
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            if let Some(quantity) = evaluate_calc(arguments, ctx) {
                return match quantity.unit {
                    CalcUnit::Px => ComputedValue::Px(quantity.value),
                    CalcUnit::Percentage => {
                        if property_name == "font-size" {
                            ComputedValue::Px(ctx.parent_font_size * (quantity.value / 100.0))
                        } else {
                            ComputedValue::Percentage(quantity.value)
                        }
                    }
                    CalcUnit::Unitless => ComputedValue::Number(quantity.value),
                };
            }
            // Try to extract mixed px + percentage from calc() arguments.
            if let Some((px, pct)) = try_extract_calc_px_percent(arguments, ctx) {
                return ComputedValue::CalcPxPercent(px, pct);
            }
            ComputedValue::Keyword(render_value(value))
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("clamp") => {
            compute_clamp_function(arguments, property_name, ctx)
                .unwrap_or_else(|| ComputedValue::Keyword(render_value(value)))
        }
        Value::Function { .. } => ComputedValue::Keyword(render_value(value)),
        Value::List(values) => {
            if property_name.eq_ignore_ascii_case("transform")
                || property_name.eq_ignore_ascii_case("overflow")
                || property_name.eq_ignore_ascii_case("box-shadow")
                || property_name.eq_ignore_ascii_case("background-size")
                || property_name.eq_ignore_ascii_case("mask-size")
                || property_name.eq_ignore_ascii_case("border-spacing")
            {
                return ComputedValue::Keyword(render_value(value));
            }
            if property_name.eq_ignore_ascii_case("font-family") {
                return ComputedValue::Keyword(render_font_family_value(values));
            }
            if let Some(first) = values.first() {
                compute_value(first, property_name, ctx)
            } else {
                ComputedValue::Keyword(String::new())
            }
        }
    }
}

fn compute_clamp_function(
    arguments: &[Value],
    property_name: &str,
    ctx: ResolutionContext,
) -> Option<ComputedValue> {
    let [minimum, preferred, maximum] = arguments else {
        return None;
    };
    let minimum = resolve_clamp_quantity(minimum, property_name, ctx)?;
    let preferred = resolve_clamp_quantity(preferred, property_name, ctx)?;
    let maximum = resolve_clamp_quantity(maximum, property_name, ctx)?;
    if minimum.unit != preferred.unit || preferred.unit != maximum.unit {
        return None;
    }

    // CSS Values 4 defines clamp(MIN, VAL, MAX) as max(MIN, min(VAL, MAX)).
    let value = preferred.value.min(maximum.value).max(minimum.value);
    Some(match minimum.unit {
        CalcUnit::Px => ComputedValue::Px(value),
        CalcUnit::Percentage => ComputedValue::Percentage(value),
        CalcUnit::Unitless => ComputedValue::Number(value),
    })
}

fn resolve_clamp_quantity(
    value: &Value,
    property_name: &str,
    ctx: ResolutionContext,
) -> Option<CalcQuantity> {
    match value {
        Value::Length(number, unit) => Some(CalcQuantity {
            value: resolve_length_to_px(*number, unit, ctx)?,
            unit: CalcUnit::Px,
        }),
        Value::Percentage(number) if property_name.eq_ignore_ascii_case("font-size") => {
            Some(CalcQuantity {
                value: ctx.parent_font_size * (*number / 100.0),
                unit: CalcUnit::Px,
            })
        }
        Value::Percentage(number) => Some(CalcQuantity {
            value: *number,
            unit: CalcUnit::Percentage,
        }),
        Value::Number(number) => Some(CalcQuantity {
            value: *number,
            unit: CalcUnit::Unitless,
        }),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            let mut quantity = evaluate_calc(arguments, ctx)?;
            if property_name.eq_ignore_ascii_case("font-size")
                && quantity.unit == CalcUnit::Percentage
            {
                quantity.value = ctx.parent_font_size * (quantity.value / 100.0);
                quantity.unit = CalcUnit::Px;
            }
            Some(quantity)
        }
        _ => None,
    }
}

fn is_prefixed_property_alias(name: &str) -> bool {
    !canonical_property_name(name).eq_ignore_ascii_case(name)
}

fn canonical_property_name(name: &str) -> &str {
    if name.eq_ignore_ascii_case("-webkit-align-items")
        || name.eq_ignore_ascii_case("-ms-flex-align")
        || name.eq_ignore_ascii_case("-webkit-box-align")
    {
        "align-items"
    } else if name.eq_ignore_ascii_case("-webkit-justify-content")
        || name.eq_ignore_ascii_case("-ms-flex-pack")
        || name.eq_ignore_ascii_case("-webkit-box-pack")
    {
        "justify-content"
    } else if name.eq_ignore_ascii_case("-webkit-flex-shrink")
        || name.eq_ignore_ascii_case("-ms-flex-negative")
    {
        "flex-shrink"
    } else if name.eq_ignore_ascii_case("-webkit-flex-grow") {
        "flex-grow"
    } else if name.eq_ignore_ascii_case("-webkit-flex-direction") {
        "flex-direction"
    } else if name.eq_ignore_ascii_case("-webkit-flex-wrap") {
        "flex-wrap"
    } else if name.eq_ignore_ascii_case("-webkit-clip-path") {
        "clip-path"
    } else if name.eq_ignore_ascii_case("-webkit-transform") {
        "transform"
    } else if name.eq_ignore_ascii_case("-webkit-mask") {
        "mask"
    } else if name.eq_ignore_ascii_case("-webkit-mask-image") {
        "mask-image"
    } else if name.eq_ignore_ascii_case("-webkit-mask-position") {
        "mask-position"
    } else if name.eq_ignore_ascii_case("-webkit-mask-position-x") {
        "mask-position-x"
    } else if name.eq_ignore_ascii_case("-webkit-mask-position-y") {
        "mask-position-y"
    } else if name.eq_ignore_ascii_case("-webkit-mask-repeat") {
        "mask-repeat"
    } else if name.eq_ignore_ascii_case("-webkit-mask-size") {
        "mask-size"
    } else {
        name
    }
}

fn render_clip_path_value(value: &Value, ctx: ResolutionContext) -> String {
    match value {
        Value::Length(number, unit) => resolve_length_to_px(*number, unit, ctx)
            .map(|px| format!("{px}px"))
            .unwrap_or_else(|| format!("{number}{unit}")),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            if let Some(quantity) = evaluate_calc(arguments, ctx) {
                return match quantity.unit {
                    CalcUnit::Px => format!("{}px", quantity.value),
                    CalcUnit::Percentage => format!("{}%", quantity.value),
                    CalcUnit::Unitless => quantity.value.to_string(),
                };
            }
            if let Some((px, percentage)) = try_extract_calc_px_percent(arguments, ctx) {
                let operator = if percentage < 0.0 { '-' } else { '+' };
                return format!("calc({px}px {operator} {}%)", percentage.abs());
            }
            render_value(value)
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("inset") => format!(
            "inset({})",
            arguments
                .iter()
                .map(|argument| render_clip_path_value(argument, ctx))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::List(values) => values
            .iter()
            .map(|value| render_clip_path_value(value, ctx))
            .collect::<Vec<_>>()
            .join(" "),
        _ => render_value(value),
    }
}

fn render_grid_template_areas(value: &Value) -> String {
    fn quoted(row: &str) -> String {
        format!("\"{}\"", row.replace('\\', "\\\\").replace('"', "\\\""))
    }
    match value {
        Value::String(row) => quoted(row),
        Value::List(rows) => rows
            .iter()
            .map(|row| match row {
                Value::String(row) => quoted(row),
                value => render_value(value),
            })
            .collect::<Vec<_>>()
            .join(" "),
        value => render_value(value),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CalcUnit {
    Px,
    Percentage,
    Unitless,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CalcQuantity {
    value: f32,
    unit: CalcUnit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CalcToken {
    Value(CalcQuantity),
    Operator(char),
}

/// Tries to extract a simple `px + percent` or `percent - px` form from calc() arguments.
/// Returns `(px_component, percent_component)` if successful.
fn try_extract_calc_px_percent(arguments: &[Value], ctx: ResolutionContext) -> Option<(f32, f32)> {
    let mut tokens = Vec::new();
    collect_calc_tokens(arguments.first()?, ctx, &mut tokens)?;

    let mut px_total = 0.0f32;
    let mut pct_total = 0.0f32;
    let mut sign = 1.0f32;
    let mut saw_px = false;
    let mut saw_pct = false;

    for token in &tokens {
        match token {
            CalcToken::Value(q) => {
                match q.unit {
                    CalcUnit::Px => {
                        px_total += sign * q.value;
                        saw_px = true;
                    }
                    CalcUnit::Percentage => {
                        pct_total += sign * q.value;
                        saw_pct = true;
                    }
                    // Only accept unitless zero; reject other unitless values.
                    CalcUnit::Unitless if q.value == 0.0 => {}
                    CalcUnit::Unitless => return None,
                }
                sign = 1.0;
            }
            CalcToken::Operator('+') => sign = 1.0,
            CalcToken::Operator('-') => sign = -1.0,
            CalcToken::Operator(_) => return None,
        }
    }

    // Only return if we actually saw both px and percentage tokens.
    if saw_px && saw_pct {
        Some((px_total, pct_total))
    } else {
        None
    }
}

fn evaluate_calc(arguments: &[Value], ctx: ResolutionContext) -> Option<CalcQuantity> {
    let expression = arguments.first()?;
    let mut tokens = Vec::new();
    collect_calc_tokens(expression, ctx, &mut tokens)?;
    if tokens.is_empty() {
        return None;
    }

    let mut index = 0usize;
    let value = parse_calc_add_sub(&tokens, &mut index)?;
    if index == tokens.len() {
        Some(value)
    } else {
        None
    }
}

fn collect_calc_tokens(
    value: &Value,
    ctx: ResolutionContext,
    out: &mut Vec<CalcToken>,
) -> Option<()> {
    match value {
        Value::List(values) => {
            for item in values {
                collect_calc_tokens(item, ctx, out)?;
            }
            Some(())
        }
        Value::Keyword(op) if matches!(op.as_str(), "+" | "-" | "*" | "/") => {
            out.push(CalcToken::Operator(op.chars().next()?));
            Some(())
        }
        Value::Length(number, unit) => {
            let px = resolve_length_to_px(*number, unit, ctx)?;
            out.push(CalcToken::Value(CalcQuantity {
                value: px,
                unit: CalcUnit::Px,
            }));
            Some(())
        }
        Value::Percentage(number) => {
            out.push(CalcToken::Value(CalcQuantity {
                value: *number,
                unit: CalcUnit::Percentage,
            }));
            Some(())
        }
        Value::Number(number) => {
            out.push(CalcToken::Value(CalcQuantity {
                value: *number,
                unit: CalcUnit::Unitless,
            }));
            Some(())
        }
        _ => None,
    }
}

fn resolve_length_to_px(number: f32, unit: &str, ctx: ResolutionContext) -> Option<f32> {
    Some(match unit.to_ascii_lowercase().as_str() {
        "px" => number,
        "em" => number * ctx.parent_font_size,
        "rem" => number * ctx.root_font_size,
        "vw" => number * ctx.viewport_width / 100.0,
        "vh" => number * ctx.viewport_height / 100.0,
        "vmin" => number * ctx.viewport_width.min(ctx.viewport_height) / 100.0,
        "vmax" => number * ctx.viewport_width.max(ctx.viewport_height) / 100.0,
        "mm" => number * (96.0 / 25.4),
        "cm" => number * (96.0 / 2.54),
        "in" => number * 96.0,
        "pt" => number * (96.0 / 72.0),
        "pc" => number * (96.0 / 6.0),
        _ => return None,
    })
}

fn parse_calc_add_sub(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let mut left = parse_calc_mul_div(tokens, index)?;
    while let Some(CalcToken::Operator(op @ ('+' | '-'))) = tokens.get(*index) {
        let op = *op;
        *index += 1;
        let right = parse_calc_mul_div(tokens, index)?;
        left = apply_calc_operator(left, op, right)?;
    }
    Some(left)
}

fn parse_calc_mul_div(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let mut left = parse_calc_factor(tokens, index)?;
    while let Some(CalcToken::Operator(op @ ('*' | '/'))) = tokens.get(*index) {
        let op = *op;
        *index += 1;
        let right = parse_calc_factor(tokens, index)?;
        left = apply_calc_operator(left, op, right)?;
    }
    Some(left)
}

fn parse_calc_factor(tokens: &[CalcToken], index: &mut usize) -> Option<CalcQuantity> {
    let value = match tokens.get(*index) {
        Some(CalcToken::Value(value)) => *value,
        _ => return None,
    };
    *index += 1;
    Some(value)
}

fn apply_calc_operator(left: CalcQuantity, op: char, right: CalcQuantity) -> Option<CalcQuantity> {
    match op {
        '+' => add_or_sub_calc_quantities(left, right, false),
        '-' => add_or_sub_calc_quantities(left, right, true),
        '*' => multiply_calc_quantities(left, right),
        '/' => divide_calc_quantities(left, right),
        _ => None,
    }
}

fn add_or_sub_calc_quantities(
    left: CalcQuantity,
    right: CalcQuantity,
    subtract: bool,
) -> Option<CalcQuantity> {
    if left.unit != right.unit {
        return None;
    }
    let rhs = if subtract { -right.value } else { right.value };
    Some(CalcQuantity {
        value: left.value + rhs,
        unit: left.unit,
    })
}

fn multiply_calc_quantities(left: CalcQuantity, right: CalcQuantity) -> Option<CalcQuantity> {
    match (left.unit, right.unit) {
        (CalcUnit::Unitless, unit) => Some(CalcQuantity {
            value: left.value * right.value,
            unit,
        }),
        (unit, CalcUnit::Unitless) => Some(CalcQuantity {
            value: left.value * right.value,
            unit,
        }),
        _ => None,
    }
}

fn divide_calc_quantities(left: CalcQuantity, right: CalcQuantity) -> Option<CalcQuantity> {
    if right.value == 0.0 || right.unit != CalcUnit::Unitless {
        return None;
    }
    Some(CalcQuantity {
        value: left.value / right.value,
        unit: left.unit,
    })
}

fn apply_presentational_hints(
    node: &NodeHandle,
    properties: &mut BTreeMap<String, ComputedValue>,
    pseudo: Option<PseudoElement>,
) {
    if pseudo.is_some() || node.node_type() != NodeType::Element {
        return;
    }

    let attributes = node.attributes().unwrap_or_default();

    if !properties.contains_key("background-color")
        && let Some(background) = attributes
            .get("bgcolor")
            .and_then(|value| parse_legacy_color_hint(value))
        {
            properties.insert(
                "background-color".to_string(),
                ComputedValue::Color(background),
            );
        }

    if !properties.contains_key("background-image")
        && let Some(background) = attributes
            .get("background")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            let escaped = background.replace('\\', "\\\\").replace('"', "\\\"");
            properties.insert(
                "background-image".to_string(),
                ComputedValue::Keyword(format!("url(\"{escaped}\")")),
            );
        }

    if !properties.contains_key("color")
        && node
            .tag_name()
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("body"))
        && let Some(color) = attributes
            .get("text")
            .and_then(|value| parse_legacy_color_hint(value))
        {
            properties.insert("color".to_string(), ComputedValue::Color(color));
        }

    if let Some(align) = attributes
        .get("align")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| matches!(value.as_str(), "left" | "right" | "center" | "justify"))
    {
        if !properties.contains_key("text-align") {
            properties.insert(
                "text-align".to_string(),
                ComputedValue::Keyword(align.clone()),
            );
        }
        // For block/table elements, align="center" means auto margins (structural centering)
        if align == "center" {
            let is_table_or_block = node
                .tag_name()
                .as_deref()
                .is_some_and(|tag| {
                    matches!(
                        tag.to_ascii_lowercase().as_str(),
                        "table" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p"
                    )
                });
            if is_table_or_block {
                if !properties.contains_key("margin-left") {
                    properties.insert(
                        "margin-left".to_string(),
                        ComputedValue::Keyword("auto".to_string()),
                    );
                }
                if !properties.contains_key("margin-right") {
                    properties.insert(
                        "margin-right".to_string(),
                        ComputedValue::Keyword("auto".to_string()),
                    );
                }
            }
        }
    }

    if !properties.contains_key("width")
        && let Some(width) = attributes
            .get("width")
            .and_then(|value| parse_legacy_dimension_hint(value))
        {
            properties.insert("width".to_string(), width);
        }

    if !properties.contains_key("height")
        && let Some(height) = attributes
            .get("height")
            .and_then(|value| parse_legacy_dimension_hint(value))
        {
            properties.insert("height".to_string(), height);
        }

    if !properties.contains_key("color")
        && let Some(color) = attributes
            .get("color")
            .and_then(|value| parse_legacy_color_hint(value))
        {
            properties.insert("color".to_string(), ComputedValue::Color(color));
        }

    if !properties.contains_key("font-family")
        && let Some(face) = attributes
            .get("face")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            properties.insert("font-family".to_string(), ComputedValue::Keyword(face));
        }
}

fn parse_legacy_color_hint(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(hex) = value.strip_prefix('#') {
        return if is_hex_color(hex) {
            Some(format!("#{hex}").to_ascii_lowercase())
        } else {
            None
        };
    }

    if is_hex_color(value) {
        return Some(format!("#{value}").to_ascii_lowercase());
    }

    if value.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Some(value.to_ascii_lowercase());
    }

    None
}

fn parse_legacy_dimension_hint(value: &str) -> Option<ComputedValue> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(percent) = value
        .strip_suffix('%')
        .and_then(|v| v.trim().parse::<f32>().ok())
    {
        return Some(ComputedValue::Percentage(percent));
    }

    if let Some(px) = value
        .strip_suffix("px")
        .and_then(|v| v.trim().parse::<f32>().ok())
    {
        return Some(ComputedValue::Px(px.max(0.0)));
    }

    value
        .parse::<f32>()
        .ok()
        .map(|px| ComputedValue::Px(px.max(0.0)))
}

fn is_hex_color(value: &str) -> bool {
    (value.len() == 3 || value.len() == 6) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn apply_ua_defaults(
    node: &NodeHandle,
    properties: &mut BTreeMap<String, ComputedValue>,
    pseudo: Option<PseudoElement>,
    parent_style: Option<&ComputedStyle>,
) {
    if pseudo.is_some() || node.node_type() != NodeType::Element {
        return;
    }
    let tag = match node.tag_name() {
        Some(tag) => tag.to_ascii_lowercase(),
        None => return,
    };
    let parent_font_size = inherited_font_size(parent_style, properties);

    if tag != "summary"
        && node.parent_node().is_some_and(|parent| {
            parent.tag_name().as_deref() == Some("details")
                && parent.get_attribute("open").is_none()
        })
    {
        properties.insert(
            "display".to_string(),
            ComputedValue::Keyword("none".to_string()),
        );
        return;
    }

    // UA stylesheet defaults per CSS 2.1 Appendix D / HTML spec
    struct UaDefaults {
        font_size_em: f32,
        font_weight_bold: bool,
        margin_em: f32,
    }

    let defaults = match tag.as_str() {
        "h1" => Some(UaDefaults { font_size_em: 2.0, font_weight_bold: true, margin_em: 0.67 }),
        "h2" => Some(UaDefaults { font_size_em: 1.5, font_weight_bold: true, margin_em: 0.83 }),
        "h3" => Some(UaDefaults { font_size_em: 1.17, font_weight_bold: true, margin_em: 1.0 }),
        "h4" => Some(UaDefaults { font_size_em: 1.0, font_weight_bold: true, margin_em: 1.33 }),
        "h5" => Some(UaDefaults { font_size_em: 0.83, font_weight_bold: true, margin_em: 1.67 }),
        "h6" => Some(UaDefaults { font_size_em: 0.67, font_weight_bold: true, margin_em: 2.33 }),
        _ => None,
    };

    if let Some(defaults) = defaults {
        // Determine the element's final font size: use existing CSS value if present,
        // otherwise apply the UA default multiplier to the inherited size.
        let element_font_size =
            if let Some(ComputedValue::Px(existing_px)) = properties.get("font-size") {
                *existing_px
            } else {
                let computed = defaults.font_size_em * parent_font_size;
                properties
                    .entry("font-size".to_string())
                    .or_insert(ComputedValue::Px(computed));
                computed
            };
        let margin_px = defaults.margin_em * element_font_size;
        if defaults.font_weight_bold {
            properties
                .entry("font-weight".to_string())
                .or_insert(ComputedValue::Keyword("bold".to_string()));
        }
        properties
            .entry("margin-top".to_string())
            .or_insert(ComputedValue::Px(margin_px));
        properties
            .entry("margin-bottom".to_string())
            .or_insert(ComputedValue::Px(margin_px));
        return;
    }

    match tag.as_str() {
        "details" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("block".to_string()));
        }
        "summary" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("list-item".to_string()));
        }
        "dialog" => {
            if node.get_attribute("open").is_none() {
                properties.insert(
                    "display".to_string(),
                    ComputedValue::Keyword("none".to_string()),
                );
            } else {
                properties
                    .entry("display".to_string())
                    .or_insert(ComputedValue::Keyword("block".to_string()));
            }
        }
        "time" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline".to_string()));
        }
        "progress" | "meter" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline-block".to_string()));
            properties
                .entry("width".to_string())
                .or_insert(ComputedValue::Px(160.0));
            properties
                .entry("height".to_string())
                .or_insert(ComputedValue::Px(16.0));
            properties
                .entry("background-color".to_string())
                .or_insert(ComputedValue::Color("#e6e6e6".to_string()));
            for side in ["top", "right", "bottom", "left"] {
                properties
                    .entry(format!("border-{side}-style"))
                    .or_insert(ComputedValue::Keyword("solid".to_string()));
                properties
                    .entry(format!("border-{side}-width"))
                    .or_insert(ComputedValue::Px(1.0));
                properties
                    .entry(format!("border-{side}-color"))
                    .or_insert(ComputedValue::Color("#767676".to_string()));
            }
        }
        "form" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("block".to_string()));
        }
        "input" => {
            let input_type = node
                .get_attribute("type")
                .unwrap_or_else(|| "text".to_string())
                .trim()
                .to_ascii_lowercase();
            if input_type == "hidden" {
                properties.insert(
                    "display".to_string(),
                    ComputedValue::Keyword("none".to_string()),
                );
            } else {
                properties
                    .entry("display".to_string())
                    .or_insert(ComputedValue::Keyword("inline-block".to_string()));
                properties
                    .entry("background-color".to_string())
                    .or_insert(ComputedValue::Color("white".to_string()));
                for side in ["top", "right", "bottom", "left"] {
                    properties
                        .entry(format!("border-{side}-style"))
                        .or_insert(ComputedValue::Keyword("solid".to_string()));
                    properties
                        .entry(format!("border-{side}-width"))
                        .or_insert(ComputedValue::Px(2.0));
                    properties
                        .entry(format!("border-{side}-color"))
                        .or_insert(ComputedValue::Color("#767676".to_string()));
                }
                properties
                    .entry("padding-top".to_string())
                    .or_insert(ComputedValue::Px(1.0));
                properties
                    .entry("padding-right".to_string())
                    .or_insert(ComputedValue::Px(2.0));
                properties
                    .entry("padding-bottom".to_string())
                    .or_insert(ComputedValue::Px(1.0));
                properties
                    .entry("padding-left".to_string())
                    .or_insert(ComputedValue::Px(2.0));
            }
        }
        "button" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline-block".to_string()));
            properties
                .entry("background-color".to_string())
                .or_insert(ComputedValue::Color("#efefef".to_string()));
            properties
                .entry("text-align".to_string())
                .or_insert(ComputedValue::Keyword("center".to_string()));
            for side in ["top", "right", "bottom", "left"] {
                properties
                    .entry(format!("border-{side}-style"))
                    .or_insert(ComputedValue::Keyword("solid".to_string()));
                properties
                    .entry(format!("border-{side}-width"))
                    .or_insert(ComputedValue::Px(2.0));
                properties
                    .entry(format!("border-{side}-color"))
                    .or_insert(ComputedValue::Color("#767676".to_string()));
            }
            properties
                .entry("padding-top".to_string())
                .or_insert(ComputedValue::Px(1.0));
            properties
                .entry("padding-right".to_string())
                .or_insert(ComputedValue::Px(6.0));
            properties
                .entry("padding-bottom".to_string())
                .or_insert(ComputedValue::Px(1.0));
            properties
                .entry("padding-left".to_string())
                .or_insert(ComputedValue::Px(6.0));
        }
        "textarea" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline-block".to_string()));
            properties
                .entry("background-color".to_string())
                .or_insert(ComputedValue::Color("white".to_string()));
            for side in ["top", "right", "bottom", "left"] {
                properties
                    .entry(format!("border-{side}-style"))
                    .or_insert(ComputedValue::Keyword("solid".to_string()));
                properties
                    .entry(format!("border-{side}-width"))
                    .or_insert(ComputedValue::Px(1.0));
                properties
                    .entry(format!("border-{side}-color"))
                    .or_insert(ComputedValue::Color("#767676".to_string()));
            }
            for side in ["top", "right", "bottom", "left"] {
                properties
                    .entry(format!("padding-{side}"))
                    .or_insert(ComputedValue::Px(2.0));
            }
        }
        "select" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline-block".to_string()));
            properties
                .entry("background-color".to_string())
                .or_insert(ComputedValue::Color("#efefef".to_string()));
            for side in ["top", "right", "bottom", "left"] {
                properties
                    .entry(format!("border-{side}-style"))
                    .or_insert(ComputedValue::Keyword("solid".to_string()));
                properties
                    .entry(format!("border-{side}-width"))
                    .or_insert(ComputedValue::Px(1.0));
                properties
                    .entry(format!("border-{side}-color"))
                    .or_insert(ComputedValue::Color("#767676".to_string()));
            }
            properties
                .entry("padding-top".to_string())
                .or_insert(ComputedValue::Px(1.0));
            properties
                .entry("padding-right".to_string())
                .or_insert(ComputedValue::Px(4.0));
            properties
                .entry("padding-bottom".to_string())
                .or_insert(ComputedValue::Px(1.0));
            properties
                .entry("padding-left".to_string())
                .or_insert(ComputedValue::Px(4.0));
        }
        "p" => {
            let em = parent_font_size;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(em));
        }
        "b" | "strong" => {
            properties.entry("font-weight".to_string()).or_insert(ComputedValue::Keyword("bold".to_string()));
        }
        "i" | "em" => {
            properties.entry("font-style".to_string()).or_insert(ComputedValue::Keyword("italic".to_string()));
        }
        "hr" => {
            properties.entry("border-top-style".to_string()).or_insert(ComputedValue::Keyword("inset".to_string()));
            properties.entry("border-top-width".to_string()).or_insert(ComputedValue::Px(1.0));
            let half_em = parent_font_size * 0.5;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(half_em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(half_em));
        }
        "ul" => {
            properties
                .entry("list-style-type".to_string())
                .or_insert(ComputedValue::Keyword("disc".to_string()));
            properties
                .entry("list-style-position".to_string())
                .or_insert(ComputedValue::Keyword("outside".to_string()));
            let em = parent_font_size;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("padding-left".to_string()).or_insert(ComputedValue::Px(em * 2.5));
        }
        "ol" => {
            properties
                .entry("list-style-type".to_string())
                .or_insert(ComputedValue::Keyword("decimal".to_string()));
            properties
                .entry("list-style-position".to_string())
                .or_insert(ComputedValue::Keyword("outside".to_string()));
            let em = parent_font_size;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("padding-left".to_string()).or_insert(ComputedValue::Px(em * 2.5));
        }
        "li" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("list-item".to_string()));
        }
        "blockquote" => {
            let em = parent_font_size;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("margin-left".to_string()).or_insert(ComputedValue::Px(40.0));
            properties.entry("margin-right".to_string()).or_insert(ComputedValue::Px(40.0));
        }
        "pre" => {
            properties.entry("font-family".to_string()).or_insert(ComputedValue::Keyword("monospace".to_string()));
            properties.entry("white-space".to_string()).or_insert(ComputedValue::Keyword("pre".to_string()));
            let em = parent_font_size;
            properties.entry("margin-top".to_string()).or_insert(ComputedValue::Px(em));
            properties.entry("margin-bottom".to_string()).or_insert(ComputedValue::Px(em));
        }
        "code" | "kbd" | "samp" | "tt" => {
            properties.entry("font-family".to_string()).or_insert(ComputedValue::Keyword("monospace".to_string()));
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("inline".to_string()));
        }
        "dd" => {
            properties.entry("margin-left".to_string()).or_insert(ComputedValue::Px(40.0));
        }
        "th" => {
            properties.entry("font-weight".to_string()).or_insert(ComputedValue::Keyword("bold".to_string()));
            properties.entry("text-align".to_string()).or_insert(ComputedValue::Keyword("center".to_string()));
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("table-cell".to_string()));
        }
        "td" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("table-cell".to_string()));
        }
        "a" => {
            properties.entry("text-decoration-line".to_string()).or_insert(ComputedValue::Keyword("underline".to_string()));
            properties.entry("color".to_string()).or_insert(ComputedValue::Color("#0000ee".to_string()));
        }
        "sub" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("inline".to_string()));
            properties.entry("vertical-align".to_string()).or_insert(ComputedValue::Keyword("sub".to_string()));
            let smaller = parent_font_size * 0.833;
            properties.entry("font-size".to_string()).or_insert(ComputedValue::Px(smaller));
        }
        "sup" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("inline".to_string()));
            properties.entry("vertical-align".to_string()).or_insert(ComputedValue::Keyword("super".to_string()));
            let smaller = parent_font_size * 0.833;
            properties.entry("font-size".to_string()).or_insert(ComputedValue::Px(smaller));
        }
        "small" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("inline".to_string()));
            let smaller = parent_font_size * 0.833;
            properties.entry("font-size".to_string()).or_insert(ComputedValue::Px(smaller));
        }
        "center" => {
            properties.entry("text-align".to_string()).or_insert(ComputedValue::Keyword("center".to_string()));
        }
        "table" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("table".to_string()));
        }
        "tr" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("table-row".to_string()));
        }
        "thead" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("table-header-group".to_string()));
        }
        "tbody" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("table-row-group".to_string()));
        }
        "tfoot" => {
            properties.entry("display".to_string()).or_insert(ComputedValue::Keyword("table-footer-group".to_string()));
        }
        _ => {}
    }
}

fn apply_initial_values(properties: &mut BTreeMap<String, ComputedValue>) {
    properties
        .entry("color".to_string())
        .or_insert_with(|| ComputedValue::Color("black".to_string()));
    properties
        .entry("font-size".to_string())
        .or_insert_with(|| ComputedValue::Px(16.0));
    properties
        .entry("text-transform".to_string())
        .or_insert_with(|| ComputedValue::Keyword("none".to_string()));
    // `cursor` initial value is `auto` (CSS UI). Ensuring it is always present
    // lets a dropped/absent `cursor` declaration serialize as `auto` in
    // getComputedStyle (Acid3 test 47).
    properties
        .entry("cursor".to_string())
        .or_insert_with(|| ComputedValue::Keyword("auto".to_string()));
}

/// CSS 2.1 §8.5.3: If border-style is 'none', the computed border-width is 0.
fn zero_border_width_for_none_style(properties: &mut BTreeMap<String, ComputedValue>) {
    for side in ["top", "right", "bottom", "left"] {
        let style_key = format!("border-{side}-style");
        let is_none = matches!(
            properties.get(&style_key),
            Some(ComputedValue::Keyword(keyword)) if keyword.eq_ignore_ascii_case("none")
        );
        if is_none {
            let width_key = format!("border-{side}-width");
            properties.insert(width_key, ComputedValue::Px(0.0));
        }
    }
}

/// `color: currentColor` is equivalent to `color: inherit` per CSS Color Level 4.
/// Resolve it before general inherit resolution so other properties that reference
/// currentColor can see the resolved color value.
fn resolve_current_color_on_color_property(
    properties: &mut BTreeMap<String, ComputedValue>,
    parent_style: Option<&ComputedStyle>,
) {
    let is_current_color = matches!(
        properties.get("color"),
        Some(ComputedValue::Color(c)) if c.eq_ignore_ascii_case("currentcolor")
    ) || matches!(
        properties.get("color"),
        Some(ComputedValue::Keyword(k)) if k.eq_ignore_ascii_case("currentcolor")
    );
    if is_current_color {
        if let Some(parent) = parent_style {
            if let Some(parent_color) = parent.get("color") {
                properties.insert("color".to_string(), parent_color.clone());
            } else {
                // Root element with color: currentColor → initial value (black)
                properties.insert(
                    "color".to_string(),
                    ComputedValue::Color("black".to_string()),
                );
            }
        } else {
            properties.insert(
                "color".to_string(),
                ComputedValue::Color("black".to_string()),
            );
        }
    }
}

fn resolve_explicit_inherit(
    properties: &mut BTreeMap<String, ComputedValue>,
    parent_style: Option<&ComputedStyle>,
) {
    let inherited_names: Vec<String> = properties
        .iter()
        .filter_map(|(name, value)| match value {
            ComputedValue::Keyword(keyword) if keyword.eq_ignore_ascii_case("inherit") => {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();

    for name in inherited_names {
        if let Some(parent_style) = parent_style
            && let Some(parent_value) = parent_style.get(&name) {
                properties.insert(name, parent_value.clone());
                continue;
            }
        properties.remove(&name);
    }
}

fn apply_inheritance(
    properties: &mut BTreeMap<String, ComputedValue>,
    parent_style: Option<&ComputedStyle>,
) {
    let Some(parent_style) = parent_style else {
        return;
    };

    // Inherited CSS properties supported by this engine.
    // Based on CSS 2.1 §6.2 and CSS Text Decoration Module Level 3.
    // https://developer.mozilla.org/en-US/docs/Web/CSS/Inheritance
    for inherited_name in [
        "border-collapse",
        "border-spacing",
        "color",
        "cursor",
        "direction",
        "font-family",
        "font-size",
        "font-style",
        "font-weight",
        "letter-spacing",
        "line-height",
        "list-style-image",
        "list-style-position",
        "list-style-type",
        "overflow-wrap",
        "text-align",
        "text-decoration-color",
        "text-decoration-line",
        "text-decoration-style",
        "text-indent",
        "text-transform",
        "visibility",
        "white-space",
        "word-break",
        "word-spacing",
    ] {
        if !properties.contains_key(inherited_name)
            && let Some(value) = parent_style.get(inherited_name) {
                properties.insert(inherited_name.to_string(), value.clone());
            }
    }

    // CSS custom properties inherit by default.
    for (name, value) in parent_style.properties() {
        if name.starts_with("--") && !properties.contains_key(name) {
            properties.insert(name.clone(), value.clone());
        }
    }
}

fn inherited_font_size(
    parent_style: Option<&ComputedStyle>,
    current: &BTreeMap<String, ComputedValue>,
) -> f32 {
    if let Some(ComputedValue::Px(value)) = current.get("font-size") {
        return *value;
    }
    if let Some(parent_style) = parent_style
        && let Some(ComputedValue::Px(value)) = parent_style.get("font-size") {
            return *value;
        }
    16.0
}

pub(crate) fn is_color_keyword(keyword: &str) -> bool {
    if keyword.eq_ignore_ascii_case("currentcolor") {
        return true;
    }
    matches!(
        keyword,
        "black"
            | "white"
            | "red"
            | "green"
            | "blue"
            | "gray"
            | "grey"
            | "silver"
            | "aqua"
            | "teal"
            | "lime"
            | "fuchsia"
            | "olive"
            | "navy"
            | "purple"
            | "maroon"
            | "yellow"
            | "orange"
            | "coral"
            | "salmon"
            | "tomato"
            | "orangered"
            | "darkorange"
            | "gold"
            | "goldenrod"
            | "darkgoldenrod"
            | "peru"
            | "chocolate"
            | "sienna"
            | "saddlebrown"
            | "brown"
            | "firebrick"
            | "darkred"
            | "crimson"
            | "pink"
            | "lightpink"
            | "hotpink"
            | "deeppink"
            | "palevioletred"
            | "mediumvioletred"
            | "lavender"
            | "thistle"
            | "plum"
            | "violet"
            | "orchid"
            | "magenta"
            | "mediumorchid"
            | "darkorchid"
            | "darkviolet"
            | "blueviolet"
            | "indigo"
            | "slateblue"
            | "darkslateblue"
            | "mediumpurple"
            | "rebeccapurple"
            | "lightblue"
            | "powderblue"
            | "lightskyblue"
            | "skyblue"
            | "deepskyblue"
            | "dodgerblue"
            | "cornflowerblue"
            | "steelblue"
            | "royalblue"
            | "mediumblue"
            | "darkblue"
            | "midnightblue"
            | "azure"
            | "aliceblue"
            | "ghostwhite"
            | "mintcream"
            | "honeydew"
            | "lightgreen"
            | "palegreen"
            | "limegreen"
            | "mediumseagreen"
            | "seagreen"
            | "forestgreen"
            | "darkgreen"
            | "yellowgreen"
            | "olivedrab"
            | "darkolivegreen"
            | "mediumaquamarine"
            | "aquamarine"
            | "turquoise"
            | "mediumturquoise"
            | "darkturquoise"
            | "lightseagreen"
            | "cadetblue"
            | "darkcyan"
            | "cyan"
            | "darkslategray"
            | "darkslategrey"
            | "slategray"
            | "slategrey"
            | "lightslategray"
            | "lightslategrey"
            | "darkgray"
            | "darkgrey"
            | "dimgray"
            | "dimgrey"
            | "lightgray"
            | "lightgrey"
            | "gainsboro"
            | "whitesmoke"
            | "snow"
            | "seashell"
            | "floralwhite"
            | "ivory"
            | "linen"
            | "oldlace"
            | "antiquewhite"
            | "bisque"
            | "blanchedalmond"
            | "wheat"
            | "moccasin"
            | "navajowhite"
            | "peachpuff"
            | "mistyrose"
            | "papayawhip"
            | "lightyellow"
            | "lemonchiffon"
            | "khaki"
            | "darkkhaki"
            | "palegoldenrod"
            | "beige"
            | "cornsilk"
            | "chartreuse"
            | "greenyellow"
            | "lawngreen"
            | "springgreen"
            | "mediumspringgreen"
            | "transparent"
    )
}

/// Extracts a numeric channel value from a CSS `Value`.
/// Handles `Value::Number` directly and `Value::Percentage` by clamping to 0–255.
fn extract_channel(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(*n),
        Value::Percentage(p) => Some(p * 255.0 / 100.0),
        _ => None,
    }
}

/// Extracts an alpha value (0.0–1.0) from a CSS `Value`.
fn extract_alpha(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(n.clamp(0.0, 1.0)),
        Value::Percentage(p) => Some((p / 100.0).clamp(0.0, 1.0)),
        _ => None,
    }
}

/// Flattens function arguments by expanding a single-argument `Value::List`.
///
/// Modern CSS color syntax `rgb(r g b / a)` is parsed as one argument that is
/// a `Value::List`.  This helper normalises both forms — comma-separated and
/// space-separated — into a flat slice.
fn flatten_color_args(arguments: &[Value]) -> Vec<&Value> {
    if arguments.len() == 1
        && let Value::List(items) = &arguments[0] {
            return items.iter().collect();
        }
    arguments.iter().collect()
}

/// Converts an `rgb()` or `rgba()` argument list into a hex color string.
///
/// Handles both the legacy comma-separated syntax and the modern
/// space-separated syntax with an optional `/ alpha` component.
fn compute_rgb_function(arguments: &[Value]) -> Option<String> {
    let flat = flatten_color_args(arguments);
    let (rgb_values, alpha) = split_slash(&flat);

    // rgb_values are the channels before "/"
    let channels: Vec<f32> = rgb_values
        .iter()
        .filter_map(|v| extract_channel(v))
        .collect();

    // Use the 4th value as alpha for rgba(r,g,b,a) comma form.
    // Extract via extract_alpha (not extract_channel) so percentages are 0-1.
    let a = alpha.or_else(|| {
        let flat = flatten_color_args(arguments);
        flat.get(3).and_then(|v| extract_alpha(v))
    });

    let (r, g, b) = match channels.as_slice() {
        [r, g, b] | [r, g, b, _] => (
            r.round().clamp(0.0, 255.0) as u8,
            g.round().clamp(0.0, 255.0) as u8,
            b.round().clamp(0.0, 255.0) as u8,
        ),
        _ => return None,
    };

    format_color_hex(r, g, b, a)
}

/// Converts an `hsl()` or `hsla()` argument list into a hex color string.
fn compute_hsl_function(arguments: &[Value]) -> Option<String> {
    let flat = flatten_color_args(arguments);
    let (hsl_values, alpha) = split_slash(&flat);

    let numbers: Vec<f32> = hsl_values
        .iter()
        .filter_map(|v| match v {
            Value::Number(n) => Some(*n),
            Value::Percentage(p) => Some(*p),
            _ => None,
        })
        .collect();

    // Use 4th value as alpha for hsla(h,s%,l%,a) comma form.
    // Extract via extract_alpha so percentages are 0-1.
    let a = alpha.or_else(|| {
        flat.get(3).and_then(|v| extract_alpha(v))
    });

    let (h, s, l) = match numbers.as_slice() {
        [h, s, l] | [h, s, l, _] => (*h, *s, *l),
        _ => return None,
    };

    let (r, g, b) = hsl_to_rgb(h, s / 100.0, l / 100.0);
    format_color_hex(r, g, b, a)
}

/// Formats an RGBA color as a hex string.
///
/// Omits the alpha byte when fully opaque to produce the shorter `#rrggbb` form.
fn format_color_hex(r: u8, g: u8, b: u8, a: Option<f32>) -> Option<String> {
    match a {
        Some(a) if a < 1.0 - f32::EPSILON => {
            let a_byte = (a * 255.0).round() as u8;
            Some(format!("#{r:02x}{g:02x}{b:02x}{a_byte:02x}"))
        }
        _ => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

/// Splits a flat argument list around the `/` keyword into the before and after parts.
///
/// Returns the values before `/`, and the alpha value after `/` (if any).
fn split_slash<'a>(flat: &[&'a Value]) -> (Vec<&'a Value>, Option<f32>) {
    let slash_pos = flat
        .iter()
        .position(|v| matches!(v, Value::Keyword(k) if k == "/"));

    if let Some(pos) = slash_pos {
        let before = flat[..pos].to_vec();
        let alpha = flat.get(pos + 1).and_then(|v| extract_alpha(v));
        (before, alpha)
    } else {
        (flat.to_vec(), None)
    }
}

/// Converts HSL to RGB.  All inputs and outputs are in the 0–255 / 0–360 range.
///
/// - `h`: hue in degrees (0–360)
/// - `s`: saturation as fraction (0.0–1.0)
/// - `l`: lightness as fraction (0.0–1.0)
pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    // CSS allows hue values outside 0-360; wrap to canonical range
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);

    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let h = h / 360.0;

    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);

    (
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Keyword(value) => value.clone(),
        Value::Length(number, unit) => format!("{number}{unit}"),
        Value::Color(value) => value.clone(),
        Value::Function { name, arguments } => format!(
            "{name}({})",
            arguments.iter().map(render_value).collect::<Vec<_>>().join(
                if name.eq_ignore_ascii_case("url") {
                    ","
                } else {
                    ", "
                }
            )
        ),
        Value::List(values) => values
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(" "),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        Value::Percentage(value) => format!("{value}%"),
    }
}

fn render_grid_track_value(value: &Value, ctx: ResolutionContext) -> String {
    match value {
        Value::Length(number, unit) if unit.eq_ignore_ascii_case("fr") => {
            format!("{number}fr")
        }
        Value::Length(number, unit) => resolve_length_to_px(*number, unit, ctx)
            .map(|px| format!("{px}px"))
            .unwrap_or_else(|| format!("{number}{unit}")),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            if let Some(quantity) = evaluate_calc(arguments, ctx) {
                return match quantity.unit {
                    CalcUnit::Px => format!("{}px", quantity.value),
                    CalcUnit::Percentage => format!("{}%", quantity.value),
                    CalcUnit::Unitless => quantity.value.to_string(),
                };
            }
            if let Some((px, percentage)) = try_extract_calc_px_percent(arguments, ctx) {
                let operator = if percentage < 0.0 { '-' } else { '+' };
                return format!("calc({px}px {operator} {}%)", percentage.abs());
            }
            render_value(value)
        }
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("minmax") => {
            let rendered = arguments
                .iter()
                .map(|argument| match argument {
                    Value::Number(number) if *number == 0.0 => "0px".to_string(),
                    _ => render_grid_track_value(argument, ctx),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{name}({rendered})")
        }
        Value::Function { name, arguments } => format!(
            "{name}({})",
            arguments
                .iter()
                .map(|argument| render_grid_track_value(argument, ctx))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::List(values) => values
            .iter()
            .map(|value| render_grid_track_value(value, ctx))
            .collect::<Vec<_>>()
            .join(" "),
        _ => render_value(value),
    }
}

fn render_font_family_value(values: &[Value]) -> String {
    values
        .iter()
        .map(render_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn inherited_custom_properties(parent_style: Option<&ComputedStyle>) -> BTreeMap<String, Value> {
    let mut custom_properties = BTreeMap::new();
    let Some(parent_style) = parent_style else {
        return custom_properties;
    };
    for (name, value) in parent_style.properties() {
        if !name.starts_with("--") {
            continue;
        }
        custom_properties.insert(name.clone(), computed_to_value(value));
    }
    custom_properties
}

fn computed_to_value(value: &ComputedValue) -> Value {
    match value {
        ComputedValue::Keyword(value) => Value::Keyword(value.clone()),
        ComputedValue::Px(value) => Value::Length(*value, "px".to_string()),
        ComputedValue::Percentage(value) => Value::Percentage(*value),
        ComputedValue::Color(value) => Value::Color(value.clone()),
        ComputedValue::String(value) => Value::String(value.clone()),
        ComputedValue::Number(value) => Value::Number(*value),
        ComputedValue::CalcPxPercent(px, pct) => {
            Value::Keyword(format!("calc({}px + {}%)", px, pct))
        }
    }
}

/// Converts a `ComputedValue` back into a `Value` for re-processing (e.g., var() resolution).
fn computed_value_to_value(cv: &ComputedValue) -> Value {
    match cv {
        ComputedValue::Px(v) => Value::Length(*v, "px".to_string()),
        ComputedValue::Number(v) => Value::Number(*v),
        ComputedValue::Percentage(v) => Value::Percentage(*v),
        ComputedValue::Color(c) => Value::Keyword(c.clone()),
        ComputedValue::Keyword(k) => Value::Keyword(k.clone()),
        ComputedValue::String(s) => Value::Keyword(s.clone()),
        ComputedValue::CalcPxPercent(px, pct) => {
            Value::Keyword(format!("calc({}px + {}%)", px, pct))
        }
    }
}

fn resolve_value_with_custom_properties(
    value: &Value,
    custom_properties: &BTreeMap<String, Value>,
) -> Option<Value> {
    let mut stack = Vec::new();
    resolve_value_with_custom_properties_inner(value, custom_properties, &mut stack, 0)
}

fn resolve_value_with_custom_properties_inner(
    value: &Value,
    custom_properties: &BTreeMap<String, Value>,
    stack: &mut Vec<String>,
    depth: usize,
) -> Option<Value> {
    if depth > 32 {
        return None;
    }

    match value {
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("var") => {
            resolve_var_function(arguments, custom_properties, stack, depth + 1)
        }
        Value::Function { name, arguments } => {
            let mut resolved_arguments = Vec::with_capacity(arguments.len());
            for argument in arguments {
                resolved_arguments.push(resolve_value_with_custom_properties_inner(
                    argument,
                    custom_properties,
                    stack,
                    depth + 1,
                )?);
            }
            Some(Value::Function {
                name: name.clone(),
                arguments: resolved_arguments,
            })
        }
        Value::List(values) => {
            let mut resolved_values = Vec::with_capacity(values.len());
            for item in values {
                resolved_values.push(resolve_value_with_custom_properties_inner(
                    item,
                    custom_properties,
                    stack,
                    depth + 1,
                )?);
            }
            Some(Value::List(resolved_values))
        }
        _ => Some(value.clone()),
    }
}

fn resolve_var_function(
    arguments: &[Value],
    custom_properties: &BTreeMap<String, Value>,
    stack: &mut Vec<String>,
    depth: usize,
) -> Option<Value> {
    let reference_name = custom_property_reference_name(arguments.first()?)?;
    if stack.iter().any(|name| name == reference_name) {
        return arguments.get(1).and_then(|fallback| {
            resolve_value_with_custom_properties_inner(fallback, custom_properties, stack, depth)
        });
    }

    if let Some(referenced) = custom_properties.get(reference_name) {
        stack.push(reference_name.to_string());
        let resolved =
            resolve_value_with_custom_properties_inner(referenced, custom_properties, stack, depth);
        let _ = stack.pop();
        if resolved.is_some() {
            return resolved;
        }
    }

    arguments.get(1).and_then(|fallback| {
        resolve_value_with_custom_properties_inner(fallback, custom_properties, stack, depth)
    })
}

fn custom_property_reference_name(value: &Value) -> Option<&str> {
    match value {
        Value::Keyword(name) if name.starts_with("--") => Some(name.as_str()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "style_tests.rs"]
mod style_tests;
