//! Narrow retained-geometry proof for an empty positioned HTML background box.
use super::*;

const PAINT_FIELDS: [&str; 6] = [
    "background-image",
    "background-color",
    "border-top-left-radius",
    "border-top-right-radius",
    "border-bottom-right-radius",
    "border-bottom-left-radius",
];

fn reads_attribute(value: &Value) -> bool {
    match value {
        Value::Function { name, arguments } => {
            name.eq_ignore_ascii_case("attr") || arguments.iter().any(reads_attribute)
        }
        Value::List(values) | Value::CommaList(values) => values.iter().any(reads_attribute),
        _ => false,
    }
}

fn safe_leaf_style(style: &ComputedStyle) -> bool {
    let keyword_is = |name, expected| matches!(style.get(name), Some(ComputedValue::Keyword(value)) if value == expected);
    keyword_is("position", "absolute")
        && keyword_is("display", "block")
        && style.get("float").is_none_or(|value| matches!(value, ComputedValue::Keyword(value) if value == "none"))
        && style.get("shape-outside").is_none_or(|value| matches!(value, ComputedValue::Keyword(value) if value == "none"))
        && PAINT_FIELDS[2..].iter().all(|name| style.get(name).is_none_or(|value| matches!(value, ComputedValue::Px(value) if value.is_finite() && *value >= 0.0)))
        && ["animation-name", "transition-duration", "transition-delay"].iter().all(|name| style.get(name).is_none_or(|value| matches!(value, ComputedValue::Keyword(value) if value == "none" || value == "0s")))
        && !style.component_values.values().chain(style.custom_properties.values()).any(reads_attribute)
}

fn pseudo_can_generate_or_schedule(candidate: &Candidate) -> bool {
    let name = candidate.name.as_str();
    name == "all"
        || name.starts_with("animation")
        || name.starts_with("transition")
        || (name == "content"
            && !matches!(&candidate.value, Value::Keyword(value) if value.eq_ignore_ascii_case("none") || value.eq_ignore_ascii_case("normal")))
}

impl StyleResolver {
    /// Tests syntax-normalized inline changes before attempting a geometry proof.
    pub(crate) fn is_background_image_inline_change(
        previous: Option<&str>,
        current: Option<&str>,
    ) -> bool {
        let previous = parsed_inline_style(previous);
        let current = parsed_inline_style(current);
        previous
            .iter()
            .filter(|d| d.name != "background-image")
            .eq(current.iter().filter(|d| d.name != "background-image"))
            && !previous
                .iter()
                .filter(|d| d.name == "background-image")
                .eq(current.iter().filter(|d| d.name == "background-image"))
    }

    /// Reads the OLD immutable cache entry only; never recomputes changed DOM.
    pub(crate) fn cached_leaf_paint_style(
        &self,
        node: &NodeHandle,
    ) -> Result<Arc<ComputedStyle>, &'static str> {
        if self.blocked_inline_style_nodes.contains(&node.identity()) {
            return Err("csp_blocked");
        }
        if self.has_running_transitions() || !self.animation_node_ids().is_empty() {
            return Err("clock_state");
        }
        let style = self
            .cache
            .get(&node.identity())
            .ok_or("old_style_missing")?;
        if !safe_leaf_style(style) {
            return Err("old_style_dependency");
        }
        Ok(Arc::clone(style))
    }

    /// Resolves current values only after ordinary invalidation reset selectors.
    pub(crate) fn validate_leaf_paint_style(
        &mut self,
        node: &NodeHandle,
        previous: &ComputedStyle,
        allow_background_delta: bool,
    ) -> Result<(), &'static str> {
        if self.leaf_has_declaration_dependency(node) {
            return Err("pseudo_dependency");
        }
        let current = self.computed_style_shared(node);
        self.cached_leaf_paint_style(node)?;
        if !safe_leaf_style(&current) {
            return Err("new_style_dependency");
        }
        if !allow_background_delta {
            return (previous == current.as_ref())
                .then_some(())
                .ok_or("data_style_changed");
        }
        // COW map clones preserve system-color provenance and every private
        // metadata field; CSSOM property iteration alone would lose evidence.
        let mut before = previous.clone();
        let mut after = current.as_ref().clone();
        for name in PAINT_FIELDS {
            before.properties.remove(name);
            after.properties.remove(name);
            before.component_values.remove(name);
            after.component_values.remove(name);
        }
        (before == after)
            .then_some(())
            .ok_or("nonpaint_style_changed")
    }

    // Candidate collection has no timeline sampling or I/O. Overridden/unknown
    // content remains a conservative rejection; ordinary ::before box-sizing
    // resets alone do not make every real thumbnail ineligible.
    fn leaf_has_declaration_dependency(&mut self, node: &NodeHandle) -> bool {
        let element_keys = ElementMatchKeys::from_node(node);
        for pseudo in [
            None,
            Some(PseudoElement::Before),
            Some(PseudoElement::After),
        ] {
            let mut candidates = Vec::new();
            let mut source_order = 0;
            for (position, ((input, index), scope)) in self
                .stylesheets
                .iter()
                .zip(&self.rule_indexes)
                .zip(&self.stylesheet_scopes)
                .enumerate()
            {
                let layer = LayerContextKey {
                    origin: input.origin,
                    scope_root: scope.root.as_ref().map(NodeHandle::identity),
                };
                let order = self
                    .layer_orders
                    .get(&layer)
                    .expect("stylesheet layers registered");
                collect_indexed_rule_candidates(
                    node,
                    &input.stylesheet.rules,
                    index,
                    input.origin,
                    self.stylesheet_ids[position],
                    layer,
                    order,
                    pseudo,
                    &mut source_order,
                    &mut candidates,
                    self.viewport_width,
                    self.viewport_height,
                    self.color_scheme_dark,
                    self.media_type,
                    &self.media_environment,
                    &mut self.media_query_cache,
                    &mut self.scope_prelude_cache,
                    &mut self.container_query_cache,
                    &self.container_contexts,
                    element_keys.as_ref(),
                    &mut self.selector_match_cache,
                    scope.root.as_ref(),
                    scope.implicit_scope_root.as_ref(),
                    scope.encapsulation_order,
                );
            }
            if candidates.iter().any(|candidate| {
                let clock = candidate.name == "all"
                    || candidate.name.starts_with("animation")
                    || candidate.name.starts_with("transition");
                clock || (pseudo.is_some() && pseudo_can_generate_or_schedule(candidate))
            }) {
                return true;
            }
        }
        false
    }
}
