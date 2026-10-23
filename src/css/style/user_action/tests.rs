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

#[test]
fn subtree_hover_recomputes_descendants_and_preserves_unrelated_styles() {
    let document = NodeHandle::document();
    let root = NodeHandle::element("main");
    let anchor = NodeHandle::element("a");
    let child = NodeHandle::element("span");
    let unrelated = NodeHandle::element("aside");
    document.append_child(root.clone());
    root.append_child(anchor.clone());
    root.append_child(unrelated.clone());
    anchor.append_child(child.clone());
    let mut resolver = resolver("a{color:blue}a:hover{color:red}a:hover span{width:17px}");
    let before_child = resolver.computed_style_shared(&child);
    let before_unrelated = resolver.computed_style_shared(&unrelated);
    anchor.set_user_action_state("hover", true);
    assert!(
        resolver
            .invalidate_user_action_styles(&[(anchor, "hover")])
            .changed()
    );
    let after_child = resolver.computed_style_shared(&child);
    let after_unrelated = resolver.computed_style_shared(&unrelated);
    assert!(!Arc::ptr_eq(&before_child, &after_child));
    assert!(Arc::ptr_eq(&before_unrelated, &after_unrelated));
    assert_eq!(after_child.get("width"), Some(&ComputedValue::Px(17.0)));
    assert_ne!(before_child.get("color"), after_child.get("color"));
}

#[test]
fn relational_hover_distinguishes_local_and_nonlocal_dependencies() {
    for (selector, full) in [
        ("a:hover + aside", true),
        ("main:has(a:hover)", true),
        (":is(a:hover)", false),
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let other = NodeHandle::element("aside");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        root.append_child(other.clone());
        let mut resolver = resolver(&format!("{selector}{{color:red}}"));
        let before = resolver.computed_style_shared(&other);
        anchor.set_user_action_state("hover", true);
        assert!(
            resolver
                .invalidate_user_action_styles(&[(anchor, "hover")])
                .changed()
        );
        let after = resolver.computed_style_shared(&other);
        assert_eq!(Arc::ptr_eq(&before, &after), !full, "{selector}");
    }
}

#[test]
fn cached_hover_updates_match_cold_cascade_in_both_directions() {
    for selector in [
        "a:hover",
        "a:hover span",
        "a:hover > span",
        "a:hover + aside",
        "a:hover ~ aside",
        "main:has(a:hover)",
        ":is(a:hover)",
        ":not(a:hover)",
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let child = NodeHandle::element("span");
        let aside = NodeHandle::element("aside");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        anchor.append_child(child.clone());
        root.append_child(aside.clone());
        let css = format!("a{{color:blue}}{selector}{{color:red;width:17px;--hover:yes}}");
        let nodes = [root, anchor.clone(), child, aside];
        let mut cached = resolver(&css);
        for node in &nodes {
            cached.computed_style_shared(node);
            cached.computed_pseudo_style_shared(node, PseudoElement::Before);
        }
        for enabled in [true, false] {
            anchor.set_user_action_state("hover", enabled);
            assert!(
                cached
                    .invalidate_user_action_styles(&[(anchor.clone(), "hover")])
                    .changed()
            );
            let mut cold = resolver(&css);
            for node in &nodes {
                assert_eq!(
                    cached.computed_style_shared(node),
                    cold.computed_style_shared(node),
                    "{selector}, hover={enabled}, tag={:?}",
                    node.tag_name()
                );
                assert_eq!(
                    cached.computed_pseudo_style_shared(node, PseudoElement::Before),
                    cold.computed_pseudo_style_shared(node, PseudoElement::Before),
                    "pseudo {selector}, hover={enabled}, tag={:?}",
                    node.tag_name()
                );
            }
        }
    }
}

