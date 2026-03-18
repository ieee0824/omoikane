//! CSS cascade and computed style resolution.

use std::collections::{BTreeMap, HashMap};

use crate::dom::{Node, NodeHandle, NodeType};

use super::{
    PseudoElement, Rule, Specificity, Stylesheet, Value, matches_selector_with_pseudo, specificity,
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
    Color(String),
    String(String),
    Number(f32),
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

/// Computes styles and caches results per node.
#[derive(Debug, Default)]
pub struct StyleResolver {
    stylesheets: Vec<StylesheetInput>,
    cache: HashMap<usize, ComputedStyle>,
    pseudo_cache: HashMap<(usize, PseudoElement), ComputedStyle>,
}

impl StyleResolver {
    /// Creates a new style resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a stylesheet with its origin.
    pub fn add_stylesheet(&mut self, origin: Origin, stylesheet: Stylesheet) {
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
        &self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
    ) -> ComputedStyle {
        self.compute_style_with_pseudo(node, parent_style, None)
    }

    fn compute_style_with_pseudo(
        &self,
        node: &NodeHandle,
        parent_style: Option<&ComputedStyle>,
        pseudo: Option<PseudoElement>,
    ) -> ComputedStyle {
        let mut candidates = Vec::new();
        let mut source_order = 0usize;

        for input in &self.stylesheets {
            collect_rule_candidates(
                node,
                &input.stylesheet.rules,
                input.origin,
                pseudo,
                &mut source_order,
                &mut candidates,
            );
        }

        candidates.sort_by(|left, right| {
            cascade_rank(left)
                .cmp(&cascade_rank(right))
                .then(left.specificity.cmp(&right.specificity))
                .then(left.source_order.cmp(&right.source_order))
        });

        let mut properties: BTreeMap<String, ComputedValue> = BTreeMap::new();

        for candidate in candidates {
            let font_size = inherited_font_size(parent_style, &properties);
            let computed = compute_value(&candidate.value, &candidate.name, font_size);
            properties.insert(candidate.name, computed);
        }

        apply_inheritance(&mut properties, parent_style);
        apply_initial_values(&mut properties);

        ComputedStyle { properties }
    }
}

#[derive(Debug, Clone)]
struct Candidate {
    name: String,
    value: Value,
    important: bool,
    origin: Origin,
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
                            name: declaration.name.clone(),
                            value: declaration.value.clone(),
                            important: declaration.important,
                            origin,
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
                    collect_rule_candidates(node, block, origin, pseudo, source_order, out);
                } else {
                    *source_order += at_rule.declarations.len();
                }
            }
        }
    }
}

fn cascade_rank(candidate: &Candidate) -> (u8, u8) {
    let importance = if candidate.important { 1 } else { 0 };
    let origin = match (candidate.important, candidate.origin) {
        (true, Origin::User) => 5,
        (true, Origin::Author) => 4,
        (true, Origin::UserAgent) => 3,
        (false, Origin::Author) => 2,
        (false, Origin::User) => 1,
        (false, Origin::UserAgent) => 0,
    };
    (importance, origin)
}

fn compute_value(value: &Value, property_name: &str, parent_font_size: f32) -> ComputedValue {
    match value {
        Value::Keyword(keyword) => {
            if is_color_keyword(keyword)
                || property_name.ends_with("color")
                || property_name == "color"
            {
                ComputedValue::Color(keyword.clone())
            } else {
                ComputedValue::Keyword(keyword.clone())
            }
        }
        Value::Length(number, unit) => {
            let px = match unit.as_str() {
                "px" => *number,
                "em" => *number * parent_font_size,
                _ => *number,
            };
            ComputedValue::Px(px)
        }
        Value::Percentage(percent) => {
            let px = if property_name == "font-size" {
                parent_font_size * (*percent / 100.0)
            } else {
                *percent
            };
            ComputedValue::Px(px)
        }
        Value::Color(color) => ComputedValue::Color(color.clone()),
        Value::String(value) => ComputedValue::String(value.clone()),
        Value::Number(value) => ComputedValue::Number(*value),
        Value::Function { name, arguments } if name.eq_ignore_ascii_case("rgb") => {
            let channels: Vec<u8> = arguments
                .iter()
                .map(|argument| match argument {
                    Value::Number(number) => *number as u8,
                    _ => 0,
                })
                .collect();
            if channels.len() == 3 {
                ComputedValue::Color(format!(
                    "#{:02x}{:02x}{:02x}",
                    channels[0], channels[1], channels[2]
                ))
            } else {
                ComputedValue::Keyword(name.clone())
            }
        }
        Value::Function { .. } => ComputedValue::Keyword(render_value(value)),
        Value::List(values) => {
            if let Some(first) = values.first() {
                compute_value(first, property_name, parent_font_size)
            } else {
                ComputedValue::Keyword(String::new())
            }
        }
    }
}

fn apply_initial_values(properties: &mut BTreeMap<String, ComputedValue>) {
    properties
        .entry("color".to_string())
        .or_insert_with(|| ComputedValue::Color("black".to_string()));
    properties
        .entry("font-size".to_string())
        .or_insert_with(|| ComputedValue::Px(16.0));
}

