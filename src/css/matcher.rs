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
            let Some(parent) = node.parent_node() else {
                return false;
            };
            let siblings = parent.child_nodes();
            let Some(position) = siblings.iter().position(|candidate| candidate == node) else {
                return false;
            };
            siblings[..position].iter().rev().any(|sibling| {
                sibling.node_type() == NodeType::Element
                    && matches_selector_part(sibling, selector, index - 1, None)
            })
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
    if let Some((function, argument)) = functional_pseudo(name) {
        // Pseudo-class names are ASCII case-insensitive; the argument keeps
        // its original case (`:lang()` compares case-insensitively itself and
        // an+b parsing lowercases internally).
        let function = function.to_ascii_lowercase();
        return match function.as_str() {
            "nth-child" => matches_nth_child(node, argument),
            "nth-last-child" => element_position(node)
                .is_some_and(|(index, total)| {
                    parse_an_plus_b(argument).is_some_and(|formula| formula.matches(total - index + 1))
                }),
            "nth-of-type" => type_position(node).is_some_and(|(index, _)| {
                parse_an_plus_b(argument).is_some_and(|formula| formula.matches(index))
            }),
            "nth-last-of-type" => type_position(node).is_some_and(|(index, total)| {
                parse_an_plus_b(argument).is_some_and(|formula| formula.matches(total - index + 1))
            }),
            "lang" => matches_language(node, argument),
            _ => false,
        };
    }

    let name = name.to_ascii_lowercase();
    match name.as_str() {
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
        "only-child" => element_position(node).is_some_and(|(_, total)| total == 1),
        "first-of-type" => type_position(node).is_some_and(|(index, _)| index == 1),
        "last-of-type" => type_position(node).is_some_and(|(index, total)| index == total),
        "only-of-type" => type_position(node).is_some_and(|(_, total)| total == 1),
        "enabled" => is_form_control(node) && get_attribute(node, "disabled").is_none(),
        "disabled" => is_form_control(node) && get_attribute(node, "disabled").is_some(),
        "checked" => node.checked(),
        "empty" => node.child_nodes().into_iter().all(|child| match child.node_type() {
            NodeType::Element => false,
            NodeType::Text => child.data().is_some_and(|data| data.is_empty()),
            _ => true,
        }),
        _ => false,
    }
}

fn is_form_control(node: &NodeHandle) -> bool {
    node.tag_name().is_some_and(|tag| {
        matches!(
            tag.as_str(),
            "button" | "input" | "select" | "textarea" | "option" | "optgroup" | "fieldset"
        )
    })
}

fn matches_language(node: &NodeHandle, range: &str) -> bool {
    let range = range.trim().trim_matches(['\'', '"']);
    if range.is_empty() {
        return false;
    }
    let mut current = Some(node.clone());
    while let Some(element) = current {
        if let Some(language) = get_attribute(&element, "lang") {
            return language.eq_ignore_ascii_case(range)
                || language
                    .get(..range.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(range))
                    && language.as_bytes().get(range.len()) == Some(&b'-');
        }
        current = element.parent_node();
    }
    false
}

fn functional_pseudo(name: &str) -> Option<(&str, &str)> {
    let open = name.find('(')?;
    let function = &name[..open];
    let argument = name[open + 1..].strip_suffix(')')?.trim();
    Some((function, argument))
}

fn matches_pseudo_element(name: &str, pseudo: Option<PseudoElement>) -> bool {
    // Pseudo-element names are ASCII case-insensitive (`::BEFORE`).
    match name.to_ascii_lowercase().as_str() {
        "before" => pseudo == Some(PseudoElement::Before),
        "after" => pseudo == Some(PseudoElement::After),
        _ => false,
    }
}