#[test]
fn unrelated_nonlocal_selector_does_not_discard_local_hover_cache() {
    let document = NodeHandle::document();
    let root = NodeHandle::element("main");
    let anchor = NodeHandle::element("a");
    let other = NodeHandle::element("aside");
    document.append_child(root.clone());
    root.append_child(anchor.clone());
    root.append_child(other.clone());
    let mut resolver = resolver("a:hover{color:red}button:hover + label{color:blue}");
    let before = resolver.computed_style_shared(&other);
    anchor.set_user_action_state("hover", true);
    assert!(
        resolver
            .invalidate_user_action_styles(&[(anchor, "hover")])
            .changed()
    );
    let after = resolver.computed_style_shared(&other);
    assert!(Arc::ptr_eq(&before, &after));
}

#[test]
fn hover_invalidation_refreshes_existing_pseudo_inherited_color() {
    let document = NodeHandle::document();
    let root = NodeHandle::element("main");
    let anchor = NodeHandle::element("a");
    document.append_child(root.clone());
    root.append_child(anchor.clone());
    let css = "a{color:blue}a:hover{color:red}a::before{content:'marker'}";
    let mut cached = resolver(css);
    let before = cached
        .computed_pseudo_style_shared(&anchor, PseudoElement::Before)
        .unwrap();
    assert_eq!(
        before.get("content"),
        Some(&ComputedValue::String("marker".into()))
    );
    anchor.set_user_action_state("hover", true);
    assert!(
        cached
            .invalidate_user_action_styles(&[(anchor.clone(), "hover")])
            .changed()
    );
    let after = cached
        .computed_pseudo_style_shared(&anchor, PseudoElement::Before)
        .unwrap();
    let expected = resolver(css)
        .computed_pseudo_style_shared(&anchor, PseudoElement::Before)
        .unwrap();
    assert_ne!(before.get("color"), after.get("color"));
    assert_eq!(
        after.get("content"),
        Some(&ComputedValue::String("marker".into()))
    );
    assert_eq!(after, expected);
}

#[test]
fn root_hover_refreshes_shared_font_units_with_full_invalidation() {
    let document = NodeHandle::document();
    let root = NodeHandle::element("html");
    let child = NodeHandle::element("div");
    document.append_child(root.clone());
    root.append_child(child.clone());
    let css = "html{font-size:16px}html:hover{font-size:32px}div{width:2rem}";
    let mut cached = resolver(css);
    let before = cached.computed_style_shared(&child);
    assert_eq!(before.get("width"), Some(&ComputedValue::Px(32.0)));
    root.set_user_action_state("hover", true);
    assert_eq!(
        cached.invalidate_user_action_styles(&[(root, "hover")]),
        UserActionStyleInvalidation::Full("fallback_document_root")
    );
    let after = cached.computed_style_shared(&child);
    assert_eq!(after.get("width"), Some(&ComputedValue::Px(64.0)));
    assert_eq!(after, resolver(css).computed_style_shared(&child));
}

#[test]
fn shadow_boundaries_keep_full_invalidation() {
    use crate::dom::ShadowRootMode;
    for inside in [false, true] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let section = NodeHandle::element("section");
        let host = NodeHandle::element("div");
        let other = NodeHandle::element("aside");
        document.append_child(root.clone());
        root.append_child(section.clone());
        root.append_child(other.clone());
        section.append_child(host.clone());
        let shadow = host.attach_shadow(ShadowRootMode::Open).unwrap();
        let inner = NodeHandle::element("span");
        shadow.append_child(inner.clone());
        let mut cached = resolver("section:hover,span:hover{color:red}");
        let before = cached.computed_style_shared(&other);
        let target = if inside { inner } else { section };
        target.set_user_action_state("hover", true);
        assert_eq!(
            cached.invalidate_user_action_styles(&[(target, "hover")]),
            UserActionStyleInvalidation::Full("fallback_shadow_or_slot")
        );
        assert!(!Arc::ptr_eq(&before, &cached.computed_style_shared(&other)));
    }
}

