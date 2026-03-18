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

    matches_selector_part(node, selector, selector.parts.len() - 1)
}

/// Returns the pseudo-element targeted by `selector`, if any.
pub fn selector_pseudo_element(selector: &Selector) -> Option<PseudoElement> {
    let mut pseudo = None;
    for part in &selector.parts {
        for simple in &part.simples {
            if let SimpleSelector::PseudoElement(name) = simple {
                let current = match name.as_str() {
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
            match simple {
                SimpleSelector::Id(_) => value.ids += 1,
                SimpleSelector::Class(_)
                | SimpleSelector::Attribute { .. }
                | SimpleSelector::PseudoClass(_) => value.classes += 1,
                SimpleSelector::Type(_) | SimpleSelector::PseudoElement(_) => value.elements += 1,
                SimpleSelector::Universal => {}
            }
        }
    }

    value
}

fn matches_selector_part(node: &NodeHandle, selector: &Selector, index: usize) -> bool {
    let part = &selector.parts[index];
    if !matches_compound(node, part) {
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
                if matches_selector_part(&parent, selector, index - 1) {
                    return true;
                }
                ancestor = parent.parent_node();
            }
            false
        }
        Combinator::Child => node
            .parent_node()
            .is_some_and(|parent| matches_selector_part(&parent, selector, index - 1)),
        Combinator::AdjacentSibling => previous_element_sibling(node)
            .is_some_and(|sibling| matches_selector_part(&sibling, selector, index - 1)),
        Combinator::GeneralSibling => {
            let mut sibling = previous_element_sibling(node);
            while let Some(current) = sibling {
                if matches_selector_part(&current, selector, index - 1) {
                    return true;
                }
                sibling = previous_element_sibling(&current);
            }
            false
        }
    }
}

fn matches_compound(node: &NodeHandle, part: &SelectorPart) -> bool {
    part.simples
        .iter()
        .all(|simple| matches_simple_selector(node, simple))
}

fn matches_simple_selector(node: &NodeHandle, simple: &SimpleSelector) -> bool {
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
        SimpleSelector::PseudoClass(name) => matches_pseudo_class(node, name),
        SimpleSelector::PseudoElement(_) => true,
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
    }
}

fn matches_pseudo_class(node: &NodeHandle, name: &str) -> bool {
    if let Some(argument) = name
        .strip_prefix("nth-child(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return matches_nth_child(node, argument.trim());
    }

    match name {
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
        let (_document, _html, _body, _main, lead, title, cta) = sample_tree();
        let first_child = selector(":first-child {}");
        assert_eq!(
            first_child.parts[0].simples,
            vec![SimpleSelector::PseudoClass("first-child".to_string())]
        );

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
}
