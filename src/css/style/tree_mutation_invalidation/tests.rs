use super::*;
use crate::css::parse_stylesheet;

struct Fixture {
    html: NodeHandle,
    body: NodeHandle,
    parent: NodeHandle,
    first: NodeHandle,
    second: NodeHandle,
    grandchild: NodeHandle,
    observer: NodeHandle,
    observer_child: NodeHandle,
}

fn fixture() -> Fixture {
    let html = NodeHandle::element("html");
    let body = NodeHandle::element("body");
    html.append_child(body.clone());
    let parent = NodeHandle::element("main");
    let first = NodeHandle::element("div");
    first.set_attribute("class", "a");
    let second = NodeHandle::element("div");
    second.set_attribute("class", "b");
    let grandchild = NodeHandle::element("span");
    second.append_child(grandchild.clone());
    parent.append_child(first.clone());
    parent.append_child(second.clone());
    let observer = NodeHandle::element("aside");
    let observer_child = NodeHandle::element("p");
    observer.append_child(observer_child.clone());
    body.append_child(parent.clone());
    body.append_child(observer.clone());
    Fixture {
        html,
        body,
        parent,
        first,
        second,
        grandchild,
        observer,
        observer_child,
    }
}

impl Fixture {
    fn nodes(&self) -> Vec<NodeHandle> {
        vec![
            self.body.clone(),
            self.parent.clone(),
            self.first.clone(),
            self.second.clone(),
            self.grandchild.clone(),
            self.observer.clone(),
            self.observer_child.clone(),
        ]
    }
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Append,
    Remove,
    MoveIn,
}

/// Applies `mutation` and returns its changed parents and moved node.
fn mutate(fixture: &Fixture, mutation: Mutation) -> (Vec<NodeHandle>, NodeHandle) {
    match mutation {
        Mutation::Append => {
            let inserted = NodeHandle::element("div");
            inserted.set_attribute("class", "c");
            fixture.parent.append_child(inserted.clone());
            (vec![fixture.parent.clone()], inserted)
        }
        Mutation::Remove => {
            fixture.parent.remove_child(&fixture.first).unwrap();
            (vec![fixture.parent.clone()], fixture.first.clone())
        }
        Mutation::MoveIn => {
            fixture
                .parent
                .insert_before(fixture.observer_child.clone(), &fixture.first)
                .unwrap();
            (
                vec![fixture.parent.clone(), fixture.observer.clone()],
                fixture.observer_child.clone(),
            )
        }
    }
}

fn resolver(css: &str) -> StyleResolver {
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    resolver
}

#[test]
fn local_tree_invalidation_matches_a_fresh_cascade() {
    for selector in [
        "main:empty",
        "main:not(:empty) > div",
        "div:first-child",
        "div:last-child span",
        "div:only-child",
        "div:nth-child(2)",
        "div:nth-last-child(1) span",
        "div:nth-child(1 of .b)",
        "div:first-of-type",
        "div + div",
        ".a + .b span",
        ".a ~ div",
        "main > *",
        "aside:empty",
        "aside > p",
        "p:first-child",
    ] {
        for mutation in [Mutation::Append, Mutation::Remove, Mutation::MoveIn] {
            let fixture = fixture();
            let css = format!(
                "{selector}{{color:blue;width:37px}} \
                 {selector}::before{{content:'x';color:green}}"
            );
            let mut local = resolver(&css);
            for node in fixture.nodes() {
                local.computed_style_shared(&node);
                local.computed_pseudo_style_shared(&node, PseudoElement::Before);
            }
            let (parents, moved) = mutate(&fixture, mutation);
            let outcome = local.invalidate_tree_mutation_subtrees(&parents, &[moved.clone()]);
            assert_eq!(outcome, "local_tree_subtree", "{selector} {mutation:?}");
            let mut fresh = resolver(&css);
            let mut nodes = fixture.nodes();
            nodes.push(moved);
            for node in nodes {
                assert_eq!(
                    local.computed_style_shared(&node).properties(),
                    fresh.computed_style_shared(&node).properties(),
                    "{selector} {mutation:?}"
                );
                assert_eq!(
                    local
                        .computed_pseudo_style_shared(&node, PseudoElement::Before)
                        .map(|style| style.properties()),
                    fresh
                        .computed_pseudo_style_shared(&node, PseudoElement::Before)
                        .map(|style| style.properties()),
                    "{selector} {mutation:?} ::before"
                );
            }
        }
    }
}