#[test]
fn scoped_and_nested_hover_keep_uncertain_selector_fallback() {
    for css in [
        "@scope (.scope:hover){a{color:red}}",
        ".scope:hover{& + aside{color:red}}",
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        anchor.set_attribute("class", "scope");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        let mut cached = resolver(css);
        cached.computed_style_shared(&anchor);
        anchor.set_user_action_state("hover", true);
        assert_eq!(
            cached.invalidate_user_action_styles(&[(anchor.clone(), "hover")]),
            UserActionStyleInvalidation::Full("fallback_uncertain_selector"),
            "{css}"
        );
        assert_eq!(
            cached.computed_style_shared(&anchor),
            resolver(css).computed_style_shared(&anchor)
        );
    }
}

#[test]
fn hover_uses_full_invalidation_during_an_unrelated_transition() {
    let document = NodeHandle::document();
    let root = NodeHandle::element("main");
    let animated = NodeHandle::element("div");
    let anchor = NodeHandle::element("a");
    document.append_child(root.clone());
    root.append_child(animated.clone());
    root.append_child(anchor.clone());
    animated.set_attribute("style", "color:red;transition:color 1s linear");
    let mut cached = resolver("a:hover{color:green}");
    cached.computed_style_shared(&animated);
    animated.set_attribute("style", "color:blue;transition:color 1s linear");
    cached.invalidate_style_cache();
    cached.computed_style_shared(&animated);
    assert!(cached.has_running_transitions());
    anchor.set_user_action_state("hover", true);
    assert_eq!(
        cached.invalidate_user_action_styles(&[(anchor, "hover")]),
        UserActionStyleInvalidation::Full("fallback_active_transition")
    );
}

#[test]
fn disjoint_paint_animation_keeps_local_hover_and_continues_sampling() {
    for (delay, times) in [
        (0, [0.0, 250.0, 500.0, 750.0, 1250.0]),
        (5, [0.0, 4500.0, 5250.0, 5500.0, 6250.0]),
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let animated = NodeHandle::element("div");
        let unrelated = NodeHandle::element("aside");
        animated.set_attribute("id", "animated");
        document.append_child(root.clone());
        for node in [&anchor, &animated, &unrelated] {
            root.append_child(node.clone());
        }
        let css = format!(
            "@keyframes fade{{from{{opacity:0.1}}to{{opacity:0.9}}}}            #animated{{animation:fade 1s linear {delay}s}}            #animated::before{{content:'x';animation:fade 1s linear {delay}s}}            a{{color:blue}}a:hover{{color:green}}aside{{width:15px}}"
        );
        let mut cached = resolver(&css);
        let mut full = resolver(&css);
        for resolver in [&mut cached, &mut full] {
            resolver.set_animation_time_ms(0.0);
            for node in [&anchor, &animated, &unrelated] {
                resolver.computed_style_shared(node);
                resolver.computed_pseudo_style_shared(node, PseudoElement::Before);
            }
        }
        let mut opacities = Vec::new();
        for (index, time) in times.into_iter().enumerate() {
            cached.set_animation_time_ms(time);
            full.set_animation_time_ms(time);
            let retained = cached.computed_style_shared(&unrelated);
            anchor.set_user_action_state("hover", index % 2 == 0);
            assert_eq!(
                cached.invalidate_user_action_styles(&[(anchor.clone(), "hover")]),
                UserActionStyleInvalidation::Local,
                "delay={delay} time={time}"
            );
            full.invalidate_style_cache();
            for node in [&anchor, &animated, &unrelated] {
                assert_eq!(
                    cached.computed_style_shared(node),
                    full.computed_style_shared(node)
                );
                assert_eq!(
                    cached.computed_pseudo_style_shared(node, PseudoElement::Before),
                    full.computed_pseudo_style_shared(node, PseudoElement::Before)
                );
            }
            assert!(Arc::ptr_eq(
                &retained,
                &cached.computed_style_shared(&unrelated)
            ));
            opacities.push(
                cached
                    .computed_style_shared(&animated)
                    .get("opacity")
                    .cloned(),
            );
        }
        assert!(
            opacities.windows(2).any(|pair| pair[0] != pair[1]),
            "animation must advance"
        );
    }
}

