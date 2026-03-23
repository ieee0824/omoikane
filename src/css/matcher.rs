//! CSS selector matching against the DOM tree.

use crate::dom::{Node, NodeHandle, NodeType};

use super::{AttributeOperator, Combinator, Selector, SelectorPart, SimpleSelector};

/// Supported pseudo-elements for style matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PseudoElement {
    Before,
    After,
}

impl PseudoElement {
    /// Returns the CSS identifier for this pseudo-element.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

/// CSS selector specificity `(a, b, c)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity {
    pub ids: u32,
    pub classes: u32,
    pub elements: u32,
}

impl Specificity {
    /// Returns the zero specificity.
    pub const fn zero() -> Self {
        Self {
            ids: 0,
            classes: 0,
            elements: 0,
        }
    }
}

/// Returns `true` when `node` matches `selector`.
pub fn matches_selector(node: &NodeHandle, selector: &Selector) -> bool {
    matches_selector_with_pseudo(node, selector, None)
}

/// Returns `true` when `node` matches `selector` for the requested pseudo-element.
pub fn matches_selector_with_pseudo(
    node: &NodeHandle,
    selector: &Selector,
    pseudo: Option<PseudoElement>,
) -> bool {
    if selector.parts.is_empty() || node.node_type() != NodeType::Element {
        return false;
    }

    if selector_pseudo_element(selector) != pseudo {
        return false;
    }

    matches_selector_part(node, selector, selector.parts.len() - 1, pseudo)
}

/// Returns the pseudo-element targeted by `selector`, if any.
pub fn selector_pseudo_element(selector: &Selector) -> Option<PseudoElement> {
    let mut pseudo = None;
    for part in &selector.parts {
        for simple in &part.simples {
            let name = match simple {
                SimpleSelector::PseudoElement(name) => Some(name.as_str()),
                SimpleSelector::PseudoClass(name)
                    if matches!(name.as_str(), "before" | "after") =>
                {
                    Some(name.as_str())
                }
                _ => None,
            };
            if let Some(name) = name {
                let current = match name {
                    "before" => PseudoElement::Before,
                    "after" => PseudoElement::After,
                    _ => return None,
                };
                if pseudo.is_some() {
                    return None;
                }
                pseudo = Some(current);
            }
        }
    }
    pseudo
}

/// Computes selector specificity.
pub fn specificity(selector: &Selector) -> Specificity {
    let mut value = Specificity::zero();

    for part in &selector.parts {
        for simple in &part.simples {
            add_simple_specificity(&mut value, simple);
        }
    }

    value
}

fn add_simple_specificity(value: &mut Specificity, simple: &SimpleSelector) {
    match simple {
        SimpleSelector::Id(_) => value.ids += 1,
        SimpleSelector::Class(_) | SimpleSelector::Attribute { .. } => value.classes += 1,
        SimpleSelector::PseudoClass(name) if matches!(name.as_str(), "before" | "after") => {
            value.elements += 1
        }
        SimpleSelector::PseudoClass(_) => value.classes += 1,
        SimpleSelector::Type(_) | SimpleSelector::PseudoElement(_) => value.elements += 1,
        SimpleSelector::Universal => {}
        SimpleSelector::Not(inner) => {
            // CSS Selectors Level 4 §17: :not() itself contributes zero specificity.
            // The specificity of :not() is that of its argument.
            // Currently we only support a single compound selector as the argument,
            // so we sum the specificities of the inner simple selectors.
            // TODO: When selector list support is added (e.g. :not(.a, #b)),
            // use the *maximum* specificity among the list items instead of the sum.
            let inner_specificity = inner.iter().fold(Specificity::zero(), |mut acc, s| {
                add_simple_specificity(&mut acc, s);
                acc
            });
            value.ids += inner_specificity.ids;
            value.classes += inner_specificity.classes;
            value.elements += inner_specificity.elements;
        }
    }
}

fn matches_selector_part(
    node: &NodeHandle,
    selector: &Selector,
    index: usize,
    pseudo: Option<PseudoElement>,
) -> bool {
    let part = &selector.parts[index];
    if !matches_compound(node, part, pseudo) {
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
            let mut ancestor = node.parent_node();
            while let Some(parent) = ancestor {
                if matches_selector_part(&parent, selector, index - 1, None) {
                    return true;
                }
                ancestor = parent.parent_node();
            }
            false
        }
        Combinator::Child => node
            .parent_node()
            .is_some_and(|parent| matches_selector_part(&parent, selector, index - 1, None)),
        Combinator::AdjacentSibling => previous_element_sibling(node)
            .is_some_and(|sibling| matches_selector_part(&sibling, selector, index - 1, None)),
        Combinator::GeneralSibling => {
            let mut sibling = previous_element_sibling(node);
            while let Some(current) = sibling {
                if matches_selector_part(&current, selector, index - 1, None) {
                    return true;
                }
                sibling = previous_element_sibling(&current);
            }
            false
        }
    }
}

