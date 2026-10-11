//! DOM counts for tree-scoped CSS integer functions.

use super::Value;
use crate::dom::{Node, NodeHandle, NodeType};

/// An owned snapshot for a single element and declaration tree scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SiblingCounts {
    pub(super) index: usize,
    pub(super) count: usize,
}

impl SiblingCounts {
    /// Counts DOM elements only, after loosely matching the declaration scope.
    /// The ancestry lists the declaration tree and its inclusive ancestor trees;
    /// `None` identifies the document tree.
    /// Callers pass the originating element for a non-element pseudo-element.
    pub(super) fn for_element(
        element: &NodeHandle,
        declaration_scope_ancestry: &[Option<usize>],
    ) -> Self {
        if !scope_matches(element, declaration_scope_ancestry) {
            return Self { index: 0, count: 0 };
        }
        let Some(parent) = element.parent_node() else {
            return Self { index: 1, count: 1 };
        };
        let mut counts = Self { index: 0, count: 0 };
        for sibling in parent.child_nodes() {
            if sibling.node_type() != NodeType::Element {
                continue;
            }
            counts.count += 1;
            if sibling.identity() == element.identity() {
                counts.index = counts.count;
            }
        }
        counts
    }
}

/// A scope inside the element's tree may refer outward to that element. A
/// declaration in an outer tree cannot observe a descendant shadow tree.
fn scope_matches(element: &NodeHandle, declaration_scope_ancestry: &[Option<usize>]) -> bool {
    let element_scope = element.containing_shadow_root().map(|root| root.identity());
    declaration_scope_ancestry.contains(&element_scope)
}

/// Produces owned scope IDs without retaining a borrowed DOM owner.
pub(super) fn scope_ancestry(root: Option<&NodeHandle>) -> Vec<Option<usize>> {
    let mut ancestry = Vec::new();
    let mut current = root.cloned();
    while let Some(root) = current {
        ancestry.push(Some(root.identity()));
        current = root
            .shadow_host()
            .and_then(|host| host.containing_shadow_root());
    }
    ancestry.push(None);
    ancestry
}