#[test]
fn animated_branch_dependencies_keep_full_user_action_invalidation() {
    for (placement, property, from, to) in [
        ("unrelated", "width", "10px", "30px"),
        ("ancestor", "opacity", "0.1", "0.9"),
        ("descendant", "opacity", "0.1", "0.9"),
        ("self", "opacity", "0.1", "0.9"),
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let other = NodeHandle::element("div");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        let animated = match placement {
            "ancestor" => root.clone(),
            "self" => anchor.clone(),
            "descendant" => {
                anchor.append_child(other.clone());
                other.clone()
            }
            _ => {
                root.append_child(other.clone());
                other.clone()
            }
        };
        animated.set_attribute("id", "animated");
        let css = format!(
            "@keyframes run{{from{{{property}:{from}}}to{{{property}:{to}}}}}            #animated{{animation:run 1s linear}}a:hover{{color:green}}"
        );
        let mut cached = resolver(&css);
        cached.set_animation_time_ms(0.0);
        cached.computed_style_shared(&animated);
        assert!(cached.has_running_animations());
        anchor.set_user_action_state("hover", true);
        assert_eq!(
            cached.invalidate_user_action_styles(&[(anchor, "hover")]),
            UserActionStyleInvalidation::Full("fallback_active_animation"),
            "{placement}"
        );
    }
}

#[test]
fn static_functional_predicates_keep_outer_hover_subtree_local() {
    for selector in [
        "a:not(.disabled):hover",
        "a:is(.enabled):hover",
        "a:where(.enabled):hover",
        "a:has(span):hover",
        "a:has(+ aside):hover",
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        anchor.set_attribute("class", "enabled");
        let child = NodeHandle::element("span");
        let aside = NodeHandle::element("aside");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        anchor.append_child(child.clone());
        root.append_child(aside.clone());
        let css = format!("a{{color:blue}}{selector}{{color:red;width:17px}}");
        let nodes = [root, anchor.clone(), child, aside.clone()];
        let mut cached = resolver(&css);
        for node in &nodes {
            cached.computed_style_shared(node);
        }
        for enabled in [true, false] {
            let before_aside = cached.computed_style_shared(&aside);
            anchor.set_user_action_state("hover", enabled);
            assert_eq!(
                cached.invalidate_user_action_styles(&[(anchor.clone(), "hover")]),
                UserActionStyleInvalidation::Local,
                "{selector}"
            );
            assert!(Arc::ptr_eq(
                &before_aside,
                &cached.computed_style_shared(&aside)
            ));
            let mut cold = resolver(&css);
            for node in &nodes {
                assert_eq!(
                    cached.computed_style_shared(node),
                    cold.computed_style_shared(node),
                    "{selector}, hover={enabled}, tag={:?}",
                    node.tag_name()
                );
            }
            if enabled {
                assert_eq!(
                    cached.computed_style_shared(&anchor).get("width"),
                    Some(&ComputedValue::Px(17.0)),
                    "{selector}"
                );
            }
        }
    }
}