fn matches_compound(node: &NodeHandle, part: &SelectorPart, pseudo: Option<PseudoElement>) -> bool {
    part.simples
        .iter()
        .all(|simple| matches_simple_selector(node, simple, pseudo))
}

fn matches_simple_selector(
    node: &NodeHandle,
    simple: &SimpleSelector,
    pseudo: Option<PseudoElement>,
) -> bool {
    match simple {
        SimpleSelector::Type(name) => node
            .tag_name()
            .map(|tag_name| tag_name.eq_ignore_ascii_case(name))
            .unwrap_or(false),
        SimpleSelector::Universal => node.node_type() == NodeType::Element,
        SimpleSelector::Class(class_name) => get_attribute(node, "class")
            .map(|class_attr| {
                class_attr
                    .split_ascii_whitespace()
                    .any(|class| class == class_name)
            })
            .unwrap_or(false),
        SimpleSelector::Id(id) => get_attribute(node, "id")
            .map(|actual| actual == *id)
            .unwrap_or(false),
        SimpleSelector::Attribute {
            name,
            operator,
            value,
        } => matches_attribute_selector(node, name, *operator, value.as_deref()),
        SimpleSelector::PseudoClass(name) => matches_pseudo_class(node, name, pseudo),
        SimpleSelector::PseudoElement(name) => matches_pseudo_element(name, pseudo),
        SimpleSelector::Not(inner) => {
            // CSS :not() negates the entire compound argument.
            // The node must not match ALL of the inner simple selectors simultaneously.
            !inner
                .iter()
                .all(|s| matches_simple_selector(node, s, pseudo))
        }
    }
}

fn matches_attribute_selector(
    node: &NodeHandle,
    name: &str,
    operator: Option<AttributeOperator>,
    value: Option<&str>,
) -> bool {
    let Some(actual) = get_attribute(node, name) else {
        return false;
    };

    match operator {
        None => true,
        Some(AttributeOperator::Equals) => value.is_some_and(|expected| actual == expected),
        Some(AttributeOperator::Includes) => value.is_some_and(|expected| {
            actual
                .split_ascii_whitespace()
                .any(|token| token == expected)
        }),
        Some(AttributeOperator::StartsWith) => {
            value.is_some_and(|expected| actual.starts_with(expected))
        }
        Some(AttributeOperator::EndsWith) => {
            value.is_some_and(|expected| actual.ends_with(expected))
        }
        Some(AttributeOperator::Contains) => {
            value.is_some_and(|expected| actual.contains(expected))
        }
        Some(AttributeOperator::DashMatch) => value.is_some_and(|expected| {
            !expected.is_empty()
                && (actual == expected || actual.starts_with(&format!("{expected}-")))
        }),
    }
}

fn matches_pseudo_class(node: &NodeHandle, name: &str, pseudo: Option<PseudoElement>) -> bool {
    if let Some(argument) = name
        .strip_prefix("nth-child(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return matches_nth_child(node, argument.trim());
    }

    match name {
        "before" => pseudo == Some(PseudoElement::Before),
        "after" => pseudo == Some(PseudoElement::After),
        "root" => node
            .parent_node()
            .is_some_and(|parent| parent.node_type() == NodeType::Document),
        "first-child" => element_index_in_parent(node) == Some(1),
        "last-child" => {
            let Some((index, total)) = element_position(node) else {
                return false;
            };
            index == total
        }
        _ => false,
    }
}

fn matches_pseudo_element(name: &str, pseudo: Option<PseudoElement>) -> bool {
    match name {
        "before" => pseudo == Some(PseudoElement::Before),
        "after" => pseudo == Some(PseudoElement::After),
        _ => false,
    }
}

fn matches_nth_child(node: &NodeHandle, expression: &str) -> bool {
    let Some(index) = element_index_in_parent(node) else {
        return false;
    };

    if expression.eq_ignore_ascii_case("odd") {
        return index % 2 == 1;
    }
    if expression.eq_ignore_ascii_case("even") {
        return index % 2 == 0;
    }

    if let Ok(number) = expression.parse::<usize>() {
        return index == number;
    }

    false
}