fn apply_inheritance(
    properties: &mut BTreeMap<String, ComputedValue>,
    parent_style: Option<&ComputedStyle>,
) {
    let Some(parent_style) = parent_style else {
        return;
    };

    for inherited_name in ["color", "font-size", "line-height", "white-space"] {
        if !properties.contains_key(inherited_name) {
            if let Some(value) = parent_style.get(inherited_name) {
                properties.insert(inherited_name.to_string(), value.clone());
            }
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
    if let Some(parent_style) = parent_style {
        if let Some(ComputedValue::Px(value)) = parent_style.get("font-size") {
            return *value;
        }
    }
    16.0
}

fn is_color_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "black" | "white" | "red" | "green" | "blue" | "gray" | "grey"
    )
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Keyword(value) => value.clone(),
        Value::Length(number, unit) => format!("{number}{unit}"),
        Value::Color(value) => value.clone(),
        Value::Function { name, arguments } => format!(
            "{name}({})",
            arguments
                .iter()
                .map(render_value)
                .collect::<Vec<_>>()
                .join(", ")
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

#[cfg(test)]
mod tests {
    use crate::css::{PseudoElement, parse_stylesheet};
    use crate::dom::NodeHandle;

    use super::*;

    fn sample_tree() -> (NodeHandle, NodeHandle, NodeHandle, NodeHandle) {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let title = NodeHandle::element("h1");

        title.set_attribute("id", "hero");
        title.set_attribute("class", "primary");

        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(title.clone());

        (document, body, title, html)
    }

    #[test]
    fn applies_origin_importance_specificity_and_source_order() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::UserAgent,
            parse_stylesheet("h1 { color: black; }").unwrap(),
        );
        resolver.add_stylesheet(
            Origin::User,
            parse_stylesheet("h1 { color: green; }").unwrap(),
        );
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1 { color: blue; } #hero { color: red !important; }").unwrap(),
        );

        let style = resolver.computed_style(&title);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("red".to_string()))
        );
    }

    #[test]
    fn important_user_rule_beats_important_author_rule() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("#hero { color: red !important; }").unwrap(),
        );
        resolver.add_stylesheet(
            Origin::User,
            parse_stylesheet("h1 { color: green !important; }").unwrap(),
        );

        let style = resolver.computed_style(&title);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("green".to_string()))
        );
    }

    #[test]
    fn inherits_color_and_font_size() {
        let (document, body, title, html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { color: blue; font-size: 20px; }").unwrap(),
        );

        let _ = document;
        let _ = html;
        let body_style = resolver.computed_style(&body);
        let title_style = resolver.computed_style(&title);

        assert_eq!(
            body_style.get("color"),
            Some(&ComputedValue::Color("blue".to_string()))
        );
        assert_eq!(
            title_style.get("color"),
            Some(&ComputedValue::Color("blue".to_string()))
        );
        assert_eq!(title_style.get("font-size"), Some(&ComputedValue::Px(20.0)));
    }

    #[test]
    fn resolves_em_and_percentage_font_sizes() {
        let (document, _body, title, html) = sample_tree();
        let mut resolver = StyleResolver::new();

        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("body { font-size: 20px; } h1 { margin-top: 2em; font-size: 150%; }")
                .unwrap(),
        );

        let _ = document;
        let _ = html;
        let style = resolver.computed_style(&title);

        assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(30.0)));
        assert_eq!(style.get("margin-top"), Some(&ComputedValue::Px(40.0)));
    }

    #[test]
    fn caches_computed_styles() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1 { color: blue; }").unwrap(),
        );

        let first = resolver.computed_style(&title);
        let second = resolver.computed_style(&title);

        assert_eq!(first, second);
        assert!(resolver.cache.len() >= 1);
    }

    #[test]
    fn applies_initial_values_when_no_rule_matches() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();

        let style = resolver.computed_style(&title);
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("black".to_string()))
        );
        assert_eq!(style.get("font-size"), Some(&ComputedValue::Px(16.0)));
    }

    #[test]
    fn keeps_pseudo_element_rules_out_of_normal_computed_style() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1::before { content: \"prefix\"; color: red; }").unwrap(),
        );

        let style = resolver.computed_style(&title);
        assert_eq!(style.get("content"), None);
        assert_eq!(style.get("color"), Some(&ComputedValue::Color("black".to_string())));
    }

    #[test]
    fn resolves_computed_style_for_pseudo_elements() {
        let (_document, _body, title, _html) = sample_tree();
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet("h1 { color: blue; } h1::before { content: \"prefix\"; }").unwrap(),
        );

        let style = resolver
            .computed_pseudo_style(&title, PseudoElement::Before)
            .unwrap();
        assert_eq!(
            style.get("content"),
            Some(&ComputedValue::String("prefix".to_string()))
        );
        assert_eq!(
            style.get("color"),
            Some(&ComputedValue::Color("blue".to_string()))
        );
    }
}
