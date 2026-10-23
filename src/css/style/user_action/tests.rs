use super::*;
use crate::css::parse_stylesheet;

fn resolver(css: &str) -> StyleResolver {
    let mut resolver = StyleResolver::new();
    resolver.add_stylesheet(Origin::Author, parse_stylesheet(css).unwrap());
    resolver
}

#[test]
fn dynamic_predicates_only_observe_possible_compound_subjects() {
    let anchor = NodeHandle::element("a");
    let span = NodeHandle::element("span");
    for selector in [
        "a:hover",
        "a:hover span",
        "a:hover + aside",
        ":is(a:hover)",
        ":where(a:hover)",
        ":not(a:hover)",
        "main:has(> a:hover)",
        "a:hover::before",
    ] {
        let mut resolver = resolver(&format!("{selector}{{color:red}}"));
        assert!(
            !resolver.invalidate_user_action_selectors(&[(span.clone(), "hover")]),
            "{selector}"
        );
        assert!(
            resolver.invalidate_user_action_selectors(&[(anchor.clone(), "hover")]),
            "{selector}"
        );
        assert!(
            !resolver.invalidate_user_action_selectors(&[(anchor.clone(), "active")]),
            "{selector}"
        );
    }
}

#[test]
fn static_classes_ids_and_attributes_filter_dynamic_subjects() {
    let candidate = NodeHandle::element("div");
    let other = NodeHandle::element("div");
    candidate.set_attribute("id", "candidate");
    candidate.set_attribute("class", "interactive");
    candidate.set_attribute("data-action", "open");
    for selector in [
        ".interactive:hover",
        "#candidate:hover",
        "[data-action=open]:hover",
    ] {
        let mut resolver = resolver(&format!("{selector}{{color:red}}"));
        assert!(resolver.invalidate_user_action_selectors(&[(candidate.clone(), "hover")]));
        assert!(!resolver.invalidate_user_action_selectors(&[(other.clone(), "hover")]));
    }
}

#[test]
fn functional_scope_and_nested_observations_keep_conservative_invalidation() {
    let node = NodeHandle::element("span");
    for css in [
        "a:nth-child(2 of b:hover){color:red}",
        "@scope (a:hover){span{color:red}}",
        "div{&:hover{color:red}}",
        ":host(a:hover){color:red}",
        "::slotted(a:hover){color:red}",
    ] {
        let mut resolver = resolver(css);
        assert!(
            resolver.invalidate_user_action_selectors(&[(node.clone(), "hover")]),
            "{css}"
        );
    }
}

#[test]
fn all_focus_states_are_distinct_and_universal_subjects_are_observed() {
    let node = NodeHandle::element("button");
    let mut resolver = resolver("button:focus-within{color:red}button:focus-visible{width:9px}");
    assert!(!resolver.invalidate_user_action_selectors(&[(node.clone(), "hover")]));
    assert!(!resolver.invalidate_user_action_selectors(&[(node.clone(), "focus")]));
    assert!(resolver.invalidate_user_action_selectors(&[(node.clone(), "focus-within")]));
    assert!(resolver.invalidate_user_action_selectors(&[(node.clone(), "focus-visible")]));
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet(":hover{color:blue}").unwrap(),
    );
    assert!(resolver.invalidate_user_action_selectors(&[(node, "hover")]));
}

#[test]
fn stylesheet_registration_discards_a_previous_absence_proof() {
    let node = NodeHandle::element("span");
    let mut resolver = resolver("a:hover{color:red}");
    assert!(!resolver.invalidate_user_action_selectors(&[(node.clone(), "hover")]));
    resolver.add_stylesheet(
        Origin::Author,
        parse_stylesheet("span:hover{color:blue}").unwrap(),
    );
    assert!(resolver.invalidate_user_action_selectors(&[(node, "hover")]));
}

#[test]
fn tokens_distinguish_escaped_states_from_strings_and_attribute_names() {
    let anchor = NodeHandle::element("a");
    let mut escaped = resolver(r"a:\68 over{color:red}");
    assert!(escaped.invalidate_user_action_selectors(&[(anchor.clone(), "hover")]));
    for css in [
        "a:nth-child(2 of [data-name=':hover']){color:red}",
        "[data-hover]{color:red}",
        "a{--text:':hover'}",
    ] {
        let mut resolver = resolver(css);
        assert!(
            !resolver.invalidate_user_action_selectors(&[(anchor.clone(), "hover")]),
            "{css}"
        );
    }
}

#[test]
fn observer_limit_falls_back_to_full_reach_instead_of_truncating_it() {
    let mut observers = Observers::default();
    let selector = crate::css::parse_selector_list("a:hover")
        .unwrap()
        .remove(0);
    for _ in 0..=MAX_OBSERVERS {
        observers.selector(&selector, "hover", false);
    }
    assert!(observers.uncertain);
    assert!(observers.subjects.is_empty());
}
