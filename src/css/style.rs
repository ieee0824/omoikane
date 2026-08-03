//! CSS cascade and computed style resolution.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::matcher::{
    SelectorMatchCache, matches_selector_boundary_cached, matches_selector_with_pseudo_cached,
    matches_selector_with_scope_cached,
};
use crate::dom::{Node, NodeHandle, NodeType};
use rusqlite::{Connection, params};

use super::{
    Combinator, CssToken, Declaration, MediaQuery, PseudoElement, Rule, Selector, SelectorPart,
    SimpleSelector, Specificity, Stylesheet, Value, evaluate_media_query, parse_media_query_list,
    specificity,
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

    pub(crate) fn set_paint_value(&mut self, name: &str, value: String) {
        if name.starts_with("background-position-") {
            let computed = super::parse_style_attribute(&format!("{name}: {value}"))
                .into_iter()
                .find(|declaration| declaration.name.eq_ignore_ascii_case(name))
                .map(|declaration| {
                    compute_value(&declaration.value, name, ResolutionContext::default())
                });
            if let Some(computed @ ComputedValue::CalcPxPercent(_, _)) = computed {
                self.properties.insert(name.to_string(), computed);
                return;
            }
        }
        let trimmed = value.trim();
        let computed = if let Some(number) = trimmed.strip_suffix("px") {
            number
                .parse::<f32>()
                .ok()
                .map(ComputedValue::Px)
                .unwrap_or_else(|| ComputedValue::Keyword(value.clone()))
        } else if let Some(number) = trimmed.strip_suffix('%') {
            number
                .parse::<f32>()
                .ok()
                .map(ComputedValue::Percentage)
                .unwrap_or_else(|| ComputedValue::Keyword(value.clone()))
        } else if trimmed
            .parse::<f32>()
            .ok()
            .is_some_and(|number| number.is_finite() && number == 0.0)
        {
            ComputedValue::Px(0.0)
        } else {
            ComputedValue::Keyword(value)
        };
        self.properties.insert(name.to_string(), computed);
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
    stylesheet_scopes: Vec<StylesheetScope>,
    rule_indexes: Vec<StylesheetRuleIndex>,
    cache: HashMap<usize, ComputedStyle>,
    pseudo_cache: HashMap<(usize, PseudoElement), ComputedStyle>,
    selector_match_cache: SelectorMatchCache,
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
    /// Parsed `@scope` preludes, including invalid results, keyed by source text.
    scope_prelude_cache: HashMap<String, Option<super::ScopePrelude>>,
    /// Parsed `@container` preludes, including invalid results, keyed by source text.
    container_query_cache: HashMap<String, Option<super::ContainerQuery>>,
    /// Query-container geometry and metadata from the previous layout pass.
    container_contexts: HashMap<usize, ContainerContext>,
    /// Parsed `@keyframes` rules keyed by animation name.
    keyframes: HashMap<String, Vec<KeyframeStep>>,
    /// Before/after style snapshots and running CSS transitions.
    transition_timeline: super::transition::TransitionTimeline,
    /// Node identities whose inline `style` attribute is blocked by the
    /// owning Document's CSP `style-src` policy.
    blocked_inline_style_nodes: HashSet<usize>,
}

#[derive(Debug, Clone)]
struct KeyframeStep {
    offset: f32,
    declarations: Vec<Declaration>,
}

#[derive(Debug, Clone)]
struct StylesheetScope {
    root: Option<NodeHandle>,
    implicit_scope_root: Option<NodeHandle>,
    encapsulation_order: usize,
}

/// Geometry and computed containment properties captured after a layout pass.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContainerContext {
    pub width: f32,
    pub height: f32,
    pub container_type: String,
    pub names: Vec<String>,
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

    /// Whether any loaded stylesheet contains a size container query.
    pub(crate) fn has_container_queries(&self) -> bool {
        self.stylesheets
            .iter()
            .any(|input| contains_at_rule_named(&input.stylesheet.rules, "container"))
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
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Returns the root font size used to resolve `rem` units.
    pub(crate) fn root_font_size(&self) -> f32 {
        if self.root_font_size > 0.0 {
            self.root_font_size
        } else {
            16.0
        }
    }

    /// Advances the CSS transition sampling clock without moving it backwards.
    pub(crate) fn set_transition_time_ms(&mut self, time_ms: f64) -> bool {
        if self.transition_timeline.set_time_ms(time_ms) {
            self.cache.clear();
            self.pseudo_cache.clear();
            true
        } else {
            false
        }
    }

    /// Moves transition state into a replacement resolver after stylesheet
    /// invalidation, preserving before-change values and running transitions.
    pub(crate) fn take_transition_timeline(&mut self) -> super::transition::TransitionTimeline {
        std::mem::take(&mut self.transition_timeline)
    }

    pub(crate) fn install_transition_timeline(
        &mut self,
        timeline: super::transition::TransitionTimeline,
    ) {
        self.transition_timeline = timeline;
        self.cache.clear();
        self.pseudo_cache.clear();
    }

    pub(crate) fn take_transition_events(
        &mut self,
    ) -> Vec<super::transition::TransitionEventRecord> {
        self.transition_timeline.take_events()
    }

    pub(crate) fn finish_transition_sample(&mut self, active_node_ids: &HashSet<usize>) {
        self.transition_timeline.retain_nodes(active_node_ids);
    }

    pub(crate) fn running_transition_node_ids(&self) -> Vec<usize> {
        self.transition_timeline.running_node_ids()
    }

    pub(crate) fn has_running_transitions(&self) -> bool {
        self.transition_timeline.has_running_transitions()
    }

    pub(crate) fn running_transitions_require_layout(&self) -> bool {
        self.transition_timeline
            .running_transitions_require_layout()
    }

    pub(crate) fn cancel_detached_transitions(&mut self, active_node_ids: &HashSet<usize>) {
        self.transition_timeline
            .cancel_detached_transitions(active_node_ids);
    }

    /// Drops values derived from the current DOM while retaining parsed
    /// stylesheets, rule indexes, and condition-prelude parse caches.
    pub(crate) fn invalidate_style_cache(&mut self) {
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Installs the CSP-filtered set of inline style attributes for the next
    /// computed-style pass. The attribute remains observable through CSSOM;
    /// only its cascade contribution is removed.
    pub(crate) fn set_blocked_inline_style_nodes(&mut self, nodes: HashSet<usize>) {
        if self.blocked_inline_style_nodes != nodes {
            self.blocked_inline_style_nodes = nodes;
            self.invalidate_style_cache();
        }
    }

    /// Updates one CSP-filtered inline-style entry after a `style` attribute
    /// mutation. The caller invalidates the owning document's computed-style
    /// cache separately, so this operation does not rescan or clear unrelated
    /// resolver state.
    pub(crate) fn set_blocked_inline_style_node(&mut self, node_id: usize, blocked: bool) {
        if blocked {
            self.blocked_inline_style_nodes.insert(node_id);
        } else {
            self.blocked_inline_style_nodes.remove(&node_id);
        }
    }

    #[cfg(test)]
    pub(crate) fn invalidate_style_cache_for_test(&mut self) {
        self.invalidate_style_cache();
    }

    /// Sets the viewport dimensions in px.
    ///
    /// These values are used to resolve `vw`, `vh`, `vmin`, and `vmax` units.
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport_width = width;
        self.viewport_height = height;
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
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
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Installs the query-container snapshot for the next style pass.
    pub(crate) fn set_container_contexts(
        &mut self,
        contexts: HashMap<usize, ContainerContext>,
    ) -> bool {
        if self.container_contexts == contexts {
            return false;
        }
        self.container_contexts = contexts;
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
        true
    }

    /// Adds a stylesheet with its origin.
    pub fn add_stylesheet(&mut self, origin: Origin, stylesheet: Stylesheet) {
        // Extract @keyframes rules before storing the stylesheet.
        collect_keyframes(&stylesheet.rules, &mut self.keyframes);
        self.rule_indexes
            .push(StylesheetRuleIndex::build(&stylesheet));
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.stylesheet_scopes.push(StylesheetScope {
            root: None,
            implicit_scope_root: None,
            encapsulation_order: 0,
        });
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Adds an inline stylesheet and records its owner element's parent for
    /// an omitted `@scope` start boundary.
    pub(crate) fn add_stylesheet_with_implicit_scope_root(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        implicit_scope_root: NodeHandle,
    ) {
        collect_keyframes(&stylesheet.rules, &mut self.keyframes);
        self.rule_indexes
            .push(StylesheetRuleIndex::build(&stylesheet));
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.stylesheet_scopes.push(StylesheetScope {
            root: None,
            implicit_scope_root: Some(implicit_scope_root),
            encapsulation_order: 0,
        });
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Adds an author stylesheet owned by a ShadowRoot tree scope.
    pub fn add_scoped_stylesheet(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        scope: NodeHandle,
    ) {
        let encapsulation_order = self
            .stylesheet_scopes
            .iter()
            .find(|input| input.root.as_ref() == Some(&scope))
            .map(|input| input.encapsulation_order)
            .unwrap_or_else(|| {
                self.stylesheet_scopes
                    .iter()
                    .map(|input| input.encapsulation_order)
                    .max()
                    .unwrap_or(0)
                    + 1
            });
        self.add_scoped_stylesheet_in_order(origin, stylesheet, scope, encapsulation_order);
    }

    /// Adds a ShadowRoot stylesheet with a tree-of-trees order computed by the
    /// document traversal, independent of where its first `<style>` occurs.
    pub(crate) fn add_scoped_stylesheet_in_order(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        scope: NodeHandle,
        encapsulation_order: usize,
    ) {
        self.add_scoped_stylesheet_in_order_with_implicit_scope_root(
            origin,
            stylesheet,
            scope,
            encapsulation_order,
            None,
        );
    }

    /// Adds a shadow-tree stylesheet while retaining the implicit scope root
    /// of a directly-owned `<style>` element.  A style element whose parent is
    /// the shadow root itself is implicitly scoped to the shadow host rather
    /// than to the detached ShadowRoot fragment.
    pub(crate) fn add_scoped_stylesheet_in_order_with_implicit_scope_root(
        &mut self,
        origin: Origin,
        stylesheet: Stylesheet,
        scope: NodeHandle,
        encapsulation_order: usize,
        implicit_scope_root: Option<NodeHandle>,
    ) {
        collect_keyframes(&stylesheet.rules, &mut self.keyframes);
        self.rule_indexes
            .push(StylesheetRuleIndex::build(&stylesheet));
        self.stylesheets
            .push(StylesheetInput { origin, stylesheet });
        self.stylesheet_scopes.push(StylesheetScope {
            root: Some(scope),
            implicit_scope_root,
            encapsulation_order,
        });
        self.cache.clear();
        self.pseudo_cache.clear();
        self.selector_match_cache = SelectorMatchCache::default();
    }

    /// Resolves computed style for `node`, using the cache when possible.
    pub fn computed_style(&mut self, node: &NodeHandle) -> ComputedStyle {
        let key = node.identity();
        if let Some(style) = self.cache.get(&key) {
            return style.clone();
        }

        let inheritance_parent = flattened_assigned_slot(node).or_else(|| node.parent_node());
        let inherited = inheritance_parent.map(|parent| {
            if parent.node_type() == NodeType::DocumentFragment {
                parent
                    .shadow_host()
                    .map(|host| self.computed_style(&host))
                    .unwrap_or_default()
            } else {
                self.computed_style(&parent)
            }
        });
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
            if is_root && let Some(ComputedValue::Px(px)) = style.get("font-size") {
                self.root_font_size = *px;
            }
        }

        self.cache.insert(key, style.clone());
        style
    }

    /// Resolves one computed property without cloning the complete style map
    /// when the node is already cached.  Hot hit-test paths only need a single
    /// value, such as `pointer-events`.
    pub fn computed_property(
        &mut self,
        node: &NodeHandle,
        name: &str,
    ) -> Option<ComputedValue> {
        let key = node.identity();
        if let Some(style) = self.cache.get(&key) {
            return style.get(name).cloned();
        }
        self.computed_style(node).get(name).cloned()
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
        let element_keys = ElementMatchKeys::from_node(node);

        for ((input, index), stylesheet_scope) in self
            .stylesheets
            .iter()
            .zip(&self.rule_indexes)
            .zip(&self.stylesheet_scopes)
        {
            if input.origin == Origin::Author
                && stylesheet_scope.root.is_none()
                && node.containing_shadow_root().is_some()
                && !index.has_part_selector
            {
                continue;
            }
            collect_indexed_rule_candidates(
                node,
                &input.stylesheet.rules,
                index,
                input.origin,
                pseudo,
                &mut source_order,
                &mut candidates,
                viewport_width,
                viewport_height,
                color_scheme_dark,
                &mut self.media_query_cache,
                &mut self.scope_prelude_cache,
                &mut self.container_query_cache,
                &self.container_contexts,
                element_keys.as_ref(),
                &mut self.selector_match_cache,
                stylesheet_scope.root.as_ref(),
                stylesheet_scope.implicit_scope_root.as_ref(),
                stylesheet_scope.encapsulation_order,
            );
        }

        if pseudo.is_none()
            && node.node_type() == NodeType::Element
            && !self.blocked_inline_style_nodes.contains(&node.identity())
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
                    scope_proximity: None,
                    source_order,
                    encapsulation_order: tree_scope_order(&self.stylesheet_scopes, node),
                });
                source_order += 1;
            }
        }

        candidates.sort_by(|left, right| {
            cascade_rank(left)
                .cmp(&cascade_rank(right))
                .then(encapsulation_rank(left).cmp(&encapsulation_rank(right)))
                .then(right.prefixed_alias.cmp(&left.prefixed_alias))
                .then(left.inline.cmp(&right.inline))
                .then(left.specificity.cmp(&right.specificity))
                .then_with(|| compare_scope_proximity(left, right))
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
            if is_root && let Some(ComputedValue::Px(px)) = properties.get("font-size") {
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
                    insert_computed_property(
                        &mut properties,
                        &candidate.name.to_ascii_lowercase(),
                        computed,
                    );
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
                if let Some((row_gap, column_gap)) = compute_gap_shorthand(&resolved_value, ctx) {
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
                let target = if candidate.name == "grid-row-gap" {
                    "row-gap"
                } else {
                    "column-gap"
                };
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
        resolve_writing_direction_css_wide_keywords(&mut properties, parent_style);
        resolve_non_inherited_css_wide_keywords(&mut properties);
        apply_inheritance(&mut properties, parent_style);
        apply_initial_values(&mut properties);
        normalize_background_layer_lists(&mut properties);
        properties.insert(
            "transition".to_string(),
            ComputedValue::Keyword(super::computed_transition_shorthand(&properties)),
        );
        zero_border_width_for_none_style(&mut properties);
        // CSS Animations contribute below CSS Transitions in the cascade. The
        // transition compares and samples the animation-adjusted before/after
        // values, then its active value wins for the transitioned property.
        self.apply_animation_snapshot(&mut properties, parent_style, &important_properties);
        if pseudo.is_none() {
            self.transition_timeline
                .sample(node.identity(), &mut properties);
        }

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
        let Some(steps) = self.keyframes.get(&anim_name) else {
            return;
        };

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
                let progress = ((STATIC_ANIMATION_TIME_SECONDS - delay) / duration).rem_euclid(1.0);
                steps
                    .iter()
                    .rev()
                    .find(|step| step.offset <= progress)
                    .map(|step| &step.declarations)
            }
        } else {
            None
        };
        let Some(declarations) = declarations else {
            return;
        };

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

fn contains_at_rule_named(rules: &[Rule], expected: &str) -> bool {
    rules.iter().any(|rule| match rule {
        Rule::At(at_rule) => {
            at_rule.name.eq_ignore_ascii_case(expected)
                || at_rule
                    .block
                    .as_deref()
                    .is_some_and(|block| contains_at_rule_named(block, expected))
        }
        _ => false,
    })
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
    // Logical and physical box properties participate in the same cascade.
    // Keep the logical value for CSSOM exposure, while also updating the
    // physical side consumed by the current horizontal LTR layout engine.
    // Because candidates are inserted in cascade order, a later declaration
    // in either spelling correctly wins for layout.
    if let Some(physical_name) = logical_box_property_physical_name(name) {
        properties.insert(physical_name.to_string(), computed.clone());
    }
    properties.insert(name.to_string(), computed);
}

fn logical_box_property_physical_name(name: &str) -> Option<&'static str> {
    match name {
        "padding-inline-start" => Some("padding-left"),
        "padding-inline-end" => Some("padding-right"),
        "padding-block-start" => Some("padding-top"),
        "padding-block-end" => Some("padding-bottom"),
        "margin-inline-start" => Some("margin-left"),
        "margin-inline-end" => Some("margin-right"),
        "margin-block-start" => Some("margin-top"),
        "margin-block-end" => Some("margin-bottom"),
        _ => None,
    }
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
        && let Some(valid) = enumerated_keyword_set(name)
    {
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
        "white-space" => Some(&[
            "normal",
            "pre",
            "nowrap",
            "pre-wrap",
            "pre-line",
            "break-spaces",
        ]),
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

fn is_supported_pointer_events_keyword(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case("auto")
        || value.eq_ignore_ascii_case("none")
        || value.eq_ignore_ascii_case("visiblepainted")
        || value.eq_ignore_ascii_case("visiblefill")
        || value.eq_ignore_ascii_case("visiblestroke")
        || value.eq_ignore_ascii_case("visible")
        || value.eq_ignore_ascii_case("painted")
        || value.eq_ignore_ascii_case("fill")
        || value.eq_ignore_ascii_case("stroke")
        || value.eq_ignore_ascii_case("bounding-box")
        || value.eq_ignore_ascii_case("all")
}

/// Validates a resolved declaration value against the property's grammar.
///
/// This is the single extension point for per-property value validation.
/// Properties with syntax or normalization requirements are validated here;
/// properties without a dedicated branch fall through unchanged. To add a
/// property, match its name and return [`DeclarationValidation::Valid`] /
/// [`DeclarationValidation::Invalid`].
fn validate_declaration(name: &str, value: &Value) -> DeclarationValidation {
    if let Value::CommaList(values) = value {
        let is_mask_layer_property = matches!(
            name.to_ascii_lowercase().as_str(),
            "mask-image"
                | "mask-mode"
                | "mask-composite"
                | "mask-repeat"
                | "mask-size"
                | "mask-position"
                | "mask-position-x"
                | "mask-position-y"
                | "-webkit-mask-image"
                | "-webkit-mask-mode"
                | "-webkit-mask-composite"
                | "-webkit-mask-repeat"
                | "-webkit-mask-size"
                | "-webkit-mask-position"
                | "-webkit-mask-position-x"
                | "-webkit-mask-position-y"
        );
        if values.is_empty()
            || (values.len() > crate::paint::MAX_MASK_LAYERS && is_mask_layer_property)
        {
            return DeclarationValidation::Invalid;
        }
        let mut normalized = Vec::with_capacity(values.len());
        let mut has_unvalidated = false;
        for item in values {
            match validate_declaration(name, item) {
                DeclarationValidation::Valid(value) => {
                    normalized.push(computed_value_css_text(&value));
                }
                DeclarationValidation::Invalid => return DeclarationValidation::Invalid,
                DeclarationValidation::Unvalidated => has_unvalidated = true,
            }
        }
        // Validation is per layer, but computation still needs the resolution
        // context so relative units and calc() are normalized in every layer.
        return if has_unvalidated {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Valid(ComputedValue::Keyword(normalized.join(", ")))
        };
    }
    if name.eq_ignore_ascii_case("position") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(
                        lower.as_str(),
                        "static" | "relative" | "absolute" | "fixed" | "sticky"
                    )
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("direction") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower) || matches!(lower.as_str(), "ltr" | "rtl") {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("writing-mode") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(
                        lower.as_str(),
                        "horizontal-tb"
                            | "vertical-rl"
                            | "vertical-lr"
                            | "sideways-rl"
                            | "sideways-lr"
                    )
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("unicode-bidi") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(
                        lower.as_str(),
                        "normal"
                            | "embed"
                            | "bidi-override"
                            | "isolate"
                            | "isolate-override"
                            | "plaintext"
                    )
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("transform-style") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(lower.as_str(), "flat" | "preserve-3d")
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("backface-visibility") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(lower.as_str(), "visible" | "hidden")
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("mix-blend-mode") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(
                        lower.as_str(),
                        "normal"
                            | "multiply"
                            | "screen"
                            | "overlay"
                            | "darken"
                            | "lighten"
                            | "color-dodge"
                            | "color-burn"
                            | "hard-light"
                            | "soft-light"
                            | "difference"
                            | "exclusion"
                            | "hue"
                            | "saturation"
                            | "color"
                            | "luminosity"
                            | "plus-darker"
                            | "plus-lighter"
                    )
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("isolation") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(lower.as_str(), "auto" | "isolate")
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if matches!(
        name.to_ascii_lowercase().as_str(),
        "background-origin" | "background-clip"
    ) {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(lower.as_str(), "border-box" | "padding-box" | "content-box")
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("background-image") {
        if let Value::Function { name: function, .. } = value {
            let lower = function.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "linear-gradient"
                    | "repeating-linear-gradient"
                    | "radial-gradient"
                    | "repeating-radial-gradient"
                    | "conic-gradient"
                    | "repeating-conic-gradient"
            ) {
                let rendered = render_value(value);
                return if crate::paint::parse_gradient(&rendered).is_some() {
                    DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
                } else {
                    DeclarationValidation::Invalid
                };
            }
        }
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || keyword.eq_ignore_ascii_case("none")
                    || keyword.to_ascii_lowercase().starts_with("url(") =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("mask-image") {
        if let Value::Function { name: function, .. } = value {
            let lower = function.to_ascii_lowercase();
            if lower == "linear-gradient"
                || lower == "repeating-linear-gradient"
                || lower == "radial-gradient"
                || lower == "repeating-radial-gradient"
                || lower == "conic-gradient"
                || lower == "repeating-conic-gradient"
            {
                let rendered = render_value(value);
                return if crate::paint::color::parse_gradient(&rendered).is_some() {
                    DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
                } else {
                    DeclarationValidation::Invalid
                };
            }
        }
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || keyword.eq_ignore_ascii_case("none")
                    || keyword.to_ascii_lowercase().starts_with("url(") =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("mask-mode") {
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "alpha" | "luminance" | "match-source"
                    ) =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("mask-composite") {
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "add" | "subtract" | "intersect" | "exclude"
                    ) =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("clip-path") {
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || keyword.eq_ignore_ascii_case("none") =>
            {
                DeclarationValidation::Unvalidated
            }
            Value::Function {
                name: function,
                ..
            } if matches!(
                function.to_ascii_lowercase().as_str(),
                "inset" | "circle" | "ellipse" | "polygon"
            ) => {
                let rendered = render_value(value);
                if crate::paint::is_valid_clip_path_value(&rendered) {
                    DeclarationValidation::Unvalidated
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("mask-repeat") {
        let valid_axis = |value: &Value| {
            matches!(
                value,
                Value::Keyword(keyword)
                    if matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "repeat" | "no-repeat"
                    )
            )
        };
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "repeat" | "no-repeat" | "repeat-x" | "repeat-y"
                    ) =>
            {
                DeclarationValidation::Unvalidated
            }
            Value::List(values)
                if (1..=2).contains(&values.len()) && values.iter().all(valid_axis) =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("background-repeat") {
        let valid_axis = |value: &Value| matches!(value, Value::Keyword(keyword) if matches!(keyword.to_ascii_lowercase().as_str(), "repeat" | "no-repeat"));
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "repeat" | "no-repeat" | "repeat-x" | "repeat-y"
                    ) =>
            {
                DeclarationValidation::Unvalidated
            }
            Value::List(values)
                if (1..=2).contains(&values.len()) && values.iter().all(valid_axis) =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("background-attachment") {
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "scroll" | "fixed" | "local"
                    ) =>
            {
                DeclarationValidation::Unvalidated
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("cursor") {
        return match compute_cursor_value(value) {
            Some(computed) => DeclarationValidation::Valid(computed),
            None => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("aspect-ratio") {
        // Normalizing needs the resolution context for `calc()`, so a valid
        // value goes on to `compute_value` (see `render_aspect_ratio_value`).
        let is_css_wide = matches!(
            value,
            Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase())
        );
        return if is_css_wide || aspect_ratio_parts(value).is_some() {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        };
    }
    if name.eq_ignore_ascii_case("object-fit") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower)
                    || matches!(
                        lower.as_str(),
                        "fill" | "contain" | "cover" | "none" | "scale-down"
                    )
                {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("pointer-events") {
        return match value {
            Value::Keyword(keyword) => {
                let lower = keyword.to_ascii_lowercase();
                if is_css_wide_keyword(&lower) || is_supported_pointer_events_keyword(&lower) {
                    DeclarationValidation::Valid(ComputedValue::Keyword(lower))
                } else {
                    DeclarationValidation::Invalid
                }
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("object-position") {
        // The grammar is checked here, but normalizing keywords to percentages
        // and lengths to pixels needs the resolution context, so the value goes
        // through `compute_value` (see `render_object_position_value`). A
        // CSS-wide keyword is handled by the cascade, not by this grammar.
        let is_css_wide = matches!(
            value,
            Value::Keyword(keyword) if is_css_wide_keyword(&keyword.to_ascii_lowercase())
        );
        return if is_css_wide || object_position_components(value).is_some() {
            DeclarationValidation::Unvalidated
        } else {
            DeclarationValidation::Invalid
        };
    }
    if is_non_negative_sizing_property(name) {
        return validate_sizing_value(name, value);
    }
    if name.eq_ignore_ascii_case("container-type") {
        return match value {
            Value::Keyword(keyword)
                if is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "normal" | "inline-size" | "size"
                    ) =>
            {
                DeclarationValidation::Valid(ComputedValue::Keyword(keyword.to_ascii_lowercase()))
            }
            _ => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("container-name") {
        let valid_custom_name = |keyword: &str| {
            let lower = keyword.to_ascii_lowercase();
            !is_css_wide_keyword(&lower)
                && !matches!(lower.as_str(), "none" | "and" | "or" | "not" | "default")
        };
        let valid = match value {
            Value::Keyword(keyword) => {
                keyword.eq_ignore_ascii_case("none")
                    || is_css_wide_keyword(&keyword.to_ascii_lowercase())
                    || valid_custom_name(keyword)
            }
            Value::List(values) => !values.is_empty()
                && values.iter().all(
                    |value| matches!(value, Value::Keyword(keyword) if valid_custom_name(keyword)),
                ),
            _ => false,
        };
        return if valid {
            DeclarationValidation::Valid(ComputedValue::Keyword(render_value(value)))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if name.eq_ignore_ascii_case("transform") {
        let rendered = render_value(value);
        let reference = super::TransformReferenceBox {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            font_size: 16.0,
            root_font_size: 16.0,
        };
        return if super::parse_transform_list(&rendered, reference).is_some() {
            DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if name.eq_ignore_ascii_case("perspective") {
        let rendered = render_value(value);
        let lower = rendered.to_ascii_lowercase();
        if is_css_wide_keyword(&lower) || lower == "none" {
            return DeclarationValidation::Valid(ComputedValue::Keyword(lower));
        }
        let reference = super::TransformReferenceBox {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            font_size: 16.0,
            root_font_size: 16.0,
        };
        return if super::parse_perspective_with_origin(&rendered, "50% 50%", reference).is_some() {
            DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if name.eq_ignore_ascii_case("filter") || name.eq_ignore_ascii_case("backdrop-filter") {
        let rendered = render_value(value);
        if is_css_wide_keyword(&rendered.to_ascii_lowercase()) {
            return DeclarationValidation::Valid(ComputedValue::Keyword(
                rendered.to_ascii_lowercase(),
            ));
        }
        return match super::normalize_filter_list(&rendered) {
            Some(normalized) => DeclarationValidation::Valid(ComputedValue::Keyword(normalized)),
            None => DeclarationValidation::Invalid,
        };
    }
    if name.eq_ignore_ascii_case("transform-origin") {
        let rendered = render_value(value);
        let reference = super::TransformReferenceBox {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            font_size: 16.0,
            root_font_size: 16.0,
        };
        return if super::parse_transform_with_origin("scale(2)", &rendered, reference).is_some() {
            DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if name.eq_ignore_ascii_case("perspective-origin") {
        let rendered = render_value(value);
        let reference = super::TransformReferenceBox {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            font_size: 16.0,
            root_font_size: 16.0,
        };
        return if super::parse_perspective_origin(&rendered, reference).is_some() {
            DeclarationValidation::Valid(ComputedValue::Keyword(rendered))
        } else {
            DeclarationValidation::Invalid
        };
    }
    if matches!(
        name,
        "transition-property"
            | "transition-duration"
            | "transition-timing-function"
            | "transition-delay"
    ) {
        let rendered = render_value(value);
        return match super::computed_transition_longhand(name, &rendered) {
            Some(normalized) => DeclarationValidation::Valid(ComputedValue::Keyword(normalized)),
            None => DeclarationValidation::Invalid,
        };
    }
    DeclarationValidation::Unvalidated
}

fn validate_sizing_value(name: &str, value: &Value) -> DeclarationValidation {
    let valid_keyword = |keyword: &str| {
        let keyword = keyword.to_ascii_lowercase();
        is_css_wide_keyword(&keyword)
            || matches!(
                keyword.as_str(),
                "auto" | "min-content" | "max-content" | "fit-content" | "stretch"
            )
            || (name.starts_with("max-") && keyword == "none")
    };
    match value {
        Value::Keyword(keyword) if valid_keyword(keyword) => DeclarationValidation::Unvalidated,
        Value::Length(number, unit)
            if *number >= 0.0
                && resolve_length_to_px(*number, unit, ResolutionContext::default()).is_some() =>
        {
            DeclarationValidation::Unvalidated
        }
        Value::Percentage(number) if *number >= 0.0 => DeclarationValidation::Unvalidated,
        Value::Number(number) if *number == 0.0 => DeclarationValidation::Unvalidated,
        Value::Function { name: function, .. }
            if function.eq_ignore_ascii_case("calc") || function.eq_ignore_ascii_case("clamp") =>
        {
            let computed = compute_value(value, name, ResolutionContext::default());
            match computed {
                ComputedValue::Px(_)
                | ComputedValue::Percentage(_)
                | ComputedValue::CalcPxPercent(_, _) => DeclarationValidation::Unvalidated,
                ComputedValue::Number(number) if number == 0.0 => {
                    DeclarationValidation::Unvalidated
                }
                _ => DeclarationValidation::Invalid,
            }
        }
        _ => DeclarationValidation::Invalid,
    }
}

fn is_non_negative_sizing_property(name: &str) -> bool {
    matches!(
        name,
        "width" | "height" | "min-width" | "min-height" | "max-width" | "max-height"
    )
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
    /// Ancestor hops from the styled element to the applicable scoping root.
    scope_proximity: Option<usize>,
    source_order: usize,
    /// Position of this declaration's tree scope in tree-of-trees order.
    /// Encapsulation order reverses for important declarations.
    encapsulation_order: usize,
}

#[derive(Debug, Clone)]
struct ActiveScope {
    roots: Vec<ScopeRoot>,
}

#[derive(Debug, Clone)]
struct ScopeRoot {
    node: NodeHandle,
    proximity: usize,
}

struct ElementMatchKeys {
    id: Option<String>,
    classes: HashSet<String>,
    tag_name: String,
}

#[derive(Debug, Default)]
struct StylesheetRuleIndex {
    by_id: HashMap<String, Vec<usize>>,
    by_class: HashMap<String, Vec<usize>>,
    by_tag: HashMap<String, Vec<usize>>,
    fallback: Vec<usize>,
    declaration_offsets: Vec<usize>,
    total_declarations: usize,
    has_part_selector: bool,
}

impl StylesheetRuleIndex {
    fn build(stylesheet: &Stylesheet) -> Self {
        let mut index = Self::default();
        let mut offset = 0;
        for (rule_index, rule) in stylesheet.rules.iter().enumerate() {
            index.declaration_offsets.push(offset);
            offset += match rule {
                Rule::Style(rule) => {
                    index.has_part_selector |= rule.selectors.iter().any(selector_uses_part_pseudo);
                    rule.declarations.len()
                }
                Rule::At(rule) => {
                    if let Some(block) = rule.block.as_deref() {
                        index.has_part_selector |= rules_contain_part_selector(block);
                        count_declarations(block)
                    } else {
                        rule.declarations.len()
                    }
                }
                Rule::FontFace(_) => 0,
            };
            let Rule::Style(style_rule) = rule else {
                continue;
            };
            let keys: Option<Vec<RuleMatchKey>> = style_rule
                .selectors
                .iter()
                .map(selector_match_key)
                .collect();
            let Some(keys) = keys else {
                index.fallback.push(rule_index);
                continue;
            };
            for key in keys {
                let bucket = match key {
                    RuleMatchKey::Id(value) => index.by_id.entry(value).or_default(),
                    RuleMatchKey::Class(value) => index.by_class.entry(value).or_default(),
                    RuleMatchKey::Tag(value) => index.by_tag.entry(value).or_default(),
                };
                if bucket.last() != Some(&rule_index) {
                    bucket.push(rule_index);
                }
            }
        }
        index.total_declarations = offset;
        index
    }

    fn candidates(&self, keys: &ElementMatchKeys) -> BTreeSet<usize> {
        let mut candidates = self.fallback.iter().copied().collect::<BTreeSet<_>>();
        if let Some(id) = &keys.id
            && let Some(rules) = self.by_id.get(id)
        {
            candidates.extend(rules);
        }
        for class in &keys.classes {
            if let Some(rules) = self.by_class.get(class) {
                candidates.extend(rules);
            }
        }
        if let Some(rules) = self.by_tag.get(&keys.tag_name.to_ascii_lowercase()) {
            candidates.extend(rules);
        }
        candidates
    }
}

enum RuleMatchKey {
    Id(String),
    Class(String),
    Tag(String),
}

fn selector_match_key(selector: &super::Selector) -> Option<RuleMatchKey> {
    if selector_uses_shadow_pseudo(selector) {
        return None;
    }
    let rightmost = selector.parts.last()?;
    rightmost
        .simples
        .iter()
        .find_map(|simple| match simple {
            SimpleSelector::Id(value) => Some(RuleMatchKey::Id(value.clone())),
            _ => None,
        })
        .or_else(|| {
            rightmost.simples.iter().find_map(|simple| match simple {
                SimpleSelector::Class(value) => Some(RuleMatchKey::Class(value.clone())),
                _ => None,
            })
        })
        .or_else(|| {
            rightmost.simples.iter().find_map(|simple| match simple {
                SimpleSelector::Type(value) => Some(RuleMatchKey::Tag(value.to_ascii_lowercase())),
                _ => None,
            })
        })
}

impl ElementMatchKeys {
    fn from_node(node: &NodeHandle) -> Option<Self> {
        Some(Self {
            id: node.get_attribute("id"),
            classes: node
                .get_attribute("class")
                .map(|value| {
                    value
                        .split_ascii_whitespace()
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            tag_name: node.tag_name()?,
        })
    }
}

fn style_rule_might_match(style_rule: &super::StyleRule, keys: &ElementMatchKeys) -> bool {
    style_rule.selectors.iter().any(|selector| {
        if selector_uses_shadow_pseudo(selector) {
            return true;
        }
        let Some(rightmost) = selector.parts.last() else {
            return false;
        };
        rightmost.simples.iter().all(|simple| match simple {
            SimpleSelector::Id(id) => keys.id.as_deref() == Some(id.as_str()),
            SimpleSelector::Class(class) => keys.classes.contains(class),
            SimpleSelector::Type(tag) => tag.eq_ignore_ascii_case(&keys.tag_name),
            _ => true,
        })
    })
}

fn selector_uses_shadow_pseudo(selector: &super::Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::PseudoClass(name) => {
                name.eq_ignore_ascii_case("host")
                    || functional_selector(name)
                        .is_some_and(|(function, _)| function.eq_ignore_ascii_case("host"))
            }
            SimpleSelector::PseudoElement(name) => {
                functional_selector(name).is_some_and(|(function, _)| {
                    function.eq_ignore_ascii_case("slotted")
                        || function.eq_ignore_ascii_case("part")
                })
            }
            _ => false,
        })
    })
}

fn rules_contain_part_selector(rules: &[Rule]) -> bool {
    rules.iter().any(|rule| match rule {
        Rule::Style(rule) => rule.selectors.iter().any(selector_uses_part_pseudo),
        Rule::At(rule) => rule
            .block
            .as_deref()
            .is_some_and(rules_contain_part_selector),
        Rule::FontFace(_) => false,
    })
}

fn selector_uses_part_pseudo(selector: &Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| {
            matches!(simple, SimpleSelector::PseudoElement(name) if functional_selector(name).is_some_and(|(function, _)| function.eq_ignore_ascii_case("part")))
        })
    })
}

fn functional_selector(name: &str) -> Option<(&str, &str)> {
    let open = name.find('(')?;
    Some((&name[..open], name[open + 1..].strip_suffix(')')?.trim()))
}

fn matches_shadow_scoped_selector(
    node: &NodeHandle,
    selector: &super::Selector,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    if selector_uses_part_pseudo(selector) {
        return pseudo.is_none() && matches_part_selector(node, selector, Some(scope), cache);
    }
    if selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| {
            matches!(simple, SimpleSelector::PseudoClass(name) if name.eq_ignore_ascii_case("host") || functional_selector(name).is_some_and(|(function, _)| function.eq_ignore_ascii_case("host")))
        })
    }) {
        return matches_host_selector(node, selector, scope, pseudo, cache);
    }

    if selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| {
            matches!(simple, SimpleSelector::PseudoElement(name) if functional_selector(name).is_some_and(|(function, _)| function.eq_ignore_ascii_case("slotted")))
        })
    }) {
        return pseudo.is_none() && matches_slotted_selector(node, selector, scope, cache);
    }

    node.containing_shadow_root().as_ref() == Some(scope)
        && matches_selector_with_pseudo_cached(node, selector, pseudo, cache)
}

fn matches_host_selector(
    node: &NodeHandle,
    selector: &super::Selector,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    if selector.parts.is_empty() {
        return false;
    }
    matches_host_selector_part(
        node,
        selector,
        selector.parts.len() - 1,
        scope,
        pseudo,
        cache,
    )
}

fn matches_host_selector_part(
    node: &NodeHandle,
    selector: &Selector,
    index: usize,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    let part = &selector.parts[index];
    if !matches_shadow_compound(node, part, scope, pseudo, cache) {
        return false;
    }

    let Some(combinator) = part.combinator else {
        return true;
    };
    if index == 0 {
        return false;
    }

    match combinator {
        Combinator::Descendant => {
            let mut ancestor = shadow_selector_parent(node, scope);
            while let Some(parent) = ancestor {
                if matches_host_selector_part(&parent, selector, index - 1, scope, None, cache) {
                    return true;
                }
                ancestor = shadow_selector_parent(&parent, scope);
            }
            false
        }
        Combinator::Child => shadow_selector_parent(node, scope).is_some_and(|parent| {
            matches_host_selector_part(&parent, selector, index - 1, scope, None, cache)
        }),
        Combinator::AdjacentSibling => previous_element_sibling(node).is_some_and(|sibling| {
            matches_host_selector_part(&sibling, selector, index - 1, scope, None, cache)
        }),
        Combinator::GeneralSibling => {
            let Some(parent) = node.parent_node() else {
                return false;
            };
            let siblings = parent.child_nodes();
            let Some(position) = siblings.iter().position(|candidate| candidate == node) else {
                return false;
            };
            siblings[..position].iter().rev().any(|sibling| {
                sibling.node_type() == NodeType::Element
                    && matches_host_selector_part(sibling, selector, index - 1, scope, None, cache)
            })
        }
    }
}

fn matches_shadow_compound(
    node: &NodeHandle,
    part: &SelectorPart,
    scope: &NodeHandle,
    pseudo: Option<PseudoElement>,
    cache: &mut SelectorMatchCache,
) -> bool {
    let mut ordinary_part = part.clone();
    ordinary_part.combinator = None;
    let mut has_host = false;
    ordinary_part.simples.retain(|simple| {
        let SimpleSelector::PseudoClass(name) = simple else {
            return true;
        };
        let is_host = name.eq_ignore_ascii_case("host")
            || functional_selector(name)
                .is_some_and(|(function, _)| function.eq_ignore_ascii_case("host"));
        has_host |= is_host;
        !is_host
    });

    if has_host {
        let Some(host) = scope.shadow_host() else {
            return false;
        };
        if node != &host || !host_arguments_match(&host, part, cache) {
            return false;
        }
    } else if node.containing_shadow_root().as_ref() != Some(scope) {
        return false;
    }

    if ordinary_part.simples.is_empty() {
        return pseudo.is_none();
    }
    matches_selector_with_pseudo_cached(
        node,
        &Selector {
            parts: vec![ordinary_part],
        },
        pseudo,
        cache,
    )
}

fn host_arguments_match(
    host: &NodeHandle,
    part: &SelectorPart,
    cache: &mut SelectorMatchCache,
) -> bool {
    part.simples.iter().all(|simple| {
        let SimpleSelector::PseudoClass(name) = simple else {
            return true;
        };
        if name.eq_ignore_ascii_case("host") {
            return true;
        }
        let Some((function, argument)) = functional_selector(name) else {
            return true;
        };
        if !function.eq_ignore_ascii_case("host") {
            return true;
        }
        super::parse_selector_list(argument)
            .ok()
            .is_some_and(|selectors| {
                selectors.len() == 1
                    && selectors[0].parts.len() == 1
                    && matches_selector_with_pseudo_cached(host, &selectors[0], None, cache)
            })
    })
}

fn shadow_selector_parent(node: &NodeHandle, scope: &NodeHandle) -> Option<NodeHandle> {
    let parent = node.parent_node()?;
    if &parent == scope {
        scope.shadow_host()
    } else {
        Some(parent)
    }
}

fn previous_element_sibling(node: &NodeHandle) -> Option<NodeHandle> {
    let parent = node.parent_node()?;
    let siblings = parent.child_nodes();
    let position = siblings.iter().position(|candidate| candidate == node)?;
    siblings[..position]
        .iter()
        .rev()
        .find(|sibling| sibling.node_type() == NodeType::Element)
        .cloned()
}

fn matches_slotted_selector(
    node: &NodeHandle,
    selector: &super::Selector,
    scope: &NodeHandle,
    cache: &mut SelectorMatchCache,
) -> bool {
    let Some(slot) = assigned_slot_in_scope(node, scope) else {
        return false;
    };

    let mut slot_selector = selector.clone();
    let mut argument = None;
    for part in &mut slot_selector.parts {
        part.simples.retain(|simple| {
            let SimpleSelector::PseudoElement(name) = simple else {
                return true;
            };
            let Some((function, candidate)) = functional_selector(name) else {
                return true;
            };
            if !function.eq_ignore_ascii_case("slotted") || argument.is_some() {
                return true;
            }
            argument = Some(candidate.to_string());
            false
        });
    }
    let Some(argument) = argument else {
        return false;
    };
    let argument_matches = super::parse_selector_list(&argument)
        .ok()
        .is_some_and(|selectors| {
            selectors.len() == 1
                && selectors[0].parts.len() == 1
                && matches_selector_with_pseudo_cached(node, &selectors[0], None, cache)
        });
    if !argument_matches {
        return false;
    }

    slot_selector.parts.retain(|part| !part.simples.is_empty());
    if slot_selector.parts.is_empty() {
        true
    } else {
        slot_selector.parts[0].combinator = None;
        matches_selector_with_pseudo_cached(&slot, &slot_selector, None, cache)
    }
}

fn assigned_slot_in_scope(node: &NodeHandle, scope: &NodeHandle) -> Option<NodeHandle> {
    let mut current = node.clone();
    while let Some(slot) = current.assigned_slot() {
        if slot.containing_shadow_root().as_ref() == Some(scope) {
            return Some(slot);
        }
        current = slot;
    }
    None
}

fn flattened_assigned_slot(node: &NodeHandle) -> Option<NodeHandle> {
    let mut current = node.clone();
    let mut outermost = None;
    while let Some(slot) = current.assigned_slot() {
        current = slot.clone();
        outermost = Some(slot);
    }
    outermost
}

#[allow(clippy::too_many_arguments)]
fn collect_indexed_rule_candidates(
    node: &NodeHandle,
    rules: &[Rule],
    index: &StylesheetRuleIndex,
    origin: Origin,
    pseudo: Option<PseudoElement>,
    source_order: &mut usize,
    out: &mut Vec<Candidate>,
    viewport_width: f32,
    viewport_height: f32,
    color_scheme_dark: bool,
    media_cache: &mut HashMap<String, Vec<MediaQuery>>,
    scope_cache: &mut HashMap<String, Option<super::ScopePrelude>>,
    container_cache: &mut HashMap<String, Option<super::ContainerQuery>>,
    container_contexts: &HashMap<usize, ContainerContext>,
    element_keys: Option<&ElementMatchKeys>,
    selector_cache: &mut SelectorMatchCache,
    shadow_scope: Option<&NodeHandle>,
    implicit_scope_root: Option<&NodeHandle>,
    encapsulation_order: usize,
) {
    let Some(element_keys) = element_keys else {
        collect_rule_candidates(
            node,
            rules,
            origin,
            pseudo,
            source_order,
            out,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_cache,
            scope_cache,
            container_cache,
            container_contexts,
            None,
            selector_cache,
            shadow_scope,
            implicit_scope_root,
            encapsulation_order,
            None,
        );
        return;
    };

    let base_order = *source_order;
    for rule_index in index.candidates(element_keys) {
        let mut rule_order = base_order + index.declaration_offsets[rule_index];
        collect_rule_candidates(
            node,
            &rules[rule_index..=rule_index],
            origin,
            pseudo,
            &mut rule_order,
            out,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_cache,
            scope_cache,
            container_cache,
            container_contexts,
            Some(element_keys),
            selector_cache,
            shadow_scope,
            implicit_scope_root,
            encapsulation_order,
            None,
        );
    }
    for (rule_index, rule) in rules.iter().enumerate() {
        if !matches!(rule, Rule::At(_)) {
            continue;
        }
        let mut rule_order = base_order + index.declaration_offsets[rule_index];
        collect_rule_candidates(
            node,
            &rules[rule_index..=rule_index],
            origin,
            pseudo,
            &mut rule_order,
            out,
            viewport_width,
            viewport_height,
            color_scheme_dark,
            media_cache,
            scope_cache,
            container_cache,
            container_contexts,
            Some(element_keys),
            selector_cache,
            shadow_scope,
            implicit_scope_root,
            encapsulation_order,
            None,
        );
    }
    *source_order += index.total_declarations;
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
    scope_cache: &mut HashMap<String, Option<super::ScopePrelude>>,
    container_cache: &mut HashMap<String, Option<super::ContainerQuery>>,
    container_contexts: &HashMap<usize, ContainerContext>,
    element_keys: Option<&ElementMatchKeys>,
    selector_cache: &mut SelectorMatchCache,
    shadow_scope: Option<&NodeHandle>,
    implicit_scope_root: Option<&NodeHandle>,
    encapsulation_order: usize,
    active_scope: Option<&ActiveScope>,
) {
    if node.node_type() != NodeType::Element {
        return;
    }

    for rule in rules {
        match rule {
            Rule::Style(style_rule) => {
                if element_keys.is_some_and(|keys| !style_rule_might_match(style_rule, keys)) {
                    *source_order += style_rule.declarations.len();
                    continue;
                }
                let mut matching = None;
                for selector in &style_rule.selectors {
                    let selector_specificity = specificity(selector);
                    if let Some(active) = active_scope {
                        for root in &active.roots {
                            if root.node == *node && !selector_references_scope(selector) {
                                continue;
                            }
                            if matches_selector_with_scope_cached(
                                node,
                                selector,
                                pseudo,
                                selector_cache,
                                Some(&root.node),
                            ) {
                                retain_best_scoped_match(
                                    &mut matching,
                                    selector_specificity,
                                    root.proximity,
                                );
                            }
                        }
                    } else {
                        let matches = if origin == Origin::Author
                            && shadow_scope.is_none()
                            && node.containing_shadow_root().is_some()
                        {
                            pseudo.is_none()
                                && selector_uses_part_pseudo(selector)
                                && matches_part_selector(node, selector, None, selector_cache)
                        } else if let Some(scope) = shadow_scope {
                            matches_shadow_scoped_selector(
                                node,
                                selector,
                                scope,
                                pseudo,
                                selector_cache,
                            )
                        } else {
                            matches_selector_with_pseudo_cached(
                                node,
                                selector,
                                pseudo,
                                selector_cache,
                            )
                        };
                        if matches {
                            retain_best_scoped_match(
                                &mut matching,
                                selector_specificity,
                                usize::MAX,
                            );
                        }
                    }
                }

                if let Some((specificity, proximity)) = matching {
                    for declaration in &style_rule.declarations {
                        out.push(Candidate {
                            name: canonical_property_name(&declaration.name).to_string(),
                            prefixed_alias: is_prefixed_property_alias(&declaration.name),
                            value: declaration.value.clone(),
                            important: declaration.important,
                            origin,
                            inline: false,
                            specificity,
                            scope_proximity: active_scope.map(|_| proximity),
                            source_order: *source_order,
                            encapsulation_order,
                        });
                        *source_order += 1;
                    }
                } else {
                    *source_order += style_rule.declarations.len();
                }
            }
            Rule::At(at_rule) => {
                if let Some(block) = &at_rule.block {
                    if at_rule.name.eq_ignore_ascii_case("scope") {
                        let prelude = scope_cache
                            .entry(at_rule.prelude.clone())
                            .or_insert_with(|| super::parse_scope_prelude(&at_rule.prelude));
                        let Some(prelude) = prelude.as_ref() else {
                            *source_order += count_declarations(block);
                            continue;
                        };
                        let Some(scope) = applicable_scope(
                            node,
                            &prelude,
                            active_scope,
                            implicit_scope_root,
                            selector_cache,
                        ) else {
                            *source_order += count_declarations(block);
                            continue;
                        };
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
                            scope_cache,
                            container_cache,
                            container_contexts,
                            element_keys,
                            selector_cache,
                            shadow_scope,
                            implicit_scope_root,
                            encapsulation_order,
                            Some(&scope),
                        );
                        continue;
                    }
                    // Evaluate @media queries before descending into the block.
                    let should_apply = if at_rule.name == "media" {
                        media_query_matches(
                            &at_rule.prelude,
                            viewport_width,
                            viewport_height,
                            color_scheme_dark,
                            media_cache,
                        )
                    } else if at_rule.name.eq_ignore_ascii_case("supports") {
                        super::supports_condition_matches(&at_rule.prelude)
                    } else if at_rule.name.eq_ignore_ascii_case("container") {
                        container_query_matches(
                            node,
                            &at_rule.prelude,
                            container_cache,
                            container_contexts,
                        )
                    } else if at_rule.name.eq_ignore_ascii_case("keyframes")
                        || at_rule.name.eq_ignore_ascii_case("-webkit-keyframes")
                    {
                        // @keyframes rules are handled separately; skip them in cascade.
                        false
                    } else {
                        // Non-conditional grouping rules (e.g. @layer) pass through.
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
                            scope_cache,
                            container_cache,
                            container_contexts,
                            element_keys,
                            selector_cache,
                            shadow_scope,
                            implicit_scope_root,
                            encapsulation_order,
                            active_scope,
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

/// Matches an outer-tree `::part()` selector against a shadow-tree element.
///
/// Each entry in the exposure chain pairs the host visible in a tree scope
/// with the names exported into that scope. A nested host only forwards names
/// listed by its own `exportparts` attribute, so ordinary document selectors
/// can never pierce an unexported shadow boundary.
fn matches_part_selector(
    node: &NodeHandle,
    selector: &Selector,
    stylesheet_scope: Option<&NodeHandle>,
    cache: &mut SelectorMatchCache,
) -> bool {
    let Some((part_name, host_selector)) = part_selector_components(selector) else {
        return false;
    };
    let Some(mut root) = node.containing_shadow_root() else {
        return false;
    };
    let mut exposed_names = HashSet::new();
    if let Some(value) = node.get_attribute("part") {
        exposed_names.extend(value.split_ascii_whitespace().map(str::to_string));
    }
    if exposed_names.is_empty() {
        return false;
    }

    loop {
        let Some(host) = root.shadow_host() else {
            return false;
        };
        let visible_in_stylesheet_scope = match stylesheet_scope {
            Some(scope) => &root == scope || host.containing_shadow_root().as_ref() == Some(scope),
            None => host.containing_shadow_root().is_none(),
        };
        if visible_in_stylesheet_scope && exposed_names.contains(&part_name) {
            let host_matches = if let Some(scope) = stylesheet_scope {
                matches_shadow_scoped_selector(&host, &host_selector, scope, None, cache)
            } else {
                matches_selector_with_pseudo_cached(&host, &host_selector, None, cache)
            };
            if host_matches {
                return true;
            }
        }

        let Some(outer_root) = host.containing_shadow_root() else {
            return false;
        };
        exposed_names = forwarded_part_names(&host, &exposed_names);
        if exposed_names.is_empty() {
            return false;
        }
        root = outer_root;
    }
}

fn part_selector_components(selector: &Selector) -> Option<(String, Selector)> {
    let mut host_selector = selector.clone();
    let mut part_name = None;
    for (part_index, part) in host_selector.parts.iter_mut().enumerate() {
        part.simples.retain(|simple| {
            let SimpleSelector::PseudoElement(name) = simple else {
                return true;
            };
            let Some((function, argument)) = functional_selector(name) else {
                return true;
            };
            if !function.eq_ignore_ascii_case("part") {
                return true;
            }
            if part_index + 1 != selector.parts.len() || part_name.is_some() {
                return true;
            }
            part_name = Some(argument.to_string());
            false
        });
    }
    let part_name = part_name?;
    if host_selector.parts.last()?.simples.is_empty() {
        host_selector
            .parts
            .last_mut()?
            .simples
            .push(SimpleSelector::Universal);
    }
    Some((part_name, host_selector))
}

fn forwarded_part_names(host: &NodeHandle, inner_names: &HashSet<String>) -> HashSet<String> {
    let Some(mapping) = host.get_attribute("exportparts") else {
        return HashSet::new();
    };
    mapping
        .split(',')
        .filter_map(parse_exportparts_entry)
        .filter_map(|(inner, outer)| inner_names.contains(&inner).then_some(outer))
        .collect()
}

fn parse_exportparts_entry(entry: &str) -> Option<(String, String)> {
    let tokens: Vec<CssToken> = super::tokenize(entry)
        .ok()?
        .into_iter()
        .filter(|token| *token != CssToken::Whitespace)
        .collect();
    match tokens.as_slice() {
        [CssToken::Ident(name)] => Some((name.clone(), name.clone())),
        [
            CssToken::Ident(inner),
            CssToken::Colon,
            CssToken::Ident(outer),
        ] => Some((inner.clone(), outer.clone())),
        _ => None,
    }
}

fn applicable_scope(
    node: &NodeHandle,
    prelude: &super::ScopePrelude,
    outer: Option<&ActiveScope>,
    implicit_scope_root: Option<&NodeHandle>,
    selector_cache: &mut SelectorMatchCache,
) -> Option<ActiveScope> {
    let mut ancestors = Vec::new();
    let mut current = Some(node.clone());
    while let Some(candidate) = current {
        if candidate.node_type() == NodeType::Element {
            ancestors.push(candidate.clone());
        }
        current = candidate.parent_node();
    }
    if ancestors.is_empty() {
        return None;
    }

    let ambient_roots: Vec<NodeHandle> = if let Some(outer) = outer {
        outer.roots.iter().map(|root| root.node.clone()).collect()
    } else {
        vec![ancestors.last()?.clone()]
    };

    let mut roots = Vec::new();
    for ambient_root in ambient_roots {
        let Some(ambient_proximity) = ancestors
            .iter()
            .position(|candidate| candidate == &ambient_root)
        else {
            continue;
        };
        let candidates: Vec<(usize, NodeHandle)> = if let Some(start) = &prelude.start {
            ancestors
                .iter()
                .take(ambient_proximity + 1)
                .enumerate()
                .filter(|(_, candidate)| {
                    start.iter().any(|selector| {
                        matches_selector_boundary_cached(
                            candidate,
                            selector,
                            None,
                            selector_cache,
                            Some(&ambient_root),
                        )
                    })
                })
                .map(|(proximity, root)| (proximity, root.clone()))
                .collect()
        } else if let Some(implicit_root) = implicit_scope_root {
            ancestors
                .iter()
                .take(ambient_proximity + 1)
                .position(|candidate| candidate == implicit_root)
                .map(|proximity| vec![(proximity, implicit_root.clone())])
                .or_else(|| {
                    // A style directly in a shadow tree is implicitly scoped
                    // to its host, which is not represented as a parent of
                    // nodes in the separate shadow-tree fragment.
                    let in_shadow_tree = node
                        .containing_shadow_root()
                        .and_then(|root| root.shadow_host())
                        .is_some_and(|host| host == *implicit_root);
                    in_shadow_tree.then_some(vec![(ancestors.len(), implicit_root.clone())])
                })
                .unwrap_or_default()
        } else {
            vec![(ambient_proximity, ambient_root)]
        };

        for (proximity, root) in candidates {
            let excluded_by_limit = prelude.end.as_ref().is_some_and(|limits| {
                ancestors.iter().take(proximity + 1).any(|candidate| {
                    limits.iter().any(|selector| {
                        matches_selector_boundary_cached(
                            candidate,
                            selector,
                            None,
                            selector_cache,
                            Some(&root),
                        )
                    })
                })
            });
            if !excluded_by_limit
                && !roots
                    .iter()
                    .any(|existing: &ScopeRoot| existing.node == root)
            {
                roots.push(ScopeRoot {
                    node: root,
                    proximity,
                });
            }
        }
    }
    (!roots.is_empty()).then_some(ActiveScope { roots })
}

fn selector_references_scope(selector: &Selector) -> bool {
    selector.parts.iter().any(|part| {
        part.simples.iter().any(|simple| match simple {
            SimpleSelector::PseudoClass(name) => name.eq_ignore_ascii_case("scope"),
            SimpleSelector::Is(selectors)
            | SimpleSelector::Where(selectors)
            | SimpleSelector::Not(selectors) => selectors.iter().any(selector_references_scope),
            SimpleSelector::Has(relative) => relative
                .iter()
                .any(|relative| selector_references_scope(&relative.selector)),
            _ => false,
        })
    })
}

fn retain_best_scoped_match(
    current: &mut Option<(Specificity, usize)>,
    specificity: Specificity,
    proximity: usize,
) {
    let replace = current.is_none_or(|(current_specificity, current_proximity)| {
        specificity > current_specificity
            || specificity == current_specificity && proximity < current_proximity
    });
    if replace {
        *current = Some((specificity, proximity));
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

fn container_query_matches(
    node: &NodeHandle,
    prelude: &str,
    cache: &mut HashMap<String, Option<super::ContainerQuery>>,
    contexts: &HashMap<usize, ContainerContext>,
) -> bool {
    let prelude = prelude.trim();
    let query = cache
        .entry(prelude.to_string())
        .or_insert_with(|| super::parse_container_query(prelude));
    let Some(query) = query.as_ref() else {
        return false;
    };

    let mut ancestor = node.parent_node();
    while let Some(candidate) = ancestor {
        if candidate.node_type() == NodeType::Element
            && let Some(context) = contexts.get(&candidate.identity())
        {
            let supports_axis = context.container_type.eq_ignore_ascii_case("size")
                || (!query.requires_block_size()
                    && context.container_type.eq_ignore_ascii_case("inline-size"));
            let name_matches = query
                .name
                .as_ref()
                .is_none_or(|name| context.names.iter().any(|candidate| candidate == name));
            if supports_axis && name_matches {
                return query.matches(context.width, context.height);
            }
        }
        ancestor = candidate.parent_node();
    }
    false
}

/// Counts the total number of declarations inside a rule list (used for
/// source_order bookkeeping when a block is skipped due to a non-matching
/// media query).
fn count_declarations(rules: &[Rule]) -> usize {
    rules
        .iter()
        .map(|r| match r {
            Rule::Style(s) => s.declarations.len(),
            Rule::At(a) => {
                a.declarations.len() + a.block.as_deref().map(count_declarations).unwrap_or(0)
            }
            Rule::FontFace(_) => 0,
        })
        .sum()
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
            | "inset-inline-start"
            | "inset-inline-end"
            | "inset-block-start"
            | "inset-block-end"
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
        let Ok(stylesheet) = super::parse_stylesheet(&fake_rule) else {
            continue;
        };
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

fn compare_scope_proximity(left: &Candidate, right: &Candidate) -> std::cmp::Ordering {
    match (left.scope_proximity, right.scope_proximity) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn encapsulation_rank(candidate: &Candidate) -> usize {
    if candidate.important {
        candidate.encapsulation_order
    } else {
        usize::MAX.saturating_sub(candidate.encapsulation_order)
    }
}

fn tree_scope_order(stylesheets: &[StylesheetScope], node: &NodeHandle) -> usize {
    let Some(mut scope) = node.containing_shadow_root() else {
        return 0;
    };
    let mut unregistered_inner_scopes = 0usize;
    loop {
        if let Some(order) = stylesheets
            .iter()
            .find(|input| input.root.as_ref() == Some(&scope))
            .map(|input| input.encapsulation_order)
        {
            return order.saturating_add(unregistered_inner_scopes);
        }
        // A ShadowRoot without a <style> element has no StylesheetScope entry,
        // but inline declarations in it still occupy an inner tree context.
        // Anchor at the nearest registered ancestor and move inward from it.
        unregistered_inner_scopes += 1;
        let Some(outer_scope) = scope
            .shadow_host()
            .and_then(|host| host.containing_shadow_root())
        else {
            return unregistered_inner_scopes;
        };
        scope = outer_scope;
    }
}

fn log_unsupported_css_if_enabled(property: &str, value: &Value) {
    let Some(category) = css_audit_category(property) else {
        return;
    };

    let config = unsupported_css_config();
    if !config.logging_enabled && config.sqlite_path.is_none() {
        return;
    }

    let rendered_value = sanitize_unsupported_css_log_value(&render_value(value));
    if let Some(path) = config.sqlite_path.as_deref() {
        persist_css_audit_to_sqlite(path, category, property, &rendered_value);
        if let Some(top_n) = config.top_n {
            emit_css_audit_top_n_summary_if_updated(path, top_n, category);
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
            eprintln!("[omoikane][{}] {property}={value}", category.log_label());
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
            category TEXT NOT NULL DEFAULT 'unsupported',
            first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            occurrences INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (property, value)
        );
        CREATE INDEX IF NOT EXISTS idx_unsupported_css_log_occurrences
        ON unsupported_css_log (occurrences DESC);",
    )?;
    let has_category = conn
        .prepare("PRAGMA table_info(unsupported_css_log)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "category");
    if !has_category {
        conn.execute(
            "ALTER TABLE unsupported_css_log ADD COLUMN category TEXT NOT NULL DEFAULT 'unsupported'",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
fn persist_unsupported_css_to_sqlite(path: &str, property: &str, value: &str) {
    persist_css_audit_to_sqlite(path, CssAuditCategory::Unsupported, property, value);
}

fn persist_css_audit_to_sqlite(
    path: &str,
    category: CssAuditCategory,
    property: &str,
    value: &str,
) {
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
            "INSERT INTO unsupported_css_log (property, value, category, occurrences)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(property, value) DO UPDATE SET
               category = excluded.category,
               occurrences = unsupported_css_log.occurrences + 1,
               last_seen_at = CURRENT_TIMESTAMP",
            params![property, value, category.as_str()],
        )?;
        Ok(())
    });

    if let Err(error) = result {
        log_sqlite_error(&error);
    }
}

fn emit_css_audit_top_n_summary_if_updated(path: &str, top_n: usize, category: CssAuditCategory) {
    let rows = SQLITE_CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        let Some(conn) = connections.get_mut(path) else {
            return Ok(Vec::new());
        };
        query_css_audit_top_n(conn, top_n, category)
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
    category.as_str().hash(&mut hasher);
    for (property, value, occurrences) in &rows {
        property.hash(&mut hasher);
        value.hash(&mut hasher);
        occurrences.hash(&mut hasher);
    }
    let digest = hasher.finish();
    let key = format!("{path}#{top_n}#{}", category.as_str());
    let map = UNSUPPORTED_CSS_TOP_N_LAST_DIGEST.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map
        .lock()
        .expect("unsupported css top-n digest lock poisoned");
    if map.get(&key).copied() == Some(digest) {
        return;
    }
    map.insert(key, digest);

    let label = category.log_label();
    eprintln!("[omoikane][{label}][top-n] top {top_n} candidates (site/url anonymized)");
    for (index, (property, value, occurrences)) in rows.iter().enumerate() {
        let value = truncate_log_value(value, MAX_UNSUPPORTED_LOG_VALUE_LEN);
        eprintln!(
            "[omoikane][{label}][top-n] {}. {}={} (count={})",
            index + 1,
            property,
            value,
            occurrences
        );
    }
}

#[cfg(test)]
fn query_unsupported_css_top_n(
    conn: &Connection,
    top_n: usize,
) -> Result<Vec<(String, String, i64)>, rusqlite::Error> {
    query_css_audit_top_n(conn, top_n, CssAuditCategory::Unsupported)
}

fn query_css_audit_top_n(
    conn: &Connection,
    top_n: usize,
    category: CssAuditCategory,
) -> Result<Vec<(String, String, i64)>, rusqlite::Error> {
    let limit = i64::try_from(top_n).unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(
        "SELECT property, value, occurrences
         FROM unsupported_css_log
         WHERE category = ?1
         ORDER BY occurrences DESC, property ASC, value ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![category.as_str(), limit], |row| {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CssAuditCategory {
    Unsupported,
    VendorPrefixed,
}

impl CssAuditCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::VendorPrefixed => "vendor-prefixed",
        }
    }

    fn log_label(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported-css",
            Self::VendorPrefixed => "vendor-prefixed-css",
        }
    }
}

fn css_audit_category(property: &str) -> Option<CssAuditCategory> {
    if should_ignore_unsupported_css_logging(property) || is_supported_property(property) {
        None
    } else if property.starts_with('-') {
        Some(CssAuditCategory::VendorPrefixed)
    } else {
        Some(CssAuditCategory::Unsupported)
    }
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

pub(super) fn is_supported_property(name: &str) -> bool {
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
            | "background-clip"
            | "background-color"
            | "background-image"
            | "background-origin"
            | "background-position-x"
            | "background-position-y"
            | "background-repeat"
            | "background-size"
            | "backdrop-filter"
            | "backface-visibility"
            | "border-bottom-color"
            | "border-bottom-style"
            | "border-bottom-width"
            | "border-bottom-left-radius"
            | "border-bottom-right-radius"
            | "border-top-left-radius"
            | "border-top-right-radius"
            | "border-collapse"
            | "border-color"
            | "border-left-color"
            | "border-left-style"
            | "border-left-width"
            | "border-right-color"
            | "border-right-style"
            | "border-right-width"
            | "border-spacing"
            | "border-style"
            | "border-width"
            | "border-top-color"
            | "border-top-style"
            | "border-top-width"
            | "bottom"
            | "inset-inline-start"
            | "inset-inline-end"
            | "inset-block-start"
            | "inset-block-end"
            | "box-sizing"
            | "clear"
            | "clip-path"
            | "-webkit-clip-path"
            | "color"
            | "container-name"
            | "container-type"
            | "content"
            | "cursor"
            | "display"
            | "direction"
            | "flex-basis"
            | "flex-direction"
            | "flex-grow"
            | "flex-shrink"
            | "flex-wrap"
            | "float"
            | "filter"
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
            | "perspective"
            | "perspective-origin"
            | "pointer-events"
            | "right"
            | "row-gap"
            | "mix-blend-mode"
            | "transform"
            | "transform-origin"
            | "transform-style"
            | "transition"
            | "transition-property"
            | "transition-duration"
            | "transition-timing-function"
            | "transition-delay"
            | "text-align"
            | "text-decoration-line"
            | "text-decoration-color"
            | "text-decoration-style"
            | "text-transform"
            | "unicode-bidi"
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
            | "writing-mode"
            | "z-index"
            | "box-shadow"
            | "opacity"
            | "isolation"
            | "list-style-type"
            | "list-style-position"
            | "aspect-ratio"
            | "list-style-image"
            | "object-fit"
            | "object-position"
            | "mask"
            | "mask-image"
            | "mask-position"
            | "mask-position-x"
            | "mask-position-y"
            | "mask-repeat"
            | "mask-size"
            | "mask-mode"
            | "mask-composite"
            | "-webkit-mask"
            | "-webkit-mask-image"
            | "-webkit-mask-position"
            | "-webkit-mask-position-x"
            | "-webkit-mask-position-y"
            | "-webkit-mask-repeat"
            | "-webkit-mask-size"
            | "-webkit-mask-mode"
            | "-webkit-mask-composite"
    )
}

/// Returns whether a property/value pair is both syntactically valid and
/// implemented by Omoikane's style resolver.
///
/// This is the engine-side source of truth for the DOM `CSS.supports()` API.
/// Parsing a forgiving declaration list alone is insufficient: the CSS parser
/// deliberately retains unknown properties so they can be ignored by the
/// cascade and reported by diagnostics. Feature detection must additionally
/// require that every declaration produced by shorthand expansion is a
/// property the resolver understands.
pub(crate) fn supports_declaration(property: &str, value: &str) -> bool {
    let property = property.trim();
    let value = value.trim();
    if property.is_empty() || value.is_empty() || contains_top_level_semicolon(value) {
        return false;
    }

    let declarations = super::parse_style_attribute(&format!("{property}: {value}"));
    if declarations.is_empty() {
        return false;
    }

    // Custom properties accept the general CSS component-value grammar and
    // are consumed by var() resolution rather than the fixed property table.
    if property.starts_with("--") {
        return declarations.len() == 1 && declarations[0].name == property.to_ascii_lowercase();
    }

    declarations.iter().all(|declaration| {
        let name = canonical_property_name(&declaration.name);
        if !is_supported_property(name) {
            return false;
        }
        // A declaration containing var() is syntactically valid at parse time;
        // its property grammar is checked after custom-property substitution.
        if value_contains_var_function(&declaration.value) {
            return true;
        }
        match validate_declaration(name, &declaration.value) {
            DeclarationValidation::Invalid => false,
            DeclarationValidation::Valid(_) => true,
            DeclarationValidation::Unvalidated => {
                let computed =
                    compute_value(&declaration.value, name, ResolutionContext::default());
                !should_skip_computed_property(name, &computed)
            }
        }
    })
}

fn value_contains_var_function(value: &Value) -> bool {
    match value {
        Value::Function { name, arguments } => {
            name.eq_ignore_ascii_case("var") || arguments.iter().any(value_contains_var_function)
        }
        Value::List(values) => values.iter().any(value_contains_var_function),
        _ => false,
    }
}

/// Reject a second top-level declaration while preserving semicolons inside
/// strings and functions such as data URLs.
fn contains_top_level_semicolon(value: &str) -> bool {
    let Ok(tokens) = super::tokenize(value) else {
        return true;
    };
    let mut depth = 0usize;
    for token in tokens {
        match token {
            super::CssToken::ParenOpen | super::CssToken::BracketOpen => depth += 1,
            super::CssToken::ParenClose | super::CssToken::BracketClose => {
                depth = depth.saturating_sub(1);
            }
            super::CssToken::Semicolon if depth == 0 => return true,
            _ => {}
        }
    }
    false
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
    if property_name.eq_ignore_ascii_case("object-position") {
        return ComputedValue::Keyword(render_object_position_value(value, ctx));
    }
    if property_name.eq_ignore_ascii_case("aspect-ratio") {
        return ComputedValue::Keyword(render_aspect_ratio_value(value, ctx));
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
                let value = if is_non_negative_sizing_property(property_name)
                    && quantity.unit != CalcUnit::Unitless
                {
                    quantity.value.max(0.0)
                } else {
                    quantity.value
                };
                return match quantity.unit {
                    CalcUnit::Px => ComputedValue::Px(value),
                    CalcUnit::Percentage => {
                        if property_name == "font-size" {
                            ComputedValue::Px(ctx.parent_font_size * (value / 100.0))
                        } else {
                            ComputedValue::Percentage(value)
                        }
                    }
                    CalcUnit::Unitless => ComputedValue::Number(value),
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
                .map(|computed| clamp_sizing_computed_value(property_name, computed))
                .unwrap_or_else(|| ComputedValue::Keyword(render_value(value)))
        }
        Value::Function { .. } => ComputedValue::Keyword(render_value(value)),
        Value::List(values) => {
            if property_name.eq_ignore_ascii_case("transform")
                || property_name.eq_ignore_ascii_case("transform-origin")
                || property_name.eq_ignore_ascii_case("perspective-origin")
                || property_name.eq_ignore_ascii_case("overflow")
                || property_name.eq_ignore_ascii_case("box-shadow")
                || property_name.eq_ignore_ascii_case("background-size")
                || property_name.eq_ignore_ascii_case("background-repeat")
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
        Value::CommaList(values) if property_name.starts_with("background-") => {
            let rendered = values
                .iter()
                .map(|value| compute_background_layer_value(value, property_name, ctx))
                .collect::<Vec<_>>()
                .join(", ");
            ComputedValue::Keyword(rendered)
        }
        Value::CommaList(_) => ComputedValue::Keyword(render_value(value)),
    }
}

fn compute_background_layer_value(
    value: &Value,
    property_name: &str,
    ctx: ResolutionContext,
) -> String {
    if let Value::List(values) = value {
        return values
            .iter()
            .map(|value| computed_value_css_text(&compute_value(value, property_name, ctx)))
            .collect::<Vec<_>>()
            .join(" ");
    }
    computed_value_css_text(&compute_value(value, property_name, ctx))
}

fn clamp_sizing_computed_value(property_name: &str, value: ComputedValue) -> ComputedValue {
    if !is_non_negative_sizing_property(property_name) {
        return value;
    }
    match value {
        ComputedValue::Px(number) => ComputedValue::Px(number.max(0.0)),
        ComputedValue::Percentage(number) => ComputedValue::Percentage(number.max(0.0)),
        other => other,
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
    } else if name.eq_ignore_ascii_case("-webkit-mask-mode") {
        "mask-mode"
    } else if name.eq_ignore_ascii_case("-webkit-mask-composite") {
        "mask-composite"
    } else {
        name
    }
}

/// One `object-position` component: a keyword naming an edge or the centre, or a
/// `<length-percentage>` offset.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PositionAxis {
    /// The component may only name a horizontal edge.
    Horizontal,
    /// The component may only name a vertical edge.
    Vertical,
    /// `center` or a length-percentage, which fits either axis.
    Either,
}

/// Splits an `object-position` value into its `(x, y)` components, or `None`
/// when the value does not match `<position>`'s one- and two-value forms.
///
/// A single component centres the other axis. In the two-value form the axes are
/// assigned by the keywords, so `top center` names y first and comes back as
/// `(center, top)`; two components that name the same axis (`left right`) or
/// three or more components are rejected. The three- and four-value forms with
/// edge offsets (`left 10px top 20px`) are not supported yet.
fn object_position_components(value: &Value) -> Option<(Value, Value)> {
    let center = || Value::Keyword("center".to_string());
    let components: Vec<&Value> = match value {
        Value::List(values) => values.iter().collect(),
        single => vec![single],
    };
    let axes: Vec<PositionAxis> = components
        .iter()
        .map(|component| object_position_axis(component))
        .collect::<Option<Vec<_>>>()?;
    match components.as_slice() {
        [single] => {
            // A lone vertical keyword sets y; everything else sets x.
            if axes[0] == PositionAxis::Vertical {
                Some((center(), (*single).clone()))
            } else {
                Some(((*single).clone(), center()))
            }
        }
        [first, second] => match (axes[0], axes[1]) {
            (PositionAxis::Vertical, PositionAxis::Vertical)
            | (PositionAxis::Horizontal, PositionAxis::Horizontal) => None,
            (PositionAxis::Vertical, _) => Some(((*second).clone(), (*first).clone())),
            (_, PositionAxis::Horizontal) => Some(((*second).clone(), (*first).clone())),
            _ => Some(((*first).clone(), (*second).clone())),
        },
        _ => None,
    }
}

/// Returns which axis a single `object-position` component can name, or `None`
/// when it is not a valid component.
fn object_position_axis(value: &Value) -> Option<PositionAxis> {
    match value {
        Value::Keyword(keyword) => match keyword.to_ascii_lowercase().as_str() {
            "left" | "right" => Some(PositionAxis::Horizontal),
            "top" | "bottom" => Some(PositionAxis::Vertical),
            "center" => Some(PositionAxis::Either),
            _ => None,
        },
        Value::Percentage(_) | Value::Length(..) => Some(PositionAxis::Either),
        // A bare `0` is a length; other bare numbers are not valid offsets.
        Value::Number(number) if *number == 0.0 => Some(PositionAxis::Either),
        // Only a `calc()` that carries a length or percentage is a valid offset:
        // `calc(1)` and `calc(0)` are bare numbers, which Firefox 152 drops too.
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            calc_yields_length_or_percentage(arguments).then_some(PositionAxis::Either)
        }
        _ => None,
    }
}

/// Whether a `calc()` argument list mentions a length or percentage anywhere, so
/// its result is a `<length-percentage>` rather than a bare number.
fn calc_yields_length_or_percentage(arguments: &[Value]) -> bool {
    arguments.iter().any(|argument| match argument {
        Value::Length(..) | Value::Percentage(_) => true,
        Value::Function { arguments, .. } => calc_yields_length_or_percentage(arguments),
        Value::List(values) => calc_yields_length_or_percentage(values),
        _ => false,
    })
}

/// One component of an `aspect-ratio` value.
#[derive(Debug, Clone, PartialEq)]
enum AspectRatioPart {
    Auto,
    Slash,
    Number(Value),
}

/// Splits an `aspect-ratio` value into `auto` / `<ratio>` components, or `None`
/// when it does not match `auto || <ratio>`.
///
/// The tokenizer glues `1/1` into one keyword but keeps `2 / 1` as three
/// components, so both shapes are flattened into the same part list before the
/// grammar is checked. Numbers must be non-negative; a degenerate ratio such as
/// `0 / 1` is a valid value that layout then ignores.
fn aspect_ratio_parts(value: &Value) -> Option<(bool, Option<(Value, Value)>)> {
    let components: Vec<&Value> = match value {
        Value::List(values) => values.iter().collect(),
        single => vec![single],
    };
    let mut parts = Vec::new();
    for component in components {
        match component {
            Value::Keyword(keyword) if keyword.eq_ignore_ascii_case("auto") => {
                parts.push(AspectRatioPart::Auto)
            }
            Value::Keyword(keyword) if keyword == "/" => parts.push(AspectRatioPart::Slash),
            // A glued `1/1`, or a number the tokenizer kept as a keyword.
            Value::Keyword(keyword) => {
                let mut pieces = keyword.split('/');
                let first = pieces.next()?;
                parts.push(AspectRatioPart::Number(Value::Number(non_negative_number(
                    first,
                )?)));
                for piece in pieces {
                    parts.push(AspectRatioPart::Slash);
                    parts.push(AspectRatioPart::Number(Value::Number(non_negative_number(
                        piece,
                    )?)));
                }
            }
            // A literal negative number is invalid; an overflowing one is
            // clamped when the value is computed.
            Value::Number(number) if *number >= 0.0 => {
                parts.push(AspectRatioPart::Number(component.clone()))
            }
            // A `calc()` is a ratio component when it evaluates to a bare
            // number. Out-of-range results are clamped rather than invalid (CSS
            // Values 4), so the sign is not checked here.
            Value::Function { name, arguments }
                if name.eq_ignore_ascii_case("calc")
                    && calc_unitless_number(arguments).is_some() =>
            {
                parts.push(AspectRatioPart::Number(component.clone()))
            }
            _ => return None,
        }
    }

    let ratio_from = |parts: &[AspectRatioPart]| -> Option<Option<(Value, Value)>> {
        match parts {
            [] => Some(None),
            [AspectRatioPart::Number(width)] => Some(Some((width.clone(), Value::Number(1.0)))),
            [
                AspectRatioPart::Number(width),
                AspectRatioPart::Slash,
                AspectRatioPart::Number(height),
            ] => Some(Some((width.clone(), height.clone()))),
            _ => None,
        }
    };
    match parts.first() {
        Some(AspectRatioPart::Auto) => Some((true, ratio_from(&parts[1..])?)),
        _ => match parts.last() {
            Some(AspectRatioPart::Auto) => Some((true, ratio_from(&parts[..parts.len() - 1])?)),
            _ => {
                let ratio = ratio_from(&parts)?;
                // A bare `auto` is the only value with neither part.
                ratio.map(|ratio| (false, Some(ratio)))
            }
        },
    }
}

fn non_negative_number(text: &str) -> Option<f32> {
    let number = text.trim().parse::<f32>().ok()?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

/// Evaluates a `calc()` that must produce a bare number, for grammar checks that
/// run before a resolution context exists.
///
/// A length or percentage anywhere in the expression makes the result carry that
/// unit, which is rejected here, so the placeholder context cannot change the
/// outcome: only the unit decides, and units do not depend on it.
fn calc_unitless_number(arguments: &[Value]) -> Option<f32> {
    let placeholder = ResolutionContext {
        parent_font_size: 16.0,
        root_font_size: 16.0,
        viewport_width: 0.0,
        viewport_height: 0.0,
    };
    match evaluate_calc(arguments, placeholder) {
        Some(quantity) if quantity.unit == CalcUnit::Unitless => Some(quantity.value),
        _ => None,
    }
}

/// Clamps a ratio component into the `<number [0,∞]>` range the grammar allows.
///
/// A `calc()` resolving out of range is clamped rather than dropped, and a
/// literal that overflows the float range saturates the same way. Firefox 152
/// reports `calc(-1)` as `0`, and both `1e40` and `calc(1/0)` as the largest
/// float.
fn clamp_ratio_number(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, f32::MAX)
    }
}

/// Renders `aspect-ratio` the way getComputedStyle reports it: `auto`, a
/// `W / H` ratio, or `auto W / H` with `auto` first whichever order it was
/// written in (Firefox 152).
fn render_aspect_ratio_value(value: &Value, ctx: ResolutionContext) -> String {
    if let Value::Keyword(keyword) = value
        && is_css_wide_keyword(&keyword.to_ascii_lowercase())
    {
        return keyword.clone();
    }
    let Some((auto, ratio)) = aspect_ratio_parts(value) else {
        return render_value(value);
    };
    let ratio = ratio.and_then(|(width, height)| {
        Some(format!(
            "{} / {}",
            aspect_ratio_number(&width, ctx)?,
            aspect_ratio_number(&height, ctx)?
        ))
    });
    match (auto, ratio) {
        (true, Some(ratio)) => format!("auto {ratio}"),
        (true, None) => "auto".to_string(),
        (false, Some(ratio)) => ratio,
        // Neither part is not a value the grammar accepts.
        (false, None) => "auto".to_string(),
    }
}

fn aspect_ratio_number(value: &Value, ctx: ResolutionContext) -> Option<f32> {
    match value {
        Value::Number(number) => Some(clamp_ratio_number(*number)),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            match evaluate_calc(arguments, ctx) {
                Some(quantity) if quantity.unit == CalcUnit::Unitless => {
                    Some(clamp_ratio_number(quantity.value))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Renders `object-position` as the two `<x> <y>` components getComputedStyle
/// reports: edge keywords become percentages and lengths become pixels, matching
/// Firefox 152 (`top center` → `50% 0%`, `2em` → `32px 50%`).
fn render_object_position_value(value: &Value, ctx: ResolutionContext) -> String {
    if let Value::Keyword(keyword) = value
        && is_css_wide_keyword(&keyword.to_ascii_lowercase())
    {
        // Leave CSS-wide keywords for the cascade to resolve.
        return keyword.clone();
    }
    let Some((x, y)) = object_position_components(value) else {
        return render_value(value);
    };
    format!(
        "{} {}",
        render_object_position_component(&x, ctx),
        render_object_position_component(&y, ctx),
    )
}

/// Renders one already axis-assigned component, so an edge keyword maps to the
/// start or end of its axis without needing to know which axis that is.
fn render_object_position_component(value: &Value, ctx: ResolutionContext) -> String {
    match value {
        Value::Keyword(keyword) => match keyword.to_ascii_lowercase().as_str() {
            "left" | "top" => "0%".to_string(),
            "right" | "bottom" => "100%".to_string(),
            "center" => "50%".to_string(),
            other => other.to_string(),
        },
        Value::Percentage(percentage) => format!("{percentage}%"),
        Value::Number(number) if *number == 0.0 => "0px".to_string(),
        Value::Length(number, unit) => resolve_length_to_px(*number, unit, ctx)
            .map(|px| format!("{px}px"))
            .unwrap_or_else(|| format!("{number}{unit}")),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("calc") => {
            match evaluate_calc(arguments, ctx) {
                Some(quantity) => match quantity.unit {
                    CalcUnit::Px => format!("{}px", quantity.value),
                    CalcUnit::Percentage => format!("{}%", quantity.value),
                    CalcUnit::Unitless if quantity.value == 0.0 => "0px".to_string(),
                    CalcUnit::Unitless => quantity.value.to_string(),
                },
                None => render_value(value),
            }
        }
        other => render_value(other),
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
        Value::Function { name, arguments }
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "inset" | "circle" | "ellipse" | "polygon"
            ) =>
        {
            let canonical_name = name.to_ascii_lowercase();
            format!(
                "{}({})",
                canonical_name,
                arguments
                    .iter()
                    .map(|argument| render_clip_path_value(argument, ctx))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::List(values) => values
            .iter()
            .map(|value| render_clip_path_value(value, ctx))
            .collect::<Vec<_>>()
            .join(" "),
        Value::CommaList(values) => values
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
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
        "svw" | "lvw" | "dvw" => number * ctx.viewport_width / 100.0,
        "svh" | "lvh" | "dvh" => number * ctx.viewport_height / 100.0,
        "vi" | "svi" | "lvi" | "dvi" => number * ctx.viewport_width / 100.0,
        "vb" | "svb" | "lvb" | "dvb" => number * ctx.viewport_height / 100.0,
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

fn is_svg_element_for_presentational_hints(node: &NodeHandle) -> bool {
    if node.namespace_uri().as_deref() == Some("http://www.w3.org/2000/svg") {
        return true;
    }
    let Some(tag) = node.tag_name().map(|name| name.to_ascii_lowercase()) else {
        return false;
    };
    if !matches!(
        tag.as_str(),
        "svg" | "g" | "rect" | "circle" | "ellipse" | "line"
            | "polyline" | "polygon" | "path" | "text" | "tspan" | "textpath" | "use"
    ) {
        return false;
    }
    let mut current = Some(node.clone());
    while let Some(candidate) = current {
        let tag = candidate
            .tag_name()
            .map(|name| name.to_ascii_lowercase());
        if tag.as_deref() == Some("foreignobject") {
            return false;
        }
        if tag.as_deref() == Some("svg") {
            return true;
        }
        current = candidate.parent_node();
    }
    false
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

    // SVG presentation attributes participate in the CSS cascade below author
    // declarations. Expose pointer-events through computed style so hit
    // testing can distinguish a local attribute from an inherited value and
    // still honor explicit CSS overrides, including `auto`.
    let is_svg_element = is_svg_element_for_presentational_hints(node);
    if is_svg_element
        && !properties.contains_key("pointer-events")
        && let Some(value) = attributes
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("pointer-events"))
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
            .filter(|value| {
                let lower = value.to_ascii_lowercase();
                is_css_wide_keyword(&lower) || is_supported_pointer_events_keyword(&lower)
            })
    {
        properties.insert(
            "pointer-events".to_string(),
            ComputedValue::Keyword(value.to_ascii_lowercase()),
        );
    }

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
            let is_table_or_block = node.tag_name().as_deref().is_some_and(|tag| {
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
        "h1" => Some(UaDefaults {
            font_size_em: 2.0,
            font_weight_bold: true,
            margin_em: 0.67,
        }),
        "h2" => Some(UaDefaults {
            font_size_em: 1.5,
            font_weight_bold: true,
            margin_em: 0.83,
        }),
        "h3" => Some(UaDefaults {
            font_size_em: 1.17,
            font_weight_bold: true,
            margin_em: 1.0,
        }),
        "h4" => Some(UaDefaults {
            font_size_em: 1.0,
            font_weight_bold: true,
            margin_em: 1.33,
        }),
        "h5" => Some(UaDefaults {
            font_size_em: 0.83,
            font_weight_bold: true,
            margin_em: 1.67,
        }),
        "h6" => Some(UaDefaults {
            font_size_em: 0.67,
            font_weight_bold: true,
            margin_em: 2.33,
        }),
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
        "video" | "canvas" | "picture" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline-block".to_string()));
        }
        "audio" => {
            if node.get_attribute("controls").is_none() {
                properties.insert(
                    "display".to_string(),
                    ComputedValue::Keyword("none".to_string()),
                );
            } else {
                properties
                    .entry("display".to_string())
                    .or_insert(ComputedValue::Keyword("inline-block".to_string()));
            }
        }
        "source" => {
            properties.insert(
                "display".to_string(),
                ComputedValue::Keyword("none".to_string()),
            );
        }
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
            properties
                .entry("margin-top".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("margin-bottom".to_string())
                .or_insert(ComputedValue::Px(em));
        }
        "b" | "strong" => {
            properties
                .entry("font-weight".to_string())
                .or_insert(ComputedValue::Keyword("bold".to_string()));
        }
        "i" | "em" => {
            properties
                .entry("font-style".to_string())
                .or_insert(ComputedValue::Keyword("italic".to_string()));
        }
        "hr" => {
            properties
                .entry("border-top-style".to_string())
                .or_insert(ComputedValue::Keyword("inset".to_string()));
            properties
                .entry("border-top-width".to_string())
                .or_insert(ComputedValue::Px(1.0));
            let half_em = parent_font_size * 0.5;
            properties
                .entry("margin-top".to_string())
                .or_insert(ComputedValue::Px(half_em));
            properties
                .entry("margin-bottom".to_string())
                .or_insert(ComputedValue::Px(half_em));
        }
        "ul" => {
            properties
                .entry("list-style-type".to_string())
                .or_insert(ComputedValue::Keyword("disc".to_string()));
            properties
                .entry("list-style-position".to_string())
                .or_insert(ComputedValue::Keyword("outside".to_string()));
            let em = parent_font_size;
            properties
                .entry("margin-top".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("margin-bottom".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("padding-left".to_string())
                .or_insert(ComputedValue::Px(em * 2.5));
        }
        "ol" => {
            properties
                .entry("list-style-type".to_string())
                .or_insert(ComputedValue::Keyword("decimal".to_string()));
            properties
                .entry("list-style-position".to_string())
                .or_insert(ComputedValue::Keyword("outside".to_string()));
            let em = parent_font_size;
            properties
                .entry("margin-top".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("margin-bottom".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("padding-left".to_string())
                .or_insert(ComputedValue::Px(em * 2.5));
        }
        "li" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("list-item".to_string()));
        }
        "blockquote" => {
            let em = parent_font_size;
            properties
                .entry("margin-top".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("margin-bottom".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("margin-left".to_string())
                .or_insert(ComputedValue::Px(40.0));
            properties
                .entry("margin-right".to_string())
                .or_insert(ComputedValue::Px(40.0));
        }
        "pre" => {
            properties
                .entry("font-family".to_string())
                .or_insert(ComputedValue::Keyword("monospace".to_string()));
            properties
                .entry("white-space".to_string())
                .or_insert(ComputedValue::Keyword("pre".to_string()));
            let em = parent_font_size;
            properties
                .entry("margin-top".to_string())
                .or_insert(ComputedValue::Px(em));
            properties
                .entry("margin-bottom".to_string())
                .or_insert(ComputedValue::Px(em));
        }
        "code" | "kbd" | "samp" | "tt" => {
            properties
                .entry("font-family".to_string())
                .or_insert(ComputedValue::Keyword("monospace".to_string()));
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline".to_string()));
        }
        "dd" => {
            properties
                .entry("margin-left".to_string())
                .or_insert(ComputedValue::Px(40.0));
        }
        "th" => {
            properties
                .entry("font-weight".to_string())
                .or_insert(ComputedValue::Keyword("bold".to_string()));
            properties
                .entry("text-align".to_string())
                .or_insert(ComputedValue::Keyword("center".to_string()));
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("table-cell".to_string()));
        }
        "td" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("table-cell".to_string()));
        }
        "a" => {
            properties
                .entry("text-decoration-line".to_string())
                .or_insert(ComputedValue::Keyword("underline".to_string()));
            properties
                .entry("color".to_string())
                .or_insert(ComputedValue::Color("#0000ee".to_string()));
        }
        "sub" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline".to_string()));
            properties
                .entry("vertical-align".to_string())
                .or_insert(ComputedValue::Keyword("sub".to_string()));
            let smaller = parent_font_size * 0.833;
            properties
                .entry("font-size".to_string())
                .or_insert(ComputedValue::Px(smaller));
        }
        "sup" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline".to_string()));
            properties
                .entry("vertical-align".to_string())
                .or_insert(ComputedValue::Keyword("super".to_string()));
            let smaller = parent_font_size * 0.833;
            properties
                .entry("font-size".to_string())
                .or_insert(ComputedValue::Px(smaller));
        }
        "small" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("inline".to_string()));
            let smaller = parent_font_size * 0.833;
            properties
                .entry("font-size".to_string())
                .or_insert(ComputedValue::Px(smaller));
        }
        "center" => {
            properties
                .entry("text-align".to_string())
                .or_insert(ComputedValue::Keyword("center".to_string()));
        }
        "table" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("table".to_string()));
        }
        "tr" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("table-row".to_string()));
        }
        "thead" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("table-header-group".to_string()));
        }
        "tbody" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("table-row-group".to_string()));
        }
        "tfoot" => {
            properties
                .entry("display".to_string())
                .or_insert(ComputedValue::Keyword("table-footer-group".to_string()));
        }
        _ => {}
    }
}

fn apply_initial_values(properties: &mut BTreeMap<String, ComputedValue>) {
    properties
        .entry("background-clip".to_string())
        .or_insert_with(|| ComputedValue::Keyword("border-box".to_string()));
    properties
        .entry("background-color".to_string())
        .or_insert_with(|| ComputedValue::Color("transparent".to_string()));
    properties
        .entry("background-origin".to_string())
        .or_insert_with(|| ComputedValue::Keyword("padding-box".to_string()));
    properties
        .entry("color".to_string())
        .or_insert_with(|| ComputedValue::Color("black".to_string()));
    properties
        .entry("font-size".to_string())
        .or_insert_with(|| ComputedValue::Px(16.0));
    properties
        .entry("direction".to_string())
        .or_insert_with(|| ComputedValue::Keyword("ltr".to_string()));
    properties
        .entry("writing-mode".to_string())
        .or_insert_with(|| ComputedValue::Keyword("horizontal-tb".to_string()));
    properties
        .entry("unicode-bidi".to_string())
        .or_insert_with(|| ComputedValue::Keyword("normal".to_string()));
    properties
        .entry("text-transform".to_string())
        .or_insert_with(|| ComputedValue::Keyword("none".to_string()));
    // `cursor` initial value is `auto` (CSS UI). Ensuring it is always present
    // lets a dropped/absent `cursor` declaration serialize as `auto` in
    // getComputedStyle (Acid3 test 47).
    properties
        .entry("cursor".to_string())
        .or_insert_with(|| ComputedValue::Keyword("auto".to_string()));
    properties
        .entry("pointer-events".to_string())
        .or_insert_with(|| ComputedValue::Keyword("auto".to_string()));
    properties
        .entry("position".to_string())
        .or_insert_with(|| ComputedValue::Keyword("static".to_string()));
    properties
        .entry("container-name".to_string())
        .or_insert_with(|| ComputedValue::Keyword("none".to_string()));
    properties
        .entry("container-type".to_string())
        .or_insert_with(|| ComputedValue::Keyword("normal".to_string()));
    // CSS Sizing: `aspect-ratio` is `auto`, meaning "use the intrinsic ratio".
    properties
        .entry("aspect-ratio".to_string())
        .or_insert_with(|| ComputedValue::Keyword("auto".to_string()));
    // CSS Images: `object-fit` is `fill` and `object-position` is `50% 50%`.
    // Keeping them present lets getComputedStyle serialize the initial value
    // even when nothing declares them.
    properties
        .entry("object-fit".to_string())
        .or_insert_with(|| ComputedValue::Keyword("fill".to_string()));
    properties
        .entry("object-position".to_string())
        .or_insert_with(|| ComputedValue::Keyword("50% 50%".to_string()));
    // CSS Masking initial values.  `none` is an identity mask in the paint
    // implementation; match-source resolves gradients/images through their
    // alpha channel and keeps SVG/image defaults deterministic.
    properties
        .entry("mask-image".to_string())
        .or_insert_with(|| ComputedValue::Keyword("none".to_string()));
    properties
        .entry("mask-mode".to_string())
        .or_insert_with(|| ComputedValue::Keyword("match-source".to_string()));
    properties
        .entry("mask-composite".to_string())
        .or_insert_with(|| ComputedValue::Keyword("add".to_string()));
    properties
        .entry("transform".to_string())
        .or_insert_with(|| ComputedValue::Keyword("none".to_string()));
    properties
        .entry("transform-origin".to_string())
        .or_insert_with(|| ComputedValue::Keyword("50% 50%".to_string()));
    properties
        .entry("perspective".to_string())
        .or_insert_with(|| ComputedValue::Keyword("none".to_string()));
    properties
        .entry("perspective-origin".to_string())
        .or_insert_with(|| ComputedValue::Keyword("50% 50%".to_string()));
    properties
        .entry("transform-style".to_string())
        .or_insert_with(|| ComputedValue::Keyword("flat".to_string()));
    properties
        .entry("backface-visibility".to_string())
        .or_insert_with(|| ComputedValue::Keyword("visible".to_string()));
    properties
        .entry("mix-blend-mode".to_string())
        .or_insert_with(|| ComputedValue::Keyword("normal".to_string()));
    properties
        .entry("isolation".to_string())
        .or_insert_with(|| ComputedValue::Keyword("auto".to_string()));
    properties
        .entry("transition-property".to_string())
        .or_insert_with(|| ComputedValue::Keyword("all".to_string()));
    properties
        .entry("transition-duration".to_string())
        .or_insert_with(|| ComputedValue::Keyword("0s".to_string()));
    properties
        .entry("transition-timing-function".to_string())
        .or_insert_with(|| ComputedValue::Keyword("ease".to_string()));
    properties
        .entry("transition-delay".to_string())
        .or_insert_with(|| ComputedValue::Keyword("0s".to_string()));
}

fn normalize_background_layer_lists(properties: &mut BTreeMap<String, ComputedValue>) {
    let image_count = properties
        .get("background-image")
        .map(computed_value_css_text)
        .map(|value| super::split_top_level_commas(&value).len())
        .unwrap_or(1);
    for (name, default) in [
        ("background-position-x", "0%"),
        ("background-position-y", "0%"),
        ("background-size", "auto"),
        ("background-repeat", "repeat"),
        ("background-attachment", "scroll"),
        ("background-origin", "padding-box"),
        ("background-clip", "border-box"),
    ] {
        let raw = properties
            .get(name)
            .map(computed_value_css_text)
            .unwrap_or_else(|| default.to_string());
        let values = super::split_top_level_commas(&raw);
        if image_count == 1 && values.len() == 1 {
            continue;
        }
        let normalized = (0..image_count)
            .map(|index| values[index % values.len()].trim())
            .collect::<Vec<_>>()
            .join(", ");
        properties.insert(name.to_string(), ComputedValue::Keyword(normalized));
    }
}

fn computed_value_css_text(value: &ComputedValue) -> String {
    match value {
        ComputedValue::Keyword(value)
        | ComputedValue::String(value)
        | ComputedValue::Color(value) => value.clone(),
        ComputedValue::Px(value) => format!("{value}px"),
        ComputedValue::Percentage(value) => format!("{value}%"),
        ComputedValue::Number(value) => value.to_string(),
        ComputedValue::CalcPxPercent(px, percentage) => {
            format!("calc({px}px + {percentage}%)")
        }
    }
}

fn resolve_non_inherited_css_wide_keywords(properties: &mut BTreeMap<String, ComputedValue>) {
    for name in [
        "aspect-ratio",
        "background-clip",
        "background-origin",
        "container-name",
        "container-type",
        "object-fit",
        "object-position",
        "mask-image",
        "mask-mode",
        "mask-composite",
        "position",
        "perspective",
        "perspective-origin",
        "transform",
        "transform-origin",
        "transform-style",
        "backface-visibility",
        "mix-blend-mode",
        "isolation",
        "transition-property",
        "transition-duration",
        "transition-timing-function",
        "transition-delay",
        "unicode-bidi",
    ] {
        let uses_initial_value = matches!(
            properties.get(name),
            Some(ComputedValue::Keyword(keyword))
                if matches!(
                    keyword.to_ascii_lowercase().as_str(),
                    "initial" | "unset" | "revert" | "revert-layer"
                )
        );
        if uses_initial_value {
            properties.remove(name);
        }
    }
}

/// Resolve CSS-wide keywords for inherited writing-direction properties before
/// the normal inheritance pass. `initial`/`revert` use the property initial
/// value here (including `revert-layer`), while `unset` follows the inherited
/// value just like `inherit`.
fn resolve_writing_direction_css_wide_keywords(
    properties: &mut BTreeMap<String, ComputedValue>,
    parent_style: Option<&ComputedStyle>,
) {
    for name in ["direction", "writing-mode"] {
        let Some(ComputedValue::Keyword(keyword)) = properties.get(name) else {
            continue;
        };
        let lower = keyword.to_ascii_lowercase();
        if matches!(lower.as_str(), "initial" | "revert" | "revert-layer") {
            let initial = match name {
                "direction" => "ltr",
                "writing-mode" => "horizontal-tb",
                _ => unreachable!("writing-direction property list is fixed"),
            };
            properties.insert(name.to_string(), ComputedValue::Keyword(initial.to_string()));
        } else if lower == "unset" {
            if let Some(parent) = parent_style.and_then(|style| style.get(name)) {
                properties.insert(name.to_string(), parent.clone());
            } else {
                properties.remove(name);
            }
        }
    }
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
            && let Some(parent_value) = parent_style.get(&name)
        {
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
        "pointer-events",
        "text-align",
        "text-decoration-color",
        "text-decoration-line",
        "text-decoration-style",
        "text-indent",
        "text-transform",
        "visibility",
        "white-space",
        "writing-mode",
        "word-break",
        "word-spacing",
    ] {
        if !properties.contains_key(inherited_name)
            && let Some(value) = parent_style.get(inherited_name)
        {
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
        && let Some(ComputedValue::Px(value)) = parent_style.get("font-size")
    {
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
        && let Value::List(items) = &arguments[0]
    {
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
    let a = alpha.or_else(|| flat.get(3).and_then(|v| extract_alpha(v)));

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
        Value::CommaList(values) => values
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
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