fn get_attribute(node: &NodeHandle, name: &str) -> Option<String> {
    node.attributes().and_then(|attrs| attrs.get(name).cloned())
}

fn previous_element_sibling(node: &NodeHandle) -> Option<NodeHandle> {
    let parent = node.parent_node()?;
    let siblings = parent.child_nodes();
    let index = siblings.iter().position(|candidate| candidate == node)?;
    siblings[..index]
        .iter()
        .rev()
        .find(|candidate| candidate.node_type() == NodeType::Element)
        .cloned()
}

fn element_index_in_parent(node: &NodeHandle) -> Option<usize> {
    let (index, _) = element_position(node)?;
    Some(index)
}

fn element_position(node: &NodeHandle) -> Option<(usize, usize)> {
    let parent = node.parent_node()?;
    let element_children: Vec<NodeHandle> = parent
        .child_nodes()
        .into_iter()
        .filter(|child| child.node_type() == NodeType::Element)
        .collect();
    let total = element_children.len();
    let index = element_children
        .iter()
        .position(|candidate| candidate == node)
        .map(|position| position + 1)?;
    Some((index, total))
}

#[cfg(test)]
mod tests {
    use crate::css::parse_stylesheet;
    use crate::dom::NodeHandle;

    use super::*;

    fn selector(css: &str) -> Selector {
        let stylesheet = parse_stylesheet(css).unwrap();
        let super::super::Rule::Style(rule) = &stylesheet.rules[0] else {
            panic!("expected style rule");
        };
        rule.selectors[0].clone()
    }

    fn sample_tree() -> (
        NodeHandle,
        NodeHandle,
        NodeHandle,
        NodeHandle,
        NodeHandle,
        NodeHandle,
        NodeHandle,
    ) {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let body = NodeHandle::element("body");
        let main = NodeHandle::element("main");
        let lead = NodeHandle::element("p");
        let title = NodeHandle::element("h1");
        let cta = NodeHandle::element("a");

        main.set_attribute("id", "app");
        main.set_attribute("class", "hero primary");
        lead.set_attribute("class", "lead primary");
        lead.set_attribute("data-kind", "intro hero");
        cta.set_attribute("class", "button");

        document.append_child(html.clone());
        html.append_child(body.clone());
        body.append_child(main.clone());
        main.append_child(title.clone());
        main.append_child(lead.clone());
        main.append_child(cta.clone());

        (document, html, body, main, lead, title, cta)
    }

    #[test]
    fn matches_simple_selectors() {
        let (_, _, _, main, lead, _, _) = sample_tree();

        assert!(matches_selector(&main, &selector("main {}")));
        assert!(matches_selector(&main, &selector("#app {}")));
        assert!(matches_selector(&lead, &selector(".lead {}")));
        assert!(matches_selector(&lead, &selector("* {}")));
        assert!(!matches_selector(&lead, &selector("section {}")));
    }

