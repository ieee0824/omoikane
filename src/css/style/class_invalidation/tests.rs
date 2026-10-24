use super::*;
use crate::css::parse_stylesheet;

fn fixture() -> (NodeHandle, NodeHandle, NodeHandle, NodeHandle) {
    let parent = NodeHandle::element("main");
    let target = NodeHandle::element("div");
    target.set_attribute("id", "target");
    target.set_attribute("class", "old");
    let child = NodeHandle::element("span");
    child.set_attribute("class", "child");
    target.append_child(child.clone());
    let observer = NodeHandle::element("aside");
    observer.set_attribute("class", "observer");
    parent.append_child(target.clone());
    parent.append_child(observer.clone());
    (parent, target, child, observer)
}

#[test]
fn class_scopes_match_a_fresh_cascade_including_pseudos_and_inheritance() {
    for (selector, local) in [
        ("#target.active", true),
        (".active .child", true),
        (".active > .child", true),
        (".active > .child + span", true),
        (":is(.active)", true),
        (":not(.active)", true),
        ("[class~=active]", true),
        ("[class~=active] + .observer", false),
        (".active + .observer", false),
        (".active ~ .observer", false),
        ("main:has(.active)", false),
        ("main:nth-child(1 of :has(.active))", false),
    ] {
        let (parent, target, child, observer) = fixture();
        let css = format!(
            "{selector}{{color:blue;width:37px}} {selector}::before{{content:'x';color:green}}"
        );
        let mut local_resolver = StyleResolver::new();
        local_resolver.add_stylesheet(Origin::Author, parse_stylesheet(&css).unwrap());
        let nodes = [&parent, &target, &child, &observer];
        for node in nodes {
            local_resolver.computed_style_shared(node);
            local_resolver.computed_pseudo_style_shared(node, PseudoElement::Before);
        }
        let observer_before = local_resolver.computed_style_shared(&observer);
        target.set_attribute("class", "active");
        let outcome =
            local_resolver.invalidate_class_subtree(&target, Some("old"), Some("active"), false);
        assert_eq!(outcome == "local_class_subtree", local, "{selector}");
        let mut fresh = StyleResolver::new();
        fresh.add_stylesheet(Origin::Author, parse_stylesheet(&css).unwrap());
        for node in nodes {
            assert_eq!(
                local_resolver.computed_style_shared(node).properties(),
                fresh.computed_style_shared(node).properties(),
                "{selector}"
            );
            assert_eq!(
                local_resolver
                    .computed_pseudo_style_shared(node, PseudoElement::Before)
                    .map(|s| s.properties()),
                fresh
                    .computed_pseudo_style_shared(node, PseudoElement::Before)
                    .map(|s| s.properties()),
                "{selector} pseudo"
            );
        }
        if local {
            assert!(
                Arc::ptr_eq(
                    &observer_before,
                    &local_resolver.computed_style_shared(&observer)
                ),
                "{selector}"
            );
        }
    }
}

#[test]
fn registering_a_stylesheet_discards_negative_class_reach_cache() {
    let (_parent, target, _child, observer) = fixture();
    let mut resolver = StyleResolver::new();
    assert_eq!(resolver.class_reach("active"), ClassReach::Absent);
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(".active + .observer{color:blue}").unwrap(),
    );
    resolver.computed_style_shared(&observer);
    target.set_attribute("class", "active");
    assert_eq!(
        resolver.invalidate_class_subtree(&target, Some("old"), Some("active"), false),
        "fallback_class_selector"
    );
    assert_eq!(
        resolver.computed_style_shared(&observer).get("color"),
        Some(&ComputedValue::Color("blue".into()))
    );
}

#[test]
fn changed_class_names_leave_unchanged_sibling_dependencies_cached() {
    let (_parent, target, _child, observer) = fixture();
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(".old + .observer{color:blue}.active{width:37px}").unwrap(),
    );
    let before = resolver.computed_style_shared(&observer);
    target.set_attribute("class", "old active");
    assert_eq!(
        resolver.invalidate_class_subtree(&target, Some("old"), Some("old active"), false),
        "local_class_subtree"
    );
    assert!(Arc::ptr_eq(
        &before,
        &resolver.computed_style_shared(&observer)
    ));
}

#[test]
fn class_scope_guards_roots_shadow_scope_and_nested_selectors() {
    for css in [
        "@scope (.active){.child{color:blue}}",
        ".active{& + .observer{color:blue}}",
        ":is(.active + .observer){color:blue}",
        ":nth-child(1 of .active){color:blue}",
        ":nth-child(1 of .\\61 ctive){color:blue}",
    ] {
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
        assert_eq!(
            resolver.class_reach("active"),
            ClassReach::NonLocal,
            "{css}"
        );
    }
    let document = NodeHandle::document();
    let root = NodeHandle::element("html");
    document.append_child(root.clone());
    let mut resolver = StyleResolver::new();
    assert_eq!(
        resolver.invalidate_class_subtree(&root, None, Some("active"), false),
        "fallback_class_root_or_namespace"
    );
    let shadow_host = NodeHandle::element("x-host");
    shadow_host
        .attach_shadow(crate::dom::ShadowRootMode::Open)
        .unwrap();
    assert_eq!(
        resolver.invalidate_class_subtree(&shadow_host, None, Some("active"), false),
        "fallback_class_shadow_or_slot"
    );
    let plain = NodeHandle::element("div");
    assert_eq!(
        resolver.invalidate_class_subtree(&plain, None, Some("active"), true),
        "fallback_class_non_scalar"
    );
}