#[test]
fn boolean_functional_hover_keeps_local_caches_and_matches_cold_cascade() {
    for selector in [
        ":is(a:hover)",
        ":where(a:hover)",
        ":not(a:hover)",
        ":is(main a:hover)",
        ":is(a:hover span)",
        ":not(a:hover span)",
        ":is(:where(a:hover))",
        ":not(:not(a:hover))",
        "main :where(a:hover) span",
        "a:is(:hover,:active)",
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let child = NodeHandle::element("span");
        let aside = NodeHandle::element("aside");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        anchor.append_child(child.clone());
        root.append_child(aside.clone());
        let nodes = [root, anchor.clone(), child, aside.clone()];
        let css = format!(
            "*{{width:3px;color:blue}}{selector}{{width:17px;color:red}}a::before{{content:'marker'}}"
        );
        let mut cached = resolver(&css);
        let original = nodes
            .iter()
            .map(|node| cached.computed_style_shared(node))
            .collect::<Vec<_>>();
        for node in &nodes {
            cached.computed_pseudo_style_shared(node, PseudoElement::Before);
        }
        for enabled in [true, false] {
            let unrelated = cached.computed_style_shared(&aside);
            anchor.set_user_action_state("hover", enabled);
            let outcome = cached.invalidate_user_action_styles(&[(anchor.clone(), "hover")]);
            let mut cold = resolver(&css);
            let mut changed = false;
            for (node, original) in nodes.iter().zip(&original) {
                let actual = cached.computed_style_shared(node);
                assert_eq!(
                    actual,
                    cold.computed_style_shared(node),
                    "{selector}, hover={enabled}"
                );
                changed |= actual != *original;
                assert_eq!(
                    cached.computed_pseudo_style_shared(node, PseudoElement::Before),
                    cold.computed_pseudo_style_shared(node, PseudoElement::Before),
                    "pseudo {selector}, hover={enabled}"
                );
            }
            assert_eq!(
                changed, enabled,
                "{selector}: fixture must visibly change then restore"
            );
            assert_eq!(
                cached
                    .computed_pseudo_style_shared(&anchor, PseudoElement::Before)
                    .unwrap()
                    .get("content"),
                Some(&ComputedValue::String("marker".into()))
            );
            assert_eq!(outcome, UserActionStyleInvalidation::Local, "{selector}");
            assert!(
                Arc::ptr_eq(&unrelated, &cached.computed_style_shared(&aside)),
                "{selector}"
            );
        }
    }
}

#[test]
fn functional_sibling_and_has_arguments_keep_full_invalidation_and_cold_results() {
    for selector in [
        ":is(a:hover + aside)",
        ":where(a:hover ~ aside)",
        ":not(a:hover + aside)",
        ":is(main:has(a:hover))",
        "main:not(:has(a:hover))",
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let aside = NodeHandle::element("aside");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        root.append_child(aside.clone());
        let nodes = [root, anchor.clone(), aside.clone()];
        let css = format!("*{{width:3px}}{selector}{{width:17px;color:red}}");
        let mut cached = resolver(&css);
        let original = nodes
            .iter()
            .map(|node| cached.computed_style_shared(node))
            .collect::<Vec<_>>();
        for enabled in [true, false] {
            let before_aside = cached.computed_style_shared(&aside);
            anchor.set_user_action_state("hover", enabled);
            assert_eq!(
                cached.invalidate_user_action_styles(&[(anchor.clone(), "hover")]),
                UserActionStyleInvalidation::Full("fallback_nonlocal_selector"),
                "{selector}"
            );
            assert!(
                !Arc::ptr_eq(&before_aside, &cached.computed_style_shared(&aside)),
                "{selector}"
            );
            let mut cold = resolver(&css);
            let mut changed = false;
            for (node, original) in nodes.iter().zip(&original) {
                let actual = cached.computed_style_shared(node);
                assert_eq!(
                    actual,
                    cold.computed_style_shared(node),
                    "{selector}, hover={enabled}"
                );
                changed |= actual != *original;
            }
            assert_eq!(
                changed, enabled,
                "{selector}: fixture must visibly change then restore"
            );
        }
    }
}