/// Attaches declaration scope before variable substitution can move tokens.
pub(super) fn annotate(value: &Value, ancestry: &[Option<usize>]) -> Value {
    match value {
        Value::Function { name, arguments }
            if arguments.is_empty()
                && (name.eq_ignore_ascii_case("sibling-index")
                    || name.eq_ignore_ascii_case("sibling-count")) =>
        {
            Value::TreeCounting {
                index: name.eq_ignore_ascii_case("sibling-index"),
                scope_ancestry: ancestry.to_vec(),
            }
        }
        Value::Function { name, arguments } => Value::Function {
            name: name.clone(),
            arguments: arguments
                .iter()
                .map(|value| annotate(value, ancestry))
                .collect(),
        },
        Value::List(values) => Value::List(
            values
                .iter()
                .map(|value| annotate(value, ancestry))
                .collect(),
        ),
        Value::CommaList(values) => Value::CommaList(
            values
                .iter()
                .map(|value| annotate(value, ancestry))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Resolves only annotated functions, using this element's current DOM tree.
pub(super) fn resolve(value: &Value, element: &NodeHandle) -> Value {
    match value {
        Value::TreeCounting {
            index,
            scope_ancestry,
        } => {
            let counts = SiblingCounts::for_element(element, scope_ancestry);
            Value::Number(if *index { counts.index } else { counts.count } as f32)
        }
        Value::Function { name, arguments } => Value::Function {
            name: name.clone(),
            arguments: arguments
                .iter()
                .map(|value| resolve(value, element))
                .collect(),
        },
        Value::List(values) => {
            Value::List(values.iter().map(|value| resolve(value, element)).collect())
        }
        Value::CommaList(values) => {
            Value::CommaList(values.iter().map(|value| resolve(value, element)).collect())
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::ShadowRootMode;

    #[test]
    fn counts_only_elements_and_observes_current_dom_order() {
        let parent = NodeHandle::element("div");
        let first = NodeHandle::element("span");
        let target = NodeHandle::element("span");
        parent.append_child(first);
        parent.append_child(NodeHandle::text("text"));
        parent.append_child(NodeHandle::comment("comment"));
        parent.append_child(target.clone());
        assert_eq!(
            SiblingCounts::for_element(&target, &[None]),
            SiblingCounts { index: 2, count: 2 }
        );
        let inserted = NodeHandle::element("span");
        parent.insert_before(inserted.clone(), &target).unwrap();
        assert_eq!(
            SiblingCounts::for_element(&target, &[None]),
            SiblingCounts { index: 3, count: 3 }
        );
        parent.remove_child(&inserted).unwrap();
        assert_eq!(
            SiblingCounts::for_element(&target, &[None]),
            SiblingCounts { index: 2, count: 2 }
        );
    }

    #[test]
    fn shadow_declarations_can_refer_outward_but_outer_declarations_cannot_count_parts() {
        let parent = NodeHandle::element("div");
        parent.append_child(NodeHandle::element("span"));
        let host = NodeHandle::element("section");
        parent.append_child(host.clone());
        let root = host.attach_shadow(ShadowRootMode::Open).unwrap();
        let part = NodeHandle::element("span");
        root.append_child(part.clone());
        let ancestry = scope_ancestry(Some(&root));
        assert_eq!(
            SiblingCounts::for_element(&host, &ancestry),
            SiblingCounts { index: 2, count: 2 }
        );
        assert_eq!(
            SiblingCounts::for_element(&part, &ancestry),
            SiblingCounts { index: 1, count: 1 }
        );
        assert_eq!(
            SiblingCounts::for_element(&part, &[None]),
            SiblingCounts { index: 0, count: 0 }
        );
    }

    #[test]
    fn live_animation_recomputes_inherited_counts_without_resetting_its_timeline() {
        use crate::css::{ComputedValue, Origin, StyleResolver, parse_stylesheet};

        let parent = NodeHandle::element("div");
        parent.set_attribute("style", "--amount:calc(sibling-index() * 10px)");
        let first = NodeHandle::element("span");
        parent.append_child(first.clone());
        parent.append_child(NodeHandle::element("span"));
        let target = NodeHandle::element("div");
        target.set_attribute("id", "target");
        parent.append_child(target.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@keyframes count{from{width:var(--amount)}to{width:100px}}\
                 #target{animation:count 1000s step-end}",
            )
            .unwrap(),
        );
        resolver.set_animation_time_ms(0.0);
        assert_eq!(
            resolver.computed_style(&target).get("width"),
            Some(&ComputedValue::Px(30.0))
        );
        parent.remove_child(&first).unwrap();
        resolver.invalidate_style_cache_for_test();
        resolver.set_animation_time_ms(10.0);
        assert_eq!(
            resolver.computed_style(&target).get("width"),
            Some(&ComputedValue::Px(20.0))
        );
    }

    #[test]
    fn live_registered_animation_recomposes_aliases_and_inherited_dependants() {
        use crate::css::{ComputedValue, Origin, StyleResolver, parse_stylesheet};

        let parent = NodeHandle::element("div");
        let first = NodeHandle::element("span");
        parent.append_child(first.clone());
        parent.append_child(NodeHandle::element("span"));
        let target = NodeHandle::element("div");
        target.set_attribute("id", "target");
        parent.append_child(target.clone());
        let child = NodeHandle::element("div");
        child.set_attribute("id", "child");
        target.append_child(child.clone());
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(
            Origin::Author,
            parse_stylesheet(
                "@property --size{syntax:'<length>';inherits:true;initial-value:0px}\
             @keyframes count{from{--size:calc(sibling-index() * 10px)}to{--size:100px}}\
             #target{animation:count 1000s step-end;width:var(--size);\
             --alias:var(--size);height:var(--alias)}#child{width:var(--size)}",
            )
            .unwrap(),
        );
        resolver.set_animation_time_ms(0.0);
        let before = resolver.computed_style(&target);
        assert_eq!(before.get("width"), Some(&ComputedValue::Px(30.0)));
        assert_eq!(before.get("height"), Some(&ComputedValue::Px(30.0)));
        assert_eq!(
            resolver.computed_style(&child).get("width"),
            Some(&ComputedValue::Px(30.0))
        );
        parent.remove_child(&first).unwrap();
        resolver.invalidate_style_cache_for_test();
        resolver.set_animation_time_ms(10.0);
        let after = resolver.computed_style(&target);
        assert_eq!(after.get("width"), Some(&ComputedValue::Px(20.0)));
        assert_eq!(after.get("height"), Some(&ComputedValue::Px(20.0)));
        assert_eq!(
            resolver.computed_style(&child).get("width"),
            Some(&ComputedValue::Px(20.0))
        );
    }

    #[test]
    fn live_untyped_animation_keeps_tokens_and_counts_each_inheriting_element() {
        use crate::css::{ComputedValue, Origin, StyleResolver, parse_stylesheet};

        for registration in [
            "",
            "@property --raw{syntax:'*';inherits:true;initial-value:0}",
        ] {
            let parent = NodeHandle::element("div");
            let first = NodeHandle::element("span");
            parent.append_child(first.clone());
            parent.append_child(NodeHandle::element("span"));
            let target = NodeHandle::element("div");
            target.set_attribute("id", "target");
            parent.append_child(target.clone());
            let child = NodeHandle::element("div");
            child.set_attribute("id", "child");
            target.append_child(child.clone());
            let mut resolver = StyleResolver::new();
            let source = format!(
                "{registration}\
                @keyframes count{{from{{--raw:sibling-index()}}to{{--raw:sibling-count()}}}}\
                #target{{animation:count 1000s step-end;flex-grow:var(--raw)}}\
                #child{{flex-grow:var(--raw)}}"
            );
            resolver.add_stylesheet(Origin::Author, parse_stylesheet(&source).unwrap());
            resolver.set_animation_time_ms(0.0);
            let before = resolver.computed_style(&target);
            assert_eq!(
                before.get("--raw"),
                Some(&ComputedValue::Keyword("sibling-index()".into()))
            );
            assert_eq!(before.get("flex-grow"), Some(&ComputedValue::Number(3.0)));
            assert_eq!(
                resolver.computed_style(&child).get("flex-grow"),
                Some(&ComputedValue::Number(1.0))
            );
            parent.remove_child(&first).unwrap();
            resolver.invalidate_style_cache_for_test();
            resolver.set_animation_time_ms(10.0);
            let after = resolver.computed_style(&target);
            assert_eq!(
                after.get("--raw"),
                Some(&ComputedValue::Keyword("sibling-index()".into()))
            );
            assert_eq!(after.get("flex-grow"), Some(&ComputedValue::Number(2.0)));
            assert_eq!(
                resolver.computed_style(&child).get("flex-grow"),
                Some(&ComputedValue::Number(1.0))
            );
        }
    }

    #[test]
    fn annotation_preserves_function_tokens_until_evaluation() {
        let value = Value::Function {
            name: "sibling-index".into(),
            arguments: Vec::new(),
        };
        let annotated = annotate(&value, &[None]);
        assert_eq!(super::super::render_value(&annotated), "sibling-index()");
        assert_eq!(
            resolve(&annotated, &NodeHandle::element("div")),
            Value::Number(1.0)
        );
        let invalid = Value::Function {
            name: "sibling-index".into(),
            arguments: vec![Value::Number(1.0)],
        };
        assert_eq!(annotate(&invalid, &[None]), invalid);
    }
}

pub(super) fn contains(value: &Value) -> bool {
    match value {
        Value::TreeCounting { .. } => true,
        Value::Function { name, arguments } => {
            name.eq_ignore_ascii_case("sibling-index")
                || name.eq_ignore_ascii_case("sibling-count")
                || arguments.iter().any(contains)
        }
        Value::List(values) | Value::CommaList(values) => values.iter().any(contains),
        _ => false,
    }
}

/// Checks zero-argument invocations without evaluating an element's count.
pub(super) fn valid_arguments(value: &Value) -> bool {
    match value {
        Value::Function { name, arguments }
            if name.eq_ignore_ascii_case("sibling-index")
                || name.eq_ignore_ascii_case("sibling-count") =>
        {
            arguments.is_empty()
        }
        Value::Function { arguments, .. } => arguments.iter().all(valid_arguments),
        Value::List(values) | Value::CommaList(values) => values.iter().all(valid_arguments),
        _ => true,
    }
}