fn matches_nth_child(node: &NodeHandle, expression: &str) -> bool {
    let Some(index) = element_index_in_parent(node) else {
        return false;
    };

    parse_an_plus_b(expression).is_some_and(|formula| formula.matches(index))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AnPlusB {
    a: i64,
    b: i64,
}

impl AnPlusB {
    fn matches(self, position: usize) -> bool {
        let Ok(position) = i64::try_from(position) else {
            return false;
        };
        if position <= 0 {
            return false;
        }
        if self.a == 0 {
            return position == self.b;
        }
        let difference = position.checked_sub(self.b);
        difference.is_some_and(|difference| difference % self.a == 0 && difference / self.a >= 0)
    }
}

pub(super) fn parse_an_plus_b(expression: &str) -> Option<AnPlusB> {
    let compact: String = expression
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    match compact.as_str() {
        "odd" => return Some(AnPlusB { a: 2, b: 1 }),
        "even" => return Some(AnPlusB { a: 2, b: 0 }),
        _ => {}
    }

    if let Ok(b) = compact.parse::<i64>() {
        return Some(AnPlusB { a: 0, b });
    }

    let n = compact.find('n')?;
    if compact[n + 1..].contains('n') {
        return None;
    }
    let coefficient = &compact[..n];
    let a = match coefficient {
        "" | "+" => 1,
        "-" => -1,
        value => value.parse::<i64>().ok()?,
    };
    let remainder = &compact[n + 1..];
    let b = if remainder.is_empty() {
        0
    } else {
        if !remainder.starts_with(['+', '-']) {
            return None;
        }
        remainder.parse::<i64>().ok()?
    };
    Some(AnPlusB { a, b })
}

fn get_attribute(node: &NodeHandle, name: &str) -> Option<String> {
    node.get_attribute(name)
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
    // Tree-structural pseudo-classes (`:first-child`, `:last-child`,
    // `:nth-child`) are defined in terms of an element being a child of *some
    // other element*. A root element whose parent is the `Document` node (or a
    // `DocumentFragment`) has no element parent and therefore has no sibling
    // position: it must never match `:first-child`/`:last-child`/`:nth-child`.
    // Without this guard the document element would report position `1 of 1`
    // and wrongly claim to be a `:first-child` (Acid3 test 35).
    if parent.node_type() != NodeType::Element {
        return None;
    }
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

fn type_position(node: &NodeHandle) -> Option<(usize, usize)> {
    let parent = node.parent_node()?;
    if parent.node_type() != NodeType::Element {
        return None;
    }
    let tag_name = node.tag_name()?;
    let same_type: Vec<NodeHandle> = parent
        .child_nodes()
        .into_iter()
        .filter(|child| {
            child
                .tag_name()
                .is_some_and(|tag| tag.eq_ignore_ascii_case(&tag_name))
        })
        .collect();
    let total = same_type.len();
    let index = same_type.iter().position(|candidate| candidate == node)? + 1;
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
    fn pseudo_class_names_are_ascii_case_insensitive() {
        // Named bindings keep the whole tree alive: a bare `_` would drop the
        // ancestor handles and sever the children's weak parent links.
        let (_document, _html, _body, _main, lead, title, _cta) = sample_tree();

        assert!(
            matches_selector(&title, &selector(":FIRST-CHILD {}")),
            ":FIRST-CHILD must match like :first-child"
        );
        assert!(
            matches_selector(&title, &selector(":NTH-CHILD(ODD) {}")),
            ":NTH-CHILD(ODD) must match like :nth-child(odd)"
        );
        assert!(
            matches_selector(&lead, &selector(":Nth-Child(EVEN) {}")),
            ":Nth-Child(EVEN) must match like :nth-child(even)"
        );
        assert!(
            !matches_selector(&lead, &selector(":FIRST-CHILD {}")),
            "case-insensitive names must not loosen matching itself"
        );
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
    fn parses_and_evaluates_general_an_plus_b_expressions() {
        let cases = [
            ("odd", AnPlusB { a: 2, b: 1 }),
            (" EVEN ", AnPlusB { a: 2, b: 0 }),
            ("5", AnPlusB { a: 0, b: 5 }),
            ("n", AnPlusB { a: 1, b: 0 }),
            ("-n", AnPlusB { a: -1, b: 0 }),
            ("n + 3", AnPlusB { a: 1, b: 3 }),
            ("-n+3", AnPlusB { a: -1, b: 3 }),
            ("3n-1", AnPlusB { a: 3, b: -1 }),
            ("-5n+3", AnPlusB { a: -5, b: 3 }),
            ("0n+3", AnPlusB { a: 0, b: 3 }),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_an_plus_b(input), Some(expected), "{input}");
        }
        for invalid in ["", "n+", "2n 3", "2nn+1", "--n", "infinite"] {
            assert_eq!(parse_an_plus_b(invalid), None, "{invalid}");
        }

        assert!(parse_an_plus_b("-n+3").unwrap().matches(1));
        assert!(parse_an_plus_b("-n+3").unwrap().matches(3));
        assert!(!parse_an_plus_b("-n+3").unwrap().matches(4));
        assert!(parse_an_plus_b("3n-1").unwrap().matches(2));
        assert!(parse_an_plus_b("3n-1").unwrap().matches(5));
        assert!(!parse_an_plus_b("3n-1").unwrap().matches(3));
        assert!(parse_an_plus_b("0n+3").unwrap().matches(3));
    }

    #[test]
    fn child_structural_pseudos_follow_current_element_siblings() {
        let parent = NodeHandle::element("div");
        let first = NodeHandle::element("p");
        let second = NodeHandle::element("p");
        let third = NodeHandle::element("p");
        parent.append_child(NodeHandle::text("ignored"));
        parent.append_child(first.clone());
        parent.append_child(NodeHandle::comment("ignored"));
        assert!(matches_selector(&first, &selector(":only-child {}")));
        parent.append_child(second.clone());
        parent.append_child(third.clone());

        assert!(!matches_selector(&first, &selector(":only-child {}")));
        assert!(matches_selector(&first, &selector(":nth-child(-n+3) {}")));
        assert!(matches_selector(&third, &selector(":nth-last-child(1) {}")));
        assert!(matches_selector(&second, &selector(":nth-last-child(even) {}")));
        parent.remove_child(&first).unwrap();
        assert!(matches_selector(&second, &selector(":first-child {}")));
    }

    #[test]
    fn empty_ignores_comments_and_empty_text_but_not_content() {
        let element = NodeHandle::element("div");
        element.append_child(NodeHandle::comment("comment"));
        element.append_child(NodeHandle::text(""));
        assert!(matches_selector(&element, &selector(":empty {}")));

        let whitespace = NodeHandle::text(" ");
        element.append_child(whitespace.clone());
        assert!(!matches_selector(&element, &selector(":empty {}")));
        element.remove_child(&whitespace).unwrap();
        element.append_child(NodeHandle::element("span"));
        assert!(!matches_selector(&element, &selector(":empty {}")));
    }

    #[test]
    fn of_type_pseudos_count_only_matching_element_names() {
        let parent = NodeHandle::element("div");
        let mut paragraphs = Vec::new();
        for index in 0..8 {
            if index % 2 == 0 {
                parent.append_child(NodeHandle::element("span"));
            }
            let paragraph = NodeHandle::element("p");
            parent.append_child(paragraph.clone());
            paragraphs.push(paragraph);
        }

        assert!(matches_selector(&paragraphs[0], &selector(":first-of-type {}")));
        assert!(matches_selector(&paragraphs[7], &selector(":last-of-type {}")));
        assert_eq!(
            paragraphs
                .iter()
                .enumerate()
                .filter_map(|(i, node)| matches_selector(node, &selector(":nth-of-type(3n-1) {}"))
                    .then_some(i + 1))
                .collect::<Vec<_>>(),
            vec![2, 5, 8]
        );
        assert_eq!(
            paragraphs
                .iter()
                .enumerate()
                .filter_map(|(i, node)| matches_selector(node, &selector(":nth-last-of-type(-5n+3) {}"))
                    .then_some(i + 1))
                .collect::<Vec<_>>(),
            vec![6]
        );

        let unique = NodeHandle::element("em");
        parent.append_child(unique.clone());
        assert!(matches_selector(&unique, &selector(":only-of-type {}")));
    }

    #[test]
    fn lang_matches_inherited_exact_and_dash_prefixed_languages() {
        let outer = NodeHandle::element("section");
        let inherited = NodeHandle::element("p");
        let overridden = NodeHandle::element("span");
        let nested = NodeHandle::element("em");
        outer.set_attribute("lang", "EN-gb");
        overridden.set_attribute("lang", "english");
        outer.append_child(inherited.clone());
        outer.append_child(overridden.clone());
        overridden.append_child(nested.clone());

        assert!(matches_selector(&outer, &selector(":lang(en) {}")));
        assert!(matches_selector(&inherited, &selector(":lang(EN) {}")));
        assert!(matches_selector(&inherited, &selector(":lang(en-gb) {}")));
        assert!(!matches_selector(&overridden, &selector(":lang(en) {}")));
        assert!(!matches_selector(&nested, &selector(":lang(en) {}")));
    }

    #[test]
    fn form_state_pseudos_apply_to_eligible_controls() {
        let input = NodeHandle::element("input");
        input.set_attribute("type", "checkbox");
        let text = NodeHandle::element("input");
        text.set_attribute("type", "text");
        text.set_attribute("checked", "");
        let body = NodeHandle::element("body");

        assert!(matches_selector(&input, &selector(":enabled {}")));
        assert!(!matches_selector(&body, &selector(":enabled {}")));
        input.set_attribute("disabled", "");
        assert!(matches_selector(&input, &selector(":disabled {}")));
        assert!(!matches_selector(&input, &selector(":enabled {}")));
        input.set_checked(true);
        assert!(matches_selector(&input, &selector(":checked {}")));
        assert!(!matches_selector(&text, &selector(":checked {}")));
        input.set_attribute("type", "text");
        assert!(!matches_selector(&input, &selector(":checked {}")));
    }

    #[test]
    fn root_element_does_not_match_structural_child_pseudo_classes() {
        // `html`'s parent is the `Document` node, not an element. Per the CSS
        // Selectors spec these pseudo-classes only apply to a child of another
        // *element*, so the root element must not match even though it is the
        // sole child of the document. (Acid3 test 35 regression guard.)
        let (_document, html, _body, _main, _lead, _title, _cta) = sample_tree();

        assert!(
            !matches_selector(&html, &selector(":first-child {}")),
            "the root element has no element parent and must not be :first-child"
        );
        assert!(
            !matches_selector(&html, &selector(":last-child {}")),
            "the root element has no element parent and must not be :last-child"
        );
        assert!(
            !matches_selector(&html, &selector(":nth-child(1) {}")),
            "the root element has no sibling position and must not be :nth-child(1)"
        );
        // It is, however, still the :root element.
        assert!(matches_selector(&html, &selector(":root {}")));
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