#[test]
fn contained_sibling_hover_keeps_local_caches_and_matches_cold_cascade() {
    for selector in [
        "div + a:hover",
        "div + :is(a:hover)",
        ":where(div + a:hover)",
        "a:hover > span + em",
        "a:hover span ~ em",
        ":is(a:hover > span) + em",
        ":not(a:hover > span) + em",
        ":where(:is(a:hover > span)) + em",
        "div + a:hover > span + em",
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let child = NodeHandle::element("span");
        let aside = NodeHandle::element("aside");
        let prefix = NodeHandle::element("div");
        let sibling = NodeHandle::element("em");
        document.append_child(root.clone());
        root.append_child(prefix.clone());
        root.append_child(anchor.clone());
        anchor.append_child(child.clone());
        anchor.append_child(sibling.clone());
        root.append_child(aside.clone());
        let nodes = [root, prefix, anchor.clone(), child, sibling, aside.clone()];
        let css = format!(
            "*{{width:3px;color:blue}}{selector}{{width:17px;color:red}}*::before{{content:'marker'}}"
        );
        let mut cached = resolver(&css);
        let original = nodes
            .iter()
            .map(|node| cached.computed_style_shared(node))
            .collect::<Vec<_>>();
        for node in &nodes {
            cached.computed_pseudo_style_shared(node, PseudoElement::Before);
        }
        for enabled in [true, false] {
            let unrelated = cached.computed_style_shared(&aside);
            anchor.set_user_action_state("hover", enabled);
            let outcome = cached.invalidate_user_action_styles(&[(anchor.clone(), "hover")]);
            let mut cold = resolver(&css);
            let mut changed = false;
            for (node, original) in nodes.iter().zip(&original) {
                let actual = cached.computed_style_shared(node);
                assert_eq!(
                    actual,
                    cold.computed_style_shared(node),
                    "{selector}, hover={enabled}"
                );
                changed |= actual != *original;
                assert_eq!(
                    cached.computed_pseudo_style_shared(node, PseudoElement::Before),
                    cold.computed_pseudo_style_shared(node, PseudoElement::Before),
                    "pseudo {selector}, hover={enabled}"
                );
            }
            assert_eq!(
                changed, enabled,
                "{selector}: fixture must visibly change then restore"
            );
            assert_eq!(
                cached
                    .computed_pseudo_style_shared(&anchor, PseudoElement::Before)
                    .unwrap()
                    .get("content"),
                Some(&ComputedValue::String("marker".into()))
            );
            assert_eq!(outcome, UserActionStyleInvalidation::Local, "{selector}");
            assert!(
                Arc::ptr_eq(&unrelated, &cached.computed_style_shared(&aside)),
                "{selector}"
            );
        }
    }
}

#[test]
fn nested_siblings_cannot_become_local_after_later_descendant_steps() {
    for selector in [
        ":is(a:hover + aside) > span",
        ":is(a:hover, a:hover > span) + aside",
        ":not(a:hover) + aside",
        ":where(a:hover ~ aside) > span",
    ] {
        let document = NodeHandle::document();
        let root = NodeHandle::element("main");
        let anchor = NodeHandle::element("a");
        let aside = NodeHandle::element("aside");
        let child = NodeHandle::element("span");
        document.append_child(root.clone());
        root.append_child(anchor.clone());
        root.append_child(aside.clone());
        aside.append_child(child.clone());
        let nodes = [root, anchor.clone(), aside.clone(), child];
        let css = format!("*{{width:3px}}{selector}{{width:17px;color:red}}");
        let mut cached = resolver(&css);
        let original = nodes
            .iter()
            .map(|node| cached.computed_style_shared(node))
            .collect::<Vec<_>>();
        for enabled in [true, false] {
            let before_aside = cached.computed_style_shared(&aside);
            anchor.set_user_action_state("hover", enabled);
            assert_eq!(
                cached.invalidate_user_action_styles(&[(anchor.clone(), "hover")]),
                UserActionStyleInvalidation::Full("fallback_nonlocal_selector"),
                "{selector}"
            );
            assert!(
                !Arc::ptr_eq(&before_aside, &cached.computed_style_shared(&aside)),
                "{selector}"
            );
            let mut cold = resolver(&css);
            let mut changed = false;
            for (node, original) in nodes.iter().zip(&original) {
                let actual = cached.computed_style_shared(node);
                assert_eq!(
                    actual,
                    cold.computed_style_shared(node),
                    "{selector}, hover={enabled}"
                );
                changed |= actual != *original;
            }
            assert_eq!(
                changed, enabled,
                "{selector}: fixture must visibly change then restore"
            );
        }
    }
}