    #[test]
    fn matches_attribute_selectors() {
        let (_, _, _, _, lead, _, _) = sample_tree();

        assert!(matches_selector(&lead, &selector("[data-kind] {}")));
        assert!(matches_selector(
            &lead,
            &selector(r#"[data-kind="intro hero"] {}"#)
        ));
        assert!(matches_selector(&lead, &selector("[data-kind~=hero] {}")));
        assert!(!matches_selector(
            &lead,
            &selector("[data-kind~=missing] {}")
        ));
    }

    #[test]
    fn matches_pseudo_classes() {
        let (_document, html, _body, _main, lead, title, cta) = sample_tree();
        let first_child = selector(":first-child {}");
        assert_eq!(
            first_child.parts[0].simples,
            vec![SimpleSelector::PseudoClass("first-child".to_string())]
        );

        assert!(matches_selector(&html, &selector(":root {}")));
        assert!(matches_selector(&title, &first_child));
        assert!(matches_selector(&cta, &selector(":last-child {}")));
        assert!(matches_selector(&lead, &selector(":nth-child(2) {}")));
        assert!(matches_selector(&title, &selector(":nth-child(odd) {}")));
        assert!(matches_selector(&lead, &selector(":nth-child(even) {}")));
        assert!(!matches_selector(&lead, &selector(":first-child {}")));
    }

    #[test]
    fn matches_combinators_right_to_left() {
        let (_, _, body, main, lead, title, cta) = sample_tree();

        assert!(matches_selector(&main, &selector("body > main {}")));
        assert!(matches_selector(&lead, &selector("main p {}")));
        assert!(matches_selector(&lead, &selector("h1 + p {}")));
        assert!(matches_selector(&cta, &selector("h1 ~ a {}")));
        assert!(!matches_selector(&title, &selector("body > p {}")));
        assert!(!matches_selector(&body, &selector("html + body {}")));
    }

    #[test]
    fn computes_specificity() {
        let value = specificity(&selector("main#app.hero[data-kind]:first-child::before {}"));
        assert_eq!(
            value,
            Specificity {
                ids: 1,
                classes: 3,
                elements: 2,
            }
        );
    }

    #[test]
    fn legacy_single_colon_before_is_treated_as_pseudo_element() {
        let (_, _, _, main, _, _, _) = sample_tree();
        let selector = selector("main:before {}");

        assert_eq!(
            selector_pseudo_element(&selector),
            Some(PseudoElement::Before)
        );
        assert!(matches_selector_with_pseudo(
            &main,
            &selector,
            Some(PseudoElement::Before)
        ));
        assert!(!matches_selector(&main, &selector));

        let value = specificity(&selector);
        assert_eq!(
            value,
            Specificity {
                ids: 0,
                classes: 0,
                elements: 2,
            }
        );
    }

    #[test]
    fn matches_attribute_selector_starts_with() {
        let (_, _, _, _, lead, _, _) = sample_tree();

        // data-kind="intro hero" → starts with "intro"
        assert!(matches_selector(&lead, &selector("[data-kind^=intro] {}")));
        assert!(!matches_selector(&lead, &selector("[data-kind^=hero] {}")));
    }

    #[test]
    fn matches_attribute_selector_ends_with() {
        let (_, _, _, _, lead, _, _) = sample_tree();

        // data-kind="intro hero" → ends with "hero"
        assert!(matches_selector(&lead, &selector("[data-kind$=hero] {}")));
        assert!(!matches_selector(&lead, &selector("[data-kind$=intro] {}")));
    }

    #[test]
    fn matches_attribute_selector_contains() {
        let (_, _, _, _, lead, _, _) = sample_tree();

        // data-kind="intro hero" → contains "ro h"
        assert!(matches_selector(&lead, &selector("[data-kind*=intro] {}")));
        assert!(matches_selector(&lead, &selector(r#"[data-kind*="ro h"] {}"#)));
        assert!(!matches_selector(&lead, &selector("[data-kind*=missing] {}")));
    }

    #[test]
    fn matches_attribute_selector_dash_match() {
        let document = NodeHandle::document();
        let html = NodeHandle::element("html");
        let en = NodeHandle::element("p");
        let en_us = NodeHandle::element("p");
        let fr = NodeHandle::element("p");

        en.set_attribute("lang", "en");
        en_us.set_attribute("lang", "en-US");
        fr.set_attribute("lang", "fr");

        document.append_child(html.clone());
        html.append_child(en.clone());
        html.append_child(en_us.clone());
        html.append_child(fr.clone());

        // [lang|=en] should match lang="en" and lang="en-US" but not lang="fr"
        assert!(matches_selector(&en, &selector("[lang|=en] {}")));
        assert!(matches_selector(&en_us, &selector("[lang|=en] {}")));
        assert!(!matches_selector(&fr, &selector("[lang|=en] {}")));
    }

    #[test]
    fn matches_not_pseudo_class() {
        let (_, _, _, main, lead, title, cta) = sample_tree();

        // :not(p) matches elements that are not <p>
        assert!(matches_selector(&main, &selector(":not(p) {}")));
        assert!(matches_selector(&title, &selector(":not(p) {}")));
        assert!(!matches_selector(&lead, &selector(":not(p) {}")));

        // :not(.button) matches elements without class "button"
        assert!(matches_selector(&main, &selector(":not(.button) {}")));
        assert!(!matches_selector(&cta, &selector(":not(.button) {}")));

        // :not(#app) matches elements without id "app"
        assert!(!matches_selector(&main, &selector(":not(#app) {}")));
        assert!(matches_selector(&lead, &selector(":not(#app) {}")));
    }

    #[test]
    fn not_selector_specificity() {
        // :not() specificity counts the inner selector's specificity
        let value = specificity(&selector(":not(p) {}"));
        assert_eq!(
            value,
            Specificity {
                ids: 0,
                classes: 0,
                elements: 1,
            }
        );

        let value = specificity(&selector(":not(.foo) {}"));
        assert_eq!(
            value,
            Specificity {
                ids: 0,
                classes: 1,
                elements: 0,
            }
        );
    }
}