#[test]
fn local_tree_invalidation_retains_styles_outside_the_changed_parent() {
    let fixture = fixture();
    let mut resolver = resolver("div{width:10px} aside{width:20px} p{color:red}");
    for node in fixture.nodes() {
        resolver.computed_style_shared(&node);
    }
    let observer = resolver.computed_style_shared(&fixture.observer);
    let observer_child = resolver.computed_style_shared(&fixture.observer_child);
    let body = resolver.computed_style_shared(&fixture.body);
    let second = resolver.computed_style_shared(&fixture.second);
    let (parents, moved) = mutate(&fixture, Mutation::Append);
    assert_eq!(
        resolver.invalidate_tree_mutation_subtrees(&parents, &[moved]),
        "local_tree_subtree"
    );
    assert!(Arc::ptr_eq(
        &observer,
        &resolver.computed_style_shared(&fixture.observer)
    ));
    assert!(Arc::ptr_eq(
        &observer_child,
        &resolver.computed_style_shared(&fixture.observer_child)
    ));
    assert!(Arc::ptr_eq(
        &body,
        &resolver.computed_style_shared(&fixture.body)
    ));
    assert!(
        !Arc::ptr_eq(&second, &resolver.computed_style_shared(&fixture.second)),
        "children of the changed parent must be recomputed"
    );
}

#[test]
fn relational_selectors_clear_every_cached_style() {
    for css in [
        "body:has(.c){color:red}",
        "aside:is(:has(div)){color:red}",
        "aside:not(:has(div)){color:red}",
        "@media screen{body:has(div){color:red}}",
        "@scope (body:has(.c)){p{color:red}}",
        "body{&:has(.c){color:red}}",
    ] {
        let fixture = fixture();
        let mut resolver = resolver(css);
        for node in fixture.nodes() {
            resolver.computed_style_shared(&node);
        }
        let observer = resolver.computed_style_shared(&fixture.observer);
        let (parents, moved) = mutate(&fixture, Mutation::Append);
        assert_eq!(
            resolver.invalidate_tree_mutation_subtrees(&parents, &[moved]),
            "fallback_tree_relational_selector",
            "{css}"
        );
        assert!(
            !Arc::ptr_eq(
                &observer,
                &resolver.computed_style_shared(&fixture.observer)
            ),
            "{css}"
        );
    }
}

#[test]
fn relational_reach_is_reclassified_after_a_stylesheet_is_added() {
    let fixture = fixture();
    let mut resolver = resolver("div{width:10px}");
    let (parents, moved) = mutate(&fixture, Mutation::Append);
    assert_eq!(
        resolver.invalidate_tree_mutation_subtrees(&parents, &[moved.clone()]),
        "local_tree_subtree"
    );
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("body:has(.c){color:red}").unwrap(),
    );
    assert_eq!(
        resolver.invalidate_tree_mutation_subtrees(&parents, &[moved]),
        "fallback_tree_relational_selector"
    );
}

#[test]
fn document_level_and_detached_parents_clear_every_cached_style() {
    let fixture = fixture();
    let mut resolver = resolver("div{width:10px}");
    let inserted = NodeHandle::element("div");
    fixture.body.append_child(inserted.clone());
    assert_eq!(
        resolver.invalidate_tree_mutation_subtrees(&[fixture.html.clone()], &[inserted.clone()]),
        "fallback_tree_root_or_namespace"
    );
    assert_eq!(
        resolver.invalidate_tree_mutation_subtrees(&[], &[inserted]),
        "fallback_tree_root_or_namespace"
    );
}
